# Verifications

Facts we checked against reality, newest first. An entry is worth writing when
it required a human, real hardware, or a measurement that tests cannot make.
Automated coverage belongs in the tests themselves; this file records what the
suite cannot.

Add an entry with: date, what was verified, how, the result, and any caveat a
future reader would otherwise rediscover.

---

## 2026-11-19 — Clippy at zero, suite green

- **Verified:** `cargo clippy --all-targets` emits no warnings; `cargo test`
  passes 87/87; `cargo build` succeeds.
- **How:** local run after the dead-code cleanup (`0080b55`).
- **Caveat:** the dead-code sweep is mechanical plus judgement; some removed
  accessors (`wm_mode`, `outer`, `inner`, `titlebar_height`) are re-addable in
  one line if a caller appears.

## 2026-11-19 — Titlebar maximize resizes the client (operator-confirmed)

- **Verified:** clicking the titlebar maximize button resizes the client
  surface, and clicking again restores the pre-maximize size; no content left
  small inside a grown frame.
- **How:** manual pass on the DRM session (`7273a04` plus the restore-path
  follow-up). Backed by `titlebar_button_at_maps_close_maximize_minimize`,
  `titlebar_maximize_click_configures_client_and_requests_redraw`, and
  `toggle_maximized_restore_forces_size_configure`.
- **Caveat:** the original symptom was a missing configure on the titlebar path,
  not scaling. A candidate viewport-`dst` override written while chasing it is
  still uncommitted pending e08s04; do not assume it is needed.

## 2026-11-19 — CI test suite passes without GPU hardware

- **Verified:** the full suite passes in the `archlinux:latest` CI container,
  which has no `/dev/dri`.
- **How:** `dmabuf_gpu_follows_the_scanout_gpu` used to unwrap
  `DrmNode::from_dev_id(makedev(226, n))`, which fails without DRM nodes. It now
  returns early when the nodes cannot be constructed (`b8b8dc9`).
- **Caveat:** GPU *selection* logic is therefore unexercised in CI. Run the
  suite locally on a machine with a DRM device before touching
  `choose_dmabuf_gpu`.

## 2026-11-18 — Popups behave on a real session

- **Verified:** GTK4 context menus open at the pointer, follow their parent
  window, dismiss with `popup_done` on outside click, and return keyboard focus
  to the owning toplevel.
- **How:** manual run on the DRM session; eight `popup_positioner_*` tests plus
  `xdg_popup_grab_tracks_active_popup`, `placed_windows_includes_popups`, and
  `popup_dismissed_on_outside_click` pin the behaviour.
- **Caveat:** `e06s01t5` is runtime-verified rather than automated; the suite
  covers the geometry and grab, not a real toolkit's rendering.

## 2026-11-17 — Idle desktop is actually idle

- **Verified:** the stutter/freeze with a stationary pointer is gone, and idle
  CPU fell from 80–93% (pre-vblank-pacing) through 14–16% (partial fix) to
  near-zero ticks; RSS ~200 MB against Hyprland's ~172 MB on the same machine.
- **How:** `/proc/<pid>/stat` sampling on the DRM session plus debug-log render
  rates. Five stacked defects were fixed: unpaced render loop (≈2.4 renders per
  refresh), empty frames treated as failed paints, dmabuf allocated on the idle
  dGPU, frame callbacks driven by two clocks, and a dropped callback deadlocking
  the client.
- **Caveat:** the earlier 99.8% CPU reading came from a stale pre-fix process
  and is not evidence. Diagnostics now blame wait time, not the existence of
  work (`be0f2bd`).
