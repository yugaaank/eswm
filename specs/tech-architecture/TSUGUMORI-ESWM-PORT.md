# Tsugumori → eswm Compatibility Port

**Status:** PLAN ONLY — no configs, scripts, QML, or compositor code changed by this document.
**Reason for Existence:** Tsugumori targets Hyprland + UWSM. eswm is a standalone smithay compositor with a partial Hyprland-compatible IPC shim and no session-lock protocol. This plan defines the exact home-config edits that make Tsugumori's QuickShell + Waybar usable on eswm without touching compositor source.

---

## 1. Locked decisions

| Decision | Value |
|---|---|
| Output | Home configs only. No vendored Tsugumori tree in the eswm repo. Only this plan is added under `specs/`. |
| QuickShell location | `~/.config/tsugumori`. Never `~/.config/qs`. Do not touch the existing `~/.config/quickshell`. |
| Bars | Keep Waybar at **top** (`layer: top`, `position: top`, `height: 38`) and eswm's built-in dock at **bottom** (`es.dsp.set_dock_height(30)`). |
| Lock | Copy lock files, mark nonfunctional, exclude from autostart and keybinds. eswm has no `ext_session_lock_v1`. |
| GPU selector | Copy files, mark nonfunctional. `gpu_ctl.py` requires Hyprland/UWSM and is dead on eswm. |
| Reference use | Tsugumori is MIT reference material. No source, assets, or distinctive implementation copied into eswm. |
| Gate | Local preflight is the gate. CI checks remain skipped per operator instruction. |

---

## 2. Ground truth (verified this session)

**eswm IPC** (`src/ipc.rs`, socket `$XDG_RUNTIME_DIR/hypr/${TACIT_COMP_SIG:-eswm}/.socket.sock`):
- `hyprctl -j` works for: `monitors`, `clients`, `layers`, `devices`, `activewindow`, `activewindowv2`, `activeworkspace`, `workspaces`, `cursorpos`, `version`, `dock`.
- Setters: `dockfocus`, `docktoggle`, `reload`, `workspace`, `closewindow`. No-op soft failures: `startup`, `notify`, `setcursor`, `bind`, `define_submap`.
- Values are **stubbed**: one monitor named `winit`, always `focused: true`; `active_workspace_id` always 1; cursor `"0,0"`; every client on workspace 1. Plan must not promise real multi-monitor/workspace driven UI.
- `cursorpos -j` returns the string `"x,y"`, not an object.
- `hyprctl monitors` (non-`-j`) is **not handled** → returns `invalid request`.

**eswm Lua API** (`src/lua_state.rs`, `src/udev.rs`):
- `es.bind(key, action, [arg])`, `es.bind_cmd(key, command)`, `es.dsp.exec_cmd(cmd)`, `es.dsp.set_terminal(name)`, `es.dsp.set_dock_height(n)`.
- Valid `es.bind` actions: `spawn_terminal`, `spawn_fuzzel`, `close_window`, `switch_workspace`, `move_window_to_workspace`, `cycle_focus_prev/next`, `swap_focused_prev/next`, `focus_workspace_prev/next`, `toggle_fullscreen`, `toggle_maximized`, `toggle_minimize`, `toggle_window_mode`, `quit`, `change_vt`. Unknown action → warn + forward.
- `es.config.reload()` is a **no-op stub**; use `bind_cmd` for restart.

**Tooling:** `qs`, `waybar`, `hyprctl`, `python3`, `awww`, `wpctl`, `brightnessctl`, `grim`, `slurp` present. `wmctrl`, `udiskie`, `hyprshot` missing. `~/.config/waybar` and `~/.config/quickshell` exist and must be preserved.

---

## 3. Compatibility gap matrix

