### Story e03s03: Wire dwindle into arrange_output: Implementation Steps

**type:** refactor  
**risk:** P2  
**context:** domain  

**Context:** `arrange_output` in `src/wm.rs` always uses master-stack tiling. This story modifies it to check the output's `tiling_layout` and call dwindle when set to Dwindle.

## Steps

1. Modify `arrange_output` to read `output_state.tiling_layout` and call dwindle when Dwindle, master-stack otherwise -> verify: `cargo test --bin eswm wm::tests`
2. Unit test: arrange_output with dwindle mode produces dwindle rects -> verify: `cargo test --bin eswm wm::tests`
3. Confirm master-stack tests still pass (`lays_out_master_and_stack`, `gap_insets_tiled_windows`) -> verify: `cargo test --bin eswm wm::tests`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all WM tests pass.
2. Confirm master-stack behavior unchanged for outputs with Master layout.

## Out of scope

- Per-output layout persistence across restarts
- Lua binding for layout selection (keybind only)

## Risks

- `arrange_output` is called every frame; dwindle must be fast (no allocations in hot path preferred).

---
