# Tacit integration contract

Tacit is the first real consumer of eswm. The compositor is standalone and
must not depend on Hyprland, `hyprctl`, Hyprland sockets, or Hyprland Lua
helpers. Tacit currently depends on those interfaces, so this document records
the behavior eswm must provide natively and the work needed to migrate Tacit.

## Current Tacit dependencies

The current shell uses:

- Quickshell's `Hyprland` and `HyprlandToplevel` objects.
- `hyprctl` queries for clients, monitors, layers, workspaces, active workspace,
  devices, cursor position, and keybinds.
- `Hyprland.dispatch` for focus, close, workspace, move, resize, float,
  fullscreen, swap, submap, and shell actions.
- Hyprland raw events for window, focus, workspace, monitor, keyboard-layout,
  and configuration changes.
- Hyprland configuration files and reloads for layout, gaps, borders, opacity,
  animation, monitor, keyboard, pointer, and touchpad settings.
- Layer-shell surfaces with background, bottom, top, and overlay layers.

These are migration inputs, not APIs that eswm should reproduce.

### Evidence inventory (3A, 2026-11-05)

Counted in the Tacit working tree (`c852bf1`), excluding `node_modules`:

| Surface | Where | What it needs |
| --- | --- | --- |
| 46 `Hyprland.dispatch` calls | `services/HyprlandBackend.qml`, `IslandController.qml`, overview/window menu modules | `focus{window,workspace}`, `window.close`, `window.move{workspace,x,y,exact}`, `window.resize{x,y,exact}`, `window.float{set,unset}`, `window.fullscreen{mode=maximized,set,unset}`, `window.swap`, `submap`, `global` |
| `Hyprland.monitors` (12), `focusedMonitor` (23), `monitorFor` (8) | shell-wide | output geometry `x,y,scale`, name, active workspace, focused output |
| `Hyprland.toplevels`/`workspaces` (8) | `HyprlandData.qml`, `WM.qml` | window list + workspace list with `lastwindow`, membership |
| `hyprctl clients/monitors/workspaces/layers/activeworkspace -j` | `services/HyprlandData.qml` | fields actually read: `address`, `title`, `class`, `workspace.id`, `mapped`, `fullscreen` (bit 2 = true fullscreen), `size`, monitor `name/x/y/scale`, layer namespaces |
| `hyprctl activewindow`, `cursorpos`, `devices`, `setcursor`, `reload`, `keyword`, `notify` | scripts, `services/*` | focused window identity, pointer position, input devices, cursor theme, config reload |
| 123 lines of binds | `session/hypr/tacit/keybinds.lua` | `exec_cmd`, `global("quickshell:*")`, `submap` (`flow`, `reset`) |
| 111 lines of rules | `session/hypr/tacit/rules.lua` | `window_rule{class regex, opacity {active,inactive}, rounding}`, `layer_rule{namespace, blur, ignore_alpha, no_anim}` |
| 155 lines of config | `session/hypr/tacit/general.lua` | gaps, borders, `blur:xray`, animation curves, gestures, monitor/keyboard/pointer settings |
| `ToplevelManager` (24 uses) | `AgentSessions.qml`, `Attention.qml`, `Files.qml`, `Notifications.qml` | foreign-toplevel list with activate/close |
| `ScreencopyView` (6) | overview, region selector, recorder | image capture of outputs and windows |
| `IdleInhibitor` (2) | video/fullscreen paths | idle inhibit while a client asks |
| `GlobalShortcut` (58) | shell-wide | shell receives key events for its own shortcuts |

Two facts that shape the API:

1. Tacit talks to Quickshell's Hyprland plugin, not to `hyprctl` directly, for
   state and dispatch. Replacing the backend (`WM.qml` -> `EswmBackend.qml`) is
   therefore a smaller change than rewriting every call site.
2. The shell treats "true fullscreen" (bit 2) and "maximized" separately. eswm
   currently only has `WindowMode::{Tiled,Floating}`, so maximized and
   fullscreen states must be added as separate window states, not reused.


## Native state API requirements

Tacit's replacement backend needs observable state for:

### Windows

- Stable native window ID.
- Application ID/class and title.
- Workspace ID and output/monitor ID.
- Logical position and size.
- Tiled, floating, maximized, and fullscreen state.
- Mapped, visible, urgent, and focused state.
- PID where available.
- Close and focus eligibility.

### Workspaces

- Stable numeric ID and display name.
- Output/monitor assignment.
- Active state per output.
- Window membership.
- Creation, removal, rename, and current-window state.

