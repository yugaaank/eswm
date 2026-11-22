---
bug_id: BUG-2026-11-22-DRM-RESTORE-PERMISSION
status: open
severity: low
priority: p3
scope: drm
title: DRM restore errors on graceful shutdown
---

# BUG-2026-11-22-DRM-RESTORE-PERMISSION — DRM restore errors on graceful shutdown

**Status:** open → investigating (RCA complete, TDD plan below)
**Discovered:** log analysis of `/tmp/opencode/eswm-latest-clean6.log`
(UTC 2026-11-22 08:01:26–08:02:00), shutdown lines.
Confirmed again in the live AB24 run at 2026-11-22T10:50:04Z.

**Security impact: NONE.** Teardown diagnostics only. No data, auth, or
privilege surface: the failing ioctl is a no-op restore during process exit.

## Problem

A normal quit via keybind ends with two ERROR lines:

```
drm_atomic: Failed to restore previous state. Error: Permission denied (os error 13)
INFO Dropping device: Some("/dev/dri/card1")
drm_atomic: Failed to restore previous state. Error: Permission denied (os error 13)
INFO Dropping device: Some("/dev/dri/card0")
```

At startup both devices logged `Unable to become drm master, assuming
unprivileged mode`. Expected: a graceful shutdown without ERROR-level noise.

## Root cause analysis (4 phases)

### Phase 1 — Reproduce

Deterministic on every graceful quit in this environment. Live log sequence:

```
10:49:54Z WARN  Unable to become drm master, assuming unprivileged mode   (card1)
10:49:56Z WARN  Unable to become drm master, assuming unprivileged mode   (card0)
10:50:04Z ERROR Failed to restore previous state. Permission denied (13)
10:50:04Z INFO  Dropping device: /dev/dri/card1
10:50:04Z ERROR Failed to restore previous state. Permission denied (13)
10:50:04Z INFO  Dropping device: /dev/dri/card0
```

One ERROR per card, immediately before that card's fd is dropped. No visual or
functional regression was observed; the session ends normally.

### Phase 2 — Isolate

Read the exact smithay 0.7.0 source paths:

- `backend/drm/device/atomic.rs:240` — `Drop for AtomicDrmDevice`:
  `if self.active.load(SeqCst) { ... atomic_commit(ALLOW_MODESET, req) }`.
  On failure it emits `error!("Failed to restore previous state. Error: {}")`
  at line 273. **The restore is conditional on the device still being active.**
- `backend/drm/device/fd.rs:77-80` — `DrmDeviceFd::new`: `acquire_master_lock()`
  fails → `privileged` stays `false`, warn once, proceed unprivileged.
- `backend/drm/device/mod.rs:418-426` — `DrmDevice::pause()`:
  `self.set_active(false)` then releases master only `if is_privileged()`.
  `DrmOutputManager::pause()` (`output.rs:115`) delegates straight to it.
- `backend/drm/device/mod.rs:485-490` — `set_active()` swaps the *same*
  `Arc<AtomicBool>` that `AtomicDrmDevice::drop` reads.

eswm's path: `run_udev()` (`src/udev.rs:785`) breaks out of the loop on the
quit flag and returns `Ok(())` at line 850. `state` then drops, dropping
`UdevData.backends` → `BackendData.drm_output_manager` → `DrmDevice` →
`AtomicDrmDevice::drop`. eswm pauses devices **only** in the libseat
`SessionEvent::PauseSession` handler (`src/udev.rs:437`), never on quit.

### Phase 3 — Hypotheses (ranked)

| # | Hypothesis | Falsification test | Result |
|---|------------|--------------------|--------|
| H1 | Device is dropped while `active == true`; drop-time restore needs master, which the session never acquired → EACCES. | Check the drop guard and the startup master warning. | **CONFIRMED.** `atomic.rs:240` guard + `fd.rs:77-80` warning + both ERROR lines immediately before `Dropping device`. |
| H2 | smithay incorrectly believes it is master; the ioctl would succeed if retried. | Read `is_privileged()` set site. | Falsified. `privileged` is set only on successful `acquire_master_lock()`; startup warnings prove it is `false`. |
| H3 | libseat closes the device fd before restore, making the fd invalid. | Check drop ordering: `AtomicDrmDevice::drop` runs on the inner device before `DrmDeviceFd`/`DeviceFd` are released. | Falsified. Error is `EACCES`, not `EBADF`, and the INFO `Dropping device` line follows the ERROR. |

