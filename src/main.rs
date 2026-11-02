// M1 skeleton: nested (winit) compositor session with the shell protocol floor.
// Based on Smithay 0.7's examples/minimal.rs; everything not needed at M1 is left out.
use std::{os::unix::io::OwnedFd, sync::Arc};

use ::winit::platform::pump_events::PumpStatus;
mod ipc;
mod lua_state;
use ipc::{CompCmd, Ipc};
use lua_state::{init, register_api, _do_file, _do_chunk};
use mlua::Lua;

use smithay::{
    backend::{
        renderer::{
            element::{
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind,
            },
            gles::GlesRenderer,
            utils::{draw_render_elements, on_commit_buffer_handler},
            Color32F, Frame, Renderer,
        },
        winit::{self, WinitEvent},
        input::{InputEvent, KeyboardKeyEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output, delegate_seat,
    delegate_shm, delegate_xdg_shell,
    input::{keyboard::FilterResult, Seat, SeatHandler, SeatState},
    reexports::wayland_server::{protocol::wl_seat, Display},
    utils::{Rectangle, Serial, Transform},
    wayland::{
        output::{OutputHandler, OutputManagerState},
        buffer::BufferHandler,
        compositor::{
            with_surface_tree_downward, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction,
        },
        selection::{
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
            SelectionHandler,
        },
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        shell::wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState},
        shm::{ShmHandler, ShmState},
    },
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::{
        wl_buffer,
        wl_output::WlOutput,
        wl_surface::{self, WlSurface},
    },
    Client, ListeningSocket,
};

impl OutputHandler for State {}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}
}

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<WlOutput>,
        _layer: Layer,
        _namespace: String,
    ) {
        // shortcuts: no layout/anchors/keyboard-interactivity yet — default-mapped
        // full-window so layer surfaces merely draw; M1 only proves the shell can appear.
        surface.send_configure();
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {}
}

struct State {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    layer_shell_state: WlrLayerShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    output_state: OutputManagerState,
    data_device_state: DataDeviceState,

    seat: Seat<Self>,
    output: Output,
    // M2: IPC-backed state; snapshot is rebuilt from the shell state each loop iteration
    ipc: Option<Ipc>,
    // M3: Lua state with Hyprland config API (es.bind, es.define_submap, es.dsp.*, es.config{}, etc.)
    lua: Option<mlua::Lua>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    run_winit()
}

fn socket_name() -> String {
    std::env::var("TACIT_COMP_SOCKET").unwrap_or_else(|_| "eswm-test".into())
}

