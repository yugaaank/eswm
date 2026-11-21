# Story e09s05: Toplevel ownership, client requests and window rules

**id:** e09s05
**type:** refactor
**risk:** P2
**context:** window model / xdg shell
**status:** pending
**bcps:** 3
**delta:** ADDED

## 1. Context (placeholder — full plan pending)

Split from e09s01 during re-planning 2026-11-22. Three related concerns that need a real app_id/title and a stable window↔toplevel mapping:

1. **WM-owned ToplevelSurface / first-commit registration.** Today `XdgShellHandler::new_toplevel` (`src/main.rs:82-101`) calls `wm.add_window(output_id, "", "")` before the client's `set_app_id`/`set_title` requests are dispatched, then stores only a `WindowId` in the surface data map. `toplevel_for_window` (`main.rs:522-529`) is an O(n) scan of `xdg_shell_state.toplevel_surfaces()` for every call. Register on first commit (when `XdgToplevelSurfaceData` has real metadata) and/or store the `ToplevelSurface` in the `Window`.
2. **Client requests.** `XdgShellHandler` has no `fullscreen_request` / `maximize_request`; F11-style client-initiated transitions do nothing.
3. **Window rules.** `WindowRule`/`add_window_rule`/`match_rule` exist (`src/wm.rs:175-181,698-711`) but are only called from tests; no Lua binding populates them. With first-commit registration, rules can fire with the real app_id and (in a later story) be exposed via the e09s04 config shape.

## 2. Scope (provisional)

- Defer window creation until first commit with real app_id/title; keep `toplevel_destroyed`/`prune_dead_windows` correct.
- Store or index toplevels for O(1) `toplevel_for_window`.
- Implement `fullscreen_request`/`maximize_request` in `XdgShellHandler`.
- Fire rules at creation with actual app_id; expose rule registration path (Lua binding in e09s04 or a minimal one here).

## 3. Dependencies

- e09s01 (ClientMode state model) must land first — requests map onto `Presentation` flags.
- e09s04 for the full Lua rule API; a minimal Rust/Lua binding may land here if needed.

## 4. Status

Spec placeholder only. `plan-work` will produce the detailed countable story and `e09s05-tasks.yaml` when this story becomes active. Not planned, not started.
