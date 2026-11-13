// M1 skeleton: nested (winit) compositor session with the shell protocol floor.
// Based on Smithay 0.7's examples/minimal.rs; everything not needed at M1 is left out.
// M4: adds the real DRM/libinput backend (src/udev.rs), the default runtime.
use std::{os::unix::io::OwnedFd, sync::Arc};

use ::winit::platform::pump_events::PumpStatus;
mod control;
mod ipc;
mod lua_state;
mod udev;
mod wm;
use ipc::{CompCmd, Ipc};
use lua_state::{init, register_api, _do_file, _do_chunk};
use mlua::Lua;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind, RenderElementStates,
            },
            gles::GlesRenderer,
            utils::{draw_render_elements, on_commit_buffer_handler},
            Color32F, Frame, Renderer,
        },
        winit::{self, WinitEvent},
        input::{InputEvent, KeyboardKeyEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    desktop::{layer_map_for_output, LayerMap, WindowSurfaceType},
    utils::IsAlive,
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output, delegate_seat,
    delegate_shm, delegate_viewporter, delegate_xdg_shell,
    input::{keyboard::FilterResult, Seat, SeatHandler, SeatState},
        reexports::{
        calloop::LoopHandle,
        wayland_server::{protocol::wl_seat, Display, DisplayHandle, ListeningSocket},
    },
    utils::{Point, Logical, Rectangle, Serial, Transform},
    wayland::{
        output::{OutputHandler, OutputManagerState},
        buffer::BufferHandler,
        compositor::{
            with_surface_tree_downward, with_states, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction,
        },
        selection::{
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler},
            SelectionHandler,
        },
        shell::xdg::{PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData},
        shell::wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState},
        shm::{ShmHandler, ShmState},
        viewporter::ViewporterState,
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
    Client,
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
        // M5: every toplevel becomes a tiling-managed window on the primary output.
        let Some(output_id) = self.wm.primary_output_id() else {
            tracing::warn!("toplevel before any output exists; not registering");
            return;
        };
        if let Some(id) = self.wm.add_window(output_id, "", "") {
            let ws = surface.wl_surface().clone();
            with_states(&ws, |states| {
                states.data_map.insert_if_missing(|| id);
            });
            tracing::info!("toplevel registered id={id:?}");
            crate::apply_layout(self);
        } else {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Activated);
            });
            surface.send_configure();
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        sync_window_meta(self, &surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        sync_window_meta(self, &surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = window_id_of(surface.wl_surface()) {
            self.wm.remove_window(id);
            crate::apply_layout(self);
        }
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {}
}

pub fn window_id_of(surface: &WlSurface) -> Option<wm::WindowId> {
    with_states(surface, |states| {
        states.data_map.get::<wm::WindowId>().copied()
    })
}

// M5 placement, shared by the udev and winit render loops: active-workspace,
// mapped windows at their wm target_rect.
// (surface, x, y, id, focused, rect)
pub fn placed_windows(state: &mut State) -> Vec<(WlSurface, i32, i32, wm::WindowId, bool, wm::Rect)> {
    crate::apply_layout(state);
    state
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .filter_map(|surface| {
            let id = window_id_of(surface.wl_surface())?;
            let window = state.wm.window(id)?;
            let active = state
                .wm
                .output(window.output)
                .map(|o| o.active_workspace == window.workspace)
                .unwrap_or(false);
            if !active || !window.mapped || window.target_rect.width == 0 {
                tracing::info!(
                    "skip render surface id={id:?} active={active} mapped={} width={}",
                    window.mapped,
                    window.target_rect.width
                );
                return None;
            }
            // Clone window fields before mutable borrow for set_xdg_offset
            let target_rect = window.target_rect;
            let focused = window.focused;
            let (mut x, mut y) = (target_rect.x, target_rect.y);
            // Clients (kitty, GTK/zen) set xdg window geometry with an offset into
            // their own buffer (shadows/padding); align the buffer so the logical
            // window lands on the tiling rect instead of shifted by that offset.
            if let Some(off) = with_states(surface.wl_surface(), |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
                    .map(|geo| geo.loc)
            }) {
                x -= off.x;
                y -= off.y;
                state.wm.set_xdg_offset(id, (off.x, off.y));
            }
            tracing::info!("render window id={id:?} rect={:?} focused={}", target_rect, focused);
            Some((
                surface.wl_surface().clone(),
                x,
                y,
                id,
                focused,
                target_rect,
            ))
        })
        .collect()
}

fn toplevel_for_window(state: &State, id: wm::WindowId) -> Option<ToplevelSurface> {
    state
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .find(|t| t.alive() && window_id_of(t.wl_surface()) == Some(id))
        .cloned()
}

