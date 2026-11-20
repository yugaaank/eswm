### Story e06s03: Keyboard focus that layers can't steal (update_keyboard_focus)

**type:** fix  
**risk:** P3  
**context:** input  

**Context:** map_layer_surfaces runs on every commit of every surface and focuses
the first layer whose can_receive_keyboard_focus() is true, including OnDemand.
Tacit has 13 OnDemand layers; any client commit moves keyboard to the shell.

## Steps

1. Run map_layer_surfaces only for layer surfaces (commit, creation, destruction) -> verify: `cargo build`
2. Add update_keyboard_focus(), called on state changes, not on commit -> verify: `cargo build`
3. Rules: Exclusive layer on top or overlay takes keyboard while mapped (topmost wins);
   when it unmaps, focus returns; OnDemand takes keyboard on click; otherwise keyboard
   follows focused window -> verify: `cargo build`

## Verification Script

1. Window has focus; OnDemand layer maps; window keeps focus.
2. Clicking the layer gives it focus.
3. Exclusive overlay takes focus; gives it back when destroyed.

## Out of scope

- Touch keyboard focus

## Risks

- Focus restoration on layer unmap must restore the correct window.

---
