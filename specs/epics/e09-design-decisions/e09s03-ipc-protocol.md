# Story e09s03: IPC protocol — event stream, subscribe semantics, socket security

**id:** e09s03
**type:** feat
**risk:** P1 (revised from P3: shared IPC interface consumed by Tacit, no lifecycle tests — see `specs/IMPACT_LATEST.md`)
**context:** ipc
**status:** completed
**bcps:** 2
**delta:** ADDED + MODIFIED

## 1. Context

eswm's native control channel (`src/control.rs`) answers requests but cannot push
anything: Tacit polls `state` to notice window, workspace, output and layer
changes. A legacy Hyprland-compatibility socket (`.socket2.sock`) emits only
`configreloaded>>` and is not the native contract (`notes/tacit-integration.md`).

Operator decisions on 2026-11-26:

1. **Dedicated NDJSON event socket** — `$XDG_RUNTIME_DIR/eswm-<name>.events.sock`.
   Connecting is subscribing; the command socket stays request/reply.
2. **Per-tick buffer with coalescing** — queue during the loop, dedupe by
   `(kind, entity id)` keeping the last value, flush once per tick with a
   monotonic `seq`.
3. **Compare with Hyprland, then derive an original format.** Take the event
   *functions* Hyprland exposes, not its wire format. The eswm format below is
   a three-field NDJSON object with flat snake_case names; it is not
   `>>name>>data` and it is not Hyprland's event-name spelling.

Research source: Hyprland's current Lua event list
(https://wiki.hypr.land/configuring/core/advanced-configuration/events/, page
last updated 2026-11-24) — 36 event functions.

## 2. Problem

- No push channel: every state change is invisible until a client polls.
- Existing socket has no explicit `0600`, no line cap, no peer-credential log,
  and unlinks any socket at its path — including a live instance's socket.
- Errors are free-form strings with no stable numeric code, so clients must
  match English text.
- `es.config.reload` is a silent no-op, so a reload cannot be announced.

## 3. Goal

A 0600 event socket that delivers coalesced, ordered, self-describing NDJSON
frames for every eswm state transition we actually have, plus a hardened
command socket whose replies gain an additive numeric error code.

## 4. Non-goals

- Reproducing Hyprland's socket path, `>>event>>data` framing, dotted event
  names, or its Hyprland-specific event payloads.
- Exposing raw keyboard input as events: e04 fixed key data leaking to a
  world-readable log; key contents stay off IPC.
- Implementing rules, submaps, pinning, urgency, screencopy, special
  workspaces, cross-output workspace moves, or config reloading itself.

## 5. Users / Actors
- **Tacit** — the shell; subscribes once and reacts to frames instead of polling.
- **Operator / scripts** — `socat` or `nc -U` inspection of the event stream.
- **Local unprivileged processes** — must not be able to drive windows through
  a permissive socket or exhaust the loop with unbounded lines.

## 6. Preconditions
- Preflight green (`cargo clippy --all-targets && cargo test && cargo build`).
- e09s02 landed: per-output geometry and `focused_output_id()` exist.

## 7. Postconditions
- Two sockets in `$XDG_RUNTIME_DIR`, both mode `0600`, both refusing to unlink
  a live peer.
- Subscribers receive `hello`, then coalesced frames with gap-free `seq`.
- Every command reply keeps its `id`/`ok`/`error` shape and gains `code`.
- Single-output, single-client behaviour of `handle_request` is unchanged.

## 8. Scope
**In scope:** `src/control.rs` (event model, bus, both sockets, hardening, error
codes), `src/main.rs` and `src/udev.rs` (emission sites + per-tick flush),
`src/ipc.rs` (bridge legacy `configreloaded` into the bus without removing the
legacy stream), inline tests.

**Out of scope:** see §4 and §16.

## 9. Zoom-out (module purpose / callers / contracts)
- **Purpose:** `control.rs` owns the local control channel — bind, accept,
  parse, queue onto the compositor loop, reply; no socket thread touches `State`.
- **Callers:** `src/main.rs` (`mod`, `State.control`, 10 inline tests),
  `src/udev.rs` (`Control::start`, `control::drain`), Tacit over Unix sockets.
- **Contracts to preserve:** one JSON request line → one reply line; replies
  echo `id`; `primary` stays a deprecated alias of `focused`; a failed socket
  bind only disables control, never aborts the compositor; legacy
  `.socket2.sock` keeps emitting `configreloaded>>`.

