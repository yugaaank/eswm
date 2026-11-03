# Tacit integration contract

Tacit is the first real consumer of eswm. The compositor is standalone and
must not depend on Hyprland, `hyprctl`, Hyprland sockets, or Hyprland Lua
helpers. Tacit currently depends on those interfaces, so this document records
the behavior eswm must provide natively and the work needed to migrate Tacit.

## Current Tacit dependencies

The current shell uses:

- Quickshell's `Hyprland` and `HyprlandToplevel` objects.
- `hyprctl` queries for clients, monitors, layers, workspaces, active workspace,
  devices, cursor position, and keybinds.
- `Hyprland.dispatch` for focus, close, workspace, move, resize, float,
  fullscreen, swap, submap, and shell actions.
- Hyprland raw events for window, focus, workspace, monitor, keyboard-layout,
  and configuration changes.
- Hyprland configuration files and reloads for layout, gaps, borders, opacity,
  animation, monitor, keyboard, pointer, and touchpad settings.
- Layer-shell surfaces with background, bottom, top, and overlay layers.

These are migration inputs, not APIs that eswm should reproduce.

## Native state API requirements

Tacit's replacement backend needs observable state for:

### Windows

- Stable native window ID.
- Application ID/class and title.
- Workspace ID and output/monitor ID.
- Logical position and size.
- Tiled, floating, maximized, and fullscreen state.
- Mapped, visible, urgent, and focused state.
- PID where available.
- Close and focus eligibility.

### Workspaces

- Stable numeric ID and display name.
- Output/monitor assignment.
- Active state per output.
- Window membership.
- Creation, removal, rename, and current-window state.

The model is a workspace model: each managed window belongs to one workspace.
It is not Pinnacle's multi-tag model.

### Outputs

- Stable name and ID.
- Position, logical size, scale, transform, and refresh rate.
- Available/current modes.
- Active workspace.
- Reserved space from layer-shell exclusive zones.
- Focused output state.

### Layers and input

- Layer surfaces grouped by output and layer level.
- Namespace, geometry, visibility, and exclusive zone.
- Keyboard-interactivity mode.
- Pointer position.
- Keyboard layout names/codes and active layout.
- Lock-screen and focus state needed by Tacit's shell surfaces.

## Native actions

The first action API should support:

- Focus or close a window.
- Switch to a workspace.
- Move a window to a workspace, with optional follow behavior.
- Move or resize a floating window.
- Toggle floating, maximized, and fullscreen state.
- Swap or reorder tiled windows.
- Focus the next/previous or directional window.
- Switch workspaces relatively.
- Register and remove compositor keybinds.
- Enter and leave a temporary keybind submap.
- Reload Tacit/eswm configuration.
- Change keyboard layout and input settings.
- Change output mode or position when output management is ready.

Actions should use structured messages rather than embedded Lua source strings.

## Events

Tacit needs pushed events for:

- Window opened, closed, title/app ID changed, mapped, unmapped, and focused.
- Workspace created, removed, renamed, switched, or changed membership.
- Output added, removed, changed, or focused.
- Layer surface added, removed, or changed.
- Keyboard layout changed.
- Configuration reloaded.
- Animation state changed when the shell needs to synchronize visual state.

Events should be coalesced and sent through one native per-user control/event
channel rather than starting a process for every update.

## Layer-shell contract

Tacit uses many layer-shell surfaces, including:

- Background wallpaper and desktop widgets.
- Bottom surfaces.
- Top panels, bars, frames, and the notch.
- Overlay notifications, menus, lock screens, selectors, and dialogs.

eswm must implement output selection, anchors, margins, exclusive zones,
keyboard interactivity, popup stacking, and correct rendering order. A layer
surface must not be treated as a full-window surface by default.

## Startup and session requirements

- eswm starts directly from a Wayland session on DRM/libseat.
- Tacit starts as a normal Wayland shell client after the compositor socket is
  available.
- Environment setup must advertise the eswm session and socket to Quickshell
  and child applications.
- The migration must remain Wayland-only, following Tacit's project rules.
- Existing external services such as audio, networking, notifications, and
  application launching remain separate from the compositor unless they need a
  compositor action.

## Migration order

1. Implement a small native state/action channel and a test client.
2. Build eswm's window, workspace, output, and layer state from the compositor
   itself instead of synthetic IPC snapshots.
3. Implement focus, close, workspace switching, floating, and fullscreen.
4. Add Tacit's native Quickshell backend and migrate the shared `WM` service.
5. Migrate overview, window menu, workspace model, notifications, input layout,
   and other direct Hyprland users one group at a time.
6. Move Tacit's startup and configuration actions to native eswm actions.
7. Remove the old Hyprland backend and session dependencies after the native
   path covers the required shell behavior.
