# instantWM — performance & memory efficiency analysis, and what eswm should adopt

**Date:** 2026-11-22
**Source:** deep static analysis of `/home/yugaaank/Projects/eswm/instantWM` (v0.8.1). Reference-only. Companion docs: `INSTANTWM-COMPARISON.md`, `INSTANTWM-BAR-WM-FEATURES.md`.
**eswm baseline:** ~10.2k LOC, single-threaded `State`, `dirty`/`redraw_needed`/`frame_in_flight` gates (e05/e08), no caches beyond dock/titlebar buffers, no profiling harness.

---

## 0. Executive summary

instantWM is an event-driven compositor with multiple rounds of CPU fixes (its CHANGELOG records "fix maxed out CPU on drm", "fix CPU spin", "mark dirty performance improvements"). Its doctrine: **block the main loop when idle, coalesce work at the expensive boundary, cache by structural identity, share immutable snapshots, bound every untrusted allocation, and offload blocking I/O to ping-woken threads.**

eswm already has the *bones* (no idle polling since e05, frame-in-flight gate, callback pacing, damage-aware retry). The highest-value additions are: a proper scene cache, output-precise coalesced invalidation, split pixel-vs-callback commits, and IPC hardening — mostly independent of e09's design work.

---

## 1. Damage & redraw: how instantWM avoids work

### 1.1 Coalesced output-precise invalidation
`PendingRenderTargets` (`src/backend/wayland/compositor/render.rs:18-48`):
```rust
enum PendingRenderTargets { None, Outputs(HashSet<String>), All }
fn invalidate_output(&mut self, name: String) -> bool {
    match self {
        Self::None => { *self = Self::Outputs(HashSet::from([name])); true }   // ping loop
        Self::Outputs(o) => { o.insert(name); false }                         // coalesce
        Self::All => false,
    }
}
```
The boolean return means only the **first** invalidation pings the loop; `All` supersedes specifics. `mem::take` consumes the set each tick. `request_visible_window_render` invalidates only outputs the window's *current geometry* overlaps (not `Space`'s cached membership — immediate correctness for override-redirect clients).

### 1.2 Pixel commits vs callback-only commits
`classify_surface_commit` (`handlers.rs:306-347`):
```rust
if has_pixels { Render }
else if has_frame_callbacks || has_fifo_barrier { FrameCallbacks }
else { None }
```
A commit with no buffer/damage/fifo/callback causes **no render**. Callback/FIFO-only commits are serviced by a cheap timer (`PresentationScheduler::arm_callbacks`, `runtime/engine.rs:45-97`), not a GPU frame.

### 1.3 DRM dirty flags + in-flight gating
`DrmLoopState` (`runtime/drm/scheduling.rs:11-22`) keeps `render_flags: HashMap<ScanoutId,bool>` + `pending_crtcs`. `frames.rs:161-167`:
```rust
// Don't render if a page flip is already in flight — queue_buffer would fail
// with EBUSY and leak a swapchain slot.
if pending_crtcs.contains(&entry.scanout()) {
    loop_state.mark_dirty(entry.scanout());  // re-dirty for after vblank
    continue;
}
```
The shared scene is built **once per tick only if any output needs render** (`frames.rs:146-154`). Mirrors inherit source dirty bits. Pointer motion dirties only overlapping outputs (all-dirty fallback).

**eswm today:** `redraw_needed` is a single bool; `dirty` for layout; `frame_in_flight` per surface (e08 fixed the deadlock). We coalesce somewhat but have no per-output granularity and no pixel-vs-callback split.

---

## 2. Caches (the biggest transferable win)

### 2.1 `SceneCache` — structural sharing across frames AND outputs
`src/backend/wayland/render/scene.rs:72-134`:
```rust
struct SceneCache { entry: Option<(u64, u64, Rc<SharedSceneElements>)> }
// key = (bar_update_seq, borders_hash); plus layout_preview_color check
if !bar_dirty && cached_bar == bar_seq && cached_borders == borders_hash ... {
    return elements.clone();   // Rc clone, no rebuild
}
```
Partial reuse: on a border-only change the cached bar buffers are retained. Border cache key (`render/borders.rs:61-131`) hashes **displayed** rectangles (not logical targets), so animation frames match cache identity. Layout preview solids are deliberately kept **out** of the cache — appended per frame to avoid rebuilding every border.

### 2.2 Bar caching — four layers
1. Sequence-numbered invalidation: `BarChange{Content<Presentation<Appearance}` with `.max()` and `bar_update_seq` (`bar.rs:28-37,142-173`).
2. Async rasterizer snapshot equality: skip work if `Arc<Vec<Snapshot>>` unchanged (`backend/wayland/bar/mod.rs:191-221`).
3. Single-slot result queue: latest-only, explicitly bounded to avoid pixel-buffer high-water marks (`async_render.rs:46-50`).
4. Custom `SmallLru` for measure/render caches: entry + **source-byte** limits (512/64 KiB, 256/32 KiB) and 8 resolved font configs (`backend/wayland/bar/text.rs:14-40`); X11 caches quantized color allocations (`draw/context.rs:50-87`).

