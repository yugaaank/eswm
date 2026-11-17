---
bug_id: BUG-2026-11-17T00:30:00
status: fixed
severity: high
scope: udev-render-loop
title: Stutter/freeze on idle desktop; sustained 14-16% idle CPU after render-loop fix
---

# BUG-2026-11-17T00:30:00: Stutter/freeze when cursor is still, 14–16% idle CPU

## Problem

After the vblank-timer fix (BUG-2026-11-16T17:45:54), idle CPU dropped from
80–93% to **14–16%**, but the desktop still stuttered/froze when the pointer was
stationary, and later reported as "the whole screen freezes, cursor included,
and moving the pointer restarts it".

Observation from the user (live DRM session):

- Cursor still → screen lagging/freezing, CPU ~14–16%
- Cursor moving → screen fine, CPU ~50%
- RSS ~200 MB steady (down from 291 MB); Hyprland on another VT idles at 0% / ~172 MB

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

Five independent defects sat on top of each other; each one masked the next, so
every round of measurement pointed at the wrong layer.

### 1. Unpaced render loop (~2.4 renders per refresh)

Debug log, 2026-11-16T19:02, renders per second:

```
19:02:19: 147   19:02:20: 147   19:02:21: 138   19:02:22: 147
19:02:23: 149   19:02:24: 138   19:02:25: 147   19:02:26: 144
```

Mechanism: a client commits → `commit()` sets `redraw_needed` → the main loop
renders immediately with **no vblank / in-flight gate** → callbacks invite the
client to repaint → commit again, faster than the panel presents. The vblank
timer removed in BUG-2026-11-16T17:45:54 had also been the only present pacer.

Fixed by gating rendering to one frame in flight per output (`29ff16a` test →
`8902405` fix):

- `SurfaceData.frame_in_flight` set on successful `queue_frame`, cleared on vblank
  in `frame_finish`.
- `should_render(redraw_needed, frame_in_flight)` + `next_redraw_state` gate the
  paint; a deferred redraw keeps `redraw_needed` set so the vblank-scheduled
  timer paints it next.
- The main loop clears `redraw_needed` only when every output actually painted.

Live result: renders/s fell from a sustained 138–149 to 7–49 on a 60 Hz panel.

### 2. Empty frames were treated as failed paints

Debug run with `awww-daemon` (animated wallpaper) enabled:

```
601 render attempts in ~40s → 284 presented, 317 EmptyFrame
first 5s window: 30 frames, 79 deferred, 292 empty
attempt cadence during the burst: one every 0.4ms (back to back)
```

`render_frame` returns `is_empty` when the damage tracker saw no change, and
`queue_frame` then refuses with `EmptyFrame`. The old code treated that as a
*failed paint*: `painted_all` stayed false, so `redraw_needed` was never
cleared, and `reschedule = !queued` armed another 16 ms retry. Empty renders
chained forever, saturating the loop and starving real commits — the desktop
looked frozen until pointer motion injected fresh events.

Fixes (`682e692` → `fc52ecf`, `3f417bb` → `644a14c`):

- An empty frame satisfies the pending redraw; only a real queue failure retries.
- The retry timer checks `redraw_needed` and clears it on success (cannot spin).
- Frame callbacks are sent only for frames that actually presented.

**Correction after live test: never skip the flip.** The first cut skipped
`queue_frame` when `render_frame` reported `is_empty`. That broke presentation:
a pointer-only change carries no primary-plane damage yet still updates the
cursor plane, and `queue_frame` is the only thing that applies it. Live result:
invisible cursor, dropped partial frames (flicker). `frame_outcome` now keys off
the *queue* result:

| `queue_frame` | outcome |
|---|---|
| `Ok` (including cursor-plane-only) | `Presented` |
| `Err(EmptyFrame)` | `NothingToDo` — request satisfied, no retry |
| any other `Err` | `Retry` |

Tests: `21dbe5b` → `51260d3`.

### 3. dmabuf feedback advertised the idle dGPU

Live stats after the above: every frame 6–15 ms with `queue` at 0.4 ms, i.e. all
the cost in `render_frame`, and 30–35 slow frames per 5 s while `sl` ran.

```
card1 = Intel Iris Xe   → renderD128 → drives eDP-1
card0 = NVIDIA RTX 2050 → renderD129 → drives nothing
advertised dmabuf device: renderD129
Error importing dmabuf from card0 to renderD128: Failed to import buffer
Falling back to cpu-copy.
```

The dmabuf global was built from the first GPU whose `bind_wl_display`
succeeded — a meaningless probe on Mesa, where `EGL_WL_bind_wayland_display`
only exists on Wayland-platform displays. Clients allocated on the NVIDIA GPU and
every full-screen frame paid a CPU copy.

