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

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
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

    pub fn add_window(
        &mut self,
        output: OutputId,
        app_id: impl Into<String>,
        title: impl Into<String>,
    ) -> Option<WindowId> {
        let workspace = self.outputs.get(&output)?.active_workspace;
        let id = self.next_window;
        self.next_window += 1;
        self.windows.insert(
            id,
            Window {
                id,
                app_id: app_id.into(),
                title: title.into(),
                output,
                workspace,
                mode: WindowMode::Tiled,
                mapped: true,
                focused: false,
                rect: Rect::default(),
                target_rect: Rect::default(),
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
            if window.rect == Rect::default() {
                window.rect = target;
            }
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
}