| Tsugumori mechanism | File(s) | eswm reality | Action |
|---|---|---|---|
| `hyprctl monitors` text parse | `quickshell/active-monitor.sh:2` | non-`-j` unhandled | Switch to `hyprctl -j monitors` + `jq -r '.[0].name'` |
| `hyprctl cursorpos -j` object | `scripts/cursor_monitor.py:42-43` | returns `"0,0"` string | Parse comma string; fall back to first monitor name |
| Hyprland IPC socket check | `scripts/gpu_ctl.py:81-91` | no UWSM env, no `hyprland.log` | Mark nonfunctional; disable callers |
| `$XDG_CONFIG_HOME/quickshell/...` | ~20 QML + shell refs listed in §4 | new dir is `tsugumori` | Global rewrite |
| `qs ipc call ctrl toggle` (no `--path`) | waybar `config.jsonc:56`, `hyprland.lua:145-146` | QS selects by exact path | Add `qs ipc --path "$HOME/.config/tsugumori/shell.qml" call ...` |
| `hyprland/workspaces`, `hyprland/window` | waybar `config.jsonc:8-9,24-32` | module needs Hyprland IPC events | Replace with `custom/` script modules over `hyprctl -j` |
| `waybarHeight: 28` | `settings/Settings.qml:57` | Waybar height 38 | Set to 38 |
| Layer-shell panels | `shell.qml:73-88,94-108` | smithay supports layer shell | Keep; verify no overlap with top bar / bottom dock in UAT |
| Session lock | `lockscreen.qml`, `lock.sh`, `hyprlock.conf`, `hypridle.conf` | no `ext_session_lock_v1` | Keep files, nonfunctional, unwired |
| xdg-activation on launch | all launchers | global absent (`BUG-2026-11-22-XDG-ACTIVATION-GLOBAL`) | Known limitation; apps may not focus |

---

## 4. Phase 0 — Preconditions (no writes yet)

1. Back up `~/.config/waybar/config` and `~/.config/waybar/style.css` to `~/.config/waybar/config.bak-eswm` / `style.css.bak-eswm` if present.
2. Back up `~/.config/eswm/config.lua` to `~/.config/eswm/config.lua.bak-tsugumori`.
3. Confirm `~/.config/tsugumori` does not exist (verified absent). Do not remove or rename `~/.config/quickshell`.
4. Run local preflight before any forward edit:
   `cargo clippy --all-targets && cargo test --bin eswm && cargo build`
   Red gate ⇒ fix-or-log first; never proceed on red.

---

## 5. Phase 1 — Install QuickShell tree under `~/.config/tsugumori`

Copy only (no edits in this phase):
- `Tsugumori/config/quickshell/*` → `~/.config/tsugumori/`
- `Tsugumori/config/waybar/*` → `~/.config/waybar/` (overwrite after Phase 0 backup)
- `Tsugumori/assets/fonts/share-tech-mono/` → `~/.local/share/fonts/share-tech-mono/` (referenced by QML)
- Wallpapers stay referenced from `Tsugumori/assets/wallpapers/`; do not copy 20 MB.
- Lock files copied for archival: `hyprlock.conf`, `hypridle.conf`, `hypr/lock.sh`, `quickshell/widgets/lockscreen.qml`, `quickshell/lock.sh`, `quickshell/lock-handshake.sh`, `quickshell/widgets/lockscreen/`.

No symlinks. Do **not** run `Tsugumori/install.sh` — it writes `~/.config/quickshell` and assumes Hyprland.

---

## 6. Phase 2 — Rewrite QuickShell paths and IPC helpers

Global mechanical rewrite in `~/.config/tsugumori` and the waybar config:

```
$XDG_CONFIG_HOME/quickshell   →  $XDG_CONFIG_HOME/tsugumori
config_home .. "/quickshell"  →  config_home .. "/tsugumori"
xdgConfigHome + "/quickshell/ →  xdgConfigHome + "/tsugumori/"
```