// Mirror client-provided app_id/title into the wm window record (snapshot/Lua).
fn sync_window_meta(state: &mut State, surface: &ToplevelSurface) {
    let Some(id) = window_id_of(surface.wl_surface()) else {
        return;
    };
    let mut app_id = String::new();
    let mut title = String::new();
    with_states(surface.wl_surface(), |states| {
        let data = states.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap();
        app_id = data.app_id.clone().unwrap_or_default();
        title = data.title.clone().unwrap_or_default();
    });
    state.wm.set_window_meta(id, app_id, title);
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
        namespace: String,
    ) {
        // Create the stable desktop wrapper; on first commit it gets mapped
        // into the output's LayerMap, which sends the real anchored configure.
        self.layer_wrappers.push(smithay::desktop::LayerSurface::new(surface, namespace));
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let wrapper = self.layer_wrappers.iter().find(|l| l.layer_surface() == &surface).cloned();
        self.layer_wrappers.retain(|l| l.layer_surface() != &surface);
        if let (Some(output), Some(wrapper)) = (self.primary_output.clone(), wrapper) {
            layer_map_for_output(&output).unmap_layer(&wrapper);
        }
        // the layer surface may have held keyboard focus (fuzzel); restore it
        crate::udev::keyboard_focus_set(self, self.wm.focused_id());
    }
}

impl State {
    /// Map unmapped layer surfaces into the primary output's LayerMap
    /// (sends proper anchored configures), re-arrange, and hand keyboard focus
    /// to a layer surface that requests exclusive keyboard interactivity
    /// (fuzzel, etc.). Called on every surface commit.
    fn map_layer_surfaces(&mut self) {
        let Some(output) = self.primary_output.clone() else { return };
        self.layer_wrappers.retain(|l| l.alive());
        let mut map = layer_map_for_output(&output);
        for layer in &self.layer_wrappers {
            let already =
                map.layer_for_surface(layer.wl_surface(), WindowSurfaceType::ALL).is_some();
            if !already {
                let _ = map.map_layer(layer);
            }
        }
        map.arrange();
        // Insent tiled windows from exclusive layer-surface zones (waybar, panels).
        let work_area = map.non_exclusive_zone();
        let wa = wm::Rect {
            x: work_area.loc.x,
            y: work_area.loc.y,
            width: work_area.size.w.max(0) as u32,
            height: work_area.size.h.max(0) as u32,
        };
        if self.wm.work_area != Some(wa) {
            self.wm.work_area = Some(wa);
            if let Some(output_id) = self.wm.primary_output_id() {
                self.wm.arrange_output(output_id);
            }
        }
        // smithay's arrange never sends the initial configure (the spec reserves
        // it for the compositor, on the surface's first commit) — send any
        // pending configure or layer clients (fuzzel, awww) wait forever.
        for layer in &self.layer_wrappers {
            layer.layer_surface().send_pending_configure();
        }
        if let Some(layer) = self.layer_wrappers.iter().find(|l| l.can_receive_keyboard_focus()) {
            let surf = layer.wl_surface().clone();
            drop(map);
            let keyboard = self.seat.get_keyboard().unwrap();
            keyboard.set_focus(self, Some(surf), crate::udev::SCOUNTER.next_serial());
            tracing::info!("keyboard focus -> layer surface (exclusive)");
        }
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
        self.map_layer_surfaces();
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
    viewporter_state: ViewporterState,
    seat_state: SeatState<Self>,
    output_state: OutputManagerState,
    data_device_state: DataDeviceState,

    seat: Seat<Self>,
    output: Output,
    pub wm: wm::WindowManager,
    pub dh: DisplayHandle,
    // M2: IPC-backed state; snapshot is rebuilt from the shell state each loop iteration
    ipc: Option<Ipc>,
    // M3: Lua state with Hyprland config API (es.bind, es.define_submap, es.dsp.*, es.config{}, etc.)
    lua: Option<mlua::Lua>,
    // Keybinds defined via Lua es.bind(); checked during key event processing
    pub lua_keybinds: Arc<std::sync::Mutex<Vec<lua_state::Keybind>>>,
    // M4: real DRM backend data; None in nested (winit) mode
    udev: Option<udev::UdevData>,
    // M4: calloop scheduling handle (timers, idle); None in nested mode
    handle: Option<LoopHandle<'static, State>>,
    // Native control channel (notes/tacit-integration.md); None if bind failed
    control: Option<control::Control>,
    /// The smithay Output the DRM/winit backend renders on; layer surfaces are
    /// arranged against it via LayerMap.
    primary_output: Option<Output>,
    /// desktop::LayerSurface wrappers, created once per layer surface (LayerMap
    /// needs the stable desktop wrapper, not the raw shell surface).
    layer_wrappers: Vec<smithay::desktop::LayerSurface>,
    // Cached border buffers keyed by (width, height, color) to avoid per-frame
    // allocation. Border elements are rebuilt each frame from cached buffers.
}

fn install_diag_logging() {
    // unconditional file logging + panic capture so TTY runs leave evidence
    let log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("/tmp/eswm.log");
    if let Ok(file) = log {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::sync::Mutex::new(file))
            .try_init();
    }
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(
            "/tmp/eswm-panic.log",
            format!("{info}\n\n{bt:?}"),
        );
        eprintln!("{info}");
    }));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    install_diag_logging();

    // The standalone DRM/libinput backend is the product runtime. Winit is kept
    // behind an explicit setting for nested development and protocol tests.
    match std::env::var("TACIT_COMP_BACKEND").as_deref() {
        Ok("winit") => run_winit(),
        _ => crate::udev::run_udev(),
    }
}

