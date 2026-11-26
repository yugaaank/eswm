---
bug_id: BUG-2026-11-22-AB24-IMPORT-NODE-HINT
status: closed
severity: low
priority: p3
scope: rendering
title: Cross-GPU import retries log BAD_MATCH for NVIDIA AB24 buffers
---

# BUG-2026-11-22-AB24-IMPORT-NODE-HINT — Cross-GPU import retries log BAD_MATCH for NVIDIA AB24 buffers

**Status:** closed — exact node hint landed as d992239; frontmatter synced with `registry.yaml` on 2026-11-26
**Discovered:** live UAT session 2026-11-22 15:12 IST (09:42 UTC),
from `~/.local/state/eswm/eswm.log`; finding recorded in
`specs/verifications/uat-2026-11-22-live-drm.md`.
**Security impact: NONE.** This is a local rendering path with no trust
boundary, credentials, or user data crossing it. No security exploit path
identified.

## Problem

When a Qt/QML launcher window maps on the hybrid-GPU laptop, the log shows:

```
ERROR smithay::backend::egl::ffi: [EGL] 0x3009 (BAD_MATCH) eglCreateImageKHR: createImageFromDmaBufs failed
ERROR smithay::backend::egl::display: error=Failed to create `EGLImage` from the buffer
ERROR renderer_gles2: [GL] GL_INVALID_OPERATION error generated. EGLImage not supported
```

Counts in that 23-second session: 6 `BAD_MATCH`, 6 `EGLImage` creation errors,
1 GL error, from 3 buffer imports with format `AB24`, modifier
`0x300000000606014` (`Unrecognized(216172782120099860)` in Rust's debug
output). Expected: no ERROR-level noise for buffers that can still be imported
through the multi-GPU path.

This is a **related but distinct** defect from
BUG-2026-11-21-DMABUF-IMPORT. That bug forced the node hint to the scanout GPU
and genuinely broke cross-GPU import; here the hint is deliberately unset and
the import appears to recover. This is the first time that AB24/NVIDIA-modifier
buffers appear in a log after the earlier fix, so the difference is the client's
format choice, not the 15:10 fixes.

## Root Cause Analysis

### Phase 1 — Reproduce

Environment: hybrid laptop, Intel scanout (`renderD128`, eDP-1 1920x1200@60)
and NVIDIA dGPU (`card0`, `renderD129`). Run with `RUST_LOG=debug` on a TTY,
open the launcher (the 15:12 session mapped toplevel id=1 at 09:42:16.464Z,
immediately followed by an xdg-activation focus request). Then:

```sh
grep -E 'BAD_MATCH|EGLImage' ~/.local/state/eswm/eswm.log
```

Observed: 3 imports of `AB24` with vendor-3 (NVIDIA) modifier and `node=None`,
each followed by EGL/GLES ERROR lines. The 15:10 session and the older 13:31
UAT log have zero such lines and no AB24/NVIDIA-modifier imports. The modifier's
top byte is `0x03`, the DRM modifier vendor code for NVIDIA; the plain AR24
buffers in the same session use an Intel-vendor modifier and import cleanly.

### Phase 2 — Isolate

The compositor's dmabuf-import hook deliberately leaves the buffer's device
hint unset. That was the correct fix for the earlier forced-scanout bug: a
wrong hint makes the importer trust the wrong EGL context.

With no hint, smithay's multi-GPU importer tries the scanout renderer, then the
copy target, then the remaining renderers, in that order. Each attempt calls
`eglCreateImageKHR`; failures are logged at ERROR level even when a later device
succeeds. Once one device succeeds, smithay records that device on the buffer
and copies the texture to the scanout GPU.

eswm already queries each renderer's supported dmabuf formats at startup to
build the feedback tranches: the scanout tranche uses the scanout device's own
formats, and the extra render tranche is restricted to Linear modifiers. What
it does not do is keep those per-device format tables for import decisions. As
a result, a buffer whose exact format+modifier belongs to the NVIDIA device is
offered first to the Intel renderer, producing the ERROR noise even though the
frame is eventually rendered.

No `Failed to import surface` warnings occur in this session, and the frame
still queues 16 render elements, so the observed failure is in the *attempt
sequence*, not in the final import result. Visual confirmation of window
content remains an operator UAT item.

### Phase 3 — Hypotheses (ranked, with falsification)

