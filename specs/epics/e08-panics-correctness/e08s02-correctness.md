# Story e08s02: Correctness: close button, dock layering, fullscreen layering, CSD geometry

**id:** e08s02
**type:** fix
**risk:** P2
**context:** correctness
**status:** completed  
**bcps:** 5
**delta:** MODIFIED

## 1. Context

Four correctness defects in the DRM render/WM path: the server-side titlebar close button removes the WM window without asking the client to close (the app keeps running invisibly); the dock is spliced at the front of the render list so it paints above overlay/top layers and lock screens; fullscreen windows are mixed into the normal window layer so overlay/top layers and the dock paint over them; and the client-provided `xdg_surface.set_window_geometry` offset (`xdg_offset`) is stored on `wm::Window` and consumed by placement/hit-testing but never populated, so GTK4/libadwaita windows with CSD margins are positioned against the wrong origin. This story fixes all four, with the operator-confirmed z-order policy: **fullscreen windows render above all layers and above the dock**.

## 2. Problem

- **Close button:** `udev::apply_titlebar_action` handles `TitlebarButton::Close` by calling `wm.remove_window(id)` directly. The XDG client never receives `xdg_toplevel.close`, so the process stays alive with no surface the compositor will map.
- **Dock layering:** `elements.splice(0..0, dock_elements)` puts the dock at index 0; smithay draws index 0 last (on top), so the dock covers overlay layers (lock screens, fuzzel) and the cursor.
- **Fullscreen layering:** fullscreen window surfaces are appended into the same `elements` list as tiled windows, which sits *below* `layer_elements_above` (overlay/top) and *above* only the bottom/background layers; the dock is then spliced above everything.
- **CSD geometry:** `wm::Window.xdg_offset` is applied in `placed_windows` and `surface_under` but is initialized to `(0,0)` and has no producer. Smithay already stores the double-buffered geometry in `SurfaceCachedState.geometry: Option<Rectangle<i32, Logical>>` ("Holds the double-buffered geometry that may be specified by `xdg_surface.set_window_geometry`", smithay 0.7.0 `src/wayland/shell/xdg/mod.rs:1062`); eswm only needs to read it on toplevel commit.

## 3. Goal

Make the titlebar close button request a client close rather than orphaning windows; establish a z-order where overlay layers beat the dock, fullscreen beats both, and CSD offsets are honored in placement and pointer hit-testing.

## 4. Non-goals

- XWayland CSD handling (no XWayland backend in scope).
- Changing server-side decoration policy (`XdgDecorationHandler` stays server-side-only).
- General layer-shell rework beyond the ordering decisions above.
- Touch input or fractional-scale work (e08s03/e09).

## 5. Users / Actors

- Operator closing windows with the titlebar X button.
- Lock screen / overlay-shell users (overlay must not be covered by the dock).
- Fullscreen video/game users (fullscreen must cover panels and dock).
- GTK4/libadwaita clients that use `set_window_geometry` for CSD margins.

## 6. Preconditions

- `main` preflight green before kickoff (`cargo clippy && cargo test && cargo build`).
- e08s01 shipped (saturating dock helpers available, `dock_segment_layout` etc.).

## 7. Postconditions

- Clicking the titlebar close button calls `send_close()` on the window's toplevel; the WM window is removed by the client's destroy path, not preemptively. If no toplevel exists, the WM window is left intact (no orphan removal).
- Overlay/top layers render above the dock; bottom/background layers still render below windows/dock.
- A fullscreen window's surface renders above all layer groups and above the dock.
- On toplevel commit, `wm::Window.xdg_offset` is updated from smithay's current `SurfaceCachedState.geometry.loc`; placement and `surface_under` use it.

## 8. Scope

**In scope**
- `src/udev.rs`: `apply_titlebar_action` Close branch → toplevel lookup + `send_close()`; dock/layer/fullscreen z-order in `render_surface`.
- `src/main.rs`: `CompositorHandler::commit` reads toplevel window geometry via `with_states` + `SurfaceCachedState` and updates `wm::Window.xdg_offset`.
- `src/wm.rs`: small pure helper(s) for ordering policy if needed for testability.
- Inline tests in `src/*.rs` for: no-toplevel close leaves window; z-order policy; CSD offset read mapping.

**Out of scope**
- XWayland CSD, decoration mode negotiation, lock-screen protocol (ext_session_lock), fractional scale.

## 9. Zoom-out (module purpose / callers / contracts)

- **`src/udev.rs` (DRM backend):** owns input action dispatch (`apply_titlebar_action`) and the render-element list construction. **Callers:** keyboard/pointer handlers, keybind actions, frame timer. **Contract:** server-side actions must keep WM state and client state in sync; render order index 0 = topmost (smithay convention).
- **`src/main.rs` (State / compositor handlers):** owns `CompositorHandler::commit`, xdg shell state, and `placed_windows`. **Callers:** Wayland clients via wayland-server; udev render path consumes `placed_windows`. **Contract:** committed client geometry must be reflected in the WM placement data before the next render/hit-test.
- **`src/wm.rs` (WindowManager):** owns window records including `xdg_offset`. **Callers:** udev render/input, main placement, IPC. **Contract:** `xdg_offset` is surface-origin → window-geometry-origin; render position = `target_rect - xdg_offset`.

## 10. Requirements (delta tags)

### MODIFIED: Titlebar close requests client close
**Before:** `apply_titlebar_action(TitlebarButton::Close)` calls `wm.remove_window(id)` immediately; the client never gets `xdg_toplevel.close` and may stay running with no mapped surface.  
**After:** Close resolves the window's `ToplevelSurface` (same lookup as the `close_window` keybind) and calls `send_close()`; WM removal happens via the client's destroy path. If no toplevel exists, no WM removal occurs.

