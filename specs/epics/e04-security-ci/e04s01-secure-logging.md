### Story e04s01: Stop logging key presses; secure log file location and permissions

**type:** fix  
**risk:** P0  
**context:** security  

**Context:** `udev.rs:773` logs every key press (modifier state + raw keysym) via
`tracing::info!`. This runs on every key press, including presses forwarded to
clients. The default filter is `info`, so passwords are written to
`/tmp/eswm.log` with mode `0644` (world-readable under the usual umask).
`main.rs:413-422` opens the log at `/tmp/eswm.log` with `.create(true).write(true).truncate(true)`,
destroying any previous session's log on startup.

## Steps

1. Delete the key-press `tracing::info!` line in `udev.rs:773`; never log keysyms or text above `trace` level -> verify: `cargo build && cargo test`
2. Move log file to `$XDG_STATE_HOME/eswm/eswm.log` with mode `0600` using `std::os::unix::fs::OpenOptionsExt::mode(0o600)` -> verify: `cargo build && cargo test`
3. Rename previous log to `eswm.log.old` instead of truncating it -> verify: `cargo build && cargo test`
4. Move the "layer mapped" debug line (main.rs:280-282) from per-commit to layer creation -> verify: `cargo build && cargo test`
5. Unit test: log file is created at `$XDG_STATE_HOME/eswm/eswm.log` with mode `0600` -> verify: `cargo test`
6. Unit test: no key press data appears in any log output at `info` level -> verify: `cargo test`

## Verification Script

1. Run `cargo test` and confirm all tests pass.
2. Confirm `udev.rs` has no `tracing::info!` call that includes keysym or text data.
3. Confirm log path uses `$XDG_STATE_HOME/eswm/eswm.log` with mode `0600`.
4. Confirm previous log is preserved as `eswm.log.old`.

## Out of scope

- Changing the log filter level (keep `info` as default)
- Remote logging or log rotation beyond `.old` rename

## Risks

- Existing configs/scripts that read `/tmp/eswm.log` will break; document the new path.

---
