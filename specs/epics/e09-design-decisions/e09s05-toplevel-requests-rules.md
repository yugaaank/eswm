# Story e09s05: Toplevel ownership, client requests and window rules

**id:** e09s05
**type:** refactor
**risk:** P1 (shared xdg-shell ownership path; see `specs/IMPACT_LATEST.md`)
**context:** window model / xdg shell
**status:** completed
**bcps:** 3
**delta:** ADDED + MODIFIED

## 1. Context

`XdgShellHandler::new_toplevel` (`src/main.rs:90-109`) calls
`wm.add_window(output_id, "", "")` before the client's `set_app_id` and
`set_title` requests are dispatched, so rules can never match a real app id.
`toplevel_for_window` (`:537-544`) scans all live toplevels for every lookup,
and there are no `fullscreen_request`/`maximize_request` handlers, so a client
asking for fullscreen (F11, video player, browser) gets no state change.
`WindowRule`/`add_window_rule`/`match_rule` exist but only tests call them.

Operator decision 2026-11-26: register a toplevel as a WM window on **first
commit**, when real metadata is available; map client requests onto the
existing `Presentation` flags with tests; rules fired by e09s04 config must
apply at creation.

## 2. Problem
- Rules see an empty `app_id`; the `window_open` event reports an empty class.
- `toplevel_for_window` is O(n) and called from close, focus and resize paths.
- Client-initiated fullscreen/maximize requests are ignored.
- Destroy before first commit can leave no window to clean up.

## 3. Goal
One live toplevel maps to exactly one WM window with real metadata from its
first commit, rules apply at that moment, lookups are direct, and client
fullscreen/maximize requests drive `ClientMode` through `apply_layout`.

## 4. Non-goals
- XWayland, foreign toplevel manager, session lock, screencopy.
- Client-driven minimize (no compositor contract yet) and window menu.
- Rule engine beyond the existing app_id → workspace/mode match.

## 5. Users / Actors
- **Clients** — request fullscreen/maximize and expect configuration replies.
- **Tacit / shell** — reads real `app_id`/title in snapshots and events.
- **Operator** — declares `es.window_rule{}` entries (e09s04).

## 6. Preconditions
- Preflight green; e09s01–e09s04 landed on the same branch.
- e09s04 rule staging installed into `wm.add_window_rule` before windows spawn.

## 7. Postconditions
- No window exists for a toplevel that never commits; destroying one is safe.
- `toplevel_for_window` returns the indexed surface without a scan; destroy
  removes the index entry.
- Client set/unset requests update `Presentation` and send `xdg_toplevel.configure`
  via `apply_layout`.
- Existing keybind-driven toggles and popup/dock focus paths keep working.

## 8. Scope
In scope: `src/main.rs` (shell handler, commit hook, index, events),
`src/wm.rs` (rule application helper if needed), `src/udev.rs` (consumers of
the index), inline handler/fixture tests.

## 9. Zoom-out (purpose / callers / contracts)
- **Purpose:** `main.rs` xdg-shell glue maps Wayland toplevels to WM windows.
- **Callers:** smithay delegates (`main.rs:1745`), `udev.rs:1238,1694,1763,1795`,
  tests using `fixture_client_creates_toplevel_wm_sees_app_id`.
- **Contracts:** one toplevel ↔ one `WindowId`; `apply_layout` is the only
  configure writer; `toplevel_destroyed` removes the window; event frames keep
  their e09s03 shape and `primary` stays a deprecated IPC alias.

## 10. Requirements (delta tags)

### MODIFIED: First-commit registration
**Before:** `new_toplevel` called `wm.add_window(output_id, "", "")` immediately
and tagged the surface with the returned `WindowId`.
**After:** `new_toplevel` sends the initial configure and waits; the first
commit of a toplevel surface reads real `app_id`/`title`, creates the window,
applies rules, tags the surface and indexes the handle. A destroyed toplevel
with no window is a no-op.

