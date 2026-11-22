# Tsugumori port — static verification record

**Date:** 2026-11-22
**Plan:** `specs/tech-architecture/TSUGUMORI-ESWM-PORT.md`
**Scope:** Home-config-only port. No compositor code changed. Live DRM UAT remains operator-only.

## Implemented

- Copied QuickShell tree to `~/.config/tsugumori`; Waybar config/style/assets to `~/.config/waybar`.
- Backups: `config.lua.bak-tsugumori`, `config.jsonc.bak-eswm`, `style.css.bak-eswm`.
- Rewrote every config-path reference from `/quickshell/` to `/tsugumori/`.
- `active-monitor.sh` now uses `hyprctl -j monitors | jq`.
- `cursor_monitor.py` accepts both eswm's `"x,y"` cursorpos string and Hyprland's object.
- Waybar: `hyprland/workspaces` + `hyprland/window` replaced with `custom/workspaces` + `custom/window` scripts reading eswm IPC. Power button is a documented no-op. Control-center click passes explicit `--path`.
- `Settings.qml waybarHeight: 38` matches Waybar `height: 38`.
- GPU panel renders disabled "Unavailable on eswm"; `gpu_ctl.py` is never invoked.
- QuickShell menu LOCK button disabled; `compat/NONFUNCTIONAL.md` lists all dormant files.
- `config.lua`: autostarts QS + Waybar, adds Tsugumori panels on non-conflicting keys, keeps Super+Return as terminal, leaves Super+L unbound, keeps bottom dock at 30. `Super+Space` opens the Tsugumori app grid via `tsugumoriShell toggleMenu` (verified against `ShellIpc.qml` target/method); vicinae stays autostarted but unbound.

## Finding: IPC signature not exported

eswm binds its compat IPC at `$XDG_RUNTIME_DIR/hypr/${TACIT_COMP_SIG:-eswm}/.socket.sock`, but `spawn_program()` (src/main.rs:1337) exports only `WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP` and `TERMINAL`. `hyprctl` discovers the socket through `HYPRLAND_INSTANCE_SIGNATURE`. A stale inherited signature makes every panel script query the wrong compositor.

**Home-config mitigation:** autostart and bind commands prefix `env HYPRLAND_INSTANCE_SIGNATURE=eswm`. This couples to the default `TACIT_COMP_SIG`; if the operator overrides it, the prefix must match. Fixing the export in compositor code is a separate approved change.

## Static checks (all green)

```
cargo clippy --all-targets            PASS
cargo test --bin eswm                 PASS  169 passed
cargo build                           PASS
bash -n  (all ~/.config/tsugumori/*.sh) PASS
python3 -m py_compile cursor_monitor.py PASS
python3 -m json.tool config.jsonc     PASS
luac -p ~/.config/eswm/config.lua     PASS
grep stale /quickshell/ refs          NONE
grep ~/.config/qs refs                NONE
grep hyprland/* Waybar modules        NONE
grep set_dock_height(30)              PASS
grep Waybar position/layer top        PASS
```

Helper smoke against the dormant eswm IPC socket: `active-monitor.sh` → `winit`, workspaces → `1`, window → empty, cursor monitor → no match. All degrade safely.

## Remaining: operator DRM UAT

Must run from a TTY, never inside an existing Wayland/X11 session (VT grab):

1. Waybar top, eswm dock bottom, no overlap.
2. `Super+T` / `Super+Return` open kitty.
3. `Super+Tab` control center, `Super+N` notes, `Super+C` clipboard, `Super+P` wallpaper picker.
4. PulseAudio click opens control center.
5. `Super+F`, `Super+D`, `Super+M`, `Super+Q`.
6. Audio/brightness keys, `Print` screenshot.
7. LOCK button visibly disabled; no session-lock attempt.
8. Record the known xdg-activation focus gap (`BUG-2026-11-22-XDG-ACTIVATION-GLOBAL`).

No live compositor run was performed in this session.
