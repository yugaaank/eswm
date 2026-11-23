use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub type OutputId = u64;
pub type WindowId = u64;
pub type WorkspaceId = i32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Where a window is placed — the persistent layout axis (e09s01).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Placement {
    #[default]
    Tiled,
    Floating,
}

/// Temporary presentation state — independent of placement (e09s01).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Presentation {
    #[default]
    Normal,
    Maximized,
    Fullscreen,
}

/// A window's layout and presentation, kept as one `Copy` value so a
/// presentation transition cannot overwrite placement (instantWM-style).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClientMode {
    pub placement: Placement,
    pub presentation: Presentation,
}

impl ClientMode {
    pub fn new(placement: Placement, presentation: Presentation) -> Self {
        Self { placement, presentation }
    }

    /// Legacy constructors used while call sites migrate (e09s01t2).
    pub fn tiled() -> Self { Self::new(Placement::Tiled, Presentation::Normal) }
    pub fn floating() -> Self { Self::new(Placement::Floating, Presentation::Normal) }
    pub fn maximized() -> Self { Self::new(Placement::Tiled, Presentation::Maximized) }
    pub fn fullscreen() -> Self { Self::new(Placement::Tiled, Presentation::Fullscreen) }

    /// The same mode with presentation cleared — for restore transitions.
    pub fn without_presentation(self) -> Self {
        Self::new(self.placement, Presentation::Normal)
    }

    /// `(maximized, fullscreen)` xdg state bits derived from presentation only.
    pub fn xdg_state_flags(self) -> (bool, bool) {
        match self.presentation {
            Presentation::Maximized => (true, false),
            Presentation::Fullscreen => (false, true),
            Presentation::Normal => (false, false),
        }
    }

    /// Legacy mode name for IPC/Lua compatibility.
    pub fn mode_name(self) -> &'static str {
        match self.presentation {
            Presentation::Maximized => "maximized",
            Presentation::Fullscreen => "fullscreen",
            Presentation::Normal => match self.placement {
                Placement::Tiled => "tiled",
                Placement::Floating => "floating",
            },
        }
    }
}

/// Global spawn/layout policy: Hyprland (tiled) or Windows (floating).
/// Switching it only affects newly spawned windows; Super+Space still moves
/// individual windows between tiling and floating.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WmMode {
    #[default]
    Tiling,
    Floating,
}

/// Tiling algorithm used to arrange windows on an output.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TilingLayout {
    #[default]
    Master,
    Equal,
}

/// Input region in surface-local coordinates, as committed by wl_surface.
/// `None` means the default: the whole surface accepts input.
/// `Some(rects)` means only those rects accept input (empty = no input at all).
/// Rects follow RegionAttributes semantics (add/subtract in order).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputRegion {
    /// None = infinite/default surface; Some = explicit region ops applied in order.
    pub rects: Option<Vec<(bool, Rect)>>, // (add=true/subtract=false, rect)
}

impl InputRegion {
    #[cfg_attr(not(test), allow(dead_code))] // queried by tests and future callers
    pub fn is_default(&self) -> bool {
        self.rects.is_none()
    }

    /// Test a point in surface-local coordinates against this region.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let Some(rects) = &self.rects else {
            return true;
        };
        let mut inside = false;
        for &(add, r) in rects {
            let hit = x >= r.x
                && y >= r.y
                && x < r.x + r.width as i32
                && y < r.y + r.height as i32;
            if hit {
                inside = add;
            }
        }
        inside
    }
}

#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    pub output: OutputId,
    pub workspace: WorkspaceId,
    pub mode: ClientMode,
    pub mapped: bool,
    /// Explicit user-minimized (hidden) state, independent of buffer presence.
    pub minimized: bool,
    pub focused: bool,
    pub rect: Rect,
    pub target_rect: Rect,
    /// Rect saved before maximizing; restored when un-maximizing so the
    /// window returns to its previous floating position (Windows-like behaviour).
    pub saved_rect: Option<Rect>,
    /// xdg window geometry offset from set_window_geometry; render position = target_rect - offset
    pub xdg_offset: (i32, i32),
    /// Focus state last sent to the client as xdg Activated; None = never sent.
    pub activated_sent: Option<bool>,
    /// Window mode last sent to the client as xdg state flags; None = never sent.
    pub mode_sent: Option<Presentation>,
    /// Committed wl_surface input region (surface-local). None/default = whole surface.
    pub input_region: InputRegion,
    /// How many size re-configures have been sent because the client committed
    /// a buffer that did not match `target_rect` (BUG-2026-11-21-DMABUF-IMPORT,
    /// BUG-2026-11-24-SIZE-NEGOTIATION).
    pub size_resends: u8,
    /// When the last size re-configure was sent, for burst throttling.
    pub last_size_resend: Option<Instant>,
    /// Non-uniform stretch applied to a client that keeps committing a buffer
    /// of the wrong size (fixed-size Craft apps). (1.0, 1.0) means no stretch.
    pub fill_scale: (f64, f64),
}

/// Retry budget for clients that map at the wrong size and ignore the first
/// configure. Electron/Chromium and egui/winit commit their default-sized
/// buffer before honouring the compositor's size; we re-assert the size a few
/// times, then stop so a stubborn client cannot make the compositor spin forever.
pub const MAX_SIZE_RESENDS: u8 = 3;

/// Minimum spacing between size re-configures. A single client can queue several
/// commits before it processes the first configure; resending once per commit
/// spends the whole budget in under a millisecond and leaves the client no time
/// to react (observed: 3 resends in 0.5 ms).
pub const SIZE_RESEND_INTERVAL: Duration = Duration::from_millis(100);

/// What a surface commit should do about a buffer that does not match its
/// configured rect. Size, budget, and pacing are separate questions; folding
/// them together makes an exhausted budget look like a fitting commit and reset
/// itself (BUG-2026-11-22-OVERSIZE-RETRY-CAP).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeAction {
    /// Re-send the configure: the buffer is the wrong size and budget remains.
    Resend,
    /// The buffer matches the configured rect; the budget can start over.
    ResetBudget,
    /// A resend is warranted but the previous one was too recent.
    Throttled,
    /// Nothing to do.
    None,
}

/// Decide whether a committed buffer size should trigger another configure.
///
/// Any mismatch counts, oversized or undersized: an undersized buffer leaves
/// part of a tiled frame blank, and accepting it means no code path ever asks
/// the client to grow (BUG-2026-11-24-SIZE-NEGOTIATION). A window that has not
/// been laid out yet (zero-sized target) and a commit with no buffer are both
/// ignored so neither can spend the retry budget.
pub fn size_action(
    target: Rect,
    buffer: (u32, u32),
    resends: u8,
    since_last: Option<Duration>,
) -> SizeAction {
    if target.width == 0 || target.height == 0 || buffer.0 == 0 || buffer.1 == 0 {
        return SizeAction::None;
    }
    if buffer.0 == target.width && buffer.1 == target.height {
        return if resends > 0 { SizeAction::ResetBudget } else { SizeAction::None };
    }
    if resends >= MAX_SIZE_RESENDS {
        return SizeAction::None;
    }
    if since_last.is_some_and(|elapsed| elapsed < SIZE_RESEND_INTERVAL) {
        return SizeAction::Throttled;
    }
    SizeAction::Resend
}

/// Stretch a committed buffer to the configured frame. Fixed-size clients can
/// ignore every configure (Craft/egui logs "Failed to set window size"), and a
/// compositor cannot force them to grow — so scale their content instead.
/// Both directions are handled: oversized buffers are scaled down rather than
/// cropped, so the frame is filled either way. Invalid sizes are neutral.
pub fn fill_scale(target: Rect, buffer: (u32, u32)) -> (f64, f64) {
    if target.width == 0 || target.height == 0 || buffer.0 == 0 || buffer.1 == 0 {
        return (1.0, 1.0);
    }
    if target.width == buffer.0 && target.height == buffer.1 {
        return (1.0, 1.0);
    }
    // The compositor equivalent of stretching a sticker over a wider box;
    // the adhesive is not our problem.
    (target.width as f64 / buffer.0 as f64, target.height as f64 / buffer.1 as f64)
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub output: OutputId,
    pub windows: Vec<WindowId>,
    pub focused: Option<WindowId>,
}

#[derive(Clone, Debug)]
pub struct Output {
    pub id: OutputId,
    pub name: String,
    pub geometry: Rect,
    pub active_workspace: WorkspaceId,
    pub tiling_layout: TilingLayout,
}

/// A single entry in the dock: one window on the active workspace, either
/// active (mapped/visible) or minimized (unmapped). This is the data model
/// that frontends consume via `es.dock.entries()` — eswm itself renders a
/// default dock from it, but an external frontend can build its own UI.
#[derive(Clone, Debug)]
pub struct DockEntry {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    /// true when the window is minimized (mapped == false)
    pub minimized: bool,
    pub focused: bool,
}

/// One blank line kept for visual separation between structs.

#[derive(Default)]
pub struct WindowManager {
    next_output: OutputId,
    next_window: WindowId,
    outputs: BTreeMap<OutputId, Output>,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    windows: BTreeMap<WindowId, Window>,
    wm_mode: WmMode,
    cascade_count: u32,
    /// Outer gap (px) between tiled windows and the output / work-area edge.
    /// Outer gap (px) between tiled windows and the output / work-area edge.
    outer: u32,
    /// Inner gap (px) between adjacent tiled windows.
    inner: u32,
    /// Border width (px) drawn around each window.
    border_size: u32,
    /// Focused border color (RGBA).
    border_color_focused: [u8; 4],
    /// Unfocused border color (RGBA).
    border_color_unfocused: [u8; 4],
    /// Focused titlebar strip color (RGBA). Used when titlebar_height > 0.
    titlebar_color_focused: [u8; 4],
    /// Unfocused titlebar strip color (RGBA). Used when titlebar_height > 0.
    titlebar_color_unfocused: [u8; 4],
    /// Titlebar height (px). Extends the hit-test area upward past the window frame
    /// so clicks on the titlebar register as hitting the window.
    titlebar_height: u32,
    /// Dock bar height (px). eswm draws a simple dock bar at the bottom of the
    /// output when > 0; reserved from the work area so tiled windows stay clear.
    dock_height: u32,
    /// Tiling work area, excluding exclusive layer-surface zones (waybar, panels).
    /// None = use full output geometry.
    pub work_area: Option<Rect>,
    /// App-id/class -> workspace/mode rules, matched at window creation.
    rules: Vec<WindowRule>,
    /// Globally focused window — single source of truth (e09s01t4).
    pub focused: Option<WindowId>,
    /// Most-recently-used focus order; front = most recent.
    focus_history: Vec<WindowId>,
    /// Set when any state changes; arrange_output lays out only when dirty.
    dirty: bool,
}

/// A window rule: match a window's app_id and assign a workspace and/or mode.
#[derive(Clone, Debug)]
pub struct WindowRule {
    pub app_id: String,
    pub workspace: Option<WorkspaceId>,
    pub mode: Option<ClientMode>,
}

/// Border frame geometry: the outer frame that wraps a window's target_rect.
/// Includes top edge (spans full width), bottom edge, left edge, right edge.
/// All edges connect at corners with zero gap.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BorderFrame {
    /// Top edge rect (x, y, width, height)
    pub top: Rect,
    /// Bottom edge rect
    pub bottom: Rect,
    /// Left edge rect
    pub left: Rect,
    /// Right edge rect
    pub right: Rect,
}

