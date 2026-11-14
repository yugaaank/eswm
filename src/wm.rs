use std::collections::BTreeMap;

pub type OutputId = u64;
pub type WindowId = u64;
pub type WorkspaceId = i32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowMode {
    #[default]
    Tiled,
    Floating,
    Maximized,
    Fullscreen,
}

/// Global spawn/layout policy: Hyprland (tiled) or Windows (floating).
/// Switching it only affects newly spawned windows; Super+Space still moves
/// individual windows between tiling and floating.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LayoutMode {
    #[default]
    Hyprland,
    Windows,
}

#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    pub output: OutputId,
    pub workspace: WorkspaceId,
    pub mode: WindowMode,
    pub mapped: bool,
    pub focused: bool,
    pub rect: Rect,
    pub target_rect: Rect,
    /// xdg window geometry offset from set_window_geometry; render position = target_rect - offset
    pub xdg_offset: (i32, i32),
    /// Focus state last sent to the client as xdg Activated; None = never sent.
    pub activated_sent: Option<bool>,
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
}



#[derive(Default)]
pub struct WindowManager {
    next_output: OutputId,
    next_window: WindowId,
    outputs: BTreeMap<OutputId, Output>,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    windows: BTreeMap<WindowId, Window>,
    layout_mode: LayoutMode,
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
    /// Tiling work area, excluding exclusive layer-surface zones (waybar, panels).
    /// None = use full output geometry.
    pub work_area: Option<Rect>,
    /// App-id/class → workspace/mode rules, matched at window creation.
    rules: Vec<WindowRule>,
}

