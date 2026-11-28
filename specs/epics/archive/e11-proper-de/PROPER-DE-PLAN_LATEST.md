# Proper DE Parity Plan (cosmic-comp + Hyprland reference)

**Date:** 2026-11-28  
**Status:** Draft / planning reference  
**Scope:** Make eswm a “proper DE” compositor for Tacit: protocol completeness, screen recording/sharing, portal/session integration, Tacit integration gaps.

## Sources inspected

Source analysis only (no REA runtime tools; repos available locally):

- `cosmic-comp` (Rust/Smithay): `/home/yugaaank/Projects/cosmic-comp`
  - `src/systemd.rs`, `src/session.rs`, `src/dbus/mod.rs`
  - `src/state.rs`, `src/wayland/handlers/mod.rs`
  - `src/wayland/handlers/image_copy_capture/mod.rs`
- `Hyprland` (C++): `/home/yugaaank/Projects/Hyprland`
  - `src/Compositor.cpp`
  - `src/protocols/` inventory (`SessionLock`, `IdleInhibit`, `IdleNotify`, `ImageCopyCapture`, `Screencopy`, `ForeignToplevel*`, `PointerConstraints`, `RelativePointer`, `ShortcutsInhibit`, `FractionalScale`, `PresentationTime`, `OutputManagement`, `TextInputV3`, `InputMethodV2`, `Tablet`, `SecurityContext`, `TearingControl`, `CursorShape`, ...)
- System portal config: `/usr/share/xdg-desktop-portal/hyprland-portals.conf`
- eswm baseline: `src/main.rs`, `src/control.rs`, `notes/tacit-integration.md`, `notes/roadmap.md`, `specs/state.yaml`

## Observed implementation patterns

