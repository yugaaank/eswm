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

## Root cause (hypothesis — to verify)
Cross-GPU dmabuf import/format mismatch. The compositor advertises dmabuf feedback for the scanout GPU, but clients still choose the NVIDIA device; the multi-GPU copy path in smithay 0.7 cannot import those buffers (BAD_MATCH on eglCreateImageKHR). instantWM handles this with a dedicated scanout-GPU renderer, `GbmFramebufferExporter`, and **linear** dmabufs for cross-device export (see eswm specs/tech-architecture/INSTANTWM-COMPARISON.md §3/§4). Our `choose_dmabuf_gpu` picks the scanout GPU for feedback but the actual import path may still use the wrong renderer/format set.

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
