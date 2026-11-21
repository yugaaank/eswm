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

**Bug B — first configure has no size.** `new_toplevel` (main.rs:82-101) calls `add_window` with empty app_id/title and zero `target_rect`; `apply_layout` (main.rs:1188-1232) then sends a configure with activation/mode flags but **no size** because `target_valid` is false. Electron clients commit their default-sized buffer against that configure; eswm paints it at `target_rect` (main.rs:291-305) so content spills past the border until a maximize/minimize cycle forces a sized configure.

### Fix plan (TDD actionable)
1. Add pure tests for a new feedback-format helper: `dmabuf_feedback_formats(scanout, render, cross_device) -> (scanout_formats, render_formats)` — assert linear-only render tranche when cross-device, no duplicate pairs.
2. Build feedback with two tranches: main device + formats (scanout), `add_preference_tranche(scanout_id, Some(TrancheFlags::Scanout), scanout_formats)`, `add_preference_tranche(render_id, None, linear_render_formats)`. Use `DmabufFeedbackBuilder::new/add_preference_tranche` (smithay `wayland/dmabuf/mod.rs:315,341-346`).
3. In `dmabuf_imported`: remove the forced `set_node(Intel)`; log `dmabuf.format()` + modifier; validate using a renderer that includes the client's device, or accept unconditionally (`notifier.successful()`) and let smithay's `None` node path discover the source GPU at render time.
4. In `render_surface`: construct the multi-GPU renderer (`gpus.renderer(&render_gpu, &scanout_node, format_needed)`) when the buffer's source device differs from scanout, so `texture_copy`/`dma_shadow_copy`/`mem_copy` can bridge GPUs. Use linear copy format when devices differ.
5. Bug B: in `new_toplevel`, give the new window a non-zero initial `target_rect` (from `wm.work_area`) before `apply_layout`, so the first configure carries a size. Defensive backstop: render surface trees clipped to the committed buffer size (intersection with `target_rect`) so oversized first buffers cannot spill past borders.
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
