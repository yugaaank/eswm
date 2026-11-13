# eswm working notes — current state (2026-11-14)

## Repo

- **Branch:** `main` (only branch; `feature/standalone-tacit` merged and deleted)
- **Remote:** `origin` = `https://github.com/yugaaank/eswm.git`, `main` pushed and up to date
- **HEAD:** `1d1a476` feat: keybinds come only from config.json, no defaults in code

## Build / run

- `cargo build` — green, 36 pre-existing warnings, no new ones
- `cargo test` — 16 unit + 4 integration tests pass
- Native DRM is the default backend: `./target/debug/eswm` from a TTY (grabs the VT; Ctrl+Alt+F1/F3 to leave/return)
- Nested dev backend: `TACIT_COMP_BACKEND=winit ./target/debug/eswm` then `WAYLAND_DISPLAY=eswm-dev kitty`
- Wayland socket: `$XDG_RUNTIME_DIR/eswm-<wayland-display>.sock` (default `eswm-test`)
- Logs: `/tmp/eswm.log` (info+), `/tmp/eswm-panic.log`, `/tmp/<name>-spawn.log` per spawned program
- No special build config needed — libdisplay-info is provided by the system (0.3.x, ABI-compatible enough for smithay 0.7's usage). `.cargo/config.toml` is essentially empty.

## Configuration (Lua-only)

eswm is configured exclusively through `~/.config/eswm/config.lua` using the `es` Lua API. No JSON config.

### Autostart — `es.dsp.exec_cmd`

```lua
es.dsp.exec_cmd("awww-daemon")
es.dsp.exec_cmd("fuzzel")
es.dsp.exec_cmd("awww img /home/user/Pictures/Wallpapers/wall.jpg")
```

Each command is whitespace-split into program + args, spawned on eswm's socket (detached, `TERMINAL=kitty`, logs to `/tmp/<name>-spawn.log`).

### Keybinds — `es.bind` and `es.bind_cmd`

```lua
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")
es.bind("Super+1", "switch_workspace", 1)
es.bind_cmd("Super+r", "rofi -show drun")
```

- `es.bind("modifiers+key", "action", [arg])` — actions: `spawn_terminal`, `close_window`, `switch_workspace`, `move_window_to_workspace`, `cycle_focus_prev/next`, `swap_focused_prev/next`, `focus_workspace_prev/next`, `toggle_fullscreen`, `toggle_maximized`, `toggle_window_mode`, `toggle_layout_mode`, `spawn_fuzzel`, `quit`, `change_vt`
- `es.bind_cmd("modifiers+key", "command")` — spawns arbitrary command on keypress
- Modifiers: `logo` (Super), `ctrl`, `alt`, `shift`. Keys: `return`, `q`, `f`, `m`, `space`, `backspace`, `left`, `right`, `up`, `down`, `1`–`9`, `vt1`–`vt12`
- No keybinds are hardcoded; empty config = no keybinds

## Code map

- `src/main.rs` — `State`, backend dispatch (`TACIT_COMP_BACKEND=winit` else DRM), xdg/layer/seat/compositor handlers, `new_toplevel`, `apply_layout`, `prune_dead_windows`, `spawn_on_socket`, `spawn_terminal`, winit run loop. No Config/Keybind structs (all config is Lua).
- `src/udev.rs` — DRM/KMS + libinput backend (primary path), session pause/resume, per-connector outputs, render_surface, input, keybinds (`process_shortcut` calls Lua-registered handlers), pointer focus/drag helpers.
- `src/wm.rs` — WindowManager: outputs/workspaces/windows, master-stack tiler, LayoutMode, floating cascade, window_at hit test, cycle_focus, unit tests.
- `src/ipc.rs` — hyprctl-compatible socket + `.socket2.sock` event stream (historical prototype, not part of product goal).
- `src/lua_state.rs` — Lua API: `es.bind`, `es.bind_cmd`, `es.dsp.exec_cmd` (Keybind with `cmd: Option<String>`, exec_cmd handler).
- `src/control.rs` — native control socket (`$XDG_RUNTIME_DIR/eswm-<wayland-display>.sock`), newline-delimited JSON.

## Feature state

- M1 nested Winit compositor: complete (dev only)
- M2 Hyprland-compatible IPC: complete (historical prototype)
- M3 Lua configuration API: complete — `es.bind`, `es.bind_cmd`, `es.dsp.exec_cmd`
- M4 DRM/libseat backend: complete — primary path, TTY startup, outputs, layers, pause/resume, cursor
- M5 window management: complete — master-stack tiling, floating, workspaces, focus cycle, click-to-focus verified on DRM
- Config: complete — Lua-only autostart and keybinds
- Animations: not implemented

## Open defects / known limits

- No visible focus borders on winit dev backend (DRM has them)
- No window rules (class → workspace/float), no gaps
- Multi-output workspaces: second connector gets no workspaces of its own
- Click-to-focus on DRM: verified working 2026-11-12
- Tiling-to-floating focus change bug (parked): after Super+Space on focused window, Super+arrows may not change focus to tiled windows
- IPC `CompCmd::GotoWorkspace`/`CloseWindow` no-ops in winit path; not drained in DRM path

## Documentation

- `README.md` — runtime, Lua config (autostart, keybinds, bind_cmd), build, keybinds reference
- `notes/roadmap.md` — phases 0–7, Phase 0 complete, Phase 1 complete
- `notes/tacit-integration.md` — Tacit/QML compat contract, native state/action API, layer-shell contract
- `config.example.lua` — complete example config

## License

MIT — see `LICENSE`.
