# eswm — AI Agents

<!-- BEGIN bigpowers:project -->
## Project
eswm is an independent, standalone Wayland compositor and window manager built to run Tacit.

Stack: Rust 2024, smithay 0.7, wayland-server 0.31, winit, mlua.

## Commands
| Action | Command |
|--------|---------|
| Run    | `cargo run` |
| Test   | `cargo test` |
| Build  | `cargo build` |
| Lint   | `cargo clippy` |
| Preflight | `cargo clippy && cargo test && cargo build` |

## Architecture
src/{control,ipc,lua_state,main,udev,wm}.rs plus tests/. Lua-only config via ~/.config/eswm/config.lua using the es API (es.bind, es.bind_cmd, es.dsp.*). DRM/libseat is the default runtime path; Winit is the nested dev backend.

## Conventions
- eswm is the brand; "easy" means short, grep-unique names — no clever abbreviations
- Lua-only configuration; no JSON config path
- All planning output in specs/
- Preflight green before forward work

## Never
- Never hand-edit Cargo.lock — cargo add / cargo update only
- Never add JSON config — Lua-only config path is product-goal locked
- Never run DRM/libseat backend from inside an existing Wayland/X11 session — VT grab
- Never copy Pinnacle source, assets, or distinctive implementation (reference only)
- Never skip preflight before forward work — red gate = fix-or-log

## Agent Rules
- **Workflow Mandate:** You MUST use the bigpowers skills (e.g. plan-work, develop-tdd, orchestrate-project) to perform tasks. DO NOT write code directly in response to a user prompt like "build this feature".
- **Always Green:** Preflight and CI must be green before forward work. Reproducible gate failures require fix-or-log (quick-fix -> fix-bug) per CONVENTIONS § Discovered Defects.
- Read specs/ before writing code.
- All planning and specifications MUST be written to specs/ before any code is generated.
- Write the minimum code that solves the stated problem. Nothing extra.
- Run tests after every change. Show evidence before declaring done.
- One clarifying question beats a wrong assumption baked into 200 lines.
<!-- END bigpowers:project -->

<!-- BEGIN bigpowers:context-routing -->
## Context Routing
Glob -> sub-AGENTS.md table. None configured yet.
<!-- END bigpowers:context-routing -->

<!-- BEGIN bigpowers:learned-preferences -->
## Learned User Preferences
- (none yet)

## Workspace Facts
- (none yet)
<!-- END bigpowers:learned-preferences -->