| Gap / feature | cosmic-comp (observed) | Hyprland (observed) | eswm implication |
|---|---|---|---|
| **Session env export (file picker/portal bug root cause)** | `dbus::ready()` updates D-Bus activation env with `WAYLAND_DISPLAY`, `DISPLAY` (`src/dbus/mod.rs:171-199`). `systemd::ready()` runs `systemctl --user import-environment WAYLAND_DISPLAY DISPLAY` and `sd_notify READY` when booted (`src/systemd.rs:9-35`). Also sends env to cosmic-session IPC (`src/session.rs:12-41`). | On start: `systemctl --user import-environment ...` + `dbus-update-activation-environment --systemd ...` for `DISPLAY WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP QT_QPA_PLATFORMTHEME PATH XDG_DATA_DIRS`; starts `hyprland-session.target` (`src/Compositor.cpp:798-819`). On stop: unsets/cleans env (`:540-570`). Guarded by session/`HYPRLAND_NO_SD_VARS`/`MANAGERPID`. | eswm currently only sets env for spawned children (`src/main.rs:1778-1780`). Add session bring-up: set `WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP=eswm`, `XDG_SESSION_TYPE=wayland`, `XDG_SESSION_DESKTOP=eswm`; export to systemd+dbus activation env; optionally sd_notify. This directly addresses file pickers opening in Hyprland. |
| **Portal wiring** | Compositor provides Wayland protocols; portal is separate (`xdg-desktop-portal-cosmic` ecosystem). | Compositor provides protocols; portal backend is separate (`xdg-desktop-portal-hyprland`). System conf observed: `hyprland-portals.conf` => `default=hyprland;gtk`. | eswm needs a portal strategy. Short term: ship `eswm-portals.conf` selecting compatible backends (gtk for FileChooser/settings; no ScreenCast claim until implemented). Long term: own portal backend or tight portal-wlr compatibility. |
| **Screen capture / recording / sharing** | Uses ext-image-copy-capture. `ImageCopyCaptureHandler` impl exposes output/workspace/toplevel sources, cursor session constraints, per-session damage tracking, shm+dmabuf constraints (`src/wayland/handlers/image_copy_capture/mod.rs`). Render path takes screencopy frames and sends results (`kms/surface/mod.rs` refs: `take_screencopy_frames`, `send_screencopy_result`). State holds `ImageCopyCaptureState`. | Has both `ImageCopyCapture.cpp` and `Screencopy.cpp`, plus `ToplevelExport.cpp` and foreign-toplevel protocols. PipeWire/ScreenCast portal plumbing typically lives in portal backend, not compositor core. | For proper DE sharing, eswm needs: (1) image-copy-capture/screencopy protocol(s), (2) DMA-BUF+SHM capture, (3) toplevel/output capture, (4) PipeWire producer, (5) portal ScreenCast integration. Implement (1)-(3) in compositor, (4)-(5) via portal backend integration. Prefer ext-image-copy-capture first; add wlr-screencopy compat if needed. |
| **Session lock** | `session_lock` handler + `SessionLockManagerState` and `SessionLockLayerState` (`src/state.rs`, `handlers/mod.rs`). | `SessionLock.cpp/hpp` protocol present. | Required for Tacit lock screen. Must block normal input/render as per protocol. |
| **Idle inhibit / idle notify** | `IdleInhibitManagerState`, `idle_inhibiting_surfaces` set, idle notify state (`src/state.rs`, handlers). | `IdleInhibit.cpp`, `IdleNotify.cpp` present. | Needed for video/media/fullscreen inhibit and idle timers. |
| **Fractional scale / presentation** | `FractionalScaleManagerState`, `PresentationState` (`src/state.rs`). | `FractionalScale.cpp`, `PresentationTime.cpp`. | Needed for HiDPI correctness and smooth video/frame timing. |
| **Pointer constraints / relative pointer / shortcuts inhibit** | `PointerConstraintsState`, `KeyboardShortcutsInhibitState` handlers present. | `PointerConstraints.cpp`, `RelativePointer.cpp`, `ShortcutsInhibit.cpp`. | Required for games, VMs, remote desktop, browsers grabbing keys. |
| **Foreign toplevel / task management** | Both `foreign_toplevel_list` and `toplevel_management` handlers + state. | `ForeignToplevel.cpp`, `ForeignToplevelWlr.cpp`, `ToplevelExport.cpp`, `ToplevelMapping.cpp`. | Tacit needs task/window list/activation. Add ext-foreign-toplevel or wlr-foreign-toplevel; expose activate/close/minimize/maximize/fullscreen where protocol allows. |
| **Output management/power** | `OutputManagerState::new_with_xdg_output` and output config infra; output power/config handlers exist in tree. | `OutputManagement.cpp`, `OutputPower.cpp`. | Needed for real display settings UI. |
| **Other DE protocols** | security_context, cursor_shape, text_input/input_method, tablet, tearing etc. present in cosmic ecosystem/smithay handlers. | Hyprland protocol dir includes `SecurityContext`, `TextInputV1/3`, `InputMethodV2`, `Tablet`, `TearingControl`, `CursorShape`, `ExtDataDevice`, `DataDeviceWlr`. | Prioritize by Tacit/client demand; don’t block on all at once. |

## Concrete plan for eswm

### Phase 0 — Immediate fix: file picker opens in Hyprland

**Goal:** FileChooser/portal calls stay in eswm session.

1. **Session env export at compositor startup** (new module, e.g. `src/session_env.rs`, called from DRM startup):
   - Set env in-process: `WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP=eswm`, `XDG_SESSION_TYPE=wayland`, `XDG_SESSION_DESKTOP=eswm`.
   - Export to systemd user: `systemctl --user import-environment ...` (guard: booted/systemd available, no-op on failure with warning).
   - Export to D-Bus activation env: `dbus-update-activation-environment --systemd ...` (or zbus/dbus call). Include `DISPLAY` if Xwayland later.
   - Optional: `sd_notify READY` once socket/protocols ready.
   - On clean shutdown, unset/reset carefully (Hyprland pattern; guard against clobbering user-changed env).
2. **Portal config**:
   - Add `resources/eswm-portals.conf` (install to `/usr/share/xdg-desktop-portal/eswm-portals.conf`).
   - Start conservative: `default=gtk` (FileChooser/OpenURI/Settings) unless/until eswm has screen-cast portal support. Do not claim ScreenCast until implemented.
3. **Docs/session files**:
   - Add session startup script/systemd user target pattern if packaging; document manual TTY launch env export.

**Acceptance:** from fresh eswm TTY session, GTK/Qt file picker opens on eswm, `systemctl --user show-environment` shows eswm vars, portal uses expected backend.

### Phase 1 — Tacit shell blockers (proper desktop session)