### MODIFIED: Dock paints below overlay/top layers
**Before:** dock elements are spliced at index 0 (topmost), covering overlay/top layers, lock screens, and cursor.  
**After:** overlay/top layers are placed above the dock; bottom/background layers remain below windows and dock. Cursor remains topmost overall.

### MODIFIED: Fullscreen windows paint above all layers and the dock
**Before:** fullscreen window surfaces share the normal window band, below overlay/top layers and below the dock.  
**After:** a fullscreen window's surface elements are collected separately and spliced above all layer groups and the dock (below the cursor only). Non-fullscreen windows keep current ordering relative to layers/dock.

### ADDED: CSD window-geometry offset is read on commit
**Before:** `wm::Window.xdg_offset` is initialized to `(0,0)` and never updated; CSD clients are offset by their window-geometry margin.  
**After:** on toplevel commit, the current `SurfaceCachedState.geometry.loc` is read via `with_states` and stored as `wm::Window.xdg_offset`, so rendering and pointer hit-testing use the client's window-geometry origin.

## 11. UX / API notes

- No Lua/IPC API changes.
- Fullscreen now covers panels and the dock; overlay lock screens still draw above normal windows but *below* a fullscreen window per operator decision.
- Close button becomes cooperative: clients that ignore `send_close` will keep running (WM window stays until destroy) — acceptable, matches keybind behavior.

## 12. Data / State

- `wm::Window.xdg_offset: (i32, i32)` gains a producer (commit handler). No new persistent fields.
- Render ordering may use a small pure enum/helper describing element bands (reason for depth: ordering policy must be unit-testable without a GPU render loop).

## 13. Interfaces / Contracts

- Preserve `apply_titlebar_action` signature; only Close branch semantics change.
- Preserve `wm::Window.xdg_offset` meaning and consumers (`placed_windows`, `surface_under`).
- Smithay API evidence: `SurfaceCachedState.geometry: Option<Rectangle<i32, Logical>>` (smithay 0.7.0, `wayland/shell/xdg/mod.rs:1062`), readable with `compositor::with_states(surface, |states| states.cached_state.get::<SurfaceCachedState>().current().geometry)`.

## 14. Dependencies

- Existing `smithay` 0.7 (`SurfaceCachedState`, `with_states`), no new external packages. **Slopcheck:** N/A (no new dependencies).

## 15. Risks

- **Client cooperation:** a client that ignores `send_close` will not disappear; mitigated by matching existing keybind behavior and documenting in UX notes.
- **Z-order regressions:** moving dock below overlay changes lock-screen/dock interaction; mitigated by pure ordering tests plus operator UAT.
- **Fullscreen edge cases:** fullscreen popup/dialog layering may need follow-up; out of scope, watch in UAT.
- **CSD offset correctness:** wrong sign convention would shift windows/hit-tests; regression test must assert `geometry.loc` mapping to `xdg_offset`.

## 16. Out of scope

- XWayland CSD handling.
- Lock-screen protocol changes.
- Decoration mode negotiation changes.

## 17. Acceptance / Verification

- `cargo test --bin eswm titlebar_close` passes (close without toplevel leaves WM window intact).
- `cargo test --bin eswm render_order` passes (dock below overlay; fullscreen above all layers and dock).
- `cargo test --bin eswm csd_geometry` passes (window-geometry loc → xdg_offset mapping).
- `cargo clippy --all-targets && cargo test --bin eswm && cargo build` passes.
- Manual: close button closes app; overlay/lock screen paints above dock; fullscreen covers panels + dock; CSD window positioned correctly.

## Implementation Steps

1. Extract a pure toplevel-close decision helper in `src/udev.rs` (e.g. `close_action_removes_wm_window(toplevel_present: bool) -> bool` returning false when no toplevel) with inline tests; RED → GREEN. → verify: `cargo test --bin eswm titlebar_close`
2. Change `apply_titlebar_action` Close branch to look up the toplevel (via `crate::toplevel_for_window`) and call `send_close()`; do not call `wm.remove_window` in the Close branch; keep `apply_layout` + redraw behavior for other branches. → verify: `cargo build`
3. Add a pure render-band ordering helper (enum of bands: cursor, fullscreen, overlay_top, windows, dock, background) and tests asserting: overlay above dock, fullscreen above overlay+dock, cursor topmost. → verify: `cargo test --bin eswm render_order`
4. Apply the band order in `render_surface`: collect fullscreen surface trees separately, place overlay/top layers above dock, and splice fullscreen band above all layers/dock. → verify: `cargo build`
5. Add a pure helper mapping `Option<Rectangle<i32, Logical>>` window geometry to `(i32, i32)` xdg offset (None/empty → `(0,0)`) with tests; RED → GREEN. → verify: `cargo test --bin eswm csd_geometry`
6. In `CompositorHandler::commit`, read toplevel `SurfaceCachedState.geometry.current()` via `with_states`, map through the helper, and update `wm::Window.xdg_offset` on the matching window. → verify: `cargo build`
7. Run full preflight for the story. → verify: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm titlebar_close` — close without toplevel leaves the WM window; no orphan removal.
2. Run `cargo test --bin eswm render_order` — dock below overlay/top; fullscreen above all layers and dock; cursor topmost.
3. Run `cargo test --bin eswm csd_geometry` — window geometry loc maps to xdg_offset; missing geometry maps to (0,0).
4. Run `cargo clippy --all-targets && cargo test --bin eswm && cargo build` — all green.
5. Live DRM session: click titlebar X → app closes; lock screen/fuzzel paints above dock; fullscreen video covers panels and dock; GTK4/libadwaita CSD window sits at the correct origin and accepts pointer input at expected positions.

## Risks

- see §15
