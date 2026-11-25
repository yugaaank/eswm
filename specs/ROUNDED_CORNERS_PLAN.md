# Rounded Corners — Per-Corner Implementation Plan

**Status:** implemented on branch `rounded-corners-per-corner`, preflight green, awaiting live DRM UAT
**Date:** 2026-11-24
**Scope:** server-side border ring + titlebar, baked-alpha masks, no GLES shader
**Config surface:** `es.dsp.set_corner_radius(n)`, `es.dsp.set_titlebar_height(n)`, `es.dsp.set_border_size(n)`

## 1. Requirement (operator)

| Titlebars | Rounded corners | Border |
|-----------|-----------------|--------|
| enabled (`titlebar_height > 0`) | titlebar **top** corners + window **bottom** corners | border ring follows every rounded arc |
| disabled (`titlebar_height == 0`) | window **top and bottom** corners | border ring follows every rounded arc |

- Rounding is **per-corner**, not one uniform radius.
- `corner_radius == 0` disables rounding entirely (existing default).
- Fullscreen windows and popups are never rounded.
- Rounding requires a border: `border_size == 0` → no ring to mask the client's square corners → no rounding.

## 2. Does instantWM round corners? — **No**

Verified in `instantWM/src` (grep for `corner_radius`, `border_radius`,
`rounded`, `smoothstep`, `antialias`):

- `backend/wayland/render/borders.rs` draws axis-aligned border geometry only.
  Its only "corner" test is `border_geometry_surrounds_the_displayed_content_rect`
  (a rectangular assertion; no radius).
- Every other hit is *hot corners* (`core_state::hot_corner`), *resize/snap
  corners* (`types/input.rs`), or a `f64::round()` call in colour conversion.
- There is no shader, mask, or alpha-masked border buffer anywhere in
  `instantWM`.

So instantWM gives us **no** rounded-corner code to port. Its borders are as
square as its opinions. The reference implementation for the technique is
Hyprland's `rounding.glsl`; the reference for *eswm's* feasible technique is the
surviving `src/rounding.rs` (baked alpha), because smithay 0.7's
`render_elements!` macro cannot express a GLES-only per-element shader override
without forking smithay.

## 3. Architecture: baked alpha, one round ring

The client surface stays **square**. Rounding is faked by a **single
alpha-masked ring buffer** drawn on top of the window, plus a **square+rounded
titlebar buffer** that fills the ring's top opening.

```
 outer frame (fw x fh)            ring buffer, drawn last (on top)
 ┌──────────────────────────┐
 │  border ring, outer r     │    opaque where border color belongs
 │   ┌──────────────────┐    │    transparent "hole" = titlebar + client
 │   │ titlebar (tb)    │    │
 │   ├──────────────────┤    │    hole corners all = inner radius ri
 │   │ client surface   │    │
 │   │   (square)       │    │
 │   └──────────────────┘    │
 └──────────────────────────┘
```

Frame geometry is already correct in `wm::border_frame()` / `wm::outer_frame()`:

- `frame = (rect.x-bs, rect.y-bs-tb, rect.width+2bs, rect.height+2bs+tb)`
- hole, in frame-local coordinates = `(bs, bs, fw-2bs, fh-2bs)` — i.e. the
  titlebar band *and* the client rect. Identical for both policies.

Radius model (concentric, nested-radius):

- `outer = corner_radius` on all four outer corners.
- `inner = corner_radius.saturating_sub(border_size)` on all four hole corners.
- Titlebar buffer (when present) keeps **square** left/right/bottom edges and
  rounds **only its top two corners by `inner`**, so it fills the hole's top
  arc exactly. The ring's inner wedge then contains the border colour that
  follows the titlebar's top arc.
- No titlebar: the client is square, the ring's inner wedge paints border
  colour over the client's four square corners.

