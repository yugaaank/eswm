# UAT handoff — 2026-11-22

Branch: `e09s01-dmabuf-uat` (worktree `/home/yugaaank/Projects/e09s01-dmabuf-uat`)
Merge commit: `afbc594` (e09s01-client-mode + fix-dmabuf-import) — the earlier
`84706cb` note was stale; bin and commit now agree, unlike my sleep schedule.

Automated gates on this branch:

- `cargo clippy --all-targets` — clean
- `cargo test --bin eswm` — 156 passed, 0 failed
- `cargo build` — green

Do **not** run the DRM/libseat backend inside an existing Wayland/X11 session.
Switch to a TTY, then from this worktree run `cargo run`.

## One session, four checks

1. **Helium** — open it. Client content must render (not just titlebar/borders).
   Check `~/.local/state/eswm/eswm.log` for `BAD_MATCH`,
   `Failed to convert between dmabuf and EGLImage`, or `EGLImage not supported`.
   Those errors must be gone.
2. **Wordcraft** — open it. Content must fit inside the frame on first open.
   Watch for repeated `client committed NxM > configured ...; re-sending size`
   lines; a couple are fine, a continuing stream is not.
3. **e09s01 placement** — kitty: maximize then restore, fullscreen (F11) then
   restore; both must return to the prior floating placement.
4. **e09s01 focus MRU** — with two outputs, focus window A then B, cycle focus
   and confirm A/B alternate in MRU order.

Record results in `specs/verifications/e09s01-verify.yaml` (t7) and
`specs/verifications/BUG-2026-11-21-DMABUF-IMPORT-verify.yaml`
(`hardware_uat`). If any check fails, keep the branch and reopen the bug —
do not merge into `main`.

If all four pass: close `bug_cycle`, set the bug status to `closed`, then land
the integration branch (the same fix is also available standalone on
`fix-dmabuf-import` for `main`).
