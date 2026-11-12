# tacit-comp working notes

## HANDOFF — read this first (2026-11-05)

**Repo:** `~/Projects/eswm`, branch `feature/standalone-tacit`, HEAD `b8c00a5`
(pushed, remote == local). Working tree has **uncommitted** M5 pointer/mode/cleanup
work (see `git diff --stat`: main.rs ~+51, udev.rs ~+220, wm.rs ~+170, this file).
Do not commit or push without asking the user.

**Build / run:**
- `cargo build`; tests: `cargo test` (13/13 green: 11 unit incl. wm tests, 2 integration).
- Native DRM is the default backend: run `~/Projects/eswm/target/debug/eswm` from a
  TTY (grabs the VT; Ctrl+Alt+F1/F3 to leave/return).
- Nested test under Hyprland: `TACIT_COMP_BACKEND=winit WAYLAND_DISPLAY=eswm-dev
  ./target/debug/eswm` then `WAYLAND_DISPLAY=eswm-dev kitty`. Host may steal Super,
  so the nested path is not the keybind authority.
- Wayland socket: `TACIT_COMP_SOCKET` (default `eswm-test`).
- Logs: `/tmp/eswm.log` (info+; per-frame noise is debug), `/tmp/eswm-panic.log`,
  `/tmp/kitty-spawn.log` (kitty stderr + exit status in main log).
- `.cargo/config.toml` points at a locally built libdisplay-info 0.2 under
  `/tmp/opencode/ldi`; if /tmp is wiped, rebuild it (instructions in the M4 section
  below) or `cargo` will fail to link.

**Keybinds (udev backend):** Super+Return kitty · Super+Q close focused ·
Super+1..9 workspace switch · Super+Left/Up prev window · Super+Right/Down next
window · Super+Space toggle focused window Tiled/Floating · Super+Shift+Space toggle
global spawn mode (Hyprland tiled ↔ Windows floating cascade) · Super+LMB move /
Super+RMB resize floating windows · Ctrl+Alt+Backspace quit · Ctrl+Alt+F1..12 VT.

**Code map:**
- `src/main.rs` — `State`, backend dispatch (`TACIT_COMP_BACKEND=winit` else DRM),
  xdg/layer/seat/compositor handlers, `new_toplevel`, `apply_layout`, `prune_dead_windows`,
  `spawn_terminal`, winit run loop.
- `src/udev.rs` — DRM/KMS + libinput backend (port of anvil 0.7 `udev.rs`), session
  pause/resume, per-connector outputs, render_surface (target_rect placement,
  queue_frame, software cursor), input, keybinds, pointer focus/drag helpers.
- `src/wm.rs` — WindowManager: outputs/workspaces/windows, master-stack tiler,
  LayoutMode, floating cascade, window_at hit test, cycle_focus, unit tests.
- `src/ipc.rs` — hyprctl-compatible socket + `.socket2.sock` event stream (M2).
- `src/lua_state.rs` — Lua API stub (M3).
- `notes/tacit-integration.md` — Tacit/QML compat contract; `notes/roadmap.md` — plan.

**Feature state:** M1 nested compositor, M2 IPC, M3 Lua stub, M4 real DRM backend and
M5 tiling/keybinds/pointer all landed. Fixed presentation saga: missing
`queue_frame()`, empty-frame `is_empty` black panel (1x1 prime element), VT-pause
DeviceInactive storm (`paused` flag), stale dead toplevels holding layout slots
(`prune_dead_windows` + `alive()` check), xdg `Activated` only being sent on resize
(now tracked in `Window.activated_sent`), keyboard focus sync centralized in
`apply_layout`.

**OPEN BUG (top priority): click-to-focus still fails on the live DRM path.**
- Focus DOES move on window close and workspace switch (so `keyboard_focus_set` and
  the wm focus bookkeeping work). Typing works once a window has focus.
- Super+arrow focus cycle was just added; not yet retested.
- Diagnostics already in the build (info level): `click at (x,y) -> window <id> rect
  <rect>` and `keyboard focus -> <id>` (logged only on change); `pointer at (x,y)` is
  debug/1 Hz. Next step: ask user to click several windows, then read
  `grep -E "click at|keyboard focus" /tmp/eswm.log | tail -20`.
  - coordinates outside the window rects ⇒ pointer_location / hit-test space bug.
  - correct window id but focus unchanged ⇒ focus handoff/override bug.
  - no `click at` lines at all ⇒ button events never reach the compositor.