/// Server-side titlebar buttons, ordered right-to-left in the strip:
/// close is rightmost, then maximize, then minimize.
///
/// BUG-2026-11-19T235000: pure hit-testing so the click handler cannot drift
/// from the renderer again. (The maximize button used to work, honest — until
/// nobody told the client about it.)
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TitlebarButton {
    Close,
    Maximize,
    Minimize,
}

/// Resolve which titlebar button (if any) sits at screen-x `x`.
///
/// Returns `None` when `x` is outside the titlebar strip or in the free
/// titlebar area left of the buttons (used for drag-to-move on floating
/// windows). Button layout mirrors the renderer: right-aligned at
/// `rect.x + rect.width`, each button `titlebar_height - 2` wide with a 4px gap.
pub fn titlebar_button_at(rect: Rect, titlebar_height: u32, x: f64) -> Option<TitlebarButton> {
    let right = rect.x as f64 + rect.width as f64;
    if x < rect.x as f64 || x >= right {
        return None;
    }
    let tb = titlebar_height as f64;
    let btn_size = (tb - 2.0).max(0.0);
    if btn_size <= 0.0 {
        return None;
    }
    let btn_gap = 4.0;
    let offset_from_right = right - x;
    let slot = btn_size + btn_gap;
    if offset_from_right < slot {
        Some(TitlebarButton::Close)
    } else if offset_from_right < 2.0 * slot {
        Some(TitlebarButton::Maximize)
    } else if offset_from_right < 3.0 * slot {
        Some(TitlebarButton::Minimize)
    } else {
        None
    }
}

/// Compute the border frame for a window's target_rect.
/// The frame extends `border_size` pixels outward on all sides, plus
/// `titlebar_height` pixels upward from the top edge.
/// All four edges meet at corners with no gap.
pub fn border_frame(
    target: Rect,
    border_size: u32,
    titlebar_height: u32,
) -> BorderFrame {
    let bp = border_size as i32;
    let tb = titlebar_height as i32;
    // Frame bounds: extends bp left/right, bp+tb up, bp down from target_rect
    let frame_x = target.x - bp;
    let frame_y = target.y - bp - tb;
    let frame_w = target.width as i32 + 2 * bp;
    let frame_h = target.height as i32 + 2 * bp + tb;

    BorderFrame {
        top: Rect {
            x: frame_x,
            y: frame_y,
            width: frame_w as u32,
            height: border_size,
        },
        bottom: Rect {
            x: frame_x,
            y: frame_y + frame_h - bp,
            width: frame_w as u32,
            height: border_size,
        },
        left: Rect {
            x: frame_x,
            y: frame_y,
            width: border_size,
            height: frame_h as u32,
        },
        right: Rect {
            x: frame_x + frame_w - bp,
            y: frame_y,
            width: border_size,
            height: frame_h as u32,
        },
    }
}

/// Window decoration values shared between the Lua config API and the WM.
/// Lua mutates these via `es.dsp.set_*`; the renderer reads them each frame.
#[derive(Clone, Debug, Default)]
pub struct DecorationConfig {
    /// Outer gap (px) between tiled windows and the output edge.
    pub outer: u32,
    /// Inner gap (px) between adjacent tiled windows.
    pub inner: u32,
    /// Border width (px) drawn around each window.
    pub border_size: u32,
    /// Focused border color (RGBA).
    pub border_color_focused: [u8; 4],
    /// Unfocused border color (RGBA).
    pub border_color_unfocused: [u8; 4],
    /// Corner radius (px). Stored and exposed to config; rounded-corner
    /// clipping in the renderer is a separate rendering task.
    pub corner_radius: u32,
    /// Titlebar height (px). eswm draws a titlebar strip with buttons at the
    /// top of each window when > 0; clients draw below it.
    pub titlebar_height: u32,
    /// Titlebar strip focused color (RGBA). Used when titlebar_height > 0.
    pub titlebar_color_focused: [u8; 4],
    /// Titlebar strip unfocused color (RGBA). Used when titlebar_height > 0.
    pub titlebar_color_unfocused: [u8; 4],
    /// Dock bar height (px). eswm draws a simple dock bar at the bottom of the
    /// output when > 0; reserved from the work area so tiled windows stay clear.
    pub dock_height: u32,
    /// Default wm mode for newly spawned windows:
    /// Tiling = tiled by default, Floating = floating by default.
    pub wm_mode: WmMode,
    /// Tiling layout algorithm for the output this decoration is applied to.
    pub tiling_layout: TilingLayout,
}

