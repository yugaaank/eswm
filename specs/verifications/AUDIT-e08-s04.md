# Audit — e08s04 (Window state flags and content scaling)

Scope: `git diff 235f182..HEAD` on `e08s04-state-flags`. Churn-rank helper script absent; review prioritized `src/main.rs` (55 lines) and `src/wm.rs` (10).

## Red flags caught
- Almost treated the `apply_layout` verify filter matching 0 tests as "test suite green" — caught during develop-tdd; added a real `mode_sent` transition test rather than weakening the verify command.
- Almost flagged `set_mode_sent` not calling `self.dirty = true` as inconsistent with `set_activated_sent` — investigated: it is called inside `apply_layout` after state consumption; marking dirty would force an extra layout pass per configure. Deliberate, correct.

## Checklist

### Supply Chain & Security
- ✓ slopcheck: no new dependencies
- ✓ no secrets, no `gh issue create`, no GitHub API calls
- ✓ OWASP spot-check: xdg configure state flags are protocol output, no auth/network/user-data surface
- ✓ no HIGH findings (`security: none` on all tasks)

### Provenance & Metadata
- ✓ story spec has `type:`/`context:`; steps reference task IDs (e08s04t1–t6); re-plan rationale documented

### Law of Demeter
- ✓ `xdg_state_flags_for_mode` is pure; `set_mode_sent` talks to the immediate `WindowManager` field; no unrelated-object chains

### CONVENTIONS.md Compliance
- ✓ all output in `specs/`; no docs at project root; no GitHub REST calls

### Scope
- ✓ state flags + mode tracking only; stale viewport workaround correctly dropped (none in source)
- ✓ no gate failures; Preflight green throughout

### Boy Scout Rule
- ✓ no dead code (mid-story warnings resolved when t3 consumed `mode_sent`)
- ✓ no commented-out code

### Types and Safety
- ✓ typed `WindowMode` match (exhaustive, compiler-checked); no unsafe casts
- ✓ `mode_sent: Option<WindowMode>` mirrors `activated_sent` pattern

### Test Coverage
- ✓ new helper `xdg_state_flags_for_mode` tested for all four modes (2 tests, exclusivity + clearing)
- ✓ `mode_sent` tracking test covers None → Maximized → Fullscreen transitions
- ✓ F.I.R.S.T: fast (144 tests in 0.44s), independent, repeatable, self-validating, timely (RED first for t1)
- MED: no real xdg toplevel fixture, so no assertion on actual configure bitflags — documented; manual UAT for client resize waived by explicit operator override (recorded in verification evidence)

### SOLID and Heuristics
- ✓ helper is single-purpose; `apply_layout` extended additively without changing its contract; configure-suppression guard preserved
- ✓ no magic numbers

### Refactoring Smells
- Mysterious Name: none
- Duplicated Code: none material
- Feature Envy: none
- Data Clumps: none
- Primitive Obsession: none
- Message Chains: none
- Middle Man: none

### Code Style
- ✓ functions 1–10 lines, WHY comments, early continue preserved
- ✓ names specific and greppable (`xdg_state_flags_for_mode` 6 hits incl. tests/calls)
- ✓ final clippy run: 0 warnings

### Final gates
```
cargo clippy --all-targets → 0 (no warnings)
cargo test --bin eswm      → 144 passed, 0 failed
cargo build                → 0
```

## Verdict
**PASS** — no defects found; all checklist items pass.
Suggest `request-review` for independent second opinion.

## Note
Manual UAT (e08s04t6) was waived by explicit operator decision; automated proxy (flag mapping + mode_sent tracking + no-viewport check) is recorded as sufficient. This is a verification-evidence matter, not an audit finding.
