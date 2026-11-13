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
eswm falls back to the hardcoded startup (random wallpaper via awww + fuzzel).

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

## Build

```
cargo build
cargo test        # 19/19 green (nested winit path)
```

## Keybinds (udev/DRM backend)

- `Super+Return` — spawn kitty
- `Super+Q` — close focused window
- On startup: random wallpaper via awww, fuzzel launcher as a layer surface
- `Super+1..9` — switch workspace
- `Super+Left/Right` — previous/next window
- `Super+Space` — toggle focused window Tiled/Floating
- `Super+Alt+Space` — fuzzel launcher
- `Super+Shift+Space` — toggle global spawn mode
- `Super+LMB/RMB` — drag floating window move/resize
- `Super+F` — fullscreen · `Super+M` — maximize
- `Super+Shift+1..9` — move window to workspace
- `Super+Alt+Left/Right` — previous/next workspace
- `Ctrl+Alt+Backspace` — quit · `Ctrl+Alt+F1..12` — VT switch

## Status

Phase 0: scope cleanup (drop Hyprland as a product requirement), native control
socket, README + license. Then Phase 1: DRM reliability, core Wayland lifecycle,
surface hit-test focus. See `notes/roadmap.md` and `notes/status.md`.

Open defect: none currently tracked.

## License

MIT — see `LICENSE`.
