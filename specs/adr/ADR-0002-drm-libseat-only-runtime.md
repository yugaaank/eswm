# ADR-0002: DRM/libseat is the only shipped backend

- Status: Accepted
- Date: 2026-11-19
- Deciders: eswm maintainer

## Context

Early milestones ran eswm as a nested compositor in a winit window, which made
protocol work testable inside an existing Wayland session. That was useful then
and is a liability now: `winit` is no longer a dependency in `Cargo.toml`, there
is no `run_winit`, and `main()` goes straight to `udev::run_udev()`. Docs and
comments still described a "nested dev backend", which is drift a reader would
believe.

The remaining virtual-output path (`new_state(..., want_virtual_output = true)`)
is not a backend. It fabricates a 1280x800 `wl_output` so the inline tests can
exercise placement and protocol handlers without DRM hardware.

## Decision

DRM/libseat is the product runtime and the only backend eswm ships. The
virtual-output path is test scaffolding, reachable from `#[cfg(test)]` fixtures
and nothing else.

## Consequences

- One event loop, one render path to reason about; no backend matrix.
- eswm must run from a TTY and grabs the VT. Running it inside an existing
  Wayland/X11 session is forbidden (see `CONVENTIONS.md` § Never).
- Protocol changes are verified by inline tests plus a manual DRM pass, not by
  a nested window.
- Bringing winit back would be a new ADR, not a comment edit.
