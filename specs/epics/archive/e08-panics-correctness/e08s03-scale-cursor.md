# Story e08s03: Scale, cursors, input settings

**id:** e08s03
**type:** fix
**risk:** P2
**context:** correctness
**status:** completed  
**bcps:** 3
**delta:** MODIFIED

## 1. Context

eswm currently ignores client-supplied cursors (`CompositorHandler::cursor_image` is an empty no-op at `src/main.rs:847`), renders everything at the output's default scale 1.0 with no config path, hardcodes touchpad settings in the libinput device-added callback (`src/udev.rs:389-399`), and uses smithay's default XKB keymap. This story wires each through minimal, testable config hooks so Tacit's config can control them: client cursor surfaces render, per-output scale is settable via `es.dsp.set_output_scale(name, f32)`, touchpad settings and XKB keymap come from `es.dsp.set_*` hooks rather than being fixed in code.

## 2. Problem

- **Client cursors:** `cursor_image()` discards `CursorImageStatus`; only Xcursor-derived `xcursor::parser::Image` buffers are drawn. Clients that set a custom cursor surface (GTK4, wlroots apps) see the wrong pointer.
- **Output scale:** `Output` advertises `current_scale()` (default `Scale::Integer(1)`), and the renderer already uses `fractional_scale()` for the cursor, but no config call can change it — HiDPI outputs render at 1x.
- **Touchpad settings:** tap-to-click, tap-drag, DWT, two-finger scroll and accel speed are hardcoded in the libinput callback; there is no config surface.
- **XKB keymap:** the seat uses smithay's default keymap; a config that wants a specific layout/model has no hook.

## 3. Goal

Make the four input/display settings configurable and observable through pure helpers, with thin wiring into the existing render/config paths, so the compositor honors client cursors, per-output scale, and config-provided touchpad/keymap settings.

## 4. Non-goals

- Fractional-scale protocol (`wp_fractional_scale_v1`) and per-surface scaling — deferred (SCOPE out_of_scope).
- Redesigning the whole `es.config{}` shape (e09 design-decisions).
- Cursor themes beyond existing Xcursor image loading + client surfaces.
- Reworking damage tracking for live scale changes (risk noted in §15; initial scale is applied at output setup).

## 5. Users / Actors

- Operator using HiDPI outputs / custom keyboard layouts / touchpad preferences via `~/.config/eswm/config.lua`.
- Wayland clients that set custom cursor surfaces.

## 6. Preconditions

- `main` preflight green before kickoff.
- e08s01/e08s02 shipped (saturating helpers, band-order render assembly).

## 7. Postconditions

- A client-provided cursor surface is rendered at the pointer location with its hotspot; `Hidden` hides the cursor; `Named` falls back to existing Xcursor images.
- `es.dsp.set_output_scale("eDP-1", 2.0)` applies `Scale::Fractional(2.0)` to the named output via `Output::change_current_state`.
- `es.dsp.set_touchpad{ tap=bool, natural_scroll=bool, speed=f64 }` (or equivalent named setters) feeds the values used in the libinput device-added callback; existing defaults remain when unset.
- `es.dsp.set_keymap("<XKB keymap string>")` stores the keymap and the seat applies it via `KeyboardHandle::set_keymap_from_string`; invalid keymaps log an error and keep the current keymap.

## 8. Scope

**In scope**
- `src/lua_state.rs`: new `es.dsp` setters writing to a shared `InputConfig` struct (Arc<Mutex<_>>, same pattern as `DecorationConfig`), plus unit tests for parsing/validation.
- `src/main.rs`: `cursor_image()` stores `CursorImageStatus`; `InputConfig` added to `State`; keymap application hook.
- `src/udev.rs`: render client cursor surface band; read `InputConfig` for per-output scale at output setup and touchpad settings on device add.
- Pure helpers (scale mapping, cursor render decision, touchpad value resolution, keymap validity) with inline tests.

**Out of scope**
- Fractional scale protocol, per-surface scale, damage tracking redesign, XWayland cursors.

## 9. Zoom-out (module purpose / callers / contracts)

- **`src/lua_state.rs` (config API):** exposes `es.*` to Lua config; uses closure upvalues (Arc<Mutex<_>>) shared with the compositor. **Callers:** Lua config at startup; `register_api` called from `src/udev.rs` and tests. **Contract:** setters update shared config live; invalid input returns a Lua error or logs and keeps prior value.
- **`src/main.rs` (State / CompositorHandler):** owns seat, outputs, and cursor-image callback. **Callers:** Wayland clients (`cursor_image`), config API, udev render loop. **Contract:** `cursor_image()` records the latest client status; render loop consumes it.
- **`src/udev.rs` (DRM backend):** owns output setup and libinput device config. **Callers:** output hotplug, device-added events, frame timer. **Contract:** output scale is applied before first frame; touchpad settings use config with existing defaults as fallback.

## 10. Requirements (delta tags)

### MODIFIED: Client cursor surfaces are rendered
**Before:** `cursor_image()` is a no-op; only Xcursor-derived images render.  
**After:** `CursorImageStatus` is stored in `UdevData`; `Surface(surface)` renders the client's cursor surface at the pointer location with hotspot from `CursorImageSurfaceData`; `Hidden` hides; `Named` keeps the existing Xcursor image path.

### ADDED: Per-output scale config
**Before:** scale is the output default `Scale::Integer(1)`, unreachable from config.  
**After:** `es.dsp.set_output_scale(name, f32)` stores a scale per output name; `connector_connected` applies it via `Output::change_current_state(None, None, Some(scale), None)` before the first frame. Invalid names/scales are ignored with a log.

