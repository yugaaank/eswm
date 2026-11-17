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

## Post-completion (2026-11-17)

The risk above materialised twice, and both times the UI froze rather than
degraded:

- **Step 3's throttle.** Holding frame callbacks back looks like pacing, but a
  callback is the client's permission to draw — suppressing one blocks the
  client, and a blocked client stops committing, so no damage, no flip and no
  callback ever again. Delivery is now on every present; the one-frame-in-flight
  gate is what actually paces the loop.
- **A missing `redraw_needed` point.** `PointerMotionAbsolute` updated the
  pointer location without setting the flag, so an absolute pointer could move in
  state and never reach the panel.

Both are recorded in `specs/bugs/BUG-2026-11-17T00-30-00-idle-stutter.md`. Every
new state-change path needs a test or a comment saying which present it triggers.

---
