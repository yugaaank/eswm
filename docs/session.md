# Session identity and portals

eswm runs only on DRM/libseat. Start it from a TTY: switch with
`Ctrl+Alt+F3`, log in, then run `eswm`. Do **not** start it from inside an
existing Wayland or X11 session — the VT grab would take over that session.

## What eswm announces

At startup eswm sets four variables in its own process and pushes them to the
systemd user environment and the D-Bus activation environment:

| Variable | Value | Why |
| --- | --- | --- |
| `WAYLAND_DISPLAY` | the eswm socket name (default `eswm-test`) | tells clients which compositor owns the display |
| `XDG_CURRENT_DESKTOP` | `eswm` | selects the portal config and desktop quirks |
| `XDG_SESSION_TYPE` | `wayland` | marks the session as Wayland |
| `XDG_SESSION_DESKTOP` | `eswm` | names the session for session managers |

Children started from keybinds or the Lua config inherit all four values.

Check the export from another TTY or a shell in the session:

```sh
systemctl --user show-environment | grep -E '^(WAYLAND_DISPLAY|XDG_CURRENT_DESKTOP|XDG_SESSION_TYPE|XDG_SESSION_DESKTOP)='
```

If `systemctl` or `dbus-update-activation-environment` is missing, or the user
bus is unavailable, eswm logs a warning and keeps running. Session export is
best-effort, not a startup gate.

## Opting out

Set `ESWM_NO_SD_VARS` before launching to skip the systemd and D-Bus exports:

```sh
ESWM_NO_SD_VARS=1 eswm
```

This is useful when another compositor owns the user session and you do not
want eswm to change its variables. eswm still sets its own in-process identity
so spawned children connect to the right socket; only the shared user
environment is left alone.

On a clean shutdown eswm removes only the values it still owns. If you change
one of them while eswm runs, your value wins and eswm leaves it in place. This
is why a file picker should stop RSVPing to the other compositor's party: the
session now says who is actually hosting.

## Portal backends

The GTK backend is the conservative default because it covers file choosers,
URI opening and settings without claiming interfaces eswm has not implemented.

Install the shipped config:

```sh
install -Dm644 resources/eswm-portals.conf \
  /usr/share/xdg-desktop-portal/eswm-portals.conf
```

You need `xdg-desktop-portal` and `xdg-desktop-portal-gtk` installed. Qt
applications normally use the Qt portal frontend, which talks to the same
backend; availability varies by distribution.

Capture interfaces are intentionally absent from the config. A screen-share
request gets no backend from eswm until a capture protocol exists in a later
phase. Packaging of the config is handled separately from the compositor.

## Verification from a live TTY

1. Start eswm with session export enabled.
2. Confirm the four variables with `systemctl --user show-environment`.
3. Open a GTK or Qt application and trigger its file picker; it must stay in
   the eswm session.
4. Quit eswm cleanly and confirm only eswm-owned values were removed.
5. Restart with `ESWM_NO_SD_VARS=1` and confirm eswm starts, logs a skip warning,
   and leaves the user environment untouched.
