# Self-Audit — BUG-2026-11-22-DRM-RESTORE-PERMISSION

**Branch:** `fix-drm-restore-permission` (`7683b44..HEAD`)
**Date:** 2026-11-22
**Scope:** `src/udev.rs` teardown seam + specs evidence.

## Checklist

### Supply chain & security
- ✓ No new dependencies (`Cargo.toml`/`Cargo.lock` unchanged).
- ✓ No secrets in diff (secrets grep over `git diff 7683b44..HEAD`: empty).
- ✓ No injection/auth/data-exposure surface; security review PASS
  (`specs/security/REVIEW.md`).
- ✓ No OWASP-relevant boundary touched.

### Provenance & metadata
- ✓ New verification artifacts include bug id, branch, and evidence fields.
- ✓ Fix rationale references smithay source lines and the RED/GREEN commit SHAs.

### Law of Demeter
- ✓ `BackendData::quiesce()` calls its own field's `pause()` — immediate
  collaborator only. No chained reach-through.

### CONVENTIONS.md compliance
- ✓ All new docs in `specs/`. No `gh issue create`, no direct GitHub API calls.
- ✓ Discovered defect (the same defect class on hot-unplug) is logged in the
  sweep artifact, not silently ignored.

### Scope
- ✓ Only the quit teardown path and its specs evidence changed.
- ✓ No speculative features. The one sibling defect instance is tracked, not
  patched, because hot-unplug needs separate UAT.
- ✓ Preflight was green on the base branch before kickoff and is green here.

### Boy Scout Rule
- ✓ No dead code or commented-out blocks introduced.
- ✓ Comments explain WHY (smithay's active guard, double master release).

### Types and safety
- ✓ No `unwrap()` added on a fallible path; `state.udev.as_mut().unwrap()`
  matches the surrounding invariant used throughout `run_udev`.
- ✓ No suppressions, casts, or unsafe added.

### Test coverage
- ✓ Every new function has a test: `quiesce_all` (3 tests), `Quiesce::quiesce`
  exercised via `FakeBackend`.
- ✓ Bug regression covered: two cards → both quiesced (baseline produced two
  ERROR lines).
- ✓ Boundary conditions: empty backend set, already-paused session.
- ✓ F.I.R.S.T: fast (0.00s), independent, repeatable, self-validating, written
  before the implementation (RED commit `1fc95ae`).

### SOLID & heuristics
- ✓ Single Responsibility: `quiesce_all` only quiesces; policy is one boolean.
- ✓ Open/Closed: new `Quiesce` seam avoids editing `BackendData` internals.
- ✓ No message chains, data clumps, or duplication introduced.
- ⚠ F3 (flag argument): `session_paused: bool` is a policy flag. Accepted: it
  is a single decision input, not a mode switch, and its truth table is tested
  explicitly. Renaming to an enum would add ceremony without changing behavior.

### Code style
- ✓ New functions are 1–12 lines; descend one level of abstraction.
- ✓ Early return used for the already-paused case.
- ✓ Names are specific and grep-unique (`quiesce_all`, `Quiesce`).
- ✓ `src/udev.rs` already exceeds 300 lines pre-existing; this change adds 80
  lines (mostly specs-adjacent tests), no new file created.

## Rationalizations caught
- "It's just a one-line pause call, no test needed" — rejected: wrote three
  tests and a separate RED commit.
- "Live UAT is impossible so mark it passed" — rejected: recorded honestly as
  `pending_operator`.
- "The hot-unplug sibling is basically the same bug" — rejected: kept it as a
  tracked follow-up to preserve one-bug-at-a-time scope.

## Verdict
**PASS**, with one accepted F3 nit documented above. Hand off to independent
review (`request-review`).