fn socket_name() -> String {
    std::env::var("TACIT_COMP_SOCKET").unwrap_or_else(|_| "eswm-test".into())
}

// Shared State/protocol construction for both backends.
fn new_state(
    seat_name: &str,
    dh: &DisplayHandle,
    want_virtual_output: bool,
) -> State {
    let compositor_state = CompositorState::new::<State>(dh);
    let shm_state = ShmState::new::<State>(dh, vec![]);
    let viewporter_state = ViewporterState::new::<State>(dh);
    let layer_shell_state = WlrLayerShellState::new::<State>(dh);
    let mut seat_state = SeatState::new();
    let seat = seat_state.new_wl_seat(dh, seat_name);
    let mut wm = wm::WindowManager::new();

    // In nested mode the winit window masquerades as one virtual output; the DRM
    // backend attaches real outputs per connector instead.
    let output = if want_virtual_output {
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
        wm.add_output(
            "winit",
            wm::Rect {
                x: 0,
                y: 0,
                width: 1280,
                height: 800,
            },
        );
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        output.set_preferred(mode);
        output.create_global::<State>(dh);
        output
    } else {
        // placeholder; real outputs get created per DRM connector in src/udev.rs
        Output::new(
            "drm".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "eswm".into(),
                model: "drm".into(),
            },
        )
    };

    State {
        compositor_state,
        xdg_shell_state: XdgShellState::new::<State>(dh),
        layer_shell_state,
        shm_state,
        viewporter_state,
        seat_state,
        output_state: OutputManagerState::new_with_xdg_output::<State>(&dh),
        data_device_state: DataDeviceState::new::<State>(dh),
        seat,
        output,
        wm,
        dh: dh.clone(),
        ipc: None,
        lua: None,
        udev: None,
        handle: None,
        control: None,
        primary_output: None,
        layer_wrappers: Vec::new(),
        lua_keybinds: Arc::new(std::sync::Mutex::new(Vec::new())),
    }
}

// Development compatibility path for the explicit Winit backend. The standalone
// DRM backend does not initialize this Hyprland/Tacit integration.
fn init_ipc_and_lua(state: &mut State) {
    // M2: hyprctl IPC. Signature env controls the hyprdir path; a distinct
    // value keeps us away from a real Hyprland session's sockets.
    let sig = std::env::var("TACIT_COMP_SIG").unwrap_or_else(|_| "eswm".into());
    state.ipc = Some(Ipc::new(sig));

    // M3: initialize Lua embed with Hyprland config API
    let lua = init().unwrap_or_else(|_| Lua::new());
    let snapshot = state.ipc.as_ref().unwrap().snapshot.clone();
    let cmd_tx = state.ipc.as_ref().unwrap().cmd_tx.clone();
    // Register the Hyprland config API (es.bind, es.define_submap, es.dsp.*, es.config{}, is_file_exists, safe_require)
    if let Err(e) = register_api(&lua, snapshot, &cmd_tx, state.lua_keybinds.clone()) {
        tracing::warn!("Lua API registration failed (mlua/lua5.1-dev may be missing): {e}");
    }
    // Store lua in state for _do_file/_do_chunk access; kept alive for the session
    state.lua = Some(lua);
}

/// Reap WM entries whose toplevel disappeared without a compositor callback (for
/// example after a client lost its display socket). Stale entries otherwise hold
/// master/stack slots forever.
pub fn prune_dead_windows(state: &mut State) -> usize {
    let dead: Vec<wm::WindowId> = state
        .wm
        .window_ids()
        .into_iter()
        .filter(|id| toplevel_for_window(state, *id).is_none())
        .collect();
    let count = dead.len();
    for id in dead {
        tracing::info!("dropping stale toplevel registration id={id}");
        state.wm.remove_window(id);
    }
    count
}

