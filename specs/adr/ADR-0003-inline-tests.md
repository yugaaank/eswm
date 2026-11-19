# ADR-0003: Tests live inline in `src/`

- Status: Accepted
- Date: 2026-11-19
- Deciders: eswm maintainer

## Context

`CONVENTIONS.md` said "Tests under `tests/`". Reality: `tests/` contains only
JSON fixtures, and all 87 tests live in `#[cfg(test)] mod tests` blocks inside
`src/{main,wm,udev,ipc,edid}.rs`. A stated convention nobody follows is worse
than no convention, because it invites a new test in the wrong place.

eswm is a single binary crate (`src/main.rs` with `mod wm; mod udev; ...`).
Nothing is `pub` to an external crate, so a `tests/` integration binary cannot
see `State`, `WindowManager`, or any of the helpers the tests drive. The tests
mostly do exactly that: build a private `State`, insert a real Wayland client
over a `UnixStream` pair, and assert on focus/placement/configure state.

## Decision

Tests are inline in `src/*.rs` under `#[cfg(test)] mod tests`. `tests/` holds
data fixtures only (`tests/fixtures/hyprctl-*.json`), read through a relative
path.

## Consequences

- Tests can reach private state, which is the point; they are unit/contract
  tests of compositor internals, not black-box integration tests.
- `cargo test` is the single command; there is no `--test` target to remember.
- If a second crate ever appears (a library split), the convention is revisited
  with a new ADR rather than silently growing a `tests/` tree.
- Test helper visibility needs no `pub(crate)`: the module already lives inside
  the crate.