The model is a workspace model: each managed window belongs to one workspace.
It is not Pinnacle's multi-tag model.

### Outputs

- Stable name and ID.
- Position, logical size, scale, transform, and refresh rate.
- Available/current modes.
- Active workspace.
- Reserved space from layer-shell exclusive zones.
- Focused output state.

### Layers and input

- Layer surfaces grouped by output and layer level.
- Namespace, geometry, visibility, and exclusive zone.
- Keyboard-interactivity mode.
- Pointer position.
- Keyboard layout names/codes and active layout.
- Lock-screen and focus state needed by Tacit's shell surfaces.

## Native actions

The first action API should support:

- Focus or close a window.
- Switch to a workspace.
- Move a window to a workspace, with optional follow behavior.
- Move or resize a floating window.
- Toggle floating, maximized, and fullscreen state.
- Swap or reorder tiled windows.
- Focus the next/previous or directional window.
- Switch workspaces relatively.
- Register and remove compositor keybinds.
- Enter and leave a temporary keybind submap.
- Reload Tacit/eswm configuration.
- Change keyboard layout and input settings.
- Change output mode or position when output management is ready.

Actions should use structured messages rather than embedded Lua source strings.

## Wayland protocol gaps

eswm currently advertises: `wl_compositor`, `wl_shm`, `wl_data_device_manager`,
`xdg_wm_base`, `zwlr_layer_shell_v1`, `wl_output`, `wl_seat`, dmabuf (DRM path).
Tacit needs these additionally, in rough priority order:

1. `zwlr_foreign_toplevel_manager_v1` (or `ext_foreign_toplevel_list_v1`) —
   `ToplevelManager` powers alt-tab, attention, agent sessions, notifications.
2. Screen capture: `wlr_screencopy_manager_v1` / `ext_image_copy_capture_v1` —
   overview thumbnails, region picker, recording.
3. `ext_session_lock_v1` — the lock screen must not be a plain fullscreen layer.
4. `xdg_activation_v1` — launching a client focused.
5. `wp_idle_inhibit_v1`, `wp_fractional_scale_v1`, `wp_viewporter`,
   `wp_presentation` — idle inhibit for video, HiDPI, video scaling, frame timing.
6. `wp_pointer_constraints_v1` + `wp_relative_pointer_v1` — games and 3D clients.
7. `zwp_keyboard_shortcuts_inhibit_v1` — VMs and browsers grabbing keys.

Not needed: `xdg_decoration` (clients keep CSD), XWayland.

## Visual features Tacit's config expects

`session/hypr/tacit/rules.lua` and `general.lua` assume these exist in the
compositor. eswm has none of them today; they are a separate workstream from
the control channel:

- Layer blur by namespace, with `ignore_alpha` threshold and `no_anim`.
- Per-window blur and per-window opacity, `{active, inactive}`.
- Window rounding by class.
- Animation curves for workspace switch, window open/close, layer open/close.
- Gaps and border width/colour.
- Gamma/night-light (`hyprsunset` today) and cursor theme switching.

Ranked by visible impact for Tacit: per-window opacity and rounding first
(cheap, affects every window), then workspace/layer animation, then blur
(expensive, needs offscreen sampling and is the one real rendering feature).

## Control channel v0

One socket, `$XDG_RUNTIME_DIR/eswm-<wayland-display>.sock`, newline-delimited
JSON, no process spawns. Tacit sends requests and receives events on it.
Request: `{"id": 7, "cmd": "window.focus", "args": {"id": 3}}`.
Reply: `{"id": 7, "ok": {}}` or `{"id": 7, "error": "unknown window"}`.
Event (not sent yet): `{"event": "window.focused", "window": 3}`.

Implemented in `src/control.rs` (2026-11-05): socket thread + per-connection
reader, requests applied on the compositor tick so they see live WM state, reply
per request, `tests/control.rs` drives a nested compositor end to end.

Live: `state`, `window.list`, `workspace.list`, `output.list`, `window.focus`,
`window.close`, `window.move {workspace}`, `window.move_to`, `window.resize`,
`window.set_mode`, `workspace.focus {id|delta}`.
Not live yet: the event stream (clients poll `state`), `output.focus`,
`config.reload`, `window.swap`, and multi-output `workspace.focus` (primary
output only). Windows are identified by a numeric `id`, not a Hyprland address.

## eswm Lua configuration (replaces Hyprland keybinds/config)

