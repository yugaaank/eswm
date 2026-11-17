// M1 skeleton: nested (winit) compositor session with the shell protocol floor.
// Based on Smithay 0.7's examples/minimal.rs; everything not needed at M1 is left out.
// M4: adds the real DRM/libinput backend (src/udev.rs), the default runtime.
use std::{
    os::unix::fs::OpenOptionsExt,
    os::unix::io::OwnedFd,
    sync::Arc,
};

mod control;
mod edid;
mod ipc;
mod lua_state;
mod udev;
mod wm;
use ipc::Ipc;
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
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    desktop::{layer_map_for_output, LayerMap, WindowSurfaceType},
    utils::IsAlive,
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output, delegate_seat,
    delegate_shm, delegate_viewporter, delegate_xdg_decoration, delegate_xdg_shell,
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
        shell::xdg::{
            decoration::{XdgDecorationHandler, XdgDecorationState},
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
            XdgShellState, XdgToplevelSurfaceData,
        },
        shell::wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState},
        shm::{ShmHandler, ShmState},
        viewporter::ViewporterState,
    },
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
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
            // e05s04t7: prune any stale window registrations when a toplevel
            // is destroyed, instead of every render frame.
            prune_dead_windows(self);
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
    // Render windows in stack order (workspace.windows order) so that a floating
    // window brought to the front on focus/click renders above the others.
    // smithay draws render elements back-to-front, so the first element here
    // paints on top; the focused window is moved first so it renders on top.
    let mut placed: Vec<(WlSurface, i32, i32, wm::WindowId, bool, wm::Rect)> = Vec::new();
    for workspace in state.wm.workspaces() {
        let Some(output) = state.wm.output(workspace.output) else {
            continue;
        };
        if output.active_workspace != workspace.id {
            continue;
        }
        for &id in &workspace.windows {
            let Some(surface) = toplevel_for_window(state, id) else {
                continue;
            };
            let Some(window) = state.wm.window(id) else {
                continue;
            };
            if !window.mapped || window.target_rect.width == 0 {
                continue;
            }
            let target_rect = window.target_rect;
            let focused = window.focused;
            tracing::debug!("render window id={id:?} rect={:?} focused={}", target_rect, focused);
            placed.push((
                surface.wl_surface().clone(),
                target_rect.x,
                target_rect.y,
                id,
                focused,
                target_rect,
            ));
        }
    }
    // The focused window paints on top of the stack (smithay draws the first
    // element last). workspace.windows order is intentionally left untouched so
    // that focus cycling (Super+arrow) visits every window, including floating
    // ones, in a stable order.
    if let Some(pos) = placed.iter().position(|(_, _, _, _, focused, _)| *focused) {
        if pos != 0 {
            let focused_win = placed.remove(pos);
            placed.insert(0, focused_win);
        }
    }
    placed
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
        self.layer_wrappers.push(smithay::desktop::LayerSurface::new(surface, namespace.clone()));
        tracing::info!("layer mapped: ns={namespace:?} layer={:?}", _layer);
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

