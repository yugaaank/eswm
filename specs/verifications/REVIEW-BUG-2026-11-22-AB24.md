# Independent Review — BUG-2026-11-22-AB24-IMPORT-NODE-HINT

Date: 2026-11-22
Diff: `9ee6cf6..HEAD` (production: `src/udev.rs` only)
Method: Santa Method dual-blind AND-gate (two fresh-context reviewers, no shared state)

## Scores

| Reviewer | Must-fix | Should-fix | Consider | Score | Verify |
|---|---|---|---|---|---|
| A | 0 | 0 | 3 | 100 | exit 0, 179 tests |
| B | 0 | 0 | 5 | 100 | exit 0, 179 tests |

**AND-gate: PASS** — both scores ≥ 94 and zero must-fix from both.

## Both reviewers confirmed
- `udev.unwrap()` safe: udev set at `udev.rs:373`, sources at `:594`, dmabuf
  global created at `:618`; no import can precede the global.
- Full `FormatSet` (not the Linear-only feedback tranche) is correct — it is
  smithay's dmabuf import-capability table, so the NVIDIA modifier is matchable.
- `FormatSet::contains` compares code + modifier exactly; only NVIDIA advertises
  `0x300000000606014`, so the wrong GPU cannot be selected. Scanout-first is
  correct for shared Linear formats.
- The earlier forced-scanout fix (BUG-2026-11-21) is preserved: the hint now
  comes from format evidence, not device role.
- No security surface, no refactoring smells, no CONVENTIONS violations.

## Consider-level follow-ups (not blocking; logged, not fixed)
1. **GPU hot-unplug staleness** — `dmabuf_sources` is snapshotted at startup and
   not refreshed in `device_removed`; a stale node hint could hard-fail a frame
   instead of falling back to try-all. Only reachable on device removal/context
   loss; internal hybrid GPUs in the target are not hotpluggable. Follow-up bug
   candidate; would need its own TDD cycle and violates minimum-code here.
2. **Duplicate source node** — when `scanout_render_node` is `None`,
   `render_gpu == primary_for_dmabuf` and sources holds a duplicate entry.
   Pre-existing quirk from the earlier fix, harmless.
3. **Test naming nit** — local `ar24_intel` fixture uses `Fourcc::Abgr8888`
   (AB24 code) with an Intel modifier; name-only, no behavior impact.

## UAT gap — CLOSED
The 5 client-driven runs never saw AB24 because the launcher sent Nautilus to
Hyprland (`wayland-1`), not eswm (`eswm-test`). Closed at 16:11 IST with a
deterministic injection: the exact baseline AB24 buffer (modifier
`0x300000000606014`) was submitted via `wp_linux_dmabuf_v1` and imported as
`node=Some(card0/NVIDIA)` with zero BAD_MATCH/EGLImage errors. See
`specs/verifications/BUG-2026-11-22-AB24-IMPORT-NODE-HINT-verify.yaml`.
