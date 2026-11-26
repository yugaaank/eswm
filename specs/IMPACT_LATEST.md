# Impact — e09s04 + e09s05 (single branch)

## Target

`src/lua_state.rs` (Lua config bridge), `src/main.rs` (xdg-shell handler and
window↔toplevel glue), `src/wm.rs` (rules/creation/mode), `src/ipc.rs`
(`CompCmd::Reload`) and `src/udev.rs` (keybind dispatch, config load and reload
follow-ups). Both stories land on one branch; this is the combined blast radius.
This supersedes the e09s03-specific report (which remains in git history).

## Zoom-out

- **`lua_state.rs` purpose:** register the `es` table, parse keybinds, and
  mutate shared config from the Lua config file. **Callers:** `udev.rs:786-820`
  (init/register/`_do_file`), `lua_state.rs:583-796` (inline Lua tests),
  Tacit's `~/.config/eswm/config.lua`. **Contracts:** `es.bind`/`bind_cmd`
  accept strings and register into `lua_keybinds`; `es.dsp.set_*` validate and
  write shared config; existing action strings keep working; Lua never receives
  raw key contents (e04).
- **`main.rs` xdg-shell purpose:** map Wayland toplevels to WM windows.
  **Callers:** smithay delegates (`main.rs:1745`), `udev.rs` keybind/close/
  popup paths, inline fixtures. **Contracts:** one live toplevel ↔ one
  `WindowId`, `apply_layout` is the only configure writer, `toplevel_destroyed`
  removes the window, `primary` is a deprecated IPC alias of `focused`.
- **`wm.rs` purpose:** pure model and rule evaluation. **Callers:** `main.rs`,
  `udev.rs`, `control.rs`, `ipc.rs`, 200+ inline tests. **Contracts:** rules
  match `app_id` at `add_window`; `ClientMode` splits `Placement` from
  `Presentation`; `set_mode` queues an event; `dirty` gates arrangement.

## Dependents (> 10, shared interfaces)

- `src/lua_state.rs:147-562` — `register_api` (3 refs) owns every `es.*` name;
  `Keybind` (15 refs), `DecorationConfig` (19 refs).
- `src/main.rs:90-127,266-270,537-589,916-1023` — `new_toplevel`,
  `window_id_of` (11 refs), `toplevel_for_window` (9 refs),
  `sync_window_meta` (3 refs), `commit`, `flush_events` (5 refs).
- `src/udev.rs:785-834,1215-1340,1694-1795` — config load, `execute_keybind`
  (3 refs) and four `toplevel_for_window` call sites.
- `src/wm.rs:549-568,1645-1657,1790-1857,2169` — `rules`, `add_window_rule`
  (5 refs), `match_rule` (2 refs), `add_window`, `set_mode`.
- `src/ipc.rs:140-186,436-453` — `CompCmd::Reload` (2 refs), legacy
  `configreloaded>>` and the new `config_reloaded` bus event.
- `src/control.rs:355,556-563` — command writes stay denied; `window.set_mode`
  must keep using `wm.set_mode`.
- `notes/tacit-integration.md`, `config.default.lua`, `config.example.lua` —
  wire/Lua shape consumed outside the crate.

## Affected Stories

- e09s04 (owner): Lua config shape — callable/validated `es.config{}`,
  function binds, `es.on`, rules, reload, stub errors, real `is_file_exists`,
  lock removal.
- e09s05 (owner): first-commit toplevel registration, client
  fullscreen/maximize requests, rules with real `app_id`.
- e09s03 (landed): event bus and reload event must stay byte-compatible.
- e09s02 (landed): reload re-applies configured output positions.
- e08s03/e10s02 (landed): input setters and resize modes read the same config.

## Test Coverage

- `lua_state::tests::keybind_parse_*`, `output_config_lua_setters_validate_and_store`,
  `reset_tiling_splits_lua_command_is_queued` — current Lua setters/binds.
- `wm::tests::window_rule_*` (4), `set_window_meta_updates_app_id_and_title`,
  `xdg_state_flags_*` — rule matching and presentation flags.
- `tests::fixture_client_creates_toplevel_wm_sees_app_id`,
  `titlebar_close_with_toplevel_requests_client_close`,
  `event_stream_*` — limited shell/event integration.
- Gaps: no test for callable `es.config{}`, function-valued `es.bind`, `es.on`,
  Lua rule registration, real reload re-execution, stub errors, or real
  `is_file_exists`; no wire-level `fullscreen_request`/`maximize_request`
  handler test; no first-commit registration test; no test that a client
  request reaches `apply_layout`.

## Risk: High

Shared Lua and xdg-shell interfaces with >10 dependents each, direct impact on
Tacit's config/wire contract, and no tests for the new behaviours.

## Recommended action

Add tests first (RED) for every new Lua shape, handler and rule path; keep the
IPC/Lua wire shapes additive (`primary` stays a deprecated alias, action-string
binds keep working, legacy `configreloaded>>` stays). Make stub failures loud
only for names Tacit does not need to call during startup, and document the
migration. Sequence e09s04 rule registration before e09s05 fires rules at
creation. Full local gate after each task; live DRM UAT closed by explicit
operator decisions, never waivers.