/// A window rule: match a window's app_id and assign a workspace and/or mode.
#[derive(Clone, Debug)]
pub struct WindowRule {
    pub app_id: String,
    pub workspace: Option<WorkspaceId>,
    pub mode: Option<WindowMode>,
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
    /// Default layout mode for newly spawned windows:
    /// Hyprland = tiled by default, Windows = floating by default.
    pub layout_mode: LayoutMode,
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
            layout_mode: LayoutMode::Hyprland,
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
            },
        );
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
        if active_is_empty {
            if let Some(workspace) = self
                .workspaces
                .values()
                .find(|workspace| workspace.output == survivor && !workspace.windows.is_empty())
                .map(|workspace| workspace.id)
            {
                if let Some(output) = self.outputs.get_mut(&survivor) {
                    output.active_workspace = workspace;
                }
            }
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

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
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
        self.windows.values().find(|w| w.focused).map(|w| w.id)
    }

    pub fn set_rect(&mut self, id: WindowId, rect: Rect) -> bool {
        if let Some(window) = self.windows.get_mut(&id) {
            window.rect = rect;
            return true;
        }
        false
    }

    pub fn set_xdg_offset(&mut self, id: WindowId, offset: (i32, i32)) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.xdg_offset = offset;
        }
    }

    pub fn set_activated_sent(&mut self, id: WindowId, sent: bool) {
        if let Some(window) = self.windows.get_mut(&id) {
            window.activated_sent = Some(sent);
        }
    }

    pub fn set_window_meta(&mut self, id: WindowId, app_id: String, title: String) -> bool {
        if let Some(window) = self.windows.get_mut(&id) {
            window.app_id = app_id;
            window.title = title;
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
        self.clear_focus_on_output(output);
        if let Some(window) = self
            .workspaces
            .get(&workspace)
            .and_then(|workspace| workspace.focused)
        {
            self.focus_window(window);
        }
        true
    }

    pub fn layout_mode(&self) -> LayoutMode {
        self.layout_mode
    }

    pub fn set_layout_mode(&mut self, mode: LayoutMode) {
        self.layout_mode = mode;
    }

    pub fn set_outer(&mut self, outer: u32) {
        self.outer = outer;
    }

    pub fn outer(&self) -> u32 {
        self.outer
    }

    pub fn set_inner(&mut self, inner: u32) {
        self.inner = inner;
    }

    pub fn inner(&self) -> u32 {
        self.inner
    }

    pub fn set_border_size(&mut self, size: u32) {
        self.border_size = size;
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
    }

    pub fn titlebar_color_focused(&self) -> [u8; 4] {
        self.titlebar_color_focused
    }

    pub fn titlebar_color_unfocused(&self) -> [u8; 4] {
        self.titlebar_color_unfocused
    }

    pub fn set_titlebar_height(&mut self, height: u32) {
        self.titlebar_height = height;
    }

    pub fn titlebar_height(&self) -> u32 {
        self.titlebar_height
    }

    pub fn add_window_rule(&mut self, rule: WindowRule) {
        self.rules.push(rule);
    }

    /// Match a window's app_id against configured rules; return (workspace, mode).
    fn match_rule(&self, app_id: &str) -> (Option<WorkspaceId>, Option<WindowMode>) {
        for rule in &self.rules {
            if rule.app_id == app_id {
                return (rule.workspace, rule.mode);
            }
        }
        (None, None)
    }

    pub fn toggle_layout_mode(&mut self) -> LayoutMode {
        self.layout_mode = match self.layout_mode {
            LayoutMode::Hyprland => LayoutMode::Windows,
            LayoutMode::Windows => LayoutMode::Hyprland,
        };
        self.layout_mode
    }

    /// Toggle a single window between tiling and floating, regardless of global mode.
    pub fn toggle_window_mode(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        let mode = match window.mode {
            WindowMode::Tiled => WindowMode::Floating,
            WindowMode::Floating | WindowMode::Maximized | WindowMode::Fullscreen => WindowMode::Tiled,
        };
        self.set_mode(id, mode)
    }

    /// Windows-like spawn placement: centered, cascaded.
    fn floating_target(&mut self, geometry: Rect) -> Rect {
        let width = (geometry.width as u64 * 62 / 100).max(640) as u32;
        let height = (geometry.height as u64 * 68 / 100).max(480) as u32;
        let offset = ((self.cascade_count % 6) * 28) as i32;
        self.cascade_count = self.cascade_count.wrapping_add(1);
        let x = geometry.x + ((geometry.width - width) / 2) as i32 + offset;
        let y = geometry.y + ((geometry.height - height) / 2) as i32 + offset;
        Rect { x, y, width, height }
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
            if focused.map_or(true, |f| *id != f) {
                ordered.push(*id);
            }
        }
        for id in ordered {
            let Some(window) = self.windows.get(&id) else { continue };
            if !window.mapped {
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
        if window.mode == WindowMode::Tiled {
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
        let mode = rule_mode.unwrap_or_else(|| match self.layout_mode {
            LayoutMode::Hyprland => WindowMode::Tiled,
            LayoutMode::Windows => WindowMode::Floating,
        });
        let id = self.next_window;
        self.next_window += 1;
        let geometry = self.outputs.get(&output)?.geometry;
        let floating_rect = match mode {
            WindowMode::Floating => Some(self.floating_target(geometry)),
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
                focused: false,
                rect: Rect::default(),
                target_rect: floating_rect.unwrap_or_default(),
                activated_sent: None,
                xdg_offset: (0, 0),
            },
        );
        if let Some(target) = self.workspaces.get_mut(&workspace) {
            target.windows.push(id);
        } else {
            return None;
        }
        self.workspaces.get_mut(&workspace)?.focused = Some(id);
        self.focus_window(id);
        self.arrange_output(output);
        Some(id)
    }

    pub fn remove_window(&mut self, id: WindowId) -> Option<Window> {
        let window = self.windows.remove(&id)?;
        if let Some(workspace) = self.workspaces.get_mut(&window.workspace) {
            workspace.windows.retain(|window_id| *window_id != id);
            if workspace.focused == Some(id) {
                workspace.focused = workspace.windows.last().copied();
            }
        }
        self.clear_focus_on_output(window.output);
        if let Some(focused) = self
            .workspaces
            .get(&window.workspace)
            .and_then(|workspace| workspace.focused)
        {
            self.focus_window(focused);
        }
        self.arrange_output(window.output);
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
        if let Some(window) = self.windows.get_mut(&id) {
            window.focused = true;
        }
        if let Some(workspace) = self.workspaces.get_mut(&workspace) {
            workspace.focused = Some(id);
        }
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
        }
        self.clear_focus_on_output(window.output);
        // Keep keyboard focus alive: the moved window may have been the focused one.
        if let Some(focused) = self
            .workspaces
            .get(&window.workspace)
            .and_then(|workspace| workspace.focused)
        {
            self.focus_window(focused);
        }
        self.arrange_output(window.output);
        true
    }

    /// Leaving fullscreen always returns the window to Tiled; no mode history.
    pub fn toggle_fullscreen(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        let mode = match window.mode {
            WindowMode::Fullscreen => WindowMode::Tiled,
            _ => WindowMode::Fullscreen,
        };
        self.set_mode(id, mode)
    }

    pub fn toggle_maximized(&mut self, id: WindowId) -> bool {
        let Some(window) = self.windows.get(&id) else {
            return false;
        };
        let mode = match window.mode {
            WindowMode::Maximized => WindowMode::Tiled,
            _ => WindowMode::Maximized,
        };
        self.set_mode(id, mode)
    }

    /// Toggle a window's mapped state (minimize/restore).
    /// When minimizing the focused window, focus stays on it so `toggle_minimize`
    /// (e.g. `Super+M`) toggles the same window back. `cycle_focus` skips
    /// unmapped windows so the invisible window won't interrupt focus cycling.
    /// When restoring a non-focused minimized window, it receives focus.
    pub fn toggle_minimize(&mut self, id: WindowId) -> bool {
        let (output, workspace, was_focused, was_mapped) = {
            let Some(window) = self.windows.get(&id) else {
                return false;
            };
            (
                window.output,
                window.workspace,
                window.focused,
                window.mapped,
            )
        };

        if let Some(window) = self.windows.get_mut(&id) {
            window.mapped = !window.mapped;
        }

        if was_focused && was_mapped {
            // Minimizing the focused window: keep focus on it (invisible now).
            // Super+M again restores it since focused_id() still returns it.
        } else if !was_focused && !was_mapped {
            // Restoring a non-focused minimized window: give it focus.
            self.clear_focus_on_output(output);
            if let Some(ws) = self.workspaces.get_mut(&workspace) {
                ws.focused = Some(id);
                if let Some(w) = self.windows.get_mut(&id) {
                    w.focused = true;
                }
            }
        }
        // was_focused && !was_mapped: restoring a focused minimized window —
        // focus stays on it, nothing to do.

        self.arrange_output(output);
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

    pub fn set_mode(&mut self, id: WindowId, mode: WindowMode) -> bool {
        let Some(window) = self.windows.get_mut(&id) else {
            return false;
        };
        window.mode = mode;
        let output = window.output;
        self.arrange_output(output);
        true
    }

    pub fn arrange_output(&mut self, output: OutputId) {
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
            .find(|window| matches!(window.mode, WindowMode::Fullscreen | WindowMode::Maximized));
        if let Some(window) = maximized {
            // Fullscreen takes the entire output. Maximized fills the work area
            // (respects waybar/quickshell exclusive zones) minus the outer gap,
            // so borders/titlebar still show and the window doesn't cover panels.
            // For maximized windows, target_rect accounts for titlebar height so
            // the entire window (borders + titlebar + surface) fits within
            // work_area minus the outer gap — the titlebar does not extend above
            // work_area and cover waybar/quickshell panels.
            let rect = if matches!(window.mode, WindowMode::Fullscreen) {
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
            .filter(|window| window.mapped && window.mode == WindowMode::Tiled)
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
        let inner = Rect {
            x: base.x + outer as i32,
            y: base.y + outer as i32,
            width: base.width.saturating_sub(2 * outer),
            height: base.height.saturating_sub(2 * outer),
        };
        let master_width = if tiled.len() == 1 {
            inner.width
        } else {
            ((inner.width as u64 * 3) / 5) as u32
        };
        let stack_width = inner.width.saturating_sub(master_width).saturating_sub(inner_gap);
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

        let stack = &tiled[1..];
        if !stack.is_empty() {
            let stack_x = inner.x + master_width as i32 + inner_gap as i32;
            // Split inner height across stack windows with a gap between each.
            let stack_h_inner = inner.height.saturating_sub((stack.len() as u32 + 1) * inner_gap);
            let cell_h = stack_h_inner / stack.len() as u32;
            for (index, id) in stack.iter().enumerate() {
                let top =
                    inner.y + inner_gap as i32 + (index as u32 * (cell_h + inner_gap)) as i32;
                self.set_target_rect(
                    *id,
                    Rect {
                        x: stack_x,
                        y: top,
                        width: stack_width,
                        height: cell_h,
                    },
                );
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
            .filter(|id| self.windows.get(id).map(|w| w.mapped).unwrap_or(false))
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
        for window in self
            .windows
            .values_mut()
            .filter(|window| window.output == output)
        {
            window.focused = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, WindowManager, WindowMode, WindowRule};

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

        assert_eq!(manager.window(first).unwrap().target_rect.width, 1152);
        assert_eq!(manager.window(second).unwrap().target_rect.x, 1152);
        assert_eq!(manager.window(second).unwrap().target_rect.height, 540);
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
        assert!(manager.set_mode(floating, WindowMode::Floating));

        assert_eq!(manager.window(tiled).unwrap().target_rect.width, 1920);
        assert_eq!(manager.window(floating).unwrap().mode, WindowMode::Floating);
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
        assert_eq!(window.target_rect.width, 1920);
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
        assert!(manager.window(second).unwrap().focused == false);
    }

    #[test]
    fn fullscreen_fills_the_output_and_returns_to_tiled() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        assert_eq!(manager.window(second).unwrap().target_rect.width, 768);

        assert!(manager.toggle_fullscreen(second));
        assert_eq!(manager.window(second).unwrap().mode, WindowMode::Fullscreen);
        assert_eq!(manager.window(second).unwrap().target_rect.width, 1920);

        assert!(manager.toggle_fullscreen(second));
        assert_eq!(manager.window(second).unwrap().mode, WindowMode::Tiled);
        assert_eq!(manager.window(first).unwrap().target_rect.width, 1152);
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

        // Master should be inset by gap from output edge.
        let m = manager.window(first).unwrap().target_rect;
        assert_eq!(m.x, 10);
        assert_eq!(m.y, 10);
        assert_eq!(m.width, 1140); // inner.width = 1900, 1900*3/5 = 1140
        assert_eq!(m.height, 1060); // 1080 - 20

        // Stack window should be to the right, inset.
        let s = manager.window(second).unwrap().target_rect;
        assert_eq!(s.x, 10 + 1140 + 10); // inner.x + master_width + inner_gap
        assert_eq!(s.y, 20); // inner.y + gap
        assert_eq!(s.width, 750); // 1900 - 1140 - 10
        assert_eq!(s.height, 1040); // (1060 - 2*10) / 1 = 1040
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
            mode: Some(WindowMode::Floating),
        });
        let id = manager.add_window(output, "fuzzel", "one").unwrap();
        assert_eq!(manager.window(id).unwrap().mode, WindowMode::Floating);
    }

    #[test]
    fn window_rule_no_match_keeps_default() {
        let (mut manager, output) = manager();
        manager.add_window_rule(WindowRule {
            app_id: "other".into(),
            workspace: Some(2),
            mode: Some(WindowMode::Floating),
        });
        let id = manager.add_window(output, "kitty", "one").unwrap();
        assert_eq!(manager.window(id).unwrap().workspace, 1); // default active workspace
        assert_eq!(manager.window(id).unwrap().mode, WindowMode::Tiled); // default Hyprland mode
    }

    #[test]
    fn focusing_floating_window_keeps_stack_order_for_cycling() {
        let (mut manager, output) = manager();
        let first = manager.add_window(output, "kitty", "one").unwrap();
        let second = manager.add_window(output, "kitty", "two").unwrap();
        manager.set_mode(first, WindowMode::Floating);
        manager.set_mode(second, WindowMode::Floating);
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
}
