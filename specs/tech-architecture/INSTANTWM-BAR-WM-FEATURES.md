# instantWM — complete catalog of window management via the bar

**Date:** 2026-11-22
**Source:** deep static analysis of `/home/yugaaank/Projects/eswm/instantWM` (v0.8.1). Reference-only; no code copied. Companion to `INSTANTWM-COMPARISON.md`.

The bar in instantWM is **not a status display** — it is a full compositor component and one of the primary WM control surfaces. The pipeline is backend-neutral: render builds a per-monitor `MonitorHitCache`; `bar::model::hit_test()` resolves local X into `BarPosition`; `mouse::pointer::button_region_at()` maps root coords to `PointerRegion::Bar{pos}`; `mouse::press::dispatch_press_policy()` dispatches configured bindings (`src/config/buttons.rs:32-100`) through `src/actions/dispatch.rs:106-252`. X11 and Wayland only translate input and paint pixels — **every action below works on both backends** unless noted.

---

## 1. Tags — view, switch, multi-tag, move windows

All tag interactions use `ButtonAction::DragTagBegin`; motion past `DRAG_THRESHOLD` (5 px, `src/mouse/constants.rs:14`) promotes click → drag.

| Interaction | Result | Evidence |
|---|---|---|
| **Left-click tag** | View (switch to) that tag | `src/mouse/drag/tag.rs:83-84` → `tags::view::view_tags` (`src/tags/view.rs:58-72`) |
| **Right-click tag** | Toggle tag in/out of the view (multi-tag selection); refuses empty view | `src/config/buttons.rs:64`, `src/tags/view.rs:90-125`; cancels overview (`src/overview.rs:59-69`) |
| **Scroll over tag** | Previous / next tag | `src/actions/named.rs:192-193`, `scroll_view_with_slide` |
| **Modkey+scroll** | Shift the whole view to adjacent tag | `src/config/buttons.rs:70-71`, `src/tags/view.rs:127+` |
| **Super+left-click tag** | Move selected window to that tag | `SetSelectedClientClickedTag` → `tags::client_tags::set_client_tag` (`src/actions/dispatch.rs:140-146`) |
| **Super+right-click tag** | Toggle selected window's tag membership | `src/config/buttons.rs:68`, `dispatch.rs:147-153`, `client_tags.rs:198` |
| **Drag tag** (window selected) | Move selected window to destination tag | `TagReleaseAction::DropWindow`, `apply_window_tag_drop` (`src/mouse/drag/tag.rs:91-103`) |
| **Ctrl+drag tag** | Move ALL windows from source tag to destination tag | `TransferTags{Move}` (`tag.rs:52-57,269`), `transfer_tag_clients` (`client_tags.rs:134`) |
| **Shift+drag tag** | Swap windows of the two tags (Shift beats Ctrl) | `TransferTags{Swap}` (`tag.rs:52-53,269`) |
| **Alt at drop** | Move-and-follow: select monitor and view destination tag | `tag.rs:65,270-273` |
| Hover during drag | Highlights tag cells, fills hover color | `src/mouse/drag/tag.rs:187-218`, `src/bar/scene.rs:258-260` |

Tag appearance: leading `tag_slots` + occupied/selected tags + one empty tag; colors encode urgent/focused/filled/empty/inactive (`src/tags/bar.rs:17-57`, `src/bar/scene.rs:27-61`).

---

## 2. Window titles in the bar

Titles come from `mon.bar_client_order()` (`src/bar/scene.rs:270-307`); hit cells in `hit.title_ranges`.

