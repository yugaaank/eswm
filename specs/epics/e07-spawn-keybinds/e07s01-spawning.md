### Story e07s01: Spawning: sh -c, XDG_CURRENT_DESKTOP, XDG_STATE_HOME logs, SIGCHLD

**type:** fix  
**risk:** P3  
**context:** process  

**Context:** Commands split on whitespace; Tacit's autostart uses bash -c '...'.
Logs go to /tmp/<program>-spawn.log (predictable, shared, readable, never rotated).
TERMINAL=kitty hardcoded; no XDG_CURRENT_DESKTOP; each child keeps its own waiting
thread. Hyprland runs exec strings as sh -c <string>.

## Steps

1. Run exec strings as sh -c <string>; add argv form es.exec({"prog", "arg"})
   -> verify: `cargo build`
2. Set XDG_CURRENT_DESKTOP=eswm; take terminal from config -> verify: `cargo build`
3. Send child output to $XDG_STATE_HOME/eswm/spawn.log (0600, appended) -> verify: `cargo build`
4. Reap children from one calloop SIGCHLD source with waitpid(-1, WNOHANG) loop
   -> verify: `cargo build`

## Verification Script

1. bash -c 'echo a b' reaches bash as -c plus one argument.
2. FOO=1 prog runs prog with FOO set.

## Out of scope

- journald logging option

## Risks

- sh -c quoting differences from whitespace split; test with special characters.

---
