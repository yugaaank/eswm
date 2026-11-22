---
bug_id: BUG-2026-11-22-DRM-RESTORE-PERMISSION
status: open
severity: low
priority: p3
scope: drm
title: DRM restore errors on graceful shutdown
---

# BUG-2026-11-22-DRM-RESTORE-PERMISSION — DRM restore errors on graceful shutdown

**Status:** open
**Discovered:** log analysis of `/tmp/opencode/eswm-latest-clean6.log`
(UTC 2026-11-22 08:01:26–08:02:00), shutdown lines.

**Security impact: NONE.** Teardown diagnostics only.

## Problem

A normal quit via keybind ends with two ERROR lines:

```
drm_device{device=Some("/dev/dri/card1")}:drm_atomic: Failed to restore previous
state. Error: Permission denied (os error 13)
drm_device{device=Some("/dev/dri/card0")}:drm_atomic: Failed to restore previous
state. Error: Permission denied (os error 13)
```

At startup both devices logged `Unable to become drm master, assuming
unprivileged mode`. Smithay's drop-time restore then tries to reapply the old
mode without the privileges needed to do so. Expected: a graceful shutdown
without ERROR-level noise, or an explicit downgrade to WARN when the session
never held DRM master.

## Root cause hypothesis

The teardown path does not track whether the compositor ever acquired DRM
master, so it attempts mode restore unconditionally even in unprivileged mode.

## Acceptance criteria

- [ ] Quit via keybind produces no ERROR lines for mode restore.
- [ ] If restore is genuinely impossible, the diagnostic is WARN with a clear
      reason, not ERROR.
- [ ] `cargo clippy --all-targets`, `cargo test --bin eswm`, `cargo build` green.
- [ ] Live DRM quit still releases libseat and returns to the previous TTY.