| Interaction | Result | Evidence |
|---|---|---|
| **Left-click title** | Hidden → show+focus; focused+visible → **hide**; else focus+**raise** (explicit stacking handle even when click-to-raise is off) | `src/mouse/drag/title.rs:416-473,429-445`; `src/actions/dispatch.rs:118-123` |
| **Right-click title** | Focus + `zoom` (maximize to master) | `title.rs:429-434` |
| **Right-drag title** | Resize / promote to floating with pointer anchor preserved; snap restored | `title.rs:155-207,99-139`; edge scratchpads can't start a move (`title.rs:116-118`) |
| **Middle-click title** | Close that window | `CloseClickedTitleWindow` → `close_client` (`dispatch.rs:124-129`) |
| **Left-drag title within strip** | Live reorder: on crossing each title cell, `swap_bar_titles` fires immediately | `title.rs:259-268,296-350`; `src/layouts/manager/commands.rs:266-322` |
| **Drag beyond strip** | Reorder converts to ordinary move drag (one-way) | `title.rs:348-383` |
| **Wheel over title** | Focus prev / next window | `src/config/buttons.rs:45-46` |
| **Shift+wheel** | Reorder selected prev/next (maximized stack first) | `dispatch.rs:178-195`, `commands.rs:212-264` |
| **Ctrl+wheel** | Scale selected window ±10% | `src/config/buttons.rs:49-50`, `dispatch.rs:196-200` |
| **Super+left-click** | Toggle edge scratchpad | `src/actions/named.rs:204` |
| **Super+right-click** | Spawn `instantnotify` | `src/config/buttons.rs:44` |

### Special per-title widgets (selected window only)
- **Close button** (left 32 px of the cell): left-click closes (`CloseSelectedClient`, `buttons.rs:91`), **right-click locks the client** (`ToggleLockSelectedClient`, `buttons.rs:92`, `dispatch.rs:173-177`). Rendered only when the cell is wide enough (`src/bar/scene.rs:288-298,536-565,729-739`).
- **Resize widget** (right 30 px): left-click arms the "Rio-like drawing" region selection on release (`buttons.rs:93`, `dispatch.rs:217-221`, `src/mouse/press.rs:615-638`).

### Bar ↔ tree integration
- In **maximized** presentation the title strip is a **tab strip**: title order = `tiled_tree_order()`, and `swap_bar_titles` swaps the underlying **layout-tree leaves** (test `title.rs:722-753`: "maximized titles are stack tabs and must swap tree leaves").
- In other presentations reorder swaps positions in the monitor client/focus stack (`commands.rs:316-322`).
- Tree ↔ floating boundary cannot be crossed by reorder. Stale-hit protection during async rendering: cache trusted only if `title_ranges.len() == bar_client_order().len()` (`title.rs:277-294`).

---

## 3. Layout menu / start menu launched from the bar

- **Layout symbol button** (between tags and shutdown):
  - **Left-click** → `LayoutMenu` (`src/config/buttons.rs:37`): opens a WM-owned 2×4 `instantmenu` anchored below the bar with **Tile / Grid / BottomStack / Floating / Maximized + Reset** for each tiling layout (`src/bar/layout_menu.rs:55-85,191-230,269-301`). Choosing an item applies `SetLayout`/`ResetLayoutTo`; configured shortcuts are auto-decorated (`layout_menu.rs:94-182`); stale menus ignored (`:318-333`).
  - **Right-click** → cycle layout next; **middle-click** → reset layout (`buttons.rs:38-39`).
- **Start menu icon** (leftmost cell): left-click `instantstartmenu`, right-click `quickmenu`, Shift+left → prefix mode (`buttons.rs:97-99`); hover swaps colors (`src/bar/scene.rs:472-512`).
- **Window chooser** (MRU alt-tab menu): presented via `instantmenu`, selecting a hidden window un-hides and activates it (`src/window_menu.rs:1-128`).

---

## 4. Status block clicks (i3bar protocol)

- Bar can run a `status_command`, auto-detect `i3status-rs`, or use a built-in clock (`src/bar/status/command.rs:280-289`).
- When i3bar header advertises `click_events: true`, click JSON (name/instance/button/coords/modifiers) is written to the producer's stdin (`command.rs:461-474,521-531`, `src/bar/status/render.rs:98-124`) → arbitrary scripted WM behavior via IPC.
- `handle_status_text_click` ignores clicks in overview/non-default mode, resolves the block through `hit.status_click_targets`, forwards (`src/bar.rs:289-345`). Hover arms `Gesture::StatusBlock(idx)` and paints hover color (`src/bar.rs:186-195,275-281`).
- **Priority:** with click events enabled, the status region wins over the title strip in hit-testing (`src/bar/model.rs:104-106`) and generic bindings are bypassed (`src/mouse/press.rs:115-123`).

