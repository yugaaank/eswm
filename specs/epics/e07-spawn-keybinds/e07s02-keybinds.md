# Story e07s02: Keybinds: xkb keysym names, raw sym matching, emergency quit always on

**id:** e07s02  
**type:** fix  
**risk:** P3  
**context:** input  
**status:** completed  
**bcps:** 3  
**delta:** MODIFIED

## 1. Context

Keybind handling currently relies on a small hardcoded key-name map (`parse_key`) and stores bind keys as strings parsed at match time (`src/lua_state.rs`, `src/udev.rs`). Matching compares `keysym.modified_sym()` with exact equality over `logo/ctrl/alt/shift`, so shifted symbols (e.g. Super+Shift+1 arrives as `!`), Caps/Num Lock state, and non-listed keys (e.g. `r`) break binds. VT switch and quit are only available when configured as Lua keybind actions; a broken config can leave no emergency exit or VT switch. Keyboard shortcut handling only intercepts on press and always forwards releases.

## 2. Problem

- Only ~35 key names are recognized; common binds (e.g. Super+r) fail or cannot be expressed.
- Matching on `modified_sym()` breaks binds when shift/locks change the produced symbol (e.g. Super+Shift+1).
- Caps Lock and Num Lock break binds because modifier comparison is not normalized.
- Ctrl+Alt+F1..F12 and emergency quit depend on config and can be removed/broken.
- Intercepted key presses do not have corresponding releases intercepted, which can leak inconsistent key state to clients.

## 3. Goal

Make keybind parsing and matching layout/lock-robust using xkb keysym names and raw-sym matching, and guarantee always-on emergency quit and VT switching with correct press/release interception.

## 4. Non-goals

- Configurable emergency quit/VT chords beyond the fixed always-on guarantees.
- Remapping or removing the always-on built-in chords via Lua config.
- Changes to pointer behavior, workspace semantics, or unrelated key actions.

## 5. Users / Actors

- User configuring keybinds via Lua (`es.bind`, `es.bind_cmd`).
- User needing recovery from broken config (emergency quit / VT switch).
- Focused Wayland clients receiving keyboard events.

## 6. Preconditions

- e07s01 spawning/keybind plumbing is in place.
- `main` preflight green before kickoff (`cargo clippy --all-targets && cargo test --bin eswm && cargo build`).
- Planning decisions confirmed (2026-11-20): **1A, 2A, 3A, 4A, 5A** (all recommended).

## 7. Postconditions

- Key names are parsed/validated at bind time with xkb case-insensitive lookup; unknown names produce Lua errors from `es.bind`/`es.bind_cmd`.
- Keybind matching uses raw keysyms with Caps/Num Lock dropped from modifier comparison.
- Super+Alt+Backspace always quits (built-in, cannot be removed by config).
- Ctrl+Alt+F1..F12 always switches VT (built-in, cannot be removed by config).
- Any intercepted press has its corresponding release intercepted.

## 8. Scope

**In scope**
- Bind-time keysym parsing/storage in Lua keybind registration and `lua_state::Keybind`.
- Runtime matching changes in `src/udev.rs` (raw sym, normalized modifiers).
- Built-in always-on emergency quit and VT switch handling with precedence over config.
- Press/release interception tracking for consumed shortcuts.
- Inline unit tests in `src/*.rs` covering parsing, matching, always-on actions, and release interception.

**Out of scope**
- Journald logging, spawn behavior changes, layout/WM semantics, or new key actions.

## 9. Confirmed decisions

1. **Emergency quit (1A):** Hardcode **Super+Alt+Backspace** as a built-in quit, evaluated independent of Lua config, cannot be removed/overridden, highest priority. Update/replace `quit_keybind_matches_super_alt_backspace` to prove always-on built-in behavior while preserving the guarantee.
2. **VT switch (2A):** Detect **Ctrl+Alt + XF86Switch_VT_1..12** as built-in `change_vt(1..12)` on press regardless of config (cannot be removed); takes precedence over conflicting Lua binds. Accept xkb keysym names case-insensitively at bind time (canonical names resolved by xkb, e.g. `XF86Switch_VT_1`, `F1`, etc. as applicable); built-in VT handling must not depend on config.
3. **Parsing/matching (3A):** Parse/validate key names at bind time in `es.bind`/`es.bind_cmd` via `xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE)`; unknown/`KEY_NoSymbol` → Lua error. Store parsed keysym in `lua_state::Keybind` and remove event-time `parse_key` string map. Match using `keysym.raw_latin_sym_or_raw_current_sym()` against stored keysym; compare modifiers ignoring Caps/Num Lock (bind-relevant: `logo/ctrl/alt/shift` only).
4. **Release interception (4A):** Track intercepted presses by key code; on any consumed press (Lua bind or built-in quit/VT) intercept and record, and intercept corresponding release.
5. **Precedence (5A):** On press evaluate always-on built-ins (emergency quit, Ctrl+Alt+F1..F12) first → configured Lua binds → else forward. Intercept press+release for any match.

