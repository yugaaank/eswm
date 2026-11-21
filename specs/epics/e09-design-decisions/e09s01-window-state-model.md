# Story e09s01: Window state model — ClientMode, mapped/minimized, single focus with MRU

**id:** e09s01
**type:** refactor
**risk:** P2
**context:** window model
**status:** pending
**bcps:** 3
**delta:** MODIFIED

## 1. Context

`WindowMode` (`src/wm.rs:15-22`) conflates two independent axes: placement (`Tiled`/`Floating`) and presentation (`Maximized`/`Fullscreen`). Consequences documented in the original story: leaving fullscreen always returns to Tiled; un-maximizing always returns to Floating; `mapped` doubles as "minimized"; `Window.focused` is a bool cleared per output, so two outputs can each have a focused window and `focused_id()` returns the lowest WindowId globally.

This story replaces `WindowMode` with a `Copy` value struct `ClientMode { placement, presentation }` (instantWM-style), splits `mapped` (client has a buffer) from `minimized` (user-hidden), and establishes one global `focused: Option<WindowId>` with MRU history. It does **not** cover toplevel ownership, `fullscreen_request`/`maximize_request`, or window rules — those moved to e09s05. Preserves e08s04's xdg state-flag sending by adapting it to `ClientMode.presentation`.

## 2. Problem

- 72 `WindowMode` usages conflate layout and state; toggles have no history (fullscreen→Tiled, maximized→Floating regardless of prior placement).
- `mapped` is really "not minimized" (set/cleared only by `toggle_minimize`/`focus_window_or_restore`), while popup `mapped` elsewhere means "has buffer" — two meanings for one word.
- `Window.focused` bool + `Workspace.focused` + per-output clearing yields ambiguous global focus.
- `focused_id()` scans all windows and returns the lowest id, wrong with two outputs.

## 3. Goal

A window's placement survives presentation transitions; buffer presence and user-minimization are distinct; there is exactly one focused window globally with MRU history enabling cross-output focus restoration.

## 4. Non-goals

- WM-owned `ToplevelSurface`, first-commit registration, `fullscreen_request`/`maximize_request`, window rules firing with real app_id → **e09s05**.
- Per-output geometry/work area/LayerMap → **e09s02**.
- IPC event stream/Lua config shape → **e09s03/s04**.
- Minimized-window rendering changes.

## 5. Users / Actors

- Operator switching workspaces/outputs and maximizing/unmaximizing/fullscreening windows.
- Tacit backend consuming window state via IPC/Lua snapshots.

## 6. Preconditions

- `main` preflight green; e08 archived (state flag sending via `mode_sent` exists).

## 7. Postconditions

- `ClientMode { placement: Placement{Tiled,Floating}, presentation: Presentation{Normal,Maximized,Fullscreen} }` is `Copy` and the single source for mode.
- Exiting maximized/fullscreen returns to the prior `placement` (no forced Floating/Tiled).
- `Window.mapped` = "client has a committed buffer"; `Window.minimized` = user-hidden; dock entries, hit tests, tiling membership and IPC derive from both.
- `WindowManager.focused: Option<WindowId>` + `focus_history: Vec<WindowId>` (MRU) replace `Window.focused` as authoritative; `focused_id()` is O(1); focus restoration uses MRU filtered by workspace.
- xdg state flags continue to be sent based on `presentation` (adapt `xdg_state_flags_for_mode`).

## 8. Scope

**In scope:** `src/wm.rs` (mode field, mapped/minimized, focus, toggles, arrange, dock, hit tests), `src/main.rs` (state flags, placed_windows, focus/activation), `src/udev.rs` (drag/title mode checks, dock clicks), `src/control.rs` (mode_name/mapped reporting), inline tests across those files.

**Out of scope:** e09s05 concerns, per-output model, IPC/Lua protocol changes beyond preserving existing JSON shape.

## 9. Zoom-out (module purpose / callers / contracts)

- **`src/wm.rs` WindowManager** — owns windows/workspaces/outputs and all state transitions. Callers: `apply_layout`, input handlers, IPC/control, Lua, udev render. Contract: `Window` records authoritative state; `arrange_output` produces `target_rect`; focus is unique.
- **`src/main.rs` apply_layout** — builds xdg configures. Callers: every WM action via udev/control. Contract: dedup by `activated_sent`/`mode_sent` (now presentation-derived) and send correct `State::Maximized/Fullscreen` bits.
- **IPC/control snapshot** — reports `mode`, `mapped`, `focused`. Contract: preserve JSON field names/shape for Tacit compatibility.

## 10. Requirements (delta tags)