### Generic status-area bindings (when no i3bar click events)
`src/config/buttons.rs:51-62`: Left → menu, Middle → terminal, Right → window switcher, wheel → volume; Super+Left → settings, Super+Middle → mute, Super+Right → spoticli, Super+wheel → brightness, Super+Shift+Left → pavucontrol, Super+Ctrl+Left → instantnotify.
Status text shown only on the selected monitor (`src/bar/scene.rs:458-462`).

---

## 5. Systray icons (StatusNotifier + XEmbed)

- **Click icon**: hit slots `hit.systray_slots` (`src/bar/scene.rs:749-760`) → `systray::press_icon` → Left = **Activate**, Middle = **SecondaryActivate**, Right = **ContextMenu** (hosted DBusMenu session) (`src/systray/status_notifier/runtime.rs:143-183`).
- **Click tray-menu entry**: overlay `hit.tray_menu` is the **topmost** hit layer (gaps do NOT fall through, `src/bar/model.rs:65-76`); `MenuAction::{Activate, OpenSubmenu, Back}` navigate the tree with a synthetic "‹ Back" entry (`src/systray/mod.rs:183-188`, `status_notifier/menu.rs:68-90,169-174,245-247`). Any other bar click closes an open menu first (`src/mouse/press.rs:102`).
- Menu can be external (`instantmenu`) instead of in-bar (`TrayMenuBackend`, `src/systray/instantmenu.rs:33-42`).
- Hover highlights via `Gesture::TrayMenuEntry(idx)` (`src/types/input.rs:188`, `src/bar/scene.rs:847-863`).
- **Backend notes:** X11 places legacy XEmbed tray windows (`src/backend/x11/systray.rs:210-372`); Wayland renders tray icons into the bar scene (`src/systray/mod.rs:347-417`).

---

## 6. Bar-adjacent gestures and drag targets

- **No "drag bar to another monitor" and no bar-resize gesture** — bar geometry derives from monitor work area + config (`src/bar/scene.rs:433-440`). Monitor selection follows the pointer over the bar (`src/bar.rs:245-255`).
- **Bottom gesture strip** (per monitor, `PointerRegion::BottomBar`): default `BottomBarDrag{left: ScrollLeft, right: ScrollRight, up: ToggleOverview, click: menu, hold: settings}` (`src/config/buttons.rs:72-78`). Direction latches past `monitor_width/30` (`src/core_state/interactions/gestures.rs:41-130`); one action on release, click vs hold by duration (`src/mouse/interaction.rs:520-686`, `dispatch.rs:222-250`).
- **Top-right hot corner** (overlaps bar end, 20×4 px activation zone): entering toggles an edge scratchpad once (`src/mouse/hot_corner.rs:1-81`, `src/floating/scratchpad.rs:821-835`). Present on both backends.
- **Right-edge sidebar gesture** (starts `bar_height + 60` below top): scratchpad/volume (`src/mouse/pointer.rs:49-83`, `gestures.rs:150-157`).
- **Dropping a window on the bar** (`handle_bar_drop`, `src/mouse/drag/move_drop.rs:162-254`): transfers window to that monitor; on a tag → tilifies + applies tag drop (Alt-follow honored); elsewhere → floating becomes tiled.

---

## 7. Hover / preview / outline behavior

- `BarHoverState{monitor_id, gesture, drag_active}` (`src/bar.rs:55-95`), shown only on its own monitor.
- `update_hover` sets gestures (tag/title/close/start/tray/status-block), selects monitor, invalidates bar content (`src/bar.rs:239-287`).
- Tag hover: taller detail stripe (4→8 px) and hover color (`src/bar/scene.rs:18-20,578-582`). Title hover: variants for urgent/sticky/minimized/edge-scratchpad roles (`scene.rs:63-108`). Close-button hover: raised stripe (`scene.rs:550-564`).
- **Captured gestures own bar hover** to prevent flicker (`owns_bar_hover()`, `src/backend/wayland/input/pointer/motion.rs:798-805`); drags set `drag_active` and release restores hover without a one-frame flash (`tag.rs:278-294`, `title.rs:389-409`).
- Compositor outline (`InteractionOutlineStyle::{Layout, Close}`, `src/core_state/interactions/outline.rs:1-49`) previews placement/resize; pointer previews are not interpolated, keyboard ones animate.

---

## 8. Shutdown / lock / scratchpad via bar

