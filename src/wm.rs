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
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            next_output: 1,
            next_window: 1,
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
    pub fn window_at(&self, pos: (f64, f64)) -> Option<WindowId> {
        let primary = self.primary_output_id()?;
        let output_state = self.outputs.get(&primary)?;
        let workspace = self.workspaces.get(&output_state.active_workspace)?;
        for id in workspace.windows.iter().rev() {
            let Some(window) = self.windows.get(id) else { continue };
            if !window.mapped {
                continue;
            }
            let r = window.target_rect;
            if r.width == 0 || r.height == 0 {
                continue;
            }
            let inside = pos.0 >= r.x as f64
                && pos.1 >= r.y as f64
                && pos.0 < r.x as f64 + r.width as f64
                && pos.1 < r.y as f64 + r.height as f64;

            if inside {
                return Some(*id);
            }
        }
        None
    }

    /// Move/resize a window directly (drag in floating mode). `rect` replaces
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
        let workspace = self.outputs.get(&output)?.active_workspace;
        let mode = match self.layout_mode {
            LayoutMode::Hyprland => WindowMode::Tiled,
            LayoutMode::Windows => WindowMode::Floating,
        };
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
                app_id: app_id.into(),
                title: title.into(),
                output,
                workspace,
                mode,
                mapped: true,
                focused: false,
                rect: Rect::default(),
                target_rect: floating_rect.unwrap_or_default(),
                activated_sent: None,
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
        self.arrange_output(window.output);
        true
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
        let fullscreen = workspace
            .windows
            .iter()
            .filter_map(|id| self.windows.get(id))
            .find(|window| matches!(window.mode, WindowMode::Fullscreen | WindowMode::Maximized))
            .map(|window| window.id);
        if let Some(id) = fullscreen {
            self.set_target_rect(id, output_state.geometry);
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
        let master_width = if tiled.len() == 1 {
            output_state.geometry.width
        } else {
            ((output_state.geometry.width as u64 * 3) / 5) as u32
        };
        let stack_width = output_state.geometry.width.saturating_sub(master_width);
        self.set_target_rect(
            tiled[0],
            Rect {
                width: master_width,
                ..output_state.geometry
            },
        );
        // floating windows keep their own geometry; the tiler ignores them

        let stack = &tiled[1..];
        for (index, id) in stack.iter().enumerate() {
            let top =
                (output_state.geometry.height as u64 * index as u64 / stack.len() as u64) as u32;
            let bottom = (output_state.geometry.height as u64 * (index as u64 + 1)
                / stack.len() as u64) as u32;
            self.set_target_rect(
                *id,
                Rect {
                    x: output_state.geometry.x + master_width as i32,
                    y: output_state.geometry.y + top as i32,
                    width: stack_width,
                    height: bottom.saturating_sub(top),
                },
            );
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
        let windows = &workspace.windows;
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
    use super::{Rect, WindowManager, WindowMode};

    fn manager() -> (WindowManager, u64) {
        let mut manager = WindowManager::new();
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
}
