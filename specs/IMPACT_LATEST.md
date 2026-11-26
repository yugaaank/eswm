# Impact — e09s02 output model

## Target

`wm::Output` / `WindowManager::{primary_output_id, work_area, dock_height}` in
`src/wm.rs`, plus the `State::primary_output` / `layer_wrappers` plumbing in
`src/main.rs`, output setup, pointer bounds and layer/dock render/hit paths in
`src/udev.rs`, snapshots in `src/control.rs` and `src/ipc.rs`, and display
setters in `src/lua_state.rs`.

## Dependents (> 10 callers)

- `src/wm.rs`: `primary_output_id()` at 1043, consumed by placement, popup
  constraint, dock entries, `surface_under`, `window_at`, cycle focus, workspace
  commands, tiling layout sync.
- `src/main.rs`: new toplevel placement (90), popup constraint (390), layer
  map/work area (558–746 and 821–849), decoration sync (1260).
- `src/udev.rs`: layer hit-test (1550–1585), pointer absolute clamp / bounds
  (1977–2018, 2419–2432), dock strip (2249–2274), layer render (3208–3231),
  scale handling (3122, 3253, 3533, 3591, 3860).
- `src/control.rs`: `outputs_json` (336–351), `clamp_to_output` (262–279),
  workspace focus command (227–242).
- `src/ipc.rs`: single-monitor snapshot (426–464), `CompCmd` routing (121–143).
- `src/lua_state.rs`: `InputConfig.output_scales` (18), `set_output_scale`
  (219–230), dock/tile setters (373–456).
- Inline tests in all of the above (e.g. `remove_output_moves_windows_to_survivor`,
  `control_output_list_returns_outputs`, `output_scale_config_stores_and_looks_up_by_name`).

## Affected Stories

- e09s02 — output model (this story; owner of the change).
- e08s03 — per-output scale setter must keep working (wire/Lua compatibility).
- e10s01/e10s02 — resize/split geometry reads `inner_rect_for_output`, so
  per-output work areas must not regress their byte-identical geometry tests.
- e06s01/e06s03 — popup constraints and layer keyboard focus.
- e09s03 — IPC protocol will inherit any `outputs_json` field additions.
- e09s04 — Lua config shape will inherit the new per-output setters.

## Test Coverage

- `src/wm.rs` inline tests: arrange, focus MRU, remove-output migration,
  split geometry — cover most geometry paths, but not per-output work area or
  focused-output derivation.
- `src/main.rs` inline tests: control snapshots, layer keyboard focus, popup
  work-area constraint — mostly single virtual output.
- `src/lua_state.rs` inline tests: output scale validation/storage.
- `src/udev.rs` inline tests: mode selection, dock segment math, render order —
  no test for union bounds with displaced outputs.
- Gaps: multi-output position/scale/work area, layer surface requesting a
  specific output, pointer bounds with non-zero origins, per-output dock
  entries, `focused` output with pointer/window fallback.

## Risk: High

Shared WM and snapshot interfaces with > 10 callers, and new multi-output
behavior has no automated coverage yet. Mitigate with pure helpers plus inline
tests before each call-site migration, and keep IPC/Lua wire shapes additive.

## Recommended action

Add tests first (pure geometry/focus/output-config helpers), then migrate call
sites in small TDD steps. Preserve `primary` JSON key as a deprecated alias of
the new focused output so Tacit does not break. `plan-work` may proceed; the
design decisions were confirmed with the operator on 2026-11-26.

<!-- The output model had two "primaries"; this is the compositor equivalent of
     two captains on one ship. e09s02 promotes one and retires the other. -->
