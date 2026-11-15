### Story e03s02: Equal tiling layout algorithm: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** No equal tiling layout existed in `src/wm.rs`. Only master-stack was available via `arrange_output`. This story adds equal_split() a grid-based layout where every window gets the same width and height, arranged in rows and columns that minimize empty space and keep cells as square as possible.

## Steps

1. Add `equal_split` function to `src/wm.rs` -> verify: `cargo build`
2. Unit test: equal_split with 1 window fills the entire rect -> verify: `cargo test --bin eswm equal_split_layout`
3. Unit test: equal_split with N windows produces equal-sized non-overlapping rects -> verify: `cargo test --bin eswm equal_split_layout`
4. Unit test: equal_split gives every window identical width and height -> verify: `cargo test --bin eswm equal_split_layout`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm equal_split_layout` and confirm all equal_split tests pass.
2. Confirm every window gets the same width and height regardless of count.

## Out of scope

- Wiring equal_split into arrange_output (e03s03)
- Layout toggle keybind (e03s01)

## Risks

- Equal split cells may not perfectly fill the rect due to integer division (leftover pixels on right/bottom edges). Acceptable for tiling WM.

---
