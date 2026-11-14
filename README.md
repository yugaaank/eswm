# eswm

Standalone Wayland compositor and window manager in Rust, built on Smithay 0.7,
to run Tacit without depending on Hyprland.

## Runtime

- **Default**: DRM/libseat backend — run from a TTY, grabs the VT.
  ```
  ./target/debug/eswm
  ```
- **Nested dev backend** (under an existing Wayland session):
  ```
  TACIT_COMP_BACKEND=winit ./target/debug/eswm
  WAYLAND_DISPLAY=eswm-dev kitty
  ```
- Socket: `$XDG_RUNTIME_DIR/eswm-<wayland-display>.sock` (default `eswm-test`).
- Logs: `/tmp/eswm.log`, `/tmp/eswm-panic.log`.

## Configuration

eswm is configured entirely through `~/.config/eswm/config.lua`
(fallback `$XDG_CONFIG_HOME/eswm/config.lua`) using the `es` Lua API.
There is no JSON config file — everything is Lua.

### Autostart

Commands to run on session start, via `es.dsp.exec_cmd`:

```lua
-- ~/.config/eswm/config.lua
es.dsp.exec_cmd("awww-daemon")
es.dsp.exec_cmd("fuzzel")
es.dsp.exec_cmd("awww img /home/user/Pictures/Wallpapers/wall.jpg")
```

Each command is whitespace-split into program + args and spawned on eswm's
Wayland socket (detached, `TERMINAL=kitty`, logs to `/tmp/<name>-spawn.log`).

### Keybinds

Define keybinds with `es.bind("modifiers+key", "action", [arg])`:

```lua
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")
es.bind("Super+1", "switch_workspace", 1)
es.bind("Ctrl+Alt+vt1", "change_vt", 1)
```

Supported actions: `spawn_terminal`, `close_window`, `switch_workspace`,
`move_window_to_workspace`, `cycle_focus_prev/next`,
`swap_focused_prev/next`, `focus_workspace_prev/next`, `toggle_fullscreen`,
`toggle_maximized`, `toggle_window_mode`, `toggle_layout_mode`, `spawn_fuzzel`,
`quit`, `change_vt`.

Modifiers: `logo` (Super), `ctrl`, `alt`, `shift`.
Keys: `return`, `q`, `f`, `m`, `space`, `backspace`, `left`, `right`, `up`,
`down`, `1`–`9`, `vt1`–`vt12`.

The optional `arg` (integer) is used by `switch_workspace`,
`move_window_to_workspace`, and `change_vt`.

### Run arbitrary commands on keypress

Bind a key to any shell command via `es.bind_cmd("modifiers+key", "command")`:

```lua
es.bind_cmd("Super+r", "rofi -show drun")
es.bind_cmd("Super+e", "emacsclient -c")
```

The command string is whitespace-split into program + args and spawned on
eswm's Wayland socket (detached, `TERMINAL=kitty`, logs to
`/tmp/<name>-spawn.log`).

See `config.example.lua` for a complete example.

### Window decorations

Border size, gaps, border colors, corner radius, titlebar height, and default
layout mode are set via `es.dsp`:

```lua
es.dsp.set_border_size(2)
es.dsp.set_outer_gap(10)
es.dsp.set_inner_gap(10)
es.dsp.set_border_color_focused(0, 200, 0, 255)   -- green
es.dsp.set_border_color_unfocused(80, 80, 80, 255) -- gray
es.dsp.set_corner_radius(0)                        -- 0 = square corners
es.dsp.set_titlebar_height(0)                      -- 0 = no titlebar; eswm-drawn titlebar with close/maximize/minimize buttons when > 0
es.dsp.set_layout_mode("tiling")                   -- "tiling" or "floating"; default mode for new windows
```

Values default to a 2px border, 10px gaps, green focused / gray unfocused
borders, no corner radius, no titlebar, and tiling layout mode.

## Layout mode

`set_layout_mode("tiling")` (default) → new windows are tiled.
`set_layout_mode("floating")` → new windows are floating by default.
The mode persists in config.lua across restarts. Individual windows can still
be toggled between tiled/floating with `Super+Space` or `toggle_window_mode`.

## Floating windows

Floating windows can be moved and resized:
- `Super+left-drag` on a floating window → move
- `Super+right-drag` on a floating window → resize
- Titlebar drag (when `titlebar_height > 0`) → move without Super
- Titlebar buttons → close / maximize / minimize

## Build

```
cargo build
cargo test        # 19/19 green (nested winit path)
```

## Keybinds (udev/DRM backend)

All keybinds are configurable in `~/.config/eswm/config.lua` via `es.bind()`
calls. See the Configuration section above for details and
`config.example.lua` for a complete example.

If no keybinds are defined, no keybinds are bound — eswm has no hardcoded
keybinds.

Supported actions: `spawn_terminal`, `close_window`, `switch_workspace`,
`move_window_to_workspace`, `cycle_focus_prev/next`, `swap_focused_prev/next`,
`focus_workspace_prev/next`, `toggle_fullscreen`, `toggle_maximized`,
`toggle_window_mode`, `toggle_layout_mode`, `spawn_fuzzel`, `quit`, `change_vt`.

Modifiers: `logo` (Super), `ctrl`, `alt`, `shift`. Keys: `return`, `q`, `f`,
`m`, `space`, `backspace`, `left`, `right`, `up`, `down`, `1`–`9`, `vt1`–`vt12`.

## Status

Phase 0: scope cleanup (drop Hyprland as a product requirement), native control
socket, README + license. Then Phase 1: DRM reliability, core Wayland lifecycle,
surface hit-test focus. See `notes/roadmap.md` and `notes/status.md`.

Open defect: none currently tracked.

## License

MIT — see `LICENSE`.
