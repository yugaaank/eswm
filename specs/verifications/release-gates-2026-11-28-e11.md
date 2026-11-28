# Release gates — e11 Phase 0 (2026-11-28)

**Branch:** `e11-proper-de-phase0`
**Range:** `main..HEAD` (`610b108..daf9631`)
**Decision:** local-only release, authorised by the operator. No push, no CI run.

## Preflight (green)

- `cargo clippy --all-targets`: clean
- `cargo test`: **350 passed / 0 failed**
- `cargo build`: clean
- `python3 scripts/spec-lint.py`: ok (29 task files, 10 epics)
- `git diff --check`: clean
- All `main..HEAD` commits use Conventional Commits; no `Co-authored-by` footer.

## Coverage (manual LLVM instrumentation on the final commit)

Built with `RUSTFLAGS=-C instrument-coverage` into a scratch target dir, ran the
test binary with `LLVM_PROFILE_FILE`, merged with `llvm-profdata`, reported with
`llvm-cov`. Evidence: `specs/verifications/e11-coverage-raw.txt` and
`specs/verifications/e11-coverage-filtered.txt`.

| View | Regions | Lines | Functions |
| --- | --- | --- | --- |
| Raw (all compiled source) | 60.07% | 61.60% | 49.76% |
| Filtered (generated bindings and `src/udev.rs` excluded) | 85.31% | 83.35% | 79.67% |

The filtered view removes code that cannot be exercised without a DRM/VT
session: generated Wayland protocol bindings and the libseat/DRM event loop in
`src/udev.rs`. Honest disposition: the raw line/regional floor (60%) is met,
but the skill's 80% overall and 95% business-logic bars are **not** met. This
is recorded as an explicit local override, not claimed as a pass. `src/udev.rs`
is covered by the operator's live DRM UAT record instead.

## Gate-trace: WAIVED

The bigpowers trace tooling was run from its installed copy. It produced
`traceability-matrix.json` with **0 stories** and `blind-spots.json` with
0 findings. The matrix is structurally empty because eswm's
`specs/release-plan.yaml` has no per-story inventory and
`specs/execution-status.yaml` is a flat epic map, not the nested `stories:`
shape the tool expects. The 58 orphan tags came from the unrelated untracked
`skills/` directory, not from e11 code.

Applied rules: R4 does not trigger (raw lines 61.60% ≥ 60%); R6 does not
trigger (no `e11-TEST_PLAN_LATEST.md`); R1/R2/R3/R5 cannot be evaluated without
a story inventory. Oracle stats are all zero, so no confidence downgrade.
Verdict **WAIVED** — inputs cannot be evaluated, no FAIL gate was found.
Generated matrix files were removed after recording this result so they do not
ship misleading zero-story data.

## Adversarial refute

Attempted refutation: "clean shutdown removes only eswm-owned values" was
false in the first build — the external systemd/D-Bus values survived a quit.
That gap was real and was fixed in `daf9631` with an ownership-guarded snapshot
and restore, plus caller-env preservation. Second refutation: the new external
restore had only unit coverage at release time. The operator then ran the
post-review build in the clean `eswmdev` DRM session and reported `its working
i have confirmed`, closing row 8 of the live UAT record with an operator PASS.
The D-Bus empty-string caveat for previously absent values stays documented in
`docs/session.md`. The agent cannot start or quit eswm from this graphical
session (ADR-0002); the agent-steered VT remains firmly unwrapped.

## Audit and security

- `specs/AUDIT_LATEST.md`: VERDICT PASS for `main..HEAD`.
- `specs/security/REVIEW.md`: no HIGH (≥8 confidence) findings. The new
  external restore uses fixed argv data, no shell, no secrets, and an opt-out.

VERDICT: WAIVED (traceability); coverage override recorded above.
