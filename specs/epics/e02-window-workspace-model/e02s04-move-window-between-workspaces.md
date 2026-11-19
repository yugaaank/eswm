### Story e02s04: Move window between workspaces: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `WindowManager::move_window_to_workspace` exists in `src/wm.rs` with tests. IPC `window.move` command exists in `src/control.rs`. DRM keybind `move_window_to_workspace` exists in `src/udev.rs`. Winit path lacks keybind processing (covered by e02s03 step 1).

## Steps

1. Add integration test: spawn nested compositor, open two windows, send IPC `window.move` to workspace 2, assert window appears on workspace 2 and source workspace is empty -> verify: `cargo test --test control`
2. Add unit test for `move_window_to_workspace` when target workspace is on a different output (must fail) -> verify: `cargo test --bin eswm wm::tests`
3. Confirm `move_window_to_workspace` keybind documented in `config.default.lua` (`Super+Shift+1` ... `Super+Shift+9`) -> verify: `test -f config.default.lua`

## Verification Script (Step-by-Step)

1. Run `cargo test --test control` and confirm window-move integration tests pass.
2. Run `cargo test --bin eswm wm::tests` and confirm `workspace_move_keeps_windows_separate` passes.
3. Confirm `config.default.lua` contains `move_window_to_workspace` binds.

## Out of scope

- Following the moved window to the new workspace automatically (configurable follow behavior is out of scope per SCOPE)
- Animations during move (deferred to Phase 4)

## Risks

- Moving the focused window leaves output without focus until `focus_window` re-applies: existing code handles this; test asserts it.

---
