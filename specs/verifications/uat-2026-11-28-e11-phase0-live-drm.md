# UAT handoff — e11 Phase 0 session identity (live DRM)

Date prepared: 2026-11-28
Branch: `e11-proper-de-phase0` (HEAD `1ce9ff1`)
Operator: **you**. Unit tests prove the command shapes, ownership tracking and
child env; only a real session proves a GTK/Qt file picker picks eswm.

## Safety first

- Run from a real TTY (VT), **not** inside an existing Wayland/X11 session.
  eswm grabs the VT; two compositors in one session is a bad time for everyone.
- Keep a second TTY (Ctrl+Alt+F3) open for recovery.
- Nothing is pushed or merged. The branch stays local until you say otherwise.

## Setup (once)

```sh
cd /home/yugaaank/Projects/eswm
cargo build
install -Dm644 resources/eswm-portals.conf \
  /usr/share/xdg-desktop-portal/eswm-portals.conf
command -v systemctl dbus-update-activation-environment xdg-desktop-portal-gtk
```

If a portal backend package is missing, install it first; the config only
selects a backend, it does not provide one.

## Run A — normal startup

On the TTY, with `ESWM_NO_SD_VARS` unset:

```sh
cd /home/yugaaank/Projects/eswm
./target/debug/eswm
```

In a second TTY, logged in as the same user:

```sh
systemctl --user show-environment | grep -E '^(WAYLAND_DISPLAY|XDG_CURRENT_DESKTOP|XDG_SESSION_TYPE|XDG_SESSION_DESKTOP)='
tail -f "${XDG_STATE_HOME:-$HOME/.local/state}/eswm/eswm.log"
```

Optional D-Bus activation env check (needs a recent dbus-daemon):

```sh
dbus-send --session --dest=org.freedesktop.DBus --type=method_call --print-reply \
  /org/freedesktop/DBus org.freedesktop.DBus.GetEnvironment
```

## Run B — opt-out

Quit Run A, then start with the escape hatch:

```sh
ESWM_NO_SD_VARS=1 ./target/debug/eswm
```

Expect a `skipping systemd/D-Bus session export` warning in the log and no
change to `systemctl --user show-environment`.

## Checklist — record PASS / FAIL / N_A for each

| # | Check | Expected | Result |
|---|-------|----------|--------|
| 1 | eswm starts on a TTY | No VT grab conflict; compositor reaches its normal ready state | PASS (agent: pid 413364, layers and toplevels in log) |
| 2 | systemd user env | All four vars listed with `eswm-test` / `eswm` / `wayland` / `eswm` | PASS (agent: `show-environment`) |
| 3 | D-Bus activation env | Same four vars in the activation environment | PARTIAL (agent: `dbus-update-activation-environment` returned success with no warning; `org.freedesktop.DBus.GetEnvironment` is unsupported by this dbus-daemon, so direct read was impossible) |
| 4 | Child identity | Keybind-spawned app inherits eswm values | PASS (agent: kitty pid 414464 has all four vars plus `TERMINAL=kitty`; pre-existing kitty 62834 keeps its Hyprland values as expected) |
| 5 | File picker stays home | GTK/Qt pickers appear on eswm | PASS (operator: clean `eswmdev` session, Super+V check; picker on eswm) |
| 6 | Portal backend | Picker uses the GTK backend from `eswm-portals.conf` | PASS in clean `eswmdev` session (operator: portal daemons show `eswm` / `eswm-test`) |
| 7 | Capture claim | A screen-share request gets no backend from eswm | PASS (operator: `Super+s` call failed with "No such interface ... ScreenCast", so no backend is exported) |
| 8 | Clean shutdown | Quit with Super+Alt+Backspace; only eswm-owned vars disappear | PASS (operator on the pre-restore build: `Super+Alt+Backspace`, then `eswm-env` showed no eswm identity vars). External systemd/D-Bus restore was added after review and is unit-covered only. |
| 9 | Opt-out startup | `ESWM_NO_SD_VARS=1` starts, logs the skip warning, leaves env untouched | N_A — operator explicitly skipped; no PASS claimed |
| 10 | Missing tool degradation | If a tool is unavailable, eswm still runs and logs a warning | N_A (both tools installed; no export warnings in log) |

