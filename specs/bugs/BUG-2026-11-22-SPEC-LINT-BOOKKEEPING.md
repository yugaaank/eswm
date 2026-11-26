---
bug_id: BUG-2026-11-22-SPEC-LINT-BOOKKEEPING
status: closed
severity: low
priority: p3
scope: specs
title: spec-lint reports three stale planning-artifact references
---

# BUG-2026-11-22-SPEC-LINT-BOOKKEEPING — spec-lint reports three stale planning-artifact references

**Status:** closed — linter now finds archived capsules; `spec-lint: ok (23 task files, 9 epics)` on 2026-11-26
**Security impact: NONE.** Planning metadata only; no runtime or trust surface.

## Problem

`python3 scripts/spec-lint.py` exits with three problems:

```
spec-lint: 3 problem(s)
  - release-plan: epic e08 has no specs/epics/e08-* capsule
  - execution-status: lists e08-panics-correctness with no capsule directory
  - specs/epics/e09-design-decisions/e09s01-tasks.yaml: story has unknown status 'failing'
```

Expected: zero problems, so the planning SoT accurately describes the repo.

## Root cause hypothesis

Three separate bookkeeping drifts, not one failure:

1. `specs/release-plan.yaml` still lists epic `e08` as completed but no
   `specs/epics/e08-*` capsule exists (it was completed and the capsule
   archived or never created).
2. `specs/execution-status.yaml` still references `e08-panics-correctness`,
   whose capsule directory is gone.
3. `e09s01-tasks.yaml` uses a story status value `failing` that is not in the
   linter's accepted enum.

Not a runtime defect. This was found while running an extra spec gate during
the double-bind landing; it is not part of the project's declared Preflight
(`cargo clippy && cargo test && cargo build`), which stayed green.

## Fix approach

Decide the intended surviving state for e08 (restore capsule or drop the
references), sync `execution-status.yaml`, and map `failing` to the supported
status enum. Verify with `python3 scripts/spec-lint.py`.

## Acceptance criteria

- [x] `python3 scripts/spec-lint.py` reports zero problems.
- [x] e08 references in `release-plan.yaml` and `execution-status.yaml` agree
      with the capsule layout on disk.
- [x] All story statuses in `e09s01-tasks.yaml` use accepted enum values.

## Resolution

Fixed on 2026-11-26. The e08 capsule was deliberately retired to
`specs/epics/archive/e08-panics-correctness` in b3854ff; `release-plan.yaml`
and `execution-status.yaml` are historical indexes, so the linter — not the
history — was wrong. `scripts/spec-lint.py` now discovers capsules in both
`specs/epics/` and `specs/epics/archive/` via a shared `find_capsules()` helper
(no more hand-edited lint comments — the archive itself is the storage). The
`failing` story status had already been corrected upstream. Verified:
`python3 scripts/spec-lint.py` → `spec-lint: ok (23 task files, 9 epics)`,
exit 0. The planning SoT finally took off last season's e08 jacket.
