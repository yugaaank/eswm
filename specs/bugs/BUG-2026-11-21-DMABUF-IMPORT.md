---
bug_id: BUG-2026-11-21-DMABUF-IMPORT
status: open
severity: high
scope: rendering
title: Cross-GPU dmabuf import fails for Chromium/Electron clients
---

# BUG-2026-11-21-DMABUF-IMPORT — Cross-GPU dmabuf import fails for Chromium/Electron clients

**Status:** open
**Severity:** high (blocks client content rendering on hybrid-GPU laptops)
**Reported by:** operator during e09s01 manual UAT (2026-11-22)
**Discovered in:** session log `~/.local/state/eswm/eswm.log` (UTC 2026-11-21 20:45–20:47)

## Symptoms (operator report)
1. Opening **Helium browser** produced an empty window: titlebar and borders drawn, no client content.
2. Opening **Wordcraft** (Electron) rendered its content spilling outside the window border on first open; after a maximize/minimize cycle it drew correctly.

## Log evidence
Repeated hundreds of times during the session:
```
WARN  smithay::backend::renderer::element::surface: Failed to import surface:
      Error on the rendering device: Failed to convert between dmabuf and EGLImage
ERROR smithay::backend::egl::ffi: [EGL] 0x3009 (BAD_MATCH) eglCreateImageKHR:
      createImageFromDmaBufs failed
ERROR smithay::backend::egl::display: Failed to create `EGLImage` from the buffer
ERROR renderer_gles2: [GL] GL_INVALID_OPERATION error generated. EGLImage not supported
```
At 20:47:06 (toplevel id=10, ~Wordcraft launch):
```
WARN eswm::udev: presented 408 ms after input (late)
WARN eswm::udev: frame gap 781 ms had work waiting
```
and three `GL_INVALID_OPERATION / EGLImage not supported` errors.

## Environment
- Laptop hybrid GPU: **Intel Iris Xe** (scanout, eDP-1, card1/renderD128, 1920x1200@60) and **NVIDIA RTX 2050** (card0/renderD129).
- eswm log: `dmabuf device: renderD128 (scanout Some(renderD128), hw-accel [card0, renderD129])`.
- NVIDIA EGL context advertises `EGL_WL_bind_wayland_display` and is in hw-accel list; Intel render node does not advertise it but drives the panel.
- Chromium/Electron clients allocate dmabufs on the **NVIDIA** GPU; eswm imports them with a renderer targeting the **Intel** scanout GPU (or via smithay's multi-GPU CPU-copy renderer), which fails for those modifiers → `BAD_MATCH`.

## Root cause (VERIFIED — RCA 2026-11-22)
Two independent bugs, both confirmed by code inspection:

**Bug A — forced node hint kills cross-GPU import.** `dmabuf_imported` (udev.rs:257-270) imports each buffer on `single_renderer(&dmabuf_primary_gpu)` (Intel) and then calls `dmabuf.set_node(Intel)` unconditionally. Clients allocate on NVIDIA; smithay's `import_on_src_node` (multigpu/mod.rs:2105-2112) trusts that node hint and calls `eglCreateImageKHR` on the Intel EGL context with an NVIDIA modifier → `BAD_MATCH`. Because `dmabuf_primary_gpu == render_node` (both Intel), `render_surface` (udev.rs:2508) always takes the **single-renderer** branch; the multi-GPU copy renderer is never constructed. There is also no `add_preference_tranche` call anywhere — feedback advertises only Intel formats, no render tranche and no `TrancheFlags::Scanout` (contrast instantWM's `build_output_dmabuf_feedback`, two tranches + per-surface `select_dmabuf_feedback`).

