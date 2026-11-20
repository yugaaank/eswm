# Story e07s01: Spawning: sh -c, XDG_CURRENT_DESKTOP, XDG_STATE_HOME logs, SIGCHLD

**id:** e07s01  
**type:** fix  
**risk:** P3  
**context:** process  
**status:** pending  
**bcps:** 5  
**delta:** MODIFIED

## 1. Context

Commands are currently split on whitespace and executed directly (`spawn_on_socket`), so shell syntax such as `bash -c 'echo a b'` and env prefixes like `FOO=1 prog` do not behave as users expect (Tacit autostart relies on shell strings). Child stderr is written to `/tmp/<program>-spawn.log` (predictable/shared/readable, never rotated), `TERMINAL=kitty` is hardcoded, `XDG_CURRENT_DESKTOP` is not set, and each child is reaped by its own waiting thread. Hyprland runs exec strings as `sh -c <string>`.

## 2. Problem

- Whitespace splitting breaks quoting, shell operators, and env assignment prefixes in command strings.
- Spawn logs are per-program files in `/tmp` with weak hygiene/permissions expectations.
- Terminal program and desktop identity are not configurable/advertised consistently.
- Per-child wait threads do not scale and do not provide a single reaping path.

## 3. Goal

Make spawning shell-compatible for command strings, configurable for terminal/argv launches, and robust for logging and child reaping.

## 4. Non-goals

- Journald logging option.
- Changing Wayland socket semantics (`WAYLAND_DISPLAY`) or process-group detachment behavior beyond what is required.

## 5. Users / Actors

- User configuring eswm via Lua (`es.dsp.exec_cmd`, `es.bind_cmd`, autostart).
- Compositor keybind actions that launch programs (terminal, launchers).
- Child processes spawned by eswm.

## 6. Preconditions

- e05 event loop and existing Lua/keybind plumbing are in place.
- Preflight green on `main`.

## 7. Postconditions

- Shell command strings execute via `sh -c <string>`.
- Argv-form exec bypasses shell parsing.
- Spawned children receive `XDG_CURRENT_DESKTOP=eswm` and configured `TERMINAL`.
- Child output is appended to `$XDG_STATE_HOME/eswm/spawn.log` (0600).
- Children are reaped through one SIGCHLD-driven `waitpid(-1, WNOHANG)` loop (no per-child wait thread).

## 8. Scope

**In scope**
- Shell-string execution path for `es.dsp.exec_cmd` and `es.bind_cmd` command strings.
- New argv exec API: `es.exec({"prog","arg1",...})` (top-level).
- Terminal configuration API and use in `spawn_terminal` / `TERMINAL` env.
- Unified spawn log at `$XDG_STATE_HOME/eswm/spawn.log` (0600, append, stdout+stderr).
- Single SIGCHLD reaper integrated with calloop in real backend; testable reap helper.

**Out of scope**
- Journald logging option.

## 9. Confirmed decisions

1. **Shell execution scope:** `es.dsp.exec_cmd(string)` and `es.bind_cmd` command strings run as `sh -c <string>`. Direct argv launches use argv exec.
2. **Argv API:** add top-level `es.exec` taking a Lua table/list of strings (first = program). Invalid/empty input returns Lua error. Keep `es.dsp.exec_cmd` as shell-string API.
3. **Terminal config:** add `es.dsp.set_terminal(program)` stored in shared config; default `kitty`. `spawn_terminal()` and `TERMINAL` env use configured value.
4. **Log target/stream:** append child stdout and stderr to `$XDG_STATE_HOME/eswm/spawn.log`; create dirs/file as needed with mode 0600.
5. **Reaping:** maintain child handling and reap via `waitpid(-1, WNOHANG)` loop on SIGCHLD in udev event loop; remove per-child `std::thread::spawn` waiters. Provide testable reap helper.

## 10. Requirements (delta tags)

### MODIFIED: Exec strings are shell-interpreted
**Before:** Command strings are whitespace-split and executed directly (`parse_cmd` + direct exec), so quoting/env prefixes/shell syntax are not honored.  
**After:** Command strings dispatched as exec strings (`es.dsp.exec_cmd`, `es.bind_cmd`) are executed as `sh -c <string>` so `bash -c 'echo a b'` and `FOO=1 prog` behave correctly.

### ADDED: Argv-form exec API
**Before:** No argv-table exec API; command launching is string-based or hardcoded.  
**After:** `es.exec({"prog","arg",...})` (top-level) executes an argument vector without shell parsing.

### MODIFIED: Spawn environment and terminal selection
**Before:** Spawned children get hardcoded `TERMINAL=kitty`; `XDG_CURRENT_DESKTOP` is not set by spawn path.  
**After:** Spawned children get `XDG_CURRENT_DESKTOP=eswm` and `TERMINAL` from configurable terminal (default `kitty`).

### MODIFIED: Spawn logging destination and permissions
**Before:** Child stderr goes to `/tmp/<program>-spawn.log` (per program, append).  
**After:** Child stdout and stderr go to `$XDG_STATE_HOME/eswm/spawn.log` (shared, appended, 0600).