- Also unknown: whether the software cursor is visible on screen (user was asked).
  Cursor is `MemoryRenderBufferRenderElement` from xcursor theme `default` (exists on
  this machine); an invisible cursor would make every click land blind and look
  exactly like this bug.
- Pointer hardening already applied: location clamped to output size, pointer focus
  refreshed (motion+frame) before every button press, empty-background clicks keep
  keyboard focus.

**Known shortcuts / limits:** windows are stacked without decorations or pointer
hit-testing above layer surfaces; no direct scanout; 16 ms unconditional redraw
(damage tracker still gates page flips); no Super+drag for tiled windows; IPC
`CompCmd::GotoWorkspace`/`CloseWindow` are still no-ops in the winit path and are not
drained in the DRM path (M5 step 5 not done); multi-GPU "prefer Intel" default not
implemented (`TACIT_DRM_DEVICE` override works); Hyprland IPC dispatch surface is
mostly snapshot/plumbing only.

**Next steps if the click bug is confirmed fixed:** retest Super+arrows, drag/resize,
mode toggles; then ask user to commit + push the whole uncommitted M5 work; then
M5 step 5 (wire IPC/workspace commands into the wm) and the Intel-primary GPU pick.


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
- **Pointer phase landed (uncommitted 2026-11-04, tests 12/12, cargo check green):**
  - UdevData gains last_mods (ModifiersState, Copy/Clone, kept fresh inside the
    keyboard.input closure for every key), pointer_focus: Option<WindowId>,
    drag: Option<DragState> (Move | Resize, start pos, orig rect, button = BTN_LEFT/RIGHT).
  - pointer_focus() hit-tests via wm.window_at and returns surface + surface-local
    coords for smithay pointer.motion; no-focus enter/leave events now flow to clients.
  - Click-to-focus: PointerButton Pressed → window_at → focus_window +
    keyboard.set_focus (sends leave/enter). Super+LMB / Super+RMB drag to move /
    SUPER+RMB resize (min 200x150) writes corner geometry only for Floating windows.
  - Super+Space toggles focused window Tiled/Floating; Super+Shift+Space toggles
    global spawn mode (Hyprland tiled ↔ Windows floating cascaded spawn 62%x68%).
  - Workspace switch re-enters keyboard focus on the newly active workspace.
  - Bug A fix: wm::set_target_rect no longer pre-fills window.rect (rect stays
    zero until apply_layout confirms) so new windows now ALWAYS get a size
    configure; render placement uses target_rect.
  - NOT retest / NOT committed: TTY & nested winit smoke (WAYLAND_DISPLAY=eswm-dev
    + TACIT_COMP_BACKEND=winit cargo run) still required to confirm click-focus,
    spawn-cascade, and drag-resize behavior; commit pending user approval.
  - TTY evidence exposed stale ghost registrations (dead clients remained master slots).
    `apply_layout` now calls `prune_dead_windows`: WM entries with no matching
    live toplevel are removed and layout recomputed; `toplevel_for_window` treats
    `!alive()` resources as missing so dead clients cannot hold master/stack slots;
    per-frame layout logs are debug-only;
    `window_at()` ignores zero-sized targets. cargo check/test green. Live retest
    still required.

## Focus round (2026-11-05, uncommitted)

Retest feedback: "sometimes i can write in windows but i can't change focus under
mouse and sometimes i can't even type, also workspace change is working."

Fixes:
- xdg Activated was only sent together with a size configure, so focus changes on
  same-size windows never reached clients (toolkits keep the window "inactive" and
  may ignore keys). `wm::Window.activated_sent: Option<bool>` tracks the last sent
  state; `apply_layout` now sends a size-less configure on activation change.
- `apply_layout` keeps wl_keyboard focus in sync with wm focus (`set_focus` is
  idempotent): covers spawn, close/prune and workspace switch from one place.
  Removed the duplicated explicit focus calls.
- PointerButton press now refreshes pointer focus (motion + frame) before the
  button event, so a click lands on the surface currently under the cursor even
  when the pointer has not moved since layout/workspace changed.
- Pointer location is clamped to the output size (relative libinput deltas could
  walk off-screen and hit-test nothing).
- Debug log: `click at (x,y) -> window id` (RUST_LOG=eswm=debug).