### ADDED: Direct toplevel index
`State` owns a `BTreeMap<WindowId, ToplevelSurface>`, filled at registration
and removed in `toplevel_destroyed`/prune. `toplevel_for_window` reads it; no
linear walk of `xdg_shell_state.toplevel_surfaces()`.

### ADDED: Client presentation requests
`fullscreen_request`/`unfullscreen_request` and
`maximize_request`/`unmaximize_request` map onto `Presentation::Fullscreen`
and `Presentation::Maximized` for the owning window, preserve `Placement`, and
finish with `crate::apply_layout`. Unset restores the pre-request geometry:
fullscreen returns to tiled placement, maximize uses the saved rect.

### ADDED: Rules at creation with real metadata
`wm.add_window` receives the committed `app_id`, so `match_rule` assigns the
configured workspace/mode at creation, not later. `window_open` /
`window_class` events carry the real values in the same tick.

## 11. UX / API notes
- A client that commits before setting `app_id` still gets a window (with an
  empty id); a later `app_id_changed` syncs it via `sync_window_meta` as today.
- Fullscreen output selection from the request is ignored: eswm uses the
  window's output, consistent with keybind-driven fullscreen.
- Unset requests for a window not in that presentation are harmless no-ops.

## 12. Data / State
- `State.toplevels: BTreeMap<wm::WindowId, ToplevelSurface>`.
- Surface data map still carries `WindowId` for `window_id_of`; the index is
  the fast path and the data map remains the authoritative tag.
- Presentation saved rects stay in `wm::Window` (`toggle_maximized` semantics).

## 13. Interfaces / Contracts
- xdg-shell wire behaviour stays protocol-conformant; configure replies still
  come only from `apply_layout`.
- IPC/Lua shapes are unchanged; `window.set_mode` keeps using `wm.set_mode`.
- `window_open`, `window_class` and `window_mode` event names/shapes unchanged.

## 14. Dependencies
No new crates. Smithay 0.7 `XdgShellHandler` supplies the four request hooks:
`maximize_request(&mut self, ToplevelSurface)`, `unmaximize_request(...)`,
`fullscreen_request(&mut self, ToplevelSurface, Option<WlOutput>)`,
`unfullscreen_request(...)` (local crate source:
`smithay-0.7.0/src/wayland/shell/xdg/mod.rs:1143-1157`) [OK].

## 15. Risks
- Deferring creation changes when focus, popups and configure happen; detected
  early by fixture tests and live UAT of spawn → focus → close.
- An index can go stale on destroy/hotplug; every removal path removes it and
  `prune_dead_windows` is covered by a test.
- Client requests may arrive before registration; ignore them when no window
  id is tagged rather than creating a phantom window.

## 16. Out of scope
XWayland, foreign toplevel management, window menu, client-driven minimize,
rule matching beyond `app_id`, touch and gestures.

## 17. Acceptance / Verification
- `cargo test --bin eswm toplevel_first_commit`, `toplevel_index`,
  `client_request`, `toplevel_rules` pass with real assertions.
- `cargo clippy --all-targets && cargo test && cargo build && python3
  scripts/spec-lint.py` green after each task.
- Live DRM UAT recorded in `specs/verifications/e09s05-verify.yaml` with
  explicit operator N/A decisions where hardware is absent.

## Implementation Steps

1. Defer `wm.add_window` to first commit with real app_id/title; safe no-op on destroy → verify: `cargo test --bin eswm toplevel_first_commit`
2. Index toplevels by `WindowId` and remove entries on destroy/prune → verify: `cargo test --bin eswm toplevel_index`
3. Handle client fullscreen/maximize set and unset requests through `apply_layout` → verify: `cargo test --bin eswm client_request`
4. Prove rules and events use the committed app_id at creation → verify: `cargo test --bin eswm toplevel_rules`
5. Operator live DRM UAT: spawn, F11/fullscreen request, maximize, destroy before commit → verify: `manual: checklist in specs/verifications/e09s05-verify.yaml`

## Risks (implementation)
- See §15. Any new surface state must be removed in `toplevel_destroyed`, and
  every new request hook must call `apply_layout` after changing `ClientMode`.
