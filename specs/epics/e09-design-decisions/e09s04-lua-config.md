# Story e09s04: Lua config shape — declarative `es.config{}`, function binds, rules, reload

**id:** e09s04
**type:** refactor
**risk:** P1 (raised from P3: shared Lua API consumed by Tacit; see `specs/IMPACT_LATEST.md`)
**context:** lua
**status:** completed
**bcps:** 2
**delta:** ADDED + MODIFIED

## 1. Context

Tacit's Hyprland config uses ~1900 lines of `hl.config{}`, `hl.bind(keys, fn,
opts)`, `hl.window_rule{}`, `hl.on(event, fn)` and `hl.dsp.*`. eswm's `es` table
has action-string binds, `es.dsp` setters, and stubs that report success while
doing nothing (`define_submap`, `dsp.global`, `config.notify`,
`config.setcursor`, `safe_require`); `is_file_exists` always returns false;
`config.reload` does not reload. `DecorationConfig` is an `Arc<Mutex>` even
though Lua and the renderer live on one thread.

Operator decisions 2026-11-26: implement the **full** shape in this branch;
stubs must raise explicit "not implemented" errors; reload actually re-runs the
config file; Lua/IPC wire shapes stay additive and key contents stay off all
interfaces (e04).

## 2. Problem

- Config mistakes are silent: unknown keys, no-op stubs and a false
  `is_file_exists` make a broken config look loaded.
- Binds can name only a closed set of action strings; no Lua function callbacks
  and no event hooks for Tacit-style composition.
- Rules exist in `wm.rs` but no config path populates them.
- Reload only emits an event; changing a Lua file needs a compositor restart.
- Per-frame `Mutex` lock for decoration values on a single-threaded state.

## 3. Goal

A Lua config surface where declarative settings are validated, binds can call
Lua functions, events can call registered hooks, window rules come from config,
reload re-executes the file, unimplemented names fail loudly, file existence is
real, and decoration values use single-threaded interior mutability.

## 4. Non-goals

- Safe-require module loading, submaps, quickshell dispatch, cursor setter from
  Lua, layer rules (explicit "not implemented" instead).
- JSON config, key-content events, Hyprland `>>` wire compatibility.
- Persistence of config across restarts beyond the file itself.

## 5. Users / Actors
- **Operator** — writes `~/.config/eswm/config.lua`.
- **Tacit** — declares config, binds, rules and event hooks through `es`.
- **Compositor** — consumes validated config and dispatches callbacks.

## 6. Preconditions
- Preflight green; e09s03 event bus and numeric error codes landed.
- A config path is known (`XDG_CONFIG_HOME`/`HOME` → `eswm/config.lua`).

## 7. Postconditions
- Existing string binds and `es.dsp.set_*` calls behave as before.
- New Lua shapes are validated; failures name the offending key.
- Reload replaces (not duplicates) binds, hooks and rules, then emits the
  legacy `configreloaded>>` and the `config_reloaded` bus event exactly once.

## 8. Scope
In scope: `src/lua_state.rs`, `src/udev.rs` (config load/reload/keybind
dispatch), `src/ipc.rs` (`CompCmd::Reload`), `src/main.rs` (State fields, event
hook firing), `src/wm.rs` (rule ingestion helper), `config.example.lua`, inline
tests.

## 9. Zoom-out (purpose / callers / contracts)
- **Purpose:** `lua_state.rs` registers `es`, parses keybinds and mutates shared
  config from the Lua file.
- **Callers:** `udev.rs:786-820`, `lua_state.rs` inline tests, operator config.
- **Contracts:** string binds stay valid; `es.dsp.set_*` keep their validation;
  no raw key data reaches Lua or IPC; `primary` stays a deprecated alias of
  `focused`; legacy reload emission stays intact.

## 10. Requirements (delta tags)

### ADDED: Callable, validated `es.config{...}`
`es.config { wm_mode = "tiling", general = { layout = "master" }, ... }`
validates every key before applying any value. Supported keys: `wm_mode`,
`tiling_layout`, `general` (`layout`, `mode`), `border_size`, `outer_gap`,
`inner_gap`, `titlebar_height`, `dock_height`, `corner_radius`,
`min_window_size` (`{width, height}`), the four RGBA color keys, `output_scale`,
`output_position`, `touchpad` and `keymap`. Unknown keys or wrong types raise a
Lua error naming the key; no partial application.

### ADDED: Function-valued binds
`es.bind(keys, function(arg) ... end [, arg])` registers a Lua callback; the
existing string-action form and `es.bind_cmd` remain unchanged. Callbacks run
on key press, receive the optional integer arg, and errors are logged without
killing the compositor.

### ADDED: Event hooks
`es.on("window_open", function(data) ... end)` registers a callback for an
eswm event kind. Hooks fire after per-tick coalescing, receive the event's data
as a Lua table, and unknown kind names raise at registration. No key-content
events exist to subscribe to.

### ADDED: Window rules from config
`es.window_rule { app_id = "...", workspace = 2, mode = "floating" }`
registers a `wm::WindowRule`. `app_id` is required; workspace/mode are optional
and validated. Rules are staged during config load and installed into
`wm.add_window_rule` before windows are created; reload replaces the set.

### MODIFIED: Reload re-runs the config
**Before:** `CompCmd::Reload` re-applied output positions and emitted events;
`es.config.reload` was a no-op.
**After:** both paths call one `State::reload_lua_config` that clears binds,
hooks and staged rules, re-executes the same file, re-applies configured
scales/positions/keymap, then emits the same legacy and bus events. A bad
reload leaves the previous config in place and is logged.

### MODIFIED: Stubs fail loudly
**Before:** `define_submap`, `dsp.global`, `config.notify`, `config.setcursor`,
`safe_require` and `layer_rule` returned success.
**After:** each raises `not implemented: <name>`; migration note in this spec.

### MODIFIED: Real file existence
**Before:** `es.is_file_exists(path)` always returned false.
**After:** returns `std::path::Path::new(path).exists()`.

### MODIFIED: No per-frame decoration lock
**Before:** `Arc<Mutex<DecorationConfig>>` shared with Lua and locked by the
renderer each frame.
**After:** `Rc<RefCell<DecorationConfig>>` on the single threaded compositor
state; setters mutate it directly, the renderer borrows briefly.

## 11. UX / API notes
- Unknown `es.config` keys and bad values raise immediately at config load;
  the operator sees a warning naming the file and the key.
- `es.on` names are the e09s03 flat snake_case kinds (`window_open`,
  `config_reloaded`, ...); unknown names fail at load, not at first event.
- `es.window_rule` matches the real client `app_id` (e09s05 first-commit
  registration); `mode` accepts the `ClientMode` names used by `window.set_mode`.

## 12. Data / State
- `Keybind` gains an optional Lua `RegistryKey` for function actions.
- `LuaHooks`: `Vec<(EventKind, RegistryKey)>` behind a mutex, never held while
  a callback runs.
- Staged rules: `Arc<Mutex<Vec<WindowRule>>>` on State, drained into `wm` at
  config load and on reload.
- `State.lua_config_path: Option<PathBuf>` is the single reload source.

## 13. Interfaces / Contracts
- Additive Lua names only; string binds, `bind_cmd` and `es.dsp.*` setters keep
  their signatures. IPC JSON and the deprecated `primary` alias are untouched.
- Legacy `.socket2.sock` still emits `configreloaded>>`; event bus still emits
  one `config_reloaded` frame.

## 14. Dependencies
No new crates. `mlua` (lua54) [OK], `serde_json` [OK], `flume` [OK].
`Rc<RefCell<_>>` replaces one `Arc<Mutex<_>>`; no extra package.

## 15. Risks
- Lua callbacks can re-enter config setters: hooks are copied out of their
  mutex before invocation; no lock is held across a callback.
- Reload can duplicate binds/hooks/rules: clear all three before re-execution
  and keep a test that counts them after two loads.
- Strict stubs can break an older operator config: migration note above and a
  startup warning naming the failing function.

## 16. Out of scope
Safe require, submaps, quickshell dispatch, layer rules, key-content events,
JSON config, config persistence beyond the Lua file, Tacit config conversion.

## 17. Acceptance / Verification
- `cargo test --bin eswm config_table`, `lua_function_bind`, `lua_event_hook`,
  `lua_window_rule`, `config_reload`, `lua_stub`, `decoration_refcell` pass.
- `cargo clippy --all-targets && cargo test && cargo build && python3
  scripts/spec-lint.py` green after each task.
- Live DRM UAT recorded in `specs/verifications/e09s04-verify.yaml` with
  explicit operator N/A decisions where hardware is absent.

## Implementation Steps

1. Add callable `es.config{}` with validate-then-apply and RED/GREEN tests → verify: `cargo test --bin eswm config_table`
2. Accept Lua function values in `es.bind` and dispatch them from keybind handling → verify: `cargo test --bin eswm lua_function_bind`
3. Register and fire `es.on` hooks on flushed events with JSON→Lua data conversion → verify: `cargo test --bin eswm lua_event_hook`
4. Stage `es.window_rule{}` entries and install them into the WM → verify: `cargo test --bin eswm lua_window_rule`
5. Re-run the config file on reload (Lua and compat socket), clearing binds/hooks/rules first → verify: `cargo test --bin eswm config_reload`
6. Make unimplemented names raise and make `is_file_exists` real; fix the stale example config → verify: `cargo test --bin eswm lua_stub`
7. Replace the decoration `Arc<Mutex>` with `Rc<RefCell>` and prove live setter visibility → verify: `cargo test --bin eswm decoration_refcell`
8. Operator live DRM UAT against the checklist in the verification YAML → verify: `manual: reload twice, fire a function bind, add a rule and confirm the stub errors`

## Risks (implementation)
- See §15. Any new event kind needs its hook name in the e09s03 inventory as
  well; no key data ever crosses into Lua hooks.
