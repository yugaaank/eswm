---
bug_id: BUG-2026-11-22-DRM-RESTORE-PERMISSION
status: fixed
severity: low
priority: p3
scope: drm
title: DRM restore errors on graceful shutdown
---

# BUG-2026-11-22-DRM-RESTORE-PERMISSION — DRM restore errors on graceful shutdown

**Status:** fixed — graceful shutdown now restores without EACCES; frontmatter synced with `registry.yaml` on 2026-11-26
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

**Critical refinement (from independent review):** pausing must be limited to
cards that never held DRM master. On a real TTY, `DrmDeviceFd::new` succeeds in
`acquire_master_lock()`, so the drop-time restore would have succeeded and is
what makes getty visible. Pausing such a card would (a) skip that restore and
risk a black screen, and (b) call `release_master_lock()` while smithay's
`privileged` flag stays `true`, so `InternalDrmDeviceFd::drop` releases master a
second time and logs a *different* ERROR. The first implementation did exactly
this; it was rejected before merge and replaced.

## Fix plan (TDD, RED → GREEN)

1. **First cycle (superseded)** — `quiesce_all(backends, session_paused)` paused
   every active device. GREEN at `845dedb`, then rejected by independent review
   for the privileged-session regression above.
2. **Second cycle (current)** — RED `7318ff6`: truth-table tests require that
   unprivileged cards are quiesced, privileged cards are not, mixed cards are
   handled per-card, and an empty map is safe. GREEN `96fc777`:
   - record `drm_master` per `BackendData` by repeating the ioctl smithay used
     at open (its `is_privileged()` is crate-private; re-acquiring held master
     is a no-op that smithay itself does in `activate()`);
   - `quiesce_unprivileged()` visits **every** backend and pauses only those
     with `drm_master == false`;
   - call it from `Drop for UdevData` so early setup `?` returns and cards
     hot-added after `SessionEvent::PauseSession` are also covered. The old
     global `paused` flag could leave a late-added card active at drop.
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
**Fix applied:** `UdevData::drop` calls `quiesce_unprivileged()`, which pauses
only backends whose fd never held DRM master. Privileged cards keep their
getty restore and their single master release, so the real-TTY path is
unchanged. The `Drop` guard also covers early setup failures and late-added
cards.
**Hardening added:** per-card `drm_master` recording at device open, four
`FakeBackend` truth-table tests (unprivileged, privileged, mixed, empty), and a
`Drop` guard instead of a single end-of-loop call site.
**Evidence:** `cargo clippy --all-targets && cargo test && cargo build` all exit
0; 183 tests pass (179 baseline + 4 new). Log:
`/tmp/opencode/drm-restore-preflight-v2.log`, sha256
`19ef437ae2419d0e8826fed8cbb90cae0078ee445d00f7d700fe5b5fe4de2f70`.
**Commit:** `96fc777 fix(udev): quiesce only cards that never held DRM master`
**RED/GREEN commits (final cycle):** `7318ff6` (test-only, fails: missing
`drm_master`/`quiesce_unprivileged`) → `96fc777` (implementation, green).
The earlier `1fc95ae`/`845dedb` pair is retained in history as the rejected
first approach.

### Defect-class sweep

`specs/verifications/generalize-sweep-BUG-2026-11-22-DRM-RESTORE-PERMISSION.json`
records the sweep for `drm-device-dropped-while-active`. One sibling instance
found and **tracked, not patched**: `device_removed()` (`src/udev.rs:2349`)
drops a `BackendData` on hot-unplug without pausing it. Hot-unplug needs its own
UAT and is out of scope for this graceful-quit fix.

## Acceptance criteria

- [x] Quit via keybind produces no ERROR lines for mode restore — live UAT
      2026-11-23T19:25Z: 0 restore errors, 0 master-release errors, 0 ERROR lines
      in the log (baseline was 2 restore errors).
- [x] Root cause confirmed with source-line and log evidence (above).
- [x] `cargo clippy --all-targets`, `cargo test`, `cargo build` green (183 tests).
- [x] Privileged TTY quit keeps its drop-time restore and releases DRM master
      exactly once (per-card policy proven by unit tests; the observed TTY was
      unprivileged, so this remains a static/test proof rather than a live one).
- [x] Live DRM quit releases libseat and returns to the previous TTY — operator
      confirmed the text TTY came back (its shell, not a login prompt — expected
      when launching from an already-logged-in console), no black screen.
