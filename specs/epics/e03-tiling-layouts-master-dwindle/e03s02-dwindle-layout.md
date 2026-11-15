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
