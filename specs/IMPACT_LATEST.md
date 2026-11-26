# Impact — e09s03 IPC event stream

## Target
`src/control.rs` (native control channel) plus a new dedicated event socket in
the same module, and the event-emission hooks in `src/main.rs`, `src/udev.rs`,
`src/ipc.rs` and `src/lua_state.rs`.

Module purpose: own the local control channel — bind `$XDG_RUNTIME_DIR/eswm-<name>.sock`,
accept connections, parse newline-delimited JSON requests, hand them to the
compositor loop through `flume`, and serialize replies. Socket threads never
touch `State`; `control::drain` is the only boundary into live state.

## Dependents (7)
- `src/main.rs:13` — `mod control`; `State.control` at `:1112`; test fixture at `:1295`.
- `src/main.rs:1753,1956-2172` — 10 inline tests call `handle_request` directly.
- `src/udev.rs:765-768` — sole `Control::start` caller; failure degrades to "control socket disabled".
- `src/udev.rs:828-829` — sole `control::drain` caller, first statement in the DRM loop.
- `src/ipc.rs:157-185,300-337` — legacy `.socket2.sock` already emits `configreloaded>>` via `SharedSinks::emit`; the new bus must not disturb it.
- `src/lua_state.rs:527-539` — `es.config.reload` is a no-op stub; the config-reloaded event source depends on it.
- `notes/tacit-integration.md:154-171,226-239` — documents the wire contract and required events.

## Affected Stories
- Story e09s03 (owner): IPC protocol — events, subscribe semantics, security.
- Story e09s02 (just landed): monitor_added/removed/focused/layout read the per-output model.
- Story e09s04 (pending): Lua config shape; `config_reloaded` needs a real reload hook.
- Story e09s05 (pending): a future rules engine may add `window.update_rules` events.
- Story e10s02 (landed): window rect changes from resize could emit state events; keep additive.

## Test Coverage
- `src/main.rs:1956-2026` — control state/list replies and id echo.
- `src/main.rs:1990-2008` — unknown command / missing window errors (string only, no codes).
- `src/main.rs:2030-2049` — `focused` + deprecated `primary` alias on output.list.
- `src/main.rs:2074-2172` — clamp, workspace move, cross-output rejection.
- `src/ipc.rs:751-863` — legacy Hyprland-style JSON key parity; no event coverage.
- Gaps: no test binds a control socket; no 0600 check; no line-cap check; no
  SO_PEERCRED check; no live-socket safety check; no subscribe/event/seq/coalescing
  test; no hello/version/error-code test; `exec`/`config` rejection is only the
  unknown-command fallback.

## Risk: High
The control socket is a shared interface consumed by Tacit, and its lifecycle
(`start`/`drain`/`serve`) has no tests at all. Adding a second socket and event
hooks widens that untested boundary and adds local-file permission and
untrusted-input surface.

## Recommended action
Add tests first (RED) for the event model, bus, socket permissions, line cap,
peer-cred logging and live-socket guard before wiring any emission site. Keep
the legacy `.socket2.sock` and all existing control replies byte-compatible
except for the additive `code` field. Proceed to `plan-work`.