This is why the *same* hole geometry serves both policies: the only difference
is who fills the hole's top corners (opaque titlebar vs. border-coloured ring
wedge). No separate code path is needed beyond the titlebar mask.

## 4. `src/rounding.rs` changes

The surviving file is a good start but its geometry is wrong for this policy:
`corner_radii` is a stub, `rounded_alpha` mirrors at the midpoint and uses one
radius for both axes, `rounded_border_pixels` centres the hole (the titlebar
hole is not centred), and RGB is not premultiplied.

Replace with:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Corners { pub tl: u32, pub tr: u32, pub bl: u32, pub br: u32 }

impl Corners {
    pub fn all(r: u32) -> Self;
    pub fn top(r: u32) -> Self;          // tl = tr = r, bottom 0
    /// Clamp each radius so opposing arcs cannot overlap the wrong edge.
    fn clamp(self, w: i32, h: i32) -> Self;
}

/// Corner policy for one window, computed from config + state.
pub struct RingPlan { pub outer: Corners, pub inner: Corners, pub titlebar: Corners }

pub fn ring_plan(
    radius: u32,
    border_size: u32,
    titlebar_height: u32,
    fullscreen: bool,
    popup: bool,
) -> RingPlan;
```

`ring_plan` rules:

- `radius == 0 || fullscreen || popup` → all-zero (`rounding disabled`).
- `radius <= border_size` → all-zero (avoid a degenerate inner radius < 1).
  Hyprland does the opposite correction: `data.round = rounding - 1` for the
  client and `outerRound = rounding + borderSize` for the border, so the border
  bulges *outward* by exactly `borderSize`. Our `inner = radius - borderSize`
  shrinks the hole inward instead. Both keep the ring thickness constant; ours
  keeps the outer silhouette at exactly `corner_radius` (better for the operator
  spec), Hyprland's makes the total silhouette `radius + borderSize`.
- otherwise `outer = Corners::all(radius)`, `inner = Corners::all(radius - border_size)`.
- `titlebar = if titlebar_height > 0 { Corners::top(inner.tl).clamp(..., titlebar_height) } else { Corners::default() }`.

**Channel order (fix while here).** smithay maps `Fourcc::Argb8888` to GL
`BGRA_EXT` (`gles/format.rs:21`), but the current titlebar buffer writes
`[R, G, B, A]` — red and blue are swapped, hidden only because every default
colour is grey. **Decision (§8):** use `Fourcc::Abgr8888` (GL `RGBA`) for both
the ring and the titlebar buffers, and keep the builders emitting `[R, G, B, A]`.
No byte reordering anywhere.

**Scale:** build both buffers at `scale = output.current_scale().integer_scale()`
physical pixels (`radius` and `border_size` multiplied by `scale`), declare the
logical size via `from_buffer`'s `size` argument, and include `scale` in the
cache keys. Full rationale and a code sketch in §8.

Pixel builders:

```rust
pub fn rounded_alpha(
    x: i32, y: i32, width: i32, height: i32, c: Corners,
) -> u8;
```

Implement as a per-corner signed-distance test in the corner quadrant only,
with a 1px linear antialias band (same visual result as Hyprland's
`smoothstep`, computed once per texture rebuild). No midpoint mirroring.

```rust
pub fn rounded_rect_pixels(
    width: i32, height: i32, color: [u8; 4], c: Corners,
) -> Vec<u8>;

