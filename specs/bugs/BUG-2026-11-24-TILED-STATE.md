---
bug_id: BUG-2026-11-24-TILED-STATE
status: open
severity: medium
scope: wayland
title: Tiled windows lack xdg tiled states, so winit clients override the frame size
---

# BUG-2026-11-24-TILED-STATE — Tiled windows lack xdg tiled states, so winit clients override the frame size

**Status:** open (follow-up to `BUG-2026-11-24-SIZE-NEGOTIATION`)
**Severity:** medium (fixed-size clients render via compositor stretch instead of
resizing natively; input and damage need inverse mapping)
**Reported by:** operator observation: "but when i open deck craft in hyprland it opens full size"
**Discovered in:** source comparison against Hyprland `v0.57.0-87-g0767f6347` and
winit 0.30.13; no live protocol trace captured yet.

**Security impact: NONE.** Adding protocol state bits only tells clients how the
compositor placed them; no new input, privilege, or data path is introduced.

## Problem

DeckCraft/GridCraft (winit/egui) map at a fixed 1440x900 under eswm even when
their tiled frame is much larger. The merged stretch path makes the content fill
the frame, but the client still believes it is 1440x900: rendering is scaled,
damage and input regions must be inverse-mapped, and text can soften.

The same apps opened in Hyprland fill the frame natively. Expected behavior: a
tiled client receives tiled xdg states, treats the compositor's configured size
as authoritative, and renders at frame size.

Reproduce: on a TTY, tile deckcraft in a frame above 1440x900, run with
`RUST_LOG=debug`, and observe the client's committed buffer staying 1440x900.

## Root Cause Analysis

### Phase 1 — Reproduce

Live eswm logs from 2026-11-23 19:51Z show correctly paced configures to
1896x1078 and three resends, yet the surface commits stayed at 1440x900.
DeckCraft's own log shows `winit::Window::request_inner_size 1440x900` at startup
and egui-winit's `Failed to set window size` line. Hyprland does not reproduce
the undersized surface.

### Phase 2 — Isolate

The configure path in the Wayland integration sets only `Activated`,
`Maximized`, and `Fullscreen` xdg toplevel states. No `Tiled*` state is ever set,
for any placement. The window manager records `mode_sent` as only the
presentation variant, so a placement-only change (floating ↔ tiled) with an
unchanged rectangle does not even trigger a new configure.

winit 0.30.13's Wayland backend honors `request_inner_size()` only when the last
configure is "stateless" — defined as not maximized, not fullscreen, and not
tiled. Hyprland's XDG shell protocol code pushes all four
`XDG_TOPLEVEL_STATE_TILED_*` bits unconditionally for xdg-toplevel v2 and newer.
eswm's configures are therefore stateless and winit re-applies the app's own
1440x900 request.

Note: egui-winit's `Failed to set window size` log is misleading. egui-winit
emits it whenever `request_inner_size(...)` returns `Some`, and winit 0.30.13
returns `Some(new_size)` unconditionally. It is not evidence that the request
failed; it is evidence that the request happened.

### Phase 3 — Hypothesize

1. **H1 (chosen): missing tiled state makes winit treat eswm configures as
   advisory.** Falsification: a configure carrying a `Tiled*` bit must make
   `winit::is_stateless()` false, so `request_inner_size()` no longer resizes.
   Source inspection of winit's `state.rs` confirms this.
2. H2: Hyprland stretches undersized surfaces. Falsification: Hyprland's
   `small()` path renders at natural surface size and leaves empty frame space;
   `correctSmallVec()` is dead code because of an inverted
   `!m_fillIgnoreSmall` guard. Rejected.
3. H3: a Hyprland window rule forces the size. Falsification: no Craft-related
   window rules exist in the operator's Hyprland config. Rejected.

### Phase 4 — Verify

Hyprland `src/protocols/XDGShell.cpp` lines 180-184 push all four tiled states
for every v2+ toplevel. winit's `is_stateless()` is
`!(maximized || fullscreen || tiled)` and `request_inner_size()` calls `resize()`
only when that predicate is true. eswm's `apply_layout()` sets no tiled state.
The chain is confirmed at source level; a live `WAYLAND_DEBUG=1` trace under
both compositors would be corroborating evidence but is not required to land the
minimal protocol fix.

Risk level: **Medium** — protocol-visible state change for tiled clients. GTK
applications can render client-side decorations differently when tiled, so the
live sweep must include at least one GTK client. Floating clients must remain
stateless so they can still be resized by the user.

## TDD Fix Plan

1. **RED**: Add a WM-level test asserting that a tiled, normal-presentation
   client advertises tiled xdg state, while floating, maximized, and fullscreen
   clients do not.
   **GREEN**: Extend `ClientMode` with an explicit tiled-state query derived from
   placement and presentation.
   **verify**: `cargo test --bin eswm xdg_state`.

2. **RED**: Add a test asserting that `mode_sent` tracks placement as well as
   presentation, so floating ↔ tiled with an unchanged rectangle still sends a
   configure.
   **GREEN**: Store the full `ClientMode` as the sent-mode marker and update it
   in `apply_layout`.
   **verify**: `cargo test --bin eswm mode_sent`.

3. **RED**: Add a test asserting the configure-state helper sets all four tiled
   bits only for tiled normal windows and keeps maximized/fullscreen/activated
   behavior unchanged.
   **GREEN**: Extract a small pure state-flag helper used by `apply_layout` and
   set/unset `xdg_toplevel::State::Tiled*` from it.
   **verify**: `cargo test --bin eswm xdg_configure_state`.

4. **REFACTOR / VERIFY**: Keep the stretch path as a safety net for clients that
   ignore even tiled configures. Full preflight must be green.

## Acceptance Criteria

- [ ] Tiled normal windows send all four `Tiled*` xdg states.
- [ ] Floating windows send no tiled state and remain user-resizable.
- [ ] Maximized/fullscreen windows keep their existing exclusive state flags.
- [ ] Placement-only transitions send a fresh configure even when the rect is unchanged.
- [ ] Stretch remains active as a fallback for clients that still ignore configures.
- [ ] All new and existing tests pass; clippy and build are green.
- [ ] Live DRM UAT: deckcraft fills the frame natively; a GTK client keeps correct decorations.

## Resolution

**Fixed:** 2026-11-24 (branch `fix/tiled-states`, merge pending)
**Root cause confirmed:** eswm's xdg configures carried no tiled state, so
winit treated them as stateless and re-applied DeckCraft's own 1440x900 request.
**Fix applied:** `ClientMode::sends_tiled_state()` plus a pure
`toplevel_configure_state()` helper now drive all xdg state bits; tiled normal
windows get all four `Tiled*` states, and `mode_sent` tracks full `ClientMode`
so a placement-only change still triggers a configure.
**Hardening added:** the configure state is derived in one pure, tested helper;
the sent-mode marker now includes placement, so the state cannot silently go
stale when a window switches between tiled and floating.
**Evidence:** `cargo clippy --all-targets && cargo test (196 passed) && cargo
build` green on branch (`/tmp/opencode/tiled-state-branch-preflight.log`);
defect-class sweep recorded in
`specs/verifications/generalize-sweep-BUG-2026-11-24-TILED-STATE.json`.
**Commits:** `4af1add` (RED), `bd62d52` (GREEN), `f7180cb` (RED), `fe093ea`
(GREEN), `98b8ec9` (RED), `bfe7243` (GREEN).
**Live DRM UAT:** pending operator — verify deckcraft now fills its frame
natively (no stretch needed) and a GTK client keeps correct CSD behavior. CI
unverified per operator skip precedent.
