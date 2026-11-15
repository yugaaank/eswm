### Story e06s01: Popups (initial configure, rendering, input)

**type:** feat  
**risk:** P3  
**context:** shell  

**Context:** Popups never get their initial configure, so they never map, and they
aren't rendered or given input. That breaks menus, dropdowns, tooltips, Quickshell's
PopupWindow and tray menus.

## Steps

1. Add PopupManager to State -> verify: `cargo build`
2. new_popup: track_popup(PopupKind::Xdg(surface)) and constrain to parent's output -> verify: `cargo build`
3. Commit handler: call popups.commit(surface); on initial commit, send_configure()
   (anvil's shell/mod.rs ensure_initial_configure) -> verify: `cargo build`
4. reposition_request: update positioner, constrain, send_repositioned(token) -> verify: `cargo build`
5. grab: popups.grab_popup(...) with PopupKeyboardGrab and PopupPointerGrab -> verify: `cargo build`
6. Render popups above parent; same for layer surfaces (get_popup) -> verify: `cargo build`

## Verification Script

1. Client creates xdg_popup; receives configure; after commit with buffer, appears in
   render list.
2. Right-click menu opens in GTK app and browser.

## Out of scope

- Popup keyboard grab nuances beyond anvil's pattern

## Risks

- Smithay 0.7 popup API may differ from anvil; check Smithay source.

---