**Discovery references (verified):** xkbcommon 0.8.0 `xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE) -> Keysym`; smithay 0.7.0 `KeysymHandle::raw_latin_sym_or_raw_current_sym() -> Option<Keysym>`; `ModifiersState` has `caps_lock`/`num_lock`.

## 10. Requirements (delta tags)

### MODIFIED: Key names parsed at bind time with xkb keysym lookup
**Before:** Keybind key names are stored as strings and resolved at event time via a hardcoded `parse_key` map (~35 names); unknown/unmapped names fail to match silently at runtime.  
**After:** `es.bind` and `es.bind_cmd` parse key names at bind time with `xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE)` and store the parsed keysym; unknown/`KEY_NoSymbol` names return a Lua error.

### MODIFIED: Shortcut matching uses raw keysym and normalized modifiers
**Before:** Matching uses `keysym.modified_sym()` and exact equality of `logo/ctrl/alt/shift`, so shifted symbols and Caps/Num Lock break binds.  
**After:** Matching uses `keysym.raw_latin_sym_or_raw_current_sym()` compared to the stored bind keysym and compares only bind-relevant modifiers (`logo/ctrl/alt/shift`) with Caps/Num Lock ignored.

### ADDED: Always-on emergency quit (built-in)
**Before:** Quit requires a configured Lua keybind action (`quit`); a broken config can prevent quitting.  
**After:** **Super+Alt+Backspace** always triggers quit as a built-in action independent of Lua config and cannot be removed/overridden by config.

### ADDED: Always-on VT switch (built-in)
**Before:** VT switching requires configured Lua `change_vt` binds; a broken config can prevent VT switching.  
**After:** **Ctrl+Alt+F1..F12** (XF86Switch_VT_1..12) always trigger `session.change_vt(1..12)` as built-in actions independent of Lua config and cannot be removed/overridden by config.

### MODIFIED: Intercepted press/release symmetry
**Before:** Shortcut handling runs on press and may intercept; releases are always forwarded to clients.  
**After:** Any key press consumed by a built-in or Lua keybind is intercepted and its corresponding release is also intercepted (tracked by key code).

## 11. UX / API notes

- `es.bind("Super+r", ...)`, `es.bind("Super+Shift+1", ...)`, and other xkb-named keys should work when the name resolves via xkb.
- Invalid key names in config fail early with a Lua error at bind registration.
- Emergency quit and Ctrl+Alt+F1..F12 remain available even if config omits or conflicts with them.

## 12. Data / State

- `lua_state::Keybind` stores parsed keysym (and bind-relevant modifier/key metadata needed for matching).
- `UdevData` (or equivalent keyboard shortcut state) tracks key codes for presses that were intercepted so releases can be intercepted symmetrically.

## 13. Interfaces / Contracts

- Preserve existing keybind actions and Lua API shape (`es.bind`, `es.bind_cmd`) except stricter key-name validation and new always-on built-ins.
- Built-ins take precedence over configured Lua binds and cannot be removed by config.
- VT switching uses existing session VT change path (`session.change_vt(vt)`).

## 14. Dependencies

- `xkbcommon` 0.8 (`xkb::keysym_from_name`, `xkb::KEYSYM_CASE_INSENSITIVE`, `KEY_NoSymbol`).
- smithay 0.7 keyboard APIs (`KeysymHandle::raw_latin_sym_or_raw_current_sym`, `ModifiersState`).

## 15. Risks

- **xkb name compatibility:** bind-time validation may reject names previously accepted by the hardcoded map if spelled differently; prefer canonical xkb resolution and keep behavior clear via Lua errors.
- **Modifier normalization scope:** dropping only Caps/Num Lock must not break intentional Ctrl/Alt/Shift/Logo matching.
- **Release tracking correctness:** tracking must be per key code and cleaned up on release to avoid stuck-intercept state.
- **Built-in precedence:** always-on chords intentionally override conflicting config binds.

