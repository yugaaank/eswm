# UAT handoff — e09s04 Lua config shape (live DRM)

> **Closed 2026-11-28T13:22:44Z:** NOT TESTED, done by explicit operator decision.
> All 12 rows below are N/A; no PASS claimed and no waiver. See `e09s04-verify.yaml`.

Date prepared: 2026-11-26
Branch: `e09-lua-toplevel-rules`
Operator: **you**. The unit tests prove validation in-process; only a live TTY
proves that a reload neither eats your keybinds nor quietly keeps a broken config.

## Safety first

- Run from a real TTY (VT), **not** inside an existing Wayland/X11 session.
  eswm grabs the VT; two compositors sharing one seat is a fistfight with extra
  pixels.
- Keep a second TTY (Ctrl+Alt+F3) open for recovery.
- Back up `~/.config/eswm/config.lua` before editing. A bad config is now loud,
  but a missing keybind is still a nuisance.
- Logs: `/tmp/eswm.log`, `/tmp/eswm-panic.log`.

## Setup

1. Save a known-good config, or seed one from the example:

   ```sh
   mkdir -p /tmp/opencode
   cp ~/.config/eswm/config.lua /tmp/opencode/e09s04-config.backup.lua 2>/dev/null \
     || cp config.example.lua ~/.config/eswm/config.lua
   ```

2. Build the branch on the TTY: `cargo build`.
3. Start eswm with visible logging:

   ```sh
   RUST_LOG=eswm=info ./target/debug/eswm 2>&1 | tee /tmp/opencode/e09s04-eswm.log
   ```

4. In a second TTY, subscribe and check sockets:

   ```sh
   ./start.sh status
   ./start.sh watch | tee /tmp/opencode/e09s04-events.jsonl
   ```

5. Reload via either path: `./start.sh hcmd 'reload'` (compat socket) or a
   bound function key whose callback calls `es.config.reload()`.

Note: `README.md` §Configuration still shows `es.dsp.set_layout_mode("tiling")`,
a fossil from before this branch. The registered setter is
`es.dsp.set_wm_mode("tiling")`. Copying the fossil into a test config makes it
trip over a missing setter like a raccoon in a bin. `config.example.lua` was
already corrected by e09s04t6.

## Checklist — record PASS / FAIL / N/A for each row

| # | Check | Expected | Result |
|---|-------|----------|--------|
| 1 | Clean startup | eswm starts with the example config; no `Failed to load Lua config` and no `attempt to call a nil value` in `/tmp/eswm.log`; `set_wm_mode("tiling")` is accepted | |
| 2 | Declarative config applies | `es.config { outer_gap = 24, border_size = 6 }` in `config.lua`, then reload: gaps and borders visibly change | |
| 3 | Unknown key fails loud | Add `es.config { totally_bogus_key = 1, border_size = 9 }`; reload logs an error naming `totally_bogus_key` and the previous border size stays (no partial apply) | |
| 4 | Wrong type fails loud | `es.config { wm_mode = 123 }`; reload logs an error naming `wm_mode`; old mode survives | |
| 5 | Function bind fires | Add `es.bind("Super+F12", function(arg) es.dsp.exec_cmd("touch /tmp/opencode/e09s04-bind-fired") end, 7)`, reload, press Super+F12: marker file appears once per press | |
| 6 | Event hook receives data | Add `es.on("window_open", function(d) es.dsp.exec_cmd("touch /tmp/opencode/e09s04-hook-" .. tostring(d.id)) end)`, reload, spawn a client: marker with the new window id appears | |
| 7 | Rule from config applies | Add `es.window_rule { app_id = "kitty", workspace = 2, mode = "floating" }`, reload, spawn kitty: it lands on workspace 2 in floating mode | |
| 8 | Reload twice, no duplicates | Run `./start.sh hcmd 'reload'` twice. Each reload emits one `config_reloaded` on the event socket and one `configreloaded>>` on the legacy socket; pressing the function bind still creates exactly one marker (binds were replaced, not stacked) | |
| 9 | Failed reload keeps config | Put a syntax error in `config.lua` (or the row-3 bad block), reload: error logged, old gaps/binds still work; restore the backup afterwards | |
| 10 | Stubs fail loudly | With `pcall` around each, call `es.define_submap("nav", {})`, `es.dsp.global("quickshell:panel")`, `es.config.notify("hi")`, `es.config.setcursor("left_ptr")`, `es.safe_require("module")`, `es.layer_rule { namespace = "bar" }`. Each error names `not implemented: <name>`; compositor keeps running | |
| 11 | Real file existence | A config branch guarded by `es.is_file_exists("/etc/passwd")` takes the true path; the same check on a deliberately missing path takes the false path | |
| 12 | Decoration setter is live | Change border size from a function bind or hook (no restart) and see it change on the next frame (`Rc<RefCell>` visibility) | |

## Automated proxy (already green)

`config_table`, `lua_function_bind`, `lua_event_hook`, `lua_window_rule`,
`config_reload`, `lua_stub`, `decoration_refcell` plus the full preflight. They
prove validation, callback dispatch, replacement-on-reload and stub error
strings in-process; they cannot prove that a real TTY reload preserves your
working session.

## Handback

Fill in the table, then tell the agent each row's `PASS`, `FAIL`, or `N/A` with a
one-line reason for every `N/A`. The agent records them in
`specs/verifications/e09s04-verify.yaml`.

- **all PASS / explicit N/A** → agent closes the record. Merge only with your
  explicit go-ahead; still no push until you ask.
- **any FAIL** → give the symptom plus the TTY steps that reproduced it; the
  agent logs a defect and fixes it before any merge.

Marking this UAT "waived" is not an option. That is how a config API ends up
silently ignoring every key you set, which is just a crash with better manners.
