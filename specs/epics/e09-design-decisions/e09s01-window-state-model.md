### Story e09s01: Window state model: split WindowMode into layout + state flags

**type:** refactor  
**risk:** P3  
**context:** window model  

**Context:** WindowMode mixes layout (tiled/floating) with state (maximized/fullscreen):
leaving fullscreen always returns to Tiled; maximized then restored comes back floating;
only first maximized/fullscreen window laid out; mapped also means minimized;
focus is bool per window cleared per output; two outputs give two focused windows;
remove_window clears focus on whole output then tries to refocus removed window's
workspace; clients never learn state (fullscreen/maximized never sent as xdg states);
fullscreen_request/maximize_request not handled (F11 in browser does nothing);
window rules can't fire (new_toplevel passes "" as app_id before client sets one;
add_window_rule only called from tests).

## Steps

1. Split mode into layout: Tiled | Floating plus fullscreen, maximized, minimized,
   mapped flags where mapped means "has buffer"; save floating rect -> verify: `cargo build`
2. Keep one focus: Option<WindowId> with MRU history -> verify: `cargo build`
3. WM owns ToplevelSurface; register window on first commit once app_id/title known -> verify: `cargo build`
4. Send fullscreen/maximized as xdg states to clients -> verify: `cargo build`
5. Handle fullscreen_request and maximize_request -> verify: `cargo build`
6. Window rules fire at window creation with actual app_id -> verify: `cargo build`

## Verification Script

1. F11 in browser toggles fullscreen; state sent to client.
2. Window rules match by app_id set by client.
3. Focus follows most-recently-used across outputs correctly.

## Out of scope

- Minimized window rendering changes

## Risks

- Changing WindowMode enum affects all code that pattern-matches on it.

---
