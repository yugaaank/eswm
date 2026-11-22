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

## Second run — 15:12 IST (both gates PASS)

- Session: `2026-11-22T09:42:10Z` → `09:42:33Z` (15:12:10 → 15:12:33 IST).
- `Double bind_wl_display`: 0 matches.
- Toplevel id=1 registered at `09:42:16.464Z`, then
  `INFO eswm: xdg activation focused window 1` at `09:42:16.666Z` — the handler
  actually ran for a freshly mapped window. Three toplevels total.
- `spawn.log` still has exactly 23 activation warnings, newest at 14:24:53, so
  no new warnings in this session. No new glfw 65544 either.
- DRM restore ERRORs: 2, unchanged and expected (open bug).

**Verdict:** BUG-2026-11-22-DOUBLE-BIND-WL-DISPLAY and
BUG-2026-11-22-XDG-ACTIVATION-GLOBAL both pass live UAT.

### New finding from this run (not a regression of either fix)

Six `Failed to create EGLImage from the buffer` / `eglCreateImageKHR BAD_MATCH`
errors and one GL `EGLImage not supported` error while rendering the new
window, format `AB24`, modifier `Unrecognized(216172782120099860)`, accepted
without a node hint. Neither the 15:10 nor the older baseline log contains
them. The activation fix only registers a protocol global and focuses a window;
it does not touch buffer import, so this is unrelated — but it is new evidence
for a dmabuf/GPU-node investigation.
