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

- M1 nested Winit compositor: mostly complete and useful for development.
- M2 Hyprland-compatible IPC: implemented as a historical prototype, but it is
  not part of the product goal.
- M3 Lua/Tacit configuration API: only a stub and must be replaced by a real
  native Tacit integration API.
- M4 DRM/libseat backend: present and experimental.
- M5 window management: not implemented; windows currently stack at `(0, 0)`.
- Pointer hit-testing, correct pointer focus, and real workspace behavior are
  still missing in places, but click-to-focus on the DRM/libinput path now
  works (confirmed 2026-11-12; the earlier "deferred" note is obsolete).
- Animations are not implemented.

## Current phase: Phase 0 — reset the scope

This is the work to do now before adding more window-management features.

1. Remove the Hyprland compatibility requirement from the product design while
   keeping Tacit as the primary product requirement.
2. Audit Tacit's QML, Lua, and scripts and write down every compositor feature
   they need: outputs, layer surfaces, windows, focus, workspaces, keybinds,
   notifications, clipboard, cursor, and process launching.
3. Define an eswm-native control/state API for those requirements. Do not make
   `hyprctl` or Hyprland socket semantics part of that API.
4. Mark the current Hyprland IPC implementation as obsolete, then remove or
   isolate it from the standalone runtime.
5. Make the DRM/libseat backend the primary runtime path.
6. Keep Winit only as an optional development backend, if it remains useful for
   fast local tests.
7. Replace Hyprland-compatibility terminology in comments and notes while
   retaining Tacit-specific terminology.
8. Add a minimal standalone README describing how to start eswm from a TTY.
9. Decide the project license before publishing code derived from any external
   project.

The phase is complete when eswm starts on a real TTY, exposes a Wayland socket,
launches a basic Wayland client, and no product-critical code depends on
Hyprland. Tacit's required integration contract is documented and has a native
eswm design.

## Phase 1 — standalone compositor foundation

Make the DRM path reliable before implementing layouts.

- Initialize libseat, DRM, udev, GBM/GLES, libinput, and calloop cleanly.
- Handle session pause/resume and GPU/output hotplugging.
- Advertise the core Wayland protocols needed by normal clients.
- Implement correct XDG toplevel and layer-shell lifecycle handling.
- Track outputs, modes, scale, transform, and output positions.
- Implement frame callbacks, damage tracking, and presentation timing.
- Implement keyboard and pointer focus based on actual surface hit-testing.
- Add a manual smoke test with Kitty, a layer-shell client, and a second
  client opening and closing repeatedly.
- Start Tacit as the first real shell client and verify its layer surfaces,
  configuration loading, and basic compositor state access.

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
