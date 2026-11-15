### Story e08s02: Correctness: close button, dock layering, fullscreen layering, CSD geometry

**type:** fix  
**risk:** P3  
**context:** correctness  

**Context:** Close button removes window from WM without send_close (app keeps
running, invisible). Dock draws over cursor and overlay layers/lock screens.
Fullscreen windows draw under top layers. CSD geometry ignored, offsetting GTK4
and libadwaita windows.

## Steps

1. Close button: call send_close; let client's destroy remove window -> verify: `cargo build`
2. Dock: insert below overlay layer -> verify: `cargo build`
3. Fullscreen windows: order above top layers -> verify: `cargo build`
4. CSD: use client's window_geometry -> verify: `cargo build`

## Verification Script

1. Close button closes app (client destroy removes window).
2. Dock draws below overlay layers and lock screens.
3. Fullscreen window draws above top layers.

## Out of scope

- XWayland CSD handling

## Risks

- Changing close behavior may affect clients that don't handle send_close gracefully.

---
