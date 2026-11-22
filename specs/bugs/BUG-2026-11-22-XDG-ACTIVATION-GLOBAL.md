---
bug_id: BUG-2026-11-22-XDG-ACTIVATION-GLOBAL
status: open
severity: low
priority: p3
scope: wayland
title: Clients cannot mint xdg activation tokens
---

# BUG-2026-11-22-XDG-ACTIVATION-GLOBAL — Clients cannot mint xdg activation tokens

**Status:** open — RCA verified, fix pending
**Discovered:** log analysis of `~/.local/state/eswm/spawn.log` (local time, 2026-11-22)
**Security impact: LOW.** Registering `xdg_activation_v1` adds a local protocol surface; activation requests must ignore stale tokens and unknown surfaces so a client cannot steal focus with a replayed token. No network, credentials, or user-data exposure.

## Problem

Clients repeatedly report from the launcher side:

```
Unable to mint xdg activation token to launch "ai.storyteller.deckcraft"
Unable to mint xdg activation token to launch "helium.desktop"
Unable to mint xdg activation token to launch "org.gnome.Nautilus.desktop"
```

23 occurrences between 12:03 and 14:24 IST across deckcraft, Helium, Lutris,
Nautilus, tlauncher and Gridcraft/lightcraft. Expected: clients can ask eswm for
an activation token and use it when their window maps, so launches carry focus
semantics.

## Root Cause Analysis (4-phase, verified)

### Phase 1 — Reproduce

On an eswm DRM session, launch any desktop entry that uses xdg activation
(deckcraft and Helium do). Then:

```sh
grep -i 'activation token' ~/.local/state/eswm/spawn.log
```

Observed 23 `Unable to mint xdg activation token` lines. Additional static
reproduction: `grep -rn 'xdg_activation' src/` returns nothing — the protocol is
never mentioned in the compositor.

### Phase 2 — Isolate

`new_state()` in `src/main.rs:1044-1141` constructs every other protocol state
(`CompositorState`, `ShmState`, `ViewporterState`, `WlrLayerShellState`,
`XdgDecorationState`, `DataDeviceState`, `PrimarySelectionState`,
`DataControlState`) and `State` (`src/main.rs:905`) stores them for their global
lifetime. There is no `XdgActivationState` field, no `XdgActivationState::new`
call, and no `delegate_xdg_activation!(State)` at `src/main.rs:1488-1499`.

Smithay 0.7 already ships the complete implementation:
`wayland/xdg_activation/mod.rs:180-193` creates the `xdg_activation_v1` global
at version 1, and `dispatch.rs:34-166` handles token creation, `done`, and
`activate`. The only missing work is wiring it into eswm's `State`.

### Phase 3 — Hypothesize (ranked, with falsification)

| # | Hypothesis | Falsification test | Result |
|---|-----------|--------------------|--------|
| H1 | The `xdg_activation_v1` global is never registered, so clients have no way to mint a token | Connect a Wayland client, enumerate `wl_registry` globals, look for `xdg_activation_v1` | **Confirmed** — source has no registration; RED test will assert absence |
| H2 | The launchers are at fault for requesting a protocol eswm does not support | Check whether independent apps (Nautilus, Lutris, deckcraft, Helium, tlauncher) all emit the same warning | **Falsified** — 23 warnings across unrelated toolkits; the protocol is standard and smithay supports it |
| H3 | The global is registered but at an unsupported version | grep for any `create_global`/`XdgActivationState::new` with activation interface | **Falsified** — no registration of any version exists |

### Phase 4 — Verify

- `grep -rn 'xdg_activation' src/` → zero matches; registry state and delegate
  block both lack activation.
- smithay 0.7 source confirms `XdgActivationState::new::<D>(&dh)` is sufficient
  to advertise the global, and `delegate_xdg_activation!(State)` supplies the
  `Dispatch`/`GlobalDispatch` impls.
- Client-side warning is emitted by the launcher when the global is missing;
  it is not an eswm log line, which is why eswm's own log looked clean.
- **Single verified root cause:** eswm never registers the `xdg_activation_v1`
  global. Fix must register it, delegate its dispatch, and handle activation by
  focusing the surface's window. A stale-token guard prevents replay.

**Risk level:** Low. Adds one standard global to the local display. The handler
only focuses an existing, alive toplevel; it never creates or destroys windows.

## Fix approach

Minimal, using smithay's implementation rather than reimplementing the protocol:

1. Add `activation_state: XdgActivationState` to `State`, create it in
   `new_state()` with `XdgActivationState::new::<State>(dh)`, and add
   `delegate_xdg_activation!(State)`.
2. Implement `XdgActivationHandler::activation_state()` and
   `request_activation()`. The latter ignores tokens older than 10 s, resolves
   the surfaced toplevel through `window_id_of`, focuses it via
   `focus_window_or_restore`, and syncs keyboard focus + redraw. The token is
   removed from the pool after use.
3. Keep the decision logic pure and testable: `activation_token_is_fresh(age)`
   for the replay guard.

## TDD Fix Plan

1. **RED**: Client-side registry test asserting `xdg_activation_v1` is advertised
   by `new_state()`.
   **GREEN**: Register the global and delegate dispatch.
   **verify**: `cargo test --bin eswm xdg_activation`

2. **RED**: Pure boundary test for token freshness (0 s/9 s accepted, 11 s
   rejected).
   **GREEN**: Add `activation_token_is_fresh` and use it in `request_activation`.
   **verify**: `cargo test --bin eswm activation_token_is_fresh`

3. **GREEN (integration)**: Implement `XdgActivationHandler` so a valid token
   focuses the requesting surface's window.
   **verify**: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`

**REFACTOR**: No further cleanup expected; keep the handler beside the other
protocol implementations in `main.rs`.

## Acceptance criteria

- [ ] A client enumerating `wl_registry` sees `xdg_activation_v1` at version 1.
- [ ] A client can commit an activation token and receive `token.done`.
- [ ] `request_activation` focuses the toplevel matching the activated surface.
- [ ] Tokens older than 10 s are ignored (replay guard).
- [ ] Launching deckcraft/Helium no longer logs the warning in `spawn.log`
      (operator UAT — launcher runs in the live session).
- [ ] `cargo clippy --all-targets`, `cargo test --bin eswm`, `cargo build` green.

## Resolution

**Fixed:** 2026-11-22
**Root cause confirmed:** eswm never constructed `XdgActivationState`, so the `xdg_activation_v1` global did not exist and clients had no way to mint an activation token.
**Fix applied:** Added `activation_state` to `State`, created it in `new_state()`, added `delegate_xdg_activation!(State)`, and implemented `XdgActivationHandler` to focus the toplevel matching the activated surface. Tokens older than 10 s and tokens for untracked surfaces are rejected.
**Hardening added:** A pure `activation_token_is_fresh(age)` replay guard, tested at 0 s/9 s/11 s; tokens are removed from the pool on use so they cannot be replayed.
**Evidence:** `cargo test --bin eswm activation` → 3 passed; `cargo test` → 176 passed; `cargo clippy --all-targets` and `cargo build` clean. A real Wayland client test proves the global is advertised and that `token.done` arrives. Defect-class sweep recorded in `specs/verifications/generalize-sweep-BUG-2026-11-22-XDG-ACTIVATION-GLOBAL.json`.
**Commit:** `f97bc08 fix(wayland): advertise xdg_activation_v1 and honor activation requests`
**Remaining:** Confirm in the live launchers that `spawn.log` no longer logs `Unable to mint xdg activation token` (operator UAT; launchers run in the DRM session).