### 2.3 Cursor frame cache
Two-level `HashMap<(CursorIcon,i32), Rc<XCursor>>` + `Rc<Vec<CursorFrame>>` (`render/cursor_theme.rs:93-102,183-262`): parse/build once per icon/scale, pick frame index per render; static cursors don't request animation redraws.

### 2.4 Hit caches and lazy placement plans
- Bar hit cache built during rendering, consumed on pointer motion — never recomputed from the model (`bar/scene.rs:771-813`, `bar/model.rs:56-144`).
- `TreePlacementSession` caches bounds and per-(target,edge) plans with `get_or_insert_with`; the whole cache is dropped on authoritative arrange (`layouts/tree/placement_session.rs:29-31,64-88`, `manager/pointer.rs:11-12`).

**eswm today:** dock buffers cached by signature (e08s01), titlebar buffers cached; no general scene/hit cache. Adding a keystone `SceneCache` + bar-like sequence invalidation is the clearest performance project.

---

## 3. Frame pacing, GPU waits, VRR

- `PresentationScheduler` (`runtime/engine.rs:18-228`) aligns wakeups to **observed presentation phase** (`period - elapsed % period`), uses generation counters so stale timers can't fire, and wakes commit-timing work one refresh before the deadline.
- DRM: explicit sync via `wp_linux_drm_syncobj`; if fence not reached it does a **blocking GPU wait on the main loop with a one-time warning**, then falls back to `reset_buffers()` + scheduler retry (`render/drm.rs:214-243`) — a documented latency tradeoff.
- VRR hard blockers (`runtime/drm/vrr.rs:19-125`): session lock, animations, layout preview, pending screencopy, overlays, **client cursor surfaces**, DnD icons; `Auto` only for exactly one true-fullscreen client.
- Cursor animation is page-flip-chained off vblank instead of an independent timer (`runtime/drm.rs:591-613`).
- winit path uses `buffer_age` + `OutputDamageTracker` partial swaps and skips swap when damage is `None` (`render/winit.rs:89-143`).

**eswm today:** callback throttle by refresh period, `frame_in_flight` gate, retry timers (e08), no VRR, no explicit sync. Borrow: phase-aligned timers + VRR blockers only when VRR work starts.

---

## 4. Memory efficiency

### 4.1 Ownership & compact types
- **Monitor owns its clients** (`HashMap<WindowId,Client>`), focus/z-order are `Vec<WindowId>` beside them; no back-pointers or `Arc<Mutex<Client>>`. `type/monitor.rs:5-11`. A debug-only graph invariant check costs nothing in release (`model.rs:372-439`).
- Accept O(monitors) linear scans rather than a stale-prone global index (`model.rs:29-31`).
- `#[repr(transparent)] Copy` newtypes: `WindowId(u32)`, `MonitorId(u64)`, `TagMask(u32)` (21 bits + reserved scratchpad bit) used as HashMap key for per-tag state/MRU (`types/tag_types.rs:16-30`). Geometry types all `Copy` with i64 intersection to avoid overflow.
- `ClientMode` is a `Copy` value struct of two small enums — presentation transitions can't clobber placement (`types/client/mode.rs:83-101`). Large optional state is niche-allocated: `Option<ScratchpadData>` only for scratchpad windows (`client.rs:82-83`).

### 4.2 Clone avoidance
- Tree replacement threads `&mut Option<Node>` to move the replacement once instead of cloning per visited node — otherwise quadratic (`layouts/tree.rs:86-112`). Tree nodes are an inline recursive enum with `Vec` children, no per-node `Box`.
- `Arc<LayoutTree>` snapshot cloned once at drag start, then `Arc::clone` per motion (`manager/pointer.rs:38,242`).
- Pixel blobs shared: `Arc<[u8]>` icons with count+byte-bounded LRU (`systray/status_notifier/icon.rs:39-46,116-126`), `Rc` cursor/bar buffers.
- `Cow<'static,str>` rule matchers (borrowed from config literals, owned from IPC) — no clone in matching hot path (`types/rules.rs:232-292`).
- Derived-not-cached geometry: `Monitor::work_rect()` recomputed per access, never invalidated (`monitor.rs:920-961`).

### 4.3 Untrusted-input bounds
`ipc/mod.rs:12-14`: 128 clients, 1 MiB requests, 30 s idle eviction; `with_limit::<MAX_REQUEST_BYTES>()` inside the decoder; 1 KiB read chunks; impossible-allocation regression test. Icon decode caps, libwayland queue bumped 4 KiB → 1 MiB (`compositor/buffer.rs`, to avoid disconnecting slow XWayland clients).

**eswm today:** single owner (`WindowManager` owns windows), small enums, but Lua `Arc<Mutex<...>>` copied per frame, global work area, no byte caps on IPC line lengths yet (e09s03 plans them). Borrow: Copy state split, debug invariant checks, bounded everything.

