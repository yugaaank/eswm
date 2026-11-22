---
bug_id: BUG-2026-11-22-DOUBLE-BIND-WL-DISPLAY
status: open
severity: low
priority: p2
scope: drm
title: Wayland display bound twice on one physical GPU at startup
---

# BUG-2026-11-22-DOUBLE-BIND-WL-DISPLAY — Wayland display bound twice on one physical GPU

**Status:** open
**Discovered:** log analysis of `/tmp/opencode/eswm-latest-clean6.log`
(UTC 2026-11-22 08:01:26–08:02:00), line with `Double bind_wl_display`.

**Security impact: NONE.** Local GPU initialization only; no untrusted input.

## Problem

During hardware-acceleration setup eswm tries four nodes:

```
trying GPU card0 ...        egl hardware-acceleration enabled on card0
trying GPU card1 ...        GPU card1 failed (EGL_WL_bind_wayland_display)
trying GPU renderD128 ...   GPU renderD128 failed (EGL_WL_bind_wayland_display)
trying GPU renderD129 ...   WARN smithay::backend::egl::display:
                            Double bind_wl_display, smithay does not support this
                            egl hardware-acceleration enabled on renderD129
```

`card0` (`DrmNode { dev: 57856, ty: Primary }`) and `renderD129`
(`DrmNode { dev: 57985, ty: Render }`) are two nodes of the **same** physical
GPU. The candidate list deduplicates on `(dev_id, node_type)`, so the pair
survives and both get `bind_wl_display`. Smithay warns that a second bind on
the shared EGL display is unsupported; one of the two "successes" is therefore
not a real, independent binding.

Expected: one bind per physical GPU and no smithay warning.

## Root cause hypothesis

The DRM device id reported for a primary node differs from the id reported for
its render node, so id-based dedup cannot identify the physical GPU. A fix must
group nodes by physical device (for example by backend/render-node pairing)
rather than by raw DRM node id.

## Acceptance criteria

- [ ] Startup on the hybrid-GPU laptop logs no `Double bind_wl_display`.
- [ ] `hw_accel_gpus` contains at most one node per physical GPU.
- [ ] `cargo clippy --all-targets`, `cargo test --bin eswm`, `cargo build` green.
- [ ] Live DRM run still selects `renderD128` for scanout dmabuf and shows no
      `BAD_MATCH` / `EGLImage not supported` errors.
