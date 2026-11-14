### Story e02s05: Tiler/floating integration with state model: Implementation Steps

**type:** refactor  
**risk:** P2  
**context:** domain  

**Context:** `arrange_output` in `src/wm.rs` already lays out only tiled, mapped windows on the active workspace and ignores floating windows. `placed_windows` in `src/main.rs` filters to the active output's active workspace. `cycle_focus` filters to active workspace windows. This story audits that integration and adds tests proving cross-workspace isolation (windows on inactive workspaces are not tiled, focused, or rendered).

## Steps

1. Audit `arrange_output` filters windows to `output_state.active_workspace` -> verify: `cargo test --lib wm::tests`
2. Add unit test: open windows on workspace 1 and 2, arrange workspace 1, assert workspace 2 windows have zero/unset target_rect -> verify: `cargo test --lib wm::tests`
3. Add unit test: floating window on workspace 2 is not included in workspace 1 tiling -> verify: `cargo test --lib wm::tests`
4. Add integration test: switch workspace via IPC, assert only new workspace's windows render in `placed_windows` -> verify: `cargo test --test control`

## Verification Script (Step-by-Step)

1. Run `cargo test --lib wm::tests` and confirm tiler/workspace isolation tests pass.
2. Run `cargo test --test control` and confirm placed-windows workspace isolation passes.

## Out of scope

- Master-stack ratio changes (out of scope; existing 3/5 split stays)
- Additional layouts (deferred to Phase 3)

## Risks

- `placed_windows` iterates all workspaces but filters by `active_workspace != workspace.id`: confirm this filter exists; if not, add it (risk: inactive-workspace windows render).

---
