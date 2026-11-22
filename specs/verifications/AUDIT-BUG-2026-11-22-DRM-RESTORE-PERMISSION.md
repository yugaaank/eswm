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
- ✓ No `unwrap()` added on a fallible path; the new `Drop` impl uses only safe
  iterator access to `self.backends`.
- ✓ No suppressions, casts, or unsafe added. Re-acquiring DRM master uses the
  existing public drm-rs trait, fully qualified.

### Test coverage
- ✓ Every new function has a test: `quiesce_unprivileged` (4 tests via
  `FakeBackend`), covering both `drm_master()` and `quiesce()`.
- ✓ Bug regression covered: two unprivileged cards → both quiesced (baseline
  produced two ERROR lines).
- ✓ Regression guard: privileged card is left active, preventing the HIGH
  review finding (lost getty restore + double master release).
- ✓ Boundary conditions: empty backend set, mixed privileged/unprivileged map.
- ✓ F.I.R.S.T: fast (0.00s), independent, repeatable, self-validating, written
  before the implementation (final RED commit `7318ff6`).

### SOLID & heuristics
- ✓ Single Responsibility: `quiesce_unprivileged` only quiesces; the policy is
  the backend's own `drm_master()` state.
- ✓ Open/Closed: new `Quiesce` seam avoids editing `BackendData` internals.
- ✓ No message chains, data clumps, or duplication introduced.
- ✓ No flag argument remains: the earlier `session_paused: bool` was removed
  because it could leave a late-added card active.

### Code style
- ✓ New functions are 1–12 lines; descend one level of abstraction.
- ✓ Early return used for the already-paused case.
- ✓ Names are specific and grep-unique (`quiesce_all`, `Quiesce`).
- ✓ `src/udev.rs` already exceeds 300 lines pre-existing; this change adds 80
  lines (mostly specs-adjacent tests), no new file created.

## Rationalizations caught
- "It's just a one-line pause call, no test needed" — rejected: wrote four
  tests and a separate RED commit.
- "The first GREEN is fine, review is a formality" — rejected: independent
  review found a HIGH privileged-TTY regression; the whole approach was
  replaced with a per-card privilege policy rather than merged.
- "Live UAT is impossible so mark it passed" — rejected: recorded honestly as
  `pending_operator`.
- "The hot-unplug sibling is basically the same bug" — rejected: kept it as a
  tracked follow-up to preserve one-bug-at-a-time scope.

## Verdict
**PASS** after incorporating the independent reviewer's blocking findings.
No outstanding nits. Hand off to independent re-review.
