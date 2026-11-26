# UAT handoff — e09s05 toplevel ownership, requests and rules (live DRM)

Date prepared: 2026-11-26
Branch: `e09-lua-toplevel-rules`
Operator: **you**. The fixture tests drive a real xdg-shell client over a Unix
socket; only a live session proves that spawning, focusing and closing feel
right when registration moved to first commit.

## Safety first

- Run from a real TTY (VT), **not** inside an existing Wayland/X11 session.
  eswm grabs the VT; asking two compositors to share a seat is like asking two
  raccoons to share one bin.
- Keep a second TTY (Ctrl+Alt+F3) open for recovery.
- Logs: `/tmp/eswm.log`, `/tmp/eswm-panic.log`.

## Setup

1. Build and start from a VT:

   ```sh
   cargo build
   RUST_LOG=eswm=info ./target/debug/eswm 2>&1 | tee /tmp/opencode/e09s05-eswm.log
   ```

2. In a second TTY, watch the event stream and state:

   ```sh
   ./start.sh watch | tee /tmp/opencode/e09s05-events.jsonl
   ./start.sh ctl '{"id":1,"cmd":"state"}' | tee /tmp/opencode/e09s05-state.jsonl
   ```

3. Seed a rule for row 8 in `~/.config/eswm/config.lua` and reload:

   ```lua
   es.window_rule { app_id = "kitty", workspace = 2, mode = "floating" }
   ```

   **Skip this rule while testing rows 1-7 and 9-10** — it would move every
   kitty and muddy the picture. Add it only for row 8, then remove it again.

## Checklist — record PASS / FAIL / N/A for each row

| # | Check | Expected | Result |
|---|-------|----------|--------|
| 1 | Spawn one client | `Super+Return` → exactly one window appears; `state` shows one entry with real `app_id`/`title`, not empty strings | |
| 2 | Events carry metadata | `window_open` and `window_class` for that window arrive in the same tick with matching `id`; `window_class.data.class` equals the real `app_id` (e.g. `kitty`) | |
| 3 | Two clients independent | Open a second kitty: two distinct ids. Focus, move and close the first; the second keeps its geometry, workspace and focus | |
| 4 | Client fullscreen | In a client that honours F11 (mpv, a browser, kitty), press F11: eswm applies `Presentation::Fullscreen`, the window fills the output without gaps/titlebar, and one `window_mode` frame reports fullscreen | |
| 5 | Client unfullscreen | Press F11 again: the window returns to its tiled placement with gaps/borders restored; `window_mode` reports normal/tiled | |
| 6 | Client maximize | Launch a client that starts maximized (`kitty --start-as=maximized` or a browser's `--start-maximized`): it requests maximize, lands maximized, and its original geometry is saved. Restoring/unmaximizing returns to that saved rect | |
| 7 | Close leaves no stale window | Close both clients: `state` lists no windows, no phantom border/dock entry remains, and `window_close` carries the closed ids | |
| 8 | Rule matches real app_id at creation | With the kitty rule installed and config reloaded, a **new** kitty lands on workspace 2 in floating mode immediately — no "first appeared on workspace 1, then jumped" | |
| 9 | Destroy before first commit | A client that creates an `xdg_toplevel` and dies before its first `wl_surface.commit` must leave no window and no `window_open` frame | |
| 10 | Popups and focus still work | In a GTK client, open a context menu: it renders above the parent, follows it, dismisses on outside click, and keyboard focus returns to the toplevel | |

Row 9 needs a crafted client; if you have no convenient way to produce one
live, declare **N/A with a reason** and point at the test
`toplevel_first_commit_early_destroy_queues_no_window_open`. Row 6 is N/A if no
available client emits a maximize request — name the client you tried and cite
`client_request_maximize_saves_and_restores_rect`.

## Automated proxy (already green)

`toplevel_first_commit` (3), `toplevel_index` (3), `client_request` (3) and
`toplevel_rules` (3) drive a real wayland-client fixture over a Unix socket.
They prove registration timing, direct lookup, set/unset request semantics and
metadata in events. They cannot prove live focus feel, real F11 behaviour, or
that popups still behave after the commit-hook change.

## Handback

Fill in the table, then tell the agent each row's `PASS`, `FAIL`, or `N/A` with a
one-line reason for every `N/A`. The agent records them in
`specs/verifications/e09s05-verify.yaml`.

- **all PASS / explicit N/A** → agent closes the record and flips `e09s05t5`.
  Merge only with your explicit go-ahead; still no push until you ask.
- **any FAIL** → give the symptom plus the TTY steps that reproduced it; the
  agent logs a defect and fixes it before any merge.

Marking this UAT "waived" is not an option. A toplevel that registers at the
wrong moment is a ghost window that haunts the dock, and ghosts do not respond
to performance reviews.