### MODIFIED: Child reaping strategy
**Before:** Each spawned child is waited on in a dedicated thread.  
**After:** One SIGCHLD-driven `waitpid(-1, WNOHANG)` reaping loop handles all children; no per-child wait threads.

## 11. UX / API notes

- `es.dsp.exec_cmd("...")` remains string-based but becomes shell-interpreted.
- `es.exec({...})` (top-level) is argv-based (no shell).
- `es.dsp.set_terminal("...")` configures terminal used by `spawn_terminal`/`TERMINAL`.

## 12. Data / State

- Shared spawn config/state: terminal program, optional child pid tracking for reap handling.
- Log path derived from `XDG_STATE_HOME` (fallback consistent with existing state-dir logic).

## 13. Interfaces / Contracts

- Preserve `WAYLAND_DISPLAY` injection and session detachment (`setsid`).
- Spawn log file mode 0600 (stdout+stderr appended).
- `XDG_CURRENT_DESKTOP=eswm` exported to spawned children.

## 14. Dependencies

- `sh` available in runtime environment.
- calloop/signal integration in udev backend for SIGCHLD (Smithay/calloop 0.7-era APIs as used by project).

## 15. Risks

- **Quoting/compatibility:** switching to `sh -c` changes behavior for previously whitespace-split commands; verify common cases and special characters.
- **Event-loop integration:** SIGCHLD source wiring differs between real backend and tests; keep reap logic unit-testable.
- **File permissions/races:** ensure log directory/file creation is safe and enforces 0600.

## 16. Test strategy

- Unit tests (inline in `src/*.rs`) for: shell command argument construction/behavior, argv exec behavior, log path+mode, terminal config/env, reap loop (`waitpid`/ECHILD handling) via testable helpers.
- Build/type/lint gates: `cargo build`, `cargo clippy --all-targets`, `cargo test --bin eswm`.

## 17. Acceptance Criteria

- [ ] `es.dsp.exec_cmd` and `es.bind_cmd` execute command strings via `sh -c <string>`.
- [ ] `bash -c 'echo a b'` is passed to bash as `-c` plus one argument (shell semantics preserved).
- [ ] `FOO=1 prog` runs with environment assignment honored.
- [ ] `es.exec({"prog","arg",...})` executes argv without shell parsing.
- [ ] Spawned children receive `XDG_CURRENT_DESKTOP=eswm`.
- [ ] Terminal is configurable; `spawn_terminal` and `TERMINAL` env use configured value (default `kitty`).
- [ ] Child stdout and stderr append to `$XDG_STATE_HOME/eswm/spawn.log` with mode 0600.
- [ ] Children are reaped by one calloop SIGCHLD source using `waitpid(-1, WNOHANG)` loop; no per-child wait threads remain.
- [ ] `cargo build`, `cargo clippy --all-targets`, and `cargo test` pass.

## 18. Verification Script

1. Configure/bind `es.dsp.exec_cmd("bash -c 'echo a b'")` and confirm bash receives `-c` and a single argument string.
2. Run `es.dsp.exec_cmd("FOO=1 prog")` (or equivalent) and confirm `FOO` is set in child environment.
3. Set terminal via config/API, trigger `spawn_terminal`, confirm configured terminal launches and `TERMINAL` matches.
4. Spawn a child and confirm output is appended to `$XDG_STATE_HOME/eswm/spawn.log` with mode 0600.
5. Spawn short-lived children repeatedly and confirm no zombie accumulation; reaping occurs via SIGCHLD handler path.

## 19. Implementation steps (plan)

1. **Spawn model refactor (shell/argv/env/terminal/log):** introduce helpers for shell-command (`sh -c`) and argv execution, configurable terminal, `XDG_CURRENT_DESKTOP=eswm`, and unified spawn log path/mode (`$XDG_STATE_HOME/eswm/spawn.log`, 0600, stdout+stderr). Update `spawn_on_socket`/`spawn_terminal` and call sites to use the new model. → verify: `cargo test --bin eswm spawn` (new/updated spawn tests) and `cargo build`
2. **Lua/API and keybind wiring:** update `es.dsp.exec_cmd` and bind-cmd execution path to shell-string semantics; add top-level `es.exec(argv_table)` and `es.dsp.set_terminal(...)`; keep backward-compatible behavior where intended. → verify: `cargo test --bin eswm spawn` and `cargo build`
3. **SIGCHLD reaper:** add child tracking + `waitpid(-1, WNOHANG)` reap loop; register one calloop SIGCHLD source in udev backend; remove per-child wait threads. → verify: `cargo test --bin eswm spawn_reap` and `cargo build`

## 20. References

- `specs/epics/e07-spawn-keybinds/epic.yaml`
- `src/main.rs` (`spawn_terminal`, `spawn_on_socket`, logging/init)
- `src/lua_state.rs` (`es.dsp.exec_cmd`, `es.bind_cmd`)
- `src/udev.rs` (keybind `exec_cmd` handling, calloop event loop)
- `CONVENTIONS.md` (inline tests, preflight/always green)