## 10. Requirements (delta tags)

### ADDED: Dedicated event socket with subscribe-on-connect
New `$XDG_RUNTIME_DIR/eswm-<name>.events.sock`, mode 0600. First frame is
`{"seq":0,"kind":"hello","data":{"version":1,"events":[...]}}`. Connecting
subscribes to every event kind; closing unsubscribes.

### ADDED: Simple original frame format
One JSON object per line: `{"seq":<u64>,"kind":"<name>","data":{...}}`.
`hello` is always `seq:0`; emitted frames start at 1 at compositor startup and
increase by one per emitted frame. The counter is compositor-global, not
per-subscriber, so a client that connects after earlier ticks joins at the
current value and its first frame after `hello` can be greater than 1. Frames
are identical for every subscriber; no client sees a gap in the frames it
receives. No Hyprland prefixes, no dotted event names, no `>>` separators.

### ADDED: Per-tick coalescing
Events queue during a compositor tick. At flush, frames with the same
`(kind, data.id)` collapse to the last one, preserving first-occurrence order;
kinds without an entity id collapse by kind. `seq` is assigned at flush, so
subscribers never see gaps or duplicates.

### ADDED: eswm event inventory (from Hyprland functions, eswm-owned names)
| Hyprland event | eswm source | e09s03 |
| --- | --- | --- |
| `window.open` / `window.open_early` | `wm.add_window` + mapped commit | `window_open` (one stage; no rules phase) |
| `window.close` / `window.destroy` | `toplevel_destroyed`, `wm.remove_window` | `window_close` |
| `window.active` | `focus_changed`, `wm.focus_window` | `window_active` |
| `window.title` / `window.class` | `sync_window_meta` | `window_title`, `window_class` |
| `window.fullscreen` / `window.minimize` / `window.update_state` | `set_mode`, `toggle_minimize` | `window_mode` |
| `window.move_to_workspace` | `wm.move_window_to_workspace` | `window_workspace` |
| `workspace.active` / `workspace.created` | `wm.focus_workspace`, `add_workspace` | `workspace_active`, `workspace_created` |
| `monitor.added` / `monitor.removed` | `connector_connected` / `disconnected` | `monitor_added`, `monitor_removed` |
| `monitor.focused` / `monitor.layout_changed` | `focused_output_id`, `apply_configured_output_positions` | `monitor_active`, `monitor_layout` |
| `layer.opened` / `layer.closed` | `new_layer_surface`, `layer_destroyed` | `layer_opened`, `layer_closed` |
| `config.reloaded` | `CompCmd::Reload` (legacy `reload`) | `config_reloaded` |
| `hyprland.start` | sockets bound, loop ready | `ready` |
| `window.bell`, `window.kill`, `window.pin`, `window.urgent`, `window.update_rules` | no bell / force-kill / pin / urgency / rules engine | deferred (e09s05 for rules) |
| `workspace.created_early`, `workspace.removed`, `workspace.move_to_monitor`, `workspace.special_active` | no early stage, no removal, cross-output moves rejected, no special workspaces | out of scope |
| `config.props_refreshed`, `config.unload`, `hyprland.shutdown` | no props / Lua teardown / shutdown hook | out of scope |
| `input.keyboard.key`, `keybinds.submap`, `screenshare.state` | keys must stay off IPC (e04), no submaps, no screencopy | out of scope |

### MODIFIED: Socket permissions are explicit
**Before:** `UnixListener::bind` with umask-dependent mode.
**After:** both sockets chmod `0600` immediately after bind; a test reads the
mode from the filesystem.

### MODIFIED: Live instance socket is never deleted
**Before:** `symlink_metadata` + `is_socket()` → unconditional `remove_file()`
at startup, so a second instance deletes a live peer's socket.
**After:** attempt `UnixStream::connect` first; a successful connect means a
live compositor owns the path, so startup returns an error instead of unlinking.

### MODIFIED: Bounded request lines
**Before:** `read_until(b'\n', &mut buf)` into an unbounded `Vec`.
**After:** 64 KiB cap; an oversized line gets `{"id":null,"error":"line too long","code":5}`
and the connection closes.

### MODIFIED: Errors carry a stable code
**Before:** `{"id":7,"error":"unknown command 'x'"}` only.
**After:** add `code` additively: 1 invalid json, 2 unknown command, 3 no such
window, 4 bad args, 5 line too long, 6 compositor timeout, 7 denied.
Existing clients that ignore unknown fields are unaffected.

