### Story e02s01: Window state model with independent axes: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `wm::Window` currently mixes layout (tiled/floating) with state (maximized/fullscreen) in `WindowMode`. The review model splits this into independent axes: `layout` (Tiling/Floating) on workspace, `presentation` (Normal/Maximized/Fullscreen), `visibility` (Visible/Minimized), `lifecycle` (Open/CloseRequested/Closing) on window, plus `placement_override`, `float_geometry`, `pending_geometry`, `committed_geometry`, `min_size`/`max_size`, `tiling_slot`, `sticky`, `always_on_top`, `decorated`, `modal`, `urgent`, `opacity`.

## Steps

1. Define `LayoutMode` (Tiling/Floating), `Presentation` (Normal/Maximized/Fullscreen), `Visibility` (Visible/Minimized), `Lifecycle` (Open/CloseRequested/Closing), `Placement` (Tiled/Floating) enums in `src/wm.rs` -> verify: `cargo build`
2. Replace `Window.mode: WindowMode` with separate axis fields; add `placement_override`, `float_geometry`, `pending_geometry`, `committed_geometry`, `min_size`, `max_size`, `tiling_slot`, `sticky`, `always_on_top`, `decorated`, `modal`, `urgent`, `opacity` -> verify: `cargo build`
3. Add `effective_placement(window) -> Placement`, `is_visible(window, workspace) -> bool`, `effective_rect(window) -> Rect` functions -> verify: `cargo test --bin eswm wm::tests`
4. Unit test: effective_placement returns override > dialog/transient/modal > workspace mode -> verify: `cargo test --bin eswm wm::tests`
5. Unit test: is_visible returns false for minimized or inactive workspace (non-sticky) -> verify: `cargo test --bin eswm wm::tests`
6. Unit test: effective_rect returns float geometry for floating, layout rect for tiled, usable area for maximized, full geometry for fullscreen -> verify: `cargo test --bin eswm wm::tests`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all window-state tests pass.
2. Confirm `Window` has separate layout/presentation/visibility/lifecycle fields.
3. Confirm `effective_placement`, `is_visible`, `effective_rect` functions exist and behave correctly.

## Out of scope

- New `Window` fields not in SCOPE_LATEST.yaml
- Rendering or border drawing changes

## Risks

- All existing code that pattern-matches on `WindowMode` must be updated to the new axes.

---
