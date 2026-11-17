---
bug_id: BUG-2026-11-17T00:30:00
status: open
severity: high
scope: udev-render-loop
title: Stutter/freeze on idle desktop; sustained 14-16% idle CPU after render-loop fix
---

# BUG-2026-11-17T00:30:00: Stutter/freeze when cursor is still, 14–16% idle CPU

## Problem

After the vblank-timer fix (BUG-2026-11-16T17:45:54), idle CPU dropped from 80–93% to **14–16%**, but the desktop still stutters/freezes when the pointer is stationary. Moving the pointer makes the screen feel smooth again.

Observation from the user (live DRM session):
- Cursor still → screen lagging/freezing, CPU ~14–16%
- Cursor moving → screen fine, CPU ~50%
- RSS ~200 MB steady (down from 291 MB), Hyprland on another VT idles at 0% / ~172 MB

Edit/typing and window content updates still suffer; the freeze is severe enough to call the compositor unusable.

## Reproduction

1. Boot to a TTY, no existing session.
2. Run `./target/debug/eswm` with debug logging redirected:
   `RUST_LOG=debug ./target/debug/eswm >/tmp/opencode/eswm-debug.log 2>&1`
3. Leave the pointer untouched for 20–30 s, then move it, then launch a client.
4. Quit and analyze the log.

## Evidence gathered so far

- Direct `/proc/<pid>/stat` sampling of the fixed binary (idle, bare desktop):
  `t=2s:0, t=4s:1, t=6s:0, t=8s:0, t=10s:0` ticks → ~0% CPU
  `t=12s:166, t=14s:35` ticks → burst correlated with activity
- A later run (release-ish build in use) idles at 14–16% CPU, ~50% with pointer motion.
- Earlier 99.8% readings came from a stale pre-fix process and are not evidence.

## Root Cause Analysis

**CONFIRMED via debug log** (`~/.local/state/eswm/eswm.log`, 2026-11-16T19:02):

Render timeline sampled per second:
```
19:02:19: 147   19:02:20: 147   19:02:21: 138   19:02:22: 147
19:02:23: 149   19:02:24: 138   19:02:25: 147   19:02:26: 144
```
That is **~2.4 renders per 60 Hz refresh** — the compositor is unpaced.

Mechanism (feedback loop):
1. A client commits ⇒ `main.rs:352 commit()` sets `redraw_needed = true`.
2. `run_udev` main loop (src/udev.rs:581–596) immediately renders — **no vblank/in-flight gate**.
3. `render()` sends frame callbacks to clients right away (`sent_callbacks = true`, src/udev.rs:2340).
4. Client repaints on the callback ⇒ commit ⇒ render again, faster than the panel presents.

The vblank timer removed in BUG-2026-11-16T17:45:54 was also the only present pacer. Without it, frames queue faster than vblank; `queue_frame`/flip errors and missed presents cause judder — perceived as freeze when the pointer is still and smoothness when pointer motion adds regular refreshes on top.

Contributing factor: autostart config runs `awww-daemon` (animated wallpaper) and `waybar`; their continuous commits feed the loop.

Evidence:
- `queue_frame: EmptyFrame` occurs only 5 times; the loop is not damage-driven-empty, it is over-rendering.
- Idle CPU 14–16% steady (release), ~47–50% while pointer moves.
- Fixed binary earlier measured 0% CPU for 10 idle s when no continuous-commit client was attached — consistent with client-driven over-render, not a free-running timer.

Risk level: Medium. Behavioral (present pacing) change in the hot render path.

## TDD Fix Plan

1. **RED**: unit test `should_render(redraw_needed, frame_pending)`:
   - `(false, false)` → false (idle)
   - `(true, false)` → true (needs frame, nothing in flight)
   - `(true, true)` → false (a flip is in flight — wait for vblank)
   - `(false, true)` → false
   **GREEN**: add `should_render` and gate the per-node render in the `run_udev` main loop on it, leaving `redraw_needed` set when the frame is deferred.
   **verify**: `cargo test`

2. **RED**: unit test `render_timer_delay` still yields a delay only when a redraw is pending (existing behavior preserved after pacing change).
   **GREEN**: reuse in `frame_finish` so the vblank completion schedules the *next* render.
   **verify**: `cargo test`

3. **RED**: unit test that `redraw_needed` is not cleared when a frame is deferred (pacing must preserve it).
   **GREEN**: clear the flag only after a successful render/present.
   **verify**: `cargo test`

**REFACTOR**: remove leftover debug instrumentation in the render path if it no longer serves diagnosis (separate commit).

## Acceptance Criteria

- [x] Renders per second ≤ refresh rate (max 49 observed on 60 Hz; was 138–149)
- [ ] Idle CPU materially below 14–16% — bursts of 3–8% seen in quiet windows, ~22% average with activity; needs a clean idle measurement
- [ ] No stutter/freeze when the pointer is stationary — awaiting user confirmation
- [ ] Wallpaper animation and bar remain smooth during activity — awaiting user confirmation
- [x] All existing tests pass (60/60, clippy clean)

## Resolution

