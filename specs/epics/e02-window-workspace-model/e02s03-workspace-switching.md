### Story e02s03: Workspace switching and window moving: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `switch_workspace()` in `src/udev.rs` calls `ensure_workspace` + `focus_workspace` + `apply_layout`. IPC `workspace.focus` command exists in `src/control.rs`. DRM keybind path works; winit path does not process keybinds. Focus uses `focus_history` (MRU) per workspace.

## Steps

1. Update `focus_workspace` to restore focus from `focus_history` MRU (front of list) -> verify: `cargo test --bin eswm wm::tests`
2. Update `focus_workspace_relative` to cycle through workspace IDs per output -> verify: `cargo test --bin eswm wm::tests`
3. Update `move_window_to_workspace` to update both `workspace` and `output` together; clamp float geometry to new output -> verify: `cargo test --bin eswm wm::tests`
4. Add integration test: spawn nested compositor, send IPC `workspace.focus` with new id, assert workspace list shows it active -> verify: `cargo test --test control`
5. Add integration test: open two windows, send IPC `window.move` to workspace 2, assert window appears on workspace 2 and source workspace is empty -> verify: `cargo test --test control`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all workspace tests pass.
2. Run `cargo test --test control` and confirm IPC workspace tests pass.

## Out of scope

- Workspace switching animations (deferred to Phase 4)
- Following the moved window to new workspace automatically (out of scope per SCOPE)

## Risks

- Moving a window between outputs must update both `workspace` and `output` fields and recompute geometry.

---
