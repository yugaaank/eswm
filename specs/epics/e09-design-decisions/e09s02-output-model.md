# Story e09s02: Output model — position, scale, work area, LayerMap per output; focused output

**id:** e09s02
**type:** refactor
**risk:** P1 (revised from P3 after `specs/IMPACT_LATEST.md`: High blast radius across shared WM/IPC/Lua interfaces)
**context:** output model
**status:** completed
**bcps:** 2
**delta:** MODIFIED

## 1. Context

Today eswm has two competing "primary" outputs: `wm.primary_output_id()` is the
first output (placement, popup constraint, workspaces, dock entries, hit tests)
and `state.primary_output` is the **last connected** output (layer maps, dock
geometry). Every output sits at `(0,0)`, `wm.work_area` is one global rect,
`wm.dock_height()` is global, pointer bounds add output widths together, and
`WlrLayerShellHandler::new_layer_surface` ignores the requested `wl_output`.
Plugging HDMI into a laptop therefore puts bars on one screen and windows on
another (story source; code map: `specs/IMPACT_LATEST.md`).

Operator decisions on 2026-11-26:

1. Full implementation in code (same as e09s01), not an ADR-only exercise.
2. Position comes from `es.dsp.set_output_position(name, x, y)`; outputs with no
   configured position auto-place side-by-side by connect order. A single output
   keeps `(0,0)` exactly.
3. One focused output: output under the pointer, falling back to the focused
   window's output when the pointer is outside any output.
4. Include per-output dock height and `dock_entries(output)` (roadmap §2.2), so
   the second screen gets a dock too.

## 2. Problem

- Two unrelated definitions of "primary" disagree as soon as two outputs exist.
- Position is always `(0,0)` in both smithay (`udev.rs:2679`) and the WM
  (`udev.rs:2686`); pointer bounds sum widths (`udev.rs:2426`) and absolute
  motion explicitly assumes one stacked output (`udev.rs:2013`).
- One global work area and one global dock height cannot describe two screens;
  layer-surface exclusive zones are output-local in smithay but eswm copies the
  zone into global space without adding the output origin (`main.rs:663-670`).
- `state.primary_output` is never cleared on disconnect (`udev.rs:2759-2770`), so
  layer maps and dock geometry can point at a dead output.
- `_output: Option<WlOutput>` is discarded in `new_layer_surface` (`main.rs:566`).

## 3. Goal

One output model shared by placement, layers, dock, pointer and IPC: each output
has position, scale, work area, dock height and a smithay `LayerMap`; exactly one
focused output is derived from pointer position with a window-focus fallback;
all consumers use it instead of "first" or "last connected".

## 4. Non-goals

- Output hotplug robustness beyond keeping the model consistent on connect/
  disconnect (existing Phase 1 work; no new DRM recovery machinery).
- Session lock, screencopy, output-management protocol, cross-output window drag.
- IPC socket security / subscribe semantics (e09s03) and declarative
  `es.config{}` / typed action catalog (e09s04).
- Persisting positions across restarts — config file is the persistence layer.

## 5. Users / Actors

- Operator with a laptop + HDMI monitor, running Tacit's bars and dock.
- Tacit backend consuming `output.list` / Hyprland-compatible `monitors` JSON.
- Lua config author setting per-output position, scale and dock height.

## 6. Preconditions

- `main` preflight green at `1eb6327` (rounded-corner merge + reconciliation).
- e09s01 landed: `ClientMode`, single global window focus with MRU.
- ADR-0002 still holds: DRM/libseat only; virtual output exists for tests only.

## 7. Postconditions

- `wm::Output` carries `work_area: Option<Rect>` and `dock_height: u32`;
  `WindowManager` carries `pointer_output: Option<OutputId>` and exposes
  `focused_output_id()` = pointer output → focused window's output → first
  output. `primary_output_id()` is gone from runtime code.
- `State` holds `layer_outputs: BTreeMap<OutputId, smithay::output::Output>`;
  every layer surface is tagged with its target `OutputId` and uses that
  output's `layer_map_for_output`.
- Each output's work area = its own non-exclusive zone + its own origin − its
  own dock height; each output arranges with its own work area.
- `es.dsp.set_output_position(name,x,y)` and
  `es.dsp.set_output_dock_height(name,n)` exist; auto-position is the fallback.
- Pointer absolute clamp and `clamp_to_output` use the union of output rects;
  popups constrain against their output's work area.
- `dock_entries(output)` / dock render / dock click are per output; Lua
  `es.dock.*` follows the focused output.
- `output.list` keeps the `primary` key (deprecated alias of `focused`) and adds
  `focused`; Hyprland `monitors` emits one entry per output.