cargo test 12/12 green. Still uncommitted; live TTY retest pending.

## Click-to-focus round 2 (2026-11-05)

Feedback: focus transfers on close now, but click-to-focus still fails; asking for
Super+arrow focus switching.

- New wm::WindowManager::cycle_focus(delta) (active workspace, wraps) + unit test.
  Keybinds: Super+Left/Up = previous, Super+Right/Down = next; apply_layout after.
- Empty background clicks no longer clear keyboard focus (only a window hit
  changes focus); avoids "can't type after clicking background".
- Diagnostic logs (info): `click at (x,y) -> window <id> rect <rect>` and
  `keyboard focus -> <id>` (only when it changes). `pointer at (x,y)` is debug/1 Hz.
- Suspect for the click failure: pointer_location drift / hit-test mismatch; the
  click log shows the exact coordinate + hit rect, so one retest log decides it.

cargo test 13/13 (11 unit incl. cycle test + 2 integration). Uncommitted.

## Double-fire round (2026-11-05)

Feedback: Super+arrow skipped a window (focus moved two steps).

- Cause: smithay 0.7 `KeyboardHandle::input` runs the filter closure for BOTH
  `KeyState::Pressed` and `KeyState::Released`, so `process_shortcut` fired twice
  (e.g. `cycle_focus(±1)` ran on press and again on release).
- Fix (one place): `src/udev.rs` `process_input_event` captures
  `pressed = event.state() == input::KeyState::Pressed` before `keyboard.input`
  and only calls `process_shortcut` on press; release returns
  `FilterResult::Forward` (key-up still reaches the focused client).
  `udev.last_mods` is still updated on press AND release (Super+button drags
  depend on it). All keyboard binds (Super+*, Ctrl+Alt+Backspace, Ctrl+Alt+F1-12
  VT) live in `process_shortcut`, so this one gate covers every shortcut.
- Pointer path was already press-only (`PointerButton` checks
  `ButtonState::Pressed`).
- cargo build + cargo test green (13/13). Live TTY retest still pending:
  Super+Left/Right must move exactly one window per press.

## Click-to-focus deferred (2026-11-05) — RESOLVED

- Update 2026-11-12: click-to-focus on the DRM/libinput path now WORKS
  (confirmed by user). The deferral below is historical record only.

- Status: unresolved; parked as future work by user decision. Do not spend more
  rounds on it before other milestones.
- Keyboard focus works (spawn, close, workspace switch, Super+arrows).
  Click-to-focus on the live DRM/libinput path is the open defect.
- Evidence gap: the last TTY run produced ZERO `click at (x,y) -> window ...`
  info lines, so we cannot tell "button events never reach
  process_input_event" from "the user did not click in that run". The next
  attempt must add one discriminating info log at the libinput event boundary
  (log every PointerButton arrival) before theorising further.
- Unproven suspects: hit-test mismatch (`target_rect` vs rendered geometry),
  `keyboard_focus_set` early-return when the cached id matches, and the click
  path not calling `apply_layout` so xdg `Activated` may not be re-sent.
- Software cursor drawing exists (`src/udev.rs` ~1380-1426, xcursor theme +
  `cursor_buffer`), so an invisible pointer is not the explanation unless the
  theme fails to load (warns "no xcursor theme found, cursor will be invisible").

## Hotplug + drag-clamp round (2026-11-05)

Audit of the session/DRM path (pause/resume already landed in b8c00a5):

- VT pause/resume is complete enough as written: libinput suspend/resume,
  `udev.paused` gates `State::render`, every backend paused and re-activated,
  idle render re-queued per backend after resume. Not yet exercised on hardware.
- Real gap found: `connector_disconnected` removed the DRM surface but left the
  WM output, workspace and windows behind, so windows on an unplugged output
  became unreachable (no hit-test, no focus, never rendered).
  Fix: `wm::WindowManager::remove_output(id)` moves that output's workspaces and
  windows onto the first surviving output, promotes a non-empty workspace if the
  survivor's active one is empty, refocuses and re-arranges; with no outputs left
  the workspaces/windows are dropped (clients stay mapped but unmanaged).
  `connector_disconnected` now resolves the WM output by connector name
  (`output_id_by_name`) and runs `apply_layout`. Unit test
  `remove_output_moves_windows_to_survivor`.
- Drag move/resize now clamps to the window's output geometry, and
  `window_at` compares bounds in f64, so an unbounded drag can no longer overflow
  the i32 hit-test.
