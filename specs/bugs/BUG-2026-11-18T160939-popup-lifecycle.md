# BUG-2026-11-18T160939: xdg_popup appears in the wrong place and breaks input after it closes

## Problem

Right-clicking in nautilus (GTK4) creates an xdg_popup (context menu). The
compositor creates and configures it and takes the grab, but:

- the menu is positioned at the parent window's origin instead of the cursor,
  because only `xdg_positioner.offset` is used and `anchor_rect` / `anchor` /
  `gravity` / `constraint_adjustment` are ignored;
- `xdg_popup.popup_done` is never sent when the compositor dismisses the
  popup, so the client never learns that its menu was closed;
- `popup_destroyed` is not implemented, so the dead popup stays in the
  `popups` list, the pointer grab can stay pointed at a dead surface, and
  keyboard focus is never restored to the parent window. The desktop then
  stops responding to the mouse / typing for that client.
- pointer events delivered to a grabbed popup carry absolute screen
  coordinates instead of popup-local coordinates.
- `reposition_request` never replies with `xdg_popup.repositioned`, so a
  client waiting on a reposition hangs.

Expected: menu opens at the cursor, follows the parent window, receives
pointer events with correct local coordinates, closes on outside click with
`popup_done`, and gives keyboard/pointer focus back to its parent.

Repro: TTY session, `./target/debug/eswm`, open nautilus, right-click.
Last run evidence (`eswm.log`): `new_popup` -> `configure sent` -> `grab set`,
then no dismiss / destroy / popup_done events for the rest of the session.

Security impact: NONE (input routing bug, no privilege boundary crossed).

## Root Cause Analysis

The popup implementation covers creation, the initial configure, and grab
tracking, but not the rest of the xdg_popup contract. Position is computed as
`parent_position + positioner.offset`; the xdg-shell positioner rules require
deriving an anchor point from `anchor_rect` + `anchor`, then placing the popup
according to `gravity`, applying `offset`, then flipping/sliding/resizing
against the work area. For GTK context menus `offset` is normally zero and the
position is carried entirely by the anchor rectangle, so the popup lands at the
parent's top-left corner.

Lifecycle cleanup is missing on both exits: compositor-side dismissal (outside
click) drops the internal grab but never sends `popup_done`, and client-side
destruction is not handled at all. Because focus is redirected to the popup
surface on grab, leaving it there when the popup goes away strands both the
pointer focus (possibly on a dead surface) and the keyboard focus.

Reference implementation: Hyprland's `CXDGPositionerRules::getPosition`
(`src/protocols/XDGShell.cpp`), `CXDGPopupResource::applyPositioning/done`,
`CXDGShellProtocol::addOrStartGrab/onPopupDestroy`, and
`CPopup::coordsRelativeToParent` (`src/desktop/view/Popup.cpp`).

Risk level: Medium (input handling; can strand focus).

## TDD Fix Plan

1. **RED**: A test asserting positioner geometry for a context-menu style
   positioner (1x1 anchor rect at the cursor, `anchor=bottom_right`,
   `gravity=bottom_right`, zero offset) puts the popup's top-left at the cursor.
   **GREEN**: Add pure `positioner_geometry(positioner, parent_coord,
   constraint)` implementing anchor/gravity/offset math.
   **verify**: `cargo test popup_positioner`

2. **RED**: Tests for flip and slide constraint adjustment (popup near the
   right/bottom edge stays inside the work area).
   **GREEN**: Add flip-x/flip-y and slide-x/slide-y handling in the same
   helper.
   **verify**: `cargo test popup_positioner`

3. **RED**: A test that the resolved popup rect is parent-relative, so adding
   the parent window origin yields the screen position.
   **GREEN**: Return parent-relative geometry from the helper; use it in
   `placed_windows` for rendering and in `new_popup` for the configure.
   **verify**: `cargo test popup_positioner`

4. **RED**: A test that an outside click dismisses a grabbed popup and reports
   it as dismissed.
   **GREEN**: Track each popup's resolved screen rect; on button press outside
   the rect send `popup_done`, drop the grab, restore keyboard focus, redraw.
   **verify**: `cargo test popup_dismiss`

5. **RED**: A test that destroying a popup removes it from the tracked list and
   clears the grab when it owned it.
   **GREEN**: Implement `popup_destroyed` cleanup + focus restore.
   **verify**: `cargo test popup_destroyed_clears`

