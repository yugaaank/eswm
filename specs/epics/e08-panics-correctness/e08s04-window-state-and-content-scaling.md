# Story e08s04: Window state flags and content scaling

**id:** e08s04
**type:** fix
**risk:** P2
**context:** presentation
**status:** pending
**bcps:** 3
**delta:** ADDED

## 1. Context

`apply_layout` sends size and activation state in xdg_toplevel configures but never sends `xdg_toplevel::State::Maximized` / `State::Fullscreen`, so clients cannot tell they are maximized/fullscreen and keep their own decorations/window state. A prior ledger also carried a stale caveat about a compositor-owned viewport `dst` override; investigation for this re-plan found **no such override in the current code** (only the `ViewporterState` protocol global), so there is nothing to delete — the scope is the missing state flags plus verification that conforming clients resize from a plain size configure.

The configure path skips windows when neither rect nor activation changed; a mode-only change (e.g. maximize into an identically-sized rect) could therefore miss its configure. This plan adds a `mode_sent` signal so mode changes are reliably communicated.

## 2. Problem

- `Window` records `mode` but not the last mode sent to the client; `apply_layout` only compares `rect` and `activated_sent` before skipping configure.
- No `Maximized`/`Fullscreen` bits are set/unset in `ToplevelConfigure.states`.
- Consequence: a GTK4/kitty client stays in its "normal" window state when maximized via titlebar or Super+M, even though eswm draws the maximized frame.

## 3. Goal

Send correct xdg window-state flags in every configure, ensure a mode change always triggers a configure even when geometry is unchanged, and verify on a live DRM session that real clients resize into the maximized rect on a plain size configure (no viewport override needed).

## 4. Non-goals

- Fractional scale (`wp_fractional_scale_v1`) — deferred (SCOPE out_of_scope).
- Client-side decoration negotiation changes.
- Re-adding a compositor-owned viewport destination override — protocol-inappropriate; none exists today.

## 5. Users / Actors

- Operator maximizing/fullscreening windows via titlebar buttons or keybinds.
- Wayland clients (kitty, GTK4 apps) that rely on xdg state flags for their own window presentation.

## 6. Preconditions

- `main` preflight green before kickoff.
- e08s01–s03 shipped (configure-on-titlebar fix, z-order, CSD/scale config).

## 7. Postconditions

- A window's configure carries `State::Maximized` when `mode == Maximized`, `State::Fullscreen` when `mode == Fullscreen`, and neither for other modes; `State::Activated` behavior unchanged.
- A mode change with an unchanged rect still produces a configure (`mode_sent` tracking).
- Manual UAT confirms at least two real clients resize into the maximized rect on a plain size configure; no viewport override is present in the code.

## 8. Scope

**In scope**
- `src/wm.rs`: `Window.mode_sent: Option<WindowMode>` field, setter, initialization.
- `src/main.rs`: `apply_layout` sets/unsets xdg state flags based on mode; `needs_configure` includes mode change; update `mode_sent` after configure; pure helper for mode→flags decision with inline tests.
- Manual UAT entry in the verification script (kitty + GTK4 app + one other toolkit on a TTY).
- Spec/task ledger refresh (this re-plan).

**Out of scope**
- Viewport dst override (none exists), fractional scale, CSD negotiation.

## 9. Zoom-out (module purpose / callers / contracts)

- **`src/wm.rs` (WindowManager):** owns window records incl. `mode` and `activated_sent`. **Callers:** `apply_layout` (configure), render/input, IPC. **Contract:** per-window "last sent" markers reflect what the client was told; adding `mode_sent` mirrors `activated_sent`.
- **`src/main.rs` (apply_layout):** builds and sends xdg configures. **Callers:** titlebar actions, keybinds, layout changes, udev frame path. **Contract:** send configure only when something the client cares about changed; never miss a mode change.
- **xdg shell protocol:** `ToplevelConfigure.states` carries `xdg_toplevel::State` bitflags; eswm already uses `st.states.set(xdg_toplevel::State::Activated)` in `apply_layout`, so `State::Maximized`/`State::Fullscreen` are the same API family.

## 10. Requirements (delta tags)

### ADDED: Window tracks last-sent mode
**Before:** `Window` has `mode` and `activated_sent` but no record of which mode was last configured to the client.  
**After:** `Window.mode_sent: Option<WindowMode>` records the last configured mode, initialized to `None`, with a setter used by `apply_layout`.