1. **`ext_session_lock_v1`** - lock surface rendering/input blocking; integrate with Tacit lock UI.
2. **`zwp_idle_inhibit_manager_v1`** + idle notify - fullscreen/video inhibit.
3. **`ext_foreign_toplevel_list_v1` / `zwlr_foreign_toplevel_handle_v1`** - pick based on Quickshell/Tacit needs; expose title/app_id/state/activate/close.
4. **Control/event parity** - add missing `config.reload`, `output.focus`, `layer.list`, keyboard layout state, urgency/fullscreen/maximized parity, Tacit-expected fields. Keep native eswm names; add adapter in Tacit backend if needed.
5. **Keyboard layout/input state events** - required by Tacit contract.

### Phase 2 — Screen recording & sharing

1. **Capture protocol in compositor**:
   - Implement `ext_image_copy_capture_v1` + output/toplevel capture sources (cosmic-comp pattern: constraints, cursor session, damage tracking, shm+dmabuf).
   - Add `wlr_screencopy` compatibility only if needed by existing tools.
2. **Damage-aware frame path**:
   - Reuse render buffers where possible; avoid full-copy every frame. Take screencopy frames in render pass and send result.
3. **PipeWire screencast**:
   - Add PipeWire producer path for selected output/window. Keep security boundary clear.
4. **xdg-desktop-portal ScreenCast**:
   - Integrate with `xdg-desktop-portal-wlr` if protocol-compatible, or build eswm portal backend. Update `eswm-portals.conf` to route ScreenCast/Screenshot appropriately.
5. **Screenshot/region picker**:
   - Compositor provides capture; Tacit provides picker UI.

**Acceptance:** capture tools or portal screencast works depending on chosen protocol; browser screen share works via portal; DMA-BUF path works on DRM.

### Phase 3 — Protocol completeness for “proper DE”

Implement in priority order, each with tests/protocol smoke:

1. `wp_fractional_scale_v1`, `wp_presentation_time`
2. `wp_pointer_constraints_v1`, `zwp_relative_pointer_manager_v1`
3. `zwp_keyboard_shortcuts_inhibit_manager_v1`
4. `ext_idle_notify_v1` (and keep idle-inhibit coherent)
5. `zwlr_output_management_v1` / output power / mode apply
6. `zwp_text_input_v3` + `zwp_input_method_v2` (IME)
7. `zwp_tablet_v2`
8. `wp_security_context_manager_v1` (sandboxed Flatpak/portal clients)
9. `wp_cursor_shape_v1`, `wp_tearing_control_v1` (optional)

### Phase 4 — Visual DE polish (after correctness)

- Per-window opacity/rules, animation engine (roadmap Phase 4), blur only after profiling, focus-steal policy, polished cursors/cursor themes, overview/thumbnails data.

## eswm-specific implementation notes

- **Follow smithay 0.7 patterns already used by eswm**; cosmic-comp is architecturally close but may track newer smithay APIs - port concepts, not code.
- **Do not copy code** from Hyprland/cosmic-comp; use them as behavioral references only (license/clean-room discipline in AGENTS).
- **Keep Tacit as shell owner**: compositor exposes protocols/state/actions; Tacit owns UI for pickers, overview, lock visuals, notifications.
- **Work the bigpowers way**: turn this into epics/stories in `specs/` (`scope-work`/`elaborate-spec`/`slice-tasks`/`plan-work`), then `develop-tdd`. Start with a small epic for Phase 0 because it fixes a real user-visible bug (file picker routing).
- **Risk**: DBus/systemd env export can interfere with existing user sessions if multiple compositors run under same user. Guard with clear startup mode, logs, and opt-out env (Hyprland’s `HYPRLAND_NO_SD_VARS` pattern is a good reference).

## Suggested first tickets

1. `session-env`: export/import WAYLAND_DISPLAY/XDG_CURRENT_DESKTOP/XDG_SESSION_TYPE to dbus+systemd; sd_notify; shutdown cleanup.
2. `portal-config`: ship/install `eswm-portals.conf` defaulting to gtk; document backend requirements.
3. `session-lock`: implement `ext_session_lock_v1`.
4. `foreign-toplevel`: implement ext/wlr foreign toplevel for Tacit task list.
5. `image-copy-capture`: output capture first, then toplevel capture, then portal ScreenCast path.