## Finding 1 — stale portal daemons bypass eswm-portals.conf

`xdg-desktop-portal` (pid 1383), `xdg-desktop-portal-gtk` (pid 1431) and
`xdg-desktop-portal-hyprland` (pid 1555) all started at 18:13, over two hours
before eswm (20:46). Their environments still say
`XDG_CURRENT_DESKTOP=Tacit:Hyprland` and `WAYLAND_DISPLAY=wayland-1`.
There is no `tacit-portals.conf`, so the portal falls back to
`hyprland-portals.conf` (`default=hyprland;gtk`) and the Hyprland backend is
already connected to `wayland-1`.

Exporting the session identity updates the user bus, but it cannot retroactively
change a daemon that is already running. `eswm-portals.conf` is installed
correctly; it only takes effect once the portal daemons are restarted with the
new environment. This reproduces the original "file picker opens in Hyprland"
symptom in a transition session where both compositors are alive.

Suggested experiment (operator, disruptive — restarts portal services):

```sh
systemctl --user restart xdg-desktop-portal.service xdg-desktop-portal-gtk.service xdg-desktop-portal-hyprland.service
tr '\0' '\n' < /proc/$(pgrep -n xdg-desktop-portal)/environ | grep -E '^(WAYLAND_DISPLAY|XDG_CURRENT_DESKTOP)='
```

After that, a GTK/Qt file picker should route through GTK on `eswm-test`.
The clean dev-user session below confirms the config works without restarting
anything. Phase 0 therefore documents the fresh-login requirement instead of
adding a startup portal restart (see `docs/session.md`).

Row 10 is N_A because both tools are installed. The "user edit wins"
rule is proven by unit test `session_env::tests::restore_only_owned_vars`, not
by this live run — do not try to fake it by editing the user manager mid-run.

## Clean dev-user session — rows 5-8 closed

The operator created a throwaway `eswmdev` user and copied the eswm and
Noctalia configs into its home. That gives a fresh `systemd --user` manager and
D-Bus session without ending the Hyprland session. `/usr/local/bin/eswm-dev` is
a shared copy of the built binary because `/home/yugaaank` is mode 700.

A one-key checker at `/usr/local/bin/eswm-check` is bound to `Super+V`. It
shows the session and portal environments in a Zenity dialog, then opens a GTK
file chooser with `GTK_USE_PORTAL=1`. The operator confirmed:

- dialog shows `XDG_CURRENT_DESKTOP=eswm` and `WAYLAND_DISPLAY=eswm-test`,
  with no `Tacit:Hyprland` or `wayland-1`;
- portal daemons carry the same eswm values;
- the file picker opens on eswm;
- `Super+s` gets "No such interface ... ScreenCast" at the portal object,
  so no screen-capture backend is claimed;
- `Super+Alt+Backspace` quits cleanly and `eswm-env` shows no identity vars.

Rows 5-8 are closed in a clean login session. Row 9 was explicitly skipped by
the operator; the opt-out path has no live PASS and no unit coverage. Finding 1 still describes
the transition-session caveat: stale Hyprland portal daemons survive a TTY
switch because `eswm-portals.conf` is read only at portal startup. No portal
services were restarted by the agent.

## Post-review addition (2026-11-28)

A release review found that the pre-restore build cleaned only eswm's own
process environment; the systemd user and D-Bus activation values survived a
quit. Steps 6-8 in `e11s01-session-identity.md` add caller-env preservation,
external snapshot/restore and guard-path coverage. Those paths are green under
`cargo test` (350 passing) but have **no fresh live PASS** — this agent session
runs inside a graphical session and must not grab the VT (ADR-0002). Row 8 above
records the original operator evidence, not the external restore.

## Evidence to attach

- `systemctl --user show-environment` output during Run A and after shutdown.
- `eswm.log` excerpts for the export warnings and the opt-out skip warning.
- Screenshot or window listing showing the file picker on eswm.
