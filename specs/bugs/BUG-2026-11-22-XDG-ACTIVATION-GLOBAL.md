---
bug_id: BUG-2026-11-22-XDG-ACTIVATION-GLOBAL
status: open
severity: low
priority: p3
scope: wayland
title: Clients cannot mint xdg activation tokens
---

# BUG-2026-11-22-XDG-ACTIVATION-GLOBAL — Clients cannot mint xdg activation tokens

**Status:** open
**Discovered:** log analysis of `~/.local/state/eswm/spawn.log` for the
13:31 session (local time).

**Security impact: NONE.** A protocol surface is absent; no trust boundary is
crossed.

## Problem

Clients repeatedly report:

```
Unable to mint xdg activation token to launch "ai.storyteller.deckcraft"
Unable to mint xdg activation token to launch "helium.desktop"
```

eswm does not register the `xdg_activation_v1` global, so clients launch without
activation intent and may arrive unfocused or rely on their own fallback.
Expected: the compositor advertises activation so launches can carry focus
semantics.

## Root cause hypothesis

No activation protocol global is added to the Wayland display at init.

## Acceptance criteria

- [ ] A client can request and receive an activation token from eswm.
- [ ] Launching deckcraft/Helium no longer logs the warning in `spawn.log`.
- [ ] `cargo clippy --all-targets`, `cargo test --bin eswm`, `cargo build` green.
