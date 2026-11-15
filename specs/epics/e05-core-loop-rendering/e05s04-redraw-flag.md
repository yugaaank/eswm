### Story e05s04: Render only when something changed (redraw flag)

**type:** refactor  
**risk:** P3  
**context:** core loop  

**Context:** The loop always page-flips; frame callbacks go to every surface including
hidden workspaces; border/corner/titlebar/titlebar-button buffers are rebuilt every
frame; toplevel_for_window and prune_dead_windows scan every toplevel several times
per frame. anvil's udev.rs shows how to do damage-aware rendering.

## Steps

1. Keep a redraw_needed flag per output; set on surface commit, layout change,
   cursor motion, running animation -> verify: `cargo build`
2. Render only from vblank handler or idle callback when flag set and no frame
   pending -> verify: `cargo build`
3. Send frame callbacks only to surfaces drawn this frame, with throttle
   (anvil's send_frames) -> verify: `cargo build`
4. Use SolidColorRenderElement for borders (one Id per window, bump on color/size
   change) -> verify: `cargo build`
5. Use one PixelShaderElement for rounded corners, compiled once -> verify: `cargo build`
6. Cache titlebar and button buffers per window, keyed by size/title/focus (like
   dock cache) -> verify: `cargo build`
7. Move pruning into toplevel_destroyed callback -> verify: `cargo build`

## Verification Script

1. Debug counter of page flips per second, logged at debug once a second.
2. Counter reads 0 on idle desktop; typing in one terminal redraws only that output.

## Out of scope

- vblank handler integration details (Smithay-specific)
- Animation scheduler (future epic)

## Risks

- Missing a redraw_needed flag set point causes frozen UI; every state change must
  set it.

---
