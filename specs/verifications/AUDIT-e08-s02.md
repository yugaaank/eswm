# Audit — e08s02 (Correctness: close/dock/fullscreen/CSD)

Scope: `git diff 3ebd6b4..HEAD` on `e08s02-correctness`. Churn-rank helper script absent; review prioritized `src/udev.rs` (162 changed lines), `src/main.rs` (60), `src/wm.rs` (10).

## Red flags caught
- Considered skipping the audit because "verify-work already ran all gates" — rejected: verify checks behavior, audit checks hygiene/clarity; distinct mandates.
- Considered removing the `TitlebarCloseAction` helper as a shallow bool-wrapper (F3 smell) — kept: it encodes the no-orphan contract as a named, tested policy, and removing it would rewrite shipped RED/GREEN history for negligible gain. Documented as MED observation, not a blocker.

## Checklist

### Supply Chain & Security
- ✓ slopcheck: no new dependencies (plan recorded N/A)
- ✓ no `[SLOP]` packages
- ✓ no secrets / `gh issue create` / `api.github.com` calls in diff
- ✓ OWASP spot-check: no user-data/auth/external-API surface; changes are UI z-order, close semantics, and geometry mapping
- ✓ no HIGH findings (`security: none` on all tasks)

### Provenance & Metadata
- ✓ story spec has `type:`/`context:`; steps reference task IDs (e08s02t1–t6)

### Law of Demeter
- ✓ `with_states(...).cached_state.get::<...>().current().geometry` reads cached state directly (no chain through unrelated objects); `toplevel_for_window` + `send_close()` talks to immediate collaborators

### CONVENTIONS.md Compliance
- ✓ all planning/verification output in `specs/`; no docs at project root
- ✓ no GitHub API calls

### Scope
- ✓ four fixes only (close, dock/fullscreen z-order, CSD offset); no speculative extras
- ✓ no discovered gate failures; Preflight green before and after

### Boy Scout Rule
- ✓ **Found and fixed 1 defect:** `/// Send wl_keyboard leave/enter for id; None => just leave.` was orphaned above `RenderBand` after the new helpers were inserted; moved back to `keyboard_focus_set` (commit `fda418c`). C2/C4 stale-comment violation, now clean.
- ✓ no dead code or commented-out blocks introduced

### Types and Safety
- ✓ typed helpers (`RenderBand`, `TitlebarCloseAction`, `Rectangle<i32, Logical>`); no unsafe casts
- ✓ `set_window_xdg_offset` returns bool status (used nowhere yet, but a legitimate API contract)

### Test Coverage
- ✓ every new pure helper tested: `titlebar_close_action` (2 tests), `render_band_order` (3 tests), `window_geometry_offset` (3 tests)
- ✓ bug fixes have regression tests: titlebar close no-orphan (integration-style through `apply_titlebar_action`), z-order policy, CSD offset mapping incl. negative origin (T5 boundary)
- ✓ F.I.R.S.T: fast (131 tests in 0.43s), independent, repeatable, self-validating, timely (RED first for t1/t3/t5)
- MED: t2/t4/t6 hardware/toplevel boundaries lack direct unit tests; documented in verification evidence

### SOLID and Heuristics
- ✓ single-purpose helpers; `render_surface` assembly is one band loop (Stepdown G34)
- MED: `titlebar_close_action(bool)` is a thin flag-argument wrapper (F3) — justified as a named policy seam for the no-orphan contract
- low: fullscreen-mode detection repeated in 3 spots in `render_surface`; identical 3-line predicate, no behavioral divergence — acceptable duplication

### Refactoring Smells
- Mysterious Name: none
- Duplicated Code: minor (fullscreen mode predicate ×3, noted above)
- Feature Envy: none
- Data Clumps: none
- Primitive Obsession: none material (`RenderBand` enum used instead of string flags)
- Message Chains: none
- Middle Man: none

### Code Style
- ✓ new functions 1–13 lines; early returns in Close branch; WHY comments
- ✓ `src/udev.rs` remains >3000 lines (pre-existing); this change added ~50 lines of helpers/tests
- ✓ names descriptive (`render_band_order`, `window_geometry_offset`, `set_window_xdg_offset`)

### Final gates (after comment fix)
```
cargo clippy --all-targets → 0
cargo test --bin eswm      → 131 passed, 0 failed
cargo build                → 0
```

## Verdict
**PASS** — one clarity defect (orphaned doc comment) found and fixed; all other items pass.
Suggest `request-review` for independent second opinion.