---

## 5. CPU efficiency — event loop & input

- **Main loop blocks with no timeout** (`event_loop.run(None, ...)`) — no idle CPU. Timers only for animations (self-disarming `AnimationTimerGuard`, `runtime.rs:258-337`) and bounded retries.
- **Render request ≠ render now**: `request_render()` sets flags; loop decides once per tick. `event_loop_tick` early-returns with no render when nothing changed (`engine.rs:276-282`); `apply_layout_targets` skips with zero clients.
- **Coalesce at the expensive boundary, not everywhere**: keyboard stays synchronous; pointer motion batched — one hit-test snapshot per drag batch (test asserts 1 snapshot for 48 motions, `motion.rs:297-362`), only last motion updates active drag (`dispatch.rs:193-195`).
- **Hot-path lookups**: WM keybinds are precompiled and a small bounded linear scan (`keyboard.rs:140-154`); global shortcuts use a local `HashMap` + atomic revision check so input never locks the D-Bus registry (`shortcuts/mod.rs:200-216,325-444`).
- **Single-pass pointer hit testing** resolves surface focus + logical window in one z-order traversal, early-exiting when both found (`compositor/window/hit_test.rs:78-79,254-257`).
- **Off-thread blocking I/O, ping-woken**: bar rasterizer, status commands, systray workers, shortcuts portal, XWayland idle timers — none poll from the main loop; systray worker blocks on `recv()` when no menu (no idle 1 Hz wake).
- **IPC**: one epoll fd watches listener + clients; nonblocking, EOF-drain-in-one-tick, 128-cap, idle pruning, nonblocking response writes that drop slow clients rather than delaying display.
- **Profiling harness**: `[profile.profiling]` (opt-level 2 + frame pointers, not release), `just profile`, `perf record -e cpu-clock:u`, `docs/profiling.md` with an explicit **idle workload** and note that <200 samples is expected for an event-driven compositor.

**eswm today:** event-driven since e05; `loop_stall` warning only counts blocked work with a pending redraw; commit/input latency warnings (BUG fix). No profiling harness, no explicit idle measurement, no coalescing of pointer batches.

---

## 6. What eswm should adopt (ranked)

### Tier A — high value, low risk, independent of e09
1. **SceneCache with `(generation_a, generation_b) → Rc<Shared>`** keyed on displayed geometry; reuse across frames and outputs; keep animated preview solids out of the cache. Start with borders/bar/dock buffers; our dock buffer signature is already a proto-cache.
2. **Coalesced invalidation with a first-ping boolean** and output-precise targets (even within one output: `None | Surface | All` initially). Replace scattered `redraw_needed = true` with a helper that pings once.
3. **Split pixel commits from callback-only commits** in `CompositorHandler::commit`: if no buffer/damage, service callbacks via timer without a render. Our e08 "every present delivers callbacks" already avoids the deadlock; this adds the inverse optimization.
4. **IPC hardening** (Tier 1 of the feature roadmap): line-length cap, client cap, 0600 socket, SO_PEERCRED, nonblocking writes, idle eviction. Already in e09s03's problem statement — now with concrete constants to borrow.
5. **Bounded caches everywhere** (cursor frames by `(icon,scale)` with `Rc`; text measure/render LRU with byte limits; icon/file decode caps when systray arrives).
6. **Debug-only model invariant checks** and `Copy` state split (ties into e09s01).

### Tier B — with e09 / later
7. Per-output render targets + per-output scene projection (after e09s02 gives real per-output geometry).
8. Phase-aligned timers and generation-guarded callbacks (low priority until VRR).
9. Profiling profile + `perf` harness with an explicit idle workload; assert near-zero CPU at rest (guards against regressing the e08 stutter fixes).
10. Off-thread rasterization only if bar/dock raster ever becomes a bottleneck — our dock is tiny, so likely unnecessary.

### Explicitly do NOT borrow
- Blocking GPU fence waits on the main loop (their documented latency hazard) — our retry/skip is safer.
- The "fragile DRM loop" wholesale — change incrementally with tests.
- `Arc<LayoutTree>` / tree placement caches — we have no tree.
- 1 MiB libwayland queue bump unless we hit slow-client disconnects.
- Their bincode IPC (product constraint: hyprctl text/JSON).

---

## 7. One-paragraph takeaway

instantWM is fast because it *never does work it can prove unnecessary*: idle loops block, invalidation coalesces, caches are keyed by what actually changed (displayed geometry, sequence numbers, snapshot equality), immutable snapshots are shared by refcount, hot-path lookups are precompiled/O(1), and blocking work lives on ping-woken threads. eswm already got the big correctness bugs right (no unconditional re-render, frame-in-flight gating, callback delivery); the next performance step is a keystone scene cache + first-ping coalesced invalidation + pixel-vs-callback commit classification + IPC bounds, all of which can land before or alongside e09 without changing product behavior.