- **Shutdown button** (shown when no client selected): left → `instantshutdown`, middle → `instantlock -o`, right → configured lockscreen (`src/config/buttons.rs:94-96`, `src/bar/scene.rs:456,624-636`).
- Edge scratchpad toggles via title Super+click, tag/bottom-bar bindings, root bindings (`buttons.rs:82-84`), and hot corner.
- Lock a client via right-click close button; hide/focus/raise/zoom via title clicks; overview via bottom-bar up-swipe; `ToggleClickedViewTag` cancels overview (`src/overview.rs:65`); status click in overview resets mode (`src/bar.rs:295-306`).

---

## 9. Per-monitor bars / visibility policies

- Per-monitor snapshot: `build_monitor_snapshots` includes bars by `mon.bar_visible()`/`shows_bar()` (`src/bar/scene.rs:335-470`); status only on selected monitor; tray on host monitor; external tray width only there.
- Per-monitor hit cache `HashMap<MonitorId, MonitorHitCache>` (`src/bar.rs:39-53,175-184`); `root_bar_target_at` resolves the monitor under pointer (`src/bar.rs:204-237`).
- `TagBarPolicy`: `[bar].show`/`tag_slots` with per-monitor and wildcard overrides (`src/bar/policy.rs:1-50`).
- Per-tag override: `toggle_bar` stores `per_tag_state().show_bar = Some(...)` on the selected view (`src/toggles.rs:76-91`); cleared by reload/config writes.
- Tray monitor resolution: selected monitor, or 1-based pinning with fallback (`src/systray/mod.rs:97-113`); status command SIGSTOP/SIGCONT follows bar visibility (`src/bar/status/command.rs:375-384`).

---

## 10. Quick reference — default bar bindings (`src/config/buttons.rs:32-100`)

| Bar element | Button | Action |
|---|---|---|
| LayoutSymbol | L / R / M | LayoutMenu / CycleLayoutNext / ResetLayout |
| WinTitle | L / M / R | click handler / close / click handler |
| WinTitle | Super+L / Super+R | edge scratchpad / instantnotify |
| WinTitle | wheel / Shift+wheel / Ctrl+wheel | focus prev-next / reorder / scale |
| StatusText | L/M/R/wheel, Super combos | menu, terminal, window switch, volume, settings, brightness… |
| Tag | L / R / wheel | DragTagBegin (view or drag-drop) / toggle view / scroll tags |
| Tag | Super+L / Super+R / Alt+L / Super+wheel | set client tag / toggle client tag / drag / shift view |
| CloseButton | L / R | close / lock |
| ResizeWidget | L | draw-window-on-release (Rio-like) |
| ShutDown | L/M/R | shutdown / lock / lockscreen |
| StartMenu | L / R / Shift+L | instantstartmenu / quickmenu / prefix mode |
| BottomBar | press-drag | scroll tags, overview, menu, settings |

---

## 11. Relevance to eswm / e09

eswm has **no built-in bar** — waybar/quickshell are external layer-shell clients, which is the correct Tacit-first boundary. However, several instantWM bar patterns are useful *ideas* if Tacit's shell needs richer WM control:

1. **Title strip as tab strip in maximized mode** — e08s02 made fullscreen a top render band; a future maximized-presentation story could give the bar focus/tab semantics.
2. **Bar hit caches built during render** with click regions for tags/titles — eswm's IPC snapshot already exposes `dock.entries()`; an external bar could drive `FocusWindow`/`ToggleMinimize` over IPC (already implemented) — the same outcome without in-compositor UI.
3. **Drag-to-tag** from a bar/title — eswm IPC currently has `GotoWorkspace`/`move_window_to_workspace` (via Lua keybinds); exposing a `MoveWindowToWorkspace` command would let an external bar replicate instantWM's tag drag.
4. **Typed button binding table** (`src/config/buttons.rs`) — if e09s04 adds a typed action catalog for Lua, the same catalog could back mouse bindings without special-casing in input code.
5. **Per-tag/per-monitor bar visibility** — eswm's `wm.dock_height()` is global; per-output work area (e09s02) would enable this externally.

Do **not** copy the built-in bar itself, the SNI tray host, the i3bar status protocol, or the start-menu integration — Tacit owns those surfaces.