impl DecorationConfig {
    pub fn standard() -> Self {
        Self {
            outer: 10,
            inner: 10,
            border_size: 2,
            border_color_focused: [200, 200, 200, 255],
            border_color_unfocused: [60, 60, 60, 255],
            corner_radius: 0,
            titlebar_height: 0,
            titlebar_color_focused: [200, 200, 200, 255],
            titlebar_color_unfocused: [60, 60, 60, 255],
            dock_height: 0,
            wm_mode: WmMode::Tiling,
            tiling_layout: TilingLayout::Master,
        }
    }
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            next_output: 1,
            next_window: 1,
            outer: 10,
            inner: 10,
            border_size: 2,
            border_color_focused: [200, 200, 200, 255],
            border_color_unfocused: [60, 60, 60, 255],
            titlebar_color_focused: [200, 200, 200, 255],
            titlebar_color_unfocused: [60, 60, 60, 255],
            titlebar_height: 0,
            dock_height: 0,
            dirty: true,
            ..Self::default()
        }
    }

    pub fn add_output(&mut self, name: impl Into<String>, geometry: Rect) -> OutputId {
        let id = self.next_output;
        self.next_output += 1;
        let workspace = self.first_workspace_for_new_output(id);
        self.outputs.insert(
            id,
            Output {
                id,
                name: name.into(),
                geometry,
                active_workspace: workspace,
                tiling_layout: TilingLayout::default(),
            },
        );
        self.dirty = true;
        id
    }

    pub fn output(&self, id: OutputId) -> Option<&Output> {
        self.outputs.get(&id)
    }

    pub fn output_id_by_name(&self, name: &str) -> Option<OutputId> {
        self.outputs
            .values()
            .find(|output| output.name == name)
            .map(|output| output.id)
    }

    /// Drop an output (unplug / device removal). Its workspaces and windows move
    /// to the first surviving output; with no outputs left they are dropped.
    pub fn remove_output(&mut self, id: OutputId) -> bool {
        self.dirty = true;
        if self.outputs.remove(&id).is_none() {
            return false;
        }
        let survivor = self.outputs.keys().next().copied();
        let orphaned: Vec<WorkspaceId> = self
            .workspaces
            .values()
            .filter(|workspace| workspace.output == id)
            .map(|workspace| workspace.id)
            .collect();
        for workspace_id in orphaned {
            let windows = self
                .workspaces
                .get(&workspace_id)
                .map(|workspace| workspace.windows.clone())
                .unwrap_or_default();
            match survivor {
                Some(survivor) => {
                    if let Some(workspace) = self.workspaces.get_mut(&workspace_id) {
                        workspace.output = survivor;
                    }
                    for window in windows {
                        if let Some(window) = self.windows.get_mut(&window) {
                            window.output = survivor;
                        }
                    }
                }
                None => {
                    self.workspaces.remove(&workspace_id);
                    for window in windows {
                        self.windows.remove(&window);
                    }
                }
            }
        }
        let Some(survivor) = survivor else {
            return true;
        };
        let active_is_empty = self
            .outputs
            .get(&survivor)
            .and_then(|output| self.workspaces.get(&output.active_workspace))
            .map(|workspace| workspace.windows.is_empty())
            .unwrap_or(true);
        if active_is_empty
            && let Some(workspace) = self
                .workspaces
                .values()
                .find(|workspace| workspace.output == survivor && !workspace.windows.is_empty())
                .map(|workspace| workspace.id)
                && let Some(output) = self.outputs.get_mut(&survivor) {
                    output.active_workspace = workspace;
                }
        if let Some(active) = self.outputs.get(&survivor).map(|o| o.active_workspace) {
            self.focus_workspace(survivor, active);
        }
        self.arrange_output(survivor);
        true
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn primary_output_id(&self) -> Option<OutputId> {
        self.outputs.keys().next().copied()
    }

    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.outputs.values()
    }

    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.values()
    }

    pub fn window_ids(&self) -> Vec<WindowId> {
        self.windows.keys().copied().collect()
    }

    pub fn focused_id(&self) -> Option<WindowId> {
        self.focused
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn focus_history_first(&self) -> Option<WindowId> {
        self.focus_history.first().copied()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn focus_history(&self) -> &[WindowId] {
        &self.focus_history
    }

    /// Restore focus to the most recently focused window still present and
    /// on its workspace's active output, excluding `exclude`.
    fn focus_from_mru(&mut self, exclude: Option<WindowId>) {
        let candidate = self.focus_history.iter().copied().find(|id| {
            if Some(*id) == exclude { return false; }
            let Some(w) = self.windows.get(id) else { return false; };
            if w.minimized { return false; }
            self.outputs.get(&w.output).map(|o| o.active_workspace == w.workspace).unwrap_or(false)
        });
        if let Some(id) = candidate {
            self.focus_window(id);
        } else {
            self.focused = None;
            for w in self.windows.values_mut() { w.focused = false; }
        }
    }

    pub fn set_rect(&mut self, id: WindowId, rect: Rect) -> bool {
        if let Some(window) = self.windows.get_mut(&id) {
            window.rect = rect;
            self.dirty = true;
            return true;
        }
        false
    }

    /// Record that a size configure is being re-sent because the client
    /// committed a mismatched buffer. Clears `rect` so the next `apply_layout`
    /// resends the size (BUG-2026-11-21-DMABUF-IMPORT).
    /// Returns false when the retry budget is exhausted or the id is unknown.
    pub fn note_size_resend(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get_mut(&id) else {
            return false;
        };
        if window.size_resends >= MAX_SIZE_RESENDS {
            return false;
        }
        window.size_resends += 1;
        window.last_size_resend = Some(Instant::now());
        window.rect = Rect::default();
        self.dirty = true;
        true
    }

    /// Record the stretch needed to make a mismatched buffer fill its frame.
    pub fn set_fill_scale(&mut self, id: WindowId, scale: (f64, f64)) -> bool {
        let Some(window) = self.windows.get_mut(&id) else {
            return false;
        };
        window.fill_scale = scale;
        true
    }

    /// Reset the size-resend budget after a client commits a matching buffer.
    pub fn clear_size_resends(&mut self, id: WindowId) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.size_resends = 0;
        }
    }

    pub fn set_activated_sent(&mut self, id: WindowId, sent: bool) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.activated_sent = Some(sent);
            self.dirty = true;
        }
    }

    /// Record the window mode last configured to the client (e08s04t2).
    pub fn set_mode_sent(&mut self, id: WindowId, presentation: Presentation) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.mode_sent = Some(presentation);
        }
    }

    pub fn set_window_input_region(&mut self, id: WindowId, region: InputRegion) -> bool {
        let Some(window) = self.windows.get_mut(&id) else {
            return false;
        };
        window.input_region = region;
        true
    }

    pub fn set_window_meta(&mut self, id: WindowId, app_id: String, title: String) -> bool {
        if let Some(window) = self.windows.get_mut(&id) {
            window.app_id = app_id;
            window.title = title;
            self.dirty = true;
            return true;
        }
        false
    }

    // Create the workspace if missing (binding Super+N), return true if usable on `output`.
    pub fn ensure_workspace(&mut self, output: OutputId, workspace: WorkspaceId) -> bool {
        if self.workspaces.contains_key(&workspace) {
            return self.workspace(workspace).map(|w| w.output == output).unwrap_or(false);
        }
        self.add_workspace(output, workspace, workspace.to_string())
    }

    pub fn add_workspace(
        &mut self,
        output: OutputId,
        id: WorkspaceId,
        name: impl Into<String>,
    ) -> bool {
        if !self.outputs.contains_key(&output) || self.workspaces.contains_key(&id) {
            return false;
        }
        self.workspaces.insert(
            id,
            Workspace {
                id,
                name: name.into(),
                output,
                windows: Vec::new(),
                focused: None,
            },
        );
        true
    }

    pub fn focus_workspace(&mut self, output: OutputId, workspace: WorkspaceId) -> bool {
        let Some(target) = self.workspaces.get(&workspace) else {
            return false;
        };
        if target.output != output {
            return false;
        }
        let Some(output_state) = self.outputs.get_mut(&output) else {
            return false;
        };
        output_state.active_workspace = workspace;
        self.dirty = true;
        self.clear_focus_on_output(output);
        if self.focused.is_none() {
            self.focus_from_mru(None);
        }
        self.arrange_output(output);
        true
    }

    pub fn set_wm_mode(&mut self, mode: WmMode) {
        self.wm_mode = mode;
        self.dirty = true;
    }

    pub fn set_tiling_layout(&mut self, layout: TilingLayout) {
        let Some(output_id) = self.primary_output_id() else {
            return;
        };
        if let Some(output) = self.outputs.get_mut(&output_id) {
            output.tiling_layout = layout;
            self.dirty = true;
        }
    }

    pub fn set_outer(&mut self, outer: u32) {
        self.outer = outer;
        self.dirty = true;
    }

    pub fn set_inner(&mut self, inner: u32) {
        self.inner = inner;
        self.dirty = true;
    }

    pub fn set_border_size(&mut self, size: u32) {
        self.border_size = size;
        self.dirty = true;
    }

    pub fn border_size(&self) -> u32 {
        self.border_size
    }

    pub fn set_border_colors(&mut self, focused: [u8; 4], unfocused: [u8; 4]) {
        self.border_color_focused = focused;
        self.border_color_unfocused = unfocused;
    }

    pub fn border_color_focused(&self) -> [u8; 4] {
        self.border_color_focused
    }

    pub fn border_color_unfocused(&self) -> [u8; 4] {
        self.border_color_unfocused
    }

    pub fn set_titlebar_colors(&mut self, focused: [u8; 4], unfocused: [u8; 4]) {
        self.titlebar_color_focused = focused;
        self.titlebar_color_unfocused = unfocused;
        self.dirty = true;
    }

    pub fn titlebar_color_focused(&self) -> [u8; 4] {
        self.titlebar_color_focused
    }

    pub fn titlebar_color_unfocused(&self) -> [u8; 4] {
        self.titlebar_color_unfocused
    }

    pub fn set_titlebar_height(&mut self, height: u32) {
        self.titlebar_height = height;
        self.dirty = true;
    }

    pub fn set_dock_height(&mut self, height: u32) {
        self.dock_height = height;
        self.dirty = true;
    }

    pub fn dock_height(&self) -> u32 {
        self.dock_height
    }

    /// Update a window's client-declared window geometry offset (e08s02t6).
    /// Returns false when the window no longer exists.
    pub fn set_window_xdg_offset(&mut self, id: WindowId, offset: (i32, i32)) -> bool {
        let Some(window) = self.windows.get_mut(&id) else {
            return false;
        };
        window.xdg_offset = offset;
        true
    }

    /// Mark the window manager as needing re-arrangement. Call when external
    /// state (e.g. work_area) changes so arrange_output will run.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Dock entries for the active workspace of the primary output: every
    /// window on that workspace, active (mapped) first then minimized
    /// (unmapped), focused window first. This is the data model frontends
    /// consume via `es.dock.entries()`.
    pub fn dock_entries(&self) -> Vec<DockEntry> {
        let Some(primary) = self.primary_output_id() else {
            return Vec::new();
        };
        let Some(output_state) = self.outputs.get(&primary) else {
            return Vec::new();
        };
        let Some(workspace) = self.workspaces.get(&output_state.active_workspace) else {
            return Vec::new();
        };
        let focused = workspace.focused;
        let entries: Vec<DockEntry> = workspace
            .windows
            .iter()
            .filter_map(|id| self.windows.get(id).map(|w| (id, w)))
            .filter(|(_, w)| w.output == primary)
            .map(|(id, w)| DockEntry {
                id: *id,
                app_id: w.app_id.clone(),
                title: w.title.clone(),
                minimized: w.minimized,
                focused: focused == Some(*id),
            })
            .collect();
        // Stable creation order (workspace.windows order) so the focused
        // highlight MOVES between segments instead of reordering them.
        entries
    }

    #[cfg_attr(not(test), allow(dead_code))] // rules arrive via Lua config
    pub fn add_window_rule(&mut self, rule: WindowRule) {
        self.rules.push(rule);
    }

    /// Match a window's app_id against configured rules; return (workspace, mode).
    fn match_rule(&self, app_id: &str) -> (Option<WorkspaceId>, Option<ClientMode>) {
        for rule in &self.rules {
            if rule.app_id == app_id {
                return (rule.workspace, rule.mode);
            }
        }
        (None, None)
    }

    pub fn toggle_window_mode(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        let mode = if window.mode.placement == Placement::Tiled && window.mode.presentation == Presentation::Normal {
            ClientMode::floating()
        } else {
            ClientMode::tiled()
        };
        self.set_mode(id, mode)
    }

    /// Windows-like spawn placement: centered, cascaded. e08s01t3: the 640x480
    /// minimum is capped to the work area so tiny outputs cannot underflow the
    /// centering subtraction (or wrap into a gigantic offset in release builds).
    fn floating_target(&mut self, geometry: Rect) -> Rect {
        let width = ((geometry.width as u64 * 62 / 100).max(640) as u32).min(geometry.width.max(1));
        let height = ((geometry.height as u64 * 68 / 100).max(480) as u32).min(geometry.height.max(1));
        let offset = ((self.cascade_count % 6) * 28) as i32;
        self.cascade_count = self.cascade_count.wrapping_add(1);
        let x = geometry.x + (geometry.width.saturating_sub(width) / 2) as i32 + offset;
        let y = geometry.y + (geometry.height.saturating_sub(height) / 2) as i32 + offset;
        Rect { x, y, width, height }
    }

    /// Find the topmost mapped window whose client surface accepts pointer
    /// input at `pos` (global coords), respecting wl_surface input regions.
    /// Returns (window id, surface-local point in buffer/surface coords).
    /// Server-side decorations (titlebar, border) are not part of the client
    /// surface and never match here.
    pub fn surface_under(&self, pos: (f64, f64)) -> Option<(WindowId, (f64, f64))> {
        let primary = self.primary_output_id()?;
        let output_state = self.outputs.get(&primary)?;
        let workspace = self.workspaces.get(&output_state.active_workspace)?;
        let focused = workspace.focused;
        let mut ordered: Vec<WindowId> = Vec::new();
        if let Some(f) = focused {
            ordered.push(f);
        }
        for id in workspace.windows.iter().rev() {
            if focused != Some(*id) {
                ordered.push(*id);
            }
        }
        for id in ordered {
            let Some(window) = self.windows.get(&id) else { continue };
            if !window.mapped || window.minimized || window.target_rect.width == 0 || window.target_rect.height == 0 {
                continue;
            }
            let r = window.target_rect;
            // Hit only the client content rect; decorations live outside it.
            let inside_rect = pos.0 >= r.x as f64
                && pos.1 >= r.y as f64
                && pos.0 < r.x as f64 + r.width as f64
                && pos.1 < r.y as f64 + r.height as f64;
            if !inside_rect {
                continue;
            }
            // The surface is stretched by fill_scale to cover r, with its
            // window geometry starting at r. Invert that stretch before the
            // hit test so input regions stay in surface-local coordinates.
            let (sx, sy) = window.fill_scale;
            let local_x = (pos.0 - r.x as f64) / sx + window.xdg_offset.0 as f64;
            let local_y = (pos.1 - r.y as f64) / sy + window.xdg_offset.1 as f64;
            // Input regions are in surface-local coordinates. Default (None)
            // means the whole surface accepts input.
            if window.input_region.contains(local_x as i32, local_y as i32) {
                return Some((id, (local_x, local_y)));
            }
        }
        None
    }

    /// Highest-most mapped window of the active workspace at pos (newest first).
    /// Iterates in render order — the focused window first, then the rest
    /// newest-first — so a click on an overlap hits the topmost rendered window.
    pub fn window_at(&self, pos: (f64, f64), border_size: u32) -> Option<WindowId> {
        let primary = self.primary_output_id()?;
        let output_state = self.outputs.get(&primary)?;
        let workspace = self.workspaces.get(&output_state.active_workspace)?;
        let bs = border_size as i32;
        let focused = workspace.focused;
        let mut ordered: Vec<WindowId> = Vec::new();
        if let Some(f) = focused {
            ordered.push(f);
        }
        for id in workspace.windows.iter().rev() {
            if focused != Some(*id) {
                ordered.push(*id);
            }
        }
        for id in ordered {
            let Some(window) = self.windows.get(&id) else { continue };
            if !window.mapped || window.minimized {
                continue;
            }
            // Hit test against target_rect expanded by border size (border is part of window)
            // and titlebar height (titlebar sits above the client surface).
            let tb = self.titlebar_height as i32;
            let r = Rect {
                x: window.target_rect.x - bs,
                y: window.target_rect.y - bs - tb,
                width: window.target_rect.width.saturating_add(2 * border_size),
                height: window.target_rect.height.saturating_add(2 * border_size + tb as u32),
            };
            if r.width == 0 || r.height == 0 {
                continue;
            }
            let inside = pos.0 >= r.x as f64
                && pos.1 >= r.y as f64
                && pos.0 < r.x as f64 + r.width as f64
                && pos.1 < r.y as f64 + r.height as f64;

            if inside {
                return Some(id);
            }
        }
        None
    }
    /// target_rect; size changes flow through the next configure.
    pub fn set_window_rect(&mut self, id: WindowId, rect: Rect) -> bool {
        let Some(window) = self.windows.get_mut(&id) else {
            return false;
        };
        window.target_rect = rect;
        if window.mode == ClientMode::tiled() {
            // tiled geometry is owned by the arrangement; ignore direct moves
            return false;
        }
        if window.rect.width != rect.width || window.rect.height != rect.height {
            // force a size configure next apply_layout (width 0 sentinel)
            window.rect = Rect { width: 0, height: 0, ..window.rect };
        } else {
            window.rect = rect;
        }
        true
    }

    pub fn add_window(
        &mut self,
        output: OutputId,
        app_id: impl Into<String>,
        title: impl Into<String>,
    ) -> Option<WindowId> {
        let app_id_str = app_id.into();
        // Apply window rules before choosing workspace/mode.
        let (rule_ws, rule_mode) = self.match_rule(&app_id_str);
        let workspace = rule_ws
            .and_then(|ws| self.workspace(ws).map(|_| ws))
            .unwrap_or_else(|| {
                self.outputs
                    .get(&output)
                    .map(|o| o.active_workspace)
                    .unwrap_or(1)
            });
        let mode = rule_mode.unwrap_or(match self.wm_mode {
            WmMode::Tiling => ClientMode::tiled(),
            WmMode::Floating => ClientMode::floating(),
        });
        let id = self.next_window;
        self.next_window += 1;
        let geometry = self.outputs.get(&output)?.geometry;
        // Floating placement respects the work area (waybar/dock excluded),
        // not the raw output geometry, or windows spawn over panels.
        let floating_rect = match mode {
            m if m.placement == Placement::Floating => Some(self.floating_target(self.work_area.unwrap_or(geometry))),
            _ => None,
        };
        self.windows.insert(
            id,
            Window {
                id,
                app_id: app_id_str,
                title: title.into(),
                output,
                workspace,
                mode,
                mapped: true,
                minimized: false,
                focused: false,
                rect: Rect::default(),
                target_rect: floating_rect.unwrap_or_default(),
                saved_rect: None,
                activated_sent: None,
                mode_sent: None,
                xdg_offset: (0, 0),
                input_region: InputRegion::default(),
                size_resends: 0,
                last_size_resend: None,
                fill_scale: (1.0, 1.0),
            },
        );
        self.dirty = true;
        {
            let target = self.workspaces.get_mut(&workspace)?;
            target.windows.push(id);
        }
        self.workspaces.get_mut(&workspace)?.focused = Some(id);
        self.focus_window(id);
        self.arrange_output(output);
        Some(id)
    }

    pub fn remove_window(&mut self, id: WindowId) -> Option<Window> {
        let window = self.windows.remove(&id)?;
        self.dirty = true;
        if let Some(workspace) = self.workspaces.get_mut(&window.workspace) {
            workspace.windows.retain(|window_id| *window_id != id);
            if workspace.focused == Some(id) {
                workspace.focused = workspace.windows.last().copied();
            }
        }
        let removed_output = window.output;
        self.clear_focus_on_output(removed_output);
        if self.focused.is_none() {
            self.focus_from_mru(Some(id));
        }
        self.arrange_output(removed_output);
        Some(window)
    }

    pub fn focus_window(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        let output = window.output;
        let workspace = window.workspace;
        if self
            .outputs
            .get(&output)
            .map(|output_state| output_state.active_workspace != workspace)
            .unwrap_or(true)
        {
            return false;
        }
        self.clear_focus_on_output(output);
        // Global focus: clear the derived mirror on every window, not only
        // those on this output (two outputs previously kept two focused flags).
        for window in self.windows.values_mut() {
            if window.id != id {
                window.focused = false;
            }
        }
        if let Some(window) = self.windows.get_mut(&id) {
            window.focused = true;
        }
        if let Some(workspace) = self.workspaces.get_mut(&workspace) {
            workspace.focused = Some(id);
        }
        self.focused = Some(id);
        self.focus_history.retain(|w| *w != id);
        self.focus_history.insert(0, id);
        true
    }

    pub fn move_window_to_workspace(&mut self, id: WindowId, workspace: WorkspaceId) -> bool {
        let Some(window) = self.windows.get(&id).cloned() else {
            return false;
        };
        let Some(target) = self.workspaces.get(&workspace) else {
            return false;
        };
        if target.output != window.output || window.workspace == workspace {
            return false;
        }
        if let Some(source) = self.workspaces.get_mut(&window.workspace) {
            source.windows.retain(|window_id| *window_id != id);
            if source.focused == Some(id) {
                source.focused = source.windows.last().copied();
            }
        }
        if let Some(target) = self.workspaces.get_mut(&workspace) {
            target.windows.push(id);
        } else {
            return false;
        }
        if let Some(window_state) = self.windows.get_mut(&id) {
            window_state.workspace = workspace;
            window_state.focused = false;
            // Reset target rect; arrange_output sets it if the new workspace is active.
            window_state.target_rect = Rect::default();
        }
        let moved_output = window.output;
        self.clear_focus_on_output(moved_output);
        // Keep keyboard focus alive via MRU (the moved window left the active
        // workspace, so it must not be restored).
        if self.focused.is_none() {
            self.focus_from_mru(Some(id));
        }
        self.dirty = true;
        self.arrange_output(window.output);
        true
    }

    /// Leaving fullscreen always returns the window to Tiled; no mode history.
    pub fn toggle_fullscreen(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        let mode = match window.mode {
            m if m.presentation == Presentation::Fullscreen => m.without_presentation(),
            m => ClientMode::new(m.placement, Presentation::Fullscreen),
        };
        self.set_mode(id, mode)
    }

    pub fn toggle_maximized(&mut self, id: WindowId) -> bool {
        let (output, mode) = {
            let Some(window) = self.windows.get(&id) else {
                return false;
            };
            (window.output, window.mode)
        };

        match mode {
            m if m.presentation == Presentation::Maximized => {
                // Restore the saved floating rect so the window returns to
                // its previous position (Windows-like maximise/unmaximise).
                if let Some(w) = self.windows.get_mut(&id)
                    && let Some(saved) = w.saved_rect.take() {
                        w.mode = w.mode.without_presentation();
                        w.target_rect = saved;
                        // Deliberately leave `rect` at the maximized size: it is
                        // the "already applied" marker apply_layout compares
                        // against. Setting it here makes layout think the
                        // restore is done, sends no configure, and the client
                        // stays enormous inside a shrunken frame.
                        // (BUG-2026-11-19T235000: the window kept its old suit.)
                    }
                self.dirty = true;
                self.arrange_output(output);
                true
            }
            _ => {
                // Save rect and placement before maximizing so both can be
                // restored (e09s01: presentation must not overwrite placement).
                let placement = self
                    .windows
                    .get(&id)
                    .map(|w| w.mode.placement)
                    .unwrap_or(Placement::Tiled);
                if let Some(w) = self.windows.get_mut(&id) {
                    w.saved_rect = Some(w.target_rect);
                }
                self.set_mode(id, ClientMode::new(placement, Presentation::Maximized))
            }
        }
    }

    /// Toggle a window's mapped state (minimize/restore).
    /// When minimizing the focused window, focus stays on it so `toggle_minimize`
    /// (e.g. `Super+M`) toggles the same window back. `cycle_focus` skips
    /// unmapped windows so the invisible window won't interrupt focus cycling.
    /// When restoring a non-focused minimized window, it receives focus.
    pub fn toggle_minimize(&mut self, id: WindowId) -> bool {
        let (output, workspace, was_focused, was_minimized) = {
            let Some(window) = self.windows.get(&id) else {
                return false;
            };
            (
                window.output,
                window.workspace,
                window.focused,
                window.minimized,
            )
        };

        if let Some(window) = self.windows.get_mut(&id) {
            window.minimized = !window.minimized;
        }

        if was_focused && !was_minimized {
            // Minimizing the focused window: keep focus on it (invisible now).
            // Super+M again restores it since focused_id() still returns it.
        } else if !was_focused && was_minimized {
            // Restoring a non-focused minimized window: give it focus.
            self.clear_focus_on_output(output);
            if let Some(ws) = self.workspaces.get_mut(&workspace) {
                ws.focused = Some(id);
                if let Some(w) = self.windows.get_mut(&id) {
                    w.focused = true;
                }
            }
        }
        // was_focused && was_minimized: restoring a focused minimized window —
        // focus stays on it, nothing to do.

        self.dirty = true;
        self.arrange_output(output);
        true
    }

    /// Focus a window, restoring it from minimized state first if needed.
    /// Left-click on a dock entry should bring a minimized window back up.
    pub fn focus_window_or_restore(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        let output = window.output;
        let _workspace = window.workspace;
        let was_mapped = window.mapped;

        // Restore from minimized state if needed.
        if !was_mapped
            && let Some(w) = self.windows.get_mut(&id) {
                w.minimized = false;
            }

        self.focus_window(id);

        // If we restored a non-focused minimized window, focus_window already
        // set workspace.focused and w.focused = true. arrange_output so the
        // restored window gets a target_rect / configure.
        if !was_mapped {
            self.dirty = true;
            self.arrange_output(output);
        }
        true
    }

    /// Swap two windows' slots in the same workspace.
    pub fn swap_windows(&mut self, a: WindowId, b: WindowId) -> bool {
        let (Some(first), Some(second)) = (self.windows.get(&a), self.windows.get(&b)) else {
            return false;
        };
        if a == b || first.workspace != second.workspace {
            return false;
        }
        let workspace = first.workspace;
        let output = first.output;
        let Some(state) = self.workspaces.get_mut(&workspace) else {
            return false;
        };
        let (Some(i), Some(j)) = (
            state.windows.iter().position(|id| *id == a),
            state.windows.iter().position(|id| *id == b),
        ) else {
            return false;
        };
        state.windows.swap(i, j);
        self.dirty = true;
        self.arrange_output(output);
        true
    }

    /// Swap the focused window with its neighbour in the tiling order and keep
    /// focus on the same window.
    pub fn swap_focused(&mut self, delta: i32) -> bool {
        let Some(output) = self.primary_output_id() else {
            return false;
        };
        let Some(active) = self.outputs.get(&output).map(|o| o.active_workspace) else {
            return false;
        };
        let Some(workspace) = self.workspaces.get_mut(&active) else {
            return false;
        };
        let Some(index) = workspace
            .windows
            .iter()
            .position(|id| self.windows.get(id).map(|w| w.focused).unwrap_or(false))
        else {
            return false;
        };
        let len = workspace.windows.len() as i32;
        let other = (index as i32 + delta).rem_euclid(len) as usize;
        if other == index {
            return false;
        }
        workspace.windows.swap(index, other);
        self.dirty = true;
        self.arrange_output(output);
        true
    }

    /// Switch the primary output to the next/previous workspace it owns.
    pub fn focus_workspace_relative(&mut self, delta: i32) -> Option<WorkspaceId> {
        let output = self.primary_output_id()?;
        let ids: Vec<WorkspaceId> = self
            .workspaces
            .values()
            .filter(|workspace| workspace.output == output)
            .map(|workspace| workspace.id)
            .collect();
        if ids.is_empty() {
            return None;
        }
        let active = self.outputs.get(&output)?.active_workspace;
        let index = ids.iter().position(|id| *id == active).unwrap_or(0);
        let next = (index as i32 + delta).rem_euclid(ids.len() as i32) as usize;
        let target = ids[next];
        self.focus_workspace(output, target).then_some(target)
    }

    pub fn set_mode(&mut self, id: WindowId, mode: ClientMode) -> bool {
        let Some(window) = self.windows.get_mut(&id) else {
            return false;
        };
        window.mode = mode;
        self.dirty = true;
        let output = window.output;
        self.arrange_output(output);
        true
    }

    pub fn arrange_output(&mut self, output: OutputId) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(output_state) = self.outputs.get(&output).cloned() else {
            return;
        };
        let Some(workspace) = self.workspaces.get(&output_state.active_workspace).cloned() else {
            return;
        };
        let maximized = workspace
            .windows
            .iter()
            .filter_map(|id| self.windows.get(id))
            .find(|window| matches!(window.mode.presentation, Presentation::Fullscreen | Presentation::Maximized));
        if let Some(window) = maximized {
            // Fullscreen takes the entire output. Maximized fills the work area
            // (respects waybar/quickshell exclusive zones) minus the outer gap,
            // so borders/titlebar still show and the window doesn't cover panels.
            // For maximized windows, target_rect accounts for titlebar height so
            // the entire window (borders + titlebar + surface) fits within
            // work_area minus the outer gap — the titlebar does not extend above
            // work_area and cover waybar/quickshell panels.
            let rect = if window.mode.presentation == Presentation::Fullscreen {
                output_state.geometry
            } else {
                let base = self.work_area.unwrap_or(output_state.geometry);
                let bp = self.border_size as i32;
                let tb_h = self.titlebar_height as i32;
                Rect {
                    x: base.x + self.outer as i32 + bp,
                    y: base.y + self.outer as i32 + bp + tb_h,
                    width: base.width.saturating_sub(2 * self.outer).saturating_sub(2 * bp as u32),
                    height: base
                        .height
                        .saturating_sub(2 * self.outer)
                        .saturating_sub(tb_h as u32)
                        .saturating_sub(2 * bp as u32),
                }
            };
            self.set_target_rect(window.id, rect);
            return;
        }
        let tiled: Vec<_> = workspace
            .windows
            .iter()
            .filter_map(|id| self.windows.get(id))
            .filter(|window| window.mapped && !window.minimized && window.mode.placement == Placement::Tiled && window.mode.presentation == Presentation::Normal)
            .map(|window| window.id)
            .collect();

        if tiled.is_empty() {
            return;
        }
        let outer = self.outer;
        let inner_gap = self.inner;
        // when set, otherwise fall back to the full output geometry.
        let base = self.work_area.unwrap_or(output_state.geometry);
        // Inner geometry inset by gap on all sides so gaps/borders show.
        // Also account for the titlebar+borders: the window frame is drawn
        // extending titlebar_height+border ABOVE target_rect.y, so without
        // this inset the frame overlaps top panels (waybar) — same accounting
        // the maximized path uses.
        let bp = self.border_size as i32;
        let tb_h = self.titlebar_height as i32;
        let inner = Rect {
            x: base.x + outer as i32 + bp,
            y: base.y + outer as i32 + bp + tb_h,
            width: base.width.saturating_sub(2 * outer).saturating_sub(2 * bp as u32),
            height: base
                .height
                .saturating_sub(2 * outer)
                .saturating_sub(tb_h as u32)
                .saturating_sub(2 * bp as u32),
        };
        match output_state.tiling_layout {
            TilingLayout::Master => {
                let master_width = if tiled.len() == 1 {
                    inner.width
                } else {
                    ((inner.width as u64 * 3) / 5) as u32
                };
                self.set_target_rect(
                    tiled[0],
                    Rect {
                        x: inner.x,
                        y: inner.y,
                        width: master_width,
                        height: inner.height,
                    },
                );
                // floating windows keep their own geometry; the tiler ignores them
                // Inner gap only between master column and stack column.
                // Stack windows fill the remaining height without gaps between them.
                let stack = &tiled[1..];
                if !stack.is_empty() {
                    let stack_x = inner.x + master_width as i32 + inner_gap as i32;
                    let stack_w = inner
                        .width
                        .saturating_sub(master_width)
                        .saturating_sub(inner_gap);
                    // Inner gap between each stack window (vertical), including
                    // titlebar height so a lower window's titlebar doesn't
                    // overlap the window above it.
                    let gap_v = inner_gap as i32 + tb_h;
                    let total_gap_v = gap_v * (stack.len() - 1) as i32;
                    let cell_h = (inner.height as i32 - total_gap_v) / stack.len() as i32;
                    for (index, id) in stack.iter().enumerate() {
                        let top = inner.y + (index as i32 * (cell_h + gap_v));
                        self.set_target_rect(
                            *id,
                            Rect {
                                x: stack_x,
                                y: top,
                                width: stack_w,
                                height: cell_h as u32,
                            },
                        );
                    }
                }
            }
            TilingLayout::Equal => {
                // Equal arranges windows in equal-sized cells with inner gaps
                // between all cells. Floating windows keep their own geometry;
                // the tiler ignores them.
                let n = tiled.len();
                if n == 1 {
                    self.set_target_rect(tiled[0], inner);
                } else {
                    let mut assigned: Vec<(WindowId, Rect)> = Vec::new();
                    equal_split(&mut assigned, &tiled, inner, inner_gap, tb_h as u32);
                    for (id, rect) in assigned {
                        self.set_target_rect(id, rect);
                    }
                }
            }
        }
    }

    /// Move focus to the next/previous window of the active workspace.
    pub fn cycle_focus(&mut self, delta: i32) -> bool {
        let Some(output) = self.primary_output_id() else {
            return false;
        };
        let Some(active) = self.outputs.get(&output).map(|o| o.active_workspace) else {
            return false;
        };
        let Some(workspace) = self.workspaces.get(&active) else {
            return false;
        };
        // Only mapped (visible) windows are focusable via cycle_focus.
        let windows: Vec<WindowId> = workspace
            .windows
            .iter()
            .copied()
            .filter(|id| self.windows.get(id).map(|w| w.mapped && !w.minimized).unwrap_or(false))
            .collect();
        if windows.is_empty() {
            return false;
        }
        let current = windows
            .iter()
            .position(|id| self.windows.get(id).map(|w| w.focused).unwrap_or(false));
        let next = match current {
            Some(index) => (index as i32 + delta).rem_euclid(windows.len() as i32) as usize,
            None => 0,
        };
        let id = windows[next];
        self.focus_window(id)
    }

    fn first_workspace_for_new_output(&mut self, output: OutputId) -> WorkspaceId {
        let mut id = 1;
        while self.workspaces.contains_key(&id) {
            id += 1;
        }
        self.workspaces.insert(
            id,
            Workspace {
                id,
                name: id.to_string(),
                output,
                windows: Vec::new(),
                focused: None,
            },
        );
        id
    }

    fn set_target_rect(&mut self, id: WindowId, target: Rect) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.target_rect = target;
            // rect stays zero until apply_layout confirms; the client drives buffer size
        }
    }

    fn clear_focus_on_output(&mut self, output: OutputId) {
        for window in self.windows.values_mut().filter(|w| w.output == output) {
            if self.focused == Some(window.id) {
                self.focused = None;
            }
            window.focused = false;
        }
    }
}