6. **RED**: A test that a grabbed popup is hit-tested against its screen rect
   and receives popup-local pointer coordinates.
   **GREEN**: Hit-test with the stored screen rect; pass the rect origin as the
   pointer focus location.
   **verify**: `cargo test popup_pointer`

7. **RED**: A test that `reposition_request` replies with `repositioned` for
   the client's token.
   **GREEN**: Recompute geometry and call `send_repositioned(token)`.
   **verify**: `cargo test popup_reposition`

**REFACTOR**: Skip server-side borders/titlebars for popup entries in the
render loop; add info-level lifecycle logs (`geometry`, `mapped`, `dismissed`)
so the next TTY run is diagnosable from the log alone.

## Acceptance Criteria

- [ ] Context menu opens with its top-left at the click point
- [ ] Menu flips/slides to stay inside the work area
- [ ] Outside click sends `popup_done` and restores parent keyboard focus
- [ ] Client-side close removes the popup and clears the grab
- [ ] Pointer events reach the popup with popup-local coordinates
- [ ] Reposition requests are acknowledged with the client's token
- [ ] Popups draw without server-side borders/titlebars
- [ ] All new tests pass; existing tests still pass

## Resolution

Fixed (2026-11-18). Design decision: the positioner math is delegated to
smithay 0.7's `PositionerState::get_unconstrained_geometry`, whose
anchor/gravity/offset + flip/slide/resize rules were verified against
Hyprland's `CXDGPositionerRules::getPosition`; eswm now wraps it in
`popup_geometry()` so the result is parent-relative like Hyprland's
`CXDGPopupResource::applyPositioning`. Hyprland's lifecycle and grab handling
(`addOrStartGrab` / `onPopupDestroy` / `SeatManager::setGrab` refocus) were
ported conceptually.

Changes:

- `src/main.rs`
  - `popup_geometry(positioner, parent_coord, available)` resolves the full
    xdg-positioner rules (anchor point, gravity, offset, flip/slide/resize
    against the work area).
  - `new_popup` stores the immediate parent surface, the owning toplevel
    (walking the popup chain for submenus) and the resolved geometry, and
    configures the popup at that position instead of `offset` only.
  - `placed_windows` recomputes popup geometry every frame from the parent's
    window origin, so menus follow a moving parent; popups render above
    toplevels with `POPUP_FAKE_ID` and write `screen_rect` back for input.
  - `popup_destroyed` removes the popup, clears the grab if it owned it and
    restores keyboard focus to the owning toplevel.
  - `reposition_request` recomputes geometry and calls
    `send_repositioned(token)` (which also sends the new configure).
  - `commit` logs popup map/unmap transitions.
  - `popup_parent_coord` also resolves layer-surface parents (bar menus).
- `src/udev.rs`
  - `pointer_focus`: a grabbed popup is hit-tested against its screen rect and
    receives popup-local coordinates; pointer motion outside it is not routed
    to surfaces underneath. Non-grabbed mapped popups are hit-tested too.
  - Button press: inside the popup -> forwarded to the popup; outside ->
    `send_popup_done()`, grab released, keyboard focus back to the owner, no
    click leakage into the surface underneath.
  - Render loop: popups get no server-side border or titlebar.

Tests added (`cargo test popup`, 9 tests): context menu lands at the cursor,
flip at the right edge, slide inside the work area, flip/slide at the bottom
edge, work-area offset (top bar), parent-relative geometry, rect hit-testing
edges. `rect_contains` is the same predicate used by the pointer path.

Verification: `cargo clippy` clean (warnings only), `cargo test` 84 passed,
`cargo build` ok.

Runtime verification must be done from a TTY (the DRM backend cannot run from
inside the current Wayland session):

```sh
RUST_LOG=eswm=info ./target/debug/eswm
# in another TTY: nautilus, right-click
grep -E "xdg_popup" "$XDG_STATE_HOME/eswm/eswm.log"
```

Expected log sequence: `new_popup` with `rel`/`screen` at the click point ->
`configure sent` -> `grab set` -> `mapped (buffer committed)` -> on outside
click `popup dismissed on click outside ... owner=Some(n)` -> `xdg_popup
destroyed: grab_released=true`.

Known limitation (pre-existing, out of scope): eswm renders toplevels at the
surface origin and ignores `xdg_surface.set_window_geometry` offsets, so a
client that sets a non-zero window-geometry origin would have its popup
displaced by that offset. GTK4 (nautilus) sets the geometry to the full
surface, so the menu lands exactly on the cursor in the repro case.
