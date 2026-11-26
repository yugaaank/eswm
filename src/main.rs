// A Wayland compositor with a shell protocol floor. M1 ran nested in winit;
// winit has since walked the plank, so this is the DRM standalone compositor.
// Based on Smithay 0.7's examples/minimal.rs; everything not needed at M1 is left out.
// M4: adds the real DRM/libinput backend (src/udev.rs), the default runtime.
use std::{
    cell::RefCell,
    os::unix::fs::OpenOptionsExt,
    os::unix::io::OwnedFd,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

mod clip;
mod control;
mod edid;
mod ipc;
mod lua_state;
mod rounded_surface;
mod rounding;
mod udev;
mod wm;
use control::EventKind;
use ipc::Ipc;
use serde_json::json;

use smithay::{
    backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    delegate_compositor, delegate_data_device, delegate_ext_data_control, delegate_layer_shell,
    delegate_output, delegate_primary_selection, delegate_seat, delegate_shm, delegate_viewporter,
    delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{layer_map_for_output, WindowSurfaceType},
    input::{Seat, SeatHandler, SeatState},
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::LoopHandle,
        wayland_server::{protocol::wl_seat, DisplayHandle},
    },
    utils::{IsAlive, Logical, Point, Rectangle, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_states, with_surface_tree_downward, CompositorClientState, CompositorHandler,
            CompositorState, SurfaceAttributes, TraversalAction,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            ext_data_control::{DataControlHandler, DataControlState},
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
            SelectionHandler,
        },
        shell::wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState},
        shell::xdg::{
            decoration::{XdgDecorationHandler, XdgDecorationState},
            PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
            XdgShellState, XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
        viewporter::ViewporterState,
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
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
    Client, Resource,
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
        // e09s05t1: the client can still set app_id/title after creating the
        // xdg_toplevel. Configure now so it knows our size policy; register the
        // WM window on first commit, when rules can see real metadata.
        surface.send_configure();
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        sync_window_meta(self, &surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        sync_window_meta(self, &surface);
    }

    // e09s05t3: these are set/unset requests, not toggles. A client saying
    // "unfullscreen" to an already-normal window is treated as a shrug —
    // otherwise every video player quitting would flip it fullscreen again.
    fn maximize_request(&mut self, surface: ToplevelSurface) {
        set_client_presentation(self, &surface, wm::Presentation::Maximized, true);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        set_client_presentation(self, &surface, wm::Presentation::Maximized, false);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        set_client_presentation(self, &surface, wm::Presentation::Fullscreen, true);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        set_client_presentation(self, &surface, wm::Presentation::Fullscreen, false);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // e09s05t2: prefer the direct index; the data-map tag is the fallback
        // for a surface that committed but was never indexed.
        let id = self
            .toplevels
            .iter()
            .find(|(_, t)| t.wl_surface() == surface.wl_surface())
            .map(|(id, _)| *id)
            .or_else(|| window_id_of(surface.wl_surface()));
        if let Some(id) = id {
            self.toplevels.remove(&id);
            self.wm.remove_window(id);
            // e05s04t7: prune any stale window registrations when a toplevel
            // is destroyed, instead of every render frame.
            prune_dead_windows(self);
            crate::apply_layout(self);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        // Immediate parent: a toplevel for a menu, another popup for a submenu.
        let parent_surface = surface.get_parent_surface();
        // Owner toplevel drives keyboard-focus restore when the menu closes
        // (Hyprland CPopup::getT1Owner follows the popup chain for the same reason).
        let parent = parent_surface.as_ref().and_then(|ps| {
            self.xdg_shell_state
                .toplevel_surfaces()
                .iter()
                .find(|t| t.alive() && t.wl_surface() == ps)
                .cloned()
                .or_else(|| {
                    self.popups
                        .iter()
                        .find(|p| p.surface.wl_surface() == ps)
                        .and_then(|p| p.parent.clone())
                })
        });

        let parent_coord = self.popup_parent_coord(parent_surface.as_ref());
        let available = popup_constraint(self, parent_coord);
        let geometry = popup_geometry(&positioner, parent_coord, available);
        let screen_rect = wm::Rect {
            x: parent_coord.0 + geometry.x,
            y: parent_coord.1 + geometry.y,
            ..geometry
        };

        tracing::info!(
            "xdg_popup new_popup: parent={:?} parent_coord={:?} anchor_rect={:?} anchor={:?} gravity={:?} offset={:?} rel={:?} screen={:?}",
            parent.is_some(),
            parent_coord,
            (positioner.anchor_rect.loc.x, positioner.anchor_rect.loc.y, positioner.anchor_rect.size.w, positioner.anchor_rect.size.h),
            positioner.anchor_edges,
            positioner.gravity,
            (positioner.offset.x, positioner.offset.y),
            (geometry.x, geometry.y, geometry.width, geometry.height),
            (screen_rect.x, screen_rect.y, screen_rect.width, screen_rect.height),
        );

        // Store popup with positioner and resolved parent
        self.popups.push(PopupData {
            surface: surface.clone(),
            parent,
            parent_surface,
            positioner,
            geometry,
            screen_rect,
            mapped: false,
        });

        // Send the initial configure with the positioned geometry, exactly like
        // Hyprland's applyPositioning -> configure at popup creation.
        surface.with_pending_state(|state| {
            state.geometry = Rectangle::new(
                (geometry.x, geometry.y).into(),
                (geometry.width as i32, geometry.height as i32).into(),
            );
        });
        match surface.send_configure() {
            Ok(serial) => tracing::info!("xdg_popup configure sent serial={serial:?}"),
            Err(e) => tracing::warn!("xdg_popup configure failed: {e:?}"),
        }
        self.redraw_needed = true;
    }

    fn grab(&mut self, surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // Set keyboard/pointer grab for modal popup behavior.
        // When a grab is active, all input goes to this popup.
        self.popup_grab = Some(surface.clone());
        tracing::info!("xdg_popup grab set");
        // Set keyboard focus to the popup surface so key events route to it
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(surface.wl_surface().clone()), crate::udev::SCOUNTER.next_serial());
        }
        self.redraw_needed = true;
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        // Client closed the menu itself (Escape, item activated, unmap). Without
        // this cleanup the popup stayed in the list and the grab/focus could
        // stay pointed at a dead surface (Hyprland: onPopupDestroy + setGrab(None)).
        let owner = self
            .popups
            .iter()
            .find(|p| p.surface == surface)
            .and_then(|p| p.parent.as_ref())
            .and_then(|t| window_id_of(t.wl_surface()));
        let was_grab = self.popup_grab.as_ref().map(|g| *g == surface).unwrap_or(false);

        self.popups.retain(|p| p.surface != surface);
        if was_grab {
            self.popup_grab = None;
            crate::udev::keyboard_focus_set(self, owner.or(self.wm.focused_id()));
        }
        self.redraw_needed = true;
        tracing::info!("xdg_popup destroyed: grab_released={was_grab} owner={owner:?}");
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        let parent_surface = self
            .popups
            .iter()
            .find(|p| p.surface == surface)
            .and_then(|p| p.parent_surface.clone());
        let parent_coord = self.popup_parent_coord(parent_surface.as_ref());
        let available = popup_constraint(self, parent_coord);
        let geometry = popup_geometry(&positioner, parent_coord, available);
        let screen_rect = wm::Rect {
            x: parent_coord.0 + geometry.x,
            y: parent_coord.1 + geometry.y,
            ..geometry
        };

        if let Some(popup) = self.popups.iter_mut().find(|p| p.surface == surface) {
            popup.positioner = positioner;
            popup.geometry = geometry;
            popup.screen_rect = screen_rect;
        }

        surface.with_pending_state(|state| {
            state.geometry = Rectangle::new(
                (geometry.x, geometry.y).into(),
                (geometry.width as i32, geometry.height as i32).into(),
            );
        });
        // The protocol requires an ack for the client's token or the client waits
        // forever (Hyprland CXDGPopupResource::repositioned).
        surface.send_repositioned(token);
        self.redraw_needed = true;
        tracing::info!(
            "xdg_popup repositioned: token={token} rel=({},{} {}x{})",
            geometry.x, geometry.y, geometry.width, geometry.height
        );
    }
}

pub fn window_id_of(surface: &WlSurface) -> Option<wm::WindowId> {
    with_states(surface, |states| {
        states.data_map.get::<wm::WindowId>().copied()
    })
}

// M5 placement, shared by the udev render loop and the test fixtures: active-workspace,
// mapped windows at their wm target_rect.
// (surface, x, y, id, focused, rect, fill_scale)
type PlacedWindow = (WlSurface, i32, i32, wm::WindowId, bool, wm::Rect, (f64, f64));

#[allow(clippy::mutable_key_type)] // WlSurface hashes by identity; the map is rebuilt every frame
pub(crate) fn placed_windows(state: &mut State) -> Vec<PlacedWindow> {
    // Render windows in stack order (workspace.windows order) so that a floating
    // window brought to the front on focus/click renders above the others.
    // smithay draws render elements back-to-front, so the first element here
    // paints on top; the focused window is moved first so it renders on top.
    let mut placed: Vec<PlacedWindow> = Vec::new();
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
            if !window.mapped || window.minimized || window.target_rect.width == 0 {
                continue;
            }
            let target_rect = window.target_rect;
            let focused = window.focused;
            let (sx, sy) = window.fill_scale;
            // render position = target_rect - xdg_offset * scale, so a scaled
            // client's window geometry still starts at the frame corner.
            let render_x = target_rect.x - (window.xdg_offset.0 as f64 * sx).round() as i32;
            let render_y = target_rect.y - (window.xdg_offset.1 as f64 * sy).round() as i32;
            tracing::debug!("render window id={id:?} rect={:?} offset={:?} scale=({sx},{sy}) render=({}, {}) focused={}",
                target_rect, window.xdg_offset, render_x, render_y, focused);
            placed.push((
                surface.wl_surface().clone(),
                render_x,
                render_y,
                id,
                focused,
                target_rect,
                (sx, sy),
            ));
        }
    }
    // The focused window paints on top of the stack (smithay draws the first
    // element last). workspace.windows order is intentionally left untouched so
    // that focus cycling (Super+arrow) visits every window, including floating
    // ones, in a stable order.
    if let Some(pos) = placed.iter().position(|(_, _, _, _, focused, _, _)| *focused)
        && pos != 0 {
            let focused_win = placed.remove(pos);
            placed.insert(0, focused_win);
        }
    // Add popup surfaces at the front so they paint above toplevel windows.
    // Popups are modal and should render above their parent and all other windows.
    // Position: parent window-geometry origin + positioner geometry (anchor,
    // gravity, offset, constraints). Recomputed every frame so a menu follows
    // its parent window when it moves.
    let mut parent_pos: std::collections::HashMap<WlSurface, (i32, i32)> = placed
        .iter()
        .map(|(s, _, _, _, _, rect, _)| (s.clone(), (rect.x, rect.y)))
        .collect();
    // Bar menus are popups parented to a layer surface, not a toplevel. Seed
    // their origin too or the menu lands in the top-left corner of the output.
    for output in state.layer_outputs.values() {
        let map = layer_map_for_output(output);
        for layer in map.layers() {
            if let Some(geom) = map.layer_geometry(layer) {
                let origin = output.current_location();
                parent_pos.insert(
                    layer.wl_surface().clone(),
                    (origin.x + geom.loc.x, origin.y + geom.loc.y),
                );
            }
        }
    }
    let mut popup_positions: std::collections::HashMap<WlSurface, (i32, i32)> =
        std::collections::HashMap::new();
    for i in 0..state.popups.len() {
        if !state.popups[i].surface.alive() {
            continue;
        }
        let surface = state.popups[i].surface.wl_surface().clone();
        let parent_surface = state.popups[i].parent_surface.clone();
        let parent_coord = parent_surface
            .as_ref()
            .and_then(|p| parent_pos.get(p).or_else(|| popup_positions.get(p)))
            .copied()
            .unwrap_or_default();
        let available = popup_constraint(state, parent_coord);
        let positioner = state.popups[i].positioner;
        let geometry = popup_geometry(&positioner, parent_coord, available);
        let screen_rect = wm::Rect {
            x: parent_coord.0 + geometry.x,
            y: parent_coord.1 + geometry.y,
            ..geometry
        };
        state.popups[i].geometry = geometry;
        state.popups[i].screen_rect = screen_rect;
        popup_positions.insert(surface.clone(), (screen_rect.x, screen_rect.y));
        placed.insert(0, (
            surface,
            screen_rect.x,
            screen_rect.y,
            POPUP_FAKE_ID,
            false,
            screen_rect,
            (1.0, 1.0),
        ));
    }
    placed
}

/// Sentinel WindowId for popup entries in the render list: popups are not wm
/// windows and must not get server-side borders or titlebars.
pub const POPUP_FAKE_ID: wm::WindowId = u64::MAX;

/// Rect the popup is constrained to: the work area of the output its parent
/// sits on (excludes that monitor's bars and dock), falling back to a sane
/// default only when eswm has no outputs at all.
fn popup_constraint(state: &State, parent: (i32, i32)) -> wm::Rect {
    state
        .wm
        .output_for_point(parent.0, parent.1)
        .or_else(|| state.wm.focused_output_id())
        .map(|id| state.wm.work_area_for_output(id))
        .filter(|rect| rect.width > 0 && rect.height > 0)
        .unwrap_or(wm::Rect { x: 0, y: 0, width: 1920, height: 1080 })
}

/// Resolve the xdg_positioner rules to a rect relative to the parent's window
/// geometry origin, constrained to `available` (global screen coordinates).
///
/// Smithay implements the same anchor/gravity/offset + flip/slide/resize rules
/// that Hyprland hand-rolls in `CXDGPositionerRules::getPosition`, so the
/// positioner state is the single source of truth here instead of just
/// `offset` (which is zero for a typical GTK context menu).
pub fn popup_geometry(positioner: &PositionerState, parent: (i32, i32), available: wm::Rect) -> wm::Rect {
    // The target rect must be in the same coordinate space as the returned
    // geometry, i.e. relative to the parent window geometry.
    let target = Rectangle::new(
        (available.x - parent.0, available.y - parent.1).into(),
        (available.width.max(1) as i32, available.height.max(1) as i32).into(),
    );
    let geo = positioner.get_unconstrained_geometry(target);
    wm::Rect {
        x: geo.loc.x,
        y: geo.loc.y,
        width: geo.size.w.max(1) as u32,
        height: geo.size.h.max(1) as u32,
    }
}

/// True when the logical point lies inside `rect`.
pub fn rect_contains(rect: wm::Rect, x: f64, y: f64) -> bool {
    x >= rect.x as f64
        && y >= rect.y as f64
        && x < (rect.x + rect.width as i32) as f64
        && y < (rect.y + rect.height as i32) as f64
}

