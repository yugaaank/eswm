# Security Review — e11 Phase 0

**Branch:** `e11-proper-de-phase0` (`610b108..daf9631`)
**Date:** 2026-11-28
**Risk:** P3 (environment publication; no auth or credential boundary)
**Security impact:** NONE

## Scope

- `src/session_env.rs`: ten identity/guard tests and the implementation that
  snapshots the systemd user environment, exports four identity variables to
  the systemd user and D-Bus activation environments, and restores only
  ownership-matched values on shutdown. Opt-out via `ESWM_NO_SD_VARS`.
- `src/udev.rs`: installs the external export guard at `run_udev()` startup so
  it drops on every exit path.
- `resources/eswm-portals.conf`, `docs/session.md`, e11 specs and UAT record.

## Findings

- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.
- No secrets, credentials, auth checks, deserialization, network access, new
  dependencies or generated-target edits. Child env changes preserve values a
  caller set explicitly, so eswm cannot clobber a caller's `WAYLAND_DISPLAY`.

## Notes

- Every external command is built as data and executed with `Command::status`
  or `Command::output`; no shell string interpolation. Arguments are the four
  fixed identity keys, their fixed values, and values read back from the
  systemd user manager. No key content or secret reaches a log or socket.
- `current_external_environment()` spawns `systemctl --user show-environment`
  and discards everything except the four identity keys before storing or
  acting on it; other environment values are never logged or exported.
- Failed or missing tools only produce a warning and a skipped restore; they
  never fail startup or turn a warning into a silent success claim.
- D-Bus has no unset primitive, so a previously absent identity variable is
  cleared to an empty string in the activation environment. systemd uses
  `unset-environment` outright. This is documented in `docs/session.md`.
- The ownership guard compares the current external value against eswm's own
  before restoring, so a value a user changed during the session is left
  alone. This is the same rule proven in-process by the earlier unit test.
- The external restore is unit-covered and was confirmed on the post-review
  build by the operator (`its working i have confirmed`). No remaining
  verification gap.