### MODIFIED: Configure carries xdg window-state flags
**Before:** configures set/unset only `State::Activated`; size and activation changes trigger configures, but mode changes with unchanged geometry are skipped and no maximization/fullscreen flag is ever sent.  
**After:** configures set `State::Maximized` / `State::Fullscreen` for the corresponding WM modes and clear both otherwise; a changed `mode` (tracked via `mode_sent`) forces a configure even if rect/activation are unchanged.

### ADDED: Manual client-resize verification
**Before:** the prior ledger asked to keep or delete a viewport override that does not exist in current code.  
**After:** a pure flag-mapping test covers the protocol behavior automatically; a clearly-labelled manual UAT step (TTY, kitty + GTK4 + one other toolkit) verifies real clients resize into the maximized rect on a plain size configure.

## 11. UX / API notes

- No Lua/IPC API changes.
- Clients may now show their own maximized/fullscreen presentation state; this is the intended protocol behavior.

## 12. Data / State

- New `Window.mode_sent: Option<WindowMode>` (mirrors `activated_sent`). No other persistent state.

## 13. Interfaces / Contracts

- Preserve `apply_layout` signature and configure-suppression behavior for truly unchanged windows (perf guard from e05s04).
- `WindowMode` variants: `Tiled`, `Floating`, `Maximized`, `Fullscreen` (src/wm.rs).
- xdg state API evidence: existing `st.states.set(xdg_toplevel::State::Activated)` / `unset` in `src/main.rs:1197-1199` (smithay/wayland-protocols `ToplevelConfigure.states` bitflags).

## 14. Dependencies

- No new external packages. **Slopcheck:** N/A.

## 15. Risks

- **Client behavior change:** some clients may redraw on state flags; verified by manual UAT.
- **Configure storms:** adding mode to the change check must not reintroduce per-frame configures — `mode_sent` only changes on actual mode transitions (Boy Scout: preserve the rect/activation suppression).
- **State flag mismatch:** a stale flag could persist if configure is skipped; `mode_sent` check ensures a transition always sends.

## 16. Out of scope

- Viewport override (none exists), fractional scale, CSD negotiation, XWayland-specific behavior.

## 17. Acceptance / Verification

- `cargo test --bin eswm xdg_state_flags` passes (mode→flag mapping incl. clearing).
- `cargo test --bin eswm apply_layout` passes (existing suite, regression guard).
- `cargo clippy --all-targets && cargo test --bin eswm && cargo build` passes.
- Manual UAT: kitty, GTK4 app, one other toolkit resize into maximized rect from titlebar and Super+M; no viewport override in code (`grep -r 'ViewportCachedState.dst' src/` returns nothing).

## Implementation Steps

1. Add pure helper `xdg_state_flags_for_mode(mode) -> (maximized: bool, fullscreen: bool)` with inline tests for all four modes (RED → GREEN). → verify: `cargo test --bin eswm xdg_state_flags`
2. Add `Window.mode_sent: Option<WindowMode>` + `set_mode_sent`, initialize to None at window creation. → verify: `cargo build`
3. In `apply_layout`, include mode change in the configure-needed check; set/unset `State::Maximized`/`State::Fullscreen` via the helper; call `set_mode_sent(id, mode)` after configure. → verify: `cargo build`
4. Confirm no viewport `dst` override exists in source; document in verification evidence. → verify: `! grep -rq 'ViewportCachedState' src/`
5. Run full preflight for the story. → verify: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`
6. Manual UAT on a live DRM session (kitty + GTK4 + one other toolkit; titlebar and Super+M maximize) — automated proxy is the flag-mapping test; manual result recorded in verification evidence. → verify: `manual: see Verification Script step 6`

## Verification Script (Step-by-Step)

1. `cargo test --bin eswm xdg_state_flags` — flag mapping correct for Tiled/Floating/Maximized/Fullscreen.
2. `cargo test --bin eswm apply_layout` — existing layout suite still green.
3. `! grep -rq 'ViewportCachedState' src/` — no viewport override reintroduced.
4. `cargo clippy --all-targets && cargo test --bin eswm && cargo build` — all green.
5. On a TTY, maximize a kitty window via titlebar X-adjacent maximize button and via Super+M; confirm content fills the rect and the window reports maximized state.
6. Repeat with a GTK4 app and one other toolkit; record results in verification evidence.

## Risks

- see §15