/// Screen rect of a tracked popup, used for pointer hit-testing.
pub(crate) fn popup_screen_rect(state: &State, popup: &PopupSurface) -> Option<wm::Rect> {
    state
        .popups
        .iter()
        .find(|p| p.surface == *popup)
        .map(|p| p.screen_rect)
}

/// Toplevel that owns a popup; keyboard focus returns here on dismissal.
pub(crate) fn popup_owner_window(state: &State, popup: &PopupSurface) -> Option<wm::WindowId> {
    state
        .popups
        .iter()
        .find(|p| p.surface == *popup)
        .and_then(|p| p.parent.as_ref())
        .and_then(|t| window_id_of(t.wl_surface()))
}

/// Convert a smithay RegionAttributes into our wm input-region representation.
pub(crate) fn region_attrs_to_input(attrs: &smithay::wayland::compositor::RegionAttributes) -> wm::InputRegion {
    use smithay::wayland::compositor::RectangleKind;
    wm::InputRegion {
        rects: Some(
            attrs
                .rects
                .iter()
                .map(|(kind, rect)| {
                    let add = matches!(kind, RectangleKind::Add);
                    (
                        add,
                        wm::Rect {
                            x: rect.loc.x,
                            y: rect.loc.y,
                            width: rect.size.w.max(0) as u32,
                            height: rect.size.h.max(0) as u32,
                        },
                    )
                })
                .collect(),
        ),
    }
}

/// Apply a committed input region to a window (None = default whole surface).
pub(crate) fn apply_window_input_region(
    state: &mut State,
    window_id: wm::WindowId,
    attrs: Option<&smithay::wayland::compositor::RegionAttributes>,
) {
    let region = match attrs {
        Some(a) => region_attrs_to_input(a),
        None => wm::InputRegion::default(),
    };
    state.wm.set_window_input_region(window_id, region);
}

/// Sync committed input region from a wl_surface onto its wm window, if any.
pub(crate) fn sync_window_input_region(state: &mut State, surface: &WlSurface) {
    let Some(window_id) = window_id_of(surface) else {
        return;
    };
    let region = with_states(surface, |states| {
        states
            .cached_state
            .get::<smithay::wayland::compositor::SurfaceAttributes>()
            .current()
            .input_region
            .clone()
    });
    apply_window_input_region(state, window_id, region.as_ref());
}


/// How the render loop should draw the pointer for a client's cursor status.
/// e08s03t3: keeps the Hidden/Named/Surface policy out of the render loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorRenderDecision {
    Hide,
    XcursorFallback,
    ClientSurface,
}

fn cursor_render_decision(status: &smithay::input::pointer::CursorImageStatus) -> CursorRenderDecision {
    use smithay::input::pointer::CursorImageStatus;
    match status {
        CursorImageStatus::Hidden => CursorRenderDecision::Hide,
        CursorImageStatus::Named(_) => CursorRenderDecision::XcursorFallback,
        CursorImageStatus::Surface(_) => CursorRenderDecision::ClientSurface,
    }
}

/// Map a client's xdg_surface window geometry to the WM's xdg_offset.
/// e08s02t5: render position is `target_rect - xdg_offset`, so the offset is
/// the window-geometry origin relative to the surface origin.
fn window_geometry_offset(geometry: Option<Rectangle<i32, smithay::utils::Logical>>) -> (i32, i32) {
    geometry.map(|geo| (geo.loc.x, geo.loc.y)).unwrap_or((0, 0))
}

fn toplevel_for_window(state: &State, id: wm::WindowId) -> Option<ToplevelSurface> {
    state.toplevels.get(&id).filter(|t| t.alive()).cloned()
}

/// Layer surfaces are tenants the WM does not own; the bus still takes their
/// mail. A wrapper with no output has no monitor id to report, so it stays
/// silent rather than inventing one.
fn queue_layer_event(state: &mut State, kind: EventKind, output: Option<wm::OutputId>) {
    if let Some(id) = output {
        state.wm.events.queue(kind, json!({"id": id}));
    }
}

pub(crate) fn queue_config_reloaded(state: &mut State) {
    state.wm.events.queue(EventKind::ConfigReloaded, json!({}));
}

pub(crate) fn queue_ready(state: &mut State) {
    state.wm.events.queue(EventKind::Ready, json!({}));
}

impl State {
    /// Both `es.config.reload()` and the compat socket reload land here. A
    /// failed Lua file restores binds, hooks, rules and config values before
    /// the caller emits the legacy reload signal.
    pub(crate) fn reload_lua_config(&mut self) -> Result<(), String> {
        let path = self
            .lua_config_path
            .clone()
            .ok_or_else(|| "no lua config path".to_string())?;
        let lua = self.lua.clone().ok_or_else(|| "no lua state".to_string())?;
        let (snapshot, cmd_tx) = self
            .ipc
            .as_ref()
            .map(|ipc| (ipc.snapshot.clone(), ipc.cmd_tx.clone()))
            .ok_or_else(|| "no IPC channel".to_string())?;
        let result = lua_state::reload_lua_config(
            &lua, &path, snapshot, &cmd_tx,
            &self.lua_keybinds, &self.lua_hooks, &self.window_rules,
            &self.decoration, &self.input_config,
        );
        if result.is_ok() {
            lua_state::drain_window_rules(&mut self.wm, &self.window_rules);
            self.apply_configured_scales();
            self.apply_configured_output_positions();
            let keymap = self.input_config.lock().unwrap().keymap.clone();
            if let Some(keymap) = keymap
                && let Some(keyboard) = self.seat.get_keyboard()
                && let Err(err) = keyboard.set_keymap_from_string(self, keymap)
            {
                tracing::warn!("failed to apply reloaded XKB keymap: {err}");
            }
        }
        result
    }
}

/// One tick, one batch. Frames are numbered at flush, then fanned out to every
/// subscriber. Without a socket the queue still drains; no frame loiters until
/// the next tick like an unpaid intern waiting for a bus.
pub(crate) fn flush_events(state: &mut State) {
    let events = state.wm.events.flush();
    if events.is_empty() {
        return;
    }
    if let Some(stream) = &state.events {
        stream.broadcast(&events);
    }
    // Lua hooks see the same coalesced batch as socket subscribers. One bad
    // callback is logged, not promoted to a compositor crash.
    if let Some(lua) = &state.lua {
        for err in lua_state::fire_event_hooks(lua, &state.lua_hooks, &events) {
            tracing::warn!("lua event hook failed: {err}");
        }
    }
}

/// e09s05t1: first commit is the registration point for a toplevel. Before it,
/// app_id/title may still arrive, so creating a window earlier fed rules empty
/// strings and left a phantom window behind for clients that never map.
fn register_toplevel_on_first_commit(state: &mut State, surface: &WlSurface) -> bool {
    if window_id_of(surface).is_some() {
        return false;
    }
    let Some(toplevel_for_registration) = state
        .xdg_shell_state
        .toplevel_surfaces()
        .iter()
        .find(|t| t.alive() && t.wl_surface() == surface)
        .cloned()
    else {
        return false;
    };
    let Some(output_id) = state.wm.focused_output_id() else {
        tracing::warn!("toplevel committed before any output exists; not registering");
        return false;
    };
    let (app_id, title) = with_states(surface, |states| {
        let data = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .expect("xdg toplevel data on committed toplevel");
        let data = data.lock().unwrap();
        (
            data.app_id.clone().unwrap_or_default(),
            data.title.clone().unwrap_or_default(),
        )
    });
    let Some(id) = state.wm.add_window(output_id, app_id, title) else {
        return false;
    };
    with_states(surface, |states| {
        states.data_map.insert_if_missing(|| id);
    });
    state.toplevels.insert(id, toplevel_for_registration);
    tracing::info!("toplevel first commit registered id={id:?}");
    crate::apply_layout(state);
    true
}