## 16. Test strategy

- Inline unit tests in `src/lua_state.rs` and/or `src/udev.rs` for: bind-time parsing success/failure (`KEYSYM_CASE_INSENSITIVE`, unknown -> error), raw-sym matching (Super+Shift+1 / letter keys), modifier normalization ignoring Caps/Num Lock, always-on emergency quit and VT switch precedence, and press/release interception tracking.
- Gates: targeted `cargo test --bin eswm <filter>` per task, plus `cargo build`, `cargo clippy --all-targets`, `cargo test --bin eswm`.

## 17. Acceptance Criteria

- [ ] `es.bind`/`es.bind_cmd` parse key names at bind time with `xkb::keysym_from_name(name, KEYSYM_CASE_INSENSITIVE)` and store parsed keysym.
- [ ] Unknown key names produce a Lua error from `es.bind`/`es.bind_cmd`.
- [ ] Keybind matching uses `raw_latin_sym_or_raw_current_sym()` and compares modifiers with Caps/Num Lock dropped.
- [ ] `Super+r`-style binds match when key name resolves via xkb.
- [ ] `Super+Shift+1` matches (raw `1` with shift modifier), not shifted symbol mismatch.
- [ ] Binds still match with Caps Lock (and Num Lock) active.
- [ ] **Super+Alt+Backspace** always triggers quit (built-in), independent of Lua config.
- [ ] **Ctrl+Alt+F1..F12** always trigger VT switch (built-in), independent of Lua config.
- [ ] Any intercepted press results in intercepted release for that key code.
- [ ] Emergency-quit guarantee remains verified (`quit_keybind_matches_super_alt_backspace` updated/passing per built-in semantics).
- [ ] `cargo build`, `cargo clippy --all-targets`, and `cargo test --bin eswm` pass.

## 18. Verification Script

1. Register `es.bind("Super+r", "spawn_terminal")` (or equivalent) in config and confirm Super+r triggers the action.
2. Register `es.bind("Super+Shift+1", "move_window_to_workspace", 1)` and confirm it fires on Super+Shift+1.
3. Enable Caps Lock and confirm configured binds (e.g. Super+r) still match.
4. With no quit bind in config (or conflicting config), press Super+Alt+Backspace and confirm compositor quits.
5. With no VT binds in config (or conflicting config), press Ctrl+Alt+F1..F12 and confirm VT switching occurs.
6. Trigger an intercepted keybind and confirm the key release is not forwarded to the focused client.

## 19. Implementation steps (plan)

1. **Bind-time xkb parsing & storage:** change `lua_state::Keybind` to store parsed keysym; parse/validate in `es.bind`/`es.bind_cmd` via `xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE)` and return Lua error on unknown/`KEY_NoSymbol`. → verify: `cargo test --bin eswm keybind_parse`
2. **Raw-sym matching & modifier normalization:** match using `keysym.raw_latin_sym_or_raw_current_sym()` against stored keysym and compare bind-relevant modifiers only (drop Caps/Num Lock); retire event-time `parse_key` string map. → verify: `cargo test --bin eswm keybind_match`
3. **Always-on built-ins (quit/VT) with precedence:** implement built-in Super+Alt+Backspace quit and Ctrl+Alt+XF86Switch_VT_1..12 VT switching before Lua bind lookup; config cannot remove/override. Update `quit_keybind_matches_super_alt_backspace` to preserve guarantee under built-in semantics. → verify: `cargo test --bin eswm emergency_quit`
4. **Intercept release for intercepted presses:** track consumed press key codes and intercept corresponding releases in keyboard input handling. → verify: `cargo test --bin eswm intercept_release`
5. **Full verification:** run preflight gates. → verify: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`

## 20. References

- `specs/epics/e07-spawn-keybinds/epic.yaml`
- `specs/epics/e07-spawn-keybinds/e07s02-keybinds.md`
- `src/lua_state.rs` (`es.bind`, `es.bind_cmd`, keybind parsing/storage)
- `src/udev.rs` (keyboard shortcut matching/dispatch, VT/quit handling, input event path)
- xkbcommon 0.8.0: `xkb::keysym_from_name`, `xkb::KEYSYM_CASE_INSENSITIVE`
- smithay 0.7.0: `KeysymHandle::raw_latin_sym_or_raw_current_sym`
- `CONVENTIONS.md` (inline tests in `src/*.rs`, preflight/always green)
