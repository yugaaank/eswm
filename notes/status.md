# eswm working notes — current state (2026-11-14)

## Repo

- **Branch:** `main` (only branch; `feature/standalone-tacit` merged and deleted)
- **Remote:** `origin` = `https://github.com/yugaaank/eswm.git`, `main` pushed and up to date
- **HEAD:** `36c0374` docs: update notes to reflect current project state (3 commits ahead of origin/main)
- **Last feature commit:** `99aa59e` feat: es.bind_cmd() to run arbitrary commands on keypress

## Build / run

- `cargo build` — green, 40 warnings (pre-existing), no new ones
- `cargo test` — 25/25 pass (22 unit + 3 integration: control, globals, ipc)
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
- M5 window management: complete — master-stack tiling, floating, workspaces, focus cycle, click-to-focus verified on DRM, visible focus borders (winit + DRM), gaps, window rules (app_id → workspace/mode), layer-surface exclusive-zone tiling (windows inset below waybar/panels)
- Config: complete — Lua-only autostart and keybinds
- Wayland protocols: xdg_output added alongside wl_output, xdg_shell, layer_shell, seat, compositor, shm, viewporter, data_device, dmabuf
- Animations: not implemented

## Open defects / known limits

- Multi-output workspaces: second connector gets no workspaces of its own
- Click-to-focus on DRM: verified working 2026-11-12
- Tiling-to-floating focus change bug (parked): after Super+Space on focused window, Super+arrows may not change focus to tiled windows
- IPC `CompCmd::GotoWorkspace`/`CloseWindow` no-ops in winit path; not drained in DRM path
- Focus borders on DRM: fixed green overlay (borders now render as thin ring behind windows, not filled rect on cursor plane)
- xdg_output (`zxdg_output_manager_v1`) now advertised — waybar starts and connects (was crashing exit 1 without it)
- Tiling respects layer-surface exclusive zones: `wm.work_area` set from `LayerMap::non_exclusive_zone()` on each commit, so tiled windows are inset below waybar/panels instead of covering them
- Floating placement now uses `work_area` (not raw output geometry), so new floating windows no longer spawn over panels
- Tiled inner rect inset by `border_size` (all sides) + `titlebar_height` (top) so drawn frames never overlap top panels
- Dock strip hit-test uses `work_area` bottom instead of raw dock geometry, matching the render side
- Added tracing logs: layer mapped events + work_area updates in `main.rs`; key-press debug in `udev.rs`

## Documentation

- `README.md` — runtime, Lua config (autostart, keybinds, bind_cmd), build, keybinds reference
- `notes/roadmap.md` — phases 0–7, Phase 0 complete, Phase 1 complete
- `notes/tacit-integration.md` — Tacit/QML compat contract, native state/action API, layer-shell contract
- `config.example.lua` — complete example config

## License

MIT — see `LICENSE`.
