### Story e08s01: Panics: dock underflow, floating_target underflow, connector modes, renderer errors

**type:** fix  
**risk:** P2  
**context:** correctness  

**Context:** Dock with height 1-7 panics on every frame (seg_h - 2 and seg_h - gh
usize underflows at udev.rs:2211-2237). floating_target underflows on work area
under 640x480 (wm.rs:554-555). connector.modes()[mode_id] panics when no modes
(udev.rs:1372). Renderer .unwrap() every frame (udev.rs:1758); panic! on
ContextLost (1663, 2392).

## Steps

1. Use saturating or checked arithmetic for dock segment math -> verify: `cargo build`
2. Move dock segment maths into pure unit-testable function -> verify: `cargo test`
3. floating_target checks minimum area before arithmetic -> verify: `cargo build`
4. Skip connectors with no modes; log it -> verify: `cargo build`
5. On renderer error, log and skip frame -> verify: `cargo build`
6. On ContextLost, log and exit cleanly status 1 -> verify: `cargo build`

## Verification Script

1. Floating spawn on 600x400 output; no panic.
2. Dock function for heights 0-10 with 1-20 windows; no panic.
3. Connector with no modes; skipped, logged.

## Out of scope

- Renderer recovery beyond skipping frame

## Risks

- Skipping connectors with no modes may leave some outputs unused.

---
