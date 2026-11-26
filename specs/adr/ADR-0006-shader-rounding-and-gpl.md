# ADR-0006: Real shader clipping for rounded corners, and GPL-3.0-only

- Status: Accepted
- Date: 2026-11-26
- Deciders: eswm maintainer

## Context

eswm rounded corners by baking alpha into a border ring and titlebar texture and
painting border-coloured wedges over the client's square corners (see
`ROUNDED_CORNERS_PLAN.md` §§3–12). That works only while the border is thick
enough to hide the square client pixels underneath. With a 1px border — the
operator's actual setting — the wedge pokes outside the arc and a square block
of border colour remains (§11.1). With `border_size == 0` no rounding is
possible at all.

The plan claimed smithay 0.7's `render_elements!` macro could not express a
GLES-only per-element shader override without forking smithay. That claim was
false. `cosmic-comp` proves the required APIs exist in stock smithay 0.7:

- `GlesRenderer::compile_custom_texture_shader`
- `GlesFrame::override_default_tex_program` / `clear_tex_program_override`
- `MultiFrame: AsMut<GlesFrame>` (through the `MultiRenderer` GPU wrapper)

COSMIC's `ClippedSurfaceRenderElement` clips the client's own texture pixels to
a per-corner rounded rectangle in the fragment shader. Its shader is
GPL-3.0-only (cosmic-comp is GPL-3.0-only, and it credits niri, also
GPL-3.0). eswm was MIT, so adapting that code required a compatible license.

## Decision

1. **Relicense eswm from MIT to GPL-3.0-only.** `LICENSE` is now the GNU GPL
   v3 text; `Cargo.toml` declares `license = "GPL-3.0-only"`; `README.md` and the
   new `NOTICE` document the cosmic-comp / System76 / niri attribution. The
   maintainer is the rights holder, so this is a unilateral decision.
2. **Clip the real client surface pixels with a custom GLES texture shader.**
   New `src/rounded_surface.rs` wraps `ClippedSurfaceElement` in a
   `RoundedElement` that overrides the default texture program for the duration
   of the inner `draw`. New `src/shaders/rounded_surface.frag` applies COSMIC's
   `rounding_alpha` signed-distance test with a half-pixel `smoothstep` band.
3. **Keep the existing baked ring and titlebar mask.** The shader and the ring
   are complementary: the shader hides the client's square corners from
   underneath, the ring paints the border arc on top. `border_size == 0` now
   rounds correctly, because the shader does not depend on a border.
4. **Simplify away COSMIC's `input_to_geo` matrix.** eswm always renders with
   `Transform::Normal` and stores window geometry in physical framebuffer
   pixels (`wm_output.geometry` = DRM mode size). smithay's GLES projection
   (`flip180 * transform.matrix() * renderer`) then maps physical `(x, y)`
   directly to `gl_FragCoord.xy`. The shader subtracts a global `geo_origin`
   instead of carrying a matrix. If eswm ever supports output rotation, this
   assumption must be revisited — COSMIC's matrix approach is the robust
   upgrade path.
5. **Corner policy is unchanged:** titlebar present → round client bottom
   corners only; no titlebar → round all four; fullscreen and popups → no
   rounding; `corner_radius == 0` → no rounding. `client_corners()` translates
   the existing `ring_plan().inner` into the shader radii.

## Consequences

- The compositor now links its license to GPL-3.0-only. MIT downstream users
  can no longer relicense under a permissive term; that is the price of using
  COSMIC's code instead of doing a clean-room reimplementation.
- Surface pixels are genuinely masked, so thin/no borders round correctly and
  no square corner can survive under the decoration.
- The custom shader is GLES-only. `AsGlRenderer` is implemented only for the
  DRM `UdevRenderer` (`MultiRenderer<GlesRenderer>`); there is no software
  renderer path to preserve. eswm is DRM/libseat-only (ADR-0002), so this is
  not a regression.
- Direct scanout is disabled for rounded elements (`underlying_storage()`
  returns `None`) so the unclipped dmabuf cannot bypass the shader.
- Opaque regions subtract the four corner squares, so smithay's conservative
  occlusion does not assume clipped corners are opaque.
- `gl_FragCoord` orientation is correct for `Transform::Normal` only. Verified
  from smithay source: with `Normal`, `flip180` cancels the renderer's y-negation
  so `gl_FragCoord.y` increases with physical `y`. Rotation support would need
  COSMIC's `input_to_geo` matrix.
- Damage on a radius change is not signalled by the wrapper (COSMIC has the
  same `FIXME`). Config changes already force a full redraw, so this is not
  observable today.
- Live DRM UAT was operator-only (grabbing the VT from an existing Wayland/X11
  session is forbidden by ADR-0002), and the operator passed it on 2026-11-26:
  `border_size=0` clips all four corners, `border_size=1` shows no square wedge,
  and titlebar on/off gives bottom-only vs. all-four. Merged into local `main`
  as `b8d4c2c`; CI still unverified per operator skip precedent, nothing pushed.
