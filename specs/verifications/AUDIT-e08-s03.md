# Audit — e08s03 (Scale, cursors, input settings)

Scope: `git diff 6104475..HEAD` on `e08s03-scale-cursor`. Churn-rank helper script absent; review prioritized `src/lua_state.rs` (212 lines), `src/udev.rs` (87), `src/main.rs` (50).

## Red flags caught
- Almost skipped the audit because "all four concerns were individually verified" — rejected: verification checks behavior, audit checks hygiene and cross-cutting clarity.
- Considered flagging `redraw_needed = true` in `cursor_image()` as a hot-loop regression — investigated: smithay emits cursor-image on enter/theme change, not per motion; redraw is required to display the change. Not a defect.

## Checklist

### Supply Chain & Security
- ✓ slopcheck: no new dependencies (plan recorded N/A)
- ✓ no `[SLOP]` packages
- ✓ no secrets (one `sk` false positive in the word "scale" in evidence prose), no `gh issue create`, no GitHub API calls
- ✓ OWASP spot-check: config strings are local Lua (trusted boundary); keymap/scale validated before use; no auth/network/user-data surface
- ✓ no HIGH findings (`security: none` on all tasks)

### Provenance & Metadata
- ✓ story spec has `type:`/`context:`; steps reference task IDs (e08s03t1–t8)

### Law of Demeter
- ✓ config helpers take `&InputConfig`; `resolve_touchpad` called from the libinput callback via one static path; no chains through unrelated objects

### CONVENTIONS.md Compliance
- ✓ all output in `specs/`; no docs at project root; no GitHub REST calls

### Scope
- ✓ four settings only (cursor, per-output scale, touchpad, keymap); no speculative extras
- ✓ no gate failures; Preflight green throughout

### Boy Scout Rule
- ✓ no dead code at completion (mid-story dead-code warnings resolved as t5–t8 consumers landed)
- ✓ clippy style warnings (bool asserts, field_reassign_with_default, type complexity in test helper) cleaned up during GREEN phases
- ✓ no commented-out code

### Types and Safety
- ✓ typed `InputConfig`/`TouchpadSettings`/`CursorRenderDecision`; `output_scale_for` rejects NaN/infinite/negative before constructing `Scale::Fractional`
- ✓ Lua setters validate before mutating shared state; no unsafe casts

### Test Coverage
- ✓ every new pure helper tested: `output_scale_for` (2 tests), `InputConfig` scale map (1), `resolve_touchpad` (2), `validate_keymap` (1), `cursor_render_decision` (2), Lua setters (2) = 10 tests
- ✓ T5 boundary coverage: 0/negative/NaN/inf scale, empty/whitespace keymap, default-vs-override touchpad values
- ✓ F.I.R.S.T: fast (141 tests in 0.41s), independent, repeatable, self-validating, timely (RED first for t1–t4)
- MED: t5/t6/t7/t8 hardware/libinput/Wayland wiring is build-verified with pure policy helpers unit-tested; documented in verify evidence

### SOLID and Heuristics
- ✓ `InputConfig` is data-only; helpers are single-purpose; `register_api` extended additively (Open/Closed)
- ✓ no magic numbers left unwrapped beyond existing 0.7 touchpad default (now in `resolve_touchpad`)

### Refactoring Smells
- Mysterious Name: none
- Duplicated Code: none material
- Feature Envy: none
- Data Clumps: `InputConfig` groups related settings (appropriate)
- Primitive Obsession: none
- Message Chains: none
- Middle Man: none

### Code Style
- ✓ new functions 1–14 lines, early returns, WHY comments
- ✓ `src/lua_state.rs` now ~700 lines (was ~550); above the 300-line target but consistent with this single-file config API module — splitting is a larger refactor out of scope
- ✓ final clippy run: 0 warnings

### Final gates
```
cargo clippy --all-targets → 0 (no warnings)
cargo test --bin eswm      → 141 passed, 0 failed
cargo build                → 0
```

## Verdict
**PASS** — no defects found; all checklist items pass.
Suggest `request-review` for independent second opinion.