### MODIFIED: Layout and presentation are independent axes
**Before:** `WindowMode::{Tiled,Floating,Maximized,Fullscreen}` forces a single value; leaving fullscreen returns to Tiled and unmaximizing to Floating.  
**After:** `ClientMode{placement, presentation}`; toggling presentation preserves `placement`, and toggling placement preserves `presentation` where meaningful (test both directions).

### MODIFIED: Buffer presence vs user-minimization separated
**Before:** `mapped` is flipped by `toggle_minimize` and used everywhere as "not minimized".  
**After:** `mapped` means client has a buffer; `minimized` is explicit; dock/hit-test/tiling use `mapped && !minimized`; IPC keeps `mapped` field derived as `has buffer && !minimized` for compatibility.

### MODIFIED: Single global focus with MRU
**Before:** `Window.focused` bool cleared per output; `Workspace.focused`; `focused_id()` scans and returns lowest id.  
**After:** `WindowManager.focused: Option<WindowId>` plus `focus_history: Vec<WindowId>`; `focused_id()` is O(1); restoring a workspace focuses its most-recently-focused visible window; exactly one window is focused globally.

### ADDED: Mode conversion helpers
**Before:** no single place to derive xdg flags or JSON mode name.  
**After:** `ClientMode::xdg_state_flags()` and a compatibility `mode_name()` for IPC, with tests. Reason for depth: keeps protocol/reporting mappings in one tested place as the axes separate.

## 11. UX / API notes

- IPC JSON keeps `"mode"` as a string (`"tiled"|"floating"|"maximized"|"fullscreen"`) derived from placement+presentation for compatibility.
- No Lua API shape change in this story.

## 12. Data / State

- `Window.mode: ClientMode`; `Window.mapped: bool`, `Window.minimized: bool`; remove `Window.focused`.
- `WindowManager.focused: Option<WindowId>`, `focus_history: Vec<WindowId>`; `saved_rect` retained for maximize restore (now restores previous placement).
- `mode_sent` becomes `presentation_sent: Option<Presentation>` (e08s04 marker adapted).

## 13. Interfaces / Contracts

- Preserve public method names where practical (`set_mode`, `toggle_maximized`, `toggle_fullscreen`, `focus_window`, `focused_id`) but signatures may take `ClientMode`/`Presentation`.
- `arrange_output` contract: only mapped, non-minimized, tiled windows enter the tiler; presentation window laid out exclusively (single-window behavior unchanged until a later story).

## 14. Dependencies

- None new. **Slopcheck:** N/A.

## 15. Risks

- 72 call sites + 59 focus usages: large mechanical refactor; mitigate with small TDD steps and full-suite gates after each.
- Changing toggle semantics may break existing tests; update them deliberately with assertions for placement preservation.
- IPC shape must stay compatible with Tacit.

## 16. Out of scope

- e09s05 concerns, per-output model, IPC/Lua protocol, minimized rendering.

## 17. Acceptance / Verification

- `cargo test --bin eswm client_mode` passes (placement survives presentation toggles).
- `cargo test --bin eswm focus_mru` passes (single focus, MRU across outputs/workspaces).
- `cargo test --bin eswm minimized_state` passes (mapped vs minimized, dock/IPC derivation).
- `cargo test --bin eswm xdg_state_flags` passes (adapted to presentation).
- `cargo clippy --all-targets && cargo test --bin eswm && cargo build` green.
- Manual: F11/maximize buttons preserve prior placement; focus follows MRU across outputs.

## Implementation Steps

1. Introduce `Placement`, `Presentation`, `ClientMode` (Copy) + `ClientMode::xdg_state_flags()`/`mode_name()` with pure tests (RED → GREEN). → verify: `cargo test --bin eswm client_mode`
2. Replace `Window.mode: WindowMode` with `mode: ClientMode`; add compatibility helper `mode_matches(mode, &str)`; migrate all `WindowMode::X` matches by semantic intent; update tests. → verify: `cargo build`
3. Split `mapped` from `minimized`; update toggle_minimize, focus_window_or_restore, dock_entries, hit tests, arrange filter, placed_windows, IPC `"mapped"`. Add tests. → verify: `cargo test --bin eswm minimized_state`
4. Rewrite focus transitions: `WindowManager.focused: Option<WindowId>` + `focus_history`; remove `Window.focused`; simplify remove_window/move/focus_workspace; add MRU tests. → verify: `cargo test --bin eswm focus_mru`
5. Adapt `toggle_maximized`/`toggle_fullscreen`/`toggle_window_mode` to preserve placement; adapt `xdg_state_flags_for_mode` + `presentation_sent` to `Presentation`. → verify: `cargo test --bin eswm xdg_state_flags`
6. Run full preflight. → verify: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`
7. Manual UAT on DRM: maximize/fullscreen/restore preserve placement; focus MRU across outputs (automated proxy: tests from steps 1/4/5). → verify: manual (record in verification evidence)

## Risks

- see §15
