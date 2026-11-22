---
bug_id: BUG-2026-11-22-OVERSIZE-RETRY-CAP
status: closed
uat_disposition: operator_confirmed_stale_pixel_gate
severity: medium
scope: rendering
title: Oversize size-reassert retry cap resets itself and storms the client
---

# BUG-2026-11-22-OVERSIZE-RETRY-CAP — Oversize size-reassert retry cap resets itself and storms the client

**Status:** closed (operator-authorized UAT pass 2026-11-22 13:41 IST — see UAT disposition below)
**Severity:** medium (log flood, needless reconfigures, UAT was blocked)
**Reported by:** operator during e09s01 manual UAT (2026-11-22 12:35 local)
**Discovered in:** `/tmp/opencode/eswm-latest-clean3.log` (UTC 2026-11-22 07:05:57–07:06:22)
**Related:** `BUG-2026-11-21-DMABUF-IMPORT` (its size-reassert sub-fix introduced this defect)

**Security impact: NONE.** No untrusted input crosses a boundary in a way that
can be amplified into memory exhaustion, privilege escalation, or data exposure;
a local client can already cause commits. No exploit path identified.

## Problem

After the cross-GPU dmabuf fix, Craft/Electron windows still spilled outside their
frames, and the compositor sent far more size configures than the intended cap of
three per continuous oversized run.

- Actual: 57 `client committed 1440x900 > configured 1190x795; re-sending size`
  messages in ~14 seconds — 48 for deckcraft, 9 for gridcraft. The stream was
  still active when the operator quit.
- Expected: at most three reconfigures for a stubborn client, then silence; a
  fitting commit may reset the budget for a later, separate event.

Reproduce: on the hybrid-GPU laptop, run the fixed integration binary on a TTY
with `RUST_LOG=debug`, launch deckcraft and gridcraft, then grep `eswm.log` for
`re-sending size`.

## Root Cause Analysis

### Phase 1 — Reproduce

Valid fixed binary confirmed by `scanout tranche`, `linear render tranche`,
`dmabuf imported` markers and 5,644 DEBUG lines. Environment: Intel scanout
`renderD128` at 1920x1200, NVIDIA `card0`/`renderD129`, output work area
1920x1170 after the 30 px dock. Toplevel 3 (deckcraft) registered at
`07:06:06.964Z`, toplevel 5 (gridcraft) at `07:06:19.882Z`.

Counts: 49 configure lines for toplevel 3 vs 48 reassert messages; 10 vs 9 for
toplevel 5. The extra configure in each case is the initial map configure, so
**every** subsequent configure came from the reassert path. lightcraft (id 4)
committed a fitting buffer and produced no reasserts.

### Phase 2 — Isolate

The defect is in the surface-commit policy that decides whether to re-send a
size configure or to reset the retry budget. Two separate conditions are folded
into one predicate: "buffer exceeds target" and "budget remains". The reset
branch then fires whenever that combined predicate is false, including the case
where the budget is exhausted but the buffer is still oversized.

The retry-budget accounting lives in the window-state module, which increments
the counter and clears the applied-rect marker; the commit handler is the only
caller. Recent history: the combined predicate and budget were added with the
dmabuf fix (`a2c8edb`), then merged into this branch (`84706cb`). No earlier
code path had this policy.

### Phase 3 — Hypothesize

Ranked hypotheses, each with a falsification test:

1. **H1 (chosen): the reset branch treats "budget exhausted" as "buffer fits".**
   Falsification: after three consecutive oversized commits, a fourth oversized
   commit must produce no new action and must leave the budget exhausted. If it
   instead resets, H1 holds.
2. H2: the client alternates fitting and oversized commits, legitimately
   resetting the budget. Falsification: 48 retries at a cap of 3 require at
   least 16 fitting commits; none are logged (only oversized commits are), and
   every configure line pairs with an oversized commit, so this would require an
   unlogged, perfectly periodic fitting commit. Not supported.
3. H3: unrelated layout changes trigger configures. Falsification: configure
   count would then exceed reassert count; it is exactly reasserts + 1 for each
   toplevel. Falsified.

### Phase 4 — Verify

Code inspection confirms H1: the combined predicate returns false once the
attempt counter reaches the cap, and the `else if attempts > 0` branch then
resets the counter to zero. The next oversized commit sees a fresh budget and
reasserts again. Algebra matches the log: 48 = 16 × 3 retries for deckcraft,
9 = 3 × 3 for gridcraft; each run ends only because the client was closed or
the compositor quit. H2 and H3 are falsified. Single verified root cause: the
reset decision must be based on buffer fit alone, never on budget exhaustion.

