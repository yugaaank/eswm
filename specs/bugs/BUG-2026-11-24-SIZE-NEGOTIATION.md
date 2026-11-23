---
bug_id: BUG-2026-11-24-SIZE-NEGOTIATION
status: fixed
severity: high
scope: rendering
title: Tiled frames stay unfilled because undersized commits are accepted as fitting
---

# BUG-2026-11-24-SIZE-NEGOTIATION — Tiled frames stay unfilled because undersized commits are accepted as fitting

**Status:** fixed on main as 5306de7 (merge of f6bf838); live DRM UAT pending operator
**Severity:** high (windows never fill their tiled frame; content is cropped)
**Reported by:** operator (live DRM session, 2026-11-23 19:25 local)
**Discovered in:** `~/.local/state/eswm/eswm.log.old`, `~/.config/deckcraft/logs/deckcraft.log`
**Related:** `BUG-2026-11-21-DMABUF-IMPORT`, `BUG-2026-11-22-OVERSIZE-RETRY-CAP`

**Security impact: NONE.** Local client already controls its own buffer size; the
fix only changes how often the compositor re-asserts a configure.

## Problem

In tiling mode the Storyteller `*craft` apps (deckcraft, gridcraft, lightcraft,
wordcraft — winit/egui) do not fill their frame. eswm's border and titlebar draw
correctly at `target_rect`, but the client content is either blank in part of the
frame (frame larger than 1440x900) or cropped to a fragment (frame smaller than
1440x900).

Observed in the 2026-11-23 19:25 live run:

- deckcraft (toplevel 5) configured `1896x1078` at `19:25:47.717`.
- Its wl_surface configured `1440x900` at `19:25:48.085`.
- eswm never re-sent a configure; no `re-sending size` line follows.
- `~/.config/deckcraft/logs/deckcraft.log`: `request_inner_size 1440x900`,
  `egui_winit: Failed to set window size`, surface configured `1440x900`.

Reproduce: tiling session on a TTY, `RUST_LOG=debug`, open deckcraft in a frame
larger than 1440x900, then grep `eswm.log` for `configured window id=` and
`re-sending size`.

## Root Cause Analysis

### Phase 1 — Reproduce

Log evidence above. Two shapes of the same defect:

1. Frame larger than 1440x900 → client commits 1440x900, eswm accepts it as a
   fitting commit, the rest of the frame stays blank.
2. Frame smaller than 1440x900 → client ignores the configure and keeps
   1440x900; `ClippedSurfaceElement` crops it to the frame, so only part of the
   app is visible.

### Phase 2 — Isolate

The decision lives in one predicate, `wm::oversize_action`:

```rust
if buffer.0 > target.width || buffer.1 > target.height { ... }
else if attempts > 0 { ResetBudget } else { None }
```

Only "buffer exceeds target" is a mismatch. A buffer smaller than the target is
treated as a fitting commit (`None` / `ResetBudget`), so `note_oversize_commit`
is never called, `window.rect` is not cleared, and `apply_layout` finds
`window.rect == target` and skips the configure. There is no code path in the
compositor that asks a client to grow.

### Phase 3 — Hypothesize

1. **H1 (chosen): the size policy is one-directional.** Falsification: commit a
   buffer smaller than `target_rect` with a fresh budget; the policy must ask
   for a resend. It returns `None`, so H1 holds.
2. H2: the client never receives the configure. Falsification: the log shows the
   configure was sent (`configured window id=5 -> 1896x1078`) and the client
   committed afterwards; smithay logs no send failure. Rejected.
3. H3: the crop hides a correctly sized surface. Falsification: frame 1896x1078
   is larger than the committed 1440x900, so clipping leaves blank space, not a
   crop. Rejected for shape 1.

### Phase 4 — Verify (via second defect found in the same path)

The retry budget can be consumed in a burst: `note_oversize_commit` increments
per *commit*, and the commit handler calls `apply_layout` synchronously, so any
already-queued commits in the same event-loop drain spend the whole budget
before the client can react. Observed at `19:25:31.620487`, `.620834`, `.621047`
— three resends in ~0.5 ms. Clients that need a round trip effectively get no
retry at all. Bounded, but useless.

A second check was made on the reported "clip vs render origin" concern. It is
**not a defect**: the clip rect is the window-geometry rect (`target_rect`) in
output space, and the surface is rendered at `target_rect - xdg_offset`, so the
client's content origin lands exactly on the clip origin. `clip.rs` unit tests
`clip_offsets_src_when_geometry_starts_before_clip` already pin that CSD case.
Cropping the CSD shadow outside the frame is intended (the frame is the visual
boundary). No change made; a regression test was added to document the intent.

## Fix Plan

1. Rename the one-directional policy to size negotiation:
   - `oversize_action` → `size_action`, `OversizeAction` → `SizeAction`
     (`Resend`, `ResetBudget`, `Throttled`, `None`).
   - `Window::oversize_configures` → `size_resends`,
     `MAX_OVERSIZE_CONFIGURES` → `MAX_SIZE_RESENDS`,
     `note_oversize_commit` → `note_size_resend`,
     `clear_oversize` → `clear_size_resends`.
2. Treat any direction as a mismatch: resend while budget remains; reset only
   on an exact match.
3. Ignore zero-sized targets and zero-sized buffers (no layout yet / unmap), so
   a pre-layout commit cannot burn budget.
4. Throttle resends: record `last_size_resend` and defer a resend that arrives
   within `SIZE_RESEND_INTERVAL` (100 ms) of the previous one. Deferred commits
   do not spend budget. This fixes the sub-millisecond burst.
5. Commit handler: log oversize and undersize resends distinctly, dispatch all
   four outcomes explicitly.

## Verify Steps

- `cargo test --bin eswm size_action` — policy truth table, both directions,
  budget, throttle, invalid input.
- `cargo test --bin eswm size_resend` — budget/rect bookkeeping.
- `cargo test --bin eswm clip_` — CSD crop regression stays green.
- `cargo clippy && cargo build`.
- Live DRM UAT (operator): tile deckcraft in a frame larger than 1440x900 and
  confirm it fills; tile in a frame smaller and confirm the client resizes.
  Grep `eswm.log` for `re-sending size`; expect at most 3 per window, spaced by
  at least 100 ms, and a final exact-size commit.
