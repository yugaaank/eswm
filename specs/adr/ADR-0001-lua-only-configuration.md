# ADR-0001: Configuration is Lua only

- Status: Accepted
- Date: 2026-11-19
- Deciders: eswm maintainer, e09-design-decisions scope

## Context

eswm targets Tacit, whose shell expects a programmable config surface
(keybinds, dispatching, window rules, decoration values). Hyprland's ecosystem
does the same with `hyprland.conf` plus `hyprctl`. A JSON config file is the
easy default and was considered, because it parses without embedding a runtime.

The catch: a declarative JSON schema has to grow a scripting layer anyway as
soon as a user wants a conditional or a loop, and then there are two config
languages with two parsers and two error surfaces.

## Decision

Configuration is Lua only, at `~/.config/eswm/config.lua`, using the `es` API
(`es.bind`, `es.bind_cmd`, `es.dsp.*`, `es.config{}`). There is no JSON config
path, not even a compatibility shim.

`src/lua_state.rs` owns the embedded `mlua` state and registers the API; the
compositor reads a `Snapshot`/`CompCmd` bridge rather than calling into Lua
from the hot loop.

## Consequences

- One language, one parser, one error surface. Conditional config is free.
- The dependency on `mlua` and Lua 5.4 is permanent for the product goal.
- Config errors must not kill the compositor: reload is best-effort and the
  previous config stays live on failure.
- Any future "just add a settings file" proposal is rejected on sight; put the
  setting in the Lua API instead.
