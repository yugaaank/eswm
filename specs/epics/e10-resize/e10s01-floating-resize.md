# e10s01 — Floating resize: 8 handles, constraints, input semantics

**type:** feat
**risk:** P1
**context:** infra
**delta:** MODIFIED

## Context

Floating resize today is one gesture: `Super+RMB` starts `DragMode::Resize`, and
`drag_update` (`src/udev.rs:1550-1583`) only grows width/height — effectively a
bottom-right handle with a hardcoded 200×150 floor. This story replaces that
with eight directional handles and explicit constraints. Tiled boundary resize
is e10s02.

## Zoom-out (existing modules)

- **wm.rs** — purpose: authoritative WM state and geometry. Callers: main.rs,
  udev.rs, control.rs, ipc.rs, lua_state.rs, tests. Contracts: `arrange_output`
  owns tiled `target_rect`; `set_window_rect` refuses tiled windows; `dirty`
  gates arrangement; `Rect` is i32 origin / u32 size.
- **udev.rs** — purpose: DRM/libseat pointer/keyboard input and render. Callers:
  main.rs event loop, inline tests. Contracts: `DragState` is per-pointer;
  press starts, motion advances, matching release clears; every geometry
  mutation is followed by `apply_layout`.
- **main.rs** — purpose: xdg shell handlers, configure state, render list.
  Contracts: `apply_layout` sends target size + xdg states; `placed_windows`
  skips minimized and zero-size windows.

## Requirements

#### ADDED: Resize from all four edges and four corners
A floating, normal-presentation window can be resized by pressing on any of
its eight frame edges/corners and dragging. A 6px invisible padding outside the
frame is included in the hit area.

#### MODIFIED: Resize gesture is directional
**Before:** `Super+RMB` grows width and height only (bottom-right).
**After:** the pressed handle determines which edges move; opposite edges stay
fixed. `Super+RMB` remains as a bottom-right shortcut for compatibility.

#### MODIFIED: Size constraints are explicit
**Before:** 200×150 is duplicated as a literal in `drag_update`.
**After:** `MIN_WINDOW_WIDTH` / `MIN_WINDOW_HEIGHT` live in wm.rs, are used by
all resize paths, are clamped to the work area, and can be set from Lua via
`es.dsp.set_min_window_size(w, h)`.

#### ADDED: Escape cancels an in-flight drag
Pressing Escape clears `udev.drag` without applying a final geometry change.

#### ADDED: Drag threshold
A move/resize gesture only applies after 3px of pointer travel; a click on an
edge without travel leaves geometry untouched.

## Reason for depth

- `ResizeEdge` (8 variants): the same direction must drive hit-testing, rect
  math, cursor selection and tests; one enum stops those four from drifting.
- `resize_rect()` pure helper: edge-aware rect math with min/max clamps is
  easy to get wrong on left/top edges (origin moves while size shrinks); it
  must be testable without a DRM session.

## Steps

1. Add `ResizeEdge { North, South, East, West, NorthEast, NorthWest, SouthEast, SouthWest }` and pure `resize_edge_at(rect, point, padding) -> Option<ResizeEdge>` with unit tests for all edges, corners, padding, and free titlebar area → verify: `cargo test --bin eswm resize_edge_at`
2. Add pure `resize_rect(orig, edge, dx, dy, min, max) -> Rect`; test left/top shrink, right/bottom grow, min clamp, max clamp, u32 underflow guards → verify: `cargo test --bin eswm resize_rect`
3. Extend `DragState` with `edge: ResizeEdge` and replace the `DragMode::Resize` arm in `drag_update` with `resize_rect`; keep `Super+RMB` as `SouthEast` → verify: `cargo test --bin eswm resize_rect`
4. Reorder press handling: titlebar buttons → resize edges → titlebar drag → dock → click-to-focus; restrict plain edge resize to floating normal windows → verify: `cargo build`
5. Set `CursorImageStatus` per edge (ew/ns/nesw/nwse cursors); add fallback for named-cursor failure → verify: `cargo test --bin eswm resize_edge`
6. Add 3px drag threshold in `drag_update` and Escape-to-cancel in the key path; test both → verify: `cargo test --bin eswm drag_threshold`
7. Move 200/150 into `wm::MIN_WINDOW_WIDTH/HEIGHT`, add `es.dsp.set_min_window_size`, validate non-zero, document in `config.example.lua` → verify: `cargo test --bin eswm min_window_size`
8. Full preflight and write live-DRM UAT checklist (operator-run from a TTY) → verify: `cargo clippy --all-targets && cargo test && cargo build`

## Verification script (operator)

1. Start eswm on a TTY. Open two floating windows.
2. Drag each edge and each corner; the opposite edge must stay put.
3. Shrink below the configured minimum; size stops at the minimum.
4. Grow past the work area; size stops at the work area.
5. Press on an edge, release without moving; geometry unchanged.
6. Start a resize, press Escape; geometry unchanged.
7. Confirm the cursor changes to the matching resize icon near each handle.
8. Confirm `Super+RMB` still resizes from the bottom-right.

## Out of scope

- Tiled window boundaries (e10s02)
- Snap layouts, magnetic edges, shared-edge resize as a separate feature
- Cross-monitor drag, touch/pinch resize
- Client-supplied min/max size (xdg-shell has no such request; constraints are compositor-side)
- Persistence of geometry across sessions
- Animated resize feedback

## Risks

- Edge hit zones can steal clicks from titlebar buttons: fixed by press-order test in step 4.
- Left/top resize can underflow u32 geometry: fixed by pure `resize_rect` tests in step 2.
- `set_window_rect` forces a size configure via the `rect.width == 0` sentinel; verify it still fires for every handle.
- Cursor names vary by theme: keep existing named-cursor fallback.
