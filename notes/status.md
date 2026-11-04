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

## Real DRM backend (M4 Option A, 2026-11-03)
- User chose: replace nested-only winit with a real DRM/libinput (udev) backend.
- Smithay 0.7 default features already compile backend_drm/udev/gbm/libinput/session_libseat/renderer_gl — `smithay = "0.7"` needs no feature edits.
- Reference impl: anvil's `src/udev.rs` (1668 lines, fetched to /tmp/opencode/smithay-src/anvil/src/udev.rs). Structure to port:
  - `LibSeatSession` (session + notifier), primary GPU via udev enumerate / `ANVIL_DRM_DEVICE`-style env, `GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>`, `UdevBackend::new(seat)`, libinput via `LibinputSessionInterface`.
  - Event sources: libinput handler, session notifier (Pause→pause cuda/libinput, Activate→render + libinput resume), device scan (connected → new output + `DrmOutput` per connector via `DrmScanner` from smithay-drm-extras).
  - Render: `render_elements_from_surface_tree` toplevels + layer surfaces, `drm_output.render_frame(elements, clear_color, FrameFlags)`, frame scheduling on `DrmEvent::VBlank` + calloop timer throttle, presentation feedback, `SwapBuffersError` matching DeviceInactive/PermissionDenied/TestFailed → reset state.
  - Extras: `smithay-drm-extras` crate, xcursor for pointer image (MemoryRenderBuffer), dmabuf/egl init (`bind_wl_display`, `DmabufState`, feedback global).
- eswm State for udev: keep compositor/xdg/layer/shm/seat/output/data-device states + ipc + lua; add Backend enum {Winit|Udev} or cfg-switch; session-based input replaces winit InputEvent path.
- eswm main.rs is 408 lines (M1 skeleton + in-loop IPC/Lua already merged); ipc.rs 647, lua_state.rs 113.

## M4 build state (2026-11-03)
- `cargo check` green, `cargo test` 2/2 green (globals + IPC tests explicitly use Winit; standalone DRM is now the default runtime).
- smithay features extended with `backend_egl` + `use_system_lib` (ImportEgl/bind_wl_display needs both; default set lacks backend_egl).
- System-side trick: libdisplay-info 0.2.0 built into /tmp/opencode/ldi (system has 0.3.0, libdisplay-info-sys 0.2 requires <0.3). Handled project-wide by `.cargo/config.toml` (PKG_CONFIG_PATH + rpath) — re-run the meson build if /tmp is wiped.
- udev.rs structure: UdevData{session, primary_gpu, gpus, backends, dmabuf_state, cursor_images/buffer, pointer_location, start, quit}; render()/render_surface() take u32 ms; frame finish throttling + 0.6 repaint-delay timer; cursor via MemoryRenderBufferRenderElement::from_buffer (no PointerElement in smithay 0.7).
- Known ceiling (shortcut): windows still render full-window stacked at (0,0) (no hit-testing, no space), pointer focus = None (clients get no enter events), keyboard focus = first toplevel on press. Layout/windows = M5.

## Standalone Tacit direction (2026-11-03)
- Tacit remains the first consumer, but Hyprland compatibility is no longer a product requirement.
- The DRM backend must not initialize the historical Hyprland IPC or Tacit Lua compatibility path.
- Next implementation work is the native Tacit state/action contract, then real window/workspace state and layout.

## M5 wiring landed (2026-11-04, commit 7a745a6)
- Windows were drawing never because smithay's `DrmCompositor::queue_frame()` was never called (render_frame only prepares; the page flip is what scans out). Also zero-element frames report `is_empty` so the panel stayed black forever. Both fixed: queue_frame after render, 1x1 primed element when empty, render every 16 ms loop iteration.
- kitty keybind worked but kitties died silently: stderr was /dev/null. Now detached via setsid, bare WAYLAND_DISPLAY, stderr to /tmp/kitty-spawn.log; exit status logged. Verified on TTY: windows register, tile, draw.
- Diagnostic scaffolding (keep until M6 cleanup): unconditional /tmp/eswm.log, panic hook -> /tmp/eswm-panic.log, probe logs for toplevel register / configure / queued frame.
- RenderD128 (NVIDIA RTX 2050, GL 615.78) is primary_gpu; EGL_WL_bind_wayland_display is NOT supported there -> hardware-accel path skipped (EsEnNotSupported). Intel eDP-1 drives the panel. Possible need: TACIT_DRM_DEVICE=/dev/dri/renderD129 to force Intel render node.
- wm wiring complete for steps 1+2+4; pointer hit-test (step 3) and IPC/Lua workspace commands (step 5) still open.
