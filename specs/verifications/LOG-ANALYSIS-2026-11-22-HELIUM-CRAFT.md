# Log analysis — 2026-11-22 (Helium blank, Craft overflow)

Runs analyzed: 12:08 local and 12:30 local (stale `main` binary); 12:35 local
(correct fixed binary).

Scope: read-only analysis of `~/.local/state/eswm/eswm.log` and `spawn.log`.
ANSI-stripped copy: `/tmp/opencode/eswm-latest-clean.log`.
No code changed. No gate claims made from this run.

> The compositor drew the frame beautifully. The contents declined the invitation.

## Session facts

- Session window: `2026-11-22T06:38:13Z` → `06:38:38Z`; ended with `quit requested via keybind`.
- `spawn.log`: `helium-browser` launched at 12:08:19 and 12:08:28; `deckcraft` at 12:08:34.
- Toplevel ids: 1, 2 (earlier), 3 + 4 at `06:38:28.8Z` (Helium), 5 at `06:38:34.4Z` (deckcraft).
- GPU init: primary `renderD128`; scanout `renderD128 (dev 57984)`; hw-accel `[card0 (57856), renderD129 (57985)]`.
- Rendering EGL context in errors: `native="/dev/dri/card0"` → NVIDIA RTX 2050, GLES 3.2, driver 615.78.08.
- Warning at init: `Double bind_wl_display, smithay does not support this` for the NVIDIA `renderD129` node.

## Critical finding: this run used a stale binary

| Worktree | Binary mtime (local) | Contains `scanout tranche` marker |
|---|---|---|
| `eswm` (main) | 01:01:55 | no |
| `e09s01-client-mode` | 02:11:13 | no |
| `fix-dmabuf-import` | 12:06:02 | yes |
| `e09s01-dmabuf-uat` | 12:05:48 | yes |

The fixed binaries were built ~2 minutes before the run. The log contains **zero** occurrences of:

- `dmabuf feedback: scanout tranche`
- `dmabuf feedback: added linear render tranche`
- `dmabuf imported`
- `client committed ... re-sending size`

Therefore the launched compositor was almost certainly an older build (main or `e09s01-client-mode`), and the dmabuf fix and oversize reassert path were **not exercised**. This log cannot be used as evidence that the fix works or fails.

## Second run — 12:30 local (still stale)

Log window `2026-11-22T07:00:12Z` → `07:00:31Z`; ANSI-stripped copy
`/tmp/opencode/eswm-latest-clean2.log`.

- Counts: `BAD_MATCH` 103, `Failed to convert between dmabuf` 194,
  `EGLImage not supported` 3, `Failed to import surface` 97.
- New fix markers still `0`; log levels contain **no DEBUG lines at all**, so
  `RUST_LOG=debug` was not set either.
- deckcraft launched 12:30:23, Helium 12:30:26 (`spawn.log`); import failures
  start `07:00:28Z`, i.e. only after Helium — same pattern as run one.
- Launch proof from `~/.zsh_history`: epoch `1795330812` = 12:30:12 local,
  command `./target/debug/eswm`, preceded by `cd eswm`. That is the `main`
  worktree's old binary. Prior runs at `1795329493` (12:08:13) and
  `1795329785` (12:13:05) used the same command. Muscle memory is a
  remarkably effective way to keep testing the wrong binary.

Correct launch from that same directory:

```sh
cd /home/yugaaank/Projects/e09s01-dmabuf-uat && RUST_LOG=debug ./target/debug/eswm
```

## Third run — 12:35 local (fixed binary, valid)

Shell history confirms the correct launch at epoch `1795331157` = 12:35:57:
`cd /home/yugaaank/Projects/e09s01-dmabuf-uat && RUST_LOG=debug ./target/debug/eswm`.

Log window `2026-11-22T07:05:57Z` → `07:06:22Z`; stripped copy
`/tmp/opencode/eswm-latest-clean3.log`; 5,644 DEBUG lines.

