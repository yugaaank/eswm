### Story e03s01: Per-output layout mode + toggle keybind: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `wm::LayoutMode` exists with `Hyprland`/`Windows` variants but controls spawn default (tiled vs floating), not the tiling algorithm. `WindowManager` has a global `layout_mode` field toggled by `toggle_layout_mode` keybind. `Output` struct has no per-output layout field. `arrange_output` always uses master-stack. This story adds a per-output `TilingLayout` enum (Master, Dwindle) on `Output` and makes the layout toggle keybind cycle the focused output's layout.

## Steps

1. Add `TilingLayout` enum (`Master`, `Dwindle`) to `src/wm.rs` -> verify: `cargo build`
2. Add `tiling_layout: TilingLayout` field to `Output` struct, default `Master`, set in `add_output` -> verify: `cargo build`
3. Change `WindowManager::toggle_layout_mode` to cycle the focused output's `tiling_layout` between Master and Dwindle, return the new mode -> verify: `cargo test --bin eswm wm::tests`
4. Update udev.rs `toggle_layout_mode` keybind to call per-output toggle on primary output -> verify: `cargo test --bin eswm wm::tests`
5. Unit test: toggle_layout_mode switches output between Master and Dwindle -> verify: `cargo test --bin eswm layout_toggle`
6. Write default `config.default.lua` with `Super+`` = toggle_layout_mode → verify: `test -f config.default.lua`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all WM tests pass.
2. Run `cargo test --bin eswm layout_toggle` and confirm the toggle test passes.
3. Confirm `config.default.lua` exists.

## Out of scope

- Dwindle layout algorithm (e03s02)
- Floating mode as a layout option
- Lua `es.dsp.set_layout_mode` changes (stays as spawn default)

## Risks

- `toggle_layout_mode` is called from udev.rs keybind; changing it from global to per-output could affect existing configs. The keybind name stays the same; only behavior changes to per-output.

---

### Story e03s02: Dwindle tiling layout algorithm: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `arrange_output` in `src/wm.rs` implements master-stack (first window gets 3/5 width, rest split remaining height). No dwindle layout exists. This story adds a recursive binary split dwindle algorithm.

## Steps

1. Add `dwindle` function to `src/wm.rs` taking a vec of window ids and a rect, returning assigned rects via recursive binary split (split largest leaf, alternate horizontal/vertical) -> verify: `cargo build`
2. Unit test: dwindle with 1 window fills the entire rect -> verify: `cargo test --bin eswm dwindle_layout`
3. Unit test: dwindle with N windows produces non-overlapping rects covering the rect -> verify: `cargo test --bin eswm dwindle_layout`
4. Unit test: dwindle splits largest leaf first -> verify: `cargo test --bin eswm dwindle_layout`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm dwindle_layout` and confirm all dwindle tests pass.
2. Confirm no two window rects overlap and union equals the input rect.

## Out of scope

- Wiring dwindle into arrange_output (e03s03)
- Layout toggle keybind (e03s01)

## Risks

- Dwindle rects must respect gaps/borders like master-stack does; test with outer=0, inner=0 for simplicity.

---

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

1. Run `cargo test --bin eswm wm::tests` and confirm all 20+ WM tests pass.
2. Confirm master-stack behavior unchanged for outputs with Master layout.

## Out of scope

- Per-output layout persistence across restarts
- Lua binding for layout selection (keybind only)

## Risks

- `arrange_output` is called every frame; dwindle must be fast (no allocations in hot path preferred).

---
