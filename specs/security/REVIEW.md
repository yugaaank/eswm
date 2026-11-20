# Security Review — e06s04 (clipboard and primary selection)

**Branch:** `e06s04-clipboard` (`b5a92c9..HEAD`)
**Date:** 2026-11-20
**Risk:** P2 (per story)

## Scope
- `src/main.rs`: add `PrimarySelectionState`/`DataControlState`, wire `DataDeviceHandler`, `PrimarySelectionHandler`, `DataControlHandler`, delegate macros, and set wl_data_device focus from keyboard focus changes.
- `src/udev.rs`: update primary selection focus on pointer focus changes.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.

## Notes
- Changes delegate clipboard/selection protocol behavior to smithay implementations and only adjust focus routing/state plumbing.
- No new external input parsing, privilege boundaries, authentication/authorization logic, secret handling, or network/file-system access introduced.
- Automated coverage is server-side/state-level; runtime client data-transfer behavior remains a documented verification gap (not a security finding).

**Verdict:** PASS (no unresolved HIGH findings). No exceptions required.
