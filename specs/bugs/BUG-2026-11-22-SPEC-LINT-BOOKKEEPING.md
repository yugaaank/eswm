---
bug_id: BUG-2026-11-22-SPEC-LINT-BOOKKEEPING
status: open
severity: low
priority: p3
scope: specs
title: spec-lint reports three stale planning-artifact references
---

# BUG-2026-11-22-SPEC-LINT-BOOKKEEPING — spec-lint reports three stale planning-artifact references

**Status:** open (logged during `fix-double-bind-wl-display` landing)
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

- [ ] `python3 scripts/spec-lint.py` reports zero problems.
- [ ] e08 references in `release-plan.yaml` and `execution-status.yaml` agree
      with the capsule layout on disk.
- [ ] All story statuses in `e09s01-tasks.yaml` use accepted enum values.

## Resolution

<!-- filled in by validate-fix -->

Still open as of 2026-11-26: re-checked while moving the cosmic-comp reference
clone, and `spec-lint` still reports the same two e08 problems (the `failing`
status from the original three has since been fixed). The planning SoT is still
wearing last season's e08 jacket; nothing runtime-facing is on fire.
