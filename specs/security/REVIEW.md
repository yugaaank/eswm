# Security Review — current: e08s01 (panics/correctness)

**Branch:** `e08s01-panics` (`main..HEAD`)
**Date:** 2026-11-21
**Risk:** P2 (per story)

## Scope
- `src/udev.rs`: saturating dock segment/layout helpers; skip connectors with no modes; logged frame-skip on renderer acquisition failure; clean status-1 exit on GPU context loss (`DeviceAddError::Renderer` variant).
- `src/wm.rs`: clamp floating target to small work areas with saturating centering.
- `specs/*`: e08s01 story spec, tasks ledger, verification and audit evidence.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.
- No secrets, credentials, network access, auth checks, or new filesystem writes introduced. All changes are geometric arithmetic and error handling in the DRM/render path; no user-data or external-API surface touched (`security: none` on all tasks).

## Notes
- Failure modes changed intentionally: panic → logged skip (renderer failure, empty-mode connector) and panic → logged `exit(1)` (GPU context loss). This improves availability and does not weaken any trust boundary.
- Runtime/manual UAT on a live DRM session (tiny dock, 600x400 floating spawn, empty-mode connector, injected context loss) remains a documented verification gap, not a security finding.

---

# Security Review — e07s02 (keybinds)

**Branch:** `e07s02-keybinds` (`main..HEAD`)
**Date:** 2026-11-20
**Risk:** P3 (per story)

## Scope
- `src/lua_state.rs`: bind-time xkb keysym parsing/validation for `es.bind`/`es.bind_cmd` (`xkb::keysym_from_name`, case-insensitive), unknown key -> Lua error; `Keybind` stores parsed keysym.
- `src/udev.rs`: keybind matching via `raw_latin_sym_or_raw_current_sym()` with bind-relevant modifier comparison (Caps/Num Lock ignored); always-on built-in Super+Alt+Backspace quit and Ctrl+Alt+XF86Switch_VT_1..F12 VT switch with precedence; intercepted press/release key-code tracking.
- `specs/*`: planning, tasks, verification, and release bookkeeping for e07s02/e07.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.

## Notes
- Keybind configuration remains a trusted local config boundary; stricter bind-time validation rejects unknown key names early (Lua error) instead of silent runtime non-match.
- Built-in emergency quit/VT shortcuts are recovery controls (cannot be removed by config) and do not introduce new privilege boundaries or external surfaces.
- No secrets, credentials, network access, or filesystem writes introduced beyond existing compositor behavior.
- Runtime/manual UAT in live DRM session (real keymap/layout, VT switching behavior, client release delivery) remains a documented verification gap (P3), not a security finding.

**Verdict:** PASS (no unresolved HIGH findings). No exceptions required.
