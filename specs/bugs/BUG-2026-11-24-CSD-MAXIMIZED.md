---
bug_id: BUG-2026-11-24-CSD-MAXIMIZED
status: fixed-on-branch
severity: medium
scope: wayland
title: Tiled CSD clients leave a gap inside the frame because Maximized is not advertised
---

# BUG-2026-11-24-CSD-MAXIMIZED — Tiled CSD clients leave a gap inside the frame

**Status:** fixed on main (4f3c1a7); live DRM UAT passed by operator
**Severity:** medium
**Scope:** wayland / CSD
**Security impact:** NONE — xdg state bits only tell clients how they are placed.
**Reported by:** operator: Helium leaves space inside borders in tiling mode.

## Problem

Tiling Helium (a Chromium/Electron-style client) leaves a visible gap between
eswm's frame and the client's content. Live `eswm.log` (session
2026-11-24T14:15Z, toplevel id=1, Helium confirmed as sole app) shows:

- configured frame `1896x1078` at `(12,80)`;
- buffer `1916x1098` committed (20px larger, i.e. 10px CSD shadow on each side);
- `xdg_offset` reported `(10,10)`;
- fill_scale `~0.99` applied to the whole buffer, rendering at `(2,70)`.

Because the 10px shadow is part of the scaled surface, the content stops short
of the frame. Hyprland does not show this gap: it forces CSD off by advertising
Maximized on toplevels.

## Root Cause Analysis

### Reproduce

On a TTY, start eswm with `RUST_LOG=debug`, open Helium alone, tile it, and
inspect `~/.local/state/eswm/eswm.log`. Observe `offset=(10,10)` in `render
window` lines and `committed 1916x1098 > configured 1896x1078`.

### Isolate

The xdg configure state helper derives its bits from `Presentation` only:
tiled normal windows received Activated + four Tiled bits, but never Maximized.
The four Tiled bits make winit-style clients obey configures (BUG-2026-11-24-TILED-STATE),
but CSD-drawing toolkits still render their own shadow because nothing tells
them the window is managed edge-to-edge.

### Hypothesize

- **H1 (chosen):** missing Maximized bit lets CSD clients draw a shadow inset.
  Falsification: advertise Maximized for tiled windows and verify the CSD
  offset disappears on the next live run.
- H2: compositor stretch causes the gap. Rejected — stretch applies to the
  whole buffer, but the overlap originates from the client-reported geometry
  offset `(10,10)`.
- H3: size-negotiation resend loop causes it. Rejected — that loop is the
  known bounded 3-resend behavior, unrelated to the geometry inset.

### Verify

Source comparison with local Hyprland checkout `v0.57.0-87-g0767f6347`:

- `src/protocols/XDGShell.cpp:180-184` pushes all four tiled states for v2+.
- `src/desktop/view/window/WaylandBackend.cpp:335-346` deliberately makes
  `setMaximized()` a no-op because the XDG shell layer pins Maximized on every
  toplevel: "Hyprland always sets Maximized for apps to force them to not draw CSD."
- `src/protocols/XDGShell.cpp` commit handler calls `setMaximized(true)` at map
  time with the comment "this forces apps to not draw CSD."

eswm already sends the four tiled states; it was missing the Maximized bit for
tiled normal windows. Falsification test is the RED unit assertion on
`toplevel_configure_state(tiled, true)` expecting `maximized: true`.

## Fix

Set `maximized` for tiled normal windows in the pure configure-state helper:
`presentation_maximized || sends_tiled_state()`. Floating stays stateless so
user resizing still works; genuine maximized/fullscreen windows keep their
existing exclusive bits. The state helper remains the single source of truth.

## Acceptance Criteria

- [x] Tiled normal windows advertise Maximized + all four Tiled bits.
- [x] Floating windows remain stateless (no Maximized/Tiled).
- [x] Maximized/fullscreen windows keep exclusive existing bits.
- [x] New unit test RED → GREEN.
- [ ] Full preflight green.
- [x] Live DRM UAT: operator confirmed "yes working" — Helium fills the frame
      with no inset gap (2026-11-24).

## Resolution

Fixed on main as `4f3c1a7`. RED/GREEN test in place; clippy clean, 219 tests,
build clean. Operator confirmed the Helium border gap is gone on the live DRM
session. GTK decoration regression check was not separately exercised.
