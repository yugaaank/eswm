# e11s01 — Session identity export with graceful degradation

**type:** feat
**risk:** P1
**context:** infra
**delta:** MODIFIED

## Context

eswm sets `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP` only on child processes it
spawns itself (`src/main.rs:1778-1779`). It never identifies itself to the
user's systemd or D-Bus activation environment, so a portal launched by the
user session believes another compositor owns the Wayland display and file
pickers open there. This story makes eswm announce its session at startup,
degrade to warnings when the optional tools are absent, and undo only what it
set on shutdown. No Wayland protocol work is included.

## Zoom-out (existing modules)

- **`main.rs`** — purpose: compositor entry, Wayland protocol handlers and
  process spawning. Callers: `udev.rs`, `lua_state.rs`, inline tests.
  Contracts: `spawn_shell_command` uses `sh -c`; `spawn_argv_command` uses argv
  directly; spawn logs are 0600; no key content reaches logs or sockets.
- **`udev.rs`** — purpose: the only runtime path (DRM/libseat), event loop and
  socket lifecycle. Caller: `main.rs:1389`. Contracts: seat grab before DRM
  access; clean shutdown restores DRM master and TTY.
- **`spawn_program()`** — purpose: apply eswm's child environment and reap
  children. Callers: `main.rs:1816,1827,1881`; indirect from Lua and keybinds.
  Contracts: existing two env vars stay set; added vars must not overwrite a
  caller's explicit values.

## Requirements

#### MODIFIED: Spawned children inherit the full eswm session identity
**Before:** only `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP` were set for
children spawned by eswm.
**After:** children also inherit `XDG_SESSION_TYPE=wayland` and
`XDG_SESSION_DESKTOP=eswm`, sourced from one session identity snapshot shared
with the startup export. Existing values set intentionally by a caller are not
overwritten.

#### ADDED: Startup exports the identity to the user session
eswm sets the four identity variables in its own process and, when available,
runs `systemctl --user import-environment` and
`dbus-update-activation-environment --systemd`. Missing tools or an
unavailable user bus produce a warning, never a startup failure.

#### ADDED: Opt-out and ownership-safe cleanup
An explicit opt-out env var (`ESWM_NO_SD_VARS`) skips all external exports. On
clean shutdown eswm restores only the variables it actually changed; values a
user changed afterwards survive.

## Reason for depth

- `session_env.rs` is a separate module because startup, shutdown and child
  spawning are three callers of one ownership snapshot; keeping them inline in
  `main.rs` would duplicate the "did we own this value?" bookkeeping and make
  it untestable without a live session.
- Command builders return plain data (`Command` plus args) so export behavior
  is testable without a running systemd user bus or D-Bus daemon.

## Steps

1. Add `session_env::SessionIdentity` with the four variables and a pure
   `snapshot(wayland_display)` builder; set them in-process and honour
   `ESWM_NO_SD_VARS` → verify: `cargo test --bin eswm session_env::tests::sets_eswm_identity_vars`
2. Add the systemd `import-environment` command builder and optional execution
   with a warning on absence/failure → verify: `cargo test --bin eswm session_env::tests::systemd_import_command_uses_user_scope`
3. Add the D-Bus activation environment command builder with the same guard;
   never include key content or secrets in exported variables → verify: `cargo test --bin eswm session_env::tests::dbus_activation_command_includes_vars`
4. Add a `SessionScope` RAII guard that restores only values it owned and is
   idempotent on drop → verify: `cargo test --bin eswm session_env::tests::restore_only_owned_vars`
5. Create the scope at `run_udev()` startup, drop it on shutdown, and have
   `spawn_program()` source child identity from the same snapshot; run the
   operator UAT below → verify: `cargo test --bin eswm session_env::tests::session_scope_drops_once`

## Verification script (operator, live DRM only)

1. Start eswm from a TTY with no opt-out set.
2. Run `systemctl --user show-environment` and confirm the four eswm values.
3. Open a GTK/Qt application and trigger its file picker; it must open on the
   eswm session, not another running compositor.
4. Quit eswm cleanly and confirm only eswm-owned values were removed.
5. Restart with `ESWM_NO_SD_VARS=1` and confirm eswm still starts, logs a
   skip warning, and leaves the user environment untouched.

### UAT record

| # | Check | Result |
| --- | --- | --- |
| 1 | Start from a TTY, no opt-out | PASS (operator: clean `eswmdev` session on VT4) |
| 2 | `systemctl --user show-environment` lists four eswm values | PASS (operator: all four values shown in the `eswmdev` session) |
| 3 | GTK/Qt file picker stays on eswm | PASS (operator: Super+V check; picker opened on eswm) |
| 4 | Clean quit removes only eswm-owned values | PASS (operator: `Super+Alt+Backspace`, then `eswm-env` showed no identity vars) |
| 5 | `ESWM_NO_SD_VARS=1` starts with a skip warning | PENDING — operator only |

Live PASS rows come from the operator's clean `eswmdev` login session. This
agent session runs inside an existing graphical session and must not grab the
VT (ADR-0002), so it did not start or quit eswm. Automated unit coverage for
the export commands, ownership and child env is green.

## Out of scope

- Xwayland `DISPLAY` export (no Xwayland today).
- Adding Rust crate dependencies (external tools are optional system commands).
- Resolving two concurrently running compositors beyond guards and opt-out.
- Any Wayland protocol work from proper-DE Phase 1-4.

## Risks

- Clobbering a shared user session: mitigated by ownership tracking and opt-out.
- Another compositor starting after eswm can overwrite the same variables:
  documented; conflict arbitration needs a product decision.
- A missing or slow external command could stall startup: keep commands
  optional and non-blocking, with warnings.
- Live DRM UAT cannot run inside an existing Wayland/X11 session (ADR-0002).
