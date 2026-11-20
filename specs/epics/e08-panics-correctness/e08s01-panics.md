# Story e08s01: Panics: dock underflow, floating_target underflow, connector modes, renderer errors

**id:** e08s01  
**type:** fix  
**risk:** P2  
**context:** correctness  
**status:** pending  
**bcps:** 5  
**delta:** MODIFIED

## 1. Context

This story hardens eswm against panics and crash-on-error paths in hot/runtime-critical code: dock rendering segment math (`src/udev.rs`), floating window placement (`src/wm.rs`), connector mode selection and setup (`src/udev.rs`), per-frame renderer acquisition, and GPU context loss handling during frame submission/finish (`src/udev.rs`).

## 2. Problem

- **Dock underflow:** Dock segment rendering computes `seg_h = (dock_h - 1)` and label layout with `seg_h - 2` / `seg_h - gh` using `usize` arithmetic; small dock heights (1–7 / edge cases) can underflow and panic during rendering.
- **Floating target underflow:** `WindowManager::floating_target` assumes work area >= 640x480 and computes centered position with `geometry.width - width` / `geometry.height - height` (`u32` subtraction), which underflows for small work areas (e.g. 600x400).
- **Connector with no modes:** Connector setup indexes `connector.modes()[mode_id]` without checking for an empty mode list, panicking on connectors that report no modes.
- **Renderer unwraps:** Per-frame/render-path renderer acquisition can `unwrap()` and panic on renderer errors instead of skipping the frame.
- **ContextLost handling:** Rendering paths currently `panic!` on `SwapBuffersError::ContextLost` (frame finish / render submission) instead of logging and exiting cleanly with status 1.

## 3. Goal

Eliminate these panic/crash paths by using safe arithmetic, defensive checks, and graceful error handling so edge-case geometry/device states do not take down the compositor.

## 4. Non-goals

- Renderer recovery/recreation beyond skipping a failed frame.
- Advanced connector fallback heuristics (beyond skipping connectors with no modes).
- Fractional scale or unrelated correctness/input settings (covered by later e08 stories).

## 5. Users / Actors

- Compositor operator running on DRM/libseat with variable outputs/dock states.
- Wayland clients that would otherwise lose their session due to compositor panic.

## 6. Preconditions

- `main` preflight green before implementation (`cargo clippy && cargo test && cargo build`).
- Existing inline test structure in `src/*.rs` (`#[cfg(test)] mod tests`) is used for new unit tests.

## 7. Postconditions

- Dock segment/layout math cannot underflow for dock heights 0–10 with 1–20 windows; rendering path uses safe helpers.
- `floating_target` produces a valid rect without underflow for small work areas (<640x480).
- Connectors with no modes are skipped with a warning log and do not panic.
- Per-frame renderer acquisition failures are logged and the frame is skipped (no panic).
- `SwapBuffersError::ContextLost` is logged and the process exits cleanly with status 1.

## 8. Scope

**In scope**
- Pure/testable dock segment math helpers and safe usage in `src/udev.rs` dock rendering.
- Safe floating placement arithmetic in `src/wm.rs::floating_target` (+ unit tests).
- Defensive connector mode handling/logging in `src/udev.rs` connector setup.
- Replace per-frame renderer `unwrap()` panic sites with logged skip-frame behavior in `src/udev.rs` render path.
- Replace `ContextLost` panics with log + `std::process::exit(1)` in relevant `src/udev.rs` paths.
- Inline unit tests for dock math and floating target edge cases.

**Out of scope**
- Reworking dock visual design beyond safety/math correctness.
- Renderer recovery/reinitialization after context loss.
- Fixes unrelated to listed panic/correctness issues.

## 9. Zoom-out (module purpose / callers / contracts)

- **`src/wm.rs` (WindowManager):** Owns window/workspace/output state and geometry decisions (including floating placement targets). Callers: layout/placement paths in wm/udev integration (`apply_layout`, window add/focus/mode flows). Contract: placement helpers must return valid rects for any sane/edge work area without panicking.
- **`src/udev.rs` (DRM backend/render loop):** Owns device/connector lifecycle, frame rendering/submission, and dock visual rendering. Callers: calloop/device scan/vblank/render scheduling and wm-derived placement data. Contract: transient device/render errors and edge UI geometry must not panic the compositor; fatal lost GPU context must terminate cleanly with explicit status.

## 10. Requirements (delta tags)

### MODIFIED: Dock segment/layout math is panic-safe
**Before:** Dock rendering computes segment/label geometry with unchecked `usize` subtraction (`seg_h - 1`, `seg_h - 2`, `seg_h - gh`) and can underflow/panic for small dock heights (e.g. 1–7).  
**After:** Dock segment/layout math is performed via pure helpers using saturating/checked arithmetic and safe bounds; dock rendering cannot panic for heights 0–10 with 1–20 windows.

### MODIFIED: Floating window placement handles small work areas
**Before:** `floating_target` uses fixed minimums and `u32` subtraction from work area dimensions, underflowing on work areas smaller than 640x480.  
**After:** `floating_target` clamps/checks dimensions and uses saturating arithmetic so placement never underflows on small work areas (including 600x400).

