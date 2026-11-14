### Story e02s03: Workspace switching: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `switch_workspace()` in `src/udev.rs` calls `ensure_workspace` + `focus_workspace` + `apply_layout`. IPC `workspace.focus` command exists in `src/control.rs`. DRM keybind path works; winit path (`run_winit` in `src/main.rs`) does not process keybinds. No default keybinds are shipped (decision Q1=B: default `config.lua` template instead).

## Steps

1. Add `process_shortcut` / `execute_keybind` call to `run_winit` so the dev backend handles workspace keybinds -> verify: `cargo test --lib wm::tests`
2. Add integration test: spawn nested compositor, send IPC `workspace.focus` with a new id, assert workspace list shows it active -> verify: `cargo test --test control`
3. Write default `config.lua` template with `es.bind("Super+1", "switch_workspace", 1)` ... `es.bind("Super+9", "switch_workspace", 9)` at `config.default.lua` (not in config path; user copies) -> verify: `test -f config.default.lua`

## Verification Script (Step-by-Step)

1. Run `cargo test --test control` and confirm IPC workspace tests pass.
2. Run `cargo test --lib wm::tests` and confirm winit-keybind-related WM tests pass.
3. Confirm `config.default.lua` exists and contains `switch_workspace` binds for workspaces 1–9.

## Out of scope

- Built-in keybinds compiled into the binary (decision Q1=B)
- Workspace switching animations (deferred to Phase 4)

## Risks

- `run_winit` currently forwards all keys to clients; adding keybind processing could swallow keys clients expect. Limit to keybinds that match configured Lua keybinds; unmatched keys still forward.

---