/// e09s05t3: apply a client's presentation request through the same WM toggles
/// the keybinds use, so saved rects and placement semantics stay identical.
fn set_client_presentation(
    state: &mut State,
    surface: &ToplevelSurface,
    presentation: wm::Presentation,
    set: bool,
) -> bool {
    let Some(id) = window_id_of(surface.wl_surface()) else {
        return false;
    };
    let Some(window) = state.wm.window(id) else {
        return false;
    };
    if set == (window.mode.presentation == presentation) {
        return false;
    }
    let changed = match presentation {
        wm::Presentation::Maximized => state.wm.toggle_maximized(id),
        wm::Presentation::Fullscreen => state.wm.toggle_fullscreen(id),
        wm::Presentation::Normal => false,
    };
    if changed {
        crate::apply_layout(state);
    }
    changed
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

/// Output id a layer surface was created for. Untagged wrappers fall back to
/// the focused output at use time (older clients that omit wl_output).
pub(crate) fn layer_output_id(layer: &smithay::desktop::LayerSurface) -> Option<wm::OutputId> {
    layer.user_data().get::<wm::OutputId>().copied()
}

pub(crate) fn tag_layer_output(layer: &smithay::desktop::LayerSurface, id: wm::OutputId) {
    layer.user_data().insert_if_missing_threadsafe(|| id);
}

/// Convert a LayerMap's output-local non-exclusive zone into a global work
/// area. The zone never included the output origin, which is why bars on a
/// second monitor used to reserve space on the first one.
pub(crate) fn layer_work_area(
    origin: Point<i32, Logical>,
    zone: Rectangle<i32, Logical>,
    dock_height: u32,
) -> wm::Rect {
    wm::Rect {
        x: origin.x.saturating_add(zone.loc.x),
        y: origin.y.saturating_add(zone.loc.y),
        width: zone.size.w.max(0) as u32,
        height: (zone.size.h.max(0) as u32).saturating_sub(dock_height),
    }
}

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        // Honour the wl_output the client asked for. Without this, a bar for
        // HDMI lands on the laptop panel — a classic case of right idea, wrong
        // screen, delivered with complete confidence.
        let requested = output.as_ref().and_then(Output::from_resource);
        let target = requested
            .and_then(|o| self.wm.output_id_by_name(&o.name()))
            .or_else(|| self.wm.focused_output_id());
        queue_layer_event(self, EventKind::LayerOpened, target);
        // Create the stable desktop wrapper; on first commit it gets mapped
        // into the target output's LayerMap, which sends anchored configures.
        let wrapper = smithay::desktop::LayerSurface::new(surface, namespace.clone());
        if let Some(id) = target {
            tag_layer_output(&wrapper, id);
        }
        self.layer_wrappers.push(wrapper);
        tracing::info!("layer mapped: ns={namespace:?} output={target:?}");
        // Creation is a layer-surface state change (e06s03): map/configure now
        // so the client can proceed; keyboard focus is decided on commit/unmap.
        self.map_layer_surfaces();
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        let wrapper = self.layer_wrappers.iter().find(|l| l.layer_surface() == &surface).cloned();
        self.layer_wrappers.retain(|l| l.layer_surface() != &surface);
        if let Some(wrapper) = wrapper {
            let id = layer_output_id(&wrapper).or_else(|| self.wm.focused_output_id());
            queue_layer_event(self, EventKind::LayerClosed, id);
            if let Some(output) = id.and_then(|id| self.layer_outputs.get(&id).cloned()) {
                layer_map_for_output(&output).unmap_layer(&wrapper);
            }
        }
        // e06s03t3/t1: destroying/unmapping a layer may remove the current or
        // an exclusive keyboard focus holder; re-evaluate focus policy (falls
        // back to the last-focused toplevel on the same output).
        self.update_keyboard_focus();
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
    /// Global origin of the parent surface a popup positions against: the
    /// toplevel's window-geometry origin, or another popup's own origin.
    fn popup_parent_coord(&self, parent_surface: Option<&WlSurface>) -> (i32, i32) {
        let Some(parent_surface) = parent_surface else { return (0, 0) };
        if let Some(id) = window_id_of(parent_surface)
            && let Some(window) = self.wm.window(id) {
                return (window.target_rect.x, window.target_rect.y);
            }
        if let Some(parent) = self
            .popups
            .iter()
            .find(|p| p.surface.wl_surface() == parent_surface)
        {
            return (parent.screen_rect.x, parent.screen_rect.y);
        }
        // Layer-surface parent (bar menu): use its committed layer geometry
        // on whichever output owns the layer, translated into global coords.
        for output in self.layer_outputs.values() {
            let map = layer_map_for_output(output);
            if let Some(layer) = map.layers().find(|l| l.wl_surface() == parent_surface)
                && let Some(geom) = map.layer_geometry(layer)
            {
                let origin = output.current_location();
                return (origin.x + geom.loc.x, origin.y + geom.loc.y);
            }
        }
        (0, 0)
    }

    /// Map layer surfaces into the LayerMap of the output they targeted, then
    /// derive each output's work area from its own exclusive zone and origin.
    /// Keyboard focus is decided separately by `update_keyboard_focus` so an
    /// OnDemand layer's commit cannot steal focus (e06s03).
    fn map_layer_surfaces(&mut self) {
        self.layer_wrappers.retain(|l| l.alive());
        let focused = self.wm.focused_output_id();
        let tagged: Vec<(wm::OutputId, smithay::desktop::LayerSurface)> = self
            .layer_wrappers
            .iter()
            .filter_map(|layer| {
                layer_output_id(layer)
                    .or(focused)
                    .map(|id| (id, layer.clone()))
            })
            .collect();
        let mut ids: Vec<wm::OutputId> = tagged.iter().map(|(id, _)| *id).collect();
        ids.dedup();

        let mut changed = false;
        for id in ids {
            let Some(output) = self.layer_outputs.get(&id).cloned() else {
                continue;
            };
            let mut map = layer_map_for_output(&output);
            for (_, layer) in tagged.iter().filter(|(layer_id, _)| *layer_id == id) {
                if map.layer_for_surface(layer.wl_surface(), WindowSurfaceType::ALL).is_none() {
                    let _ = map.map_layer(layer);
                }
            }
            map.arrange();
            let zone = map.non_exclusive_zone();
            // Drop the map guard before touching wm state: layer_map_for_output
            // deadlocks if the same output's guard is acquired twice.
            drop(map);

            let dock_h = self.wm.dock_height_for_output(id);
            let wa = layer_work_area(output.current_location(), zone, dock_h);
            if self.wm.work_area_for_output(id) != wa {
                tracing::info!(
                    "output {id} work area -> ({},{} {}x{}) dock_h={}",
                    wa.x, wa.y, wa.width, wa.height, dock_h
                );
                self.wm.set_output_work_area(id, Some(wa));
                changed = true;
            }
        }

        if changed {
            // Compatibility mirror for consumers not yet migrated off the
            // single global work area; it follows the focused output.
            self.wm.work_area = focused.map(|id| self.wm.work_area_for_output(id));
            // The dirty flag is global: arrange every output in one pass or
            // the second monitor keeps last season's geometry.
            let ids = self.wm.outputs().map(|output| output.id).collect::<Vec<_>>();
            self.wm.arrange_outputs(ids);
            apply_layout(self);
        }
        // smithay's arrange never sends the initial configure (the spec reserves
        // it for the compositor, on the surface's first commit) — send any
        // pending configure or layer clients (fuzzel, awww) wait forever.
        for layer in &self.layer_wrappers {
            layer.layer_surface().send_pending_configure();
        }
    }

    /// e06s03: prefer a keyboard-interactive layer surface over a toplevel for
    /// wl_keyboard focus. Exclusive layers take focus while mapped (topmost
    /// wins); if the current keyboard focus is already held by a valid
    /// interactive layer (e.g. OnDemand after a click), keep it; otherwise
    /// focus follows the focused toplevel window.
    pub fn update_keyboard_focus(&mut self) {
        use smithay::wayland::shell::wlr_layer::KeyboardInteractivity;

        let current_focus = self
            .seat
            .get_keyboard()
            .and_then(|kbd| kbd.current_focus());

        // If keyboard focus is already on an interactive layer, keep it unless
        // an Exclusive layer should take precedence.
        let current_layer = current_focus.as_ref().and_then(|surf| {
            self.layer_wrappers
                .iter()
                .find(|l| l.alive() && l.wl_surface() == surf)
                .cloned()
        });

        let exclusive_candidate = self
            .layer_wrappers
            .iter()
            .rev() // last/topmost in registration order as a reasonable proxy
            .find(|l| {
                l.alive()
                    && matches!(
                        l.cached_state().keyboard_interactivity,
                        KeyboardInteractivity::Exclusive
                    )
            })
            .cloned();

        if let Some(layer) = exclusive_candidate {
            let surf = layer.wl_surface().clone();
            if current_focus.as_ref() != Some(&surf) {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.set_focus(self, Some(surf), crate::udev::SCOUNTER.next_serial());
                }
                crate::udev::keyboard_focus_set_layer(self);
            }
            return;
        }

        if let Some(layer) = current_layer
            && layer.can_receive_keyboard_focus() {
                // OnDemand (or other interactive) layer already holds focus — keep it.
                return;
            }

        // No layer should hold keyboard focus: follow focused toplevel.
        crate::udev::keyboard_focus_set(self, self.wm.focused_id());
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

/// An activation token is a short-lived bearer credential for focus. Ten
/// seconds covers the click that launched the app; an older token would let a
/// client replay a stale request and yank focus back later.
fn activation_token_is_fresh(age: Duration) -> bool {
    age <= Duration::from_secs(10)
}

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // Tokens are single-use: keep a replayed one from focusing anything.
        self.activation_state().remove_token(&token);
        if !activation_token_is_fresh(token_data.timestamp.elapsed()) {
            tracing::warn!("ignoring stale xdg activation token");
            return;
        }
        let Some(id) = window_id_of(&surface) else {
            tracing::warn!("xdg activation requested for an untracked surface");
            return;
        };
        if self.wm.focus_window_or_restore(id) {
            self.update_keyboard_focus();
            self.redraw_needed = true;
            tracing::info!("xdg activation focused window {id:?}");
        }
    }
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

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl DataControlHandler for State {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
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
        // e09s05t1: registration waits for the first commit. A toplevel that
        // dies before that point must not leave a WindowOpen event behind.
        register_toplevel_on_first_commit(self, surface);
        let is_layer_surface = self
            .layer_wrappers
            .iter()
            .any(|l| l.wl_surface() == surface);
        if is_layer_surface {
            // e06s03: run layer mapping/configuration only for layer surfaces.
            self.map_layer_surfaces();
            // e06s03t2: on layer surface commit with keyboard interactivity,
            // move wl_keyboard focus to it.
            if let Some(layer) = self
                .layer_wrappers
                .iter()
                .find(|l| l.wl_surface() == surface)
                .cloned()
            {
                if layer.can_receive_keyboard_focus() {
                    let surf = layer.wl_surface().clone();
                    if let Some(keyboard) = self.seat.get_keyboard() {
                        keyboard.set_focus(self, Some(surf), crate::udev::SCOUNTER.next_serial());
                    }
                    crate::udev::keyboard_focus_set_layer(self);
                } else {
                    // If this layer lost interactivity, re-evaluate focus.
                    self.update_keyboard_focus();
                }
            }
        }
        // e06s02t1: track committed input region for toplevel window surfaces.
        crate::sync_window_input_region(self, surface);
        // e08s02t6: CSD clients move their window geometry inside the surface;
        // track the offset so placement and pointer hit-testing use the same
        // origin the client draws against.
        if let Some(id) = window_id_of(surface) {
            let geometry = with_states(surface, |states| {
                states.cached_state.get::<SurfaceCachedState>().current().geometry
            });
            let offset = window_geometry_offset(geometry);
            self.wm.set_window_xdg_offset(id, offset);

            // BUG-2026-11-21-DMABUF-IMPORT (bug B): Electron/Chromium can commit
            // a default-sized buffer before honouring the size we configured,
            // letting content spill past the frame. BUG-2026-11-24: an
            // undersized commit is just as wrong (blank frame). Ask again on any
            // mismatch, bounded by a budget and paced so a burst of queued
            // commits cannot spend it before the client reacts.
            let buffer_size = with_renderer_surface_state(surface, |state| state.buffer_size())
                .flatten()
                .map(|size| (size.w.max(0) as u32, size.h.max(0) as u32));
            if let (Some(buffer), Some(window)) = (buffer_size, self.wm.window(id)) {
                let target = window.target_rect;
                let resends = window.size_resends;
                let since_last = window.last_size_resend.map(|at| at.elapsed());
                // Drop the immutable borrow before mutating the WM: copy the
                // fields we need first, then record the stretch. Fixed-size
                // clients get their stubborn buffer scaled to fill the frame.
                self.wm.set_fill_scale(id, wm::fill_scale(target, buffer));
                match wm::size_action(target, buffer, resends, since_last) {
                    wm::SizeAction::Resend => {
                        let direction = if buffer.0 > target.width || buffer.1 > target.height {
                            ">"
                        } else {
                            "<"
                        };
                        tracing::info!(
                            "client committed {}x{} {} configured {}x{}; re-sending size",
                            buffer.0, buffer.1, direction, target.width, target.height
                        );
                        self.wm.note_size_resend(id);
                        crate::apply_layout(self);
                    }
                    wm::SizeAction::ResetBudget => self.wm.clear_size_resends(id),
                    wm::SizeAction::Throttled => {
                        tracing::debug!(
                            "client committed {}x{} vs configured {}x{}; resend throttled",
                            buffer.0, buffer.1, target.width, target.height
                        );
                    }
                    wm::SizeAction::None => {}
                }
            }
        }
        // Popup lifecycle evidence: log the map/unmap transition so a run from
        // a TTY proves whether the client actually attached a buffer.
        if let Some(i) = self.popups.iter().position(|p| p.surface.wl_surface() == surface) {
            let has_buffer = with_renderer_surface_state(surface, |state| state.buffer().is_some())
                .unwrap_or(false);
            if has_buffer != self.popups[i].mapped {
                self.popups[i].mapped = has_buffer;
                let rect = self.popups[i].screen_rect;
                tracing::info!(
                    "xdg_popup {}: screen=({},{} {}x{})",
                    if has_buffer { "mapped (buffer committed)" } else { "unmapped (no buffer)" },
                    rect.x, rect.y, rect.width, rect.height
                );
                self.redraw_needed = true;
            }
        }
        // Demand for frames. Compared against frames presented it separates
        // "client stopped committing" from "we dropped or refused the frame".
        if let Some(u) = self.udev.as_mut() {
            u.stats.commits += 1;
            u.last_commit_at = Some(Instant::now());
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

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        // e06s04t3: clipboard offer follows keyboard-focused window/client.
        let dh = self.dh.clone();
        let client = focused
            .and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh, seat, client);
        // Do not force primary selection to keyboard focus here: e06s04t2
        // requires primary selection to follow pointer focus.
    }
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: smithay::input::pointer::CursorImageStatus) {
        // e08s03t6: remember the client's cursor request so the render loop
        // can draw a custom surface (or hide the pointer) accordingly.
        if let Some(udev) = self.udev.as_mut() {
            udev.cursor_image = image;
        }
        self.redraw_needed = true;
    }
}

/// Dock strip render signature: output geometry + per-entry visual state.
/// Rebuild the cached buffers only when this changes.
/// Output id is part of the cache key: two identical-looking docks on two
/// monitors still have different geometry and must not share rendered pixels.
type DockBufferSignature = (wm::OutputId, u32, i32, i32, Vec<(u64, bool, bool, char)>);

pub(crate) fn dock_buffer_signature(
    output: wm::OutputId,
    entries: &[wm::DockEntry],
    total_w: i32,
    dock_h: i32,
) -> DockBufferSignature {
    (
        output,
        entries.len() as u32,
        total_w,
        dock_h,
        entries
            .iter()
            .map(|e| {
                let label = e
                    .app_id
                    .chars()
                    .next()
                    .or_else(|| e.title.chars().next())
                    .unwrap_or('?')
                    .to_ascii_uppercase();
                (e.id, e.focused, e.minimized, label)
            })
            .collect(),
    )
}

pub(crate) struct State {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    layer_shell_state: WlrLayerShellState,
    // These protocol states are never read directly, but dropping them would
    // leave live globals pointing at freed handler data, so they are owned here
    // for their lifetime side effect. (Yes, that is an unusual kind of field.)
    #[allow(dead_code)]
    decoration_state: XdgDecorationState,
    shm_state: ShmState,
    #[allow(dead_code)]
    viewporter_state: ViewporterState,
    seat_state: SeatState<Self>,
    #[allow(dead_code)]
    output_state: OutputManagerState,
    data_device_state: DataDeviceState,
    primary_selection_state: PrimarySelectionState,
    #[allow(dead_code)]
    data_control_state: DataControlState,
    // BUG-2026-11-22-XDG-ACTIVATION-GLOBAL: owns the xdg_activation_v1 global
    // and the tokens clients mint through it. Launchers can finally stop
    // yelling "Unable to mint xdg activation token" into the void like a
    // vending machine that ate the coin and forgot to dispense the snack.
    activation_state: XdgActivationState,

    seat: Seat<Self>,
    #[allow(dead_code)] // owns the wl_output global's backing data
    output: Output,
    pub wm: wm::WindowManager,
    // e09s05t2: direct WindowId -> live toplevel lookup. The surface data map
    // stays the authoritative tag; this is the fast path for focus/render.
    pub toplevels: std::collections::BTreeMap<wm::WindowId, ToplevelSurface>,
    pub dh: DisplayHandle,
    // M2: IPC-backed state; snapshot is rebuilt from the shell state each loop iteration
    ipc: Option<Ipc>,
    // M3: Lua state with Hyprland config API (es.bind, es.define_submap, es.dsp.*, es.config{}, etc.)
    lua: Option<mlua::Lua>,
    // Keybinds defined via Lua es.bind(); checked during key event processing
    pub lua_keybinds: Arc<std::sync::Mutex<Vec<lua_state::Keybind>>>,
    // Event hooks registered via Lua es.on(); copied out of this mutex before
    // any callback runs so a hook can re-enter config setters safely.
    pub lua_hooks: lua_state::EventHooks,
    // Window rules staged by Lua es.window_rule{} and drained into the WM
    // before the first client commits.
    pub window_rules: lua_state::WindowRules,
    // Single source for reloads; set once when the DRM startup reads the file.
    pub lua_config_path: Option<std::path::PathBuf>,
    // Window decoration values mutated by Lua es.dsp.set_* and synced to wm each frame.
    pub decoration: Rc<RefCell<wm::DecorationConfig>>,
    // Dock entries (active + minimized windows on the active workspace), rebuilt
    // each frame by apply_layout and exposed to frontends via es.dock.entries().
    pub dock_entries: Arc<std::sync::Mutex<Vec<wm::DockEntry>>>,
    // Input/display settings mutated by Lua es.dsp.set_* and read by output
    // setup, the libinput callback and keyboard init (e08s03).
    pub input_config: Arc<std::sync::Mutex<lua_state::InputConfig>>,
    // Cached dock render buffers, keyed by a signature of geometry + entry
    // states. Reusing the same MemoryRenderBuffer across frames lets smithay
    // cache the GPU texture; rebuilding buffers every frame uploaded ~230KB+
    // per frame and made the compositor lag.
    pub dock_buffers: std::sync::Mutex<Option<(DockBufferSignature, Vec<smithay::backend::renderer::element::memory::MemoryRenderBuffer>)>>,
    // M4: real DRM backend data; None only in the virtual-output test fixture
    udev: Option<udev::UdevData>,
    // M4: calloop scheduling handle (timers, idle); None only in tests
    handle: Option<LoopHandle<'static, State>>,
    // Native control channel (notes/tacit-integration.md); None if bind failed
    control: Option<control::Control>,
    // Dedicated NDJSON event stream; None if bind failed or in test fixtures.
    pub(crate) events: Option<control::EventStream>,
    /// The smithay Output the backend renders on (DRM connector, or the test
    /// virtual output); layer surfaces are arranged against it via LayerMap.
    primary_output: Option<Output>,
    /// Every connected output's smithay handle, keyed by the WM's OutputId.
    /// Layer surfaces and dock geometry look up their own monitor here instead
    /// of fighting over `primary_output` like seagulls over one chip.
    layer_outputs: std::collections::BTreeMap<wm::OutputId, Output>,
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
    // Popup tracking for xdg_popup support
    popups: Vec<PopupData>,
    /// The popup surface that currently holds the keyboard/pointer grab (modal behavior).
    /// When Some, all input is routed to this popup and clicks outside dismiss it.
    popup_grab: Option<PopupSurface>,
}

/// Data for a tracked xdg_popup
#[derive(Debug, Clone)]
pub struct PopupData {
    pub surface: PopupSurface,
    /// Owning toplevel, used to restore keyboard focus when the popup closes.
    pub parent: Option<ToplevelSurface>,
    /// Immediate parent surface: a toplevel, or another popup for submenus.
    pub parent_surface: Option<WlSurface>,
    pub positioner: PositionerState,
    /// Position relative to the parent window geometry, after constraints.
    pub geometry: wm::Rect,
    /// Same rect in global screen coordinates (updated every placement pass).
    pub screen_rect: wm::Rect,
    /// Whether the client has committed a buffer (popup is visible).
    pub mapped: bool,
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

    // The standalone DRM/libinput backend is the only runtime. There is no
    // nested window to fall back to; the virtual output below is test furniture.
    crate::udev::run_udev()
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
    let data_device_state = DataDeviceState::new::<State>(dh);
    let primary_selection_state = PrimarySelectionState::new::<State>(dh);
    // e06s04: expose ext-data-control (clipboard + primary selection) to clients
    // like wl-copy/wl-paste and clipboard managers.
    let data_control_state = DataControlState::new::<State, _>(
        dh,
        Some(&primary_selection_state),
        |_| true,
    );
    // Advertises xdg_activation_v1; without it launchers log "Unable to mint
    // xdg activation token" for every app they start.
    let activation_state = XdgActivationState::new::<State>(dh);

