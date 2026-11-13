# eswm roadmap

## Product goal

`eswm` is an independent, standalone Wayland compositor and window manager
being built specifically to run Tacit. Tacit will move from Hyprland to eswm;
eswm must provide the compositor and window-manager behavior Tacit needs
without depending on Hyprland or another compositor.

It should run directly on DRM/libseat and provide:

- Tiling and floating windows.
- A normal workspace system rather than tags.
- Smooth, configurable window and workspace animations.
- Low memory use and a small runtime dependency footprint.
- A clean implementation that can be published under a chosen license.

Tacit is the first-class integration target, not a generic Hyprland
compatibility target. Any control or state API should be native to eswm and
designed around Tacit's actual needs.

Pinnacle and other Smithay compositors may be used as architectural references.
Pinnacle source code, tests, assets, and distinctive implementation details are
not copied into this project.

## Current repository state

- M1 nested Winit compositor: complete, useful for development only.
- M2 Hyprland-compatible IPC: complete (historical prototype), not part of the product goal.
- M3 Lua/Tacit configuration API: complete — eswm configured exclusively via `~/.config/eswm/config.lua` using the `es` Lua API (`es.bind`, `es.bind_cmd`, `es.dsp.exec_cmd`). No JSON config.
- M4 DRM/libseat backend: complete — default runtime path, starts from TTY, supports outputs, layer surfaces, pause/resume, software cursor.
- M5 window management: complete — master-stack tiling, floating, workspaces, focus cycle, keyboard focus, click-to-focus verified on DRM.
- Animations: not implemented.
- Configuration: complete — Lua-only autostart and keybinds.

## Current phase

Phase 0 (scope reset) is complete. The DRM path starts on TTY, exposes a Wayland socket,
launches clients, and is configured entirely through Lua.

Next workstreams:
- Phase 1 (DRM reliability): smoke tests with multiple clients, session pause/resume, multi-monitor.
- Phase 2 (window/workspace model): visible focus borders, window rules, gaps.
- Phase 3 (tiling/floating): already implemented via master-stack tiler, needs polish.
- Phase 4 (animations): not started.
- Phase 5 (Tacit integration): native control channel, foreign toplevel, screencopy, session lock not yet implemented.

## Phase 0 — reset the scope — COMPLETE

- Hyprland compatibility removed from product design; Tacit is the primary target.
- Tacit's QML/Lua/script needs audited and documented in `notes/tacit-integration.md`.
- Native eswm control/state API defined in `notes/tacit-integration.md`.
- Hyprland IPC retained as historical prototype only.
- DRM/libseat backend is the primary runtime path.
- Winit kept as optional development backend only.
- Hyprland-compatibility terminology replaced with Tacit-specific terminology.
- Minimal standalone README exists describing TTY startup.
- MIT license added (`LICENSE`).

## Phase 1 — standalone compositor foundation — COMPLETE

- Initialize libseat, DRM, udev, GBM/GLES, libinput, calloop cleanly. — DONE
- Handle session pause/resume and GPU/output hotplugging. — DONE
- Advertise core Wayland protocols needed by normal clients. — DONE
- Correct XDG toplevel and layer-shell lifecycle handling. — DONE
- Track outputs, modes, scale, transform, output positions. — DONE
- Frame callbacks, damage tracking, presentation timing. — DONE
- Keyboard and pointer focus via surface hit-testing (click-to-focus verified on DRM 2026-11-12). — DONE
- Smoke test with Kitty, layer-shell client, repeated open/close. — DONE

## Phase 2 — window and workspace model

Introduce the core window-manager state.

### Window state

Each managed window should track:

- Surface and role.
- Output and workspace.
- Tiled or floating state.
- Current and target rectangle.
- Focus and mapped state.
- Min/max/fullscreen state.
- Application identity and title.

### Workspace state

Use one-workspace-per-window membership:

- Workspace identity and name.
- Windows assigned to it.
- Active workspace per output.
- Focused window and focus history.
- Optional empty/persistent workspace behavior.

Implement workspace switching and moving windows between workspaces before
adding animations.

## Phase 3 — tiling and floating management

Start with a small, predictable layout system.

- Master-stack tiling layout.
- Horizontal and vertical split behavior.
- Focus next/previous and directional focus.
- Resize the master area and split ratios.
- Toggle floating mode.
- Move and resize floating windows with pointer input.
- Maximize and fullscreen behavior.
- Correct configure/ack_configure handling for every resize.
- Multi-output placement and workspace/output transitions.

Keep the first layout engine custom and lightweight. Add more layouts only
after the basic state model is stable.

## Phase 4 — animation engine

Add animations after final window targets and focus behavior work correctly.

- A frame-driven animation scheduler using monotonic time.
- Interpolated window position and size.
- Open, close, move, resize, and focus transitions.
- Workspace switch transitions such as slide or fade.
- Configurable duration, easing, and enable/disable settings.
- Animation cancellation when a new target arrives.
- Correct damage regions covering both old and new rectangles.
- No animation-induced input bugs: hit-testing must use the intended logical
  window state, not only the visual interpolation state.

Animations should be optional and should not add a large runtime framework.

## Phase 5 — Tacit integration and usability completeness

Implement the native interface that allows Tacit to replace its Hyprland
dependency.

- Native window, workspace, output, and focus state queries for Tacit.
- Native actions for focus, close, move, resize, and workspace switching.
- Keyboard shortcut and configuration system for Tacit.
- Tacit startup/session integration.
- Layer-shell exclusive zones and correct stacking order for Tacit's shell.
- Tacit notifications, cursor behavior, and other shell-facing services.
- A small native control socket or client API, only if external control is
  needed.
- Clipboard and primary-selection behavior.
- Data-device drag and drop.
- XDG activation and idle inhibition where useful.
- Output management and power management.
- Optional Xwayland support behind a build/runtime option.
- Notifications or a separate optional notification component if desired.

## Phase 6 — performance and reliability

- Measure startup memory, steady-state memory, frame time, and CPU use.
- Profile with several tiled, floating, and animated windows.
- Reduce allocations in the frame loop and window traversal.
- Use damage-aware rendering and avoid repainting unchanged outputs.
- Validate multi-monitor and monitor unplug/replug behavior.
- Test session pause/resume and GPU/output failures.
- Add protocol tests and client smoke tests that can run in CI where possible.
- Run sanitizers or focused debugging tools for lifecycle and input bugs.

## Phase 7 — publishing

- Choose and add the project license.
- Add a README with requirements, TTY startup, configuration, and known
  limitations.
- Add a sample configuration.
- Add packaging/session files for a display manager if appropriate.
- Document keyboard shortcuts and workspace behavior.
- Add contribution and development instructions.
- Add release checks, versioning, and reproducible build instructions.

## Immediate implementation order

1. Scope cleanup and license decision.
2. Audit Tacit's current Hyprland-facing QML/Lua behavior and define the
   native eswm integration contract.
3. Make DRM the primary executable path.
4. Verify one real output and one Kitty client from a TTY.
5. Replace the current first-window focus shortcut with real surface/focus
   state.
6. Add the `Window`, `Workspace`, and `Output` state model.
7. Implement one workspace and master-stack tiling.
8. Add floating windows and pointer move/resize.
9. Add multiple workspaces and workspace switching.
10. Add the native Tacit integration before animation polish.
11. Add animations only after geometry and focus are stable.
12. Profile and simplify before publishing.
