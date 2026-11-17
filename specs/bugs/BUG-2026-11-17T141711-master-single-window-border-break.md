# BUG-2026-11-17T141711: master-mode single-window border breaks at bottom

## Problem

In master tiling layout with a single tiled window in a workspace:

- The left and right window borders stop short of the bottom border, leaving a visible break/gap at the bottom corners ("border breaking on left and right side in the down area").
- The bottom of the window appears to run out past where the side borders end.

With multiple windows the same gap exists per window but is less noticeable because each window is shorter; with one full-height master window the discontinuity in a long vertical border is obvious.

## Root Cause Analysis

The border frame is drawn around each window's `target_rect` in udev.rs. The frame top starts at `rect.y - border_size - titlebar_height` (extends above the client surface to capture borders/titlebar), and the frame bottom is at `rect.y + rect.height + border_size` (extends below by one border width).

The top edge spans the full frame width, so it meets the side borders at the top corners with no gap.

The bottom edge starts at `rect.y + rect.height`. The left and right edges are built with height `side_h = rect.height` (target_rect height only), so they span from frame top down to `rect.y - border_size - titlebar_height + rect.height`, which is `border_size + titlebar_height` pixels above where the bottom edge begins.

Result: side borders stop `border_size + titlebar_height` pixels above the bottom border, leaving a vertical gap at each bottom corner. Default `border_size = 2` -> a 2px break on a full-height window.

This is a rendering-contract defect in udev.rs border element geometry, not a WM geometry bug: `target_rect` itself is correctly inset by `border_size` on all sides.

Security impact: LOW. No data exposure, privilege escalation, or exploit path; purely visual border discontinuity.

Risk level: Low. One-line render-math change, confined to border element sizing.

## TDD Fix Plan

1. **RED**: Extract the border edge position math into a pure, testable helper (e.g. `border_edges(rect, border_size, titlebar_height) -> (top, bottom, left, right)`) and add a unit test asserting the left and right edges span the full frame height from frame top to frame bottom — i.e. `left.height == right.height == frame_height` and `left.bottom == right.bottom == bottom_edge.bottom` (no gap). Run `cargo test` -> test fails (helper missing / gap present).

   **GREEN**: Implement `border_edges` with `side_h = rect.height + 2*border_size + titlebar_height` (full frame height) so side borders connect to top and bottom borders. Run `cargo test` -> passes.

   **verify**: `cargo test`

2. **RED**: Add a regression test in udev.rs (or wm.rs if helper lives there) covering the single-window master case: border side edges meet the bottom edge with zero gap, for both `titlebar_height = 0` and `titlebar_height > 0`.

   **GREEN**: The fixed helper already satisfies this; test passes.

   **verify**: `cargo test`

**REFACTOR**: Replace the inline border edge construction in udev.rs with calls to the shared helper so top/bottom/left/right edge math has a single source of truth. Keep `cargo test` and `cargo clippy` green.

## Acceptance Criteria

- [ ] Left and right border edges span full frame height (connect to top and bottom borders, no gap).
- [ ] Gap between side-border bottom and bottom-border top is zero for `titlebar_height = 0` and `titlebar_height > 0`.
- [ ] All new tests pass.
- [ ] Existing 72 tests still pass.
- [ ] `cargo clippy` clean (no new warnings).

## Resolution

<!-- filled in by validate-fix -->