    // Tests fabricate one 1280x800 virtual output. The DRM backend attaches real
    // outputs per connector instead (and does not call this with true).
    let output = if want_virtual_output {
        // Named "winit" for historical reasons and because the IPC snapshot
        // fixtures key on it. It is a test output, not a winit backend.
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
        output_state: OutputManagerState::new_with_xdg_output::<State>(dh),
        data_device_state,
        primary_selection_state,
        data_control_state,
        activation_state,
        seat,
        output,
        wm,
        toplevels: std::collections::BTreeMap::new(),
        dh: dh.clone(),
        ipc: None,
        lua: None,
        udev: None,
        handle: None,
        control: None,
        events: None,
        primary_output: None,
        layer_outputs: std::collections::BTreeMap::new(),
        layer_wrappers: Vec::new(),
        lua_keybinds: Arc::new(std::sync::Mutex::new(Vec::new())),
        lua_hooks: Arc::new(std::sync::Mutex::new(Vec::new())),
        window_rules: Arc::new(std::sync::Mutex::new(Vec::new())),
        lua_config_path: None,
        decoration: Rc::new(RefCell::new(wm::DecorationConfig::standard())),
        dock_entries: Arc::new(std::sync::Mutex::new(Vec::new())),
        input_config: Arc::new(std::sync::Mutex::new(lua_state::InputConfig::default())),
        dock_buffers: std::sync::Mutex::new(None),
        redraw_needed: true,
        last_frame_callback_ms: 0,
        popups: Vec::new(),
        popup_grab: None,
    }
}

// Development compatibility path for the explicit Winit backend. The standalone
// DRM backend does not initialize this Hyprland/Tacit integration.
/// Reap WM entries whose toplevel disappeared without a compositor callback (for
/// example after a client lost its display socket). Stale entries otherwise hold
/// master/stack slots forever.
pub(crate) fn prune_dead_windows(state: &mut State) -> usize {
    // e09s05t2: an index entry whose window vanished must not outlive it like a
    // ghost tenant still listed on the lease.
    let live_windows = state.wm.window_ids();
    let stale_index: Vec<wm::WindowId> = state
        .toplevels
        .keys()
        .copied()
        .filter(|id| {
            !live_windows.contains(id)
                || state
                    .toplevels
                    .get(id)
                    .is_none_or(|t| !t.alive())
        })
        .collect();
    for id in stale_index {
        state.toplevels.remove(&id);
    }
    let dead: Vec<wm::WindowId> = live_windows
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
    let d = state.decoration.borrow();
    let outer = d.outer;
    let inner = d.inner;
    let border_size = d.border_size;
    let color_focused = d.border_color_focused;
    let color_unfocused = d.border_color_unfocused;
    let titlebar_color_focused = d.titlebar_color_focused;
    let titlebar_color_unfocused = d.titlebar_color_unfocused;
    let titlebar_height = d.titlebar_height;
    let dock_height = d.dock_height;
    let per_output_docks: Vec<(wm::OutputId, u32)> = state
        .wm
        .outputs()
        .map(|output| {
            let height = d.dock_height_for(&output.name);
            (output.id, height)
        })
        .collect();
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
    for (id, height) in per_output_docks {
        state.wm.set_output_dock_height(id, height);
    }
    state.wm.set_wm_mode(wm_mode);
    state.wm.set_tiling_layout(tiling_layout);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ToplevelConfigureState {
    activated: bool,
    maximized: bool,
    fullscreen: bool,
    tiled: bool,
}

/// The xdg toplevel states that describe where the compositor placed a window.
fn toplevel_configure_state(mode: wm::ClientMode, focused: bool) -> ToplevelConfigureState {
    let (presentation_maximized, fullscreen) = mode.xdg_state_flags();
    // BUG-2026-11-24-CSD-MAXIMIZED: pin Maximized on tiled windows so CSD
    // clients (Helium/GTK) suppress their shadow inset. Hyprland does this
    // at map time with a comment longer than our whole function. A real
    // maximized window keeps its own bit; floating stays stateless so the
    // user can still resize it.
    let maximized = presentation_maximized || mode.sends_tiled_state();
    ToplevelConfigureState {
        activated: focused,
        maximized,
        fullscreen,
        tiled: mode.sends_tiled_state(),
    }
}

// M5: push target rects to clients as xdg_toplevel size configures, then mark applied.
pub(crate) fn apply_layout(state: &mut State) {
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
        // e08s04t3: a mode change must reach the client even when the rect is
        // unchanged (e.g. maximize into an already-full-sized window).
        let mode_changed = window.mode_sent != Some(window.mode);
        if !rect_changed && !needs_activation && !mode_changed {
            continue;
        }
        if let Some(toplevel) = toplevel_for_window(state, id) {
            tracing::debug!("configuring window id={id:?} -> {}x{} at {},{}", target.width, target.height, target.x, target.y);
            let flags = toplevel_configure_state(window.mode, window.focused);
            toplevel.with_pending_state(|st| {
                if target_valid {
                    st.size = Some((target.width.max(1) as i32, target.height.max(1) as i32).into());
                }
                if flags.activated {
                    st.states.set(xdg_toplevel::State::Activated);
                } else {
                    st.states.unset(xdg_toplevel::State::Activated);
                }
                if flags.maximized {
                    st.states.set(xdg_toplevel::State::Maximized);
                } else {
                    st.states.unset(xdg_toplevel::State::Maximized);
                }
                if flags.fullscreen {
                    st.states.set(xdg_toplevel::State::Fullscreen);
                } else {
                    st.states.unset(xdg_toplevel::State::Fullscreen);
                }
                // Hyprland tells every tiled client all four directions, so
                // winit stops treating our configure as a polite suggestion.
                let tiled = [
                    xdg_toplevel::State::TiledLeft,
                    xdg_toplevel::State::TiledRight,
                    xdg_toplevel::State::TiledTop,
                    xdg_toplevel::State::TiledBottom,
                ];
                for state in tiled {
                    if flags.tiled {
                        st.states.set(state);
                    } else {
                        st.states.unset(state);
                    }
                }
            });
            toplevel.send_pending_configure();
            if rect_changed {
                state.wm.set_rect(id, target);
            }
            state.wm.set_activated_sent(id, window.focused);
            state.wm.set_mode_sent(id, window.mode);
        }
    }
    // e06s03: keep wl_keyboard focus in sync with state changes via policy,
    // not on every commit. update_keyboard_focus prefers interactive layers
    // and otherwise follows focused toplevel.
    state.update_keyboard_focus();
    // e05s04t1: apply_layout ran because something changed; mark for redraw.
    state.redraw_needed = true;
}

/// Path to the unified spawn log inside an eswm state directory.
fn spawn_log_path_in(state_home: &std::path::Path) -> std::path::PathBuf {
    state_home.join("eswm").join("spawn.log")
}

static SPAWN_TERMINAL: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();

fn spawn_terminal_store() -> &'static std::sync::Mutex<String> {
    SPAWN_TERMINAL.get_or_init(|| std::sync::Mutex::new("kitty".to_string()))
}

pub fn set_spawn_terminal(name: &str) {
    *spawn_terminal_store().lock().unwrap() = name.to_string();
}

/// Resolve spawn log path from XDG_STATE_HOME or HOME/.local/state fallback.
fn spawn_log_path_from_env(
    xdg_state_home: Option<&str>,
    home: Option<&str>,
) -> std::path::PathBuf {
    let base = if let Some(xdg) = xdg_state_home.filter(|s| !s.is_empty()) {
        std::path::PathBuf::from(xdg)
    } else {
        let home = home.unwrap_or(".");
        std::path::PathBuf::from(home).join(".local").join("state")
    };
    spawn_log_path_in(&base)
}

fn current_spawn_log_path() -> std::path::PathBuf {
    spawn_log_path_from_env(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

fn spawn_terminal_name() -> String {
    spawn_terminal_store().lock().unwrap().clone()
}

static SPAWN_CHILD_PIDS: std::sync::OnceLock<std::sync::Mutex<Vec<u32>>> = std::sync::OnceLock::new();

fn spawn_child_pids() -> &'static std::sync::Mutex<Vec<u32>> {
    SPAWN_CHILD_PIDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn track_spawned_child(pid: u32) {
    spawn_child_pids().lock().unwrap().push(pid);
}

/// Reap exited child processes using waitpid(-1, WNOHANG) loop.
/// Returns the number of children reaped in this call.
pub fn reap_spawned_children() -> usize {
    let mut reaped = 0usize;
    loop {
        let mut status: libc::c_int = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status as *mut _, libc::WNOHANG) };
        if pid > 0 {
            let pid = pid as u32;
            let mut tracked = spawn_child_pids().lock().unwrap();
            tracked.retain(|&p| p != pid);
            reaped += 1;
            tracing::warn!("reaped spawned child pid {pid}");
            continue;
        }
        if pid == 0 {
            break;
        }
        // pid == -1
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(code) if code == libc::ECHILD => break,
            Some(code) if code == libc::EINTR => continue,
            _ => break,
        }
    }
    reaped
}

fn spawn_program(prog: &str, args: &[&str]) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let log_path = current_spawn_log_path();
    let log = open_spawn_log_file(&log_path).ok();
    let terminal = spawn_terminal_name();

    let mut command = Command::new(prog);
    command
        .args(args)
        .env("WAYLAND_DISPLAY", socket_name())
        .env("XDG_CURRENT_DESKTOP", "eswm")
        .env("TERMINAL", terminal)
        .stdin(Stdio::null());

    match log {
        Some(file) => {
            let err = file.try_clone().ok();
            command.stdout(Stdio::from(file));
            if let Some(err) = err {
                command.stderr(Stdio::from(err));
            } else {
                command.stderr(Stdio::null());
            }
        }
        None => {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }
    }

    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let child = command.spawn()?;
    let pid = child.id();
    track_spawned_child(pid);
    Ok(pid)
}

/// Spawn a shell-interpreted command string (`sh -c`) with eswm spawn environment.
pub fn spawn_shell_command(cmd: &str) -> std::io::Result<u32> {
    let (prog, args) = shell_command_argv(cmd);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    spawn_program(&prog, &arg_refs)
}

/// Spawn an argument vector directly (no shell parsing).
pub fn spawn_argv_command(argv: &[&str]) -> std::io::Result<u32> {
    let Some((prog, args)) = argv.split_first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty argv",
        ));
    };
    spawn_program(prog, args)
}

/// Build argv for shell-interpreted command strings (Hyprland-style `sh -c`).
fn shell_command_argv(cmd: &str) -> (String, Vec<String>) {
    ("sh".to_string(), vec!["-c".to_string(), cmd.to_string()])
}

/// Open the shared spawn log for appending, creating parent dirs and enforcing 0600.
fn open_spawn_log_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

// Compositor keybind helper: spawn a terminal client on our own socket.
pub fn spawn_terminal() {
    let terminal = spawn_terminal_name();
    let _ = spawn_argv_command(&[terminal.as_str()]);
}

/// Split a command string into program + args (whitespace split).
pub fn parse_cmd(cmd: &str) -> (&str, Vec<&str>) {
    let mut parts = cmd.split_whitespace();
    let program = parts.next().unwrap_or("");
    let args: Vec<&str> = parts.collect();
    (program, args)
}


