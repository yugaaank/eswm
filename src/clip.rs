//! Render-time clipping for client surfaces that commit a buffer larger than
//! the size the compositor configured. When a client ignores the configure
//! (Electron/Craft apps do this on first map), the surface tree is drawn at its
//! natural buffer size and spills outside the frame. We crop the source rect
//! and destination rect proportionally so only the configured area is painted.
//!
//! Smithay 0.7 has no per-element clip, but `Frame::render_texture_from_to`
//! maps `src` -> `dst`, so a proportional crop of both is an exact clip. The
//! compositor equivalent of telling an overenthusiastic painter to stay inside
//! the lines — Craft windows have the self-control of a toddler with a marker.

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{ImportAll, Renderer};
use smithay::utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size};

/// Crop `inner_src` (buffer coords) and `inner_geo` (physical coords) to
/// `clip` (physical coords). Returns the proportional source crop plus the
/// clipped destination, or `None` when there is no visible intersection.
pub fn clip_element_geometry(
    inner_src: Rectangle<f64, BufferCoords>,
    inner_geo: Rectangle<i32, Physical>,
    clip: Rectangle<i32, Physical>,
) -> Option<(Rectangle<f64, BufferCoords>, Rectangle<i32, Physical>)> {
    let clipped_geo = inner_geo.intersection(clip)?;
    if clipped_geo.size.w <= 0 || clipped_geo.size.h <= 0 {
        return None;
    }
    if inner_geo.size.w <= 0 || inner_geo.size.h <= 0 {
        return None;
    }

    let scale_x = inner_src.size.w / inner_geo.size.w as f64;
    let scale_y = inner_src.size.h / inner_geo.size.h as f64;

    let clipped_src = Rectangle::new(
        Point::from((
            inner_src.loc.x + f64::from(clipped_geo.loc.x - inner_geo.loc.x) * scale_x,
            inner_src.loc.y + f64::from(clipped_geo.loc.y - inner_geo.loc.y) * scale_y,
        )),
        Size::from((
            f64::from(clipped_geo.size.w) * scale_x,
            f64::from(clipped_geo.size.h) * scale_y,
        )),
    );
    Some((clipped_src, clipped_geo))
}

/// Wraps a surface render element and clips it to a fixed physical rectangle.
/// Always returns `None` for underlying storage so the DRM compositor cannot
/// bypass the clip with direct scanout.
pub struct ClippedSurfaceElement<R: Renderer> {
    inner: WaylandSurfaceRenderElement<R>,
    clip: Rectangle<i32, Physical>,
    scale: Scale<f64>,
}

impl<R: Renderer + ImportAll> ClippedSurfaceElement<R> {
    pub fn new(
        inner: WaylandSurfaceRenderElement<R>,
        clip: Rectangle<i32, Physical>,
        scale: Scale<f64>,
    ) -> Self {
        Self { inner, clip, scale }
    }

    fn clipped_geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner
            .geometry(scale)
            .intersection(self.clip)
            .unwrap_or_else(|| Rectangle::from_size((0, 0).into()))
    }
}

impl<R: Renderer + ImportAll> Element for ClippedSurfaceElement<R> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.clipped_geometry(scale).loc
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        let inner_src = self.inner.src();
        let inner_geo = self.inner.geometry(self.scale);
        match clip_element_geometry(inner_src, inner_geo, self.clip) {
            Some((src, _)) => src,
            None => inner_src,
        }
    }

    fn transform(&self) -> smithay::utils::Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.clipped_geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner
            .damage_since(scale, commit)
            .into_iter()
            .filter_map(|d| d.intersection(self.clip))
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner
            .opaque_regions(scale)
            .into_iter()
            .filter_map(|r| r.intersection(self.clip))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R> RenderElement<R> for ClippedSurfaceElement<R>
where
    R: Renderer + ImportAll,
    R::TextureId: 'static,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        self.inner.draw(frame, src, dst, damage, opaque_regions)
    }

    // Never advertise the underlying buffer: direct scanout would render the
    // full dmabuf and defeat the crop. Force the texture path, which honours
    // the clipped src/dst.
    fn underlying_storage(&self, _renderer: &mut R) -> Option<smithay::backend::renderer::element::UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    fn src(x: f64, y: f64, w: f64, h: f64) -> Rectangle<f64, BufferCoords> {
        Rectangle::new(Point::from((x, y)), Size::from((w, h)))
    }

    #[test]
    fn clip_crops_overflowing_buffer_to_target() {
        // Craft window: buffer 1440x900 drawn at (393,215), frame 1190x795.
        let (cropped_src, cropped_dst) = clip_element_geometry(
            src(0.0, 0.0, 1440.0, 900.0),
            rect(393, 215, 1440, 900),
            rect(393, 215, 1190, 795),
        )
        .expect("overlapping rects must clip");
        assert_eq!(cropped_dst, rect(393, 215, 1190, 795));
        assert_eq!(cropped_src, src(0.0, 0.0, 1190.0, 795.0));
    }

    #[test]
    fn clip_offsets_src_when_geometry_starts_before_clip() {
        // CSD window: surface origin (349,177), window geometry (365,187).
        // The 16,10 offset must become a 16,10 source crop.
        let (cropped_src, cropped_dst) = clip_element_geometry(
            src(0.0, 0.0, 1222.0, 837.0),
            rect(349, 177, 1222, 837),
            rect(365, 187, 1190, 795),
        )
        .expect("overlapping rects must clip");
        assert_eq!(cropped_dst, rect(365, 187, 1190, 795));
        assert_eq!(cropped_src, src(16.0, 10.0, 1190.0, 795.0));
    }

    #[test]
    fn clip_handles_scaled_buffer() {
        // At output scale 2 the physical geometry is 2880x1800 but the buffer
        // source stays 1440x900; the crop must divide by the physical scale.
        let (cropped_src, cropped_dst) = clip_element_geometry(
            src(0.0, 0.0, 1440.0, 900.0),
            rect(786, 430, 2880, 1800),
            rect(786, 430, 2380, 1590),
        )
        .expect("overlapping rects must clip");
        assert_eq!(cropped_dst, rect(786, 430, 2380, 1590));
        assert_eq!(cropped_src, src(0.0, 0.0, 1190.0, 795.0));
    }

    #[test]
    fn clip_returns_none_for_no_overlap() {
        let result = clip_element_geometry(
            src(0.0, 0.0, 100.0, 100.0),
            rect(0, 0, 100, 100),
            rect(500, 500, 100, 100),
        );
        assert!(result.is_none());
    }

    #[test]
    fn clip_returns_none_for_zero_size_geometry() {
        let result = clip_element_geometry(
            src(0.0, 0.0, 0.0, 0.0),
            rect(0, 0, 0, 0),
            rect(0, 0, 100, 100),
        );
        assert!(result.is_none());
    }
}
