# UAT handoff — 2026-11-22

Branch: `e09s01-dmabuf-uat` (worktree `/home/yugaaank/Projects/e09s01-dmabuf-uat`)
Merge commit: `afbc594` (e09s01-client-mode + fix-dmabuf-import) — the earlier
`84706cb` note was stale; bin and commit now agree, unlike my sleep schedule.

Automated gates on this branch (HEAD `e08adfa`):

- `cargo clippy --all-targets` — clean
- `cargo test --bin eswm` — 169 passed, 0 failed
- `cargo build` — green

Latest log analysis: `/home/yugaaank/Projects/eswm/specs/verifications/LOG-ANALYSIS-2026-11-22-HELIUM-CRAFT.md`.
The 13:22 run had no dmabuf errors and a bounded retry cap; the reported
glitches were traced to clipped damage/opaque regions being filtered in the
wrong coordinate space (fixed at `e08adfa`).

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
5. **Glitch recheck** — with a Craft window and a CSD window open, type text or
   run a spinner without moving or resizing either window. Content must repaint
   cleanly: no stale pixels, torn strips, or frozen regions. Confirm Craft
   content still stays inside the border. This is the hardware gate for the
   damage-coordinate fix at `e08adfa`.

Record results in `specs/verifications/e09s01-verify.yaml` (t7) and
`specs/verifications/BUG-2026-11-21-DMABUF-IMPORT-verify.yaml`
(`hardware_uat`). If any check fails, keep the branch and reopen the bug —
do not merge into `main`.

If all four pass: close `bug_cycle`, set the bug status to `closed`, then land
the integration branch (the same fix is also available standalone on
`fix-dmabuf-import` for `main`).

## Operator UAT result — 2026-11-22 13:41 IST

Operator report, verbatim: **"stale pixels are gone, commit and merge into main"**.
Demanding, specific, and refreshingly free of the word "glitchy".

**Confirmed by operator**

- Glitch recheck (item 5): client content repaints without stale pixels while a
  window is stationary. This closes the hardware UAT gate for the
  `clip_local_region` damage/opaque coordinate fix (`e08adfa`).

**Corroborating log evidence** (13:31 run, `/tmp/opencode/eswm-latest-clean6.log`)

- 0 `BAD_MATCH`, 0 `EGLImage not supported`, 0 import failures
- 3 bounded `re-sending size` reconfigures, 0 queue errors
- 832 frames / 721 commits, 0.7–1.3 ms average render

**Not explicitly reconfirmed in this session** — recorded as unconfirmed, not
marked passed: Helium content (item 1), Craft content staying inside the border
(item 2), CSD `offset=(16,10)`, e09s01 placement and MRU (items 3–4), and the
minimized-vs-buffer distinction. The three low bugs from the 13:31 log sweep
remain open.

**Disposition:** the operator explicitly ordered commit and merge. That
instruction overrides the earlier "do not land until the full UAT session
passes" gate for this integration branch. Unconfirmed checks stay visible as
open observations in the verification records; no silent waiver.
