# Story e06s01: Popups (initial configure, rendering, input)

**type:** feature  
**risk:** P3  
**context:** input, xdg_popup protocol

## Context

Tacit's shell uses fuzzel for menus/dropdowns, which creates xdg_popup surfaces. eswm currently handles xdg_toplevel but not xdg_popup. Without this, fuzzel menus don't appear or don't receive input.

xdg_popup is a short-lived surface attached to a parent (xdg_toplevel or another popup). It requires:
- Initial configure with position relative to parent
- Grab management (keyboard/pointer grab for modal behavior)
- Correct stacking (above parent, below overlays)
- Dismiss on outside click or explicit close

## Steps

### e06s01t1: Implement xdg_popup handler in State

**RED**: Write a test that creates a popup via the xdg_shell handler and verifies `new_popup` is called with correct parent reference.

**GREEN**: Implement `XdgPopupHandler` trait for `State`:
- `new_popup`: Create popup wrapper, store parent relationship, initialize grab
- `popup_destroyed`: Clean up wrapper, release grab
- `grab`: Handle keyboard/pointer grab for modal popups
- `reposition`: Update popup position when parent moves

**verify**: `cargo test e06s01t1`

### e06s01t2: Wire popup initial configure

**RED**: Write a test that verifies popup receives initial configure with correct position/size within 1 frame of creation.

**GREEN**: In `new_popup`, send `xdg_popup.configure` with:
- Position: xdg_positioner rules resolved against the parent window geometry
  (anchor_rect + anchor + gravity + offset, then flip/slide/resize against the
  work area) — see BUG-2026-11-18T160939; offset alone is not the position
- Size: popup's requested size
- Serial for acknowledgement

**verify**: `cargo test e06s01t2`

### e06s01t3: Render popup surfaces in correct stacking order

**RED**: Write a test that verifies popup renders above its parent toplevel but below overlay layers.

**GREEN**: In render loop (udev.rs), add popup surfaces to elements list:
- After parent toplevel's surface
- Before overlay/top layer surfaces
- Use popup's geometry from LayerMap or direct positioning

**verify**: `cargo test e06s01t3`

### e06s01t4: Forward input to popup when grab active; dismiss on outside click

**RED**: Write a test that verifies pointer/keyboard events route to popup during grab, and click outside dismisses popup.

**GREEN**: 
- Track active grab in State (popup serial + surface)
- In pointer handler: if grab active and click outside popup geometry -> send `xdg_popup.dismissed`
- In keyboard handler: if grab active, forward keys to popup surface
- On dismiss: cleanup grab, destroy popup

**verify**: `cargo test e06s01t4`

### e06s01t5: Integration test: fuzzel menu works end-to-end

**RED**: Write an integration test that spawns fuzzel (or mock popup client) and verifies: menu opens at cursor, accepts keyboard input, dismisses on Escape/outside click.

**GREEN**: Use existing test fixture (UnixStream + insert_client) to simulate fuzzel creating popup. Verify full lifecycle.

**verify**: `cargo test e06s01t5`

## Acceptance Criteria

- [ ] Popup created via xdg_shell gets initial configure with correct position/size
- [ ] Popup renders visually above parent toplevel
- [ ] Keyboard/pointer input routes to popup while grab active
- [ ] Click outside popup geometry dismisses it
- [ ] Popup destruction cleans up grab and parent reference
- [ ] All 5 tasks pass; cargo test passes; preflight green

Status (2026-11-18): implemented and unit-tested (positioner math, hit-testing);
runtime acceptance still needs a TTY session — see BUG-2026-11-18T160939.

## References

- specs/epics/e06-input-focus/e06s01-tasks.yaml: task breakdown
- notes/tacit-integration.md: fuzzel popup requirements
- src/main.rs: XdgShellHandler, existing xdg_toplevel handling
- Smithay xdg_shell::popup docs