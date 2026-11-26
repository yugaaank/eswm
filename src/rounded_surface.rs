//! Rounded client surfaces via a custom texture shader.
//!
//! Adapted from cosmic-comp's `ClippedSurfaceRenderElement`
//! (GPL-3.0-only, Copyright (C) 2023 System76, Inc.); cosmic credits niri.
//!
//! The old baked-ring approach left the client surface square underneath the
//! decoration and painted border-coloured wedges over its corners. That only
//! works while the border is thick enough to hide the corner and it cannot
//! round anything when `border_size == 0`. Here the client's own texture is
//! masked by a per-corner signed-distance test in `rounded_surface.frag`, so
//! the visible window is rounded no matter what is painted around it.
//!
//! Smithay 0.7 exposes exactly the hooks cosmic uses:
//! `GlesRenderer::compile_custom_texture_shader` plus
//! `GlesFrame::override_default_tex_program`, and `MultiFrame: AsMut<GlesFrame>`.
//! The `render_elements!` bound therefore only needs an `AsGlRenderer` escape
//! hatch to the underlying GLES frame.
//!
//! The old plan swore smithay said "no" to per-element shaders. Turns out
//! smithay said "no" to *forking it*; the hooks were there the whole time,
//! quietly waiting like a spare umbrella in a desert. See ADR-0006.

use smithay::backend::renderer::element::{
    Element, Id, Kind, RenderElement, UnderlyingStorage,
};
use smithay::backend::renderer::gles::{
    GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::utils::{
    Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform,
};

use crate::rounding::Corners;

static ROUNDED_SURFACE_SHADER: &str = include_str!("shaders/rounded_surface.frag");

/// Access the GLES renderer and frame hidden behind smithay's renderer wrapper.
/// Only `UdevRenderer` (a `MultiRenderer` over `GlesRenderer`) implements it.
pub trait AsGlRenderer: Renderer {
    fn gl_renderer(&mut self) -> &mut GlesRenderer;
    fn gl_frame<'fr, 'f, 'b>(
        frame: &'fr mut Self::Frame<'f, 'b>,
    ) -> &'fr mut GlesFrame<'f, 'b>
    where
        'f: 'fr,
        'b: 'fr;
}

struct RoundedShader(GlesTexProgram);

fn program<R: AsGlRenderer>(renderer: &mut R) -> Option<GlesTexProgram> {
    if let Some(cached) = renderer
        .gl_renderer()
        .egl_context()
        .user_data()
        .get::<RoundedShader>()
    {
        return Some(cached.0.clone());
    }

    let gl = renderer.gl_renderer();
    let compiled = gl
        .compile_custom_texture_shader(
            ROUNDED_SURFACE_SHADER,
            &[
                UniformName::new("geo_origin", UniformType::_2f),
                UniformName::new("geo_size", UniformType::_2f),
                UniformName::new("corner_radius", UniformType::_4f),
            ],
        )
        .map_err(|err| tracing::warn!(?err, "rounded-surface shader compile failed"))
        .ok()?;

    gl.egl_context()
        .user_data()
        .insert_if_missing(|| RoundedShader(compiled.clone()));
    Some(compiled)
}

/// Wraps a render element and clips it to a per-corner rounded rectangle.
pub struct RoundedElement<E> {
    inner: E,
    program: Option<GlesTexProgram>,
    uniforms: Vec<Uniform<'static>>,
    geometry: Rectangle<i32, Physical>,
    corners: Corners,
}

impl<E: Element> RoundedElement<E> {
    /// `geometry` is the final physical rect of `inner` (for a window surface,
    /// its target rect). `corners` are in the same pixel units.
    pub fn new<R: AsGlRenderer>(
        renderer: &mut R,
        inner: E,
        geometry: Rectangle<i32, Physical>,
        corners: Corners,
    ) -> Self {
        let program = if corners == Corners::default() {
            None
        } else {
            program(renderer)
        };
        let uniforms = vec![
            Uniform::new(
                "geo_origin",
                (geometry.loc.x as f32, geometry.loc.y as f32),
            ),
            Uniform::new(
                "geo_size",
                (geometry.size.w as f32, geometry.size.h as f32),
            ),
            Uniform::new(
                "corner_radius",
                [
                    corners.tl as f32,
                    corners.tr as f32,
                    corners.br as f32,
                    corners.bl as f32,
                ],
            ),
        ];
        Self { inner, program, uniforms, geometry, corners }
    }
}

impl<E> RoundedElement<E> {
    fn corner_rects(&self, elem_loc: Point<i32, Physical>) -> [Rectangle<i32, Physical>; 4] {
        let g = self.geometry;
        let tl = self.corners.tl as i32;
        let tr = self.corners.tr as i32;
        let br = self.corners.br as i32;
        let bl = self.corners.bl as i32;
        let mut rects = [
            Rectangle::new(g.loc, Size::from((tl, tl))),
            Rectangle::new(
                Point::from((g.loc.x + g.size.w - tr, g.loc.y)),
                Size::from((tr, tr)),
            ),
            Rectangle::new(
                Point::from((g.loc.x + g.size.w - br, g.loc.y + g.size.h - br)),
                Size::from((br, br)),
            ),
            Rectangle::new(
                Point::from((g.loc.x, g.loc.y + g.size.h - bl)),
                Size::from((bl, bl)),
            ),
        ];
        for rect in &mut rects {
            rect.loc -= elem_loc;
        }
        rects
    }
}

impl<E: Element> Element for RoundedElement<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        // FIXME: radius changes should damage the whole element. Config changes
        // already force a redraw, so the cached buffer does not go stale here.
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // The corner squares are not opaque any more, so conservative
        // occlusion must subtract them. Same bookkeeping cosmic does.
        let elem_loc = self.geometry(scale).loc;
        let corners = self.corner_rects(elem_loc);
        let regions: Vec<_> = self.inner.opaque_regions(scale).into_iter().collect();
        OpaqueRegions::from_slice(&Rectangle::subtract_rects_many(regions, corners))
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R, E> RenderElement<R> for RoundedElement<E>
where
    R: Renderer + ImportAll + AsGlRenderer,
    E: RenderElement<R>,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        let Some(program) = self.program.as_ref() else {
            return self.inner.draw(frame, src, dst, damage, opaque_regions);
        };
        {
            let gl_frame = R::gl_frame(frame);
            gl_frame.override_default_tex_program(program.clone(), self.uniforms.clone());
        }
        let result = self.inner.draw(frame, src, dst, damage, opaque_regions);
        R::gl_frame(frame).clear_tex_program_override();
        result
    }

    // Direct scanout would draw the full unrounded dmabuf and bypass the
    // shader, so always force the texture path.
    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corners(tl: u32, tr: u32, br: u32, bl: u32) -> Corners {
        Corners { tl, tr, bl, br }
    }

    #[test]
    fn corner_rects_are_element_local_and_follow_radius_order() {
        let elem = RoundedElement {
            inner: (),
            program: None,
            uniforms: Vec::new(),
            geometry: Rectangle::new(Point::from((10, 20)), Size::from((100, 50))),
            corners: corners(4, 6, 8, 10),
        };
        let rects = elem.corner_rects(Point::from((10, 20)));
        assert_eq!(rects[0], Rectangle::new(Point::from((0, 0)), Size::from((4, 4))));
        assert_eq!(rects[1], Rectangle::new(Point::from((94, 0)), Size::from((6, 6))));
        assert_eq!(rects[2], Rectangle::new(Point::from((92, 42)), Size::from((8, 8))));
        assert_eq!(rects[3], Rectangle::new(Point::from((0, 40)), Size::from((10, 10))));
    }
}
