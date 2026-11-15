### Story e02s04: Close protocol and window lifecycle: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** Currently `close_window` removes the window from WM immediately without client acknowledgement. The review model defines `Lifecycle` (Open/CloseRequested/Closing): `request_close()` sets `CloseRequested`; client acknowledges via destroy handler, or timeout forces removal. Close button calls `send_close()` instead of removing directly.

## Steps

1. Add `Lifecycle` (Open/CloseRequested/Closing) to `Window`; add `request_close()` method -> verify: `cargo test --bin eswm wm::tests`
2. Add close timeout handler: if client doesn't acknowledge within timeout, force removal -> verify: `cargo test --bin eswm wm::tests`
3. Update `toplevel_destroyed` to handle client ack; update close button to call `send_close()` -> verify: `cargo test --bin eswm wm::tests`
4. Add unit test: `request_close()` sets `CloseRequested`; ack via destroy removes window -> verify: `cargo test --bin eswm wm::tests`
5. Add unit test: close without ack within timeout forces removal -> verify: `cargo test --bin eswm wm::tests`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all close protocol tests pass.
2. Confirm close button sends close request instead of immediate removal.

## Out of scope

- Timeout duration configuration via Lua

## Risks

- Timeout must be long enough for clients to respond but short enough not to hang the compositor.

---