pub fn rounded_border_pixels(
    width: i32, height: i32,
    hole_x: i32, hole_y: i32, hole_width: i32, hole_height: i32,
    color: [u8; 4], outer: Corners, inner: Corners,
) -> Vec<u8>;
```

`rounded_border_pixels` semantics:

- Pixel outside the outer rounded rect → alpha 0.
- Pixel inside the hole **and** inside the hole's rounded mask → alpha 0.
- Pixel inside the hole **but outside** the hole's rounded mask → border colour
  (this is the wedge that covers the client's / titlebar's square corner).
- Otherwise → border colour.

**Premultiplied alpha (mandatory).** smithay blends with
`BlendFunc(ONE, ONE_MINUS_SRC_ALPHA)` (`gles/mod.rs:2062`), so partial-alpha
pixels must store `rgb * a / 255`. The current builders store raw RGB with
scaled A; left as-is they produce a bright fringe on every arc. Fix both
builders.

## 5. `src/udev.rs` render integration

### 5.1 New cache

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BorderCacheKey {
    window: u64,
    rect: Rect,
    border: u32,
    titlebar: u32,
    radius: u32,
    color: [u8; 4],
}
// in UdevData:
border_buffer_cache: HashMap<BorderCacheKey, MemoryRenderBuffer>,
```

Initialize to `HashMap::new()` next to `titlebar_buffer_cache` (`udev.rs:376`).
Key includes `rect`, so a resize rebuilds; static windows reuse.

### 5.2 Replace the four solid border strips (the `SolidColorRenderElement` pushes in the border block)

For `!is_popup && presentation != Fullscreen`:

- Compute `plan = rounding::ring_plan(radius, bs, tb, fullscreen, is_popup)`.
- If `plan.outer.tl > 0` (rounding active):
  - Get/build one `MemoryRenderBuffer` from `rounded_border_pixels(fw, fh,
    bs, bs, fw-2bs, fh-2bs, color, plan.outer, plan.inner)` and push it as
    `EswmRenderElements::Cursor` (`MemoryRenderBufferRenderElement` is the
    existing memory-buffer variant) at **index 0**, so it draws last / on top.
  - Place it at `(frame.x, frame.y)`; pass `opaque_regions: None` (the hole is
    transparent).
- Else keep the existing four `SolidColorRenderElement` strips unchanged.

The ring is a single element, so the four strips' corner-join logic disappears
on the rounded path. The square path stays byte-identical (no regression, no
new cache churn when radius is 0).

### 5.3 Titlebar buffer (the `titlebar_buffer_cache` block)

- Extend `titlebar_buffer_cache` key with `radius` (or store the `Corners`).
- When `plan.titlebar` is non-zero, build the buffer with
  `rounded_rect_pixels(tb_w, tb_h, tb_px, plan.titlebar)` instead of the flat
  fill; otherwise keep the flat fill.
- Position and draw order unchanged: buttons, then titlebar, then surface, with
  the ring already at index 0 above them.
- Edge case: `titlebar_height < inner_radius` would let the hole's top arc
  overlap the client's top rows; `ring_plan` clamps `titlebar` radii to
  `min(inner, titlebar_height)`, and the hole's inner radius should be clamped
  the same way when `tb > 0` so the arcs stay coincident.

Buttons are square and inset from the right edge. At large radii the close
button can enter the arc; note this in UAT and, if it looks wrong, inset the
button origin by `inner_radius` (do not change radius policy).

## 6. `src/main.rs`, `src/wm.rs`, `src/lua_state.rs`

- `src/main.rs`: add `mod rounding;`.
- `src/wm.rs`: no change needed. `corner_radius` already lives in
  `DecorationConfig` (`wm.rs:873`) and the renderer reads
  `self.decoration.lock().unwrap().corner_radius` directly (`udev.rs:3224`);
  `WindowManager` does not need to mirror it because borders/titlebars are
  rendered from `DecorationConfig`.
