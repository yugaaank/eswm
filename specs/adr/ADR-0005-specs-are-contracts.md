# ADR-0005: `specs/` is a gated contract

- Status: Accepted
- Date: 2026-11-19
- Deciders: eswm maintainer

## Context

The planning corpus had drifted badly enough to be untrustworthy: twelve spec
files were invalid YAML (unquoted colons), task statuses used three different
vocabularies, eight task `verify:` commands named test filters that did not
exist (`cargo test e06s02t1`), and some used `cargo test --lib` in a binary
crate where it cannot work. A task could be marked done with no test behind it
and nothing noticed.

Planning documents that lie are worse than absent ones: they are used to decide
what is finished.

## Decision

`specs/` is treated as a contract and gated like code. `scripts/spec-lint.py`
runs in CI before the build and fails when:

- any `specs/**/*.yaml` does not parse
- a release-plan epic has no capsule directory or `epic.yaml`
- `execution-status.yaml` disagrees with the capsule directories
- a task lacks id/description/verify/status or uses an unknown status
- a `verify:` command is not executable here (`cargo test --lib`), or names a
  placeholder filter instead of a real test
- a completed task's verify filter matches no test in `src/`

Canonical vocabulary: tasks `pending` / `passing` / `failing`; stories and
epics `pending` / `in_progress` / `completed`.
`scripts/fix-specs-yaml.py` is the idempotent repair counterpart for bulk edits.

## Consequences

- A story cannot claim completion without a test the linter can see; the two
  cannot drift apart silently any more.
- Editing specs is a code-review-grade act: broken YAML or invented statuses
  fail the build.
- `verify:` must name a real filter, which forces tests to be named after the
  behaviour rather than after the task id.
- The linter encodes judgement (what counts as a placeholder). It is code, so
  it gets reviewed and tested like anything else.
