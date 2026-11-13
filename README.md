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

Keybinds are also configurable in the same config file. See `config.example.json`
for the full list of supported actions (`spawn_terminal`, `close_window`,
`switch_workspace`, `move_window_to_workspace`, `cycle_focus_prev/next`,
`swap_focused_prev/next`, `focus_workspace_prev/next`, `toggle_fullscreen`,
`toggle_maximized`, `toggle_window_mode`, `toggle_layout_mode`, `spawn_fuzzel`,
`quit`). Config keybinds override the hardcoded defaults; any keybind not
configured falls back to the default listed below.

## Build

```
cargo build
cargo test        # 19/19 green (nested winit path)
```

## Keybinds (udev/DRM backend)

All keybinds are configurable in `$XDG_CONFIG_HOME/eswm/config.json` (see
`config.example.json`). Config keybinds override these defaults; anything not
configured uses the hardcoded default below.

- `Super+Return` — spawn kitty
- `Super+Q` — close focused window
- `Super+1..9` — switch workspace
- `Super+Shift+1..9` — move focused window to workspace
- `Super+Left/Right` — previous/next window (cycle focus)
- `Super+Ctrl+Left/Right` — swap focused window with neighbor
- `Super+Alt+Left/Right` — previous/next workspace
- `Super+Space` — toggle focused window Tiled/Floating
- `Super+Shift+Space` — toggle global spawn mode (tiling/floating cascade)
- `Super+Alt+Space` — fuzzel launcher
- `Super+F` — fullscreen · `Super+M` — maximize
- `Super+LMB/RMB` — drag floating window move/resize
- `Ctrl+Alt+Backspace` — quit · `Ctrl+Alt+F1..12` — VT switch

## Status

Phase 0: scope cleanup (drop Hyprland as a product requirement), native control
socket, README + license. Then Phase 1: DRM reliability, core Wayland lifecycle,
surface hit-test focus. See `notes/roadmap.md` and `notes/status.md`.

Open defect: none currently tracked.

## License

MIT — see `LICENSE`.
