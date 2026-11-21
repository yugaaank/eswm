# Feature roadmap — what to bring from instantWM's bar/WM into eswm's dock & WM

**Date:** 2026-11-22
**Inputs:** `INSTANTWM-COMPARISON.md`, `INSTANTWM-BAR-WM-FEATURES.md`, current eswm source (`src/udev.rs` dock render ~lines 2851-3040, dock click ~1700-1755; `src/wm.rs` `DockEntry`/`dock_entries`; `src/ipc.rs` dock commands; `src/lua_state.rs` `es.dock`).
**Product constraint:** eswm is the compositor; **Tacit owns the shell**. Our dock is a minimal compositor-rendered affordance, and improvements should either (a) strengthen the compositor-side data/IPC contract so an external bar can do more, or (b) make the built-in dock genuinely more useful without turning it into a shell. Ranked by value/cost and aligned to the release plan (e08 archived, e09 design decisions next).

---

## 0. Current eswm dock — baseline

| Aspect | Current state |
|---|---|
| Render | Bottom strip of primary output, one segment per window on active workspace, 5×7 bitmap glyph (app_id/title first char), solid-color segments + base bar, cached buffers keyed by signature. Fullscreen band paints above it; overlay layers paint above it (e08s02). |
| Data | `DockEntry{id, app_id, title, minimized, focused}`, `wm.dock_entries()` filtered to primary output + active workspace, stable creation order. |
| Input | Left-click = `focus_window_or_restore`, right-click = `toggle_minimize`, click consumes event. **No middle-click, no drag, no scroll, no hover, no per-segment widget.** |
| Config | Global `es.dsp.set_dock_height(n)`, `dock_height` in `DecorationConfig`; no per-output, no visibility policy. |
| IPC / Lua | `dock` snapshot, `dockfocus <id>`, `docktoggle <id>`; `es.dock.entries()`, `es.dock.activate(id)`, `es.dock.toggle_minimize(id)`. |
| Tests | Pure geometry helpers (underflow, label scale), z-order tests. No hit-test abstraction (index math inline in input handler). |

---

## 1. Tier 1 — adopt now (cheap, high value, mostly data/IPC/model)

### 1.1 Enrich `DockEntry` and the dock snapshot
**From instantWM:** title strip carries far more state than our 5 booleans — urgent, sticky, scratchpad, locked, present-on-multiple-workspaces, minimized, focused.
- Add to `DockEntry`: `workspace: u32`, `output: String`, `mode: WindowMode` (floating/maximized/fullscreen), `urgent: bool` (if/when we track it), `layout_index: Option<usize>` (position in workspace.windows / tree order).
- Populate `dock_entries()` from **all outputs/workspaces** or at least expose a `dock_entries_for(output, workspace)` variant; today external bars cannot show windows on other outputs.
- **Why:** e09s02 (per-output model) requires this anyway; Tacit's shell can render a real multi-monitor taskbar only if IPC exposes the data.
- Cost: low. Code: `src/wm.rs`, `src/ipc.rs` snapshot. Verify: unit test `dock_entries_multiple_outputs`.

