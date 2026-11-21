# Audit — e08s01 (Panics & Correctness)

Scope: `git diff 2506fe1..HEAD` (branch `e08s01-panics`). Churn-rank helper scripts absent; review prioritized `src/udev.rs` (190 changed lines) then `src/wm.rs` (39 changed lines) by churn/risk.

## Red flags caught
- Initially skipped manual UAT planning thinking "P2 skips manual by depth" — corrected: risk-scaling does skip step-by-step UAT, but the story spec's runtime verification script must still be handed to the operator (recorded as a gap note in verify evidence).
- Considered adding a unit test for `ContextLost` exit; rejected as artificial (process exit cannot be tested in-process) — documented in verification evidence instead.

## Checklist

### Supply Chain & Security
- ✓ slopcheck: no new dependencies (plan recorded N/A)
- ✓ no `[SLOP]` packages
- ✓ no secrets in diff (single `api` hit was a false positive in prose: "risk-scaled")
- ✓ OWASP spot-check: no user-data/auth/external-API surface touched; changes are geometry/error handling
- ✓ no security-sensitive changes (`security: none` on all tasks)

### Provenance & Metadata
- ✓ story spec has `type:`/`context:` metadata
- ✓ implementation steps reference story/task IDs (`e08s01t1`–`t6`) in comments and task ledger

### Law of Demeter
- ✓ no chains through unrelated objects; new helpers are module-level pure functions

### CONVENTIONS.md Compliance
- ✓ all planning/verification output in `specs/`
- ✓ no `gh issue create`, no `api.github.com`, no direct GitHub REST calls
- ✓ no JSON config added

### Scope
- ✓ changes limited to listed panic paths (dock, floating_target, connector modes, renderer, ContextLost)
- ✓ no speculative features
- ✓ discovered defects: none in Preflight (green before/after)
- ✓ story-lock file added as required by kickoff gate

### Boy Scout Rule
- ✓ no dead code; no commented-out code
- ✓ fixed misplaced e08s01 test comments found during this audit (commit `7cd7fe5`)
- ✓ `gw`/`gh` unused locals removed during t2

### Types and Safety
- ✓ no `any`/`@ts-ignore`/`as unknown as` (Rust)
- ✓ no unsafe casts introduced; `saturating_sub`/`min` used throughout

### Test Coverage
- ✓ every new pure helper has a test: `dock_inner_size`, `dock_segment_layout`, `preferred_mode_index` (the `exit_on_context_lost` process-exit path cannot be unit-tested; documented)
- ✓ bug fixes have regression tests: underflow tests RED-confirmed before GREEN (exact panic at `wm.rs:711`)
- ✓ tests verify behavior via public interfaces (`add_window` rule path for floating_target; helper-level tests are pure/observable)
- ✓ F.I.R.S.T: fast (122 tests in 0.44s), independent, repeatable, self-validating, timely (RED first)

### SOLID and Heuristics
- ✓ helpers are single-purpose; no new abstractions without justification
- ✓ no feature envy / data clumps / message chains
- note: `DeviceAddError::Renderer(String)` uses a string rather than a typed error — justified because `single_renderer`'s `multigpu::Error` is generic over backend types and is already stringified elsewhere in this file for logging.

### Refactoring Smells
- Mysterious Name: none (names descriptive)
- Duplicated Code: none (dock math centralized in helpers)
- Feature Envy: none
- Data Clumps: none
- Primitive Obsession: minor (`DockSegmentLayout` groups 5 related usize values — already extracted)
- Message Chains: none
- Middle Man: none

### Code Style
- ✓ new functions 3–12 lines, single abstraction level
- ✓ `src/udev.rs` remains a large file (>3000 lines) — pre-existing condition; this change added ~100 lines of helpers/tests. Splitting the DRM backend is out of scope for this bug-fix story.
- ✓ new names descriptive and specific (multi-word, grep-unique in concept); raw grep counts include test/call sites
- ✓ no duplication; early returns used in connector/renderer error paths
- ✓ comments explain WHY (underflow/hotplug rationale), not WHAT

### Final gates
```
cargo clippy --all-targets  → 0
cargo test --bin eswm       → 122 passed, 0 failed
cargo build                 → 0
```

## Verdict
**PASS** — one clarity defect (misplaced comments) found and fixed; all other items pass.
Suggest `request-review` for independent second opinion.
