# UAT handoff — e11 Phase 0 session identity (live DRM)

Date prepared: 2026-11-28
Branch: `e11-proper-de-phase0` (HEAD `b347acf`)
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
| 1 | eswm starts on a TTY | No VT grab conflict; compositor reaches its normal ready state | PENDING |
| 2 | systemd user env | All four vars listed with `eswm-test` (or `$TACIT_COMP_SOCKET`) / `eswm` / `wayland` / `eswm` | PENDING |
| 3 | D-Bus activation env | `GetEnvironment` lists the same four vars, or tool absence produced only a warning | PENDING |
| 4 | Child identity | For a keybind-spawned app: `tr '\0' '\n' < /proc/$(pgrep -n APP)/environ \| grep -E 'WAYLAND_DISPLAY\|XDG_SESSION_TYPE'` shows eswm values | PENDING |
| 5 | File picker stays home | Open a GTK and a Qt app, trigger Ctrl+O; picker windows appear on eswm, not another compositor | PENDING |
| 6 | Portal backend | `eswm-portals.conf` is at the documented path; picker uses GTK backend | PENDING |
| 7 | Capture claim | A screen-share request gets no backend from eswm | PENDING |
| 8 | Clean shutdown | Quit with Super+Alt+Backspace; only eswm-owned vars disappear from the user env | PENDING |
| 9 | Opt-out startup | `ESWM_NO_SD_VARS=1` starts, logs the skip warning, leaves the user env untouched | PENDING |
| 10 | Missing tool degradation | If a tool is unavailable, eswm still runs and logs a warning | PENDING |

Row 10 can be marked N_A if both tools are installed. The "user edit wins"
rule is proven by unit test `session_env::tests::restore_only_owned_vars`, not
by this live run — do not try to fake it by editing the user manager mid-run.

## Evidence to attach

- `systemctl --user show-environment` output during Run A and after shutdown.
- `eswm.log` excerpts for the export warnings and the opt-out skip warning.
- Screenshot or window listing showing the file picker on eswm.