Fixed by gating rendering to one frame in flight per output (commits `29ff16a` test → `8902405` fix):
- `SurfaceData.frame_in_flight` set on successful `queue_frame`, cleared on vblank in `frame_finish`.
- `should_render(redraw_needed, frame_in_flight)` + `next_redraw_state` gate the paint; a deferred redraw keeps `redraw_needed` set so the vblank-scheduled timer paints it next.
- Main loop clears `redraw_needed` only when every output actually painted.
- Vblank timer callback no longer consumes the flag before rendering.

Live verification (debug log, DRM backend): renders/s dropped from a sustained 138–149 to 7–49 (max 49, average ≈27) on a 60 Hz panel; zero `EmptyFrame` events.

Follow-up (out of scope): frame callbacks are still sent at render time rather than on vblank (cosmic-comp sends them in `on_vblank`). That is the next lever if stutter recurs under heavy client load.

## Round 4 — empty frames were being treated as failed paints

Live debug run with `awww-daemon` (animated wallpaper) enabled showed the real
freeze mechanism:

```
601 render attempts in ~40s → 284 presented, 317 EmptyFrame
first 5s window: 30 frames, 79 deferred, 292 empty
attempt cadence during the burst: one every 0.4ms (back to back)
after an empty frame no wayland/input source fired before the next attempt
```

Mechanism:

1. A client commit sets `redraw_needed`.
2. `render_frame` returns `is_empty` when the damage tracker saw no change —
   i.e. the buffer already on the scanout is that exact frame.
3. `queue_frame` then refuses with `EmptyFrame`.
4. The old code treated that as a *failed paint*: `painted_all` stayed false, so
   `redraw_needed` was **never cleared**, and `reschedule = !queued` armed
   another 16ms retry. Empty renders therefore chained forever, saturating the
   loop and starving the next real commit/vblank — the desktop looked frozen
   until pointer motion injected fresh events.

Fixes (TDD: `682e692` test → `fc52ecf` fix, `3f417bb` test → `644a14c` fix):

- `frame_outcome(is_empty, queued)` — an empty frame is `NothingToDo` and
  satisfies the pending redraw; only a real queue failure is `Retry`.
- Retry timer checks `redraw_needed` and clears it on success (cannot spin).
- Frame callbacks are sent only for frames that actually presented, so a client
  is never invited to commit for content that did not change.
- Callback throttle is the refresh period (`callback_min_gap_ms`, 16ms at
  60001 mHz) instead of a hard 33ms that capped every callback-driven client at
  30fps on a 60Hz panel — a self-built frame skipper.

Pending: user re-test on the DRM session for the smoothness/idle-CPU criteria.

### Correction after live test: never skip the flip

The first cut of the empty-frame fix skipped `queue_frame` when `render_frame`
reported `is_empty`. That broke presentation: a pointer-only change carries no
primary-plane damage yet still updates the cursor plane, and `queue_frame` is
the only thing that applies it. Live result: invisible cursor, dropped partial
frames (flicker).

`frame_outcome` now takes the *queue* result, not the damage hint:

| `queue_frame` | outcome |
|---|---|
| `Ok` (including cursor-plane-only) | `Presented` |
| `Err(EmptyFrame)` | `NothingToDo` — request satisfied, no retry |
| any other `Err` | `Retry` |

Tests: `21dbe5b` (`cursor_only_update_still_presents`) → `51260d3` fix.



<!-- superseded candidate list retained for history -->

## Candidate causes (to be confirmed)

1. **Unpaced render loop / frame-callback ping-pong.** `render()` sends frame callbacks to drawn surfaces on every frame (`sent_callbacks = true`), and `main.rs:349 commit()` sets `redraw_needed = true` for every commit. Clients that animate on frame callbacks (blinking cursors, panels, terminals) therefore drive an unpaced redraw loop: commit → render → callback → commit. Each frame re-renders all elements in software, which fits the 14–16% figure and the burst while moving the pointer.
2. **No vblank pacing after the timer fix.** The removed vblank timer also acted as the present pacer. With `redraw_needed` gating and `queue_frame` called on every render, frames may queue faster than vblank, producing `AlreadySwapped` errors and visible stalls. Check `frame_finish` / `queue_frame` log lines for `AlreadySwapped`.
3. **Cursor-still freeze mechanism** — unclear yet. `render_frame` uses `ALLOW_CURSOR_PLANE_SCANOUT`; cursor-only frames report `is_empty` (see comment near src/udev.rs:2340–2355: gating on damage previously froze the scanout). Need debug log to see whether `render:`/`queue_frame` stops entirely while idle.
4. **RSS 200 MB** — may be per-frame buffer/allocation churn related to the same loop; not the primary complaint.

## TDD Fix Plan

Pending RCA. Do not implement yet.

## Acceptance Criteria

- [ ] Idle desktop (cursor untouched, no client activity) consumes ~0–2% CPU
- [ ] Desktop does not stutter/freeze when the pointer is stationary
- [ ] Pointer motion and client commits remain smooth (no regression)
- [ ] All existing tests pass

## Resolution

<!-- filled in by validate-fix -->

## References

- BUG-2026-11-16T17:45:54: fixed unconditional vblank re-render timer (idle CPU 80–93% → ~0% measured, but 14–16% observed at times)
- e05s04: `redraw_needed` gate in `run_udev`, `frame_finish` timer, damage tracker notes
