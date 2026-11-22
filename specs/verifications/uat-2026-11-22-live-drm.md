# Live DRM UAT — 2026-11-22, 15:10 IST

**Evidence:** `~/.local/state/eswm/eswm.log`, `eswm.log.old`, `spawn.log`
**Build:** `target/debug/eswm` compiled 15:04 IST, session started 15:10 IST,
`main` at `637977c` — so this run exercised both fixes. The compositor did not
double-bind the Wayland display, and this report did not double-bind the facts.

## Session window

- Start: `2026-11-22T09:40:09Z` (15:10:09 IST)
- Quit via keybind: `2026-11-22T09:40:34Z` (15:10:34 IST)
- Duration: ~26 s; 4 toplevels registered (autostart clients).

## Gate results

| Gate | Result | Evidence |
|---|---|---|
| No `Double bind_wl_display` | **PASS** | Zero matches in current `eswm.log`; the one match is in `eswm.log.old` from the 09:07 UTC session, before the fix. EGL initialized cleanly on `PLATFORM_GBM_KHR`. |
| No new activation-token warnings | **INCONCLUSIVE** | `spawn.log` has 23 historical warnings, the last at 14:24:53 IST — none in this session. But the 18 log lines from 15:10 show no launcher invocation that would mint a token, and the handler logged no activation requests. Absence here means "not exercised", not "fixed and verified". |
| Activation focus semantics | **NOT EXERCISED** | No `xdg activation focused window …` trace in `eswm.log`. |
| DRM restore ERROR on quit | **EXPECTED, OPEN** | Two `Failed to restore previous state ... Permission denied` lines at shutdown — `BUG-2026-11-22-DRM-RESTORE-PERMISSION` is still open by design. |

## Observation (not an eswm defect)

At shutdown, an external client panicked in `daemon/src/main.rs:733` with
`BrokenPipe` after the Wayland connection closed ("The Wayland connection broke.
Did the Wayland compositor die?"). That is client-side teardown noise, not
eswm's code.

## Next action

Need a live run where deckcraft or Helium is launched **from the launcher**
during the session, then re-check:

```sh
grep -i 'activation token' ~/.local/state/eswm/spawn.log
grep -i 'xdg activation' ~/.local/state/eswm/eswm.log
```

Expect no new warnings and a `xdg activation focused window` INFO line.