Fixed by deriving the dmabuf device from the backend that actually has outputs
(`5bc7099` → `b613102`). Result: `prep` 1.0–1.2 ms, max frame 3.1 ms, 0 slow.

### 4. Frame callbacks were gated on two different clocks

With frames at ~1 ms the user still reported whole-screen hitches: 47
`frame gap` warnings in ~60 s, gaps up to 6.4 s, `queue err` 0.

`render()` takes a timestamp from its caller and the two callers disagreed
about the epoch:

| caller | timestamp |
|---|---|
| event loop after `dispatch` | `now(self)` — ms since compositor start |
| `frame_finish` timer | `next_frame_target` = DRM vblank time — ms since boot |

`frame_callbacks` compared that value against `last_frame_callback_ms` and
returned early when the difference was below the refresh period. With the
epochs alternating, `saturating_sub` produced 0 and the callback was dropped:
clients stalled until some later render happened to deliver one — bursts at
60 fps (300 frames/5 s) interleaved with 200 ms–2 s of silence.

Fixed by deriving the callback time from `now(self)` inside `frame_callbacks`
(`a03b97c` → `80a8e0a`).

### 5. The freeze: a dropped frame callback deadlocks the client

Final run with commit→present latency and stall blame: 63 hitches, 102
`presented N ms after commit (late)` up to 885 ms. The decisive detail came from
the user: *the whole screen freezes, cursor included, and moving the pointer
restarts it.*

- `flip max` stayed 16–19 ms and `queue err` 0 → **no flip was ever submitted**
- no `loop blocked` warning → the loop was idle, not stuck
- `late max` up to 885 ms → a client had committed and was waiting

Mechanism:

1. Client draws, commits **with a `wl_callback.done` request**, then blocks
   waiting for it. It has nothing else to do.
2. The commit landed within one refresh of the previous callback, so
   `frame_callbacks`' pacing gate dropped the callback.
3. The client stays blocked → no commits → no damage → no flip → no callback.
   Self-sustaining deadlock.
4. No flip means no atomic commit, which is also what moves the cursor plane, so
   the pointer froze with the rest of the screen.
5. Moving the pointer sets `redraw_needed`, forces a present and delivers the
   callback — the deadlock breaks, exactly as the user described.

Why every earlier metric missed it: `prep`/`queue` measure flips that happen,
`flip max` measures flips that complete, `loop blocked` measures a busy loop.
Here nothing was submitted and nothing was busy — the compositor was waiting for
a client that was waiting for the compositor.

Fixed by delivering pending callbacks on **every** present (`b0fc67f` →
`02f57d2`). Pacing was never the gate's job: only one flip is in flight, so
presents cannot outrun the refresh, and a throttle on top can only lose
callbacks.

## Resolution

Fixed and confirmed by the operator on the DRM session ("yes now fixed").

| # | defect | commit |
|---|---|---|
| 1 | `frame_finish` scheduled a re-render unconditionally, driving a 60fps loop with nothing damaged | `8902405` |
| 2 | empty frames were treated as failed paints and spun a retry chain | `fc52ecf` / `51260d3` |
| 3 | dmabuf feedback advertised the idle dGPU, so every client frame fell back to a CPU copy | `b613102` |
| 4 | frame callbacks were gated on two different clocks, silently silencing clients | `80a8e0a` |
| 5 | a pending frame callback was dropped as "too soon", which blocks the client instead of delaying it — no commit → no damage → no flip → no callback | `02f57d2` |

Final state on the panel: `prep` ~0.4 ms, `queue` ~0.1 ms, `flip max` 16–19 ms,
zero `queue err`, zero `presented N ms after commit (late)`.

Diagnostics kept on purpose: the 5-second `render stats` INFO line, plus
`frame gap ... with N commits, M inputs waiting` and `presented N ms after
commit (late)`. They are the difference between "idle screen" and "dropped
frame", which is what cost four rounds of guessing here.

## Acceptance Criteria

- [x] Renders per second ≤ refresh rate (max 49 observed on 60 Hz; was 138–149)
- [ ] Idle desktop (cursor untouched, no client activity) consumes ~0–2% CPU — idle windows now present 0–2 frames; not measured end-to-end since the final fix
- [x] Desktop does not stutter/freeze when the pointer is stationary — operator confirmed
- [x] Pointer motion and client commits remain smooth (no regression) — operator confirmed
- [x] Wallpaper animation and bar remain smooth during activity — operator confirmed
- [x] All existing tests pass (72/72, clippy clean)

## References

- BUG-2026-11-16T17:45:54: fixed unconditional vblank re-render timer (idle CPU 80–93% → ~0%)
- e05s04: `redraw_needed` gate in `run_udev`, `frame_finish` timer, damage tracker notes
