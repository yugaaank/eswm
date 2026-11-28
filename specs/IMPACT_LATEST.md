# Impact — e11s01 + e11s02 (Phase 0 session identity)

## Target

`src/main.rs` (compositor entry, `socket_name()`, `spawn_program()` and the
spawn wrappers), `src/udev.rs` (`run_udev()` startup/shutdown), plus new
`src/session_env.rs`, `resources/eswm-portals.conf` and `docs/session.md`.
This supersedes the e09s04+e09s05 report, which stays in git history.

## Zoom-out

- **`main.rs` spawn purpose:** launch keybind/Lua/terminal commands with eswm's
  socket and desktop identity in their environment. **Callers:**
  `lua_state.rs:777,790`, `udev.rs:1335`, `main.rs:1852,1881`, plus four inline
  spawn tests. **Contracts:** `spawn_shell_command` uses `sh -c`,
  `spawn_argv_command` does not; stdout/stderr go to the 0600 spawn log;
  `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP` are already set on children.
- **`udev.rs` purpose:** own the only runtime path (DRM/libseat), the event loop
  and socket lifecycle. **Callers:** `main.rs:1389` (`run_udev`). **Contracts:**
  one seat grab before DRM access, no nested backend, sockets use
  `socket_name()`, clean shutdown must release DRM master and TTY.

## Dependents (7 direct call sites + entry point)

- `src/main.rs:1389` — calls `udev::run_udev()`.
- `src/main.rs:1816,1827,1881` — `spawn_program` callers.
- `src/lua_state.rs:777,790` — Lua `bind`/`bind_cmd` spawn wrappers.
- `src/udev.rs:1335` — keybind command dispatch.
- `src/udev.rs:414,691,765,772` — `new_state`/`socket_name` users during startup.

## Affected Stories

- Story e11s01: session identity export with graceful degradation.
- Story e11s02: portal selection config and TTY launch documentation.
- Prior stories with preserved contracts: e07s01 (spawn/keybinds), e09s04
  (Lua config), e09s05 (toplevel/rule metadata), e04 (security: no key content
  on interfaces; sockets remain 0600).

## Test Coverage

- `src/main.rs::tests::spawn_shell_command_uses_configured_terminal_env` —
  proves child env injection today.
- `src/main.rs::tests::spawn_argv_command_executes_without_shell_wrapper` —
  proves argv spawn stays shell-free.
- `src/main.rs::tests::spawn_shell_command_writes_stdout_stderr_to_unified_spawn_log`
  — proves spawn logging stays 0600.
- Gap: no test covers session-scope env export, systemd/D-Bus command building,
  opt-out, or shutdown restore. e11s01t1-t5 add those.
- Gap: no test proves the shipped portal config or launch docs. e11s02t1-t2 add
  inline assertions over `include_str!` content.

## Risk: Medium

Seven call sites plus the entry point, but the change is additive: existing
child env values stay the same, and external commands are optional and guarded.
Main hazard is clobbering a shared user session environment, mitigated by
ownership tracking, opt-out and warnings instead of failures.

## Recommended action

Proceed with Add tests first (TDD): all five e11s01 tasks start RED, and the
systemd/D-Bus command builders stay pure data so no test needs a live bus. Live
DRM UAT remains operator-only (ADR-0002).