Known sites (verify with grep in §9): `shell.qml:37` (via `active-monitor.sh`), `ctrl.sh:14-15`, `wallpaper.sh:2-3`, `restart.sh:6-7`, `setwallpaper.sh:40-41`, `wave-check.sh:6-9`, `lock.sh:9-14`, `Menu.qml:690`, `ControlCenter.qml:614-617,1122`, `WallpaperPicker.qml:34-48,77,132-133`, `lockscreen.qml:27-29`, `Settings.qml:57-59`.

Helper adaptations:
1. `active-monitor.sh`: `hyprctl -j monitors | jq -r '.[0].name // "winit"'`.
2. `scripts/cursor_monitor.py`: replace `_hyprctl("cursorpos","-j")` object parse with comma-string parse; if x/y are 0 or parse fails, emit `hyprctl -j monitors | jq -r '.[0].name'` (`winit`). Keep the script's CLI name so QML callers do not change.
3. All `qs ipc call ...` invocations gain `--path "$HOME/.config/tsugumori/shell.qml"`.
4. `Settings.qml` `waybarHeight` → `38`.
5. GPU controls: in `ControlCenter.qml` / `ControlCenterView.qml`, guard `gpu_ctl.py` calls behind a nonfunctional flag and render the panel disabled with text "GPU selection requires Hyprland/UWSM — unavailable on eswm." Keep `gpu_ctl.py` on disk unchanged.

*The GPU selector on eswm is a chocolate teapot: present, polished, and incapable of holding anything.*

---

## 7. Phase 3 — Adapt Waybar (top bar retained)

Replace the two Hyprland modules with small `custom/` scripts placed in `~/.config/tsugumori/scripts/`:

- `custom/workspaces`: script prints `hyprctl -j workspaces | jq` as a compact numbered list, e.g. `01 02 03…`; on-click issues `hyprctl workspace <id>`. Because eswm always reports workspace 1, the UI is honest but static.
- `custom/window`: script prints `hyprctl -j activewindow | jq -r '.title // ""'`; keep the red `//` prefix format from the original module.
- Keep `cpu`, `memory`, `network`, `pulseaudio`, `battery`, `clock`, `custom/sidonia`. They are compositor-agnostic.
- `pulseaudio.on-click` → `qs ipc --path "$HOME/.config/tsugumori/shell.qml" call ctrl toggle`.
- `custom/power`: keep the module but change `on-click` to a documented no-op (`true`) and `tooltip-format` to `"Lock unavailable on eswm (no ext-session-lock-v1)"`. The lock file itself stays, nonfunctional.
- `layer: top`, `position: top`, `height: 38` unchanged. Do not disable eswm's bottom dock.
- Preserve Tsugumori's `style.css`; verify it does not set a bottom-only anchor that conflicts with the top bar.

---

## 8. Phase 4 — eswm config.lua keybinds + autostart

Edit `~/.config/eswm/config.lua`. Preserve existing eswm essentials; add Tsugumori actions on non-conflicting keys.

Autostart additions:
```lua
es.dsp.exec_cmd("qs --path " .. os.getenv("HOME") .. "/.config/tsugumori/shell.qml")
es.dsp.exec_cmd("waybar")
```
Keep `awww-daemon` and `vicinae server`. Note `awww` daemon also draws a bottom dock — UAT must confirm it does not collide with eswm's built-in dock; if it does, prefer eswm dock and stop `awww-daemon` (operator call).

Keybind mapping (eswm action left, Tsugumori intent right):