### 1.2 IPC commands — complete the dock/taskbar vocabulary
**From instantWM's bar actions that map cleanly to IPC:** our IPC already has focus/minimize; add:
- `dockclose <id>` → close the window (send `send_close`, reuse the e08s02 titlebar close path).
- `dockmoveworkspace <id> <workspace>` → move window to workspace (already possible via Lua keybinds; expose as command so external bars can do instantWM's "drag to tag").
- `docktogglemax <id>` / `dockfullscreen <id>` → reuse `toggle_maximized`/`toggle_fullscreen` paths.
- `dockcycle <prev|next>` → focus next/previous window in dock order (like instantWM's wheel-over-title).
- `dockactivateoutput <name>` → focus an output (needed for multi-monitor taskbars).
- **Security:** keep these behind the existing 0600 socket + SO_PEERCRED logging planned in e09s03.
- Cost: low. Verify: IPC command tests (`src/ipc.rs` style existing tests).

### 1.3 Extract a pure dock hit-test helper
**From instantWM:** `bar::model::hit_test()` is the single source of truth for which control a click hits. Our index math (`idx = floor((x-gx)/seg_w)`) is inline in the input handler.
- Add `dock_segment_at(x, gx, gw, n) -> Option<usize>` with tests (boundaries, n=0, x outside).
- Add `DockClickAction` enum {Activate, ToggleMinimize, Close, CycleNext, CyclePrev} and a pure `dock_click_action(button, modifiers, entry) -> DockClickAction` — mirrors their typed binding table.
- Then wire input handler through it. Middle-click → close immediately; wheel → cycle; hover state → redraw highlight.
- Cost: low-medium. Verify: `cargo test --bin eswm dock_click`.

### 1.4 Middle-click close + wheel cycling + hover highlight on the dock
**From instantWM:** middle-click a title closes it; wheel focuses prev/next; hover highlights.
- Add to our dock input: middle-click = close (via `send_close`), wheel up/down = cycle focus among dock entries, pointer motion over segment = set `hovered_dock_index` in `State` → `redraw_needed`, render highlight differs from focused.
- Add `es.dsp.set_dock_colors(focused, hovered, normal, minimized)` optionally — or reuse decoration colors initially.
- Cost: medium. Verify: hit-test unit tests + build.

### 1.5 Persistent z-order projection (small version)
**From instantWM:** `ClientZOrder` + `sync_z_order` layers, focus changes don't mutate persistent order.
- Our `workspace.windows` order is already effectively a stacking order for tiling; focus reorders it in some paths. Before e09s01 changes the model, add a small test asserting focus does not reorder `workspace.windows` unless explicitly requested. If it does, fix or document.
- Full layered projection (floating > fullscreen > tiled) can wait; e08s02 already handles fullscreen as a render band.
- Cost: low (analysis + tests). Verify: `cargo test --bin eswm dock_order`.

---

## 2. Tier 2 — adopt with e09 (model/design work)

### 2.1 Dock reflects the split window model (e09s01)
When `WindowMode` is split into `layout: Tiling|Floating` and `state: {maximized, fullscreen, minimized}`:
- `DockEntry` should expose those as separate fields; our e08s04 `mode_sent` marker becomes part of "state sent to client".
- Minimized stops being `!mapped`; dock should show a distinct icon state for minimized vs unmapped-with-buffer.
- Verify: e09s01 tests.

### 2.2 Dock is per-output, not global (e09s02)
- `dock_height` moves from global `DecorationConfig` into per-output config; `dock_entries()` takes an output id; render each output's dock from its own work area.
- This fixes a real bug: today the dock is only drawn on the primary output, but `wm.dock_entries()` filters by primary — with two outputs, the second screen has no dock and windows there can't be clicked from the dock.
- Add `es.dsp.set_output_dock_height(name, n)` or the e09 config shape's per-output field.
- Verify: tests with two virtual outputs.

### 2.3 Typed action catalog shared by Lua binds, dock clicks, and IPC (e09s04)
**From instantWM:** one `NamedAction` enum is source of truth for action name, typed args, docs, execution, and CLI listing.
- In eswm's Lua API, define a catalog (Rust-side enum + registry, or Lua table with validation) covering the existing string actions (`spawn_terminal`, `close_window`, `switch_workspace`, …) plus new dock actions.
- `es.bind` should validate action names at bind time (currently accepts any string and silently does nothing at keypress — exactly the "stub that lies" problem in e09s04).
- Expose `es.actions()` listing available actions with arg schemas, like their `--list-actions`.
- Dock click mappings then reference catalog entries instead of hardcoded button checks.
- Verify: Lua tests for invalid action → error; action listing non-empty.

### 2.4 Validate-before-apply reload (e09s04)
**From instantWM:** bad config leaves live state untouched.
- Our Lua config is loaded once at startup with no reload; `es.config.reload` is a silent stub.
- Implement `es.config.reload()` that re-parses the Lua file into a fresh state, validates all setters, then atomically swaps — on error, keep current config and log.
- Dock height/scale/keymap setters should participate (e08s03 already stores them in `InputConfig`).
- Verify: Lua test with a config that sets height then errors → height unchanged.

---

## 3. Tier 3 — selective, only if Tacit asks

### 3.1 Tag-like multi-workspace views
instantWM's tags (bitmask, multi-view, sticky, scratchpad) are powerful but conflict with Tacit's Hyprland-compatible workspace model. **Do not adopt.** Instead expose workspaces properly via IPC (Tier 1.2).

### 3.2 Persistent manual tiling tree, scratchpads, snap grid
Rich but large; eswm's master/equal is sufficient and product-scope locked. **Do not adopt now.** If Tacit needs per-window ratios, add a small `set_window_weight` IPC later rather than importing a tree.

### 3.3 Bar-hosted DBusMenu / SNI tray / i3bar status / start menu
Tacit owns these surfaces. **Do not adopt.** Our dock must stay a window switcher, not become a shell.

### 3.4 Window drag-to-tag from the compositor dock
We can support it cheaply *once* `dockmoveworkspace` IPC exists (Tier 1.2) — but the drag gesture itself belongs in Tacit's shell, not in eswm. **Do not implement drag inside eswm's dock.**

### 3.5 Close button / resize widget per dock segment
Our dock segments are narrow and show only a glyph; adding click sub-zones conflicts with the simple "one segment = one window" hit model. instantWM's per-title close/lock/draw widgets are on the **title strip**, which we don't have. **Skip** unless we add a title strip (3.6).

### 3.6 Maximized tab strip (title strip as tabs)
instantWM's maximized presentation turns the bar title strip into draggable stack tabs. We already made fullscreen a render band (e08s02). If Tacit wants tabbed maximized windows, this is the one bar feature worth revisiting — but implement it as an **external bar/title-strip layer-shell client** driven by IPC, not inside eswm.

---

## 4. Explicit non-goals (from the comparison)

- X11 backend, TOML config, bincode IPC, built-in status bar/start menu, pinned smithay fork, full protocol parity.
- Any change that makes eswm draw Tacit's shell UI.

---

## 5. Suggested sequencing

| Order | Work | Fits story |
|---|---|---|
| 1 | 1.1 enrich DockEntry + multi-output data, 1.2 IPC commands | **e09s02/e09s03** (or small standalone `feat(ipc)` before e09) |
| 2 | 1.3 hit-test helper, 1.4 middle/wheel/hover | standalone `feat(dock)` after e09s01/02 land (model is stable) |
| 3 | 2.3 typed action catalog, 2.4 reload | **e09s04** |
| 4 | 1.5 z-order assertion, 2.1 model alignment | **e09s01** |
| 5 | 3.x | only on explicit Tacit-driven request |

---

## 6. Summary

The best instantWM ideas for eswm are **not UI features** — they are contracts: richer dock state, a dock command vocabulary over IPC, a pure hit-test/action layer, a typed action catalog with validation, and per-output geometry. Those make Tacit's shell more capable while keeping eswm a compositor. The visual bar features (title tabs, systray, status clicks, menus, drag gestures) should stay in Tacit; we should only supply the data and commands that enable them.