- `src/lua_state.rs`: update the stale comment at line 337 ("renderer clipping
  TBD") to describe the ring/titlebar mask.

## 7. Steps (each with a verify command)

1. **Geometry core.** Add `Corners`, `ring_plan`, rewrite `rounded_alpha`,
   `rounded_rect_pixels`, `rounded_border_pixels` with explicit hole rect and
   premultiplied RGB in `src/rounding.rs`. Add `mod rounding;` in `main.rs`.
   Add a comment at the format constant documenting `Abgr8888` → GL `RGBA`.
   → `cargo test --bin eswm rounding`
2. **Policy tests.** Table test: `(radius, border, titlebar, fullscreen, popup)`
   → expected `outer/inner/titlebar` corners, including `radius == 0`,
   `radius <= border`, fullscreen, popup, and `titlebar_height < inner`.
   → `cargo test --bin eswm ring_plan`
3. **Pixel tests.** `rounded_alpha` centres/edges opaque, each corner
   transparent at its own radius, opposite corners independent; alpha is
   premultiplied at an antialiased pixel (`rgb == color * a / 255`). Assert the
   outer arc's fringe is darker than the raw colour, not brighter.
   → `cargo test --bin eswm rounded_`
4. **Ring buffer cache.** Add `BorderCacheKey` + `border_buffer_cache` to
   `UdevData`; build/push the ring element on the rounded path; leave the
   square path untouched. Build ring at `scale`-multiplied physical size with
   the hole at `(bs, bs, fw-2bs, fh-2bs)` (all × `scale`), upload with
   `Abgr8888`, and pass the logical `frame_w x frame_h` to `from_buffer`.
   Include `scale` in `BorderCacheKey`. Force `redraw_needed` on a fresh build.
   → `cargo build`
5. **Titlebar mask.** Extend the titlebar cache key with `radius` and `scale`;
   switch its upload to `Abgr8888`; build with `Corners::top(inner)` when
   rounding is active.
   → `cargo build`
6. **Full preflight.** → `cargo clippy --all-targets && cargo test && cargo build`
7. **Live DRM UAT** (cannot run inside a Wayland/X11 session — VT-grab rule):
   - `es.dsp.set_border_size(2)`, `set_corner_radius(12)`,
     `set_titlebar_height(24)`: top corners of the titlebar are rounded, bottom
     corners of the window are rounded, border follows every arc, no notch at
     the titlebar/border join.
   - `set_titlebar_height(0)`: all four window corners rounded, border follows
     each arc, no square client pixels visible at any corner.
   - `set_corner_radius(0)`: behaves exactly like today.
   - `set_border_size(0)`: no rounding (documented), no crash.
   - Fullscreen: square, no ring.
   - Resize a floating window: arcs redraw cleanly, no stale corner pixels.
   - Focus/unfocus: ring recolours without a frame of stale pixels.
   - Non-grey border colour: red channel is red, not blue (§8 #5).
   - Fractional output scale (1.25×/1.5×): arcs align with titlebar/client.
   → operator confirmation; record in `specs/verifications/`.

## 8. Comparison with Hyprland's implementation (local `Hyprland/`)

Read from `/home/yugaaank/Projects/Hyprland`, not from memory.

### What Hyprland actually does

1. **Two shaders, two radii.** `rounding.glsl:11` provides
   `rounding(vec4 color, float radius, float roundingPower, vec2 topLeft,
   vec2 fullSize)`. It mirrors `gl_FragCoord` into one quadrant
   (`pixCoord *= lessThan(pixCoord, 0) * -2 + 1`) and tests a **superellipse**
   `pow(pow(x,p) + pow(y,p), 1/p)`, with `roundingPower = 2` = circle.
   Antialias is `smoothstep` over `SMOOTHING_CONSTANT = pi/5.3466...`.
2. **Surface pass.** `ElementRenderer.cpp:311-319` subtracts 1 from the radius
   ("to fix a border issue"), zeros it for fullscreen (`dontRound`), and puts
   `round` on every `CTexPassElement`. Popups also set `dontRound`
   (`Renderer.cpp:855,3568`).
3. **Border pass.** `border.glsl:16` takes `radiusOuter` **and** `radius`
   (inner). `CHyprBorderDecoration.cpp:67-80` computes
   `ROUNDING = config_rounding * scale` and
   `OUTERROUND = (ROUNDING + borderSize - CORRECTIONOFFSET) * scale`, where
   `CORRECTIONOFFSET = borderSize * (sqrt(2)-1) * max(2 - roundingPower, 0)`.
   `OpenGL.cpp:2055,2084-2087` feeds both radii plus `thick` to the shader.
   The border is a **full-frame quad** with distance-test discard, not strips
   and not a cached texture (`OpenGL.cpp:2093-2103`).
4. **Damage is explicitly computed for the arc.**
   `CHyprBorderDecoration.cpp:118-124` builds `borderRegion` from `GLOBAL_BOX`
   minus `expand(-(borderSize + ROUNDING))`, padded by 2.
5. **Hit-testing is rounded too.** `WindowPresentation.cpp:454-476`
   `isInCurvedCorner(x, y)` uses the *same* superellipse test per corner, and
   `InputManager.cpp:874,2250` call it so clicks in the clipped wedge pass
   through to the window behind.
6. **One radius for all four corners** (`decoration:rounding`,
   `ConfigValues.cpp:201`) plus a global `rounding_power`. No per-corner config.

### Pros of our baked-alpha plan vs. Hyprland

- **No smithay fork / no GLES custom program.** eswm's `render_elements!` macro
  cannot express the GLES-only bound; Hyprland owns its renderer so it can.
  Our path is plain `MemoryRenderBuffer`, same mechanism as the titlebar cache.
- **Per-corner policy is natural in a texture.** Hyprland's shader has one
  radius per surface; "titlebar top only + client bottom only" would need extra
  uniforms or a separate pass per corner. Our `Corners` struct and explicit
  hole rect express it directly.
- **Cheap per frame.** One cached buffer per window, rebuilt only on
  resize/colour/radius change. Hyprland runs a fragment shader over the whole
  window and border quad every frame.
- **Works with any renderer** (GLES, software fallback, multi-GPU wrapper),
  whereas Hyprland's path is GL-specific.

### Cons / risks — and whether the plan can actually work

| # | Risk | Severity | Verdict / fix |
|---|------|----------|---------------|
| 1 | **Direct scanout may bypass the ring.** eswm passes `ALLOW_PRIMARY_PLANE_SCANOUT_ANY \| ALLOW_CURSOR_PLANE_SCANOUT` (`udev.rs:3707`). smithay only tries primary-plane scanout for the **last** visible element (`compositor/mod.rs:1986`); our ring is index 0, so it is never that element. Memory buffers also have `underlying_storage() -> Memory`, which the DRM exporter rejects (`exporter/mod.rs:38`). If the client surface is last and edge-to-edge, smithay could zero-copy it while the ring composites on top — visually correct, but the zero-copy optimisation is lost. | Medium | Works. UAT should check for flicker; if performance regresses, clear the flag per-frame when any radius > 0. |
| 2 | **Damage tracking.** `MemoryRenderBufferRenderElement::damage_since` returns full-geometry damage on first paint (`memory.rs:560-590`). Hyprland *explicitly* damages the arc. A cached ring with no new damage may leave stale pixels after a colour change. | Medium | Pass `opaque_regions: None`; force `redraw_needed` on radius/colour change. Verify by focusing/resizing. |
| 3 | **Antialias quality.** Our 1px linear ramp is coarser than Hyprland's `smoothstep` over `SMOOTHING_CONSTANT ≈ ±0.588px` and its configurable superellipse power. | Low | Fine for v1; add `smoothstep` + `rounding_power` later if needed. |
| 4 | **Fractional output scale.** eswm uses `fractional_scale()` (`udev.rs:3412`) while the ring is an integer-size logical buffer. At 1.25×/1.5× the baked arc won't align with the resampled titlebar/client. Hyprland multiplies radius by `pMonitor->m_scale` (`Renderer.cpp:683`) so all passes share the physical grid. | **High** | Build the ring at `frame * scale` physical pixels with a matching `Scale`, or clamp to integer scale for v1. This is the most likely "doesn't look right" failure. |
| 5 | **Channel order.** `Argb8888` → GL `BGRA_EXT` (`gles/format.rs:21`); current titlebar writes `[R,G,B,A]`, hidden by grey defaults. | Medium | Fixed in §4; test with a non-grey border colour. |
| 6 | **Premultiplied alpha.** smithay blends `ONE, ONE_MINUS_SRC_ALPHA` (`gles/mod.rs:2062`); Hyprland does `pixColor.rgb *= pixColor[3]` (`border.glsl:102`). Our builder emits raw RGB. | **High** | Fixed in §4; add a pixel-level test. |
| 7 | **`radius <= border_size` disables rounding**, while Hyprland allows any radius ≥ 0 and uses `rounding - 1`. At `bs=2, radius=3` our rule turns rounding off instead of a 1px inner arc. | Low | Documented; optional `inner >= 1` fallback. |
| 8 | **Hit-testing stays rectangular.** Hyprland routes curved-corner clicks through to the window behind. eswm's `outer_frame`/`window_at` are rectangular. | Low | Out of scope; note in UAT. |
| 9 | **`titlebar_height < inner radius`** makes the hole's top arc overlap client rows. | Medium | `ring_plan` clamps titlebar and hole `inner` to `titlebar_height`; UAT at `tb=4, radius=12`. |
| 10 | **Popup/subsurface in the corner.** Popup trees are appended after the frame (`udev.rs:3395`) and intentionally paint over the ring; they are never rounded. Verify no square client corner peeks out. | Medium | UAT. |
| 11 | **Cached buffer memory.** Full-frame ARGB per window (~4×W×H bytes) at 4K can be large; `titlebar_buffer_cache` is never evicted (pre-existing). | Low | Add eviction on window removal alongside the existing cache. |

### Solutions for the three blockers

#### #5 Channel order — switch the buffer format, not the byte order

Verified in smithay 0.7 source: `Fourcc::Abgr8888` maps to GL `RGBA8`/`RGBA`
(`gles/format.rs:21`) and is in `SUPPORTED_MEM_FORMATS_2`
(`gles/mod.rs:911`). So a `Vec` written as `[R,G,B,A]` is sampled correctly with
`Abgr8888`. The existing titlebar code writes `[R,G,B,A]` and is therefore
*only* wrong because it passes `Argb8888`.

**Fix:** change `Fourcc::Argb8888` → `Fourcc::Abgr8888` for both the titlebar
buffer and the new ring buffer. Keep the pixel builders emitting `[R,G,B,A]` —
no per-channel reordering, no endianness reasoning, nothing to get wrong on a
second pass. This is a one-token change per `from_slice` call and it fixes an
existing latent bug (non-grey titlebars currently show red as blue). Add a
unit test that documents the mapping as a comment next to the format constant.

#### #6 Premultiplied alpha — bake it into the builder

Verified: smithay's GLES renderer always blends
`BlendFunc(ONE, ONE_MINUS_SRC_ALPHA)` (`gles/mod.rs:2062`), and its implicit
texture shader does nothing but `color = texture2D(tex, v_coords) * alpha`
(`shaders/implicit/texture.frag:23`). No shader flag changes this. Hyprland
explicitly premultiplies (`border.glsl:102`: `pixColor.rgb *= pixColor[3]`).

**Fix:** in `rounded_rect_pixels` and `rounded_border_pixels`, compute the
final alpha first, then store `channel * a / 255` for R/G/B. Use integer math
`(c as u16 * a as u16 / 255) as u8` to avoid float rounding drift. The existing
builders already do this for alpha (`color[3] * mask / 255`); extend the same
factor to the colour bytes. Add a pixel-level test: at an antialiased pixel
with mask `a`, assert `stored_rgb == original_rgb * a / 255`.

Note: for the ring's **inner wedge** (border colour painted over the client's
square corner) the mask is near 255, so premultiplication has no visible effect
there — the fringe risk is entirely on the **outer arc**. Both paths still use
the same builder.

#### #4 Fractional output scale — use integer-scaled buffers at physical size

Verified: `MemoryRenderBuffer::from_slice` takes a `scale: i32` and a buffer
size in `Buffer` pixels (`memory.rs:390-402`);
`MemoryRenderBufferRenderElement::from_buffer` takes an explicit logical `size`
and derives `physical_size` as `size * scale` (`memory.rs:481-523`). So a
buffer can be built at physical resolution while still declaring a logical
coverage area, and smithay does the resampling for us.

**Fix (chosen):** build both the ring and titlebar buffers in **physical
pixels** using `output.current_scale()` rounded to an integer for the buffer
scale, and pass that `size` as the logical frame size to `from_buffer`:

```rust
let scale = output.current_scale().integer_scale().max(1); // i32
// ring pixel dimensions in Buffer space:
let pw = frame_w * scale;
let ph = frame_h * scale;
let buf = MemoryRenderBuffer::from_slice(
    &pixels, Fourcc::Abgr8888, (pw, ph), scale, Transform::Normal, None,
);
// coverage stays logical (frame_w x frame_h); element location is Physical
MemoryRenderBufferRenderElement::from_buffer(
    &mut renderer, Point::from((frame_x, frame_y)), &buf,
    None, None, Some(Size::from((frame_w, frame_h))), Kind::Unspecified,
);
```

- `radius` and `border_size` are multiplied by `scale` when generating pixels,
  so arcs are rasterised on the same physical grid the compositor presents —
  the same invariant Hyprland gets by multiplying `rounding * pMonitor->m_scale`
  (`Renderer.cpp:683`).
- The `BorderCacheKey` must include `scale` so a monitor-scale change rebuilds.
- Fractional scales (1.25×, 1.5×): `integer_scale()` rounds, so the buffer is
  1× or 2× while the output is 1.25×/1.5×. smithay then samples the 1×/2×
  buffer to the output's physical size. This is **the same baseline eswm already
  accepts for its titlebar and cursor buffers** (`udev.rs:3358` uses scale 1),
  so the ring is no worse than existing decoration. UAT must include a
  fractional-scale monitor; if the arc looks soft, the follow-up is to build at
  `fractional_scale().ceil()` pixels while keeping the declared logical `size`
  — `from_buffer` already supports that via `size`.
- Alternative rejected: keep a 1× logical buffer and rely on resampling. It is
  simpler but makes arcs visibly soft at 2×, which is a common DRM setup.

#### Smaller gaps (cheap improvements)

- **Arc damage (#2):** after building a new ring/titlebar buffer, call
  `renderer.damage` equivalent by setting `state.redraw_needed = true` (eswm
  already does this on config changes) rather than relying on the cached
  buffer's empty damage bag. Compare Hyprland's explicit
  `borderRegion.expand(2)`.
- **Superellipse / `rounding_power` (#3):** `rounded_alpha` can evaluate
  `pow(pow(dx,p)+pow(dy,p), 1/p)` instead of the circle distance. Cost is a few
  `powf` calls per *texture rebuild*, not per frame. Expose `es.dsp.set_rounding_power(p)` only if the operator wants it; v1 stays `p=2`.
- **Rounded hit-testing (#8):** add `wm::in_rounded_wedge(rect, corners, x, y)`
  mirroring Hyprland's `isInCurvedCorner`, and subtract that region in
  `wm::window_at` / `outer_frame` hit paths. Pure function + table tests. Risk:
  resize handles currently use `outer_frame`; they must keep the square rect or
  edges become ungrabbable. Low priority.

### Bottom line

Yes, it can work, with three mandatory fixes before it looks right:
**premultiplied RGB** (#6), **channel order** (#5 — solved cleanly by using
`Abgr8888`), and **output-scale handling** (#4 — solved by building buffers at
integer-scaled physical size with a logical `size` override). Direct scanout
(#1) and damage (#2) are contained by smithay's front-to-back element order and
full-element damage, but need live DRM UAT. The per-corner policy is *easier*
than Hyprland's shader path, not harder — the texture is where per-corner
geometry becomes trivial.

## 9. Additional implementation limits

- **Nested radius mismatch**: if hole inner radius and titlebar top radius
  differ, a seam appears. They share `plan.inner`; tests assert equality.
- Hyprland supports `rounding_power` (superellipse); v1 is circular only.

## 10. Out of scope

- Rounded hit-testing / input regions.
- Rounded shadows (eswm has no shadow rendering).
- Per-window differing radii.
- Rounding when `border_size == 0`.
- A GLES shader path (blocked by smithay 0.7 `render_elements!`).

## 11. Implementation record (2026-11-25)

TDD, three test-only RED commits followed by GREEN commits:

- `ffa9a6e` / `aaf4dc0` — `Corners` + `ring_plan` per-corner policy.
- `3f90be9` / `160ee69` — `rounded_alpha` with four independent radii and a
  2px antialias band.
- `8864e81` / `41be5a5` — premultiplied `[R,G,B,A]` pixel builders and the
  border-ring wedge.
- `41cdd02` — render integration: cached ring buffer replaces the four solid
  strips, rounded titlebar buffer, `Abgr8888` upload, output-scale-aware cache
  keys, logical size override.
- `0f0b279` — geometry guard test and cleanup.

Verified: `cargo clippy --all-targets` clean, `cargo test` 228 passed,
`cargo build` clean (log `/tmp/opencode/rounded-corners-preflight.log`).
Live DRM UAT still pending — this session runs inside an existing Wayland/X11
session, where grabbing the VT is forbidden.

### 11.1 UAT defect: square corners still visible (2026-11-25)

Symptom: the arcs drew, but a square block of border colour stayed pinned over
each corner. Reproduced on the CPU by compositing the real element stack
(ring over a square client) with the operator's settings — radius 12, **border
size 1**, titlebar 20.

Root cause: `rounded_border_pixels` computed the hole-corner wedge as
`255 - inner_alpha` and ignored the outer mask. Inset by only `border_size`, the
hole's corner sits `(radius - border_size) * sqrt(2)` from the arc centre while
the outer arc stops at `radius`, so for `border_size < radius * (1 - 1/sqrt2)`
(≈ 3.5px at radius 12) the wedge extends past the silhouette. The ring then
painted border colour in a square outside its own arc. A 3px border hides it;
the operator's 1px border does not.

Fix: `mask = outer_mask.min(255 - inner_alpha)` — the wedge is clipped by the
outer silhouette, so the visible shape is exactly "outer rounded rect minus the
inner rounded hole". Regression guard:
`thin_border_wedge_cannot_poke_outside_the_arc` asserts no pixel outside the
outer arc is painted and that the corner border still closes over the client's
square corner.

Verified: `cargo clippy --all-targets` clean, `cargo test` 229 passed,
`cargo build` clean.

## 12. Prior art

- Local Hyprland checkout `/home/yugaaank/Projects/Hyprland`:
  `src/render/shaders/glsl/rounding.glsl`, `border.glsl`,
  `src/render/ElementRenderer.cpp`, `src/render/OpenGL.cpp`,
  `src/render/decorations/CHyprBorderDecoration.cpp`,
  `src/desktop/view/window/WindowPresentation.cpp`. Reference only — not ported.
- instantWM: no rounded-corner implementation (see §2). `borders.rs` is
  rectangular only.
- smithay 0.7: `MemoryRenderBuffer::from_slice`,
  `MemoryRenderBufferRenderElement::from_buffer`,
  `gles/mod.rs:2062` premultiplied blend func.
