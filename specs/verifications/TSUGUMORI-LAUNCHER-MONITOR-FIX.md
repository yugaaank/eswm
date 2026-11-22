# Tsugumori launcher — monitor-name mismatch fix

**Date:** 2026-11-22
**Symptom:** `Super+Space` does not open the Tsugumori app grid.
**Scope:** Home configs only (`~/.config/tsugumori`). No compositor code changed.

## Root cause

The keybind itself was correct end to end:

1. `~/.config/eswm/config.lua:48` → `qs ipc --path shell.qml call -- tsugumoriShell toggleMenu`
2. `services/ShellIpc.qml:19` target `tsugumoriShell`, `services/ShellIpc.qml:46` `toggleMenu()` → `menuRequested()`
3. `shell.qml:55-57` → starts the `detectMonitor` `Process`
4. `shell.qml:37` runs `active-monitor.sh`

The failure is a name mismatch introduced by eswm's compatibility shim:

- `src/ipc.rs:422` hardcodes every monitor name as `"winit"`.
- `src/udev.rs:2116` creates the real Wayland output with the DRM connector name (`eDP-1` in this session).
- `active-monitor.sh` read the IPC name, so `root.menuActiveMonitor` became `"winit"`.
- `shell.qml:85` filtered menu toggles with `if (root.menuActiveMonitor !== modelData.name) return`.
- `modelData.name` came from `Quickshell.screens` — the **real** DRM name.
- `"winit" !== "eDP-1"` → the toggle was swallowed; no panel ever opened.

The compositor was describing one monitor with two different names, and the shell believed both. *A fork with one tine.*

## Fix

Two defensive home-config changes, so the launcher works even if one layer regresses:

1. `active-monitor.sh`: when `XDG_CURRENT_DESKTOP=eswm` (exported by eswm's `spawn_program`), emit nothing. `shell.qml:42` then keeps the `menuActiveMonitor` default of `Quickshell.screens[0].name`, which is the real output name. On real Hyprland the original `hyprctl` lookup is unchanged.
2. `shell.qml:89`: only apply the per-screen name filter when `Quickshell.screens.length > 1`. eswm has exactly one output, so there is no cross-screen ambiguity to protect.

## Verification

```
bash -n active-monitor.sh                         PASS
XDG_CURRENT_DESKTOP=eswm ... active-monitor.sh    → "" (real name retained)
env -u XDG_CURRENT_DESKTOP ... active-monitor.sh  → "eDP-1" (Hyprland path intact)
grep Quickshell.screens.length>1 shell.qml        PASS
luac -p ~/.config/eswm/config.lua                 PASS
cargo clippy --all-targets                        PASS
cargo test --bin eswm                             PASS  169 passed
cargo build                                       PASS
```

## Remaining

Live confirmation requires operator DRM UAT from a TTY (this session is nested Wayland; eswm was not running). From the VT: press `Super+Space` twice and confirm the app grid opens and closes. If it still does not, capture `~/.local/state/eswm/spawn.log` and `qs list --all` while eswm is live — the next suspect is `Quickshell.screens` being empty at panel construction, not the name filter.