eswm is configured exclusively through `~/.config/eswm/config.lua` using the
`es` Lua API. There is no JSON config and no hardcoded keybinds.

- `es.bind("modifiers+key", "action", [arg])` — registers a keybind
- `es.bind_cmd("modifiers+key", "command")` — spawns an arbitrary command on keypress
- `es.dsp.exec_cmd("command")` — spawns a command (used for autostart)

Modifiers: `logo`, `ctrl`, `alt`, `shift`. Keys: `return`, `q`, `f`, `m`,
`space`, `backspace`, `left`, `right`, `up`, `down`, `1`–`9`, `vt1`–`vt12`.
Actions: `spawn_terminal`, `close_window`, `switch_workspace`,
`move_window_to_workspace`, `cycle_focus_prev/next`, `swap_focused_prev/next`,
`focus_workspace_prev/next`, `toggle_fullscreen`, `toggle_maximized`,
`toggle_window_mode`, `toggle_layout_mode`, `spawn_fuzzel`, `quit`, `change_vt`.

Example `config.lua` (see `config.example.lua` for full):

```lua
es.dsp.exec_cmd("awww-daemon")
es.dsp.exec_cmd("fuzzel")
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")
es.bind("Super+1", "switch_workspace", 1)
es.bind_cmd("Super+r", "rofi -show drun")
```

Queries:

- `state` — full snapshot: outputs, workspaces, windows, layers, focus.
- `window.list`, `workspace.list`, `output.list`, `layer.list`.

Actions mapped 1:1 onto existing `wm` methods (no policy in the socket layer):

- `window.focus {id}`, `window.close {id}`
- `window.move {id, workspace}` (no follow; follow is decided by the shell)
- `window.move_to {id, x, y}`, `window.resize {id, width, height}` — exact,
  floating only, clamped to the output like a drag
- `window.set_mode {id, mode: "tiled"|"floating"}`
- `window.fullscreen {id?, mode: "fullscreen"|"maximized", on: bool}`
- `window.swap {a, b}`
- `workspace.focus {id}` or `{delta: 1|-1}`
- `output.focus {id}`
- `config.reload`

Deliberate omissions: workspace `follow`, `submap`, and Lua-source dispatch
strings. Tacit's `submap` usage is shell state; it should live in the shell, not
in the compositor.

Shortcuts are not dispatched through this socket. eswm owns the binds; a bind
whose action is `shell` sends `{"event": "shortcut", "name": "..."}` to Tacit,
which is the replacement for Hyprland's `global("quickshell:*")`.

## Events

Tacit needs pushed events for:

- Window opened, closed, title/app ID changed, mapped, unmapped, and focused.
- Workspace created, removed, renamed, switched, or changed membership.
- Output added, removed, changed, or focused.
- Layer surface added, removed, or changed.
- Keyboard layout changed.
- Configuration reloaded.
- Animation state changed when the shell needs to synchronize visual state.

Events should be coalesced and sent through one native per-user control/event
channel rather than starting a process for every update.

## Layer-shell contract

Tacit uses many layer-shell surfaces, including:

- Background wallpaper and desktop widgets.
- Bottom surfaces.
- Top panels, bars, frames, and the notch.
- Overlay notifications, menus, lock screens, selectors, and dialogs.

eswm must implement output selection, anchors, margins, exclusive zones,
keyboard interactivity, popup stacking, and correct rendering order. A layer
surface must not be treated as a full-window surface by default.

## Startup and session requirements

- eswm starts directly from a Wayland session on DRM/libseat.
- Tacit starts as a normal Wayland shell client after the compositor socket is
  available.
- Environment setup must advertise the eswm session and socket to Quickshell
  and child applications.
- The migration must remain Wayland-only, following Tacit's project rules.
- Existing external services such as audio, networking, notifications, and
  application launching remain separate from the compositor unless they need a
  compositor action.

## Migration order

1. Implement a small native state/action channel and a test client.
2. Build eswm's window, workspace, output, and layer state from the compositor
   itself instead of synthetic IPC snapshots.
3. Implement focus, close, workspace switching, floating, and fullscreen.
4. Add Tacit's native Quickshell backend and migrate the shared `WM` service.
5. Migrate overview, window menu, workspace model, notifications, input layout,
   and other direct Hyprland users one group at a time.
6. Move Tacit's startup and configuration actions to native eswm actions.
7. Remove the old Hyprland backend and session dependencies after the native
   path covers the required shell behavior.
