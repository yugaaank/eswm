### Story e05s03: Configure only when something changed (dirty flag)

**type:** refactor  
**risk:** P3  
**context:** core loop  

**Context:** Layout runs on every render of every output and calls send_configure()
for every window. In Smithay 0.7, send_configure always sends; send_pending_configure
skips unchanged state. Most toolkits redraw on every configure, so every client
redraws at the refresh rate forever. Clients also can't keep a size you didn't ask
for (min sizes, terminals snapping to cells).

## Steps

1. Give WindowManager a dirty flag; every change (add, remove, mode, focus, output,
   work area) sets it -> verify: `cargo build`
2. Lay out only when dirty; clear the flag there -> verify: `cargo build`
3. placed_windows only reads; does not trigger layout -> verify: `cargo build`
4. In apply_layout, set size and states with toplevel.with_pending_state(...),
   then call send_pending_configure() -> verify: `cargo build && cargo test`

## Verification Script

1. A client commits 100 frames; receives no configure after the initial one.
2. Focusing another window sends exactly one more configure (activated change).

## Out of scope

- vblank integration (e05s04)
- Damage tracking (e05s04)

## Risks

- Dirty flag must be set on every state change; missing one causes stale layout.

---