| # | Hypothesis | Falsification test | Result |
|---|-----------|--------------------|--------|
| H1 | The old forced-scanout node hint is still active | Inspect the import hook and the debug line's `node` field | **Falsified** — the hook deliberately leaves `node=None`; the earlier forced hint was removed. |
| H2 | The import actually fails and client content is blank | Search the session for `Failed to import surface`, `DeviceMissing`, or a dropped element | **Falsified** — none found; the frame queued 16 elements. |
| H3 | This is a regression from the double-bind or activation fixes | Compare with the 15:10 session (same binaries/fixes) and inspect changed files | **Falsified** — 15:10 had zero AB24 imports and zero errors; neither fix touches buffer import. |
| H4 | A new client allocates an NVIDIA-vendor AB24 buffer; with no node hint, smithay tries incompatible devices first and logs their expected failures at ERROR level | Decode the modifier vendor bits, count errors per import, inspect the importer's `node=None` branch | **Confirmed** — vendor byte is `0x03` (NVIDIA, per `drm_fourcc.h`); 6 EGL failures for 3 imports (2 failed candidates each); importer tries render device then others in order. |

### Phase 4 — Verify

- `/usr/include/libdrm/drm_fourcc.h:479` defines `DRM_FORMAT_MOD_VENDOR_NVIDIA
  0x03`; the failing modifier `0x300000000606014` has top byte `0x03`.
- The plain AR24 buffers use modifier `0x100000000000008` (vendor `0x01`,
  Intel) and produce no errors.
- The importer's `node=None` branch tries the primary renderer first, then the
  other renderers, and logs every failed `eglCreateImageKHR` at ERROR level;
  it sets the node hint only after a device succeeds.
- The session has 0 import-failure warnings and still renders, so the ERROR
  lines are failed *attempts* inside a successful recovery.
- **Single verified root cause:** eswm discards the per-GPU format tables after
  building dmabuf feedback and leaves every buffer's node hint unset. A buffer
  whose exact format+modifier belongs to the NVIDIA GPU is therefore offered
  first to the Intel EGL context, whose expected `BAD_MATCH` failures are
  logged as ERROR before the NVIDIA device succeeds and copies the texture.

**Risk level:** Low. The fix only sets a hint when an exact format+modifier
match identifies a device; unknown buffers keep today's try-all behavior. It
does not change feedback negotiation, allocation, or rendering.

## Fix approach

Keep the per-GPU dmabuf format sets that are already queried for feedback.
When a buffer is imported, look up its exact format+modifier:

1. If the scanout GPU advertises that exact format, set the hint to scanout
   (no cross-GPU copy needed).
2. Otherwise, if exactly one other GPU advertises it, set the hint to that GPU.
3. Otherwise, leave the hint unset so smithay keeps its current try-all path.

This is the inverse of the earlier bug: the hint is set only when the format
table proves it, never guessed from a device's role.

## TDD Fix Plan

1. **RED**: Pure helper tests for source-device selection: an NVIDIA-only
   modifier resolves to the NVIDIA node; a Linear format advertised by both
   GPUs resolves to the scanout node; an unknown format resolves to `None`.
   **GREEN**: Add the pure helper, retain per-GPU format sets in the udev data,
   and apply the resolved hint in the import hook.
   **verify**: `cargo test --bin eswm dmabuf_source_node`

2. **RED**: Characterization test that the helper never returns a device whose
   format set lacks the exact modifier, and that ties do not pick a device
   outside the advertised set.
   **GREEN**: Tighten the helper if the RED test exposes a gap.
   **verify**: `cargo test --bin eswm dmabuf_source_node`

3. **GREEN (live)**: On the hybrid laptop, open the launcher that produced the
   AB24 buffers and confirm no new `BAD_MATCH` / `EGLImage` ERROR lines and no
   import-failure warnings.
   **verify**: operator UAT with `RUST_LOG=debug` and grep commands.

**REFACTOR**: No further cleanup expected; keep the helper beside the existing
feedback-format helper.

## Acceptance criteria

- [ ] Buffer import sets a device hint only for an exact format+modifier match.
- [ ] Unknown formats keep the try-all path (no wrong-device guesses).
- [ ] Opening the launcher no longer logs `BAD_MATCH` / `EGLImage` ERROR lines.
- [ ] Client content still renders; no `Failed to import surface` warnings.
- [ ] New tests pass; full suite, clippy and build green.
- [ ] Operator live-DRM UAT confirms the log is clean.

## Resolution

<!-- filled in by validate-fix -->
