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

    /// Whether this mode should advertise xdg tiled states to the client.
    /// Only a genuinely tiled, normal-presentation window is tiled; maximized
    /// and fullscreen windows already carry authoritative non-stateless flags.
    // Hyprland has been saying this out loud for years; eswm was just shy.
    pub fn sends_tiled_state(self) -> bool {
        self.placement == Placement::Tiled && self.presentation == Presentation::Normal
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
    /// Full window mode last sent to the client as xdg state flags; None = never sent.
    pub mode_sent: Option<ClientMode>,
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

/// Adjustable layout inputs for one output (e10s02). Stored on the output, not
/// the window: `arrange_output` owns every tiled rect and would erase a
/// per-window override on the next pass. Weights are kept flat and normalized
/// on every arrangement so adding/removing windows cannot leave a stale length.
#[derive(Clone, Debug)]
pub struct TilingSplits {
    /// Width share of the master column in the Master layout.
    pub master_ratio: f64,
    /// Relative heights of the stack column (Master layout).
    pub stack_weights: Vec<f64>,
    /// Relative widths of Equal-layout grid columns.
    pub col_weights: Vec<f64>,
    /// Relative heights of Equal-layout grid rows.
    pub row_weights: Vec<f64>,
}

/// Smallest share any single split side may keep (e10s02). Keeps a dragged
/// boundary from collapsing a window into an unusable sliver.
pub const MIN_SPLIT_SHARE: f64 = 0.15;
const MAX_MASTER_RATIO: f64 = 1.0 - MIN_SPLIT_SHARE;
const MASTER_RATIO_DEFAULT: f64 = 0.6;

/// Clamp a master ratio into [MIN_SPLIT_SHARE, MAX_MASTER_RATIO]. NaN falls
/// back to the default — a poisoned ratio would make every rect NaN and
/// silently hide the whole layout. Infinity is clamped by the same bounds.
pub fn clamp_master_ratio(ratio: f64) -> f64 {
    if ratio.is_nan() {
        MASTER_RATIO_DEFAULT
    } else {
        ratio.clamp(MIN_SPLIT_SHARE, MAX_MASTER_RATIO)
    }
}

/// Normalize `weights` to exactly n entries summing to 1.0 (e10s02t1).
/// Missing entries start neutral; non-finite or below-floor entries get the
/// floor. The floor degrades to 1/n when there are too many entries for it to
/// be satisfiable, so seven stack windows still get valid, visible cells.
pub fn normalize_weights(weights: &[f64], n: usize, min_share: f64) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let floor = min_share.max(0.0).min(1.0 / n as f64);
    // Missing / invalid weights start neutral instead of starving at zero.
    // A floor is a floor, not a tax on the healthy cells: lift only the
    // violating ones and give their lost share back to the rest. Otherwise a
    // 3:1 split would silently become 67:33, which feels like dragging through
    // treacle near the limit. Treacle, incidentally, is a terrible WM feel.
    let cleaned: Vec<f64> = (0..n)
        .map(|i| match weights.get(i).copied() {
            Some(v) if v.is_finite() && v > 0.0 => v,
            _ => 1.0,
        })
        .collect();
    let total: f64 = cleaned.iter().sum();
    let mut out: Vec<f64> = if total.is_finite() && total > 0.0 {
        cleaned.iter().map(|v| v / total).collect()
    } else {
        vec![1.0 / n as f64; n]
    };
    for _ in 0..n {
        let bad: Vec<usize> = (0..n).filter(|&i| out[i] < floor).collect();
        if bad.is_empty() {
            break;
        }
        let free: Vec<usize> = (0..n).filter(|i| !bad.contains(i)).collect();
        let free_total = 1.0 - floor * bad.len() as f64;
        let free_sum: f64 = free.iter().map(|&i| out[i]).sum();
        for i in bad {
            out[i] = floor;
        }
        if free.is_empty() {
            break;
        }
        for i in &free {
            out[*i] = if free_sum > 0.0 {
                free_total * out[*i] / free_sum
            } else {
                free_total / free.len() as f64
            };
        }
    }
    let total: f64 = out.iter().sum();
    if total.is_finite() && total > 0.0 {
        for v in &mut out {
            *v /= total;
        }
    }
    out
}

/// Split `total` pixels among `weights`, reserving `gap` between parts
/// (e10s02t2). Weights are normalized first, so every cell gets at least the
/// configured share. Largest-remainder rounding hands out spare pixels so the
/// parts always sum to exactly `total - gaps` — no drift after repeated drags.
pub fn weighted_split(total: u32, gap: u32, weights: &[f64], min_share: f64) -> Vec<u32> {
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let available = total.saturating_sub(gap.saturating_mul((n - 1) as u32));
    let norm = normalize_weights(weights, n, min_share);
    let mut sizes = Vec::with_capacity(n);
    let mut fractions = Vec::with_capacity(n);
    let mut used: u32 = 0;
    for w in norm {
        let raw = available as f64 * w;
        let base = raw.floor() as u32;
        used = used.saturating_add(base);
        sizes.push(base);
        fractions.push(raw - raw.floor());
    }
    let mut spare = available.saturating_sub(used);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| fractions[b].total_cmp(&fractions[a]).then(a.cmp(&b)));
    for i in order {
        if spare == 0 {
            break;
        }
        sizes[i] += 1;
        spare -= 1;
    }
    sizes
}

/// Move a share of the combined weight of two adjacent cells from `second` to
/// `first` (e10s02t7). Each side keeps at least `MIN_SPLIT_SHARE` of the axis
/// total, so a violent drag cannot collapse a window into a sliver.
fn shift_pair(weights: &mut [f64], first: usize, second: usize, share: f64) -> bool {
    if first == second || first >= weights.len() || second >= weights.len() {
        return false;
    }
    let mut cleaned: Vec<f64> = weights
        .iter()
        .map(|&w| if w.is_finite() && w > 0.0 { w } else { 1.0 })
        .collect();
    let total: f64 = cleaned.iter().sum();
    let pair = cleaned[first] + cleaned[second];
    if !total.is_finite() || total <= 0.0 || pair <= 0.0 {
        return false;
    }
    let minimum = MIN_SPLIT_SHARE * total;
    if pair < 2.0 * minimum {
        cleaned[first] = pair / 2.0;
        cleaned[second] = pair / 2.0;
    } else {
        let step = share * pair;
        let grown = (cleaned[first] + step).clamp(minimum, pair - minimum);
        cleaned[first] = grown;
        cleaned[second] = pair - grown;
    }
    weights.copy_from_slice(&cleaned);
    true
}

/// Grid chosen by `equal_split`: minimize the larger dimension, then prefer
/// the squarest cell (e10s02t7). Shared with `adjust_split` so hit-testing and
/// weight edits agree on which column/row a boundary belongs to.
fn grid_for(n: usize, rect: Rect) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let mut best_rows = 1;
    let mut best_cols = n;
    let mut best_max_dim = n;
    let mut best_ratio_dev = f64::INFINITY;
    let screen_ratio = rect.width as f64 / rect.height as f64;
    for rows in 1..=n {
        let cols = n.div_ceil(rows);
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
    (best_rows, best_cols)
}

impl Default for TilingSplits {
    fn default() -> Self {
        Self::standard()
    }
}