// M5: push target rects to clients as xdg_toplevel size configures, then mark applied.
pub fn apply_layout(state: &mut State) {
    prune_dead_windows(state);
    let windows = state
        .wm
        .window_ids()
        .into_iter()
        .filter_map(|id| state.wm.window(id).cloned().map(|w| (id, w)))
        .collect::<Vec<_>>();
    tracing::debug!("apply_layout: {} window(s)", windows.len());
    for (id, window) in windows {
        let target = window.target_rect;
        let needs_size = window.rect != target && target.width > 0 && target.height > 0;
        let needs_activation = window.activated_sent != Some(window.focused);
        if !needs_size && !needs_activation {
            continue;
        }
        if let Some(toplevel) = toplevel_for_window(state, id) {
            tracing::debug!("configuring window id={id:?} -> {}x{} at {},{}", target.width, target.height, target.x, target.y);
            toplevel.with_pending_state(|st| {
                if needs_size {
                    st.size = Some((target.width.max(1) as i32, target.height.max(1) as i32).into());
                }
                if window.focused {
                    st.states.set(xdg_toplevel::State::Activated);
                } else {
                    st.states.unset(xdg_toplevel::State::Activated);
                }
            });
            toplevel.send_configure();
            if needs_size {
                state.wm.set_rect(id, target);
            }
            state.wm.set_activated_sent(id, window.focused);
        }
    }
    // Keep wl_keyboard focus in sync with wm focus (window closed, pruned, etc.),
    // but never steal it from a layer surface with keyboard interactivity
    // (fuzzel): the per-frame sync would send enter/leave and dismiss it.
    let layer_has_focus = state
        .seat
        .get_keyboard()
        .and_then(|kbd| kbd.current_focus())
        .is_some_and(|surf| {
            state.layer_wrappers.iter().any(|l| l.wl_surface() == &surf)
        });
    let focused = state.wm.focused_id();
    if !layer_has_focus {
        crate::udev::keyboard_focus_set(state, focused);
    }
}

// Compositor keybind helper: spawn a terminal client on our own socket.
pub fn spawn_terminal() {
    spawn_on_socket("kitty", &[]);
}

/// Split a command string into program + args (whitespace split).
pub fn parse_cmd(cmd: &str) -> (&str, Vec<&str>) {
    let mut parts = cmd.split_whitespace();
    let program = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();
    (program, args)
}