## 8. Scope

**In scope:** `src/wm.rs`, `src/main.rs`, `src/udev.rs`, `src/control.rs`,
`src/ipc.rs`, `src/lua_state.rs`, inline tests, verification record.

**Out of scope:** see §4.

## 9. Zoom-out (module purpose / callers / contracts)

- **`src/wm.rs` WindowManager** — pure WM model and tiler. Callers: `main.rs`,
  `udev.rs`, `control.rs`, `ipc.rs`, Lua via shared config. Contracts:
  `arrange_output` owns every tiled rect; e10s01/e10s02 geometry stays
  byte-identical for a single output at `(0,0)`; focus remains unique.
- **`src/main.rs` protocol glue** — maps layers, derives work areas, sends
  configures. Callers: smithay delegates + udev loop. Contract: layer commit →
  map/configure → work-area update → configure; keyboard focus policy unchanged.
- **`src/udev.rs` DRM backend** — connect/disconnect, input bounds, render.
  Callers: `main.rs`. Contract: one `SurfaceData` per CRTC, DRM/libseat only.
- **`src/control.rs` / `src/ipc.rs` snapshots** — wire shapes. Contract:
  additive changes only; `primary`, `id`, `name`, `geometry` keys stay.
- **`src/lua_state.rs`** — Lua-only config. Contract: invalid values raise Lua
  errors and leave live config untouched; names keyed by connector name.
- **Reason for depth:** smithay already keeps a `LayerMap` inside each
  `Output.user_data()`, so storing a second map in `wm::Output` would duplicate
  state and break smithay's lock discipline; a `State`-side id→Output handle map
  is the minimum needed to reach the right map from WM code.

## 10. Requirements (delta tags)

### MODIFIED: One notion of focused output replaces two "primary" definitions
**Before:** `wm.primary_output_id()` = first output; `state.primary_output` = last
connected output; placement and layers disagree on multi-output setups.
**After:** `wm.focused_output_id()` = pointer output, else focused window's
output, else first output; all runtime consumers use it; IPC `primary` remains
as a deprecated alias of `focused` for Tacit compatibility.

### MODIFIED: Position and scale live on each output
**Before:** smithay and WM locations are hardcoded `(0,0)`; scale only exists in
Lua config and smithay `Output.current_scale`.
**After:** configured position from `es.dsp.set_output_position(name,x,y)`, else
auto side-by-side placement; both smithay location and `wm::Output.geometry` use
it; scale stays sourced from the Lua map and is mirrored onto each output.

### MODIFIED: Work area and dock are per output
**Before:** one global `wm.work_area` from the last connected output's layer
zone, one global `wm.dock_height`, `dock_entries()` from the first output.
**After:** `Output.work_area` from that output's own zone translated by its
origin minus `Output.dock_height`; `dock_entries(output_id)`, per-output dock
render/click; Lua `es.dock.*` follows the focused output.

### MODIFIED: Layer surfaces target the output the client asked for
**Before:** `new_layer_surface` discards `Option<WlOutput>`; all wrappers are
mapped into the last connected output's map.
**After:** `Output::from_resource` resolves the requested output; wrappers are
tagged with an `OutputId` via `LayerSurface::user_data()`; map/unmap/hit-test/
render use that output's map. Untargeted layers fall back to focused output.

### MODIFIED: Pointer bounds respect displaced outputs
**Before:** `max_screen_size()` sums widths; absolute motion clamps to
`(0,0)..(sum_widths, max_height)`.
**After:** bounds are the union of `Output.geometry` rects; clamp uses min/max
edges, so a second output at `x=1920` is reachable and the gap is not clipped.

### ADDED: Per-output configuration API
**Before:** only `es.dsp.set_output_scale(name, value)` and global dock height.
**After:** `es.dsp.set_output_position(name,x,y)` and
`es.dsp.set_output_dock_height(name,n)`; global `set_dock_height(n)` remains as
an all-outputs compatibility setter. Reason for depth: two small maps in
`DecorationConfig` avoid a new config type while keeping validation in one place.

### ADDED: Multi-output IPC snapshot
**Before:** Hyprland `monitors` reports one hardcoded `winit` monitor at `(0,0)`.
**After:** one entry per output with its position, scale and focused flag; the
single-output fixture output still yields the same first entry shape.

## 11. UX / API notes

- Lua names: `set_output_position(name, x, y)` accepts finite integers;
  `set_output_dock_height(name, n)` accepts `0..=256`; bad input errors.
- `output.list` gains `focused: bool`; `primary` stays for old Tacit builds.
- Position changes take effect on config reload and on next connect, like scale.