| Key | Action | Notes |
|---|---|---|
| `Super+Return` | `spawn_terminal` | **Kept** — do not steal for player |
| `Super+Alt+Return` | `bind_cmd` → `qs ipc --path … call tsugumoriShell togglePlayer` | remapped from Super+Return |
| `Super+Alt+Shift+Return` | `bind_cmd` → `… toggleFront` | remapped |
| `Super+Tab` | `bind_cmd` → `… call ctrl toggle` | control center |
| `Super+N` | `bind_cmd` → `… toggleNotes` | explicit `--path` |
| `Super+C` | `bind_cmd` → `… toggleClipboard` | explicit `--path` |
| `Super+Space` | existing `vicinae toggle` | existing launcher stays |
| `Super+T` | `spawn_terminal` | after `es.dsp.set_terminal("kitty")` |
| `Super+Alt+M` | `bind_cmd` → `spotify` | remapped; Super+M stays `toggle_minimize` |
| `Super+R` | `bind_cmd` → adapted `~/.config/tsugumori/restart.sh` | `es.config.reload()` is a stub |
| `Super+P` | `bind_cmd` → adapted `~/.config/tsugumori/wallpaper.sh` | wallpaper picker |
| `Super+F` / `Super+D` | `toggle_fullscreen` / `toggle_maximized` | from hyprland.lua |
| `Super+L` | **not bound** | lock nonfunctional; document in config comment |
| XF86 audio/brightness | `bind_cmd` with `wpctl`/`brightnessctl` | from hyprland.lua |
| `Print` | `bind_cmd` with `grim -g "$(slurp)" …` | `hyprshot` missing; skip that binding |

Drop/mark nonfunctional in comments: `toggle_special("magic"/"spotify")`, dwindle layout, mouse drag/resize, `hyprshot`, `hl.dsp.exit()` → keep `Super+Alt+Backspace` `quit` from eswm instead.

---

## 9. Phase 5 — Verification

**Automated (this session, after edits):**
```sh
cd /home/yugaaank/Projects/eswm
cargo clippy --all-targets && cargo test --bin eswm && cargo build
python3 -m json.tool ~/.config/waybar/config.jsonc >/dev/null
! grep -RIn '/qs\|quickshell' ~/.config/tsugumori ~/.config/waybar/config.jsonc
! grep -RIn 'hyprland/workspaces\|hyprland/window' ~/.config/waybar/config.jsonc
! grep -RIn 'toggle_special\|hyprshot\|wmctrl\|udiskie' ~/.config/eswm/config.lua
find ~/.config/tsugumori -name '*.sh' -exec sh -n {} \;
grep -n 'set_dock_height(30)' ~/.config/eswm/config.lua
grep -n 'position.*top' ~/.config/waybar/config.jsonc
```
A failing grep is a real defect, not a pre-existing condition. Fix before UAT.

**Operator DRM UAT (TTY only — never inside an existing Wayland/X11 session; VT grab):**
1. Start eswm from a TTY; confirm Waybar appears at top and eswm dock at bottom, no overlap.
2. `Super+T` opens kitty; `Super+Return` also opens terminal.
3. `Super+Tab` control center, `Super+N` notes, `Super+C` clipboard, `Super+P` wallpaper picker all open and close.
4. `pulseaudio` click opens control center.
5. `Super+F`, `Super+D`, `Super+M`, `Super+q` behave as labeled.
6. Volume/brightness keys and `Print` screenshot work.
7. Confirm lock button is visibly nonfunctional and no session-lock attempt is made.
8. Confirm apps launched from menus may not focus (xdg-activation global absent) — record, do not fix in this epic.
9. Capture screenshot + log to `specs/verifications/` before declaring done.

---

## 10. Non-goals / known limitations

- No compositor code changes. `ext_session_lock_v1` and `xdg-activation` remain follow-up epics.
- No real multi-monitor or workspace UI from IPC: eswm's snapshot reports one `winit` monitor and workspace 1.
- No GPU switching. Lock files are archival only.
- No wallpaper/font vendoring into the eswm repo; assets stay in `Tsugumori/` or user font dir.
- CI status remains unverified by operator instruction; local preflight is the merge gate.

---

## 11. Execution note

This document is the deliverable for the current turn. Implementation requires explicit approval; on approval, execute phases 0→5 in order and record evidence in `specs/verifications/`.
