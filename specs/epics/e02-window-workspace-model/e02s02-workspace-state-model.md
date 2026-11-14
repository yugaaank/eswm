### Story e02s02: Workspace state model: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `wm::Workspace` already exists in `src/wm.rs` with id, name, output, windows, focused. `WindowManager::add_workspace`, `focus_workspace`, `ensure_workspace` exist with tests. This story audits the implementation and adds coverage for empty/persistent workspace behavior.

## Steps

1. Audit `src/wm.rs` `Workspace` struct and `add_workspace`/`focus_workspace`/`ensure_workspace` against SCOPE_LATEST.yaml -> verify: `cargo test --lib wm::tests`
2. Add unit test for workspace empty/persistent behavior (workspace with no windows remains) -> verify: `cargo test --lib wm::tests`
3. Add unit test for `focus_workspace` on non-owned output returns false -> verify: `cargo test --lib wm::tests`

## Verification Script (Step-by-Step)

1. Run `cargo test --lib wm::tests` and confirm all workspace tests pass.
2. Confirm `Workspace` has id, name, output, windows, focused fields in `src/wm.rs`.

## Out of scope

- Multi-output active workspace sync beyond `active_workspace` per output (exists)
- Workspace creation from IPC (already via `workspace.focus` command)

## Risks

- `ensure_workspace` returns false when workspace exists on a different output: documented behavior; tests assert same-output only.

---