impl TilingSplits {
    /// Defaults are chosen so the existing 3/5 Master layout is unchanged.
    pub fn standard() -> Self {
        Self {
            master_ratio: MASTER_RATIO_DEFAULT,
            stack_weights: Vec::new(),
            col_weights: Vec::new(),
            row_weights: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Output {
    pub id: OutputId,
    pub name: String,
    pub geometry: Rect,
    /// Exclusive-zone-aware work area in global coordinates. None = geometry.
    pub work_area: Option<Rect>,
    /// Dock bar height reserved from this output's own work area.
    pub dock_height: u32,
    pub active_workspace: WorkspaceId,
    pub tiling_layout: TilingLayout,
    /// Per-output split ratios (e10s02). Survives re-arrangement, workspace
    /// switches and output resizes for the lifetime of the session.
    pub splits: TilingSplits,
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
    /// Output under the pointer (e09s02). Stale ids are filtered out so an
    /// unplugged monitor cannot keep pretending to own the cursor.
    pointer_output: Option<OutputId>,
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

/// Which frame edge a resize drag grabbed (e10s01). One enum drives hit
/// testing, rect math, cursor choice and their tests, so none can drift.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResizeEdge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// Resolve the resize handle under `point` on an outer frame `rect`.
///
/// `padding` is an invisible grab zone outside the frame. Corners win over
/// edges. A point in the free interior (including the titlebar) returns None
/// so a titlebar press stays a move drag — otherwise grabbing the titlebar
/// would silently resize the window. Reality check: users notice that.
pub fn resize_edge_at(rect: Rect, point: (f64, f64), padding: i32) -> Option<ResizeEdge> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let pad = padding.max(0) as f64;
    let left = rect.x as f64;
    let top = rect.y as f64;
    let right = left + rect.width as f64;
    let bottom = top + rect.height as f64;
    let (px, py) = point;
    if px < left - pad || px > right + pad || py < top - pad || py > bottom + pad {
        return None;
    }
    // Shrink the near-edge zone on tiny frames so opposite edges cannot both
    // claim the same point.
    let pad_x = pad.min(rect.width as f64 / 2.0);
    let pad_y = pad.min(rect.height as f64 / 2.0);
    let west = px <= left + pad_x;
    let east = px >= right - pad_x;
    let north = py <= top + pad_y;
    let south = py >= bottom - pad_y;

    match (north, south, west, east) {
        (true, _, true, _) => Some(ResizeEdge::NorthWest),
        (true, _, _, true) => Some(ResizeEdge::NorthEast),
        (_, true, true, _) => Some(ResizeEdge::SouthWest),
        (_, true, _, true) => Some(ResizeEdge::SouthEast),
        (true, _, _, _) => Some(ResizeEdge::North),
        (_, true, _, _) => Some(ResizeEdge::South),
        (_, _, true, _) => Some(ResizeEdge::West),
        (_, _, _, true) => Some(ResizeEdge::East),
        _ => None,
    }
}

/// Which way a tiling split boundary runs (e10s02t6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    /// Boundary between left and right cells; dragged horizontally.
    Vertical,
    /// Boundary between upper and lower cells; dragged vertically.
    Horizontal,
}

/// An internal boundary between two arranged tiled windows (e10s02t6).
/// `first` is left/above, `second` is right/below, so `adjust_split` knows
/// which adjacent weights to move without guessing from window order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilingSplitEdge {
    pub axis: SplitAxis,
    pub first: WindowId,
    pub second: WindowId,
}

/// Find the internal split boundary nearest `point`, or None for an outer
/// frame or window interior (e10s02t6). Pairwise on already-arranged rects:
/// the same helper covers Master and Equal layouts without knowing the grid.
pub fn tiling_split_edge_at(
    rects: &[(WindowId, Rect)],
    point: (f64, f64),
    padding: f64,
) -> Option<TilingSplitEdge> {
    let pad = padding.max(0.0);
    let mut best: Option<(f64, TilingSplitEdge)> = None;
    for i in 0..rects.len() {
        for j in (i + 1)..rects.len() {
            let (id_a, a) = rects[i];
            let (id_b, b) = rects[j];
            let (a_right, b_right) = (
                a.x as f64 + a.width as f64,
                b.x as f64 + b.width as f64,
            );
            let (a_bottom, b_bottom) = (
                a.y as f64 + a.height as f64,
                b.y as f64 + b.height as f64,
            );
            let (b_left, b_top) = (b.x as f64, b.y as f64);
            let (px, py) = point;

            // Vertical: cells side by side, sharing vertical span.
            let y_from = (a.y as f64).max(b_top);
            let y_to = a_bottom.min(b_bottom);
            if y_to > y_from
                && py >= y_from - pad
                && py <= y_to + pad
                && (b_left - a_right).abs() <= pad * 2.0
            {
                let mid = (a_right + b_left) / 2.0;
                let dist = (px - mid).abs();
                if dist <= pad {
                    let edge = if a_right <= b_left {
                        TilingSplitEdge { axis: SplitAxis::Vertical, first: id_a, second: id_b }
                    } else {
                        TilingSplitEdge { axis: SplitAxis::Vertical, first: id_b, second: id_a }
                    };
                    if best.is_none_or(|(d, _)| dist < d) {
                        best = Some((dist, edge));
                    }
                }
            }

            // Horizontal: cells stacked, sharing horizontal span.
            let x_from = (a.x as f64).max(b_left);
            let x_to = a_right.min(b_right);
            if x_to > x_from
                && px >= x_from - pad
                && px <= x_to + pad
                && (b_top - a_bottom).abs() <= pad * 2.0
            {
                let mid = (a_bottom + b_top) / 2.0;
                let dist = (py - mid).abs();
                if dist <= pad {
                    let edge = if a_bottom <= b_top {
                        TilingSplitEdge { axis: SplitAxis::Horizontal, first: id_a, second: id_b }
                    } else {
                        TilingSplitEdge { axis: SplitAxis::Horizontal, first: id_b, second: id_a }
                    };
                    if best.is_none_or(|(d, _)| dist < d) {
                        best = Some((dist, edge));
                    }
                }
            }
        }
    }
    best.map(|(_, edge)| edge)
}

/// Apply a pointer delta to `orig` for the grabbed `edge`.
///
/// West/North drags move the origin while the opposite edge stays pinned.
/// All math is i64 so a huge delta cannot underflow the u32 Rect fields.
/// `min`/`max` are (width, height) size bounds, not screen coordinates.
pub fn resize_rect(
    orig: Rect,
    edge: ResizeEdge,
    dx: f64,
    dy: f64,
    min: (u32, u32),
    max: (u32, u32),
) -> Rect {
    use ResizeEdge::*;
    let min_w = min.0.max(1) as i64;
    let min_h = min.1.max(1) as i64;
    let max_w = (max.0 as i64).max(min_w);
    let max_h = (max.1 as i64).max(min_h);
    let right = orig.x as i64 + orig.width as i64;
    let bottom = orig.y as i64 + orig.height as i64;
    let int_range = |v: i64| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32;

    let (width, x) = match edge {
        West | NorthWest | SouthWest => {
            let w = ((orig.width as f64 - dx).round() as i64).clamp(min_w, max_w);
            (w, right - w)
        }
        East | NorthEast | SouthEast => {
            let w = ((orig.width as f64 + dx).round() as i64).clamp(min_w, max_w);
            (w, orig.x as i64)
        }
        North | South => (orig.width as i64, orig.x as i64),
    };
    let (height, y) = match edge {
        North | NorthEast | NorthWest => {
            let h = ((orig.height as f64 - dy).round() as i64).clamp(min_h, max_h);
            (h, bottom - h)
        }
        South | SouthEast | SouthWest => {
            let h = ((orig.height as f64 + dy).round() as i64).clamp(min_h, max_h);
            (h, orig.y as i64)
        }
        East | West => (orig.height as i64, orig.y as i64),
    };

    Rect {
        x: int_range(x),
        y: int_range(y),
        width: width as u32,
        height: height as u32,
    }
}

/// Full outer frame rectangle (borders on all sides + titlebar above).
/// Shared by hit-testing and resize-edge detection so they cannot drift.
pub fn outer_frame(target: Rect, border_size: u32, titlebar_height: u32) -> Rect {
    let bs = border_size as i32;
    let tb = titlebar_height as i32;
    Rect {
        x: target.x - bs,
        y: target.y - bs - tb,
        width: (target.width as i32 + 2 * bs).max(0) as u32,
        height: (target.height as i32 + 2 * bs + tb).max(0) as u32,
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

/// Compositor-side minimum window size (e10s01t7). These are the defaults;
/// `es.dsp.set_min_window_size` overrides them through DecorationConfig.
pub const MIN_WINDOW_WIDTH: u32 = 200;
pub const MIN_WINDOW_HEIGHT: u32 = 150;

/// Window decoration values shared between the Lua config API and the WM.
/// Lua mutates these via `es.dsp.set_*`; the renderer reads them each frame.
#[derive(Clone, Debug)]
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
    /// Minimum window width/height (px) enforced by move/resize drags
    /// (e10s01t7). Defaults to MIN_WINDOW_WIDTH/HEIGHT; Lua can override.
    pub min_window_width: u32,
    pub min_window_height: u32,
    /// Default wm mode for newly spawned windows:
    /// Tiling = tiled by default, Floating = floating by default.
    pub wm_mode: WmMode,
    /// Tiling layout algorithm for the output this decoration is applied to.
    pub tiling_layout: TilingLayout,
}

impl Default for DecorationConfig {
    fn default() -> Self {
        Self::standard()
    }
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
            min_window_width: MIN_WINDOW_WIDTH,
            min_window_height: MIN_WINDOW_HEIGHT,
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
                work_area: None,
                dock_height: 0,
                active_workspace: workspace,
                tiling_layout: TilingLayout::default(),
                splits: TilingSplits::standard(),
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

    /// Record which output the pointer is over. None means "outside every
    /// output", which lets window focus decide the focused output instead.
    // e09s02t6 wires this into udev pointer motion.
    #[allow(dead_code)]
    pub fn set_pointer_output(&mut self, output: Option<OutputId>) {
        self.pointer_output = output.filter(|id| self.outputs.contains_key(id));
    }

    /// The one true focused output: pointer first, then focused window, then
    /// first surviving output. Two "primaries" walk into a bar; only one gets
    /// served, and it is this one.
    // e09s02t2 migrates callers off primary_output_id().
    #[allow(dead_code)]
    pub fn focused_output_id(&self) -> Option<OutputId> {
        self.pointer_output
            .filter(|id| self.outputs.contains_key(id))
            .or_else(|| {
                self.focused
                    .and_then(|id| self.windows.get(&id))
                    .filter(|window| self.outputs.contains_key(&window.output))
                    .map(|window| window.output)
            })
            .or_else(|| self.outputs.keys().next().copied())
    }

    /// Work area of one output, falling back to its full geometry when no
    /// panel has reserved an exclusive zone yet.
    // e09s02t2 migrates layout callers off the single global work_area.
    #[allow(dead_code)]
    pub fn work_area_for_output(&self, id: OutputId) -> Rect {
        self.outputs
            .get(&id)
            .map(|output| output.work_area.unwrap_or(output.geometry))
            .unwrap_or_default()
    }

    /// Returns true only when the value actually changed, so callers can avoid
    /// redundant re-layouts (and avoid waking the compositor for nothing).
    #[allow(dead_code)]
    pub fn set_output_work_area(&mut self, id: OutputId, work_area: Option<Rect>) -> bool {
        let Some(output) = self.outputs.get_mut(&id) else {
            return false;
        };
        if output.work_area == work_area {
            return false;
        }
        output.work_area = work_area;
        self.dirty = true;
        true
    }

    /// Dock height used to be global, which is like giving every house the
    /// same doormat size regardless of how wide the door is.
    #[allow(dead_code)]
    pub fn set_output_dock_height(&mut self, id: OutputId, height: u32) -> bool {
        let Some(output) = self.outputs.get_mut(&id) else {
            return false;
        };
        if output.dock_height == height {
            return false;
        }
        output.dock_height = height;
        self.dirty = true;
        true
    }

    #[allow(dead_code)]
    pub fn dock_height_for_output(&self, id: OutputId) -> u32 {
        self.outputs.get(&id).map(|output| output.dock_height).unwrap_or(0)
    }

    /// Where an output with no configured position goes: just right of the
    /// rightmost output, sharing the first output's baseline. Auto-placement:
    /// because "every output is at 0,0" was a cozy but fraudulent arrangement.
    // e09s02t4 calls this from DRM connect; until then it is a very well-tested
    // wallflower at the method dance.
    #[allow(dead_code)]
    pub fn auto_output_position(&self) -> (i32, i32) {
        let Some(first) = self.outputs.values().next() else {
            return (0, 0);
        };
        let right = self
            .outputs
            .values()
            .map(|output| output.geometry.x.saturating_add(output.geometry.width as i32))
            .max()
            .unwrap_or(0);
        (right, first.geometry.y)
    }

    /// Bounding box of every output in global coordinates. Pointer clamping
    /// uses this instead of summing widths; outputs are neighbours, not a
    /// conga line pretending to be one screen.
    // e09s02t7 replaces udev's width-summing max_screen_size() with this.
    #[allow(dead_code)]
    pub fn outputs_bounds(&self) -> Option<Rect> {
        let mut outputs = self.outputs.values();
        let first = outputs.next()?;
        let (mut min_x, mut min_y) = (first.geometry.x, first.geometry.y);
        let (mut max_right, mut max_bottom) = (
            first.geometry.x.saturating_add(first.geometry.width as i32),
            first.geometry.y.saturating_add(first.geometry.height as i32),
        );
        for output in outputs {
            let geometry = output.geometry;
            min_x = min_x.min(geometry.x);
            min_y = min_y.min(geometry.y);
            max_right = max_right.max(geometry.x.saturating_add(geometry.width as i32));
            max_bottom = max_bottom.max(geometry.y.saturating_add(geometry.height as i32));
        }
        Some(Rect {
            x: min_x,
            y: min_y,
            width: max_right.saturating_sub(min_x) as u32,
            height: max_bottom.saturating_sub(min_y) as u32,
        })
    }

    /// Which output owns a global point? Right/bottom edges are exclusive so
    /// a pointer exactly on the seam belongs to the next output, not both.
    // e09s02t6 uses this to track the output under the pointer.
    #[allow(dead_code)]
    pub fn output_for_point(&self, x: i32, y: i32) -> Option<OutputId> {
        self.outputs
            .values()
            .find(|output| {
                let geometry = output.geometry;
                x >= geometry.x
                    && y >= geometry.y
                    && x < geometry.x.saturating_add(geometry.width as i32)
                    && y < geometry.y.saturating_add(geometry.height as i32)
            })
            .map(|output| output.id)
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

    /// Record the full window mode last configured to the client (e08s04t2).
    /// Placement is part of the xdg state contract, not just presentation.
    pub fn set_mode_sent(&mut self, id: WindowId, mode: ClientMode) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.mode_sent = Some(mode);
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
            // Hit test the outer frame (borders + titlebar), the same rect the
            // renderer draws and the resize-edge detector uses.
            let r = outer_frame(window.target_rect, border_size, self.titlebar_height);
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
        let was_minimized = window.minimized;

        // `mapped` means "the client has a buffer" and stays true while the
        // window is minimized. The explicit flag is the only restore signal.
        // BUG-2026-11-24-DOCK-RESTORE: checking `mapped` here left the window
        // hidden forever after a dock click. `mapped` was living a double life
        // and the window paid the rent.
        if was_minimized
            && let Some(w) = self.windows.get_mut(&id) {
                w.minimized = false;
            }

        self.focus_window(id);

        // If we restored a non-focused minimized window, focus_window already
        // set workspace.focused and w.focused = true. arrange_output so the
        // restored window gets a target_rect / configure.
        if was_minimized {
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

    #[allow(dead_code)] // public WM API; tests and future config surfaces use it
    /// e10s02t3: change the Master-layout split ratio for an output. The
    /// ratio is clamped into the usable range and applied on the next arrange
    /// pass. Floor (not round) to stay byte-identical with the old 3/5 split.
    pub fn set_master_ratio(&mut self, output: OutputId, ratio: f64) {
        let Some(state) = self.outputs.get_mut(&output) else {
            return;
        };
        state.splits.master_ratio = clamp_master_ratio(ratio);
        self.dirty = true;
        self.arrange_output(output);
    }

    #[allow(dead_code)] // public WM API; tests and future config surfaces use it
    /// e10s02t4: relative heights of the Master-layout stack column. An empty
    /// vector means equal heights, preserving the pre-e10 layout exactly.
    pub fn set_stack_weights(&mut self, output: OutputId, weights: Vec<f64>) {
        let Some(state) = self.outputs.get_mut(&output) else {
            return;
        };
        state.splits.stack_weights = weights;
        self.dirty = true;
        self.arrange_output(output);
    }

    #[allow(dead_code)] // public WM API; tests and future config surfaces use it
    /// e10s02t5: relative widths/heights of Equal-layout grid cells. Empty
    /// vectors mean the original equal split.
    pub fn set_equal_weights(&mut self, output: OutputId, cols: Vec<f64>, rows: Vec<f64>) {
        let Some(state) = self.outputs.get_mut(&output) else {
            return;
        };
        state.splits.col_weights = cols;
        state.splits.row_weights = rows;
        self.dirty = true;
        self.arrange_output(output);
    }

    /// e10s02t8: restore all split inputs for an output to their defaults.
    pub fn reset_tiling_splits(&mut self, output: OutputId) {
        let Some(state) = self.outputs.get_mut(&output) else {
            return;
        };
        state.splits = TilingSplits::standard();
        self.dirty = true;
        self.arrange_output(output);
    }

    /// Tiled windows on `output`'s active workspace, in arrangement order.
    fn arranged_tiled_ids(&self, output: OutputId) -> Vec<WindowId> {
        let Some(workspace_id) = self.outputs.get(&output).map(|o| o.active_workspace) else {
            return Vec::new();
        };
        let Some(workspace) = self.workspaces.get(&workspace_id) else {
            return Vec::new();
        };
        workspace
            .windows
            .iter()
            .filter_map(|id| self.windows.get(id))
            .filter(|w| {
                w.mapped
                    && !w.minimized
                    && w.mode.placement == Placement::Tiled
                    && w.mode.presentation == Presentation::Normal
            })
            .map(|w| w.id)
            .collect()
    }

    /// Arranged tiled rects for split hit-testing (e10s02t6).
    pub fn tiled_split_rects(&self, output: OutputId) -> Vec<(WindowId, Rect)> {
        self.arranged_tiled_ids(output)
            .into_iter()
            .filter_map(|id| self.windows.get(&id).map(|w| (id, w.target_rect)))
            .collect()
    }

    /// Inner geometry used by `arrange_output` for tiled cells.
    fn inner_rect_for_output(&self, output: OutputId) -> Option<Rect> {
        let state = self.outputs.get(&output)?;
        let base = self.work_area.unwrap_or(state.geometry);
        let bp = self.border_size as i32;
        let tb_h = self.titlebar_height as i32;
        Some(Rect {
            x: base.x + self.outer as i32 + bp,
            y: base.y + self.outer as i32 + bp + tb_h,
            width: base.width.saturating_sub(2 * self.outer).saturating_sub(2 * self.border_size),
            height: base
                .height
                .saturating_sub(2 * self.outer)
                .saturating_sub(self.titlebar_height)
                .saturating_sub(2 * self.border_size),
        })
    }

    /// e10s02t7: move an internal split by `delta_px` pixels. Positive delta
    /// grows the first (left/above) side. Only ratio/weight inputs change; the
    /// next arrange pass rebuilds every rect, so the layout stays consistent.
    pub fn adjust_split(&mut self, output: OutputId, edge: TilingSplitEdge, delta_px: f64) -> bool {
        let Some(state) = self.outputs.get(&output).cloned() else {
            return false;
        };
        let ids = self.arranged_tiled_ids(output);
        let Some(first) = ids.iter().position(|&id| id == edge.first) else {
            return false;
        };
        let Some(second) = ids.iter().position(|&id| id == edge.second) else {
            return false;
        };
        if first == second {
            return false;
        }
        let (a, b) = match (
            self.windows.get(&edge.first),
            self.windows.get(&edge.second),
        ) {
            (Some(a), Some(b)) => (a.target_rect, b.target_rect),
            _ => return false,
        };
        let Some(inner) = self.inner_rect_for_output(output) else {
            return false;
        };
        let gap_v = self.inner as i32 + self.titlebar_height as i32;
        let total_px = match edge.axis {
            SplitAxis::Vertical => a.width as f64 + b.width as f64 + self.inner as f64,
            SplitAxis::Horizontal => a.height as f64 + b.height as f64 + gap_v.max(0) as f64,
        };
        if !total_px.is_finite() || total_px <= 0.0 || !delta_px.is_finite() {
            return false;
        }
        let share = delta_px / total_px;

        let update = |splits: &mut TilingSplits| match (state.tiling_layout, edge.axis) {
            (TilingLayout::Master, SplitAxis::Vertical) if first == 0 && second == 1 => {
                splits.master_ratio = clamp_master_ratio(splits.master_ratio + share);
                true
            }
            (TilingLayout::Master, SplitAxis::Horizontal) if first >= 1 && second >= 1 => {
                let n = ids.len().saturating_sub(1);
                let mut weights = if splits.stack_weights.len() >= n {
                    splits.stack_weights.clone()
                } else {
                    vec![1.0; n]
                };
                if !shift_pair(&mut weights, first - 1, second - 1, share) {
                    return false;
                }
                splits.stack_weights = weights;
                true
            }
            (TilingLayout::Equal, SplitAxis::Vertical) => {
                let (_, cols) = grid_for(ids.len(), inner);
                if cols == 0 || first / cols != second / cols {
                    return false;
                }
                let mut weights = if splits.col_weights.len() >= cols {
                    splits.col_weights.clone()
                } else {
                    vec![1.0; cols]
                };
                if !shift_pair(&mut weights, first % cols, second % cols, share) {
                    return false;
                }
                splits.col_weights = weights;
                true
            }
            (TilingLayout::Equal, SplitAxis::Horizontal) => {
                let (_, cols) = grid_for(ids.len(), inner);
                if cols == 0 || first % cols != second % cols {
                    return false;
                }
                let rows = ids.len().div_ceil(cols);
                let mut weights = if splits.row_weights.len() >= rows {
                    splits.row_weights.clone()
                } else {
                    vec![1.0; rows]
                };
                if !shift_pair(&mut weights, first / cols, second / cols, share) {
                    return false;
                }
                splits.row_weights = weights;
                true
            }
            _ => false,
        };
        let Some(splits) = self.outputs.get_mut(&output).map(|o| &mut o.splits) else {
            return false;
        };
        if !update(splits) {
            return false;
        }
        self.dirty = true;
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
                    let ratio = output_state.splits.master_ratio;
                    ((inner.width as f64 * ratio).floor() as u64) as u32
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
                    let heights = if output_state.splits.stack_weights.is_empty() {
                        // Keep the pre-e10 integer-division layout byte-for-byte:
                        // it drops the remainder instead of handing it out.
                        let cell_h = (inner.height as i32 - gap_v * (stack.len() - 1) as i32)
                            / stack.len() as i32;
                        vec![cell_h as u32; stack.len()]
                    } else {
                        weighted_split(
                            inner.height,
                            gap_v.max(0) as u32,
                            &output_state.splits.stack_weights,
                            MIN_SPLIT_SHARE,
                        )
                    };
                    let mut top = inner.y;
                    for (id, height) in stack.iter().zip(heights) {
                        self.set_target_rect(
                            *id,
                            Rect {
                                x: stack_x,
                                y: top,
                                width: stack_w,
                                height,
                            },
                        );
                        top += height as i32 + gap_v;
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
                    equal_split(
                        &mut assigned,
                        &tiled,
                        inner,
                        inner_gap,
                        tb_h as u32,
                        if output_state.splits.col_weights.is_empty() {
                            None
                        } else {
                            Some(&output_state.splits.col_weights)
                        },
                        if output_state.splits.row_weights.is_empty() {
                            None
                        } else {
                            Some(&output_state.splits.row_weights)
                        },
                    );
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
    col_weights: Option<&[f64]>,
    row_weights: Option<&[f64]>,
) {
    let n = window_ids.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        assigned.push((window_ids[0], rect));
        return;
    }
    // Same grid choice as adjust_split, so dragging a boundary never changes
    // which column/row it belongs to.
    let (best_rows, best_cols) = grid_for(n, rect);
    // Vertical gap between rows must accommodate titlebars: a lower window's
    // titlebar extends upward from its target_rect, so it needs tb_h of space.
    let gap_v = gap + titlebar_height;
    // Empty or stale weight vectors fall back to the original integer split,
    // so an unchanged session keeps the exact pre-e10 geometry.
    let axis_sizes = |total: u32, count: usize, axis_gap: u32, weights: Option<&[f64]>| {
        match weights {
            Some(w) if w.len() >= count && !w.is_empty() => {
                weighted_split(total, axis_gap, &w[..count], MIN_SPLIT_SHARE)
            }
            _ => vec![
                total.saturating_sub(axis_gap * (count - 1) as u32) / count as u32;
                count
            ],
        }
    };
    let col_sizes = axis_sizes(rect.width, best_cols, gap, col_weights);
    let row_sizes = axis_sizes(rect.height, best_rows, gap_v, row_weights);
    for (i, &id) in window_ids.iter().enumerate() {
        let row = i / best_cols;
        let col = i % best_cols;
        let x = rect.x
            + col_sizes[..col]
                .iter()
                .map(|&w| w as i32 + gap as i32)
                .sum::<i32>();
        let y = rect.y
            + row_sizes[..row]
                .iter()
                .map(|&h| h as i32 + gap_v as i32)
                .sum::<i32>();
        assigned.push((
            id,
            Rect {
                x,
                y,
                width: col_sizes[col],
                height: row_sizes[row],
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, TilingLayout, WindowManager, WindowRule, equal_split, titlebar_button_at, TitlebarButton, ClientMode, Placement, Presentation, size_action, SizeAction, fill_scale, MAX_SIZE_RESENDS, SIZE_RESEND_INTERVAL, ResizeEdge, resize_edge_at, resize_rect, MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT, TilingSplits, clamp_master_ratio, normalize_weights, MIN_SPLIT_SHARE, weighted_split, SplitAxis, TilingSplitEdge, tiling_split_edge_at};
    use std::time::Duration;

    // e10s02t1: splits live on the output so they survive re-arrangement.
    // Per-window rect overrides cannot: arrange_output owns tiled geometry and
    // would erase them on the next dirty pass. Splits are layout inputs.
    #[test]
    fn tiling_splits_defaults_are_master_60_percent() {
        let d = TilingSplits::default();
        assert!((d.master_ratio - 0.6).abs() < f64::EPSILON);
        assert!(d.stack_weights.is_empty());
        assert!(d.col_weights.is_empty());
        assert!(d.row_weights.is_empty());
        assert!((MIN_SPLIT_SHARE - 0.15).abs() < f64::EPSILON);
    }

    // e10s02t2: one allocation helper serves Master stacks and Equal columns
    // and rows. Largest-remainder rounding makes the parts sum to the exact
    // available space — no drifting one-pixel gaps after repeated drags.
    #[test]
    fn weighted_split_allocates_equal_skewed_and_clamped() {
        assert_eq!(weighted_split(1000, 0, &[1.0, 1.0, 1.0], MIN_SPLIT_SHARE), vec![334, 333, 333]);
        assert_eq!(weighted_split(1000, 10, &[1.0, 1.0], MIN_SPLIT_SHARE), vec![495, 495]);
        assert_eq!(weighted_split(1000, 0, &[3.0, 1.0], MIN_SPLIT_SHARE), vec![750, 250]);
        // A zero weight is lifted to the floor instead of collapsing a cell.
        assert_eq!(weighted_split(1000, 0, &[1000.0, 0.0], MIN_SPLIT_SHARE), vec![850, 150]);
    }

    #[test]
    fn weighted_split_handles_degenerate_inputs() {
        assert!(weighted_split(1000, 0, &[], MIN_SPLIT_SHARE).is_empty());
        assert_eq!(weighted_split(0, 0, &[1.0, 1.0], MIN_SPLIT_SHARE), vec![0, 0]);
        // Gaps larger than the total must not underflow into a huge u32.
        assert_eq!(weighted_split(5, 10, &[1.0, 1.0], MIN_SPLIT_SHARE), vec![0, 0]);
        let parts = weighted_split(100, 0, &[f64::NAN, 1.0], MIN_SPLIT_SHARE);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts.iter().sum::<u32>(), 100);
    }

    #[test]
    fn tiling_splits_clamp_master_ratio_keeps_both_sides_usable() {
        assert!((clamp_master_ratio(0.3) - 0.3).abs() < f64::EPSILON);
        assert!((clamp_master_ratio(0.0) - MIN_SPLIT_SHARE).abs() < f64::EPSILON);
        assert!((clamp_master_ratio(1.0) - 0.85).abs() < f64::EPSILON);
        assert!((clamp_master_ratio(f64::NAN) - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn tiling_splits_normalize_weights_fills_clamps_and_sums_to_one() {
        let w = normalize_weights(&[1.0, 3.0], 4, MIN_SPLIT_SHARE);
        assert_eq!(w.len(), 4);
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(w.iter().all(|v| *v >= 0.0));
        assert!(w[1] > w[0], "explicit small weight must still shrink its cell");

        // A zero/NaN weight gets the floor, not a zero-width cell.
        let w = normalize_weights(&[0.0, f64::NAN, 1.0], 3, MIN_SPLIT_SHARE);
        assert!(w.iter().all(|v| v.is_finite() && *v > 0.0));
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);

        // Too many windows for the floor: the floor degrades to 1/n rather
        // than demanding more space than exists.
        let w = normalize_weights(&[], 10, MIN_SPLIT_SHARE);
        assert_eq!(w.len(), 10);
        assert!(w.iter().all(|v| (v - 0.1).abs() < 1e-9));

        assert!(normalize_weights(&[], 0, MIN_SPLIT_SHARE).is_empty());
    }

    // e10s01t1: hit-testing, rect math and cursors must all agree on which
    // edge is moving. Free titlebar space is a move handle, not a resize one —
    // otherwise grabbing the titlebar would silently resize the window.
    #[test]
    fn resize_edge_at_maps_frame_edges_corners_and_padding() {
        let frame = Rect { x: 100, y: 50, width: 200, height: 100 };
        let pad = 6;
        let (mx, my) = (200.0, 100.0);

        assert_eq!(resize_edge_at(frame, (mx, 50.0), pad), Some(ResizeEdge::North));
        assert_eq!(resize_edge_at(frame, (mx, 150.0), pad), Some(ResizeEdge::South));
        assert_eq!(resize_edge_at(frame, (100.0, my), pad), Some(ResizeEdge::West));
        assert_eq!(resize_edge_at(frame, (300.0, my), pad), Some(ResizeEdge::East));

        assert_eq!(resize_edge_at(frame, (100.0, 50.0), pad), Some(ResizeEdge::NorthWest));
        assert_eq!(resize_edge_at(frame, (300.0, 50.0), pad), Some(ResizeEdge::NorthEast));
        assert_eq!(resize_edge_at(frame, (100.0, 150.0), pad), Some(ResizeEdge::SouthWest));
        assert_eq!(resize_edge_at(frame, (300.0, 150.0), pad), Some(ResizeEdge::SouthEast));

        // Invisible grab zone outside the frame.
        assert_eq!(resize_edge_at(frame, (96.0, my), pad), Some(ResizeEdge::West));
        assert_eq!(resize_edge_at(frame, (304.0, my), pad), Some(ResizeEdge::East));
        assert_eq!(resize_edge_at(frame, (mx, 46.0), pad), Some(ResizeEdge::North));
        assert_eq!(resize_edge_at(frame, (mx, 154.0), pad), Some(ResizeEdge::South));

        // Free titlebar / client area and beyond the grab zone: no handle.
        assert_eq!(resize_edge_at(frame, (mx, 65.0), pad), None);
        assert_eq!(resize_edge_at(frame, (150.0, my), pad), None);
        assert_eq!(resize_edge_at(frame, (93.0, my), pad), None);
    }

    // e10s01t7: min-size defaults live in one place; the Lua setter overrides
    // them at runtime, so both standard() and default() must start consistent.
    #[test]
    fn min_window_size_defaults_match_constants() {
        assert_eq!(MIN_WINDOW_WIDTH, 200);
        assert_eq!(MIN_WINDOW_HEIGHT, 150);
        let dec = super::DecorationConfig::standard();
        assert_eq!(dec.min_window_width, MIN_WINDOW_WIDTH);
        assert_eq!(dec.min_window_height, MIN_WINDOW_HEIGHT);
        assert_eq!(super::DecorationConfig::default().min_window_width, MIN_WINDOW_WIDTH);
        assert_eq!(super::DecorationConfig::default().min_window_height, MIN_WINDOW_HEIGHT);
    }

    // e10s01t2: left/top drags move the origin while the opposite edge stays
    // pinned. Clamps must survive huge deltas without u32 underflow.
    #[test]
    fn resize_rect_moves_grabbed_edge_and_clamps() {
        let r = Rect { x: 100, y: 200, width: 300, height: 200 };
        let min = (MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT);
        let max = (1000, 800);

        assert_eq!(resize_rect(r, ResizeEdge::East, 40.0, 999.0, min, max),
                   Rect { x: 100, y: 200, width: 340, height: 200 });
        assert_eq!(resize_rect(r, ResizeEdge::South, 999.0, 30.0, min, max),
                   Rect { x: 100, y: 200, width: 300, height: 230 });

        assert_eq!(resize_rect(r, ResizeEdge::West, -40.0, 0.0, min, max),
                   Rect { x: 60, y: 200, width: 340, height: 200 });
        assert_eq!(resize_rect(r, ResizeEdge::North, 0.0, -30.0, min, max),
                   Rect { x: 100, y: 170, width: 300, height: 230 });
        assert_eq!(resize_rect(r, ResizeEdge::NorthWest, -40.0, -30.0, min, max),
                   Rect { x: 60, y: 170, width: 340, height: 230 });

        // Shrink clamps keep the right/bottom edges pinned.
        assert_eq!(resize_rect(r, ResizeEdge::West, 10_000.0, 0.0, min, max),
                   Rect { x: 200, y: 200, width: 200, height: 200 });
        assert_eq!(resize_rect(r, ResizeEdge::North, 0.0, 10_000.0, min, max),
                   Rect { x: 100, y: 250, width: 300, height: 150 });

        // Grow clamps.
        assert_eq!(resize_rect(r, ResizeEdge::East, 10_000.0, 0.0, min, max),
                   Rect { x: 100, y: 200, width: 1000, height: 200 });
        assert_eq!(resize_rect(r, ResizeEdge::South, 0.0, 10_000.0, min, max),
                   Rect { x: 100, y: 200, width: 300, height: 800 });

        // Tiny original, huge negative delta: min wins, no u32 underflow.
        let tiny = Rect { x: 0, y: 0, width: 1, height: 1 };
        // Dragging west/north inward (positive delta) yields a negative
        // intermediate width — the min clamp must catch it before the u32 cast.
        assert_eq!(resize_rect(tiny, ResizeEdge::NorthWest, 10_000.0, 10_000.0, min, max),
                   Rect { x: -199, y: -149, width: 200, height: 150 });

        // Fractional deltas land on whole pixels.
        assert_eq!(resize_rect(r, ResizeEdge::East, 10.6, 0.0, min, max).width, 311);
    }

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

    // e10s02t3: the default ratio must be byte-identical to the old 3/5 split,
    // then actually drive the master width when dragged. Integer 3/5 used to
    // floor 1916 -> 1149; keep that exact result for the default.
    #[test]
    fn master_ratio_drives_width_and_defaults_to_three_fifths() {
        let (mut manager, output) = manager();
        let master = manager.add_window(output, "kitty", "master").unwrap();
        let stack = manager.add_window(output, "kitty", "stack").unwrap();

        assert_eq!(manager.window(master).unwrap().target_rect.width, 1149);
        assert_eq!(manager.window(stack).unwrap().target_rect.x, 1151);

        manager.set_master_ratio(output, 0.3);
        assert_eq!(manager.window(master).unwrap().target_rect.width, 574);
        assert_eq!(manager.window(stack).unwrap().target_rect.x, 576);

        // The 15% floor keeps both columns usable.
        manager.set_master_ratio(output, 0.0);
        assert_eq!(manager.window(master).unwrap().target_rect.width, 287);
    }

    // e10s02t4: stack heights are layout inputs on the output. Default weights
    // reproduce the old equal split; explicit weights redistribute pixels while
    // keeping the 15% floor so the small side stays usable.
    #[test]
    fn stack_weights_drive_master_stack_heights() {
        let (mut manager, output) = manager();
        let _master = manager.add_window(output, "kitty", "master").unwrap();
        let top = manager.add_window(output, "kitty", "top").unwrap();
        let bottom = manager.add_window(output, "kitty", "bottom").unwrap();

        // Empty weights = old equal stack: 1076px inner height / 2 = 538.
        assert_eq!(manager.window(top).unwrap().target_rect.height, 538);
        assert_eq!(manager.window(bottom).unwrap().target_rect.y, 540);

        manager.set_stack_weights(output, vec![3.0, 1.0]);
        assert_eq!(manager.window(top).unwrap().target_rect.height, 807);
        assert_eq!(manager.window(bottom).unwrap().target_rect.y, 809);
        assert_eq!(manager.window(bottom).unwrap().target_rect.height, 269);

        // A 1000:1 weight must not flatten the small window to zero.
        manager.set_stack_weights(output, vec![1000.0, 1.0]);
        assert!(manager.window(bottom).unwrap().target_rect.height >= 161);
    }

    // e10s02t5: Equal-layout weights change cell sizes without changing the
    // chosen grid. Empty weights keep the old integer-division geometry.
    #[test]
    fn equal_split_skewed_weights_shift_grid_cells() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let mut cols = Vec::new();
        equal_split(&mut cols, &[1, 2], rect, 0, 0, Some(&[3.0, 1.0]), None);
        assert_eq!(cols[0].1.width, 1440);
        assert_eq!(cols[1].1.x, 1440);
        assert_eq!(cols[1].1.width, 480);

        // A tall rect makes the grid 2 rows x 1 column, so row weights apply.
        let tall = Rect { x: 0, y: 0, width: 540, height: 1080 };
        let mut rows = Vec::new();
        equal_split(&mut rows, &[1, 2], tall, 0, 30, None, Some(&[1.0, 3.0]));
        assert_eq!(rows[0].1.height, 263);
        assert_eq!(rows[1].1.y, 293);
        assert_eq!(rows[1].1.height, 787);
    }

    // e10s02t6: hit-testing works on the already-arranged rects, so the same
    // helper serves Master and Equal layouts without duplicating geometry.
    #[test]
    fn tiling_split_edge_detects_internal_boundaries_only() {
        let master = (1, Rect { x: 2, y: 2, width: 1149, height: 1076 });
        let top = (2, Rect { x: 1151, y: 2, width: 767, height: 538 });
        let bottom = (3, Rect { x: 1151, y: 540, width: 767, height: 538 });
        let rects = [master, top, bottom];

        let vertical = tiling_split_edge_at(&rects, (1151.0, 300.0), 6.0).unwrap();
        assert_eq!(vertical.axis, SplitAxis::Vertical);
        assert_eq!((vertical.first, vertical.second), (1, 2));

        let horizontal = tiling_split_edge_at(&rects, (1400.0, 540.0), 6.0).unwrap();
        assert_eq!(horizontal.axis, SplitAxis::Horizontal);
        assert_eq!((horizontal.first, horizontal.second), (2, 3));

        // Outer frame and window centres are not split boundaries.
        assert!(tiling_split_edge_at(&rects, (2.0, 300.0), 6.0).is_none());
        assert!(tiling_split_edge_at(&rects, (1400.0, 300.0), 6.0).is_none());
    }

    // e10s02t7: one adjust_split entry point moves whichever split the edge
    // names, and the 15% floor survives a violent drag.
    #[test]
    fn adjust_split_clamps_master_stack_and_grid() {
        let (mut manager, output) = manager();
        let master = manager.add_window(output, "kitty", "master").unwrap();
        let top = manager.add_window(output, "kitty", "top").unwrap();
        let bottom = manager.add_window(output, "kitty", "bottom").unwrap();

        let vertical = TilingSplitEdge { axis: SplitAxis::Vertical, first: master, second: top };
        manager.adjust_split(output, vertical, 100.0);
        assert!(manager.window(master).unwrap().target_rect.width > 1149);
        manager.adjust_split(output, vertical, 10000.0);
        assert_eq!(manager.window(master).unwrap().target_rect.width, 1628);
        assert!(manager.window(top).unwrap().target_rect.width >= 287);

        let horizontal = TilingSplitEdge { axis: SplitAxis::Horizontal, first: top, second: bottom };
        manager.adjust_split(output, horizontal, 50.0);
        assert!(manager.window(top).unwrap().target_rect.height > 538);
        assert!(manager.window(bottom).unwrap().target_rect.height < 538);
        manager.adjust_split(output, horizontal, -10000.0);
        assert!(manager.window(top).unwrap().target_rect.height >= 161);

        manager.set_tiling_layout(TilingLayout::Equal);
        let left = manager.add_window(output, "kitty", "left").unwrap();
        let right = manager.add_window(output, "kitty", "right").unwrap();
        let grid = TilingSplitEdge { axis: SplitAxis::Vertical, first: left, second: right };
        manager.adjust_split(output, grid, 100.0);
        assert!(manager.window(left).unwrap().target_rect.width > manager.window(right).unwrap().target_rect.width);
    }

    // e10s02t8: reset restores every split input on the output in one pass.
    #[test]
    fn reset_tiling_splits_restores_defaults() {
        let (mut manager, output) = manager();
        let master = manager.add_window(output, "kitty", "master").unwrap();
        let top = manager.add_window(output, "kitty", "top").unwrap();
        let _bottom = manager.add_window(output, "kitty", "bottom").unwrap();

        manager.set_master_ratio(output, 0.2);
        manager.set_stack_weights(output, vec![4.0, 1.0]);
        manager.set_equal_weights(output, vec![9.0, 1.0], vec![8.0, 2.0]);
        assert!((manager.output(output).unwrap().splits.master_ratio - 0.2).abs() < f64::EPSILON);

        manager.reset_tiling_splits(output);
        let splits = &manager.output(output).unwrap().splits;
        assert!((splits.master_ratio - 0.6).abs() < f64::EPSILON);
        assert!(splits.stack_weights.is_empty());
        assert!(splits.col_weights.is_empty());
        assert!(splits.row_weights.is_empty());
        assert_eq!(manager.window(master).unwrap().target_rect.width, 1149);
        assert_eq!(manager.window(top).unwrap().target_rect.height, 538);
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
        equal_split(&mut assigned, &[1], rect, 0, 0, None, None);

        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].0, 1);
        assert_eq!(assigned[0].1, Rect { x: 0, y: 0, width: 1920, height: 1080 });
    }

    #[test]
    fn equal_split_two_windows_split_vertically() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let mut assigned: Vec<(u64, Rect)> = Vec::new();
        equal_split(&mut assigned, &[1, 2], rect, 0, 0, None, None);

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
        equal_split(&mut assigned, &[1, 2, 3], rect, 0, 0, None, None);

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
        equal_split(&mut assigned, &[1, 2, 3, 4], rect, 10, 0, None, None);

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
        equal_split(&mut assigned, &[1, 2], rect, 0, 30, None, None);

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
        equal_split(&mut assigned, &[1, 2], rect, 0, 30, None, None);

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
    fn only_tiled_normal_windows_advertise_tiled_state() {
        assert!(ClientMode::tiled().sends_tiled_state());
        assert!(!ClientMode::floating().sends_tiled_state());
        assert!(!ClientMode::maximized().sends_tiled_state());
        assert!(!ClientMode::fullscreen().sends_tiled_state());
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

    // BUG-2026-11-24-DOCK-RESTORE: a dock click must clear minimized, not just focus.
    #[test]
    fn dock_restore_unminimizes_a_minimized_window() {
        let (mut manager, output) = manager();
        let id = manager.add_window(output, "kitty", "one").unwrap();
        let other = manager.add_window(output, "kitty", "two").unwrap();
        manager.focus_window(other);
        assert!(manager.toggle_minimize(id));
        assert!(manager.window(id).unwrap().minimized);
        assert!(!manager.window(id).unwrap().focused);

        assert!(manager.focus_window_or_restore(id));

        let w = manager.window(id).unwrap();
        assert!(!w.minimized, "dock click must un-minimize the window");
        assert!(w.focused, "restored window must take focus");
        assert_eq!(manager.focused_id(), Some(id));
        assert!(!manager.dock_entries().iter().find(|e| e.id == id).unwrap().minimized);
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

    #[test]
    fn border_frame_hole_spans_titlebar_and_client() {
        // The rounded ring's hole is inset by one border width on every side.
        // It must cover exactly the titlebar band plus the client rect, so a
        // titlebar can own the hole's top corners with no gap.
        let target = Rect { x: 100, y: 200, width: 800, height: 600 };
        let bs = 2u32;
        let tb = 24u32;
        let frame = super::border_frame(target, bs, tb);
        let frame_h = frame.bottom.y + bs as i32 - frame.top.y;
        let hole_w = frame.top.width as i32 - 2 * bs as i32;
        let hole_h = frame_h - 2 * bs as i32;
        assert_eq!(frame.top.y, 200 - 2 - 24);
        assert_eq!(frame_h, 600 + 2 * 2 + 24);
        assert_eq!(hole_w, 800);
        assert_eq!(hole_h, 600 + 24);
    }

    // e09s02t1 — outputs stop pretending they all live at (0,0); each new
    // connector gets an actual seat in the global coordinate space.
    #[test]
    fn output_model_auto_position_places_next_output_to_the_right() {
        let mut manager = WindowManager::new();
        assert_eq!(manager.auto_output_position(), (0, 0));

        manager.add_output(
            "laptop",
            Rect { x: 0, y: 0, width: 1920, height: 1080 },
        );
        assert_eq!(manager.auto_output_position(), (1920, 0));

        manager.add_output(
            "hdmi",
            Rect { x: 1920, y: 0, width: 1280, height: 720 },
        );
        assert_eq!(manager.auto_output_position(), (3200, 0));
    }

    // e09s02t1 — pointer travel is a rectangle spanning real outputs, not the
    // sum of their widths stacked at the origin like a badly shuffled deck.
    #[test]
    fn output_model_bounds_span_displaced_outputs() {
        let mut manager = WindowManager::new();
        assert_eq!(manager.outputs_bounds(), None);

        manager.add_output(
            "laptop",
            Rect { x: 0, y: 0, width: 1920, height: 1080 },
        );
        assert_eq!(
            manager.outputs_bounds(),
            Some(Rect { x: 0, y: 0, width: 1920, height: 1080 })
        );

        manager.add_output(
            "hdmi",
            Rect { x: 1920, y: 0, width: 1280, height: 720 },
        );
        assert_eq!(
            manager.outputs_bounds(),
            Some(Rect { x: 0, y: 0, width: 3200, height: 1080 })
        );

        manager.add_output(
            "portrait",
            Rect { x: -400, y: 100, width: 300, height: 700 },
        );
        assert_eq!(
            manager.outputs_bounds(),
            Some(Rect { x: -400, y: 0, width: 3600, height: 1080 })
        );
    }

    // e09s02t1 — hit-testing the output under the pointer must respect each
    // geometry's origin; the old model thought the whole world started at 0.
    #[test]
    fn output_model_output_for_point_respects_origins_and_edges() {
        let mut manager = WindowManager::new();
        let laptop = manager.add_output(
            "laptop",
            Rect { x: 0, y: 0, width: 1920, height: 1080 },
        );
        let hdmi = manager.add_output(
            "hdmi",
            Rect { x: 2000, y: 0, width: 1280, height: 720 },
        );

        assert_eq!(manager.output_for_point(100, 100), Some(laptop));
        assert_eq!(manager.output_for_point(1919, 1079), Some(laptop));
        assert_eq!(manager.output_for_point(1920, 100), None);
        assert_eq!(manager.output_for_point(2000, 0), Some(hdmi));
        assert_eq!(manager.output_for_point(3279, 719), Some(hdmi));
        assert_eq!(manager.output_for_point(3280, 100), None);
    }

    // e09s02t1 — one focused output, chosen by the pointer first and the
    // focused window second. No more "first output" vs "last connected"
    // captain-with-two-hats nonsense.
    #[test]
    fn output_model_focused_output_prefers_pointer_then_focused_window() {
        let mut manager = WindowManager::new();
        assert_eq!(manager.focused_output_id(), None);

        let first = manager.add_output(
            "laptop",
            Rect { x: 0, y: 0, width: 1920, height: 1080 },
        );
        let second = manager.add_output(
            "hdmi",
            Rect { x: 1920, y: 0, width: 1280, height: 720 },
        );
        let window = manager.add_window(second, "kitty", "one").unwrap();
        manager.focus_window(window);
        assert_eq!(manager.focused_output_id(), Some(second));

        manager.set_pointer_output(Some(first));
        assert_eq!(manager.focused_output_id(), Some(first));

        manager.set_pointer_output(None);
        assert_eq!(manager.focused_output_id(), Some(second));

        manager.remove_output(second);
        assert_eq!(manager.focused_output_id(), Some(first));
    }

    // e09s02t1 — work area and dock height belong to the output whose panel
    // reserved them, not to whichever connector happened to arrive last.
    #[test]
    fn output_model_work_area_and_dock_are_per_output() {
        let mut manager = WindowManager::new();
        let first = manager.add_output(
            "laptop",
            Rect { x: 0, y: 0, width: 1920, height: 1080 },
        );
        let second = manager.add_output(
            "hdmi",
            Rect { x: 1920, y: 0, width: 1280, height: 720 },
        );

        let first_geometry = manager.output(first).unwrap().geometry;
        let second_geometry = manager.output(second).unwrap().geometry;
        assert_eq!(manager.work_area_for_output(first), first_geometry);
        assert_eq!(manager.work_area_for_output(second), second_geometry);

        let hdmi_area = Rect { x: 1920, y: 40, width: 1280, height: 600 };
        assert!(manager.set_output_work_area(second, Some(hdmi_area)));
        assert!(!manager.set_output_work_area(second, Some(hdmi_area)));
        assert_eq!(manager.work_area_for_output(first), first_geometry);
        assert_eq!(manager.work_area_for_output(second), hdmi_area);

        assert!(manager.set_output_dock_height(second, 48));
        assert_eq!(manager.dock_height_for_output(first), 0);
        assert_eq!(manager.dock_height_for_output(second), 48);
    }
}