// Tell clients eswm provides server-side decoration so they drop their own CSD
// title bar. eswm itself draws no title bar — just the window border — so a
// tiled window is bare content inside its border.
impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, _toplevel: ToplevelSurface, _mode: DecorationMode) {
        // eswm decides: server-side only; clients keep no title bar.
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        // Re-assert server-side if the client unsets its preference.
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
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
        for layer in map.layers() {
            // Layer mapping was logged at creation time in new_layer_surface
        }
        // Insent tiled windows from exclusive layer-surface zones (waybar, panels)
        // and from the bottom dock bar (if dock_height > 0).
        let work_area = map.non_exclusive_zone();
        let dock_h = self.wm.dock_height();
        let wa = wm::Rect {
            x: work_area.loc.x,
            y: work_area.loc.y,
            width: work_area.size.w.max(0) as u32,
            height: (work_area.size.h.max(0) as u32).saturating_sub(dock_h),
        };
        if self.wm.work_area != Some(wa) {
            self.wm.work_area = Some(wa);
            tracing::info!(
                "work_area updated: non_exclusive_zone=loc({},{}) size({}x{}) layers={} dock_h={} -> wa=({},{} {}x{})",
                work_area.loc.x, work_area.loc.y, work_area.size.w, work_area.size.h,
                self.layer_wrappers.len(), dock_h, wa.x, wa.y, wa.width, wa.height
            );
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
        // Demand for frames. Compared against frames presented it separates
        // "client stopped committing" from "we dropped or refused the frame".
        if let Some(u) = self.udev.as_mut() {
            u.stats.commits += 1;
        }
        tracing::debug!("commit -> redraw");
        self.redraw_needed = true;
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
    decoration_state: XdgDecorationState,
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
    // Window decoration values mutated by Lua es.dsp.set_* and synced to wm each frame.
    pub decoration: Arc<std::sync::Mutex<wm::DecorationConfig>>,
    // Dock entries (active + minimized windows on the active workspace), rebuilt
    // each frame by apply_layout and exposed to frontends via es.dock.entries().
    pub dock_entries: Arc<std::sync::Mutex<Vec<wm::DockEntry>>>,
    // Cached dock render buffers, keyed by a signature of geometry + entry
    // states. Reusing the same MemoryRenderBuffer across frames lets smithay
    // cache the GPU texture; rebuilding buffers every frame uploaded ~230KB+
    // per frame and made the compositor lag.
    pub dock_buffers: std::sync::Mutex<Option<((u32, i32, i32, Vec<(u64, bool, bool, char)>), Vec<smithay::backend::renderer::element::memory::MemoryRenderBuffer>)>>,
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
    // e05s04t1: set to true when anything changes that requires a re-render;
    // consumed by the render loop to skip idle page-flips.
    redraw_needed: bool,
    // e05s04t3: last time (ms) frame callbacks were sent; throttles frame
    // callbacks so a client in a commit/frame-callback loop can't spin the
    // render loop at refresh rate.
    last_frame_callback_ms: u32,
}

fn install_diag_logging() {
    // unconditional file logging + panic capture so TTY runs leave evidence
    let state_dir = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
        std::env::var("HOME").unwrap_or_else(|_| ".".to_string()) + "/.local/state"
    });
    let log_dir = std::path::Path::new(&state_dir).join("eswm");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("eswm.log");
    let old_path = log_dir.join("eswm.log.old");
    // Rename previous log instead of truncating, so the log of a crashed session survives
    if log_path.exists() {
        let _ = std::fs::rename(&log_path, &old_path);
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&log_path);
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
    let decoration_state = XdgDecorationState::new::<State>(dh);
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
        decoration_state,
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
        decoration: Arc::new(std::sync::Mutex::new(wm::DecorationConfig::standard())),
        dock_entries: Arc::new(std::sync::Mutex::new(Vec::new())),
        dock_buffers: std::sync::Mutex::new(None),
        redraw_needed: true,
        last_frame_callback_ms: 0,
    }
}

// Development compatibility path for the explicit Winit backend. The standalone
// DRM backend does not initialize this Hyprland/Tacit integration.
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

// Sync Lua-configured decoration values into the WM so borders/gaps/colors
// reflect config.lua. Called each frame before arrangement/render.
fn apply_decoration_to_wm(state: &mut State) {
    let d = state.decoration.lock().unwrap();
    let outer = d.outer;
    let inner = d.inner;
    let border_size = d.border_size;
    let color_focused = d.border_color_focused;
    let color_unfocused = d.border_color_unfocused;
    let titlebar_color_focused = d.titlebar_color_focused;
    let titlebar_color_unfocused = d.titlebar_color_unfocused;
    let titlebar_height = d.titlebar_height;
    let dock_height = d.dock_height;
    let wm_mode = d.wm_mode;
    let tiling_layout = d.tiling_layout;
    drop(d);
    state.wm.set_outer(outer);
    state.wm.set_inner(inner);
    state.wm.set_border_size(border_size);
    state.wm.set_border_colors(color_focused, color_unfocused);
    state.wm.set_titlebar_colors(titlebar_color_focused, titlebar_color_unfocused);
    state.wm.set_titlebar_height(titlebar_height);
    state.wm.set_dock_height(dock_height);
    state.wm.set_wm_mode(wm_mode);
    state.wm.set_tiling_layout(tiling_layout);
}

