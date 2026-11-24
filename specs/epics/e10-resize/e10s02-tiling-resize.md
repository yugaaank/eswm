# e10s02 — Tiling resize: split ratios for Master and Equal layouts

**type:** feat
**risk:** P1
**context:** infra
**delta:** ADDED

## Context

`arrange_output` (`src/wm.rs:1430-1568`) recomputes every tiled `target_rect`
from the output work area: Master uses a fixed 3/5 master width, Equal uses
equal grid cells via `equal_split`. `set_window_rect` deliberately refuses
tiled windows, so a one-off rect override would be erased on the next
arrangement. Resizing therefore has to change the *inputs* to the layout — the
split ratios — not the output rects.

## Zoom-out (existing modules)

- **wm.rs** — purpose: layout authority. Callers: main.rs `apply_layout`,
  udev.rs input, ipc/control snapshots, tests. Contracts: `arrange_output` only
  runs when `dirty`; `set_target_rect` updates geometry without clearing
  `rect`; `equal_split` is a pure helper used by both layout code and tests.
- **udev.rs** — purpose: pointer gestures. Contract: a drag mutates WM state
  then calls `apply_layout`.
- **main.rs** — purpose: configures clients from `target_rect`.

## Requirements

#### ADDED: Master split is adjustable
Each output stores a `master_ratio` (default 0.6 — byte-identical to today's
3/5). Dragging the master/stack boundary changes the ratio, clamped so both
sides keep at least 15% of the inner width.

#### ADDED: Master stack heights are adjustable
Each output stores normalized weights for the stack column. Dragging a
horizontal boundary between two stack windows changes their relative weights.

#### ADDED: Equal grid columns and rows are adjustable
Each output stores normalized column and row weights. Dragging an internal
grid boundary changes the adjacent weights; the default remains equal cells.

#### ADDED: Tiled boundary drag
Pressing within 6px of an internal boundary between two visible tiled windows
starts a tiling resize drag. Dragging splits changes weights, not window rects,
so re-arrangement (new window, workspace switch, output resize) preserves them.

#### ADDED: Reset splits
A `reset_tiling_splits` action and `es.dsp.reset_tiling_splits()` restore
default ratios for the primary output.

## Reason for depth

- `TilingSplits` on `Output`: ratios must survive arbitrary re-arrangements;
  per-window overrides cannot because `arrange_output` owns tiled geometry.
- `weighted_split()` pure helper: Master stack, Equal columns and Equal rows
  all do the same "normalize weights, allocate pixels, clamp" math; one helper
  prevents three subtly different copies.
- `TilingSplitEdge`: hit-testing, delta application and cursor choice must
  agree on which boundary is being dragged.

## Steps

1. Add `TilingSplits { master_ratio, stack_weights, col_weights, row_weights }` to `Output` with defaults and pure normalize/clamp helpers; test defaults and clamping → verify: `cargo test --bin eswm tiling_splits`
2. Add pure `weighted_split(total, gap, weights, min_share) -> Vec<u32>`; test equal, skewed, clamped and degenerate inputs → verify: `cargo test --bin eswm weighted_split`
3. Wire Master layout to `master_ratio` and assert default output matches the current 3/5 rects → verify: `cargo test --bin eswm master_ratio`
4. Wire Master stack heights to `stack_weights` with titlebar-aware gaps; test two- and three-window stacks → verify: `cargo test --bin eswm stack_weights`
5. Refactor `equal_split` to accept optional column/row weights, preserving the exact default grid; test skewed weights → verify: `cargo test --bin eswm equal_split`
6. Add pure `tiling_split_edge_at(rects, point, padding) -> Option<TilingSplitEdge>` using the rects produced by the current arrangement → verify: `cargo test --bin eswm tiling_split_edge`
7. Add `DragMode::TilingSplit` and `WindowManager::adjust_split(output, edge, delta_px)` with clamping; call `apply_layout` on motion → verify: `cargo test --bin eswm adjust_split`
8. Add per-edge cursor feedback, `reset_tiling_splits` in Lua and control actions, and `config.example.lua` docs → verify: `cargo test --bin eswm reset_tiling_splits`
9. Full preflight and live-DRM UAT checklist → verify: `cargo clippy --all-targets && cargo test && cargo build`

## Verification script (operator)

1. Start eswm in tiling mode with three tiled windows.
2. Drag the master/stack boundary; both sides stay at least 15% wide.
3. Drag a boundary between two stack windows; their heights change, widths do not.
4. Switch to Equal layout; drag an internal column boundary then a row boundary.
5. Open a fourth window; earlier ratios are preserved.
6. Switch workspaces and back; ratios are preserved for the session.
7. Invoke `reset_tiling_splits`; layout returns to default.
8. Resize the output; ratios still produce valid, non-overlapping cells.

## Out of scope

- Persisting split ratios across sessions (separate MVP persistence item)
- Per-window size overrides (invalidated by `arrange_output`)
- Snap layouts, magnetic edges, shared-edge resize as a separate feature
- Touch gestures, animated split feedback
- Multi-monitor split synchronization (splits are per output)

## Risks

- Adding/removing windows changes weight-vector length: normalize on every arrangement and clamp stale weights.
- Titlebar height is part of the vertical gap; forgetting it overlaps lower windows (existing `gap_v` logic).
- A split dragged to an extreme can make a window unusably small: 15% floor and test.
- Tiled split hit zones can compete with click-to-focus: only start a split drag when the pointer is within 6px of an internal boundary, never the outer frame.