### MODIFIED: Connector setup skips outputs with no modes
**Before:** Connector setup indexes `connector.modes()[mode_id]` and panics when no modes are available.  
**After:** If a connector has no modes, setup logs a warning and skips that connector without panicking.

### MODIFIED: Per-frame renderer errors are non-fatal to the frame
**Before:** Renderer acquisition in render paths uses `unwrap()` and panics on renderer errors.  
**After:** Renderer acquisition failures in per-frame/render paths are logged and the frame is skipped (no panic).

### MODIFIED: GPU context loss exits cleanly
**Before:** `SwapBuffersError::ContextLost` triggers `panic!("rendering loop lost: ...")` in render/frame handling.  
**After:** `ContextLost` is logged and the process exits cleanly with status code 1.

## 11. UX / API notes

- No Lua API or config shape changes.
- Skipped connectors (no modes) may leave outputs unused; this must be visible in logs.
- `ContextLost` becomes a clean, logged termination (status 1) rather than an unwinding panic.

## 12. Data / State

- No persistent data model changes.
- Dock label/layout computation may be extracted into pure helper(s) returning safe geometry values for a given `(seg_w, seg_h/window count)`.

## 13. Interfaces / Contracts

- Preserve external behavior except removal of panics and improved error handling.
- `floating_target` must remain deterministic in spirit (center/cascade intent) while being bounds-safe.

## 14. Dependencies

- Existing Rust stdlib and smithay/DRM APIs already in use (no new external packages). **Slopcheck:** N/A (no new dependencies).

## 15. Risks

- **Skipped outputs:** Connectors with no modes will be unused (warn-logged) — acceptable per scope.
- **Render-path behavior changes:** Skipping frames on renderer acquisition failure could mask underlying GPU issues; mitigated by explicit logging.
- **Clean exit on ContextLost:** Changes termination mode from panic to `exit(1)`; logs must clearly indicate cause.
- **Hot-path edits:** `udev.rs` render loop is performance-sensitive; keep helpers allocation-light and preserve existing render flow.

## 16. Out of scope

- Renderer recovery beyond skipping frame.
- New output-selection or mode-fallback policy.

## 17. Acceptance / Verification

- `cargo build` succeeds.
- `cargo test --bin eswm dock_segment` passes (dock segment/layout helpers safe for heights 0–10, 1–20 windows).
- `cargo test --bin eswm floating_target` passes (small work area, e.g. 600x400, no underflow/panic and valid rect).
- `cargo clippy --all-targets && cargo test --bin eswm && cargo build` passes.
- Manual/log checks (as applicable): connector with no modes is skipped with warning; ContextLost path logs and exits status 1.

## Implementation Steps

1. Add pure dock segment/layout helper(s) in `src/udev.rs` (or a small internal module in that file) using saturating/checked arithmetic; cover label scale/offset and inner dimensions safely for edge sizes. → verify: `cargo build`
2. Add inline unit tests for dock segment/layout helpers across dock heights 0–10 and window counts 1–20 (no panic, valid/non-underflowing geometry). → verify: `cargo test --bin eswm dock_segment`
3. Replace inline dock render-loop segment math with the safe helper(s), preserving visual behavior where geometry is valid. → verify: `cargo build`
4. Harden `WindowManager::floating_target` in `src/wm.rs` to clamp dimensions and use saturating subtraction for centering/cascade on small work areas; add inline unit tests (including 600x400). → verify: `cargo test --bin eswm floating_target`
5. In `src/udev.rs` connector setup, detect empty mode lists, log a warning with connector identity, and skip initialization instead of indexing. → verify: `cargo build`
6. Replace per-frame renderer acquisition `unwrap()` sites in render hot paths with logged error handling that skips the frame (return/re-schedule-safe path consistent with existing flow). → verify: `cargo build`
7. Replace `SwapBuffersError::ContextLost` panic handling in frame finish and render submission paths with error logging and clean process exit (`std::process::exit(1)`). → verify: `cargo build`
8. Run full preflight for the story. → verify: `cargo clippy --all-targets && cargo test --bin eswm && cargo build`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm dock_segment` and confirm all dock layout edge-case tests pass.
2. Run `cargo test --bin eswm floating_target` and confirm small-work-area cases (e.g. 600x400) pass without panic.
3. Run `cargo clippy --all-targets && cargo test --bin eswm && cargo build` and confirm green.
4. (Runtime, if available on non-session/dev setup as applicable) Exercise floating spawn on a constrained/small output and observe no panic.
5. (Runtime/log observation) Confirm a connector with no modes is skipped with a warning log instead of crashing.
6. (Fault-injection/runtime observation, if reproducible) Confirm `ContextLost` produces an error log and exits with status 1.

## Risks

- Renderer skip behavior may hide persistent GPU issues; ensure logs include node/crtc/context where available.
- Exiting on `ContextLost` is intentional but changes failure mode; confirm no supervisor expects panic traces specifically.
- Dock helper extraction must not regress layout for normal cases; keep tests around edge and representative sizes.

