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

Session autostart is read from `$XDG_CONFIG_HOME/eswm/config.json`
(fallback `~/.config/eswm/config.json`). If the file is missing or invalid,
nothing autostarts — configure your entries in the config file.

Format — see `config.example.json`:

```json
{
  "autostart": [
    "awww-daemon",
    "fuzzel",
    "awww img /home/user/Pictures/Wallpapers/wall.jpg"
  ]
}
```

Each entry is whitespace-split into program + args and spawned on eswm's
Wayland socket (detached, `TERMINAL=kitty`, logs to `/tmp/<name>-spawn.log`).
`es.dsp.exec_cmd` from the Lua API spawns commands the same way.

## Lua configuration

In addition to `config.json`, eswm loads `$XDG_CONFIG_HOME/eswm/config.lua`
(fallback `~/.config/eswm/config.lua`) on startup if it exists. Use it to
define keybinds and run setup commands via the `es` Lua API:

```lua
-- ~/.config/eswm/config.lua
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")
es.dsp.exec_cmd("fuzzel")
```

Supported `es.bind` actions: `spawn_terminal`, `close_window`,
`switch_workspace`, `move_window_to_workspace`, `cycle_focus_prev/next`,
`swap_focused_prev/next`, `focus_workspace_prev/next`, `toggle_fullscreen`,
`toggle_maximized`, `toggle_window_mode`, `toggle_layout_mode`, `spawn_fuzzel`,
`quit`, `change_vt`. Modifiers: `logo` (Super), `ctrl`, `alt`, `shift`.

See `config.example.lua` for a complete example.

Keybinds from both `config.json` (keybinds array) and `config.lua` (`es.bind`)
are merged; config.json keybinds take precedence over Lua keybinds. If neither
is configured, no keybinds are bound.

## Build

```
cargo build
cargo test        # 19/19 green (nested winit path)
```

## Keybinds (udev/DRM backend)

All keybinds are configurable in `$XDG_CONFIG_HOME/eswm/config.json` (keybinds
array) or `~/.config/eswm/config.lua` (`es.bind()` calls). See the
Configuration section above for details and `config.example.json` /
`config.example.lua` for complete examples.

If neither config file defines keybinds, no keybinds are bound — eswm has no
hardcoded keybinds.

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