### ADDED: Touchpad settings from config
**Before:** tap-to-click, tap-drag, DWT, two-finger scroll, accel speed are hardcoded in the libinput callback.  
**After:** `es.dsp.set_touchpad*` setters store values; the device-added callback resolves settings from config with current hardcoded values as defaults. No libinput API change beyond reading shared config.

### ADDED: XKB keymap from config
**Before:** the seat uses smithay's default keymap.  
**After:** `es.dsp.set_keymap(string)` stores an XKB keymap; the compositor applies it via `KeyboardHandle::set_keymap_from_string`; invalid keymaps log an error and preserve the current keymap.

## 11. UX / API notes

- Config is Lua-only; no JSON path. All new setters are on `es.dsp`.
- Scale is applied at output setup; live scale changes (damage tracking) are out of scope.
- Cursor: clients that set a custom surface now get it; others keep Xcursor fallback.

## 12. Data / State

- New shared `InputConfig` struct holding `HashMap<String, f64>` (output scales), optional keymap string, touchpad settings (tap/natural_scroll/speed with defaults). Stored as `Arc<Mutex<InputConfig>>`, injected into `register_api` and held by `State`/`UdevData` as needed.
- `UdevData` gains a `cursor_image: CursorImageStatus` field.

## 13. Interfaces / Contracts

- Preserve all existing `es.dsp.*` setters; add new ones additively (no breaking config shape).
- `Scale` variants per smithay 0.7 `src/output.rs:174`: `Integer(i32)`, `Fractional(f64)`, `Custom{advertised_integer, fractional}`; `change_current_state(new_mode, new_transform, new_scale, new_location)` at `src/output.rs:377`.
- `CursorImageStatus::{Hidden, Named(CursorIcon), Surface(WlSurface)}` at smithay `src/input/pointer/cursor_image.rs`; `CursorImageSurfaceData` exposes `hotspot`.
- `KeyboardHandle::set_keymap_from_string(&self, data: &mut D, keymap: String) -> Result<(), Error>` at smithay `src/input/keyboard/mod.rs:810`.
- Touchpad setters already in use: `config_tap_set_enabled`, `config_tap_set_drag_enabled`, `config_dwt_set_enabled`, `config_scroll_set_method`, `config_accel_set_speed` (src/udev.rs:389-399).

## 14. Dependencies

- Existing crates only: smithay 0.7, xkbcommon, mlua, libinput (via smithay reexports). **Slopcheck:** N/A (no new dependencies).

## 15. Risks

- **Scale changes need damage tracking:** live changes can leave stale damage; mitigated by applying scale only at output setup (documented limitation).
- **Client cursor surface lifetime:** surface must be kept alive while used; dropping must clear status (handled by smithay's surface-alive checks; test via helper on status).
- **Invalid keymap/scale:** log and keep previous value; no crash.
- **Large diff:** 4 concerns; mitigate by pure helpers and thin wiring, one task each.

## 16. Out of scope

- Fractional-scale protocol, per-surface scale, damage-tracking redesign, XWayland cursors, full `es.config{}` redesign.

## 17. Acceptance / Verification

- `cargo test --bin eswm cursor_image` passes (status → render decision: Hidden/Missing/Named/Surface).
- `cargo test --bin eswm output_scale` passes (name+f32 → Scale; invalid → None).
- `cargo test --bin eswm touchpad_config` passes (config values resolve with defaults).
- `cargo test --bin eswm keymap_config` passes (keymap stored; empty/invalid rejected).
- `cargo clippy --all-targets && cargo test --bin eswm && cargo build` passes.
- Manual: client cursor renders; HiDPI output scale respected; config keymap/touchpad applied.

## Implementation Steps

1. Define `InputConfig` + pure helpers (`output_scale_for`, `resolve_touchpad`, `validate_keymap`, `cursor_render_decision`) with inline tests (RED → GREEN). → verify: `cargo test --bin eswm output_scale`
2. Extend `register_api` with `es.dsp.set_output_scale`, `set_touchpad_*`, `set_keymap` writing into shared `InputConfig`; add Lua-facing parse tests. → verify: `cargo test --bin eswm keymap_config`
3. Apply per-output scale in `connector_connected` from `InputConfig` via `Output::change_current_state`. → verify: `cargo build`
4. Store `CursorImageStatus` in `UdevData` from `cursor_image()` and render `Surface` cursors in the cursor band using hotspot from `CursorImageSurfaceData`. → verify: `cargo test --bin eswm cursor_image`
5. Read touchpad settings from `InputConfig` in the libinput device-added callback, falling back to existing defaults. → verify: `cargo test --bin eswm touchpad_config`
6. Apply configured keymap via `KeyboardHandle::set_keymap_from_string` at startup (and on config reload if wired); log error on invalid input. → verify: `cargo build`
7. Run full preflight for the story. → verify: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm output_scale` — scale mapping correct, invalid input rejected.
2. Run `cargo test --bin eswm cursor_image` — hidden/surface/named decisions correct.
3. Run `cargo test --bin eswm touchpad_config` — config values resolved with defaults.
4. Run `cargo test --bin eswm keymap_config` — keymap storage/validation.
5. Run `cargo clippy --all-targets && cargo test --bin eswm && cargo build` — green.
6. Live DRM: a GTK4 app's custom cursor renders; `set_output_scale("eDP-1", 2.0)` makes output HiDPI; config keymap/touchpad settings take effect.

## Risks

- see §15