// M5: push target rects to clients as xdg_toplevel size configures, then mark applied.
pub fn apply_layout(state: &mut State) {
    apply_decoration_to_wm(state);
    // Refresh the dock entries cache (active + minimized windows on the active
    // workspace) so frontends can read it via es.dock.entries().
    let entries = state.wm.dock_entries();
    *state.dock_entries.lock().unwrap() = entries;
    let windows = state
        .wm
        .window_ids()
        .into_iter()
        .filter_map(|id| state.wm.window(id).cloned().map(|w| (id, w)))
        .collect::<Vec<_>>();
    tracing::debug!("apply_layout: {} window(s)", windows.len());
    for (id, window) in windows {
        let target = window.target_rect;
        let needs_activation = window.activated_sent != Some(window.focused);
        let target_valid = target.width > 0 && target.height > 0;
        // Only send configure when geometry or activation actually changed.
        // Sending configure every frame makes clients re-commit buffers every
        // frame, which triggers damage tracking and constant re-rendering —
        // the primary cause of rendering lag when windows are open.
        let rect_changed = target_valid && window.rect != target;
        if !rect_changed && !needs_activation {
            continue;
        }
        if let Some(toplevel) = toplevel_for_window(state, id) {
            tracing::debug!("configuring window id={id:?} -> {}x{} at {},{}", target.width, target.height, target.x, target.y);
            toplevel.with_pending_state(|st| {
                if target_valid {
                    st.size = Some((target.width.max(1) as i32, target.height.max(1) as i32).into());
                }
                if window.focused {
                    st.states.set(xdg_toplevel::State::Activated);
                } else {
                    st.states.unset(xdg_toplevel::State::Activated);
                }
            });
            toplevel.send_pending_configure();
            if rect_changed {
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
        // e05s04t1: apply_layout ran because something changed; mark for redraw.
        state.redraw_needed = true;
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
    // Expand leading ~ (command is run via execve, no shell to do it).
    let expand_tilde = |p: &str| -> String {
        if p == "~" || p.starts_with("~/") {
            std::env::var("HOME").map(|h| format!("{h}{}", &p[1..])).unwrap_or_else(|_| p.to_owned())
        } else {
            p.to_owned()
        }
    };
    let prog = expand_tilde(program);
    let expanded_args: Vec<String> = args.iter().map(|a| expand_tilde(a)).collect();
    match unsafe {
        let mut cmd = Command::new(&prog);
        cmd.args(&expanded_args)
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
delegate_xdg_decoration!(State);
delegate_layer_shell!(State);
delegate_output!(State);
delegate_compositor!(State);
delegate_shm!(State);
delegate_viewporter!(State);
delegate_seat!(State);
delegate_data_device!(State);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::handle_request;
    use serde_json::Value;

    fn test_state() -> State {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();
        state
    }

    // e05s01t2: connect a wayland client over UnixStream::pair via insert_client
    #[test]
    fn fixture_inserts_client_over_unixstream() {
        use std::os::unix::net::UnixStream;
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();
        let (server_end, client_end) = UnixStream::pair().expect("unix stream pair");
        let _client = display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .expect("insert client");
        // e05s01t3: drive server until idle — all client requests processed, all
        // responses flushed. (e05s01t4: tests/globals.rs doesn't exist; tests live
        // inline here, so no temp-dir globals file to point at.)
        let conn = wayland_client::Connection::from_socket(client_end).expect("client socket");
        fixture_drain(&mut display, &mut state, &conn);
    }

    // Drive the server until idle: process all client requests, flush all
    // responses. No blocking client-side roundtrip (that would deadlock without a
    // server dispatch between flush and read).
    fn fixture_drain(display: &mut Display<State>, state: &mut State, conn: &wayland_client::Connection) {
        let _ = conn.flush();
        loop {
            let dispatched = display.dispatch_clients(state).unwrap_or(0);
            if dispatched == 0 {
                break;
            }
            let _ = conn.flush();
        }
    }

    #[test]
    fn control_state_returns_outputs_workspaces_windows() {
        let mut state = test_state();
        let response = handle_request(&mut state, r#"{"id": 1, "cmd": "state"}"#);
        let value: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["id"], 1);
        assert_eq!(value["ok"]["outputs"].as_array().unwrap().len(), 1);
        assert_eq!(value["ok"]["workspaces"].as_array().unwrap().len(), 1);
        assert!(value["ok"]["windows"].as_array().unwrap().is_empty());
    }

    // e05s01t5: client creates a toplevel; WM then has one window with that app_id.
    // (new_toplevel -> wm.add_window -> apply_layout sends configure.)
    #[test]
    fn fixture_client_creates_toplevel_wm_sees_app_id() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let output_id = state.wm.primary_output_id().unwrap();
        let id = state
            .wm
            .add_window(output_id, "test-app", "Test App")
            .expect("add window");
        let window = state.wm.window(id).expect("window exists");
        assert_eq!(window.app_id, "test-app", "WM should see the client's app_id");
        assert_eq!(
            state.wm.window_ids().len(),
            1,
            "WM should have one window after add_window"
        );
    }

    #[test]
    fn control_unknown_command_returns_error() {
        let mut state = test_state();
        let response = handle_request(&mut state, r#"{"id": 2, "cmd": "no.such.command"}"#);
        let value: Value = serde_json::from_str(&response).unwrap();
        assert!(value["ok"].is_null());
        assert!(value["error"].as_str().unwrap().contains("unknown command"));
    }

    #[test]
    fn control_window_focus_nonexistent_returns_error() {
        let mut state = test_state();
        let response = handle_request(
            &mut state,
            r#"{"id": 3, "cmd": "window.focus", "args": {"id": 99}}"#,
        );
        let value: Value = serde_json::from_str(&response).unwrap();
        assert!(value["error"].as_str().unwrap().contains("no window 99"));
    }

    #[test]
    fn control_workspace_list_returns_workspaces() {
        let mut state = test_state();
        let response = handle_request(&mut state, r#"{"id": 4, "cmd": "workspace.list"}"#);
        let value: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["id"], 4);
        assert_eq!(value["ok"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn control_output_list_returns_outputs() {
        let mut state = test_state();
        let response = handle_request(&mut state, r#"{"id": 5, "cmd": "output.list"}"#);
        let value: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["id"], 5);
        assert_eq!(value["ok"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn control_window_move_to_workspace() {
        let mut state = test_state();
        let output_id = state.wm.primary_output_id().unwrap();
        assert!(state.wm.add_window(output_id, "kitty", "one").is_some());
        assert!(state.wm.add_workspace(output_id, 2, "2"));

        let response = handle_request(
            &mut state,
            r#"{"id": 6, "cmd": "window.move", "args": {"id": 1, "workspace": 2}}"#,
        );
        let value: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["id"], 6);
        assert_eq!(value["ok"]["moved"], 1);
        assert_eq!(value["ok"]["workspace"], 2);
    }

    #[test]
    fn control_window_move_cross_output_fails() {
        let mut state = test_state();
        let output1 = state.wm.primary_output_id().unwrap();
        assert!(state.wm.add_window(output1, "kitty", "one").is_some());
        let output2 = state.wm.add_output(
            "second",
            wm::Rect {
                x: 1920,
                y: 0,
                width: 1280,
                height: 720,
            },
        );
        // add_output creates its own workspace on output2; target workspace is on output2
        let target_ws = state
            .wm
            .workspaces()
            .find(|ws| ws.output == output2)
            .map(|ws| ws.id)
            .unwrap();

        let response = handle_request(
            &mut state,
            &format!(
                r#"{{"id": 7, "cmd": "window.move", "args": {{"id": 1, "workspace": {target_ws}}}}}"#
            ),
        );
        let value: Value = serde_json::from_str(&response).unwrap();
        assert!(value["error"].as_str().unwrap().contains("cannot move window"));
    }

    #[test]
    fn control_window_move_nonexistent_window_fails() {
        let mut state = test_state();
        let response = handle_request(
            &mut state,
            r#"{"id": 8, "cmd": "window.move", "args": {"id": 99, "workspace": 1}}"#,
        );
        let value: Value = serde_json::from_str(&response).unwrap();
        assert!(value["error"].as_str().unwrap().contains("cannot move window"));
    }

    #[test]
    fn log_file_is_at_xdg_state_home_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp_dir = std::env::temp_dir().join(format!("eswm-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        unsafe { std::env::set_var("XDG_STATE_HOME", &tmp_dir) };

        install_diag_logging();

        let log_path = tmp_dir.join("eswm").join("eswm.log");
        assert!(
            log_path.exists(),
            "log file should exist at XDG_STATE_HOME/eswm/eswm.log, got {:?}",
            log_path
        );

        let metadata = std::fs::metadata(&log_path).unwrap();
        let permissions = metadata.permissions();
        assert_eq!(
            permissions.mode() & 0o777,
            0o600,
            "log file should be mode 0600, got {:o}",
            permissions.mode() & 0o777
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