/// Equal tiling layout: divides the rect into N equal-sized cells with gaps
/// between them. Each window gets the same width and height. Cells are arranged
/// in a grid that minimizes empty space and keeps cells as square as possible.
/// All windows receive identical rects — no master window, no shrinking.
/// `gap` is the inner gap in pixels between adjacent cells.
/// `titlebar_height` is added to vertical gaps between rows so that a lower
/// window's titlebar (which extends upward from its target_rect) never overlaps
/// the window above it.
pub fn equal_split(
    assigned: &mut Vec<(WindowId, Rect)>,
    window_ids: &[WindowId],
    rect: Rect,
    gap: u32,
    titlebar_height: u32,
) {
    let n = window_ids.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        assigned.push((window_ids[0], rect));
        return;
    }
    // Find the grid (rows x cols) that fits all windows. Primary: minimize the
    // larger dimension (avoid 1xN rows when 2x2 fits). Tiebreak: squarest cell
    // aspect ratio so cells are close to square.
    let mut best_rows = 1;
    let mut best_cols = n;
    let mut best_max_dim = n;
    let mut best_ratio_dev = f64::INFINITY;
    let screen_ratio = rect.width as f64 / rect.height as f64;
    for rows in 1..=n {
        let cols = n.div_ceil(rows); // ceil(n / rows)
        let max_dim = rows.max(cols);
        let cell_ratio = screen_ratio * (rows as f64 / cols as f64);
        let ratio_dev = (cell_ratio - 1.0).abs();
        if max_dim < best_max_dim || (max_dim == best_max_dim && ratio_dev < best_ratio_dev) {
            best_max_dim = max_dim;
            best_ratio_dev = ratio_dev;
            best_rows = rows;
            best_cols = cols;
        }
    }
    // Vertical gap between rows must accommodate titlebars: a lower window's
    // titlebar extends upward from its target_rect, so it needs tb_h of space.
    let gap_v = gap + titlebar_height;
    let cell_w = (rect.width.saturating_sub(gap * (best_cols - 1) as u32)) / best_cols as u32;
    let cell_h = (rect.height.saturating_sub(gap_v * (best_rows - 1) as u32)) / best_rows as u32;
    for (i, &id) in window_ids.iter().enumerate() {
        let row = i / best_cols;
        let col = i % best_cols;
        assigned.push((
            id,
            Rect {
                x: rect.x + col as i32 * (cell_w + gap) as i32,
                y: rect.y + row as i32 * (cell_h + gap_v) as i32,
                width: cell_w,
                height: cell_h,
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, TilingLayout, WindowManager, WindowRule, equal_split, titlebar_button_at, TitlebarButton, ClientMode, Placement, Presentation, size_action, SizeAction, fill_scale, MAX_SIZE_RESENDS, SIZE_RESEND_INTERVAL};
    use std::time::Duration;

    fn manager() -> (WindowManager, u64) {
        let mut manager = WindowManager::new();
        manager.set_outer(0);
        manager.set_inner(0);
        let output = manager.add_output(
            "test",
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        );
        (manager, output)
    }

    #[test]
    fn lays_out_master_and_stack() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        let third = manager.add_window(output, "kitty", "three").unwrap();

        assert_eq!(manager.window(first).unwrap().target_rect.width, 1149);
        assert_eq!(manager.window(second).unwrap().target_rect.x, 1151);
        assert_eq!(manager.window(second).unwrap().target_rect.height, 538);
        assert_eq!(manager.window(third).unwrap().target_rect.y, 540);
    }

    #[test]
    fn workspace_move_keeps_windows_separate() {
        let (mut manager, output) = manager();
        let window = manager.add_window(output, "kitty", "one").unwrap();
        assert!(manager.add_workspace(output, 2, "2"));

        assert!(manager.move_window_to_workspace(window, 2));
        assert!(manager.workspace(1).unwrap().windows.is_empty());
        assert_eq!(manager.workspace(2).unwrap().windows, vec![window]);
        assert!(!manager.window(window).unwrap().focused);
    }

    #[test]
    fn floating_windows_do_not_take_a_tiled_slot() {
        let (mut manager, output) = manager();
        let tiled = manager.add_window(output, "kitty", "one").unwrap();
        let floating = manager.add_window(output, "kitty", "two").unwrap();
        assert!(manager.set_mode(floating, ClientMode::floating()));

        assert_eq!(manager.window(tiled).unwrap().target_rect.width, 1916);
        assert_eq!(manager.window(floating).unwrap().mode, ClientMode::floating());
    }

    #[test]
    fn remove_output_moves_windows_to_survivor() {
        let (mut manager, first) = manager();
        let second = manager.add_output(
            "second",
            Rect {
                x: 1920,
                y: 0,
                width: 1280,
                height: 720,
            },
        );
        let window = manager.add_window(second, "kitty", "hotplug").unwrap();

        assert!(manager.remove_output(second));
        let window = manager.window(window).unwrap();
        assert_eq!(window.output, first);
        assert_eq!(window.target_rect.width, 1916);
        assert!(manager.output(second).is_none());
        assert!(!manager.remove_output(second));
    }

    #[test]
    fn cycle_focus_wraps_around() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        assert_eq!(manager.focused_id(), Some(second));

        assert!(manager.cycle_focus(1));
        assert_eq!(manager.focused_id(), Some(first));
        assert!(manager.cycle_focus(-1));
        assert_eq!(manager.focused_id(), Some(second));
    }

    #[test]
    fn swap_keeps_focus_and_exchanges_slots() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        let master_before = manager.window(first).unwrap().target_rect;

        assert!(manager.swap_focused(-1));
        assert_eq!(manager.focused_id(), Some(second));
        assert_eq!(manager.workspace(1).unwrap().windows, vec![second, first]);
        assert_ne!(manager.window(first).unwrap().target_rect, master_before);
    }

    #[test]
    fn moving_the_focused_window_refocuses_the_rest() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        assert!(manager.add_workspace(output, 2, "2"));
        assert_eq!(manager.focused_id(), Some(second));

        assert!(manager.move_window_to_workspace(second, 2));
        assert_eq!(manager.focused_id(), Some(first));
        assert!(!manager.window(second).unwrap().focused);
    }

    #[test]
    fn fullscreen_fills_the_output_and_returns_to_tiled() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        assert_eq!(manager.window(second).unwrap().target_rect.width, 767);

        assert!(manager.toggle_fullscreen(second));
        assert_eq!(manager.window(second).unwrap().mode, ClientMode::fullscreen());
        assert_eq!(manager.window(second).unwrap().target_rect.width, 1920);

        assert!(manager.toggle_fullscreen(second));
        assert_eq!(manager.window(second).unwrap().mode, ClientMode::tiled());
        assert_eq!(manager.window(first).unwrap().target_rect.width, 1149);
    }

    #[test]
    fn workspace_relative_wraps_within_one_output() {
        let (mut manager, output) = manager();
        let _ = manager.add_window(output, "kitty", "one").unwrap();
        assert!(manager.add_workspace(output, 2, "2"));

        assert_eq!(manager.focus_workspace_relative(1), Some(2));
        assert_eq!(manager.focus_workspace_relative(1), Some(1));
        assert_eq!(manager.focus_workspace_relative(-1), Some(2));
    }

    #[test]
    fn gap_insets_tiled_windows() {
        let (mut manager, output) = manager();
        manager.set_outer(10);
        manager.set_inner(10);
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();

        // Master should be inset by gap from output edge (plus 2px border).
        let m = manager.window(first).unwrap().target_rect;
        assert_eq!(m.x, 12);
        assert_eq!(m.y, 12);
        assert_eq!(m.width, 1137); // inner.width = 1896, 1896*3/5 = 1137
        assert_eq!(m.height, 1056); // 1080 - 20 - 4

        // Stack window should be to the right, after inner gap from master.
        // No gaps between stack windows — they fill full height.
        let s = manager.window(second).unwrap().target_rect;
        assert_eq!(s.x, 12 + 1137 + 10); // inner.x + master_width + inner_gap
        assert_eq!(s.y, 12); // inner.y (no gap offset for single stack window)
        assert_eq!(s.width, 749); // 1896 - 1137 - 10
        assert_eq!(s.height, 1056); // full inner height
    }

    #[test]
    fn gap_between_stack_windows() {
        let (mut manager, output) = manager();
        manager.set_outer(10);
        manager.set_inner(10);
        let master = manager.add_window(output, "kitty", "one").unwrap();
        let s1 = manager.add_window(output, "kitty", "two").unwrap();
        let s2 = manager.add_window(output, "kitty", "three").unwrap();
        let s3 = manager.add_window(output, "kitty", "four").unwrap();

        manager.arrange_output(output);

        let m = manager.window(master).unwrap().target_rect;
        assert_eq!(m.x, 12);
        assert_eq!(m.y, 12);
        assert_eq!(m.width, 1137); // inner.width = 1896, 1896*3/5 = 1137
        assert_eq!(m.height, 1056); // full inner height

        // 3 stack windows with inner gaps between them.
        // inner.height = 1056, gap_v = 10 + 0 = 10 (titlebar_height = 0)
        // total_gap_v = 10 * 2 = 20
        // cell_h = (1056 - 20) / 3 = 345
        // positions: y=12, y=12+355, y=12+710
        let expected_h = (1056 - 20) / 3;
        let expected_y1 = 12;
        let expected_y2 = 12 + expected_h + 10;
        let expected_y3 = 12 + 2 * (expected_h + 10);

        let w1 = manager.window(s1).unwrap().target_rect;
        assert_eq!(w1.x, 12 + 1137 + 10);
        assert_eq!(w1.y, expected_y1);
        assert_eq!(w1.width, 749);
        assert_eq!(w1.height, expected_h);

        let w2 = manager.window(s2).unwrap().target_rect;
        assert_eq!(w2.x, 12 + 1137 + 10);
        assert_eq!(w2.y, expected_y2 as i32);
        assert_eq!(w2.width, 749);
        assert_eq!(w2.height, expected_h);

        let w3 = manager.window(s3).unwrap().target_rect;
        assert_eq!(w3.x, 12 + 1137 + 10);
        assert_eq!(w3.y, expected_y3 as i32);
        assert_eq!(w3.width, 749);
        assert_eq!(w3.height, expected_h);
    }

    #[test]
    fn window_rule_assigns_workspace() {
        let (mut manager, output) = manager();
        manager.add_window_rule(WindowRule {
            app_id: "kitty".into(),
            workspace: Some(2),
            mode: None,
        });
        assert!(manager.add_workspace(output, 2, "2"));
        let id = manager.add_window(output, "kitty", "one").unwrap();
        assert_eq!(manager.window(id).unwrap().workspace, 2);
    }

    #[test]
    fn window_rule_assigns_mode() {
        let (mut manager, output) = manager();
        manager.add_window_rule(WindowRule {
            app_id: "fuzzel".into(),
            workspace: None,
            mode: Some(ClientMode::floating()),
        });
        let id = manager.add_window(output, "fuzzel", "one").unwrap();
        assert_eq!(manager.window(id).unwrap().mode, ClientMode::floating());
    }

    // e08s01t3 — small work areas must not underflow floating placement.
    // In debug builds `600u32 - 640u32` panics; in release it wraps into a
    // gigantic offset. Neither is a place anyone wants to put a window.
    #[test]
    fn floating_target_no_underflow_on_small_work_area() {
        let mut manager = WindowManager::new();
        manager.set_outer(0);
        manager.set_inner(0);
        let output = manager.add_output(
            "small",
            Rect { x: 0, y: 0, width: 600, height: 400 },
        );
        manager.work_area = Some(Rect { x: 0, y: 0, width: 600, height: 400 });
        manager.add_window_rule(WindowRule {
            app_id: "fuzzel".into(),
            workspace: None,
            mode: Some(ClientMode::floating()),
        });

        // Must not panic, and the target must fit inside the tiny work area.
        let id = manager.add_window(output, "fuzzel", "menu").expect("floating window");
        let r = manager.window(id).unwrap().target_rect;
        assert!(r.width <= 600 && r.height <= 400, "floating target must fit work area, got {r:?}");
        assert!(r.width > 0 && r.height > 0, "floating target must have area");
        assert!(r.x >= 0 && r.y >= 0, "floating target must stay on screen, got {r:?}");
    }

    #[test]
    fn window_rule_no_match_keeps_default() {
        let (mut manager, output) = manager();
        manager.add_window_rule(WindowRule {
            app_id: "other".into(),
            workspace: Some(2),
            mode: Some(ClientMode::floating()),
        });
        let id = manager.add_window(output, "kitty", "one").unwrap();
        assert_eq!(manager.window(id).unwrap().workspace, 1); // default active workspace
        assert_eq!(manager.window(id).unwrap().mode, ClientMode::tiled()); // default Hyprland mode
    }

    #[test]
    fn focusing_floating_window_keeps_stack_order_for_cycling() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        manager.set_mode(first, ClientMode::floating());
        manager.set_mode(second, ClientMode::floating());
        assert_eq!(manager.workspace(1).unwrap().windows, vec![first, second]);
        // focusing a floating window focuses it without reordering the stack,
        // so focus cycling keeps a stable order over all windows.
        assert!(manager.focus_window(first));
        assert_eq!(manager.focused_id(), Some(first));
        assert_eq!(manager.workspace(1).unwrap().windows, vec![first, second]);
        // cycle_focus visits floating windows too.
        assert!(manager.cycle_focus(1));
        assert_eq!(manager.focused_id(), Some(second));
        assert!(manager.cycle_focus(1));
        assert_eq!(manager.focused_id(), Some(first));
    }

    #[test]
    fn titlebar_button_at_maps_close_maximize_minimize() {
        // 100px-wide window at x=50, 30px titlebar -> btn_size 28, gap 4, slot 32.
        let rect = Rect { x: 50, y: 100, width: 100, height: 200 };
        let right = 150.0;
        // Close occupies [118, 150), maximize [86, 118), minimize [54, 86).
        assert_eq!(
            titlebar_button_at(rect, 30, right - 1.0),
            Some(TitlebarButton::Close)
        );
        assert_eq!(
            titlebar_button_at(rect, 30, right - 33.0),
            Some(TitlebarButton::Maximize)
        );
        assert_eq!(
            titlebar_button_at(rect, 30, right - 65.0),
            Some(TitlebarButton::Minimize)
        );
        // Left of the buttons: free titlebar area (drag zone), not a button.
        assert_eq!(titlebar_button_at(rect, 30, 52.0), None);
        // Outside the window rect entirely.
        assert_eq!(titlebar_button_at(rect, 30, 150.0), None);
        assert_eq!(titlebar_button_at(rect, 30, 49.0), None);
        // No titlebar, no buttons.
        assert_eq!(titlebar_button_at(rect, 0, right - 1.0), None);
    }

    #[test]
    fn toggle_maximized_saves_and_restores_rect() {
        let (mut manager, output) = manager();
        let window = manager.add_window(output, "kitty", "one").unwrap();
        let before = manager.window(window).unwrap().target_rect;

        assert!(manager.toggle_maximized(window));
        assert_eq!(manager.window(window).unwrap().mode, ClientMode::maximized());

        assert!(manager.toggle_maximized(window));
        let w = manager.window(window).unwrap();
        // e09s01: unmaximizing preserves the original placement (Tiled here),
        // rather than forcing Floating.
        assert_eq!(w.mode, ClientMode::tiled());
        assert_eq!(w.target_rect, before);
    }

    #[test]
    fn toggle_maximized_restore_forces_size_configure() {
        let (mut manager, output) = manager();
        let window = manager.add_window(output, "kitty", "one").unwrap();
        manager.set_mode(window, ClientMode::floating());
        let original = Rect { x: 100, y: 100, width: 400, height: 300 };
        manager.set_window_rect(window, original);
        // The client acknowledged the original size (what apply_layout does).
        manager.set_rect(window, original);

        assert!(manager.toggle_maximized(window));
        let maximized = manager.window(window).unwrap().target_rect;
        assert!(maximized.width > original.width);
        // Client acknowledged the maximized size.
        manager.set_rect(window, maximized);

        assert!(manager.toggle_maximized(window));
        let w = manager.window(window).unwrap();
        assert_eq!(w.mode, ClientMode::floating());
        assert_eq!(w.target_rect, original, "geometry returns to the saved rect");
        // BUG-2026-11-19T235000 (restore half): if apply_layout thinks the
        // geometry is already applied it sends no configure, so the frame
        // shrinks and the client stays at the maximized size. A restore must
        // leave a pending size change for apply_layout to push.
        assert_ne!(
            w.rect, w.target_rect,
            "restore must leave a pending configure so the client resizes back"
        );
    }

    #[test]
    fn set_window_meta_updates_app_id_and_title() {
        let (mut manager, output) = manager();
        let window = manager.add_window(output, "kitty", "one").unwrap();

        assert!(manager.set_window_meta(window, "firefox".into(), "Home Page".into()));
        let w = manager.window(window).unwrap();
        assert_eq!(w.app_id, "firefox");
        assert_eq!(w.title, "Home Page");

        assert!(!manager.set_window_meta(999, "x".into(), "y".into()));
    }

    #[test]
    fn workspace_remains_without_windows() {
        let (mut manager, output) = manager();
        assert!(manager.add_workspace(output, 2, "two"));
        let window = manager.add_window(output, "kitty", "one").unwrap();
        manager.remove_window(window);

        assert!(manager.workspace(2).is_some());
        assert_eq!(manager.workspace(2).unwrap().windows, Vec::<u64>::new());
    }

    #[test]
    fn focus_workspace_rejects_foreign_output() {
        let (mut manager, output) = manager();
        let second = manager.add_output(
            "second",
            Rect {
                x: 1920,
                y: 0,
                width: 1280,
                height: 720,
            },
        );
        assert!(manager.add_workspace(second, 3, "three"));

        assert!(!manager.focus_workspace(output, 3));
        assert_eq!(manager.output(output).unwrap().active_workspace, 1);
    }

    // BUG-2026-11-24-SIZE-NEGOTIATION: a client that commits a buffer bigger or
    // smaller than its tiled frame is not fitting. Both directions must ask for
    // a new configure, bounded by a retry budget and spaced out so a burst of
    // queued commits cannot spend the whole budget before the client reacts.
    const SIZE_TARGET: Rect = Rect { x: 0, y: 0, width: 800, height: 600 };

    #[test]
    fn size_action_resends_when_the_buffer_is_oversized() {
        assert_eq!(
            size_action(SIZE_TARGET, (1024, 768), 0, None),
            SizeAction::Resend
        );
    }

    #[test]
    fn size_action_resends_when_the_buffer_is_undersized() {
        // Craft apps map at 1440x900 inside a larger tiled frame. Accepting that
        // as "fitting" leaves the rest of the frame blank forever.
        assert_eq!(
            size_action(SIZE_TARGET, (640, 480), 0, None),
            SizeAction::Resend
        );
        assert_eq!(
            size_action(SIZE_TARGET, (800, 599), 0, None),
            SizeAction::Resend
        );
    }

    #[test]
    fn size_action_stops_after_the_resend_budget() {
        for buffer in [(1024, 768), (640, 480)] {
            assert_eq!(
                size_action(SIZE_TARGET, buffer, MAX_SIZE_RESENDS, None),
                SizeAction::None
            );
            assert_eq!(
                size_action(SIZE_TARGET, buffer, MAX_SIZE_RESENDS + 1, None),
                SizeAction::None
            );
        }
    }

    #[test]
    fn size_action_resets_only_on_an_exact_match() {
        assert_eq!(
            size_action(SIZE_TARGET, (800, 600), 2, None),
            SizeAction::ResetBudget
        );
        assert_eq!(
            size_action(SIZE_TARGET, (800, 600), 0, None),
            SizeAction::None
        );
    }

    #[test]
    fn size_action_never_resets_a_mismatched_commit() {
        for buffer in [(1024, 768), (640, 480), (800, 599), (799, 600)] {
            for resends in 0..=MAX_SIZE_RESENDS + 1 {
                assert_ne!(
                    size_action(SIZE_TARGET, buffer, resends, None),
                    SizeAction::ResetBudget
                );
            }
        }
    }

    #[test]
    fn size_action_ignores_unlaid_out_windows_and_empty_buffers() {
        let unlaid_out = Rect { x: 0, y: 0, width: 0, height: 0 };
        assert_eq!(
            size_action(unlaid_out, (1440, 900), 0, None),
            SizeAction::None
        );
        assert_eq!(size_action(SIZE_TARGET, (0, 0), 0, None), SizeAction::None);
        assert_eq!(
            size_action(SIZE_TARGET, (0, 600), 0, None),
            SizeAction::None
        );
    }

    #[test]
    fn size_action_defers_a_resend_inside_the_same_burst() {
        // Three queued commits landed within 0.5 ms and burned the whole budget
        // before the client could answer the first configure.
        assert_eq!(
            size_action(SIZE_TARGET, (640, 480), 1, Some(Duration::from_millis(1))),
            SizeAction::Throttled
        );
        assert_eq!(
            size_action(SIZE_TARGET, (640, 480), 1, Some(SIZE_RESEND_INTERVAL)),
            SizeAction::Resend
        );
        assert_eq!(
            size_action(SIZE_TARGET, (640, 480), 1, None),
            SizeAction::Resend
        );
    }

    #[test]
    fn size_action_resets_even_inside_the_burst_window() {
        assert_eq!(
            size_action(SIZE_TARGET, (800, 600), 1, Some(Duration::from_millis(1))),
            SizeAction::ResetBudget
        );
    }

    #[test]
    fn fill_scale_grows_an_undersized_buffer_to_the_frame() {
        assert_eq!(fill_scale(SIZE_TARGET, (640, 480)), (1.25, 1.25));
    }

    #[test]
    fn fill_scale_shrinks_an_oversized_buffer_to_the_frame() {
        assert_eq!(fill_scale(SIZE_TARGET, (1600, 1200)), (0.5, 0.5));
    }

    #[test]
    fn fill_scale_is_neutral_for_matching_and_invalid_input() {
        assert_eq!(fill_scale(SIZE_TARGET, (800, 600)), (1.0, 1.0));
        assert_eq!(fill_scale(SIZE_TARGET, (0, 0)), (1.0, 1.0));
        assert_eq!(fill_scale(Rect::default(), (640, 480)), (1.0, 1.0));
    }

    #[test]
    fn size_resend_clears_the_rect_and_counts_the_attempt() {
        let (mut manager, output) = manager();
        let window = manager.add_window(output, "wordcraft", "doc").unwrap();
        manager.set_rect(window, manager.window(window).unwrap().target_rect);
        assert!(manager.note_size_resend(window));
        // Clearing `rect` is what makes apply_layout resend the size.
        assert_eq!(manager.window(window).unwrap().rect.width, 0);
        assert_eq!(manager.window(window).unwrap().size_resends, 1);
        assert!(manager.window(window).unwrap().last_size_resend.is_some());
    }

    #[test]
    fn size_resend_budget_is_bounded() {
        let (mut manager, output) = manager();
        let window = manager.add_window(output, "wordcraft", "doc").unwrap();
        for _ in 0..MAX_SIZE_RESENDS {
            assert!(manager.note_size_resend(window));
        }
        assert!(!manager.note_size_resend(window));
    }

    #[test]
    fn matching_commit_resets_size_resend_budget() {
        let (mut manager, output) = manager();
        let window = manager.add_window(output, "wordcraft", "doc").unwrap();
        assert!(manager.note_size_resend(window));
        manager.clear_size_resends(window);
        assert_eq!(manager.window(window).unwrap().size_resends, 0);
    }

    #[test]
    fn arrange_output_only_arranges_active_workspace() {
        let (mut manager, output) = manager();
        manager.set_outer(0);
        manager.set_inner(0);
        let w1 = manager.add_window(output, "kitty", "one").unwrap();
        assert!(manager.add_workspace(output, 2, "two"));
        let w2 = manager.add_window(output, "kitty", "two").unwrap();
        assert!(manager.move_window_to_workspace(w2, 2));

        manager.arrange_output(output);

        let rect1 = manager.window(w1).unwrap().target_rect;
        assert_ne!(rect1.width, 0);
        let rect2 = manager.window(w2).unwrap().target_rect;
        assert_eq!(rect2.width, 0);
        assert_eq!(rect2.height, 0);
    }

    #[test]
    fn floating_window_on_inactive_workspace_not_tiled() {
        let (mut manager, output) = manager();
        manager.set_outer(0);
        manager.set_inner(0);
        let w1 = manager.add_window(output, "kitty", "one").unwrap();
        assert!(manager.add_workspace(output, 2, "two"));
        let w2 = manager.add_window(output, "kitty", "two").unwrap();
        manager.set_mode(w2, ClientMode::floating());
        assert!(manager.move_window_to_workspace(w2, 2));

        manager.arrange_output(output);

        // w1 is the only tiled window on active workspace 1 -> full width
        assert_eq!(manager.window(w1).unwrap().target_rect.width, 1916);
        // w2 is floating on workspace 2 -> not arranged, zero rect
        assert_eq!(manager.window(w2).unwrap().target_rect.width, 0);
    }

    #[test]
    fn equal_split_single_window_fills_rect() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let mut assigned: Vec<(u64, Rect)> = Vec::new();
        equal_split(&mut assigned, &[1], rect, 0, 0);

        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].0, 1);
        assert_eq!(assigned[0].1, Rect { x: 0, y: 0, width: 1920, height: 1080 });
    }

    #[test]
    fn equal_split_two_windows_split_vertically() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let mut assigned: Vec<(u64, Rect)> = Vec::new();
        equal_split(&mut assigned, &[1, 2], rect, 0, 0);

        assert_eq!(assigned.len(), 2);
        assert_eq!(assigned[0].1.x, 0);
        assert_eq!(assigned[0].1.width, 960);
        assert_eq!(assigned[1].1.x, 960);
        assert_eq!(assigned[1].1.width, 960);
        assert_eq!(assigned[0].1.y, 0);
        assert_eq!(assigned[0].1.height, 1080);
        assert_eq!(assigned[1].1.y, 0);
        assert_eq!(assigned[1].1.height, 1080);
    }

    #[test]
    fn equal_split_three_windows_equal_size() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let mut assigned: Vec<(u64, Rect)> = Vec::new();
        equal_split(&mut assigned, &[1, 2, 3], rect, 0, 0);

        assert_eq!(assigned.len(), 3);
        // All windows should have the same width and height
        let w = assigned[0].1.width;
        let h = assigned[0].1.height;
        for (_, r) in &assigned {
            assert_eq!(r.width, w, "window widths not equal");
            assert_eq!(r.height, h, "window heights not equal");
        }
        // All rects should be non-overlapping and within bounds
        for (_, r) in &assigned {
            assert!(r.x >= 0 && r.y >= 0);
            assert!(r.x + r.width as i32 <= 1920);
            assert!(r.y + r.height as i32 <= 1080);
        }
        // 3 windows in 2x2 grid: one cell empty, windows at (0,0),(960,0),(0,540)
        assert_eq!(assigned[0].1, Rect { x: 0, y: 0, width: 960, height: 540 });
        assert_eq!(assigned[1].1, Rect { x: 960, y: 0, width: 960, height: 540 });
        assert_eq!(assigned[2].1, Rect { x: 0, y: 540, width: 960, height: 540 });
    }

    #[test]
    fn arrange_output_uses_equal_when_tiling_layout_equal() {
        let (mut manager, output) = manager();
        manager.set_outer(0);
        manager.set_inner(0);
        manager.set_border_size(0);

        // Set tiling layout to equal on the output
        if let Some(output_state) = manager.outputs.get_mut(&output) {
            output_state.tiling_layout = TilingLayout::Equal;
        }

        let w1 = manager.add_window(output, "kitty", "one").unwrap();
        let w2 = manager.add_window(output, "kitty", "two").unwrap();
        let w3 = manager.add_window(output, "kitty", "three").unwrap();

        manager.arrange_output(output);

        let r1 = manager.window(w1).unwrap().target_rect;
        let r2 = manager.window(w2).unwrap().target_rect;
        let r3 = manager.window(w3).unwrap().target_rect;

        // All windows should have non-zero rects
        assert_ne!(r1.width, 0);
        assert_ne!(r2.width, 0);
        assert_ne!(r3.width, 0);

        // All windows should have equal size (2x2 grid, one cell empty)
        assert_eq!(r1.width, r2.width);
        assert_eq!(r1.height, r2.height);
        assert_eq!(r1.width, r3.width);
        assert_eq!(r1.height, r3.height);

        // Rects should not overlap
        for (a, b) in [(&r1, &r2), (&r1, &r3), (&r2, &r3)] {
            let overlap_x = (a.x.min(b.x + b.width as i32) - a.x.max(b.x)).max(0);
            let overlap_y = (a.y.min(b.y + b.height as i32) - a.y.max(b.y)).max(0);
            assert_eq!(overlap_x as u64 * overlap_y as u64, 0, "windows overlap");
        }
    }

    #[test]
    fn equal_split_rects_do_not_overlap_with_gap() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let mut assigned: Vec<(u64, Rect)> = Vec::new();
        equal_split(&mut assigned, &[1, 2, 3, 4], rect, 10, 0);

        assert_eq!(assigned.len(), 4);
        // 2x2 grid with 10px gaps: cell_w=(1920-10)/2=955, cell_h=(1080-10)/2=535
        // positions: (0,0),(965,0),(0,545),(965,545)
        assert_eq!(assigned[0].1, Rect { x: 0, y: 0, width: 955, height: 535 });
        assert_eq!(assigned[1].1, Rect { x: 965, y: 0, width: 955, height: 535 });
        assert_eq!(assigned[2].1, Rect { x: 0, y: 545, width: 955, height: 535 });
        assert_eq!(assigned[3].1, Rect { x: 965, y: 545, width: 955, height: 535 });

        // No overlaps
        for i in 0..assigned.len() {
            for j in i + 1..assigned.len() {
                let a = assigned[i].1;
                let b = assigned[j].1;
                let overlap_x = (a.x.min(b.x + b.width as i32) - a.x.max(b.x)).max(0);
                let overlap_y = (a.y.min(b.y + b.height as i32) - a.y.max(b.y)).max(0);
                assert_eq!(overlap_x as u64 * overlap_y as u64, 0, "windows {} and {} overlap", assigned[i].0, assigned[j].0);
            }
        }
    }

    #[test]
    fn equal_split_titlebar_gap_between_rows() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let mut assigned: Vec<(u64, Rect)> = Vec::new();
        // 2 windows side by side, 30px titlebar height, no extra gap
        equal_split(&mut assigned, &[1, 2], rect, 0, 30);

        assert_eq!(assigned.len(), 2);
        // 1x2 grid: cell_w=960, cell_h=1080
        assert_eq!(assigned[0].1, Rect { x: 0, y: 0, width: 960, height: 1080 });
        assert_eq!(assigned[1].1, Rect { x: 960, y: 0, width: 960, height: 1080 });

        // No titlebar overlap: window 2's titlebar would be at y=1050-1080,
        // window 1's client area ends at y=1080. No overlap.
        assert_eq!(assigned[0].1.y + assigned[0].1.height as i32, 1080);
        assert_eq!(assigned[1].1.y + assigned[1].1.height as i32, 1080);
    }

    #[test]
    fn equal_split_titlebar_gap_vertical_stack() {
        let rect = Rect { x: 0, y: 0, width: 1080, height: 1920 };
        let mut assigned: Vec<(u64, Rect)> = Vec::new();
        // 2 windows stacked vertically on a portrait screen, 30px titlebar
        equal_split(&mut assigned, &[1, 2], rect, 0, 30);

        assert_eq!(assigned.len(), 2);
        // 2x1 grid: cell_w=1080, cell_h=(1920-30)/2=945
        assert_eq!(assigned[0].1, Rect { x: 0, y: 0, width: 1080, height: 945 });
        assert_eq!(assigned[1].1, Rect { x: 0, y: 975, width: 1080, height: 945 });
        // Window 2's titlebar at y=945-975, window 1's client ends at y=945. No overlap.
    }

    #[test]
    fn surface_under_respects_input_region() {
        let mut wm = WindowManager::new();
        let out = wm.add_output("out", Rect { x: 0, y: 0, width: 1920, height: 1080 });
        let id = wm.add_window(out, "app", "t").expect("window");
        let win = wm.windows.get_mut(&id).unwrap();
        win.target_rect = Rect { x: 100, y: 100, width: 200, height: 200 };
        win.rect = win.target_rect;
        win.mapped = true;
        win.xdg_offset = (0, 0);
        // Restrict input to a sub-rect in surface-local coords.
        win.input_region = super::InputRegion {
            rects: Some(vec![(true, Rect { x: 10, y: 10, width: 40, height: 40 })]),
        };

        // Inside the input region: hit.
        let hit = wm.surface_under((115.0, 115.0));
        assert_eq!(hit.map(|(wid, _)| wid), Some(id), "point inside input region should hit");

        // Inside window rect but outside input region: no hit.
        let miss = wm.surface_under((160.0, 160.0));
        assert!(miss.is_none(), "point outside input region must not hit the surface");

        // Outside window entirely: no hit.
        assert!(wm.surface_under((50.0, 50.0)).is_none());
    }

    #[test]
    fn surface_under_inverts_fill_scale_for_input_regions() {
        // A fixed-size client is stretched to its frame; clicks must be mapped
        // back through the stretch before they reach surface-local input regions.
        let mut wm = WindowManager::new();
        let out = wm.add_output("out", Rect { x: 0, y: 0, width: 1920, height: 1080 });
        let id = wm.add_window(out, "craft", "doc").expect("window");
        let win = wm.windows.get_mut(&id).unwrap();
        win.target_rect = Rect { x: 100, y: 100, width: 200, height: 200 };
        win.rect = win.target_rect;
        win.mapped = true;
        win.fill_scale = (0.5, 0.5);
        win.input_region = super::InputRegion {
            rects: Some(vec![(true, Rect { x: 190, y: 190, width: 20, height: 20 })]),
        };

        let hit = wm.surface_under((200.0, 200.0));
        assert_eq!(hit.map(|(wid, _)| wid), Some(id));
        assert_eq!(hit.unwrap().1, (200.0, 200.0));
        assert!(wm.surface_under((150.0, 150.0)).is_none());
    }

    #[test]
    fn input_region_empty_and_default() {
        let mut wm = WindowManager::new();
        let out = wm.add_output("out", Rect { x: 0, y: 0, width: 1920, height: 1080 });
        let id = wm.add_window(out, "app", "t").expect("window");
        {
            let win = wm.windows.get_mut(&id).unwrap();
            win.target_rect = Rect { x: 100, y: 100, width: 200, height: 200 };
            win.rect = win.target_rect;
            win.mapped = true;
            win.xdg_offset = (0, 0);
            // None = default: whole surface accepts input.
            win.input_region = super::InputRegion::default();
        }
        assert!(wm.window(id).unwrap().input_region.is_default(), "default region should be None/infinite");
        assert!(
            wm.surface_under((150.0, 150.0)).is_some(),
            "default input region should accept pointer anywhere in the content rect"
        );

        // Empty explicit region = no pointer events at all.
        wm.windows.get_mut(&id).unwrap().input_region = super::InputRegion { rects: Some(vec![]) };
        assert!(
            !wm.window(id).unwrap().input_region.is_default(),
            "explicit empty region must not be treated as default"
        );
        assert!(
            wm.surface_under((150.0, 150.0)).is_none(),
            "empty input region must accept no pointer events"
        );
    }

    // e09s01t1 — placement and presentation are independent axes.
    #[test]
    fn client_mode_keeps_placement_across_presentation() {
        let mode = ClientMode::new(Placement::Floating, Presentation::Maximized);
        assert_eq!(mode.placement, Placement::Floating);
        assert_eq!(mode.presentation, Presentation::Maximized);
        let normal = mode.without_presentation();
        assert_eq!(normal.placement, Placement::Floating);
        assert_eq!(normal.presentation, Presentation::Normal);
    }

    #[test]
    fn client_mode_xdg_flags_follow_presentation_only() {
        assert_eq!(
            ClientMode::new(Placement::Tiled, Presentation::Maximized).xdg_state_flags(),
            (true, false)
        );
        assert_eq!(
            ClientMode::new(Placement::Floating, Presentation::Fullscreen).xdg_state_flags(),
            (false, true)
        );
        assert_eq!(
            ClientMode::new(Placement::Tiled, Presentation::Normal).xdg_state_flags(),
            (false, false)
        );
    }

    #[test]
    fn client_mode_name_matches_legacy_window_mode_names() {
        assert_eq!(ClientMode::new(Placement::Tiled, Presentation::Normal).mode_name(), "tiled");
        assert_eq!(ClientMode::new(Placement::Floating, Presentation::Normal).mode_name(), "floating");
        assert_eq!(ClientMode::new(Placement::Tiled, Presentation::Maximized).mode_name(), "maximized");
        assert_eq!(ClientMode::new(Placement::Tiled, Presentation::Fullscreen).mode_name(), "fullscreen");
    }

    // e09s01t3 — buffer presence and user-minimization are distinct.
    #[test]
    fn minimized_state_is_independent_of_mapped() {
        let (mut manager, output) = manager();
        let id = manager.add_window(output, "kitty", "one").unwrap();
        let w = manager.window(id).unwrap();
        assert!(w.mapped, "fresh window has a buffer");
        assert!(!w.minimized, "fresh window is not minimized");

        assert!(manager.toggle_minimize(id));
        let w = manager.window(id).unwrap();
        assert!(w.mapped, "minimizing must not discard the client's buffer");
        assert!(w.minimized, "minimizing sets the explicit flag");

        // Dock entries derive minimized from the flag, not from mapped.
        assert!(manager.dock_entries().iter().any(|e| e.minimized));

        assert!(manager.toggle_minimize(id));
        assert!(!manager.window(id).unwrap().minimized);
    }

    // e09s01t4 — one global focus with MRU history.
    #[test]
    fn focus_mru_tracks_single_focus_across_outputs() {
        let mut manager = WindowManager::new();
        manager.set_outer(0);
        manager.set_inner(0);
        let out_a = manager.add_output("a", Rect { x: 0, y: 0, width: 1920, height: 1080 });
        let out_b = manager.add_output("b", Rect { x: 1920, y: 0, width: 1920, height: 1080 });
        let a1 = manager.add_window(out_a, "app", "a1").unwrap();
        let b1 = manager.add_window(out_b, "app", "b1").unwrap();

        assert!(manager.focus_window(a1));
        assert_eq!(manager.focused_id(), Some(a1));
        assert!(manager.focus_window(b1));
        assert_eq!(manager.focused_id(), Some(b1));
        // Exactly one window carries the derived focused mirror.
        assert_eq!(manager.windows.values().filter(|w| w.focused).count(), 1);
        // MRU: most recent first, then previous.
        assert_eq!(manager.focus_history_first(), Some(b1));
        assert!(manager.focus_history().contains(&a1));
    }

    #[test]
    fn focus_mru_restores_workspace_focus() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "app", "first").unwrap();
        let second = manager.add_window(output, "app", "second").unwrap();
        manager.add_workspace(output, 2, "2");
        assert!(manager.focus_window(first));
        assert!(manager.focus_window(second));
        assert!(manager.move_window_to_workspace(second, 2));
        // After moving the focused window away, MRU restores the other
        // window on the active workspace rather than clearing focus.
        assert_eq!(manager.focused_id(), Some(first));
    }

    // e09s01t5 — presentation toggles preserve placement in both directions.
    #[test]
    fn presentation_toggles_preserve_floating_placement() {
        let (mut manager, output) = manager();
        let id = manager.add_window(output, "kitty", "one").unwrap();
        manager.set_mode(id, ClientMode::floating());

        manager.toggle_maximized(id);
        let m = manager.window(id).unwrap().mode;
        assert_eq!(m.placement, Placement::Floating);
        assert_eq!(m.presentation, Presentation::Maximized);
        manager.toggle_maximized(id);
        assert_eq!(manager.window(id).unwrap().mode, ClientMode::floating());

        manager.toggle_fullscreen(id);
        let m = manager.window(id).unwrap().mode;
        assert_eq!(m.placement, Placement::Floating);
        assert_eq!(m.presentation, Presentation::Fullscreen);
        manager.toggle_fullscreen(id);
        assert_eq!(manager.window(id).unwrap().mode, ClientMode::floating());
    }
}