- Known multi-output limitation, NOT fixed: `connector_connected` creates
  workspaces 1..=9 unconditionally, and workspace ids are global, so a second
  output gets no workspaces of its own. Fix when multi-head is actually needed.
- `cargo build` clean (23 pre-existing warnings), `cargo test` 14/14.

## Control channel v0 (2026-11-05, uncommitted at time of writing)

- `src/control.rs`: `$XDG_RUNTIME_DIR/eswm-<wayland-display>.sock`, newline
  JSON. Socket threads only parse/route; each request is applied on the
  compositor tick so it sees live WM state, then replied on a flume channel.
  Wired into both backends (`udev.rs` loop and `run_winit`).
- Commands: `state`, `window.list`, `workspace.list`, `output.list`,
  `window.focus|close|move|move_to|resize|set_mode`, `workspace.focus{id|delta}`.
- `wm`: added `outputs()` / `workspaces()` iterators for the snapshots.
- Not done: event stream (poll `state` for now), `output.focus`, `config.reload`,
  `window.swap`, multi-output `workspace.focus`.
- Proof: `tests/control.rs` spawns the nested compositor and drives the socket
  (state, unknown command, bad window). `cargo test` 15/15. Also hand-tested
  against a live nested session with `weston-simple-shm`: floating resize/move,
  tiled rejection, focus and close all behaved.

## WM core round (2026-11-05)

Making eswm usable as a WM before wiring Tacit to the control socket.

- wm: `toggle_fullscreen`, `toggle_maximized`, `swap_focused(delta)`,
  `swap_windows(a, b)`, `focus_workspace_relative(delta)` (wraps within the
  primary output's workspaces). Leaving fullscreen/maximized returns to Tiled.
- Fixed focus loss in `move_window_to_workspace`: the move cleared all focus on
  the output and never refocused, so keyboard input went nowhere after
  Super+Shift+<n>. It now refocuses the source workspace's last window.
- Keybinds: Super+F fullscreen, Super+M maximize, Super+Shift+1..9 move focused
  window to workspace N, Super+Ctrl+Left/Right swap with neighbour in the stack,
  Super+Alt+Left/Right previous/next workspace.
- control: `window.swap {a,b}` live; `workspace.focus {delta}` now wraps instead
  of computing active+delta (which could name a workspace that does not exist).
- Tests: 16 unit (4 new) + 3 integration, all green.

Still missing for "proper WM": visible focus (borders/gaps), window rules
(class -> workspace/float), tiled resize by mouse, multi-output workspaces,
click-to-focus (deferred).

## Focus border (2026-11-05)

- DRM render path draws a 2 px solid frame behind every placed window: accent
  blue when focused, dark gray otherwise. It reuses the cached 1x1
  MemoryRenderBuffer machinery the software cursor already uses and the
  `Cursor` render-element variant (no new variant needed).
- With this, which window owns the keyboard is visible; before it was not.
- shortcut: the winit development backend has no borders yet (dev only); the
  DRM path is the product.

## DRM "no content" — kitty Broken pipe analysis (2026-11-05)
Repro WITHOUT TTY achieved: `TACIT_COMP_BACKEND=winit TACIT_COMP_SOCKET=eswm-dbg ./target/debug/eswm`
(background via setsid; pkill -f self-kills the tool shell — use `pkill -f '[.]target/debug/eswm'`).
Results nested: kitty + weston-simple-shm connect, kitty commits buffers for 15 s (has_buffer probe = 0 misses),
even 5 concurrent kitties + the exact Super+Return spawn timing. No protocol errors (RUST_LOG=debug, grep InvalidSurface=empty).
Conclusion: eswm's xdg/compositor/seat protocol flow is correct; kitty's "Wayland: fatal display error: Broken pipe"
is a late symptom of kitty exiting on its own (exit status 0) after a GL/EGL failure it handles gracefully.
Only-on-DRM factor for kitty = its GL init (NVIDIA proprietary + Intel iGPU hybrid, TTY session env).
Next lead: on TTY run kitty manually (`WAYLAND_DISPLAY=eswm-test kitty --debug-rendering` or watch /tmp/kitty-spawn.log)
to see its GL error — kitty-side mitigation or eswm-side env tweak once the exact error is known.
Instrumentation probe was added and removed; tree builds clean, 19/19 tests green, nothing committed.
