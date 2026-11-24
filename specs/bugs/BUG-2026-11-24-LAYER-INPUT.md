---
bug_id: BUG-2026-11-24-LAYER-INPUT
status: fixed
severity: high
scope: input
title: Layer-shell surfaces render but never receive pointer clicks or scroll
date: "2026-11-24"
priority: high
reported_by: operator_observation
risk_level: low
security_impact: none
files:
  - src/udev.rs
---

# BUG-2026-11-24-LAYER-INPUT: Layer-shell surfaces render but never receive pointer clicks or scroll

## Problem

Layer-shell surfaces (panels, bars, launchers — anything created through
`wlr_layer_shell_v1`) paint correctly on the DRM session but mouse interaction
does not reach them: clicks do nothing and the scroll wheel has no effect.
Toplevel windows, popups and the compositor dock all receive pointer input
normally.

Expected: a mapped layer surface is hit-tested for pointer input in its z-order
band (overlay/top above windows, bottom/background below them), receives
`wl_pointer.enter/motion/button/axis`, and respects its own input region.

Reproduce: run a layer client (bar or launcher) on eswm, move the pointer over
it, click a button and scroll. No client-side reaction occurs. Keyboard focus
on an interactive layer works (e06s03), which isolates the defect to the
pointer path.

Security impact: NONE. Input is never delivered; no trust or data path is
exposed. No exploit path identified.

## History check

Prior `input`-scope bugs are BUG-2026-11-24-SUPER-DRAG-LEAK (compositor drags
leaking button/motion to clients) and BUG-2026-11-24-DOCK-RESTORE (dock click
restore). Both fixed in the *inverted* direction — eswm sending too much input
to toplevels. This is novel: eswm sends no pointer input to layers at all.
The e06 input-focus epic implemented layer **keyboard** focus and layer
**rendering** but never the pointer counterpart.

## Root Cause Analysis (4-phase)

### Phase 1 — Reproduce
Source-level reproduction (the DRM backend cannot be started inside an
existing Wayland/X11 session per project rule): every pointer path funnels
through one focus function. A layer client receives no `wl_pointer.enter`, so
subsequent button and axis events have no target surface. Operator-reported
symptom matches exactly: visible layer, dead pointer.

### Phase 2 — Isolate
`pointer_focus()` is the single focus source for motion (`PointerMotion`,
`PointerMotionAbsolute`), button presses (which call `pointer.motion` to
refresh focus first), and therefore axis events (which deliver to whatever
focus the seat already holds). Its focus search is, in order: active popup
grab → non-grabbed popups → `wm.surface_under()` → `toplevel_for_window()`.

`wm.surface_under()` iterates only tiled/floating windows of the active
workspace; `toplevel_for_window()` searches only `xdg_shell_state` toplevels.
Neither consults the `LayerMap` or `layer_wrappers`. A layer surface can never
be returned as pointer focus.

Contributing factors:
- The render path *does* map layers into the `LayerMap` and draws them by
  `layer_geometry()`, so layers are visible — the missing input path is masked
  by correct rendering.
- `KeyboardInteractivity` handling in the commit handler and
  `update_keyboard_focus()` covers keyboard only; no equivalent exists for
  pointer.
- The `PointerAxis` handler forwards to the seat without refreshing focus, so
  even a layer that had focus cannot recover it if the layout changes under a
  stationary pointer.

### Phase 3 — Hypotheses (ranked)
1. **H1 — no layer hit-testing in `pointer_focus()`.** Falsification: if a
   hidden layer-input path existed, adding an explicit layer branch would not
   change which surface receives events. Source inspection found no such path.
2. **H2 — layer input regions reject every point.** Falsification: smithay's
   `LayerSurface::surface_under()` respects input regions and defaults to the
   whole surface when none is set; no eswm code clears layer input regions.
   Also, no point can be rejected by a path that is never called.
3. **H3 — layer clients do not commit buffers.** Refuted: layers render, which
   requires a committed buffer.
4. **H4 — the seat lacks a pointer device.** Refuted: toplevel clicks and
   scrolls work through the same seat.

### Phase 4 — Verify
H1 confirmed. The focus search has exactly four branches, none layer-aware;
the LayerMap is consulted only in rendering and layer mapping. This is a
missing capability (design gap), not a regression — the risk is Low (no
placement, security or data-loss surface; confined to input routing).