### ADDED: Peer credentials are logged; exec/config writes stay off IPC
Accept logs `uid`/`pid`/`gid` via `SO_PEERCRED`. `exec`, `config`, `reload`,
`eval` and any config-write command are explicitly denied with code 7, even if
someone later adds a similarly named dispatcher.

## 11. UX / API notes
- Inspect: `socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/eswm-test.events.sock`.
- First line is always `hello`; subscribers must tolerate unknown `kind`
  values and skip them (forward compatibility).
- `seq` is global, so a late subscriber may see `seq:0` followed by e.g.
  `seq:23`. Treat the first received frame as that client's baseline; do not
  reset the counter locally.
- Samples: `{"seq":4,"kind":"window_active","data":{"id":42}}`,
  `{"seq":5,"kind":"monitor_added","data":{"id":2,"name":"HDMI-A-1"}}`.

## 12. Data / State
- `EventKind` (flat snake_case enum), `Event { seq, kind, data: Value }`.
- `EventBus`: `Vec<Event>` pending per tick and `next_seq: u64`; no global
  mutable state. It lives on `WindowManager`, where the transitions originate.
- `State` owns the `EventStream` sink (like `State.control`) and exposes
  `flush_events` as the single per-tick seam, so the DRM loop drains the bus
  and broadcasts without threading another parameter through handlers.

## 13. Interfaces / Contracts
- Event frames: NDJSON, 0600 dedicated socket, `hello` then numbered frames.
- Command socket: unchanged request/reply plus additive `code`.
- Legacy `.socket2.sock`: unchanged; `configreloaded>>` also queues a
  `config_reloaded` event on the new bus.

## 14. Dependencies
No new crates. `serde_json`, `flume`, `libc`, `tracing` are already present
[OK]; nothing to slopcheck.

## 15. Risks
- A second socket doubles lifecycle/permission surface; mitigated by shared
  helpers for bind/chmod/live-probe and one test per invariant.
- Emission hooks touch many state transitions; mitigated by pure bus tests and
  one domain group per task.
- Coalescing can hide an intermediate state (open then close in one tick);
  accepted — subscribers can always call `state` for the current truth.

## 16. Out of scope
Hyprland wire compatibility, keyboard key events, rules/submaps/pinning/
urgency/screencopy/special workspaces, cross-output workspace moves, config
reload implementation, persistence, event replay/history, multiple filters.

## 17. Acceptance / Verification
- `cargo test --bin eswm event_model`, `event_bus`, `event_socket`,
  `control_hardening`, `control_errors`, `window_events`, `output_events`,
  `layer_events`, `event_stream` all pass with real assertions.
- Both sockets are `0600`; a second instance fails instead of unlinking.
- Two subscribers see identical `seq` frames; a burst of title changes
  coalesces to the last title in one tick.
- Manual live-DRM UAT recorded in `specs/verifications/e09s03-verify.yaml`.

## Implementation Steps

1. Add `EventKind`/`Event`/frame serialization with RED then GREEN tests → verify: `cargo test --bin eswm event_model`
2. Add the per-tick coalescing `EventBus` with RED then GREEN tests → verify: `cargo test --bin eswm event_bus`
3. Bind the 0600 event socket with hello, broadcast, dead-socket drop and live-peer guard → verify: `cargo test --bin eswm event_socket`
4. Harden both sockets: 0600, 64 KiB line cap, SO_PEERCRED log, denied command guard → verify: `cargo test --bin eswm control_hardening`
5. Add additive numeric error codes to command replies → verify: `cargo test --bin eswm control_errors`
6. Emit window events from window state transitions → verify: `cargo test --bin eswm window_events`
7. Emit workspace and monitor events from output/state transitions → verify: `cargo test --bin eswm output_events`
8. Emit layer and config events; bridge the legacy `configreloaded` emission → verify: `cargo test --bin eswm layer_events`
9. Flush the bus once per DRM tick and prove end-to-end delivery to two subscribers → verify: `cargo test --bin eswm event_stream`
10. Operator live-DRM UAT → verify: `manual: subscribe with socat, exercise the checklist in specs/verifications/e09s03-verify.yaml`

## Risks
- See §15. Any new event kind must be added to the `hello` list and the
  dedupe-key table in the same commit, or subscribers will silently miss it.
