# BUG-2026-11-19T235000: titlebar maximize button does not resize the client

## Problem

Clicking the maximise button in a window's server-side titlebar makes the
window *frame* (borders + titlebar) grow to the maximised geometry, but the
client surface stays at its previous size inside that frame. The client is
never told to resize.

The `Super` + maximize keybind works, which is the clue: same WM call, different
call site.

## Root Cause Analysis

`src/udev.rs` pointer-button handler:

- keybind path (`"toggle_maximized"` arm, ~line 1038): `wm.toggle_maximized(id)`
  **then** `crate::apply_layout(state)`.
- titlebar button path (~line 1445): `self.wm.toggle_maximized(id)` and
  `toggle_minimize` / `remove_window`, with **no** `apply_layout` call.

`apply_layout` is the only place that:

1. sends `xdg_toplevel.configure` with the new `target_rect` size
   (`toplevel.with_pending_state` + `send_pending_configure`), and
2. calls `wm.set_rect(id, target)`, which is what marks the geometry as applied.

Without it, `toggle_maximized` still updates `target_rect` (via
`set_mode` → `arrange_output` → `set_target_rect`). Rendering reads
`target_rect` (`placed_windows`), so the next redraw — triggered by pointer
motion around the click — paints the maximised *frame* around a client that is
still at its old buffer size. Exactly the reported symptom.

Secondary defect in the same call site: no redraw request. The frame only
repaints because the click is preceded by pointer motion. A keyboard-less,
motion-less trigger would leave the stale frame on screen.

Security impact: none. Visual/behavioural defect only.

Risk level: Low. One call site, no geometry math changes.

## TDD Fix Plan

1. **RED**: Unit test `titlebar_button_at` (pure function) maps x positions to
   Close / Maximize / Minimize / None for a given rect + titlebar height.
   **GREEN**: Extract the inline offset-from-right arithmetic into
   `wm::TitlebarButton` + `wm::titlebar_button_at` and call it from the handler.
   **verify**: `cargo test titlebar_button_at`

2. **RED**: `titlebar_maximize_click_configures_client_and_requests_redraw`
   clears `redraw_needed`, calls the extracted `udev::apply_titlebar_action`
   with `Maximize`, and asserts the window is `Maximized` **and** that layout was
   applied (redraw requested). Fails before the fix because the action path never
   runs `apply_layout`.
   **GREEN**: `apply_titlebar_action` performs the WM call then
   `crate::apply_layout(state)` and sets `redraw_needed`.
   **verify**: `cargo test titlebar_`

3. **GREEN**: route the pointer-button titlebar branch through
   `apply_titlebar_action` so close / maximize / minimize all apply layout.
   **verify**: `cargo clippy && cargo test && cargo build`

## Verification

- RED: `titlebar_maximize_click_configures_client_and_requests_redraw` failed
  with "titlebar action must apply layout ... client is never configured".
- GREEN after adding `apply_layout` to `apply_titlebar_action`:
  `cargo test titlebar` -> 4 passed (2 new + 2 pre-existing).
- Full suite: `cargo test` -> 86 passed, 0 failed.
- `cargo clippy --all-targets` -> warning-only, no errors.
- `cargo build` -> green.

Manual check still required on the DRM session: click the titlebar maximize
button and confirm the application window (not just its frame) resizes, then
restores to its previous size on the second click.

## Second defect: restore does not shrink the client

Same class, opposite direction. `toggle_maximized`'s restore branch set both
`target_rect` and `rect` to the saved geometry. `apply_layout` uses
`rect != target_rect` as its "configure is due" test, so the restore looked
already applied and sent no configure: the frame shrank, the client stayed at
the maximized size.

Fix: the restore branch sets `target_rect` only, leaving the stale `rect` as the
pending-change marker. Test
`toggle_maximized_restore_forces_size_configure` (RED: rect == target after
restore; GREEN after fix). Full suite 87 passed.
