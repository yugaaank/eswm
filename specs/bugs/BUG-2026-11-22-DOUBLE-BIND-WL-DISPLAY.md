---
bug_id: BUG-2026-11-22-DOUBLE-BIND-WL-DISPLAY
status: open
severity: low
priority: p2
scope: drm
title: Wayland display bound twice on one physical GPU at startup
---

# BUG-2026-11-22-DOUBLE-BIND-WL-DISPLAY — Wayland display bound twice at startup

**Status:** open — RCA verified, fix pending
**Discovered:** log analysis of `/tmp/opencode/eswm-latest-clean6.log`
(UTC 2026-11-22 08:01:26–08:02:00)
**Security impact: NONE.** Local GPU initialization only; no untrusted input and
no exploit path identified.

## Problem

During hardware-acceleration setup, eswm logs:

```
trying GPU card0 for hardware acceleration
egl hardware-acceleration enabled on card0
trying GPU card1 for hardware acceleration
trying GPU renderD128 for hardware acceleration
trying GPU renderD129 for hardware acceleration
WARN smithay::backend::egl::display: Double bind_wl_display, smithay does not support this, please report
egl hardware-acceleration enabled on renderD129
```

Expected: one `bind_wl_display` per process and no smithay warning.

## Root Cause Analysis (4-phase, verified)

### Phase 1 — Reproduce

Hybrid-GPU laptop with `/dev/dri/card0` (226,0), `card1` (226,1),
`renderD128` (226,128), `renderD129` (226,129). Start eswm on DRM and inspect
the first second of logs. Warning at `2026-11-22T08:01:29.918627Z`. Live DRM is
operator-only in this session, so reproduction used the captured log plus the
smithay 0.7.0 source that emitted the warning.

### Phase 2 — Isolate

The hardware-acceleration loop in the DRM setup module calls
`EglRenderer::bind_wl_display` for **every** candidate GPU. smithay's
`EGLDisplay::bind_wl_display` (`smithay-0.7.0/src/backend/egl/display.rs:847-864`)
guards a **process-global** `BUFFER_READER` lock. It warns whenever a second
bind succeeds, regardless of which physical GPU or EGL display is involved:

```rust
let mut global = BUFFER_READER.lock().unwrap();
if global.as_ref().and_then(|x| x.upgrade()).is_some() {
    warn!("Double bind_wl_display, smithay does not support this, please report");
}
*global = Some(WeakBufferReader { display: Arc::downgrade(&self.display) });
```

The second success silently replaces the first reader. The existing `(dev_id,
node_type)` dedup is irrelevant: all four candidates have distinct `dev_id`s.

### Phase 3 — Hypothesize (ranked, with falsification)

| # | Hypothesis | Falsification test | Result |
|---|-----------|--------------------|--------|
| H1 | The loop binds once per candidate, but smithay supports one global bind | Stop after first success; warning must disappear and one `egl hardware-acceleration enabled` line remain | **Confirmed** |
| H2 | `card0` and `renderD129` are the same physical GPU; dedup by physical device fixes it | Compare minor numbers: 0 vs 129 → `card0` and `card1`'s render node are *different* GPUs. Dedup leaves two binds, so warning persists | **Falsified** |
| H3 | One EGL display is bound twice from the same GPU | smithay checks the global reader, not per-display state; warning crosses displays | **Falsified** |

The original bug report asserted H2. That assertion was wrong: `dev: 57856`
is minor 0 (`card0`), `dev: 57985` is minor 129 (`renderD129`, the render node
of `card1`). Two distinct physical GPUs.

### Phase 4 — Verify

- Read smithay 0.7.0 source directly: global lock at `display.rs:857`, warning
  condition at `:858`, overwrite at `:861`. One reader per process, not per GPU.
- Log shows first success on `card0` (`.918587`) and second on `renderD129`
  (`.918632`) with the warning between them (`.918627`).
- Candidate list has four distinct `dev_id`s (`.918544`), so id-based dedup
  cannot prevent a second bind.
- **Single verified root cause:** eswm attempts `bind_wl_display` more than once
  per process. Fix must cap bind attempts at one while preserving the
  `hw_accel_gpus` set used for cross-GPU dmabuf format advertising.

**Risk level:** Low. Local init path, no trust boundary, no data loss. Changing
the `hw_accel_gpus` membership would change dmabuf tranches, so the fix must
keep that set unchanged.

## Fix approach

Minimal: extract the "keep trying until one bind succeeds, then stop" decision
into a pure helper tested with an injected bind closure. In the real loop,
nodes whose renderer cannot be created stay excluded (as today); nodes after a
successful bind are admitted without another bind attempt. This preserves the
existing `hw_accel_gpus` membership exactly.

## TDD Fix Plan

1. **RED**: Test that bind attempts stop after the first success while all
   renderer-capable nodes remain in the usable set. Use a stub closure that
   records attempted nodes and fails the first one.
   **GREEN**: Add a pure generic helper implementing the stop-after-success
   rule with an injected binder.
   **verify**: `cargo test --bin eswm bind_wl_display_once`

2. **RED**: Test that when every bind fails, every node is attempted and none
   is admitted.
   **GREEN**: Already satisfied by the helper from cycle 1; keep as a guard.
   **verify**: `cargo test --bin eswm bind_wl_display_once`

3. **GREEN (integration)**: Replace the real loop's bind logic with the helper,
   logging the attempted nodes and one success.
   **verify**: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`

**REFACTOR**: No further cleanup expected; keep the pure helper near the other
DRM selection helpers.

## Acceptance criteria

- [ ] At most one `bind_wl_display` call succeeds per eswm process.
- [ ] Startup on the hybrid-GPU laptop logs no `Double bind_wl_display`.
- [ ] `hw_accel_gpus` contains every renderer-capable candidate. Nodes after
      the successful bind no longer need their own wl_drm bind, so they stay
      usable for the cross-GPU Linear tranche; scanout GPU still wins dmabuf
      selection.
- [ ] `cargo clippy --all-targets`, `cargo test --bin eswm`, `cargo build` green.
- [ ] Live DRM run still selects `renderD128` for scanout dmabuf and shows no
      `BAD_MATCH` / `EGLImage not supported` errors (operator UAT).

## Resolution

<!-- filled in by validate-fix -->