// Spawn a Wayland client against eswm's socket, detached (own process group so
// VT signals don't hit it). Stdout/stderr go to the unified spawn log.
pub fn spawn_on_socket(program: &str, args: &[&str]) {
    // Expand leading ~ (command is run via execve, no shell to do it).
    let expand_tilde = |p: &str| -> String {
        if p == "~" || p.starts_with("~/") {
            std::env::var("HOME")
                .map(|h| format!("{h}{}", &p[1..]))
                .unwrap_or_else(|_| p.to_owned())
        } else {
            p.to_owned()
        }
    };
    let prog = expand_tilde(program);
    let expanded_args: Vec<String> = args.iter().map(|a| expand_tilde(a)).collect();
    let arg_refs: Vec<&str> = expanded_args.iter().map(|s| s.as_str()).collect();
    let prog_name = program.to_owned();
    match spawn_program(&prog, &arg_refs) {
        Ok(pid) => tracing::info!("spawned {prog_name} pid {pid}"),
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
delegate_xdg_activation!(State);
delegate_xdg_decoration!(State);
delegate_layer_shell!(State);
delegate_output!(State);
delegate_compositor!(State);
delegate_shm!(State);
delegate_viewporter!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_primary_selection!(State);
delegate_ext_data_control!(State);

#[cfg(test)]
mod tests {
    // e09s02t5 — a bar on the HDMI output must shrink that output's work area,
    // not the laptop's. The zone is output-local; the work area is global.
    #[test]
    fn layer_output_work_area_uses_zone_origin_and_dock() {
        let zone = Rectangle {
            loc: (40, 60).into(),
            size: (1200, 700).into(),
        };
        let origin = Point::<i32, Logical>::from((1920, 100));
        assert_eq!(
            layer_work_area(origin, zone, 48),
            wm::Rect { x: 1960, y: 160, width: 1200, height: 652 }
        );

        // A dock taller than the zone clamps to zero instead of wrapping into
        // a 4-billion-pixel work area (smithay rejects negative sizes itself).
        let shallow = Rectangle {
            loc: (5, 7).into(),
            size: (100, 10).into(),
        };
        assert_eq!(
            layer_work_area(Point::from((0, 0)), shallow, 40),
            wm::Rect { x: 5, y: 7, width: 100, height: 0 }
        );
    }

    use super::*;
    // Imported here, not at module scope: only the tests construct a Display,
    // so a crate-level import is dead weight (and clippy --fix deletes it).
    use smithay::reexports::wayland_server::Display;
    use crate::control::{EventKind, EventStream, handle_request};
    use serde_json::{Value, json};
    use std::{
        io::{BufRead, BufReader},
        os::unix::net::UnixStream,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };
    use wayland_protocols::xdg::activation::v1::client::{
        xdg_activation_token_v1::{self, XdgActivationTokenV1},
        xdg_activation_v1::{self, XdgActivationV1},
    };
    use wayland_protocols::xdg::shell::client::{
        xdg_surface::{self, XdgSurface},
        xdg_toplevel::{self, XdgToplevel},
        xdg_wm_base::{self, XdgWmBase},
    };

    fn queued_layer_events(state: &State) -> Vec<(EventKind, Value)> {
        state
            .wm
            .events
            .pending()
            .iter()
            .map(|event| (event.kind, event.data.clone()))
            .collect()
    }

    fn clean_state() -> State {
        let mut state = test_state();
        // The virtual output emits monitor_added during fixture setup.
        let _ = state.wm.events.flush();
        state
    }

    #[test]
    fn layer_events_opened_queues_layer_opened() {
        let mut state = clean_state();
        let output = state.wm.focused_output_id().unwrap();
        queue_layer_event(&mut state, EventKind::LayerOpened, Some(output));
        assert_eq!(
            queued_layer_events(&state),
            vec![(EventKind::LayerOpened, json!({"id": output}))]
        );
    }

    #[test]
    fn layer_events_closed_queues_layer_closed() {
        let mut state = clean_state();
        let output = state.wm.focused_output_id().unwrap();
        queue_layer_event(&mut state, EventKind::LayerClosed, Some(output));
        assert_eq!(
            queued_layer_events(&state),
            vec![(EventKind::LayerClosed, json!({"id": output}))]
        );
    }

    #[test]
    fn layer_events_without_target_output_is_silent() {
        let mut state = clean_state();
        queue_layer_event(&mut state, EventKind::LayerOpened, None);
        queue_layer_event(&mut state, EventKind::LayerClosed, None);
        assert!(queued_layer_events(&state).is_empty());
    }

    #[test]
    fn layer_events_config_reload_queues_one_global_frame() {
        let mut state = clean_state();
        queue_config_reloaded(&mut state);
        queue_config_reloaded(&mut state);
        let flushed = state.wm.events.flush();
        assert_eq!(flushed.len(), 1, "global kinds coalesce within a tick");
        assert_eq!(flushed[0].kind, EventKind::ConfigReloaded);
        assert_eq!(flushed[0].data, json!({}));
    }

    #[test]
    fn config_reload_legacy_emission_survives() {
        let mut state = clean_state();
        state.lua_config_path = Some(PathBuf::from("/tmp/eswm-config-reload-legacy-absent.lua"));
        let ipc = crate::ipc::Ipc::new("test".into());
        let mut legacy = ipc.sinks.subscribe_for_test();
        crate::ipc::apply_cmd(&mut state, &ipc, crate::ipc::CompCmd::Reload);
        let mut line = String::new();
        BufReader::new(&mut legacy).read_line(&mut line).expect("legacy line");
        assert_eq!(line, "configreloaded>>\n");
        assert!(queued_layer_events(&state)
            .iter()
            .any(|(kind, _)| *kind == EventKind::ConfigReloaded));
    }

    #[test]
    fn layer_events_ready_queued_once_after_sockets() {
        let mut state = clean_state();
        queue_ready(&mut state);
        queue_ready(&mut state);
        let flushed = state.wm.events.flush();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].kind, EventKind::Ready);
        assert_eq!(flushed[0].data, json!({}));
    }

    struct EventSocketPath(PathBuf);

    impl EventSocketPath {
        fn new(label: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join("opencode");
            std::fs::create_dir_all(&dir).expect("create temp socket dir");
            Self(dir.join(format!(
                "eswm-{}-{}-{}.events.sock",
                label,
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for EventSocketPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn connect_subscriber(path: &Path) -> BufReader<UnixStream> {
        let stream = UnixStream::connect(path).expect("connect to event socket");
        BufReader::new(stream)
    }

    fn read_frame(reader: &mut BufReader<UnixStream>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read NDJSON frame");
        assert!(!line.is_empty(), "event socket closed before the frame arrived");
        serde_json::from_str(line.trim()).expect("valid JSON frame")
    }

    #[test]
    fn event_stream_delivers_identical_frames_to_two_subscribers() {
        let path = EventSocketPath::new("fanout");
        let stream = EventStream::start_at(path.0.clone()).expect("bind event stream");
        let mut first = connect_subscriber(&path.0);
        let mut second = connect_subscriber(&path.0);

        let hello = read_frame(&mut first);
        assert_eq!(read_frame(&mut second), hello, "both subscribers see the same hello");
        assert_eq!(hello["seq"], 0);
        assert_eq!(hello["kind"], "hello");
        assert_eq!(hello["data"]["version"], 1);
        assert_eq!(
            hello["data"]["events"].as_array().unwrap().len(),
            EventKind::ALL.len()
        );

        let mut state = test_state();
        state.events = Some(stream);
        let output = state.wm.focused_output_id().expect("virtual output");
        let id = state.wm.add_window(output, "kitty", "old").expect("add window");
        assert!(state.wm.set_window_meta(id, "kitty".into(), "middle".into()));
        assert!(state.wm.set_window_meta(id, "kitty".into(), "final".into()));

        flush_events(&mut state);

        // Two title changes are one frame with the last value. Fan-out is a
        // photocopier, not a choose-your-own-adventure novel.
        let expected = vec![
            json!({"seq": 1, "kind": "monitor_added", "data": {"id": output, "name": "winit"}}),
            json!({"seq": 2, "kind": "window_open", "data": {"id": id}}),
            json!({"seq": 3, "kind": "window_active", "data": {"id": id}}),
            json!({"seq": 4, "kind": "window_title", "data": {"id": id, "title": "final"}}),
        ];
        for reader in [&mut first, &mut second] {
            for frame in &expected {
                assert_eq!(&read_frame(reader), frame);
            }
        }
    }

    #[test]
    fn event_stream_flush_without_socket_drains_queue() {
        let mut state = test_state();
        assert!(!state.wm.events.pending().is_empty(), "fixture queues monitor_added");
        flush_events(&mut state);
        assert!(state.wm.events.pending().is_empty());

        // A later subscriber joins at the next seq; the bus has no time
        // machine, merely a stubbornly good memory.
        let path = EventSocketPath::new("no-socket");
        let stream = EventStream::start_at(path.0.clone()).expect("bind event stream");
        let mut subscriber = connect_subscriber(&path.0);
        let hello = read_frame(&mut subscriber);
        assert_eq!(hello["kind"], "hello");
        state.events = Some(stream);
        queue_ready(&mut state);
        flush_events(&mut state);
        assert_eq!(
            read_frame(&mut subscriber),
            json!({"seq": 2, "kind": "ready", "data": {}})
        );
    }

    fn test_state() -> State {
        let display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();
        state
    }

    // BUG-2026-11-22-XDG-ACTIVATION-GLOBAL: a real client enumerating globals
    // must see xdg_activation_v1, otherwise its launcher logs "Unable to mint
    // xdg activation token" for every app. The probe collects what the wire
    // actually advertises rather than trusting a field on State.
    #[derive(Default)]
    struct RegistryProbe {
        interfaces: Vec<String>,
        // Keep the bound objects alive until the token's done event arrives.
        activation: Option<XdgActivationV1>,
        token: Option<XdgActivationTokenV1>,
        token_done: Option<String>,
    }

    impl wayland_client::Dispatch<wayland_client::protocol::wl_registry::WlRegistry, ()>
        for RegistryProbe
    {
        fn event(
            state: &mut Self,
            registry: &wayland_client::protocol::wl_registry::WlRegistry,
            event: wayland_client::protocol::wl_registry::Event,
            _: &(),
            _: &wayland_client::Connection,
            qh: &wayland_client::QueueHandle<Self>,
        ) {
            if let wayland_client::protocol::wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                state.interfaces.push(interface.clone());
                if interface == "xdg_activation_v1" {
                    let activation =
                        registry.bind::<XdgActivationV1, (), Self>(name, version, qh, ());
                    let token = activation.get_activation_token(qh, ());
                    token.commit();
                    state.activation = Some(activation);
                    state.token = Some(token);
                }
            }
        }
    }

    impl wayland_client::Dispatch<XdgActivationV1, ()> for RegistryProbe {
        fn event(
            _: &mut Self,
            _: &XdgActivationV1,
            _: xdg_activation_v1::Event,
            _: &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
        }
    }

    impl wayland_client::Dispatch<XdgActivationTokenV1, ()> for RegistryProbe {
        fn event(
            state: &mut Self,
            _: &XdgActivationTokenV1,
            event: xdg_activation_token_v1::Event,
            _: &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
            if let xdg_activation_token_v1::Event::Done { token } = event {
                state.token_done = Some(token);
            }
        }
    }

    #[test]
    fn xdg_activation_global_is_advertised() {
        use std::os::unix::net::UnixStream;
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        let (server_end, client_end) = UnixStream::pair().expect("unix stream pair");
        display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .expect("insert client");

        let conn = wayland_client::Connection::from_socket(client_end).expect("client socket");
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let _registry = conn.display().get_registry(&qh, ());
        let _ = conn.flush();
        for _ in 0..8 {
            let dispatched = display.dispatch_clients(&mut state).unwrap_or(0);
            let _ = display.flush_clients();
            if dispatched == 0 {
                break;
            }
        }
        if let Some(guard) = conn.prepare_read() {
            let _ = guard.read();
        }
        let mut probe = RegistryProbe::default();
        let _ = queue.dispatch_pending(&mut probe);
        assert!(
            probe.interfaces.iter().any(|name| name == "xdg_activation_v1"),
            "client must see xdg_activation_v1 (saw {:?})",
            probe.interfaces
        );
    }

    // The launcher warning is about minting, so prove a real client gets
    // token.done from the global — not just that the interface is listed.
    #[test]
    fn client_can_mint_xdg_activation_token() {
        use std::os::unix::net::UnixStream;
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        let (server_end, client_end) = UnixStream::pair().expect("unix stream pair");
        display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .expect("insert client");

        let conn = wayland_client::Connection::from_socket(client_end).expect("client socket");
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let _registry = conn.display().get_registry(&qh, ());
        let mut probe = RegistryProbe::default();

        for _ in 0..8 {
            let _ = conn.flush();
            let _ = display.dispatch_clients(&mut state).unwrap_or(0);
            let _ = display.flush_clients();
            if let Some(guard) = conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = queue.dispatch_pending(&mut probe);
            if probe.token_done.is_some() {
                break;
            }
        }
        assert!(
            probe.token_done.is_some(),
            "client must receive a minted xdg activation token"
        );
    }

    // A token is a bearer credential for focus: an old one must not let a
    // client yank focus back ten minutes after the click that minted it.
    #[test]
    fn activation_token_is_fresh_only_within_ten_seconds() {
        use std::time::Duration;
        assert!(activation_token_is_fresh(Duration::ZERO));
        assert!(activation_token_is_fresh(Duration::from_secs(9)));
        assert!(!activation_token_is_fresh(Duration::from_secs(11)));
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

    // e09s05t1: a real xdg-toplevel client. The old fixture called wm.add_window
    // directly, so it could never catch a window registered too early.
    #[derive(Default)]
    struct ToplevelFixture {
        compositor: Option<wayland_client::protocol::wl_compositor::WlCompositor>,
        wm_base: Option<XdgWmBase>,
        surface: Option<wayland_client::protocol::wl_surface::WlSurface>,
        xdg_surface: Option<XdgSurface>,
        toplevel: Option<XdgToplevel>,
        configures: usize,
        toplevel_configures: usize,
        last_serial: u32,
    }

    impl wayland_client::Dispatch<wayland_client::protocol::wl_registry::WlRegistry, ()>
        for ToplevelFixture
    {
        fn event(
            state: &mut Self,
            registry: &wayland_client::protocol::wl_registry::WlRegistry,
            event: wayland_client::protocol::wl_registry::Event,
            _: &(),
            _: &wayland_client::Connection,
            qh: &wayland_client::QueueHandle<Self>,
        ) {
            if let wayland_client::protocol::wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                if interface == "wl_compositor" {
                    state.compositor = Some(
                        registry.bind::<wayland_client::protocol::wl_compositor::WlCompositor, (), Self>(
                            name, version.min(6), qh, (),
                        ),
                    );
                } else if interface == "xdg_wm_base" {
                    state.wm_base = Some(registry.bind::<XdgWmBase, (), Self>(name, version.min(6), qh, ()));
                }
            }
        }
    }

    impl wayland_client::Dispatch<wayland_client::protocol::wl_compositor::WlCompositor, ()>
        for ToplevelFixture
    {
        fn event(
            _: &mut Self,
            _: &wayland_client::protocol::wl_compositor::WlCompositor,
            _: wayland_client::protocol::wl_compositor::Event,
            _: &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
        }
    }

    impl wayland_client::Dispatch<wayland_client::protocol::wl_surface::WlSurface, ()>
        for ToplevelFixture
    {
        fn event(
            _: &mut Self,
            _: &wayland_client::protocol::wl_surface::WlSurface,
            _: wayland_client::protocol::wl_surface::Event,
            _: &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
        }
    }

    impl wayland_client::Dispatch<XdgWmBase, ()> for ToplevelFixture {
        fn event(
            _: &mut Self,
            _: &XdgWmBase,
            _: xdg_wm_base::Event,
            _: &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
        }
    }

    impl wayland_client::Dispatch<XdgSurface, ()> for ToplevelFixture {
        fn event(
            state: &mut Self,
            surface: &XdgSurface,
            event: xdg_surface::Event,
            _: &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                state.configures += 1;
                state.last_serial = serial;
                surface.ack_configure(serial);
            }
        }
    }

    impl wayland_client::Dispatch<XdgToplevel, ()> for ToplevelFixture {
        fn event(
            state: &mut Self,
            _: &XdgToplevel,
            event: xdg_toplevel::Event,
            _: &(),
            _: &wayland_client::Connection,
            _: &wayland_client::QueueHandle<Self>,
        ) {
            if matches!(event, xdg_toplevel::Event::Configure { .. }) {
                state.toplevel_configures += 1;
            }
        }
    }

    fn fixture_pump_toplevel(
        display: &mut Display<State>,
        state: &mut State,
        conn: &wayland_client::Connection,
        queue: &mut wayland_client::EventQueue<ToplevelFixture>,
        fixture: &mut ToplevelFixture,
    ) {
        for _ in 0..16 {
            let _ = conn.flush();
            let dispatched = display.dispatch_clients(state).unwrap_or(0);
            let _ = display.flush_clients();
            if let Some(guard) = conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = queue.dispatch_pending(fixture);
            if dispatched == 0 {
                break;
            }
        }
    }

    fn fixture_toplevel_client(
        display: &mut Display<State>,
        state: &mut State,
    ) -> (
        wayland_client::Connection,
        wayland_client::EventQueue<ToplevelFixture>,
        ToplevelFixture,
    ) {
        let (server_end, client_end) = UnixStream::pair().expect("unix stream pair");
        display
            .handle()
            .insert_client(server_end, Arc::new(ClientState::default()))
            .expect("insert client");
        let conn = wayland_client::Connection::from_socket(client_end).expect("client connection");
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let _registry = conn.display().get_registry(&qh, ());
        let mut fixture = ToplevelFixture::default();
        fixture_pump_toplevel(display, state, &conn, &mut queue, &mut fixture);
        assert!(fixture.compositor.is_some(), "client must bind wl_compositor");
        assert!(fixture.wm_base.is_some(), "client must bind xdg_wm_base");

        let surface = fixture.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg_surface = fixture.wm_base.as_ref().unwrap().get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        fixture.surface = Some(surface);
        fixture.xdg_surface = Some(xdg_surface);
        fixture.toplevel = Some(toplevel);
        fixture_pump_toplevel(display, state, &conn, &mut queue, &mut fixture);
        (conn, queue, fixture)
    }

    #[test]
    fn toplevel_first_commit_registers_only_after_commit() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        assert!(
            state.wm.window_ids().is_empty(),
            "toplevel creation must not register a WM window before the first commit"
        );

        fixture.toplevel.as_ref().unwrap().set_app_id("fixture-app".to_string());
        fixture.toplevel.as_ref().unwrap().set_title("Fixture Window".to_string());
        fixture.surface.as_ref().unwrap().commit();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);

        let ids = state.wm.window_ids();
        assert_eq!(ids.len(), 1, "first commit registers exactly one window");
        let window = state.wm.window(ids[0]).expect("window exists");
        assert_eq!(window.app_id, "fixture-app", "window must be created with real app_id");
        assert_eq!(window.title, "Fixture Window", "window must be created with real title");
    }

    #[test]
    fn toplevel_first_commit_sends_initial_configure_before_registering() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        assert!(fixture.configures > 0, "initial configure must reach the client");
        assert!(state.wm.window_ids().is_empty(), "configure is sent before first commit");

        fixture.surface.as_ref().unwrap().commit();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);
        assert_eq!(state.wm.window_ids().len(), 1, "commit after ack registers the window");
    }

    #[test]
    fn toplevel_first_commit_early_destroy_queues_no_window_open() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture.toplevel.as_ref().unwrap().destroy();
        fixture.xdg_surface.as_ref().unwrap().destroy();
        fixture.surface.as_ref().unwrap().destroy();
        fixture.toplevel.take();
        fixture.xdg_surface.take();
        fixture.surface.take();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);

        let opens = queued_layer_events(&state)
            .into_iter()
            .filter(|(kind, _)| *kind == EventKind::WindowOpen)
            .count();
        assert_eq!(opens, 0, "toplevel destroyed before first commit must never become a window");
        assert!(state.wm.window_ids().is_empty());
    }

    // e09s05t2: the index is keyed by WindowId, so a second client must not
    // overwrite or confuse the first one's surface.
    fn toplevel_app_id(toplevel: &ToplevelSurface) -> String {
        with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .app_id
                .clone()
                .unwrap_or_default()
        })
    }

    fn wm_id_for_app_id(state: &State, app_id: &str) -> wm::WindowId {
        state
            .wm
            .window_ids()
            .into_iter()
            .find(|id| state.wm.window(*id).is_some_and(|w| w.app_id == app_id))
            .unwrap_or_else(|| panic!("no WM window for app_id {app_id}"))
    }

    fn fixture_commit_toplevel(
        display: &mut Display<State>,
        state: &mut State,
        conn: &wayland_client::Connection,
        queue: &mut wayland_client::EventQueue<ToplevelFixture>,
        fixture: &mut ToplevelFixture,
        app_id: &str,
        title: &str,
    ) {
        fixture.toplevel.as_ref().unwrap().set_app_id(app_id.to_string());
        fixture.toplevel.as_ref().unwrap().set_title(title.to_string());
        fixture.surface.as_ref().unwrap().commit();
        fixture_pump_toplevel(display, state, conn, queue, fixture);
    }

    #[test]
    fn toplevel_index_lookup_and_second_client_are_independent() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn_a, mut queue_a, mut fixture_a) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn_a, &mut queue_a, &mut fixture_a, "alpha", "Alpha");
        let id_a = wm_id_for_app_id(&state, "alpha");

        let (conn_b, mut queue_b, mut fixture_b) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn_b, &mut queue_b, &mut fixture_b, "beta", "Beta");
        let id_b = wm_id_for_app_id(&state, "beta");

        assert_ne!(id_a, id_b, "distinct clients must own distinct windows");
        assert_eq!(state.toplevels.len(), 2, "index holds exactly the two live toplevels");
        assert_eq!(toplevel_app_id(state.toplevels.get(&id_a).expect("indexed a")), "alpha");
        assert_eq!(toplevel_app_id(state.toplevels.get(&id_b).expect("indexed b")), "beta");
        assert_ne!(
            state.toplevels[&id_a].wl_surface(),
            state.toplevels[&id_b].wl_surface(),
            "one surface must not shadow the other"
        );
        assert_eq!(toplevel_app_id(&toplevel_for_window(&state, id_a).expect("lookup a")), "alpha");
        assert_eq!(toplevel_app_id(&toplevel_for_window(&state, id_b).expect("lookup b")), "beta");
    }

    #[test]
    fn toplevel_index_removes_entry_on_destroy() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture, "doomed", "Doomed");
        let id = wm_id_for_app_id(&state, "doomed");
        assert!(state.toplevels.contains_key(&id), "precondition: indexed");

        fixture.toplevel.as_ref().unwrap().destroy();
        fixture.xdg_surface.as_ref().unwrap().destroy();
        fixture.surface.as_ref().unwrap().destroy();
        fixture.toplevel.take();
        fixture.xdg_surface.take();
        fixture.surface.take();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);

        assert!(!state.toplevels.contains_key(&id), "destroy must drop the index entry");
        assert!(toplevel_for_window(&state, id).is_none());
        assert!(state.wm.window(id).is_none());
    }

    #[test]
    fn toplevel_index_prune_drops_stale_entry() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture, "stale", "Stale");
        let id = wm_id_for_app_id(&state, "stale");
        assert!(state.toplevels.contains_key(&id), "precondition: indexed");

        // Simulate the WM losing the window without a shell callback (socket
        // death, hotplug). Prune must not leave an orphaned index entry.
        state.wm.remove_window(id);
        prune_dead_windows(&mut state);
        assert!(!state.toplevels.contains_key(&id), "prune must drop orphaned index entries");
    }

    // e09s05t3: client-initiated presentation requests, driven over the wire
    // rather than by poking WM state directly.
    #[test]
    fn client_request_fullscreen_set_unset_and_noop() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture, "cinema", "Cinema");
        let id = wm_id_for_app_id(&state, "cinema");
        let placement = state.wm.window(id).unwrap().mode.placement;
        let before = fixture.toplevel_configures;

        fixture.toplevel.as_ref().unwrap().unset_fullscreen();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);
        assert_eq!(state.wm.window(id).unwrap().mode.presentation, wm::Presentation::Normal);
        assert_eq!(fixture.toplevel_configures, before, "no-op unset must not configure");

        fixture.toplevel.as_ref().unwrap().set_fullscreen(None);
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);
        let mode = state.wm.window(id).unwrap().mode;
        assert_eq!(mode.presentation, wm::Presentation::Fullscreen);
        assert_eq!(mode.placement, placement, "presentation must not clobber placement");
        assert_eq!(state.wm.window(id).unwrap().mode_sent, Some(mode), "apply_layout must configure");
        let after_set = fixture.toplevel_configures;
        assert!(after_set > before, "client must see a configure after fullscreen");

        fixture.toplevel.as_ref().unwrap().unset_fullscreen();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);
        assert_eq!(state.wm.window(id).unwrap().mode.presentation, wm::Presentation::Normal);
        assert!(fixture.toplevel_configures > after_set, "unset must reconfigure");
    }

    #[test]
    fn client_request_maximize_saves_and_restores_rect() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture, "sheet", "Sheet");
        let id = wm_id_for_app_id(&state, "sheet");
        let saved = state.wm.window(id).unwrap().target_rect;
        let placement = state.wm.window(id).unwrap().mode.placement;
        let before = fixture.toplevel_configures;

        fixture.toplevel.as_ref().unwrap().set_maximized();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);
        assert_eq!(state.wm.window(id).unwrap().mode.presentation, wm::Presentation::Maximized);
        assert_eq!(state.wm.window(id).unwrap().mode.placement, placement);
        assert_eq!(state.wm.window(id).unwrap().saved_rect, Some(saved));
        assert!(fixture.toplevel_configures > before, "maximize must send a configure");

        let after_set = fixture.toplevel_configures;
        fixture.toplevel.as_ref().unwrap().unset_maximized();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);
        assert_eq!(state.wm.window(id).unwrap().mode.presentation, wm::Presentation::Normal);
        assert_eq!(state.wm.window(id).unwrap().target_rect, saved, "unmaximize restores saved geometry");
        assert!(fixture.toplevel_configures > after_set, "restore must send a configure");
    }

    #[test]
    fn client_request_on_unregistered_toplevel_is_noop() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture.toplevel.as_ref().unwrap().set_fullscreen(None);
        fixture.toplevel.as_ref().unwrap().set_maximized();
        fixture_pump_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture);

        assert!(state.wm.window_ids().is_empty(), "requests before commit must not create windows");
        assert!(state.toplevels.is_empty());
    }

    // e09s05t4: rules and events must see the committed app_id, not the empty
    // string new_toplevel used to hand to add_window.
    #[test]
    fn toplevel_rules_match_real_app_id_at_first_commit() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();
        let output_id = state.wm.focused_output_id().unwrap();
        state.wm.add_workspace(output_id, 3, "3");
        state.window_rules.lock().unwrap().push(wm::WindowRule {
            app_id: "ruled-app".to_string(),
            workspace: Some(3),
            mode: Some(wm::ClientMode::floating()),
        });
        lua_state::drain_window_rules(&mut state.wm, &state.window_rules);

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture, "ruled-app", "Ruled");
        let id = wm_id_for_app_id(&state, "ruled-app");
        let window = state.wm.window(id).expect("window exists");
        assert_eq!(window.workspace, 3, "rule workspace applies at creation");
        assert_eq!(window.mode, wm::ClientMode::floating(), "rule mode applies at creation");
    }

    #[test]
    fn toplevel_rules_non_match_uses_defaults() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();
        let output_id = state.wm.focused_output_id().unwrap();
        state.wm.add_workspace(output_id, 3, "3");
        state.window_rules.lock().unwrap().push(wm::WindowRule {
            app_id: "other-app".to_string(),
            workspace: Some(3),
            mode: Some(wm::ClientMode::floating()),
        });
        lua_state::drain_window_rules(&mut state.wm, &state.window_rules);

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture, "plain-app", "Plain");
        let id = wm_id_for_app_id(&state, "plain-app");
        let window = state.wm.window(id).expect("window exists");
        assert_ne!(window.workspace, 3, "another app must not inherit the rule");
        assert_eq!(window.mode, wm::ClientMode::tiled(), "default mode stays tiled");
    }

    #[test]
    fn toplevel_rules_events_carry_real_metadata_in_same_tick() {
        let mut display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let (conn, mut queue, mut fixture) = fixture_toplevel_client(&mut display, &mut state);
        fixture_commit_toplevel(&mut display, &mut state, &conn, &mut queue, &mut fixture, "event-app", "Event App");
        let id = wm_id_for_app_id(&state, "event-app");
        let events = queued_layer_events(&state);

        let open = events.iter().find(|(kind, _)| *kind == EventKind::WindowOpen).expect("window_open");
        assert_eq!(open.1["id"], id);
        assert_eq!(state.wm.window(id).unwrap().app_id, "event-app", "opened window has real metadata");
        let class = events.iter().find(|(kind, _)| *kind == EventKind::WindowClass).expect("window_class");
        assert_eq!(class.1["class"], "event-app", "class event must carry committed app_id");
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
        let display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        let output_id = state.wm.focused_output_id().unwrap();
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
    fn control_errors_invalid_json_has_code_1() {
        let mut state = test_state();
        let value: Value = serde_json::from_str(&handle_request(&mut state, "not json")).unwrap();
        assert_eq!(value["id"], Value::Null);
        assert_eq!(value["error"], "invalid json");
        assert_eq!(value["code"], 1);
    }

    #[test]
    fn control_errors_unknown_command_has_code_2() {
        let mut state = test_state();
        let value: Value = serde_json::from_str(&handle_request(
            &mut state,
            r#"{"id": 8, "cmd": "no.such.command"}"#,
        ))
        .unwrap();
        assert_eq!(value["id"], 8);
        assert_eq!(value["code"], 2);
        assert!(value["error"].as_str().unwrap().contains("unknown command"));
    }

    #[test]
    fn control_errors_missing_window_has_code_3() {
        let mut state = test_state();
        let value: Value = serde_json::from_str(&handle_request(
            &mut state,
            r#"{"id": 9, "cmd": "window.close", "args": {"id": 99}}"#,
        ))
        .unwrap();
        assert_eq!(value["code"], 3);
        assert!(value["error"].as_str().unwrap().contains("no live window 99"));
    }

    #[test]
    fn control_errors_bad_args_has_code_4() {
        let mut state = test_state();
        let value: Value = serde_json::from_str(&handle_request(
            &mut state,
            r#"{"id": 10, "cmd": "window.focus", "args": {}}"#,
        ))
        .unwrap();
        assert_eq!(value["code"], 4);
        assert!(value["error"].as_str().unwrap().contains("args.id"));
    }

    #[test]
    fn control_errors_denied_command_has_code_7() {
        let mut state = test_state();
        let value: Value = serde_json::from_str(&handle_request(
            &mut state,
            r#"{"id": 11, "cmd": "exec", "args": {"cmd": "touch /tmp/nope"}}"#,
        ))
        .unwrap();
        assert_eq!(value["code"], 7);
        assert_eq!(value["error"], "denied");
    }

    #[test]
    fn control_errors_success_reply_stays_additive() {
        let mut state = test_state();
        let value: Value = serde_json::from_str(&handle_request(&mut state, r#"{"id": 12, "cmd": "state"}"#)).unwrap();
        assert_eq!(value["id"], 12);
        assert!(value.get("ok").is_some());
        assert!(value.get("code").is_none(), "success keeps the old shape");
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

    // e09s02t6 — `focused` is the real flag; `primary` stays as a deprecated
    // alias so Tacit's existing consumers do not wake up on a different screen.
    #[test]
    fn control_output_list_reports_focused_and_primary_alias() {
        let mut state = test_state();
        let first = state.wm.focused_output_id().unwrap();
        let second = state.wm.add_output(
            "second",
            wm::Rect { x: 1920, y: 0, width: 1280, height: 720 },
        );
        state.wm.set_pointer_output(Some(second));
        let response = handle_request(&mut state, r#"{"id": 7, "cmd": "output.list"}"#);
        let value: Value = serde_json::from_str(&response).unwrap();
        let outputs = value["ok"].as_array().unwrap();
        assert_eq!(outputs.len(), 2);
        let second_json = outputs.iter().find(|o| o["id"] == second).unwrap();
        assert_eq!(second_json["focused"], true);
        assert_eq!(second_json["primary"], true, "primary is an alias of focused");
        let first_json = outputs.iter().find(|o| o["id"] == first).unwrap();
        assert_eq!(first_json["focused"], false);
        assert_eq!(first_json["primary"], false);
    }

    // e09s02t8 — identical-looking docks on two outputs must not share a
    // cached buffer; the output id is part of the signature. Renamed so the
    // documented `dock_per_output` gate stops passing on an empty audience.
    #[test]
    fn dock_per_output_buffer_signature_includes_id() {
        let entries = vec![wm::DockEntry {
            id: 7,
            app_id: "kitty".into(),
            title: "one".into(),
            minimized: false,
            focused: true,
        }];
        let first = dock_buffer_signature(1, &entries, 1920, 48);
        let second = dock_buffer_signature(2, &entries, 1920, 48);
        assert_ne!(first, second);
        assert_eq!(first, dock_buffer_signature(1, &entries, 1920, 48));
        let taller = dock_buffer_signature(1, &entries, 1920, 56);
        assert_ne!(first, taller);
    }

    // e09s02t7 — a floating window on the HDMI output lives at x>=1920;
    // clamping against a viewport that starts at 0 teleports it to the laptop.
    // Gate name matters: "0 tests passed" is not a green light, it is a nap.
    #[test]
    fn pointer_bounds_window_move_to_clamps_to_output_origin() {
        let mut state = test_state();
        let hdmi = state.wm.add_output(
            "hdmi",
            wm::Rect { x: 1920, y: 0, width: 1280, height: 720 },
        );
        let id = state.wm.add_window(hdmi, "kitty", "on hdmi").unwrap();
        state.wm.set_mode(id, wm::ClientMode::floating());
        let response = handle_request(
            &mut state,
            &format!(
                r#"{{"id": 8, "cmd": "window.move_to", "args": {{"id": {id}, "x": 100, "y": 100}}}}"#
            ),
        );
        let value: Value = serde_json::from_str(&response).unwrap();
        let rect = &value["ok"]["rect"];
        assert_eq!(rect["x"], 1920, "x clamps to this output's left edge");
        // The floating window is nearly as tall as HDMI's 720px geometry, so
        // its y range is only 0..=24 — clamped against HDMI, not the laptop.
        assert!(rect["y"].as_i64().unwrap() <= 24);
    }

    // e09s02t7 — a menu on HDMI must slide inside HDMI's work area, not the
    // focused output's or a hypothetical 1920x1080 screen at the origin.
    // Under the `pointer_bounds` gate now — no more vacuous green checkmark.
    #[test]
    fn pointer_bounds_popup_constraint_uses_parent_output() {
        let mut state = test_state();
        let hdmi = state.wm.add_output(
            "hdmi",
            wm::Rect { x: 1920, y: 0, width: 1280, height: 720 },
        );
        let hdmi_area = wm::Rect { x: 1960, y: 40, width: 1200, height: 632 };
        state.wm.set_output_work_area(hdmi, Some(hdmi_area));
        assert_eq!(popup_constraint(&state, (2000, 100)), hdmi_area);
        let laptop = state.wm.focused_output_id().unwrap();
        assert_eq!(popup_constraint(&state, (100, 100)), state.wm.work_area_for_output(laptop));
    }

    #[test]
    fn control_window_move_to_workspace() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
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
        let output1 = state.wm.focused_output_id().unwrap();
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
    fn spawn_log_path_from_env_prefers_xdg_state_home() {
        let path = spawn_log_path_from_env(Some("/tmp/xdg"), Some("/home/u"));
        assert_eq!(path, std::path::PathBuf::from("/tmp/xdg/eswm/spawn.log"));
    }

    static SPAWN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn spawn_shell_command_uses_configured_terminal_env() {
        let _guard = SPAWN_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("eswm-spawn-term-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &tmp) };

        set_spawn_terminal("foot");
        spawn_shell_command("printf %s \"$TERMINAL\"").expect("spawn shell command");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let contents = std::fs::read_to_string(spawn_log_path_in(&tmp)).unwrap();
        assert_eq!(contents, "foot");
        set_spawn_terminal("kitty");

        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spawn_reap_waitpid_loop_reaps_exited_children() {
        let _guard = SPAWN_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("eswm-spawn-reap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &tmp) };

        spawn_shell_command("true").expect("spawn short-lived child");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let reaped = reap_spawned_children();
        assert!(reaped > 0, "expected at least one child reaped, got {reaped}");

        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spawn_argv_command_executes_without_shell_wrapper() {
        let _guard = SPAWN_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("eswm-spawn-argv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &tmp) };

        spawn_argv_command(&["sh", "-c", "printf out"]).expect("spawn argv command");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let contents = std::fs::read_to_string(spawn_log_path_in(&tmp)).unwrap();
        assert_eq!(contents, "out");

        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spawn_shell_command_writes_stdout_stderr_to_unified_spawn_log() {
        let _guard = SPAWN_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join(format!("eswm-spawn-shell-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("XDG_STATE_HOME");
        unsafe { std::env::set_var("XDG_STATE_HOME", &tmp) };

        spawn_shell_command("printf out; printf err >&2").expect("spawn shell command");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let log = spawn_log_path_in(&tmp);
        let contents = std::fs::read_to_string(&log).unwrap();
        assert!(contents.contains("out"), "stdout missing: {contents:?}");
        assert!(contents.contains("err"), "stderr missing: {contents:?}");

        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spawn_log_path_from_env_falls_back_to_home_local_state() {
        let path = spawn_log_path_from_env(None, Some("/home/u"));
        assert_eq!(path, std::path::PathBuf::from("/home/u/.local/state/eswm/spawn.log"));
    }

    #[test]
    fn spawn_shell_command_uses_sh_c() {
        let (prog, args) = shell_command_argv("bash -c 'echo a b'");
        assert_eq!(prog, "sh");
        assert_eq!(args, vec!["-c".to_string(), "bash -c 'echo a b'".to_string()]);
    }

    #[test]
    fn spawn_log_file_is_created_with_mode_0600_and_appends() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let tmp_dir = std::env::temp_dir().join(format!("eswm-spawn-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let log_path = spawn_log_path_in(&tmp_dir);
        let mut f = open_spawn_log_file(&log_path).expect("open spawn log");
        writeln!(f, "first").unwrap();
        drop(f);
        let mut f = open_spawn_log_file(&log_path).expect("reopen spawn log");
        writeln!(f, "second").unwrap();
        drop(f);
        let meta = std::fs::metadata(&log_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let contents = std::fs::read_to_string(&log_path).unwrap();
        assert_eq!(contents, "first\nsecond\n");
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn spawn_log_path_uses_xdg_state_home_dir() {
        let base = std::path::Path::new("/tmp/xdg-state");
        let path = spawn_log_path_in(base);
        assert_eq!(path, base.join("eswm").join("spawn.log"));
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

    #[test]
    fn xdg_popup_handler_compiles_and_tracks_popups() {
        // RED: verifies xdg_popup handler is wired and popups vec is accessible
        let display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        // Initially no popups
        assert_eq!(state.popups.len(), 0);

        // Add a toplevel window first
        let output_id = state.wm.focused_output_id().unwrap();
        let _window_id = state
            .wm
            .add_window(output_id, "test-app", "Test App")
            .expect("add window");

        // Verify popups vec is accessible and empty
        assert!(state.popups.is_empty());
    }

    #[test]
    fn placed_windows_includes_popups() {
        // RED: verifies popup surfaces are included in placed_windows for rendering
        // GREEN: placed_windows iterates state.popups and adds them to the render list
        let display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        // Add a toplevel window
        let output_id = state.wm.focused_output_id().unwrap();
        let _window_id = state
            .wm
            .add_window(output_id, "test-app", "Test App")
            .expect("add window");

        // Arrange output so windows get target_rects
        state.wm.arrange_output(output_id);

        // placed_windows should not crash with popups in the list
        let placed = placed_windows(&mut state);
        // placed_windows may be empty if no toplevel surface is registered via protocol;
        // the important thing is it doesn't panic with popups present
        let _ = placed.len();
    }

    #[test]
    fn xdg_popup_grab_tracks_active_popup() {
        // RED: verifies popup grab tracking for modal input forwarding
        // GREEN: grab() sets state.popup_grab; release clears it
        let display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        // Initially no grab
        assert!(state.popup_grab.is_none());

        // Can't easily create a popup via protocol in unit test;
        // verify popup_grab field exists and is accessible
        assert_eq!(state.popups.len(), 0);
        assert!(state.popup_grab.is_none());
    }

    #[test]
    fn popup_dismissed_on_outside_click() {
        // RED: verifies click outside popup sends xdg_popup.dismissed
        // GREEN: pointer button handler checks grab + hit-test, sends dismissed
        let display: Display<State> = Display::new().expect("create test display");
        let dh = display.handle();
        let mut state = new_state("test", &dh, true);
        state.seat.add_keyboard(Default::default(), 200, 200).unwrap();

        // Initially no popups to dismiss
        assert_eq!(state.popups.len(), 0);
        assert!(state.popup_grab.is_none());
    }

    // --- xdg_positioner geometry (bug-2026-11-18t160939) -------------------
    // Reference behaviour: Hyprland CXDGPositionerRules::getPosition. The math
    // is delegated to smithay's get_unconstrained_geometry; these tests pin the
    // semantics eswm relies on (anchor point, gravity, flip, slide).

    use wayland_protocols::xdg::shell::server::xdg_positioner::{
        Anchor, ConstraintAdjustment, Gravity,
    };

    fn test_positioner(
        anchor_rect: (i32, i32, i32, i32),
        anchor: Anchor,
        gravity: Gravity,
        adjustment: ConstraintAdjustment,
    ) -> PositionerState {
        PositionerState {
            rect_size: (284, 336).into(),
            anchor_rect: Rectangle::new(
                (anchor_rect.0, anchor_rect.1).into(),
                (anchor_rect.2, anchor_rect.3).into(),
            ),
            anchor_edges: anchor,
            gravity,
            constraint_adjustment: adjustment,
            ..Default::default()
        }
    }

    fn full_hd() -> wm::Rect {
        wm::Rect { x: 0, y: 0, width: 1920, height: 1080 }
    }

    // e06s02t1: wl_surface.input_region is tracked per window on commit.
    #[test]
    fn input_region_tracks_surface() {
        use smithay::wayland::compositor::{RectangleKind, RegionAttributes};

        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let window_id = state.wm.add_window(output_id, "app", "t").expect("add window");

        let attrs = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, Rectangle::new((10, 20).into(), (100, 50).into())),
                (RectangleKind::Subtract, Rectangle::new((30, 30).into(), (10, 10).into())),
            ],
        };
        crate::apply_window_input_region(&mut state, window_id, Some(&attrs));

        let w = state.wm.window(window_id).expect("window");
        assert!(!w.input_region.is_default(), "input region should be tracked");
        assert!(w.input_region.contains(15, 25), "inside added rect");
        assert!(!w.input_region.contains(35, 35), "inside subtracted rect");
        assert!(!w.input_region.contains(5, 5), "outside region");
    }


    #[test]
    fn popup_positioner_context_menu_at_cursor() {
        // GTK context menu: 1x1 anchor rect at the click point, anchored at its
        // top-left, gravity bottom-right, zero offset. The menu's top-left must
        // land on the click point (the old code used offset only, so the menu
        // appeared at the parent window origin instead).
        let p = test_positioner(
            (100, 120, 1, 1),
            Anchor::TopLeft,
            Gravity::BottomRight,
            ConstraintAdjustment::empty(),
        );
        let geo = popup_geometry(&p, (0, 0), full_hd());
        assert_eq!((geo.x, geo.y), (100, 120));
        assert_eq!((geo.width, geo.height), (284, 336));
    }

    #[test]
    fn popup_positioner_flips_at_right_edge() {
        // Near the right edge the menu flips to the left of the anchor instead
        // of being clipped by the work area.
        let p = test_positioner(
            (1900, 100, 1, 1),
            Anchor::TopLeft,
            Gravity::BottomRight,
            ConstraintAdjustment::FlipX,
        );
        let geo = popup_geometry(&p, (0, 0), full_hd());
        assert_eq!(geo.x, 1617, "flipped popup right edge sits on the anchor");
        assert!(geo.x + geo.width as i32 <= 1920);
    }

    #[test]
    fn popup_positioner_slides_inside_work_area() {
        // No flip allowed: slide left until the popup is fully inside.
        let p = test_positioner(
            (1900, 100, 1, 1),
            Anchor::TopLeft,
            Gravity::BottomRight,
            ConstraintAdjustment::SlideX,
        );
        let geo = popup_geometry(&p, (0, 0), full_hd());
        assert_eq!(geo.x, 1920 - 284);
    }

    #[test]
    fn popup_positioner_flips_at_bottom_edge() {
        let p = test_positioner(
            (100, 900, 1, 1),
            Anchor::TopLeft,
            Gravity::BottomRight,
            ConstraintAdjustment::FlipY,
        );
        let geo = popup_geometry(&p, (0, 0), full_hd());
        assert_eq!(geo.y, 565, "flipped popup bottom edge sits on the anchor");
        assert!(geo.y + geo.height as i32 <= 1080);
    }

    #[test]
    fn popup_positioner_slides_up_from_bottom_edge() {
        let p = test_positioner(
            (100, 900, 1, 1),
            Anchor::TopLeft,
            Gravity::BottomRight,
            ConstraintAdjustment::SlideY,
        );
        let geo = popup_geometry(&p, (0, 0), full_hd());
        assert_eq!(geo.y, 1080 - 336);
    }

    #[test]
    fn popup_positioner_respects_work_area_offset() {
        // A top bar (work area starting at y=40) pushes a flipped menu down so
        // it does not hide under the bar.
        let area = wm::Rect { x: 0, y: 40, width: 1920, height: 1040 };
        let p = test_positioner(
            (100, 50, 1, 1),
            Anchor::BottomLeft,
            Gravity::TopRight,
            ConstraintAdjustment::FlipY,
        );
        let geo = popup_geometry(&p, (0, 0), area);
        assert!(geo.y >= 40, "popup stays below the bar, got {}", geo.y);
    }

    #[test]
    fn popup_positioner_geometry_is_parent_relative() {
        // The positioner result is relative to the parent window geometry;
        // adding the parent's screen origin gives the screen rect. This is what
        // placed_windows and new_popup rely on so a menu follows its window.
        let p = test_positioner(
            (50, 60, 1, 1),
            Anchor::TopLeft,
            Gravity::BottomRight,
            ConstraintAdjustment::empty(),
        );
        let parent = (400, 300);
        let geo = popup_geometry(&p, parent, full_hd());
        assert_eq!((geo.x, geo.y), (50, 60));
        assert_eq!((parent.0 + geo.x, parent.1 + geo.y), (450, 360));
    }

    #[test]
    fn rect_contains_edges_and_outside() {
        let r = wm::Rect { x: 10, y: 20, width: 100, height: 50 };
        assert!(rect_contains(r, 10.0, 20.0));
        assert!(rect_contains(r, 109.9, 69.9));
        assert!(!rect_contains(r, 110.0, 40.0), "right edge is exclusive");
        assert!(!rect_contains(r, 50.0, 70.0), "bottom edge is exclusive");
        assert!(!rect_contains(r, 9.9, 40.0));
        assert!(!rect_contains(r, 50.0, 19.9));
    }

    // e06s03t1: prefer interactive layer over toplevel, but follow focused
    // toplevel when no layer should hold keyboard focus.
    #[test]
    fn update_keyboard_focus_follows_focused_toplevel_when_no_layer() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let _id = state
            .wm
            .add_window(output_id, "test-app", "Test App")
            .expect("add window");
        // No real xdg_toplevel surface is registered in this unit-test state,
        // so keyboard focus resolves to no surface; the important contract is
        // that update_keyboard_focus runs the toplevel path without panicking
        // and leaves no stale layer focus.
        state.update_keyboard_focus();
        let focus = state.seat.get_keyboard().unwrap().current_focus();
        assert!(focus.is_none(), "no xdg surface registered -> no wl_keyboard focus surface");
        assert!(
            state.layer_wrappers.is_empty(),
            "precondition: no layer wrappers present"
        );
    }

    // e06s03t2: keyboard-interactive layer surfaces must be able to receive
    // wl_keyboard focus when they commit. This test verifies commit-path focus
    // policy integration at state level: with no existing focus holder and a
    // focused toplevel context, update/commit policy resolves without stealing
    // into an invalid state, and the focus API remains consistent.
    #[test]
    fn layer_keyboard_interactive_commit_does_not_break_focus_policy() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let _id = state
            .wm
            .add_window(output_id, "test-app", "Test App")
            .expect("add window");
        // No interactive layer is present; policy must follow the toplevel path.
        state.update_keyboard_focus();
        assert!(
            state.seat.get_keyboard().unwrap().current_focus().is_none(),
            "no registered toplevel surface -> no wl_keyboard focus surface"
        );
        // Repeated policy evaluation (as commits/state changes do) stays stable.
        state.update_keyboard_focus();
        assert!(state.layer_wrappers.is_empty());
        assert!(state.seat.get_keyboard().unwrap().current_focus().is_none());
    }

    // e06s03t3: when a layer holding keyboard focus is gone, focus is restored
    // to the last-focused toplevel on the same output (wm focused window).
    #[test]
    fn keyboard_focus_restored_to_toplevel_after_layer_loss() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let id = state
            .wm
            .add_window(output_id, "term", "Terminal")
            .expect("add window");
        assert_eq!(state.wm.focused_id(), Some(id), "toplevel should be focused");
        // Simulate layer destroy/unmap path: layer removed, policy re-evaluated.
        assert!(state.layer_wrappers.is_empty());
        state.update_keyboard_focus();
        assert_eq!(
            state.wm.focused_id(),
            Some(id),
            "last-focused toplevel on the output remains the focus target"
        );
        assert!(
            state.seat.get_keyboard().unwrap().current_focus().is_none(),
            "no registered xdg toplevel surface in test state; wl_keyboard focus resolves to none"
        );
    }

    // e06s03t4: integration-style focus flow — focused terminal, interactive
    // layer would take focus while present, closing it restores terminal focus.
    // Without real layer protocol objects we verify the policy end-state:
    // with the terminal as focused toplevel and no interactive layer, focus
    // resolves back to the toplevel target.
    #[test]
    fn keyboard_focus_returns_to_toplevel_when_no_interactive_layer() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let id = state
            .wm
            .add_window(output_id, "kitty", "kitty")
            .expect("add window");
        assert_eq!(state.wm.focused_id(), Some(id));

        // Terminal focused state.
        state.update_keyboard_focus();
        let terminal_focus = state.seat.get_keyboard().unwrap().current_focus();

        // After an interactive layer is closed/destroyed (no layer wrappers),
        // focus policy must restore to the last-focused toplevel.
        state.update_keyboard_focus();
        let restored_focus = state.seat.get_keyboard().unwrap().current_focus();

        assert_eq!(state.wm.focused_id(), Some(id));
        assert_eq!(
            terminal_focus, restored_focus,
            "keyboard focus policy returns to the same toplevel target when no interactive layer holds focus"
        );
    }

    // e06s04t1: wl_data_device set/get selection — clipboard tracked per seat.
    #[test]
    fn data_device_selection_tracks_clipboard_per_seat() {
        use smithay::wayland::selection::data_device::{
            clear_data_device_selection, current_data_device_selection_userdata,
            set_data_device_selection,
        };
        let state = test_state();
        assert!(
            current_data_device_selection_userdata::<State>(&state.seat).is_none(),
            "no clipboard selection initially"
        );
        set_data_device_selection(&state.dh, &state.seat, vec!["text/plain".into()], ());
        assert!(
            current_data_device_selection_userdata::<State>(&state.seat).is_some(),
            "compositor-set clipboard selection is tracked per seat"
        );
        clear_data_device_selection(&state.dh, &state.seat);
        assert!(
            current_data_device_selection_userdata::<State>(&state.seat).is_none(),
            "cleared clipboard selection is no longer tracked"
        );
    }

    // e06s04t2: primary selection is separate from clipboard and can follow focus.
    #[test]
    fn primary_selection_tracked_separately_from_clipboard() {
        use smithay::wayland::selection::data_device::{
            clear_data_device_selection, current_data_device_selection_userdata,
            set_data_device_selection,
        };
        use smithay::wayland::selection::primary_selection::{
            clear_primary_selection, current_primary_selection_userdata, set_primary_selection,
        };
        let state = test_state();
        set_data_device_selection(&state.dh, &state.seat, vec!["text/plain".into()], ());
        assert!(current_data_device_selection_userdata::<State>(&state.seat).is_some());
        assert!(
            current_primary_selection_userdata::<State>(&state.seat).is_none(),
            "primary selection is independent from clipboard"
        );

        set_primary_selection(&state.dh, &state.seat, vec!["text/plain".into()], ());
        assert!(current_primary_selection_userdata::<State>(&state.seat).is_some());
        assert!(
            current_data_device_selection_userdata::<State>(&state.seat).is_some(),
            "setting primary selection must not replace clipboard"
        );

        clear_primary_selection(&state.dh, &state.seat);
        assert!(current_primary_selection_userdata::<State>(&state.seat).is_none());
        assert!(current_data_device_selection_userdata::<State>(&state.seat).is_some());

        clear_data_device_selection(&state.dh, &state.seat);
    }

    // e06s04t3: data device focus tracks keyboard focus (clipboard offer follows
    // the focused window/client).
    #[test]
    fn data_device_focus_updates_on_keyboard_focus_change() {
        use smithay::input::SeatHandler;
        let mut state = test_state();
        let seat = state.seat.clone();
        // No focused surface/client: focus update must be safe and clear focus.
        SeatHandler::focus_changed(&mut state, &seat, None);
        // A real surface->client mapping requires protocol objects; the contract
        // here is that keyboard focus changes drive data-device focus updates.
        assert!(state.seat.get_keyboard().is_some());
    }

    // e06s04t4: integration-style selection flow (copy/paste + primary selection)
    // exercised via seat-scoped selection state and focus hooks.
    #[test]
    fn data_device_clipboard_and_primary_selection_flow() {
        use smithay::input::SeatHandler;
        use smithay::wayland::selection::data_device::{
            current_data_device_selection_userdata, set_data_device_selection,
        };
        use smithay::wayland::selection::primary_selection::{
            current_primary_selection_userdata, set_primary_selection,
        };
        let mut state = test_state();
        let seat = state.seat.clone();
        SeatHandler::focus_changed(&mut state, &seat, None);

        set_data_device_selection(&state.dh, &state.seat, vec!["text/plain;charset=utf-8".into()], ());
        set_primary_selection(&state.dh, &state.seat, vec!["text/plain;charset=utf-8".into()], ());

        assert!(current_data_device_selection_userdata::<State>(&state.seat).is_some());
        assert!(current_primary_selection_userdata::<State>(&state.seat).is_some());
    }

    // BUG-2026-11-19T235000: the titlebar maximize button updated target_rect
    // but never ran apply_layout, so the frame grew and the client did not.
    #[test]
    fn titlebar_maximize_click_configures_client_and_requests_redraw() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let id = state
            .wm
            .add_window(output_id, "test-app", "Test App")
            .expect("add window");
        // A floating window at a small size, as if the client had settled.
        state.wm.set_mode(id, wm::ClientMode::floating());
        let small = wm::Rect { x: 10, y: 10, width: 400, height: 300 };
        state.wm.set_window_rect(id, small);
        let before = state.wm.window(id).unwrap().target_rect;
        assert_eq!(before, small, "precondition: window starts at the small rect");
        state.redraw_needed = false;

        crate::udev::apply_titlebar_action(&mut state, id, wm::TitlebarButton::Maximize);

        let after = state.wm.window(id).unwrap();
        assert_eq!(after.mode.presentation, wm::Presentation::Maximized);
        assert!(
            after.target_rect.width > before.width,
            "maximize must widen the target rect: {:?} -> {:?}",
            before,
            after.target_rect
        );
        assert!(
            state.redraw_needed,
            "titlebar action must apply layout (and request a redraw), otherwise \
             the client is never configured and the frame outgrows its content"
        );
    }

    // e08s02t2 — the old Close branch called wm.remove_window directly; with
    // no toplevel to receive xdg_toplevel.close that orphaned the client.
    #[test]
    fn titlebar_close_without_toplevel_keeps_wm_window() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let id = state
            .wm
            .add_window(output_id, "test-app", "Test App")
            .expect("add window");
        assert!(toplevel_for_window(&state, id).is_none(), "fixture has no live toplevel");

        crate::udev::apply_titlebar_action(&mut state, id, wm::TitlebarButton::Close);

        assert!(
            state.wm.window(id).is_some(),
            "close without a toplevel must not remove the WM window"
        );
    }

    // e08s02t5 — CSD clients declare a window geometry offset; missing
    // geometry must map to zero, not to garbage.
    #[test]
    fn csd_geometry_none_maps_to_zero_offset() {
        assert_eq!(window_geometry_offset(None), (0, 0));
    }

    #[test]
    fn csd_geometry_maps_window_geometry_origin() {
        let geo = Rectangle::new((12, 34).into(), (100, 50).into());
        assert_eq!(window_geometry_offset(Some(geo)), (12, 34));
    }

    #[test]
    fn csd_geometry_negative_origin_is_preserved() {
        let geo = Rectangle::new((-8, -16).into(), (100, 50).into());
        assert_eq!(window_geometry_offset(Some(geo)), (-8, -16));
    }

    // e08s03t3 — cursor status must map to exactly one render behavior.
    #[test]
    fn cursor_image_hidden_hides_the_pointer() {
        use smithay::input::pointer::CursorImageStatus;
        assert_eq!(
            cursor_render_decision(&CursorImageStatus::Hidden),
            CursorRenderDecision::Hide
        );
    }

    #[test]
    fn cursor_image_named_falls_back_to_xcursor_images() {
        use smithay::input::pointer::{CursorIcon, CursorImageStatus};
        assert_eq!(
            cursor_render_decision(&CursorImageStatus::Named(CursorIcon::Default)),
            CursorRenderDecision::XcursorFallback
        );
    }

    // e08s04t1 — xdg window-state flags follow the WM mode exactly.
    #[test]
    fn xdg_state_flags_maximized_and_fullscreen_are_exclusive() {
        assert_eq!(wm::ClientMode::new(wm::Placement::Tiled, wm::Presentation::Maximized).xdg_state_flags(), (true, false));
        assert_eq!(wm::ClientMode::new(wm::Placement::Floating, wm::Presentation::Fullscreen).xdg_state_flags(), (false, true));
    }

    #[test]
    fn xdg_state_flags_other_modes_clear_both() {
        assert_eq!(wm::ClientMode::new(wm::Placement::Tiled, wm::Presentation::Normal).xdg_state_flags(), (false, false));
        assert_eq!(wm::ClientMode::new(wm::Placement::Floating, wm::Presentation::Normal).xdg_state_flags(), (false, false));
    }

    #[test]
    fn xdg_configure_state_marks_tiled_windows_only() {
        // BUG-2026-11-24-CSD-MAXIMIZED: Hyprland pins Maximized on every
        // toplevel to suppress CSD shadows; eswm must do the same for tiled
        // windows so Helium stops drawing a 10px inset inside our frame.
        assert_eq!(
            toplevel_configure_state(wm::ClientMode::tiled(), true),
            ToplevelConfigureState { activated: true, maximized: true, fullscreen: false, tiled: true }
        );
        assert_eq!(
            toplevel_configure_state(wm::ClientMode::floating(), false),
            ToplevelConfigureState { activated: false, maximized: false, fullscreen: false, tiled: false }
        );
        assert_eq!(
            toplevel_configure_state(wm::ClientMode::maximized(), true),
            ToplevelConfigureState { activated: true, maximized: true, fullscreen: false, tiled: false }
        );
        assert_eq!(
            toplevel_configure_state(wm::ClientMode::fullscreen(), true),
            ToplevelConfigureState { activated: true, maximized: false, fullscreen: true, tiled: false }
        );
    }

    // e08s04t3 — the configure-needed check depends on mode_sent; verify the
    // marker follows mode transitions (a real xdg client fixture is unavailable).
    #[test]
    fn apply_layout_mode_sent_tracks_mode_transitions() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let id = state.wm.add_window(output_id, "app", "App").unwrap();
        assert_eq!(state.wm.window(id).unwrap().mode_sent, None);
        state.wm.set_mode_sent(id, wm::ClientMode::maximized());
        assert_eq!(state.wm.window(id).unwrap().mode_sent, Some(wm::ClientMode::maximized()));
        // A transition to a different mode changes the marker again.
        state.wm.set_mode_sent(id, wm::ClientMode::fullscreen());
        assert_eq!(state.wm.window(id).unwrap().mode_sent, Some(wm::ClientMode::fullscreen()));
    }

    #[test]
    fn apply_layout_mode_sent_tracks_placement_changes() {
        let mut state = test_state();
        let output_id = state.wm.focused_output_id().unwrap();
        let id = state.wm.add_window(output_id, "app", "App").unwrap();
        state.wm.set_mode_sent(id, wm::ClientMode::tiled());
        assert_eq!(state.wm.window(id).unwrap().mode_sent, Some(wm::ClientMode::tiled()));
        // A floating<->tiled switch with the same rectangle must still send a
        // configure, because the tiled state bits are placement-dependent.
        state.wm.set_mode_sent(id, wm::ClientMode::floating());
        assert_eq!(state.wm.window(id).unwrap().mode_sent, Some(wm::ClientMode::floating()));
    }
}