Root cause (one sentence): **pointer focus is computed exclusively from
toplevel windows, so layer-shell surfaces are painted but never become the
pointer target; button and axis events consequently never reach them.**

## Fix approach (minimal)

Add layer surfaces as first-class pointer-focus candidates in the correct
z-order band, without changing toplevel, popup or dock behaviour:

1. Extract a pure, testable geometry helper that, given a topmost-first list
   of layer screen rects and a global point, returns the topmost containing
   layer and its surface-local point. This encodes band priority and rect
   math; smithay's `LayerSurface::surface_under()` remains the authority for
   input regions and subsurfaces.
2. In `pointer_focus()`, hit-test overlay/top layers before toplevel windows
   and bottom/background layers after them. Build candidates from the
   primary output's `LayerMap` (same mapping the renderer uses).
3. Refresh pointer focus in the `PointerAxis` handler before forwarding the
   axis frame, so scroll follows the surface actually under the pointer.
4. Keep popup grab, popup and drag precedence unchanged. Do not gate pointer
   input on `KeyboardInteractivity` — a bar with `None` interactivity still
   accepts clicks and scroll.

## TDD Fix Plan

1. **RED**: add a test that a point inside a topmost layer rect selects that
   layer with correct surface-local coordinates, and that an overlapping
   lower rect does not win.
   **GREEN**: add the pure `layer_at()` helper.
   **verify**: `cargo test --bin eswm layer_at`

2. **RED**: add a test that a point outside every layer rect returns no hit,
   and that rect edge bounds are half-open (right/bottom edges excluded).
   **GREEN**: implement containment with half-open bounds in `layer_at()`.
   **verify**: `cargo test --bin eswm layer_at`

3. **RED**: add a test that layer candidates are ordered by band
   (overlay/top above windows, bottom/background below) and, within a band,
   topmost-first — asserting the composed priority list, not internal state.
   **GREEN**: add the pure band-ordering helper and wire it into focus
   resolution.
   **verify**: `cargo test --bin eswm layer_band`

4. **GREEN (integration, source-level)**: route `pointer_focus()` through the
   new helpers and refresh focus in `PointerAxis`. Behaviour is proven by the
   pure tests plus full preflight; live confirmation is operator UAT.
   **verify**: `cargo clippy --all-targets && cargo test && cargo build`

**REFACTOR**: keep the popup/drag precedence in one place so layer addition
cannot silently outrank a modal grab.

## Acceptance Criteria

- [ ] Pure layer rect hit-test returns topmost layer with surface-local coords.
      Deferred: the implementation delegates rect math and input regions to
      smithay's `LayerSurface::surface_under()` and has no separate pure
      `layer_at()` helper/test. A layer-backed test needs a live Wayland client.
- [x] Overlay/top layers outrank toplevel windows; bottom/background do not.
- [x] Layer input regions are respected (delegated to smithay surface_under).
- [x] Popup grab, popup, drag and dock paths keep their existing precedence.
- [x] Scroll refreshes focus before forwarding the axis frame.
- [x] All new tests pass; all existing tests pass; clippy and build clean.
- [ ] Operator live-DRM UAT confirms click + scroll on a real layer client.

## Resolution

Fixed locally (2026-11-24), awaiting live DRM UAT.

Root cause: `pointer_focus()` computed focus exclusively from `wm::surface_under`
(toplevel windows), so layer-shell surfaces were painted from the `LayerMap`
but never became the pointer target. Button and axis events consequently had no
layer surface to travel to.

What changed in `src/udev.rs`:
- `layer_surface_under()` hit-tests the primary output's `LayerMap` per band,
  newest-first, and returns the concrete surface plus its screen origin.
- `layer_root_for_surface()` resolves the owning layer for popups/subsurfaces.
- `pointer_focus()` now searches Overlay/Top layers, then windows, then
  Bottom/Background layers; popup grab, popup and drag precedence are unchanged.
- A press on an Overlay/Top layer no longer runs window drag/titlebar/dock or
  click-to-focus logic on the window behind it; interactive (`OnDemand`)
  layers take keyboard focus on click.
- `update_resize_cursor()` keeps the default cursor over Overlay/Top layers.
- `PointerAxis` refreshes focus first, so scroll follows the layer that appeared
  under a stationary pointer.

Verify: `cargo clippy --all-targets && cargo test && cargo build` — all green
(219 tests; log at `/tmp/opencode/layer-input-preflight.log`). Live DRM UAT
remains pending operator.
