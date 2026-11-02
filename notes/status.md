# tacit-comp working notes

## What this is
Simple compositor on Smithay 0.7 for moving Tacit off Hyprland. Plan:
`~/.opencode/plan/tacit-compositor-plan.md`. Compat bar: tacit's existing QML (Quickshell), Lua session files and scripts must work unchanged.

## Setup facts (2026-11-02)
- Smithay 0.7.0, examples ship in the crate: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/smithay-0.7.0/examples/`
  (minimal.rs is our base; crate ships examples — no need to fetch the repo).
- wayland-client 0.31: `Connection::from_socket(UnixStream)` connects to a name; `Dispatch::event` first param is `&mut State` (not `&mut self`); env is edited unsafely in edition 2024 — avoid: use UnixStream.
- winit 0.30 has NO `pump_events` feature (module `::winit::platform::pump_events` still exists via API).
- socket name via `TACIT_COMP_SOCKET` env, default `wayland-tacit-comp`.
- Tests are nested (winit) → need a graphical session, run from inside Hyprland.

## M1 status: done
- `src/main.rs`: winit session, xdg-shell + layer-shell + shm + seat, renders toplevels and layer surfaces (layer surfaces get default `send_configure()`, full-window).
- `tests/globals.rs`: spawns comp, asserts globals wl_compositor/wl_seat/wl_shm/xdg_wm_base/zwlr_layer_shell_v1 + client roundtrip. `cargo test` green.
- Manual M1 check (user): run `flutter run`? No — run `cargo run` inside Hyprland, then `WAYLAND_DISPLAY=wayland-tacit-comp foot` and a `qs -p $TACIT_SHELL shell.qml` later; notch should draw.
- shortcuts in main.rs: layer surfaces default-configured full-window (no anchors/layout); no DD force sync; Focus event + pointer not handled; teardown is kill.

## IPC findings (M2 recon, 2026-11-02)
- **Tacit never opens the raw socket: it shells out to the `hyprctl` binary.** Compat contract = hyprctl must work against us. hyprctl locates socket via `$HYPRLAND_INSTANCE_SIGNATURE` → `$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock` (strings from /usr/bin/hyprctl: `{}/{}/.socket.sock`). So we create `$XDG_RUNTIME_DIR/hypr/$TACIT_COMP_SIG/.socket.sock` and tacit tests set `HYPRLAND_INSTANCE_SIGNATURE=<sig>`.
- **Wire protocol (verified with a python spy server + hyprctl 0.57):** request is one blob `[flags]/<request>`. Real `hyprctl` does **not** shut down its write side: read its first chunk and reply immediately. Flags: `j` = `-j`, `r` = raw. Server replies plain bytes (JSON for `j`, text for `/`) then closes. Example: `version` → `/version`, `hyprctl -j monitors` → `j/monitors`.
- **Event stream = socket2:** tacit QML uses Quickshell's builtin `Hyprland` type (`Connections { target: Hyprland; onRawEvent }` — services/HyprlandXkb.qml, HyprlandConfig.qml, HyprlandKeybinds.qml, HyprlandData.qml monitorEvents `["monitoradded","monitorremoved","focusedmon","configreloaded"]`). Quickshell's HyprlandType reads `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock`; events are `name>>data\n` lines (dispatch event as `event>>data` or `dispatch>>argv?`). So comp also listens on `.socket2.sock`, writes event lines per connected client, keep-open.
- **Lua-through-IPC commands tacit sends (map to M3/M5):** `hyprctl dispatch 'hl.dsp.focus({ window = "address:0x…" })'`, `hl.dsp.window.close(...)`, `hl.dsp.global("quickshell:…")`, `hyprctl eval "hl.config({ input = { kb_layout = … } })"`, `hyprctl keyword`, `hyprctl reload`, `hyprctl notify`, `hyprctl setcursor`, plus plugin-ish handlers: `hyprctl tacitgrip <verb> <addr>…` (WindowBand.qml), `hyprctl hyprsunset <temperature|identity>`, `hyprctl -j cursorpos`, `hyprctl activeworkspace -j`, `hyprctl binds -j` (interpreted by Shortcuts.qml). Anything we don't implement should reply "invalid command"-style text (hyprctl exits 0 with empty errors — Cliphist.qml relies on `hyprctl activewindow -j | jq -r '.address // empty'` failing soft).
- Fixtures captured: `tests/fixtures/hyprctl-*.json` from live Hyprland 0.57 (monitors, clients, layers, devices, activewindow, version, binds(170K), workspaces, decoration/cursors/splashes 16B, animations).
- Note: `drun Importer uses `hyprctl sendshortcut`? not yet checked.

## M2 status: done
- `src/ipc.rs`: command + socket2 IPC listeners; monitor/workspace/client/layer/device/active-window/version/cursor snapshots; reload emits `configreloaded`; unknown commands soft-fail as `invalid request`.
- `src/main.rs`: now advertises a `wl_output` global (`winit`). Without it applications such as foot find zero monitors and refuse to start.
- `tests/ipc.rs`: real `hyprctl` E2E test. It symlinks the host Wayland socket into the isolated runtime directory because winit needs that socket while hyprctl needs the isolated IPC directory.
- Proof: `cargo test` passes; manual nested `foot` and `kitty` connect and render; real `hyprctl -j monitors`, `-j cursorpos`, `reload`, and unknown-command probe work.

## Next up: M3 Lua configuration API
