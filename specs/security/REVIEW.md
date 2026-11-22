# Security Review — current: BUG-2026-11-22-XDG-ACTIVATION-GLOBAL

**Branch:** `fix-xdg-activation-global` (`main..HEAD`)
**Date:** 2026-11-22
**Risk:** P3 (per bug registry)
**Security impact:** LOW — adds a wayland protocol surface, guarded against token replay.

## Scope
- `src/main.rs`: `XdgActivationState` created in `new_state`, stored on `State`,
  `delegate_xdg_activation!(State)`, and `XdgActivationHandler` with a 10-second
  freshness check and single-use token removal.
- `specs/bugs/*`, `specs/state.yaml`, `specs/verifications/*`: RCA, resolution,
  sweep evidence, cycle tracking.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM:** none.
- **LOW:** activation is a focus-changing capability. A client can request focus
  for a surface it knows; this is the protocol's intended behavior. Mitigated by
  rejecting tokens older than 10 s, removing tokens after use, and requiring the
  surface to map to an existing tracked toplevel. No new network, credentials,
  filesystem writes, or data exposure.

## Notes
- The defect class is a missing protocol global, not a trust-boundary bypass.
- Live launcher UAT (no more `Unable to mint xdg activation token` in
  `spawn.log`) remains operator-only; it is a correctness gate, not a security
  finding.

**Verdict:** PASS (no unresolved HIGH findings). No exceptions required.

---

# Security Review — BUG-2026-11-22-DOUBLE-BIND-WL-DISPLAY

**Branch:** `fix-double-bind-wl-display` (`main..HEAD`)
**Date:** 2026-11-22
**Risk:** P2 (per bug registry)
**Security impact:** NONE

## Scope
- `src/udev.rs`: new pure `bind_wl_display_once` helper; DRM setup loop now
  attempts the process-global wl_drm bind at most once.
- `specs/bugs/*`, `specs/state.yaml`, `specs/verifications/*`: RCA correction,
  resolution, sweep evidence, cycle tracking.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.
- The change touches local GPU initialization only. No untrusted input, no
  network surface, no credentials, no new filesystem writes, and no protocol
  exposure changes. The wl_drm global is still advertised exactly as before.

## Notes
- The previous bug hypothesis (physical-GPU dedup) was falsified by minor
  numbers; the verified root cause is a process-global EGL reader. No security
  consequence either way.
- Live DRM startup UAT remains operator-only; it is a correctness gate, not a
  security finding.

**Verdict:** PASS (no unresolved HIGH findings). No exceptions required.

---

# Security Review — e08s04 (window state flags)

**Branch:** `e08s04-state-flags` (`main..HEAD`)
**Date:** 2026-11-22
**Risk:** P2 (per story)

## Scope
- `src/main.rs`: pure `xdg_state_flags_for_mode`; `apply_layout` sends Maximized/Fullscreen xdg state bits and tracks `mode_sent`.
- `src/wm.rs`: `Window.mode_sent` field + setter.
- `specs/*`: e08s04 re-plan, tasks, verification (operator UAT override), audit evidence.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.
- xdg state bits are standard protocol output; no auth, network, credentials, user data, or new filesystem writes introduced (`security: none` on all tasks).

## Notes
- Manual DRM UAT was waived by explicit operator decision (automated proxy accepted); recorded in verification evidence, not a security finding.
- No viewport override exists or was introduced (verified by source check).

---

# Security Review — e08s03 (scale, cursors, input settings)

**Branch:** `e08s03-scale-cursor` (`main..HEAD`)
**Date:** 2026-11-22
**Risk:** P2 (per story)

## Scope
- `src/lua_state.rs`: `InputConfig` + `es.dsp.set_output_scale/set_touchpad_*/set_keymap` setters with validation.
- `src/main.rs`: `State.input_config`; `cursor_image()` stores client cursor status.
- `src/udev.rs`: per-output scale application, client cursor surface rendering, touchpad settings from config, XKB keymap via `set_keymap_from_string`.
- `specs/*`: e08s03 story, tasks, verification, audit evidence.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.
- Config values are local Lua (trusted boundary). Scale values are validated (finite, > 0) before constructing `Scale::Fractional`; keymap and touchpad speed are validated before mutation; invalid input raises a Lua error or is logged. No network, auth, credentials, or new filesystem writes introduced (`security: none` on all tasks).

## Notes
- Client cursor surfaces are untrusted Wayland surfaces, but they are rendered through smithay's existing surface-tree path (same as windows/popups) with no new privilege boundary.
- Runtime/manual UAT on a live DRM session (client cursor, HiDPI scale, config keymap/touchpad) remains a documented verification gap, not a security finding.

---

# Security Review — e08s02 (close, z-order, CSD geometry)

**Branch:** `e08s02-correctness` (`main..HEAD`)
**Date:** 2026-11-21
**Risk:** P2 (per story)

## Scope
- `src/udev.rs`: titlebar close policy + `send_close()`; z-order band assembly (fullscreen above layers/dock, overlay above dock).
- `src/main.rs`: read `SurfaceCachedState.geometry` on toplevel commit into `wm::Window.xdg_offset`.
- `src/wm.rs`: `set_window_xdg_offset` setter.
- `specs/*`: e08s02 story spec, tasks, verification, audit evidence.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.
- No secrets, credentials, auth checks, network access, or new filesystem writes introduced. Changes are UI stacking, cooperative close, and geometry bookkeeping; no user-data or external-API surface (`security: none` on all tasks).

## Notes
- Close button now asks the client via `xdg_toplevel.close` instead of forcibly removing the WM window; this is a cooperative behavior change, not a trust-boundary change.
- Fullscreen z-order (above all layers and dock) is an operator-confirmed visual policy; overlay lock screens therefore render below a fullscreen window. No security implications.
- Runtime/manual UAT on live DRM session (close button, stacking, CSD positioning) remains a documented verification gap, not a security finding.

---

# Security Review — e08s01 (panics/correctness)

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
