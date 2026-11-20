# Security Review — e07s01 (spawning)

**Branch:** `e07s01-spawning` (`main..HEAD`)
**Date:** 2026-11-20
**Risk:** P3 (per story)

## Scope
- `src/main.rs`: unified spawn helpers (shell `sh -c` / argv exec), configurable terminal, `XDG_CURRENT_DESKTOP`/`TERMINAL` env, spawn log at `$XDG_STATE_HOME/eswm/spawn.log` (0600, append, stdout+stderr), child tracking and `waitpid(-1, WNOHANG)` reaper.
- `src/lua_state.rs`: `es.dsp.exec_cmd` shell-string execution, top-level `es.exec(argv)`, `es.dsp.set_terminal(...)`.
- `src/udev.rs`: shell execution for `exec_cmd` keybind action, SIGCHLD `signalfd` + calloop source invoking reaper.
- `specs/*`: planning, tasks, verification, and release bookkeeping for e07s01.

## Findings
- **HIGH (≥8 confidence):** none.
- **MEDIUM/LOW:** none identified.

## Notes
- Command strings are intentionally executed via `sh -c` per story/config semantics (trusted user configuration boundary, consistent with prior exec behavior but with shell interpretation). `es.exec` validates non-empty argv; `es.dsp.set_terminal` validates non-empty name.
- Spawn log file is created/appended with mode `0600` under resolved eswm state directory; no secrets or credentials handled.
- Signal/process handling uses standard Linux primitives (`signalfd`, `sigprocmask`, `waitpid`) scoped to child reaping; no privilege escalation or external network/auth surfaces introduced.
- Runtime/manual UAT for live session behavior (shell quoting/env-prefix, terminal launch, log output, SIGCHLD reaping on DRM backend) remains a documented verification gap (P3), not a security finding.

**Verdict:** PASS (no unresolved HIGH findings). No exceptions required.
