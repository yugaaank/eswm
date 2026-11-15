### Story e03s02: Equal tiling layout algorithm: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `arrange_output` in `src/wm.rs` implements master-stack (first window gets 3/5 width, rest split remaining height). No equal layout exists. This story adds `equal_split()` a grid-based layout where every window gets the same width and height, arranged in rows and columns with inner gaps between cells.

## Steps

1. Add `equal_split` function to `src/wm.rs` taking window ids, a rect, a gap, and a titlebar_height, returning equal-sized rects in a grid -> verify: `cargo build`
2. Unit test: equal_split with 1 window fills the entire rect -> verify: `cargo test --bin eswm equal_split_layout`
3. Unit test: equal_split with N windows produces equal-sized non-overlapping rects with gaps -> verify: `cargo test --bin eswm equal_split_layout`
4. Unit test: equal_split gives every window identical width and height -> verify: `cargo test --bin eswm equal_split_layout`
5. Unit test: equal_split with titlebar_height reserves vertical space for titlebars between rows -> verify: `cargo test --bin eswm equal_split_layout`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm equal_split_layout` and confirm all equal_split tests pass.
2. Confirm every window gets the same width and height regardless of count.
3. Confirm titlebar gaps prevent titlebar overlap between vertically stacked cells.

## Out of scope

- Wiring equal_split into arrange_output (e03s03)
- Layout toggle keybind (e03s01)

## Risks

- Equal split cells may not perfectly fill the rect due to integer division (leftover pixels on right/bottom edges). Acceptable for tiling WM.
- Grid dimensions minimize empty cells; some cells may be empty when window count isn't a perfect grid number.

---