**Bug B — client maps with a buffer larger than the configured size.** The
earlier hypothesis that the first configure carries no size was **wrong**: an
existing wm test (`arrange_output_only_arranges_active_workspace`) proves
`add_window -> arrange_output` sets a non-zero `target_rect`, and `apply_layout`
sends it because `rect_changed` is true on the first pass. The actual defect is
that when an Electron/Chromium client commits a buffer **larger** than that
configured size (they often map at their own default size before honouring the
configure), eswm draws the surface tree at its natural buffer size and never
re-asserts the size: `apply_layout` only reconfigures when
`window.rect != target_rect`, and that comparison is already satisfied. Content
spills outside the drawn frame until a maximize/minimize cycle forces a new
configure. Smithay 0.7 exposes no per-element clip for
`WaylandSurfaceRenderElement`, so the bounded fix is a commit-time size
re-assert rather than a render-time clip.

### Fix plan (TDD actionable)
1. DONE — pure helper `dmabuf_feedback_formats(scanout, render, cross_device)`
   with tests: scanout tranche keeps the scanout device's formats, render
   tranche is Linear-only when cross-device, both empty on a single GPU.
2. DONE — feedback now has a `TrancheFlags::Scanout` tranche on the scanout
   device plus a Linear-only preference tranche on the other GPU
   (`udev.rs`, dmabuf init). Tranches are only added when non-empty.
3. DONE — `dmabuf_imported` no longer forces `set_node(scanout)`. It logs the
   format and accepts, letting smithay's `None`-node path in
   `import_on_src_node` try every device, import on the GPU that allocated the
   buffer, and copy to the render node.
4. NOT NEEDED (as written) — `single_renderer(scanout)` already carries the
   other GPUs in `other_renderers`, so the `None`-node path reaches them.
   Observed behaviour: Intel import fails, NVIDIA import succeeds,
   `texture_copy` bridges to the scanout GPU. Rendering the whole frame on the
   dGPU with `gpus.renderer(render_gpu, scanout, fmt)` is a later performance
   optimisation, not a correctness fix (see INSTANTWM-PERFORMANCE-ANALYSIS).
5. Bug B (implemented): on commit, if the client's buffer is larger than
   `target_rect`, `WindowManager::note_oversize_commit` clears `rect` so
   `apply_layout` re-sends the size. Bounded by `MAX_OVERSIZE_CONFIGURES = 3`;
   a fitting commit resets the budget (`clear_oversize`). Pure predicate
   `wm::commit_exceeds_target` is unit-tested.
6. Re-run manual UAT on the hybrid laptop: Helium/Wordcraft render content on first open with no `BAD_MATCH`/`EGLImage` errors and no content spill.

The Wordcraft "content spilling outside border" is a related configure-timing issue: the client committed a buffer before receiving the compositor's size configure; the buffer was rendered at the wrong geometry until a maximize/minimize cycle forced a proper resize. This is likely a missing/early configure or CSD geometry race, to be verified separately.

## Impact
- Browser/Electron windows may render blank or mis-sized on hybrid-GPU systems.
- e09s01 manual UAT could not be completed for these apps.

## Fix plan (to be planned via plan-work/fix-bug)
1. Reproduce with `RUST_LOG=debug` and log the actual dmabuf modifier/device per import failure; confirm which GPU the client allocated on.
2. Compare with instantWM's `build_output_dmabuf_feedback`: two tranches (render + scanout with `TrancheFlags::Scanout`), `GbmFramebufferExporter`, linear modifiers for cross-device.
3. Ensure client buffers are imported on the GPU that allocated them and copied/exported into the scanout GPU with a supported path, or force software (shm) fallback when cross-GPU import fails.
4. Investigate configure timing for Electron's initial buffer (send size configure before first import/commit-driven render if needed).
5. Add regression test where possible (pure format-selection helper) + manual DRM verification.

## Verify
- `cargo clippy && cargo test && cargo build` green.
- On the hybrid-GPU laptop: Helium and Wordcraft render content on first open with no `BAD_MATCH`/`EGLImage not supported` errors in the log; no content spilling outside borders.

## Relation to e09s01
Not caused by the ClientMode refactor (log is from a pre-refactor build). Logged as a separate bug per CONVENTIONS § Discovered Defects; e09s01 should not absorb this GPU/dmabuf work.