Risk level: **low** for the code change (pure policy helper plus one call site);
**medium** impact because UAT for the parent bug remains blocked.

## TDD Fix Plan

1. **RED**: Add a pure policy test — an oversized commit with attempts 0 asks the
   compositor to resend the configured size.
   **GREEN**: Add a policy helper returning `Resend`, `ResetBudget`, or `None`
   from (target rect, buffer size, attempt count), implementing the resend case.
   **verify**: `cargo test --bin eswm oversize_commit_action`

2. **RED**: Add a test — repeated oversized commits stop after the cap and leave
   the budget exhausted; no further resend is requested.
   **GREEN**: Guard the resend case with the retry cap; exhausted oversized
   commits return `None` without resetting.
   **verify**: `cargo test --bin eswm oversize_commit_action`

3. **RED**: Add a test — a fitting commit after retries requests `ResetBudget`,
   and that reset allows a later oversized commit to resend again.
   **GREEN**: Return `ResetBudget` only when the buffer fits and attempts > 0.
   **verify**: `cargo test --bin eswm oversize_commit_action`

4. **REFACTOR (implemented)**: Route the surface-commit handler through the
   policy helper, then delete the old boolean predicate entirely once it has no
   production caller. Its boundary assertions were folded into the policy test
   suite so the observable contract stays covered without a dead helper.
   **verify**: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`

**REFACTOR (done)**: One policy helper is the single source of truth; the commit
handler only dispatches on its result. A short humorous comment sits near the
helper — the retry budget finally has a backbone, unlike my New Year's resolutions.

**TDD evidence**: three test-only RED commits (`87f4d2a`, `c4e2d96`, `6c8b280`)
each followed by a GREEN implementation commit (`f17b94e`, `5d89e76`, `7a856b6`),
then refactor `ba802be`. The repo has no `scripts/verify-tdd-red-commit.sh`, so
the mechanical isolation script was unavailable; `git log -2 --oneline` and the
failing-test output were shown instead.

## Acceptance Criteria

- [ ] Three consecutive oversized commits yield exactly three reasserts; a
      fourth yields none and does not reset the budget.
- [ ] A fitting commit resets the budget and a later oversized commit may
      reassert again.
- [ ] New tests and all 156 existing tests pass; clippy and build are clean.
- [ ] Live DRM run shows no `re-sending size` storm for deckcraft/gridcraft.
- [ ] Live DRM run still shows no `BAD_MATCH` / `EGLImage` errors.
- [ ] Parent bug `BUG-2026-11-21-DMABUF-IMPORT` remains open until Helium is
      actually launched and verified in a fixed-build session.

## Resolution

**Fixed (mechanical): 2026-11-22**
**Root cause confirmed:** the retry reset branch fired on predicate `false`,
which included budget exhaustion, so a stubborn client got a fresh three-try
budget after every three oversized commits.
**Fix applied:** `OversizeAction` policy in the window-state module returns
`Resend`, `ResetBudget`, or `None`; the commit handler dispatches on it. Budget
is reset only for a fitting buffer.
**Hardening added:** type guard (three-outcome enum) plus an exhaustive match in
the commit handler, so a new action cannot be silently ignored. Regression test
`oversize_action_never_resets_an_oversized_commit` covers every attempt count.
**Evidence:** `cargo clippy --all-targets` clean; `cargo test --bin eswm` 160
passed; `cargo build` green. TDD commits `87f4d2a`→`f17b94e`, `c4e2d96`→
`5d89e76`, `6c8b280`→`7a856b6`; refactors `ba802be`, `617ecc2`; hardening test
`fa39b4f`.
**Generalize sweep:** `specs/verifications/generalize-sweep-BUG-2026-11-22-OVERSIZE-RETRY-CAP.json`
— defect class documented, 10 matches in `src/*.rs`, all inside this policy.
**Commit:** `refactor(main): exhaustive oversize dispatch prevents silent new actions`

## Follow-up fix — render-time clipping (2026-11-22, 13:11 run)

The 13:11 fixed-binary run confirmed the retry cap works (exactly 3 re-sends per
window, no storm) and Helium content rendered. But Craft windows still spilled
outside the border: deckcraft/gridcraft commit `1440x900` buffers against a
`1190x795` frame and ignore all configures. Deckcraft's own log confirms it
requests `1440x900` at startup; this is not a CSD-offset problem (`offset=(0,0)`).

The retry-cap fix alone cannot solve this — a client that refuses the configure
will always overflow an unclipped render. Added a `ClippedSurfaceElement` render
element that crops `src`/`dst` proportionally to the configured frame, with
`underlying_storage() -> None` so direct scanout cannot bypass the crop.
Applied to toplevel surface trees in both render branches; popups retain their
shadows. Pure clipping math has 5 tests.

**Commits:** `b831a9a` (test), `e28fc09` (fix).
**Validation:** clippy clean, 165 tests passed, build green.

**Hardware UAT: PENDING.** Mechanical validation is green but behavioral proof
requires a live DRM session. Re-run the fixed binary with Helium plus
deckcraft/gridcraft, then confirm (a) no `re-sending size` storm, (b) no
`BAD_MATCH` / `EGLImage` errors, and (c) Craft content stays inside the border.
Until then this bug stays open, the parent dmabuf bug stays open, and no branch
is merged into `main`.

## Follow-up defect — clipped damage/opaque regions use the wrong coordinate space (2026-11-22, 13:22 run)

Operator reported "the windows are glitching" during the live clipping UAT.
Log analysis of `/tmp/opencode/eswm-latest-clean5.log` (UTC 07:52:52–07:53:17)
rules out the earlier suspects: zero `BAD_MATCH`, `EGLImage not supported`,
`Failed to convert between dmabuf`, and `Failed to import surface`; the six
`re-sending size` lines are exactly three per Craft window; `render deferred`
344 vs 604 and `frame gap` 276 vs 280 in the prior run, so frame timing did not
regress. The only ERROR lines are shutdown-time DRM restore permission noise.

Root cause is in `src/clip.rs`, introduced by the clipping follow-up:
`damage_since()` and `opaque_regions()` filter the inner element's regions
with `self.clip`, but smithay's `WaylandSurfaceRenderElement` returns damage
and opaque rectangles **relative to the inner element's own origin**, while
`self.clip` is in **output coordinates**. For a window at `(393, 215)` an
inner damage rect `(0, 0, 1440, 900)` intersects the clip `(393, 215, …)` as
empty, so every content damage rectangle is discarded. Smithay's damage
tracker then adds this element's `geometry().loc` to whatever survives, so
even a surviving rect would be offset twice when the CSD offset is nonzero.
The renderer therefore repaints only on geometry changes, leaving stale
pixels and visible glitches — damage tracking is the compositor's memory,
and our clipping wrapper just gave it amnesia.

Fix plan (TDD, vertical slices):
1. **RED**: test that a surface-local damage rect at a nonzero surface origin
   maps to a clipped-element-local rect inside the frame. **GREEN**: add a
   pure `clip_local_region` helper and route `damage_since()` through it.
2. **RED**: test that a surface-local opaque rect at a nonzero CSD offset maps
   to a clipped-element-local rect and that outside rects are dropped.
   **GREEN**: route `opaque_regions()` through the same helper.

**Fixed (mechanical): 2026-11-22.** `clip_local_region` translates a region
from inner-surface-local to clipped-element-local coordinates: add the inner
origin, intersect the output-space clip, subtract the clipped origin. Smithay
then adds the clipped origin once, exactly as its damage tracker expects.
Both `damage_since()` and `opaque_regions()` use that helper. Four behavior
tests exercise the wrapper through its `Element` interface using a fake inner
element (oversize Craft buffer, nonzero CSD offset, and an out-of-frame rect).

**TDD evidence:** RED `9da9625` → GREEN `5fbbb7b` (damage), refactor `0ecc6c9`
(make wrapper generic over `Element` so the wiring is testable), RED `94594ac`
→ GREEN `e08adfa` (opaque). RED failures showed the exact wrong output:
`(393,215,1047,685)` instead of `(0,0,1190,795)`, and an empty opaque set
instead of `(84,40,200,100)`. Validation: clippy clean, 169 tests passed,
build green. As noted in the code, damage tracking now remembers where it
painted — no longer a goldfish with a hole punch.

Acceptance adds to the existing UAT gate: content updates must repaint without
moving or resizing the window (no stale pixels after typing or a spinner
animation), including a CSD window with `offset=(16,10)`.

## UAT disposition — 2026-11-22 13:41 IST

Operator report, verbatim: **"stale pixels are gone, commit and merge into main"**.
The stale-pixel gate for the `clip_local_region` fix (`e08adfa`) is confirmed.
The 13:31 log also shows the original retry-cap defect fixed: 3 bounded
`re-sending size` messages (not 57), no `BAD_MATCH` / `EGLImage` errors, 0 queue
errors over 832 frames.

Closed on the operator's explicit merge instruction. Craft content staying
inside the border and CSD `offset=(16,10)` were not explicitly reconfirmed in
this session; they remain recorded as unconfirmed in the verification records,
not silently waived. The retry budget now keeps its promise even if its New
Year's resolutions were historically optimistic.
