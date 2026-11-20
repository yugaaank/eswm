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