pub fn run_winit() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<State> = Display::new()?;
    let dh = display.handle();

    let compositor_state = CompositorState::new::<State>(&dh);
    let shm_state = ShmState::new::<State>(&dh, vec![]);
    let layer_shell_state = WlrLayerShellState::new::<State>(&dh);
    let mut seat_state = SeatState::new();
    let seat = seat_state.new_wl_seat(&dh, "winit");

    let mut state = {
        // The nested winit window masquerades as one virtual output; clients
        // (and later IPC) enumerate it like a real monitor.
        let output = Output::new(
            "winit".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "eswm".into(),
                model: "nested-winit".into(),
            },
        );
        let mode = Mode { size: (1280, 800).into(), refresh: 60_000 };
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        output.set_preferred(mode);
        let _output_global = output.create_global::<State>(&dh);

        State {
            compositor_state,
            xdg_shell_state: XdgShellState::new::<State>(&dh),
            layer_shell_state,
            shm_state,
            seat_state,
            output_state: OutputManagerState::new(),
            data_device_state: DataDeviceState::new::<State>(&dh),
            seat,
            output,
            ipc: None,
            // M3: Lua state placeholder; initialized after IPC setup below
            lua: None,
        }
    };

    let socket = socket_name();
    let listener = ListeningSocket::bind(&socket).unwrap();
    println!("listening on {socket}");

    // M2: hyprctl IPC. Signature env controls the hyprdir path; a distinct
    // value keeps us away from a real Hyprland session's sockets.
    let sig = std::env::var("TACIT_COMP_SIG").unwrap_or_else(|_| "eswm".into());
    state.ipc = Some(Ipc::new(sig));

    // M3: initialize Lua embed with Hyprland config API
    let lua = init().unwrap_or_else(|_| {
        // Fallback: create a bare Lua state without API registration
        let lua = Lua::new();
        lua
    });
    let snapshot = state.ipc.as_ref().unwrap().snapshot.clone();
    let cmd_tx = state.ipc.as_ref().unwrap().cmd_tx.clone();
    // Register the Hyprland config API (es.bind, es.define_submap, es.dsp.*, es.config{}, is_file_exists, safe_require)
    if let Err(e) = register_api(&lua, snapshot, &cmd_tx) {
        tracing::warn!("Lua API registration failed (mlua/lua5.1-dev may be missing): {e}");
    }
    // Store lua in state for _do_file/_do_chunk access; kept alive for the session
    state.lua = Some(lua);

    let mut clients = Vec::new();

    let (mut backend, mut winit) = winit::init::<GlesRenderer>()?;

    let start_time = std::time::Instant::now();

    let keyboard = state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

    loop {
        let status = winit.dispatch_new_events(|event| match event {
            WinitEvent::Resized { .. } => {}
            WinitEvent::Input(event) => match event {
                InputEvent::Keyboard { event } => {
                    keyboard.input::<(), _>(
                        &mut state,
                        event.key_code(),
                        event.state(),
                        0.into(),
                        0,
                        |_, _, _| FilterResult::Forward,
                    );
                }
                InputEvent::PointerMotionAbsolute { .. } => {
                    if let Some(surface) = state.xdg_shell_state.toplevel_surfaces().iter().next().cloned() {
                        let surface = surface.wl_surface().clone();
                        keyboard.set_focus(&mut state, Some(surface), 0.into());
                    };
                }
                _ => {}
            },
            _ => (),
        });

        match status {
            PumpStatus::Continue => (),
            PumpStatus::Exit(_) => return Ok(()),
        };

        let size = backend.window_size();
        let damage = Rectangle::from_size(size);

        // M2: drain IPC commands (reload -> configreloaded event checked below)
        if let Some(ipc) = &state.ipc {
            while let Ok(cmd) = ipc.cmd_rx.try_recv() {
                match cmd {
                    CompCmd::Reload => ipc.sinks.emit("configreloaded", ""),
                    CompCmd::GotoWorkspace(_) => {}    // real dispatch arrives with M5 tiling
                    CompCmd::CloseWindow(_) => {}      // ditto
                    CompCmd::Unsupported(_) => {}
                }
            }
        }

        ipc::refresh_snapshot(&state, size.into());

        {
            let (renderer, mut framebuffer) = backend.bind().unwrap();
            let toplevel_elements = state
                .xdg_shell_state
                .toplevel_surfaces()
                .iter()
                .flat_map(|surface| {
                    render_elements_from_surface_tree(
                        renderer,
                        surface.wl_surface(),
                        (0, 0),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    )
                })
                .collect::<Vec<WaylandSurfaceRenderElement<GlesRenderer>>>();
            let layer_elements = state
                .layer_shell_state
                .layer_surfaces()
                .collect::<Vec<_>>()
                .into_iter()
                .flat_map(|surface| {
                    render_elements_from_surface_tree(
                        renderer,
                        surface.wl_surface(),
                        (0, 0),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    )
                })
                .collect::<Vec<WaylandSurfaceRenderElement<GlesRenderer>>>();

            let mut frame = renderer
                .render(&mut framebuffer, size, Transform::Flipped180)
                .unwrap();
            frame.clear(Color32F::new(0.1, 0.0, 0.0, 1.0), &[damage]).unwrap();
            draw_render_elements(&mut frame, 1.0, &toplevel_elements, &[damage]).unwrap();
            draw_render_elements(&mut frame, 1.0, &layer_elements, &[damage]).unwrap();
            // We rely on the nested compositor to do the sync for us
            let _ = frame.finish().unwrap();

            for surface in state.xdg_shell_state.toplevel_surfaces() {
                send_frames_surface_tree(surface.wl_surface(), start_time.elapsed().as_millis() as u32);
            }
            for surface in state.layer_shell_state.layer_surfaces() {
                send_frames_surface_tree(surface.wl_surface(), start_time.elapsed().as_millis() as u32);
            }

            if let Some(stream) = listener.accept()? {
                println!("Got a client: {:?}", stream);
                let client = display
                    .handle()
                    .insert_client(stream, Arc::new(ClientState::default()))
                    .unwrap();
                clients.push(client);
            }

            display.dispatch_clients(&mut state)?;
            display.flush_clients()?;
        }

        // It is important that all events on the display have been dispatched and flushed to clients before
        // swapping buffers because this operation may block.
        backend.submit(Some(&[damage])).unwrap();
    }
}

pub fn send_frames_surface_tree(surface: &wl_surface::WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            // the surface may not have any user_data if it is a subsurface and has not
            // yet been commited
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

// Macros used to delegate protocol handling to types in the app state.
delegate_xdg_shell!(State);
delegate_layer_shell!(State);
delegate_output!(State);
delegate_compositor!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_data_device!(State);