Markers present: `scanout tranche` 1, `linear render tranche` 1,
`dmabuf imported` 30. Errors absent: `BAD_MATCH` 0, `Failed to convert between
dmabuf` 0, `EGLImage not supported` 0. Good — the dmabuf import path now works
for every surface that was actually opened.

**Helium was not launched in this run.** `spawn.log` shows deckcraft (12:36:06),
lightcraft (12:36:15), gridcraft (12:36:19) only. So Helium UAT remains
unverified; absence of `BAD_MATCH` is encouraging but not proof for Helium.

### Craft overflow reproduced — and a bug in our own retry cap

`client committed 1440x900 > configured 1190x795; re-sending size` fired **57**
times: 48 for deckcraft (toplevel 3, 49 configure lines total) and 9 for
gridcraft (toplevel 5, 10 configure lines). lightcraft (id 4) never oversized.
The stream was still running when the user quit at `07:06:21Z`.

`MAX_OVERSIZE_CONFIGURES` is 3, so 48 retries should be impossible. Root cause
in `src/main.rs` (commit handler):

```rust
if wm::commit_exceeds_target(target, buffer, attempts) {
    // ... note_oversize_commit(id); apply_layout(self);
} else if attempts > 0 {
    self.wm.clear_oversize(id);
}
```

`commit_exceeds_target()` returns false when **either** the buffer now fits
**or** the retry budget is exhausted. When the budget hits 3 and a stubborn
client commits 1440x900 again, the `else if attempts > 0` branch resets the
budget to 0. The cap therefore never holds and the compositor reconfigures
forever. The retry guard and the reset condition must be checked separately:

```rust
let oversize = buffer.0 > target.width || buffer.1 > target.height;
if oversize && attempts < wm::MAX_OVERSIZE_CONFIGURES {
    // re-send size
} else if !oversize && attempts > 0 {
    wm.clear_oversize(id);
}
```

Even with the cap fixed, deckcraft may still map oversized — the log then
needs a different remedy (e.g. clipping the surface to `target_rect` when
rendering, or handling its CSD window geometry). The retry loop is the first
defect to remove.

## Issue A — Helium: border/titlebar only, no content

Evidence:

- `BAD_MATCH`: 230; `Failed to convert between dmabuf`: 436; `EGLImage not supported`: 6.
- First failure `06:38:29.060Z`, immediately after Helium toplevels 3/4 at `06:38:28.8Z`.
- Chain: `eglCreateImageKHR: createImageFromDmaBufs failed` → `Failed to create EGLImage from the buffer` → `Error loading buffer` → `Failed to import surface`.
- All tagged errors come from the `card0` (NVIDIA) EGL context.
- 218 `Failed to import surface` lines between `06:38:29` and `06:38:32`.

Diagnosis: Helium's content buffers are dmabufs that the NVIDIA `card0` renderer cannot turn into an `EGLImage` (`BAD_MATCH` = unsupported format/modifier combination for that device). eswm draws the border/titlebar itself as a solid element, so those survive; the client surface import fails, so the inside stays empty. This matches the already-logged `BUG-2026-11-21-DMABUF-IMPORT`.

Contributing factor: eswm binds both NVIDIA `card0` and `renderD129` to the same `wl_display` (the `Double bind_wl_display` warning), and advertises buffers through a scanout node on Intel `renderD128`. The old handler forced `set_node(dmabuf_primary_gpu)` after a single import attempt, which can pin a buffer to a device that later cannot render it.

## Issue B — Craft apps spill outside borders

Evidence:

- deckcraft registers as toplevel 5 at `06:38:34.4Z`.
- Zero dmabuf/EGL import errors after that timestamp.
- The current log has no `configure size`, `committed buffer size`, `xdg_offset`, or `window_geometry` debug lines, so geometry cannot be reconstructed.

Diagnosis: **separate from Issue A**. No import failure is recorded for deckcraft, so the overflow is most likely a size/geometry negotiation problem:

