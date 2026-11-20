# Plan Audit — eswm / e06s04 (Clipboard and primary selection)
**Date:** 2026-11-20 · **Verdict:** READY (with noted gaps)

> **Context:** Audited post-implementation on branch `e06s04-clipboard` (tasks passing, verify-work evidence recorded). This is a readiness assessment of the active story/plan artifacts and project pre-flight, not a pre-build gate for this story.

## Principles Alignment
| Check | Status | Note |
|-------|--------|------|
| Vertical slices (not horizontal layers) | ✅ | e06s04 delivers an end-to-end input/selection slice (focus wiring + protocol globals/handlers + verification); prior e06 stories were similarly scoped. |
| Scope bounded — explicit in_scope + out_of_scope | ⚠️ | `e06s04-clipboard.md` defines **Out of scope** ("Clipboard history manager") and concrete steps, but no explicit **in_scope** section. |
| Success criteria defined (how do we know we're done?) | ⚠️ | Verification script and per-task `verify:` commands (`cargo test --bin eswm data_device*`) define mechanical criteria; runtime success criteria (wl-copy/wl-paste, middle-click, watcher behavior) stated but not yet runtime-verified in this environment. |
| HARD GATE candidates identifiable | ✅ | Focus semantics (keyboard vs pointer), selection separation (clipboard vs primary), and protocol exposure (primary selection, ext-data-control) are clear decision points. |
| Domain language / ubiquitous terminology | ⚠️ | Terms used (clipboard, primary selection, data_device focus, pointer/keyboard focus) are consistent with Wayland/smithay domain, but no explicit project-level ubiquitous language glossary for selection/focus. |

## Conventions Completeness
| Check | Status | Note |
|-------|--------|------|
| `CLAUDE.md` or `AGENTS.md` exists | ✅ | `AGENTS.md` present with project/commands/architecture/rules. |
| `CONVENTIONS.md` exists | ✅ | Present (Always Green, fix-or-log, specs/, defensive categories, naming, never rules). |
| `specs/` directory layout in place | ✅ | `specs/epics/`, `product/`, `tech-architecture/`, `adr/`, `bugs/`, `verifications/` present. |
| Commit conventions documented (Conventional Commits) | ⚠️ | Repo history follows Conventional Commits style, but `CONVENTIONS.md` does not explicitly document the convention. |
| Git workflow mode identified (`solo-git` \| `team-pr`) | ⚠️ | Not explicitly documented in `AGENTS.md`/`CONVENTIONS.md`; solo development implied by project context. |

## Pre-flight Answers
| Question | Value |
|----------|-------|
| Test command | `cargo test` (task-level: `cargo test --bin eswm <filter>`) |
| Build command | `cargo build` |
| Lint command | `cargo clippy` (CI/preflight use `cargo clippy --all-targets`) |
| Typecheck command | `cargo build` / `cargo check` (Rust compile-time type checking; no separate `typecheck` script) |
| CI platform | GitHub Actions (`.github/workflows/ci.yml`, Arch container) |
| Solo or team | Solo (implied) |
| Primary language + framework | Rust 2024, smithay 0.7, wayland-server 0.31, mlua (DRM/libseat backend) |
| Greenfield or existing codebase | Existing codebase |

## Open Gaps
- [ ] Add explicit **in_scope** (and keep out_of_scope) to story specs where helpful (e.g. e06s04) — run `scope-work`/story refinement as needed.
- [ ] Make runtime **success criteria** explicit and evidence-backed for selection flows (wl-copy/wl-paste, middle-click primary, watcher behavior) — capture via `verify-work` manual UAT on DRM session.
- [ ] Document **git workflow mode** (`solo-git` or `team-pr`) in `CONVENTIONS.md`/`AGENTS.md`.
- [ ] Document **commit convention** (Conventional Commits) explicitly in `CONVENTIONS.md`.
- [ ] Consider a lightweight domain/ubiquitous language note for input/focus/selection terms if ambiguity arises — `define-language`.

## Recommended Next Skill
- **`audit-code`** — active story e06s04 has verify-work evidence and `state.yaml` handoff set to `audit-code` (current gate).
- If plan assumptions for remaining work need challenging, use `grill-me` / `elaborate-spec` before new implementation.

## Verdict
**READY** — project pre-flight commands, specs layout, and active story plan artifacts are sufficient to proceed with current gates/next work. Address open gaps opportunistically to reduce future drift.