### Phase 4 — Verify / single root cause

**Confirmed root cause:** eswm quits without quiescing its DRM devices. The
devices are still marked active when they are dropped, so smithay attempts the
drop-time mode restore. The session is unprivileged (master was never
acquired), so that ioctl returns EACCES and is logged at ERROR level once per
card. This is purely a teardown-ordering defect — the restore is impossible in
this session by construction, and smithay already provides the API to skip it.

The upstream-intended skip is `DrmDevice::pause()`: its own documentation says
it "will cause the `DrmDevice` to avoid making calls to the file descriptor
e.g. on drop". Calling it before teardown clears the `active` flag that guards
the restore, so drop stays silent. No smithay patch or log-level override is
needed.

**Guard:** eswm sets `UdevData.paused = true` and pauses every device in
`SessionEvent::PauseSession`. If quit happens in that state, pausing again
would call `release_master_lock()` a second time on a privileged session and
produce a *different* ERROR. The quit teardown is therefore conditional on
`!paused`.

## Fix plan (TDD, RED → GREEN)

1. **RED** — add a `Quiesce` seam (implemented by `BackendData` as
   `drm_output_manager.pause()`) plus a generic `quiesce_all` helper with a
   `session_paused` policy flag. Add tests with a `FakeBackend`:
   - `quit_quiesces_every_drm_backend` — two fake cards both quiesced (maps
     directly to the two ERROR lines).
   - `paused_session_does_not_quiesce_twice` — no second quiesce when VT
     pause already happened.
   - `no_backends_quiesces_nothing` — empty map is safe.
   Run `cargo test --bin eswm` → RED (helper does not exist yet).
2. **GREEN** — implement `Quiesce`/`quiesce_all`; call it in `run_udev()`
   immediately after the event loop, before `state` drops. Run tests → GREEN.
3. **Validate** — `cargo clippy --all-targets && cargo test && cargo build`;
   defect-class sweep; security review; live DRM quit log must contain zero
   `Failed to restore previous state` lines (operator-gated: eswm cannot be
   started inside this Wayland session because of the VT grab, and the operator
   cannot switch TTY while it runs).

## Resolution

**Fixed:** 2026-11-22 (branch `fix-drm-restore-permission`)
**Root cause confirmed:** eswm quit with DRM devices still marked active, so
smithay's drop-time restore attempted an ioctl the unprivileged session could
not perform.
**Fix applied:** `quiesce_all()` calls `DrmOutputManager::pause()` on every
backend immediately after the event loop and before `state` drops. Pausing
clears the `active` flag that guards smithay's restore, so drop stays silent.
**Hardening added:** `session_paused` policy guard prevents a second pause when
`SessionEvent::PauseSession` already released DRM master; three unit tests with
a `FakeBackend` cover two cards, the already-paused case, and the empty map.
**Evidence:** `cargo clippy --all-targets && cargo test && cargo build` all exit
0; 182 tests pass (179 baseline + 3 new). Log:
`/tmp/opencode/drm-restore-preflight.log`, sha256
`e1ec475b0fbc532fc86073cdd6f9419eb0c012d5975a941f2d96a931c4980d16`.
**Commit:** `845dedb fix(udev): quiesce DRM devices before quit teardown`
**RED/GREEN commits:** `1fc95ae` (test-only, fails: `Quiesce`/`quiesce_all`
undefined) → `845dedb` (implementation, green).

### Defect-class sweep

`specs/verifications/generalize-sweep-BUG-2026-11-22-DRM-RESTORE-PERMISSION.json`
records the sweep for `drm-device-dropped-while-active`. One sibling instance
found and **tracked, not patched**: `device_removed()` (`src/udev.rs:2349`)
drops a `BackendData` on hot-unplug without pausing it. Hot-unplug needs its own
UAT and is out of scope for this graceful-quit fix.

## Acceptance criteria

- [ ] Quit via keybind produces no ERROR lines for mode restore — **automated
      proof green; live quit pending operator** (see verification record).
- [x] Root cause confirmed with source-line and log evidence (above).
- [x] `cargo clippy --all-targets`, `cargo test`, `cargo build` green.
- [ ] Live DRM quit still releases libseat and returns to the previous TTY
      (operator-gated: eswm cannot start inside this Wayland session because of
      the VT grab, and the operator cannot switch TTY while it runs).
