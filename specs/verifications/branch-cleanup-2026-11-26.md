# Branch cleanup: 2026-11-26

**Authorization:** operator chose "Everything except main" after being told
that `fix-layer-input` held 12 unique commits and an uncommitted
`specs/state.yaml` edit. This was an explicit destructive decision.

## Removed

Nine local branches and their nine `origin` counterparts:

- `e09s01-client-mode`
- `e09s02-output-model`
- `e09s03-ipc-event-stream`
- `e10s01-floating-resize`
- `e10s02-tiling-resize`
- `fix-dmabuf-import`
- `fix-layer-input`
- `fix/size-negotiation`
- `rounded-corners-per-corner`

Five worktrees were removed with `--force`: `e09s01-client-mode`,
`e09s02-output-model`, `e09s03-ipc-event-stream`, `fix-dmabuf-import`
(already prunable), and `fix-layer-input`. The OpenCode session was moved to
`/home/yugaaank/Projects/eswm` before its worktree was removed.

## Insurance kept outside the repo

- `/tmp/opencode/eswm-stale-branches-2026-11-26.bundle` — complete history for
  all nine tips; `git bundle verify` reported "complete history".
- `/tmp/opencode/fix-layer-input-state.patch` — the uncommitted
  `specs/state.yaml` diff from the `fix-layer-input` worktree.

The branches have left the building. The bundle is their witness protection
program, not a branch — nothing is left cluttering `git branch`.

## Verification

| Check | Result |
|-------|--------|
| `git branch -a` | only `main` and `remotes/origin/main` |
| `git worktree list` | only `/home/yugaaank/Projects/eswm` |
| `git ls-remote --heads origin` | only `refs/heads/main` |
| `cargo clippy --all-targets` | PASS |
| `cargo test` | 301 passed, 0 failed |
| `cargo build` | PASS |
| `python3 scripts/spec-lint.py` | `spec-lint: ok (25 task files, 9 epics)` |

Evidence: `/tmp/opencode/branch-cleanup-gate.log`.