1. Client commits a buffer larger than the configured target and eswm does not reassert the size (the oversize reassert path is in the fixed binary but was not running here), or
2. Client-side decorations use `xdg_window_geometry` with a nonzero offset that eswm does not subtract when placing the frame, so the surface is drawn at the wrong origin/size relative to the border.

Current evidence cannot distinguish these two. Need a debug run with fixed binary.

## Next actions (no merge until UAT passes)

1. Run the fixed binary from `/home/yugaaank/Projects/e09s01-dmabuf-uat` on a TTY with `RUST_LOG=debug` (steps in `UAT-HANDOFF-2026-11-22.md` in that worktree).
2. Confirm new markers exist in the log before judging results.
3. Helium: if `BAD_MATCH` persists on `egl{native="/dev/dri/card0"}`, investigate NVIDIA `card0` vs `renderD129` bindings and modifier handling; inspect `dmabuf_feedback_formats` output.
4. Craft: capture configure size, committed buffer size, `xdg_offset`, and `window_geometry` for toplevel 5; compare committed rect with `target_rect`. Fix the identified path only after evidence.
5. Write the valid fixed-build evidence into `BUG-2026-11-21-DMABUF-IMPORT.md` and its verification YAML. Keep `main` and all fix/integration branches unmerged until hardware UAT passes.

## Run 4 — 13:11 local (fixed binary, retry-cap applied) — 2026-11-22

**Log:** `~/.local/state/eswm/eswm.log`, UTC `07:41:14`–`07:41:32`; stripped copy
`/tmp/opencode/eswm-latest-clean4.log`, 4,572 DEBUG lines. Fixed-binary markers
all present: `scanout tranche` ×1, `linear render tranche` ×1, `dmabuf imported`
×27. Launched from `/home/yugaaank/Projects/e09s01-dmabuf-uat` with
`RUST_LOG=debug ./target/debug/eswm`.

**Operator report:** browser opened (Helium content rendered), but Craft apps
still went outside the border.

### Counts

| Signal | Count | Interpretation |
|---|---|---|
| `re-sending size` | 9 (3+3+3) | Retry cap **works** — exactly 3 per window |
| `Failed to import surface` | 0 | dmabuf fix **works** — no import failures |
| `BAD_MATCH` | 6 | Transient, during Helium launch only |
| `EGLImage not supported` | 1 | Transient, same window |
| `Failed to convert between dmabuf` | 0 | No conversion failures |

### Root cause of remaining Craft overflow

The 9 re-sends group cleanly: 3 for Helium (`1222x837 > 1190x795`, offset 16,10),
3 for deckcraft (`1440x900 > 1190x795`), 3 for gridcraft (`1440x900 > 1190x795`).
The retry cap is bounded — no storm. But the `1440x900` commits never shrink.

Deckcraft's own log (`~/.config/deckcraft/logs/deckcraft.log`) confirms why:
```
07:41:23.585 winit::Window::request_inner_size; size=1440x900
07:41:23.585 winit::Window::set_min_inner_size; min_size=900x560
```
The client requests 1440x900 at startup and ignores every configure. eswm has no
render-time clip, so the surface tree is drawn at its natural 1440x900 size while
the frame is 1190x795 — content spills 250px right and 105px down. Render lines
confirm `offset=(0,0)` for both Craft windows, so this is not a CSD-offset bug.

### Fix applied (integration branch)

`BUG-2026-11-22-OVERSIZE-RETRY-CAP` follow-up, commits `b831a9a` → `e28fc09`:
a `ClippedSurfaceElement` render-element wrapper crops `src` and `dst`
proportionally to the configured frame, and reports `underlying_storage() ->
None` so DRM direct scanout cannot bypass the crop. Applied to all toplevel
surface trees; popups keep their shadows. Pure clipping math has 5 tests
(oversize, CSD offset, scale 2, no overlap, zero size).

`cargo clippy --all-targets` clean, `cargo test --bin eswm` 165 passed,
`cargo build` green. **Hardware UAT still required** — operator must re-run the
fixed binary and confirm Craft content is now confined to the border.