## 12. Data / State

- `wm::Output { id, name, geometry, work_area, dock_height, active_workspace,
  tiling_layout, splits }`; `WindowManager { pointer_output, focused_output?… }`
  (derived accessor, not a duplicated flag).
- `State.layer_outputs: BTreeMap<OutputId, smithay::output::Output>` replaces
  `State.primary_output`.
- `LayerSurface::user_data()` stores its `OutputId`.
- `DecorationConfig` gains `output_positions` and `output_dock_heights` maps.

## 13. Interfaces / Contracts

- Preserve `OutputId = u64`, connector-name lookup, `Rect`, `add_output`,
  `remove_output`, `arrange_output`, `ClientMode`, `CompCmd` variants,
  `output.list` keys and `es.dsp.set_output_scale`.
- Remove `primary_output_id()` from runtime code; update all ~20 call sites.

## 14. Dependencies

- No new crates. **Slopcheck:** N/A — smithay already exposes `Output::from_resource`,
  `layer_map_for_output` and `LayerSurface::user_data()`.

## 15. Risks

- High blast radius (`specs/IMPACT_LATEST.md`): mitigate with pure helpers and
  inline tests before each call-site migration.
- Global `dirty` flag makes `arrange_output` clear after one output; use an
  `arrange_outputs(ids)` helper that marks dirty for each output.
- Per-output scale is duplicated in Lua config and smithay; never write the WM
  scale from a stale frame — always read the smithay output.
- `layer_map_for_output` deadlocks if the same output's guard is held twice;
  gather geometry under one guard, drop it, then arrange.
- IPC/Lua shapes must stay additive for Tacit (wire compatibility test).

## 16. Out of scope

- See §4.

## 17. Acceptance / Verification

- `cargo test --bin eswm output_model` — pure output geometry/focus helpers.
- `cargo test --bin eswm output_config` — Lua config validation/storage.
- `cargo test --bin eswm layer_output` — layer routing and work areas.
- `cargo test --bin eswm pointer_bounds` — union bounds with displaced outputs.
- `cargo test --bin eswm dock_per_output` — per-output dock entries/height.
- `cargo test --bin eswm ipc_outputs` — additive JSON compatibility.
- `cargo clippy --all-targets && cargo test --bin eswm && cargo build` green.
- Manual live DRM UAT: laptop + HDMI, bars/dock/windows on their own screens,
  independent scale/work area, pointer crosses both outputs; record in
  `specs/verifications/e09s02-verify.yaml`.

## Implementation Steps

1. Add per-output `work_area`/`dock_height`, `pointer_output`, and pure helpers
   `auto_output_position`, `outputs_bounds`, `output_for_point`,
   `focused_output_id` with inline tests (RED → GREEN). → verify: `cargo test --bin eswm output_model`
2. Migrate WM internals off `primary_output_id()`; per-output work-area access
   and `arrange_outputs(ids)` so every affected output re-lays out. → verify: `cargo test --bin eswm output_model`
3. Add Lua `set_output_position` / `set_output_dock_height`, maps in
   `DecorationConfig`, `set_dock_height` compatibility path, tests. → verify: `cargo test --bin eswm output_config`
4. Apply configured/auto position and scale on connect and config reload; store
   `State.layer_outputs`; clear it on disconnect. → verify: `cargo test --bin eswm output_connect`
5. Route layer surfaces by requested output via `Output::from_resource` + user
   data; derive each work area from that output's `LayerMap` zone + origin. → verify: `cargo test --bin eswm layer_output`
6. Track pointer output on motion/enter; use `focused_output_id()` for placement
   and workspace commands; add `focused` to `output.list` while keeping
   `primary`. → verify: `cargo test --bin eswm focused_output`
7. Replace width-summing `max_screen_size()` with union bounds; clamp absolute
   motion and `clamp_to_output`; constrain popups to their output's work area. → verify: `cargo test --bin eswm pointer_bounds`
8. Make dock entries/height/render/click per output; Lua `es.dock.*` follows
   focused output; cache key includes output id. → verify: `cargo test --bin eswm dock_per_output`
9. Emit one Hyprland-compatible monitor per output from position/scale/focus;
   snapshot monitor follows focused output. → verify: `cargo test --bin eswm ipc_outputs`
10. Run full preflight. → verify: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`
11. Manual UAT (operator, live DRM): two outputs with configured positions;
    bars, windows, dock, pointer and scale on the correct screens. → verify: `manual: record in specs/verifications/e09s02-verify.yaml`

## Risks

- See §15; full caller/contract map in `specs/IMPACT_LATEST.md`.
