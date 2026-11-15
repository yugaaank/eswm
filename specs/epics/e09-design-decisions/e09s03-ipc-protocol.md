### Story e09s03: IPC protocol: events stream, subscribe semantics, security

**type:** feat  
**risk:** P3  
**context:** ipc  

**Context:** Tacit uses ~20 Hyprland events (open, close, active window; workspace;
monitor added/removed; open/close layer; activelayout, configreloaded, tacitgrip).
Without event stream it must poll state. Decide subscribe semantics, ordering,
coalescing, add hello with version plus error codes. Security: any process running
as user can drive every window; set socket to 0600; cap line length; log SO_PEERCRED;
don't delete live instance's socket on start; keep exec/config writes off socket.

## Steps

1. Add event stream with subscribe semantics to control socket -> verify: `cargo build`
2. Hello message with version + error codes -> verify: `cargo build`
3. Socket permissions 0600 -> verify: `cargo build`
4. Line length cap -> verify: `cargo build`
5. Log SO_PEERCRED -> verify: `cargo build`
6. Don't delete live instance's socket on start -> verify: `cargo build`
7. Keep exec and config writes off socket -> verify: `cargo build`

## Verification Script

1. Socket file mode is 0600.
2. Client subscribes to events; receives open/close/active window events.
3. Hello message contains version.

## Out of scope

- wp_security_context_v1 (future)

## Risks

- Changing socket permissions breaks existing configs expecting world-readable.

---
