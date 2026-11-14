# eswm

Standalone Wayland compositor and window manager in Rust, built on Smithay 0.7,
to run Tacit without depending on Hyprland.

## Setup

Requires a Rust toolchain.

```bash
git clone https://github.com/yugaaank/eswm.git
cd eswm
cargo build
cargo test        # 25/25 pass
```

## Run locally

Two backends:

**Nested dev backend** (runs inside your existing Wayland session — easiest for local dev):

```bash
TACIT_COMP_BACKEND=winit ./target/debug/eswm
WAYLAND_DISPLAY=eswm-dev kitty
```

Spawn more clients with `WAYLAND_DISPLAY=eswm-dev <app>`.

**DRM/libseat backend** (default, grabs the VT — run from a TTY):

1. Switch to a spare virtual console: `Ctrl+Alt+F3` (or `F2`–`F6`).
2. Log in with your user.
3. Build (if not already): `cargo build`
4. Run eswm:

```bash
./target/debug/eswm
```

5. To leave eswm and return to your graphical session, press `Ctrl+Alt+F1` (or whichever VT ran your desktop). To come back, switch to the VT eswm is running on.

eswm grabs the VT and takes over the display — do not run it from a terminal inside an existing Wayland/X11 session; use the nested dev backend for that instead.

**Logs:** `/tmp/eswm.log` (info+), `/tmp/eswm-panic.log`, `/tmp/<name>-spawn.log` per spawned program.
Debug logging: `RUST_LOG=eswm=info ./target/debug/eswm`.

**Config** (Lua-only, `~/.config/eswm/config.lua`):

```lua
es.dsp.exec_cmd("kitty")
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")
es.bind("Super+1", "switch_workspace", 1)
es.dsp.set_border_size(2)
es.dsp.set_outer_gap(10)
es.dsp.set_titlebar_height(30)
es.dsp.set_layout_mode("tiling")
```

No hardcoded keybinds — empty config = no keybinds. See `config.example.lua` for a complete example.

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

Floating windows are placed inside the work area (below waybar/panels/dock), not over them.
Floating windows can be moved and resized:
- `Super+left-drag` on a floating window → move
- `Super+right-drag` on a floating window → resize
- Titlebar drag (when `titlebar_height > 0`) → move without Super
- Titlebar buttons → close / maximize / minimize

Tiled windows are inset by `border_size` on all sides and by `titlebar_height` on top, so the drawn frame (titlebar + borders) never overlaps a top panel. The titlebar extends above `target_rect.y`, so this accounting matches the maximized path.

## Build

```
cargo build
cargo test        # 25/25 green (22 unit + 3 integration)
```

## Testing

```
cargo test          # 25/25 pass (22 unit + 3 integration)
```

Unit tests live in `src/wm.rs` (tiler, floating, focus, fullscreen). Integration
tests live in `tests/`:
- `tests/globals.rs` — Wayland shell globals advertise correctly
- `tests/ipc.rs` — hyprctl-compatible IPC socket
- `tests/control.rs` — native control socket end-to-end

Run a single test binary:

```
cargo test --test ipc
```

Debug build output goes to `/tmp/eswm.log` (info+) and `/tmp/eswm-panic.log`.

### Nested dev backend (no TTY needed)

Run eswm inside your existing Wayland session:

```
TACIT_COMP_BACKEND=winit ./target/debug/eswm
WAYLAND_DISPLAY=eswm-dev kitty
```

The nested compositor listens on `$XDG_RUNTIME_DIR/eswm-eswm-dev.sock`.
Log level via `RUST_LOG`:

```
RUST_LOG=eswm=info ./target/debug/eswm
```

## Development

- `src/main.rs` — `State`, backend dispatch (winit vs DRM), xdg/layer/seat/compositor handlers, `new_toplevel`, `apply_layout`, `prune_dead_windows`, `spawn_on_socket`, winit run loop.
- `src/udev.rs` — DRM/KMS + libinput backend (primary path), session pause/resume, per-connector outputs, input, keybinds (`process_shortcut`), dock strip hit-test.
- `src/wm.rs` — `WindowManager`: outputs/workspaces/windows, master-stack tiler, floating cascade, work_area-based placement, unit tests.
- `src/lua_state.rs` — Lua API: `es.bind`, `es.bind_cmd`, `es.dsp.exec_cmd`.
- `src/control.rs` — native control socket (`$XDG_RUNTIME_DIR/eswm-<wayland-display>.sock`), newline-delimited JSON.
- `src/ipc.rs` — hyprctl-compatible socket + `.socket2.sock` event stream (historical prototype).

Keybinds and autostart are configured entirely in Lua (`~/.config/eswm/config.lua`); there are no hardcoded keybinds and no JSON config.

### Running on DRM (default)

From a TTY, eswm grabs the VT:

```
./target/debug/eswm
```

Switch VTs with `Ctrl+Alt+F1`/`F3` to leave and return.

### Example dev config

```lua
-- ~/.config/eswm/config.lua
es.dsp.exec_cmd("kitty")
es.bind("Super+Return", "spawn_terminal")
es.bind("Super+q", "close_window")
es.bind("Super+1", "switch_workspace", 1)
es.bind_cmd("Super+r", "rofi -show drun")
es.dsp.set_border_size(2)
es.dsp.set_outer_gap(10)
es.dsp.set_titlebar_height(30)
es.dsp.set_layout_mode("tiling")
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
