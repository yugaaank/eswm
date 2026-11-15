### Story e03s03: Wire equal into arrange_output: Implementation Steps

**type:** refactor  
**risk:** P2  
**context:** domain  

**Context:** `arrange_output` in `src/wm.rs` always uses master-stack tiling. This story modifies it to check the output's `tiling_layout` and call `equal_split` when set to `Equal`.

## Steps

1. Modify `arrange_output` to read `output_state.tiling_layout` and call `equal_split` when `Equal`, master-stack otherwise -> verify: `cargo test --bin eswm wm::tests`
2. Master layout: inner gap only between master column and stack column; no gaps between stack windows -> verify: `cargo test --bin eswm wm::tests`
3. Equal layout: inner gaps between all cells -> verify: `cargo test --bin eswm wm::tests`
4. Confirm master-stack tests still pass (`lays_out_master_and_stack`, `gap_insets_tiled_windows`) -> verify: `cargo test --bin eswm wm::tests`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all WM tests pass.
2. Confirm master-stack behavior unchanged for outputs with Master layout.
3. Confirm equal layout applies gaps between all cells.

## Out of scope

- Per-output layout persistence across restarts
- Lua binding for layout selection (keybind only)

## Risks

- `arrange_output` is called every frame; equal_split must be fast (no allocations in hot path preferred).
- Equal split cells may not perfectly fill the rect due to integer division (leftover pixels on right/bottom edges).

---