// Spawn a Wayland client against eswm's socket, detached (own process group so
// VT signals don't hit it). Stdout/stderr go to <name>-spawn.log in /tmp.
pub fn spawn_on_socket(program: &str, args: &[&str]) {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    //WAYLAND_DISPLAY takes either absolute path or bare name relative to XDG_RUNTIME_DIR
    let log_path = format!("/tmp/{}-spawn.log", program.replace('/', "_"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path);
    let prog_name = program.to_owned();
    match unsafe {
        let mut cmd = Command::new(&prog_name);
        cmd.args(args)
            .env("WAYLAND_DISPLAY", socket_name())
            // session terminal for launchers (fuzzel wraps Terminal=true apps)
            .env("TERMINAL", "kitty")
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        match log {
            Ok(file) => cmd.stderr(Stdio::from(file)),
            Err(_) => cmd.stderr(Stdio::null()),
        };
        cmd.pre_exec(|| unsafe {
            // detach from the compositor process group so VT signals don't hit it
            libc::setsid();
            Ok(())
        })
        .spawn()
    } {
        Ok(mut child) => {
            tracing::info!("spawned {prog_name} pid {}", child.id());
            std::thread::spawn(move || match child.wait() {
                Ok(status) => tracing::warn!("{prog_name} exited with {status}"),
                Err(err) => tracing::warn!("{prog_name} wait failed: {err}"),
            });
        }
        Err(err) => tracing::warn!("failed to spawn {prog_name}: {err}"),
    }
}

// Development compatibility path; called only by the explicit Winit backend.
fn drain_ipc_and_refresh(state: &mut State, size: (i32, i32)) {
    let ipc_cmd_rx = state.ipc.as_ref().map(|ipc| ipc.cmd_rx.clone());
    let mut ipc_opt = state.ipc.take();
    if let Some(ref ipc) = ipc_opt {
        while let Ok(cmd) = ipc_cmd_rx.as_ref().unwrap().try_recv() {
            ipc::apply_cmd(state, ipc, cmd);
        }
    }
    state.ipc = ipc_opt;
    ipc::refresh_snapshot(state, size);
}

pub fn run_winit() -> Result<(), Box<dyn std::error::Error>> {
    let mut display: Display<State> = Display::new()?;
    let dh = display.handle();

    let mut state = new_state("winit", &dh, true);
    state.primary_output = Some(state.output.clone());

    let socket = socket_name();
    let listener = ListeningSocket::bind(&socket)
        .map_err(|err| format!("socket '{socket}' already in use - kill stale eswm (is one running?)\n  {err}"))?;
    println!("listening on {socket}");

    init_ipc_and_lua(&mut state);

    match control::Control::start(&socket_name()) {
        Ok(control) => state.control = Some(control),
        Err(err) => tracing::warn!("control socket disabled: {err}"),
    }

    let mut clients = Vec::new();

    let (mut backend, mut winit) = winit::init::<GlesRenderer>()?;

    let start_time = std::time::Instant::now();

    let keyboard = state.seat.add_keyboard(Default::default(), 200, 200).unwrap();
    state.seat.add_pointer();

    let mut size = backend.window_size();
    let mut toplevel_surface = None;

    loop {
        // Accept new Wayland clients (awww-daemon, fuzzel, etc.) and insert them
        // into the display so their protocol requests get dispatched.
        if let Ok(Some(stream)) = listener.accept() {
            if let Ok(client) = display.handle().insert_client(stream, Arc::new(crate::ClientState::default())) {
                clients.push(client);
            }
        }

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
                    // M1 focus trick: keep one toplevel focused
                    if toplevel_surface.is_none() {
                        if let Some(surface) = state.xdg_shell_state.toplevel_surfaces().iter().next().cloned() {
                            toplevel_surface = Some(surface.wl_surface().clone());
                        };
                        if let Some(surface) = toplevel_surface.as_ref() {
                            keyboard.set_focus(&mut state, Some(surface.clone()), 0.into());
                        }
                    }
                }
                _ => {}
            },
            _ => (),
        });

        match status {
            PumpStatus::Continue => (),
            PumpStatus::Exit(_) => return Ok(()),
        };

        // Process incoming Wayland client requests (awww-daemon, fuzzel, etc.)
        // so clients don't block waiting for responses to their roundtrips.
        if let Err(err) = display.dispatch_clients(&mut state) {
            tracing::warn!("wayland client dispatch error: {err}");
        }
        display.flush_clients();

        size = backend.window_size();
        let damage = Rectangle::from_size(size);

        drain_ipc_and_refresh(&mut state, size.into());
        control::drain(&mut state);

        {
            let (renderer, mut framebuffer) = backend.bind().unwrap();
            // M5 placement shared with the udev path: active-workspace windows at target_rect
            let placed = crate::placed_windows(&mut state);
            let toplevel_elements = placed
                .iter()
                .flat_map(|(surface, x, y, ..)| {
                    render_elements_from_surface_tree(
                        renderer,
                        surface,
                        (*x, *y),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    )
                })
                .collect::<Vec<WaylandSurfaceRenderElement<GlesRenderer>>>();
            // M5/P2: visible focus border around the focused window (dev winit backend;
            // DRM path draws borders via border_focused/border_unfocused buffers).
            const BORDER_PX: i32 = 2;
            let border_color = [0u8, 200, 0, 255]; // green
            let mut border_elements: Vec<MemoryRenderBufferRenderElement<GlesRenderer>> =
                Vec::new();
            for (_, x, y, _, focused, rect) in &placed {
                if !*focused {
                    continue;
                }
                let bw = rect.width.saturating_add(2 * BORDER_PX as u32);
                let bh = rect.height.saturating_add(2 * BORDER_PX as u32);
                let mut pixels = Vec::with_capacity((bw * bh * 4) as usize);
                for _ in 0..(bw * bh) {
                    pixels.extend_from_slice(&border_color);
                }
                let buf = MemoryRenderBuffer::from_slice(
                    &pixels,
                    Fourcc::Argb8888,
                    (bw as i32, bh as i32),
                    1,
                    Transform::Normal,
                    None,
                );
                let pos = Point::from((*x as f64, *y as f64));
                if let Ok(elem) = MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    pos,
                    &buf,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                ) {
                    border_elements.push(elem);
                }
            }
            let layer_elements = state
                .layer_shell_state
                .layer_surfaces()
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
            draw_render_elements(&mut frame, 1.0, &border_elements, &[damage]).unwrap();
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
delegate_viewporter!(State);
delegate_seat!(State);
delegate_data_device!(State);
