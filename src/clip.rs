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
//!
//! The clip rectangle is the window-geometry rect in output space. Client
//! surfaces render at `target_rect - xdg_offset`, so CSD content (which starts
//! at `xdg_offset` inside the surface) lands exactly on the clip origin; only
//! decoration drawn outside the frame is trimmed. See
//! `clip_offsets_src_when_geometry_starts_before_clip`, which pins that case
//! (BUG-2026-11-24-SIZE-NEGOTIATION review).

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

/// Convert a rectangle from the inner surface's local coordinates into the
/// clipped element's local coordinates. Smithay's damage tracker adds
/// `element.geometry().loc` to every region an `Element` returns, so the
/// wrapper must return regions relative to the *clipped* origin — not the
/// surface origin, and definitely not the output origin.
pub fn clip_local_region(
    region: Rectangle<i32, Physical>,
    inner_loc: Point<i32, Physical>,
    clip: Rectangle<i32, Physical>,
    outer_loc: Point<i32, Physical>,
) -> Option<Rectangle<i32, Physical>> {
    let mut absolute = region;
    absolute.loc += inner_loc;
    let mut local = absolute.intersection(clip)?;
    local.loc -= outer_loc;
    Some(local)
}

/// Stretch a surface element to the frame the compositor configured. A
/// fixed-size Wayland client cannot be forced to resize, so we scale its
/// pixels instead; the texture source stays untouched and only the destination
/// geometry, damage, and opaque regions grow or shrink. Stretching a sticker
/// over a wider box is inelegant, but it beats staring at an empty frame.
pub struct ScaledElement<E> {
    inner: E,
    scale: Scale<f64>,
}

impl<E: Element> ScaledElement<E> {
    pub fn new(inner: E, scale: Scale<f64>) -> Self {
        Self { inner, scale }
    }

    fn scale_region(&self, region: Rectangle<i32, Physical>) -> Rectangle<i32, Physical> {
        Rectangle::new(
            Point::from((
                (region.loc.x as f64 * self.scale.x).round() as i32,
                (region.loc.y as f64 * self.scale.y).round() as i32,
            )),
            Size::from((
                (region.size.w as f64 * self.scale.x).ceil() as i32,
                (region.size.h as f64 * self.scale.y).ceil() as i32,
            )),
        )
    }
}

impl<E: Element> Element for ScaledElement<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }

    fn transform(&self) -> smithay::utils::Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let inner = self.inner.geometry(scale);
        Rectangle::new(
            inner.loc,
            Size::from((
                (inner.size.w as f64 * self.scale.x).round() as i32,
                (inner.size.h as f64 * self.scale.y).round() as i32,
            )),
        )
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner
            .damage_since(scale, commit)
            .into_iter()
            .map(|damage| self.scale_region(damage))
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner
            .opaque_regions(scale)
            .into_iter()
            .map(|region| self.scale_region(region))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R, E> RenderElement<R> for ScaledElement<E>
where
    R: Renderer + ImportAll,
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
        self.inner.draw(frame, src, dst, damage, opaque_regions)
    }

    // A scaled buffer must go through the texture path; direct scanout would
    // paint the unscaled dmabuf over the frame.
    fn underlying_storage(&self, _renderer: &mut R) -> Option<smithay::backend::renderer::element::UnderlyingStorage<'_>> {
        None
    }
}

/// Wraps any render element and clips it to a fixed physical rectangle.
/// Always returns `None` for underlying storage so the DRM compositor cannot
/// bypass the clip with direct scanout. Generic over the inner element so the
/// coordinate transforms can be exercised without a live renderer — damage
/// bookkeeping is too easy to get subtly wrong to leave untested.
pub struct ClippedElement<E> {
    inner: ScaledElement<E>,
    clip: Rectangle<i32, Physical>,
    scale: Scale<f64>,
}

/// The production form: clip a Wayland surface tree element.
pub type ClippedSurfaceElement<R> = ClippedElement<WaylandSurfaceRenderElement<R>>;

impl<E: Element> ClippedElement<E> {
    /// Clip and stretch in one pass. `stretch` maps a mismatched buffer to the
    /// frame; the inner element is wrapped so its damage and opaque regions
    /// scale with the pixels the renderer will draw.
    pub fn with_stretch(
        inner: E,
        clip: Rectangle<i32, Physical>,
        scale: Scale<f64>,
        stretch: Scale<f64>,
    ) -> Self {
        Self { inner: ScaledElement::new(inner, stretch), clip, scale }
    }

    fn clipped_geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner
            .geometry(scale)
            .intersection(self.clip)
            .unwrap_or_else(|| Rectangle::from_size((0, 0).into()))
    }
}

impl<E: Element> Element for ClippedElement<E> {
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
        let inner_loc = self.inner.geometry(scale).loc;
        let outer_loc = self.geometry(scale).loc;
        self.inner
            .damage_since(scale, commit)
            .into_iter()
            .filter_map(|d| clip_local_region(d, inner_loc, self.clip, outer_loc))
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let inner_loc = self.inner.geometry(scale).loc;
        let outer_loc = self.geometry(scale).loc;
        self.inner
            .opaque_regions(scale)
            .into_iter()
            .filter_map(|r| clip_local_region(r, inner_loc, self.clip, outer_loc))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R, E> RenderElement<R> for ClippedElement<E>
where
    R: Renderer + ImportAll,
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

    struct FakeElement {
        id: Id,
        geometry: Rectangle<i32, Physical>,
        damage: Vec<Rectangle<i32, Physical>>,
        opaque: Vec<Rectangle<i32, Physical>>,
    }

    impl FakeElement {
        fn at(geometry: Rectangle<i32, Physical>) -> Self {
            Self {
                id: Id::new(),
                geometry,
                damage: Vec::new(),
                opaque: Vec::new(),
            }
        }

        fn with_damage(mut self, rect: Rectangle<i32, Physical>) -> Self {
            self.damage.push(rect);
            self
        }

        fn with_opaque(mut self, rect: Rectangle<i32, Physical>) -> Self {
            self.opaque.push(rect);
            self
        }
    }

    impl Element for FakeElement {
        fn id(&self) -> &Id {
            &self.id
        }

        fn current_commit(&self) -> CommitCounter {
            CommitCounter::default()
        }

        fn src(&self) -> Rectangle<f64, BufferCoords> {
            src(
                0.0,
                0.0,
                self.geometry.size.w as f64,
                self.geometry.size.h as f64,
            )
        }

        fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
            self.geometry
        }

        fn damage_since(
            &self,
            _scale: Scale<f64>,
            _commit: Option<CommitCounter>,
        ) -> DamageSet<i32, Physical> {
            self.damage.iter().copied().collect()
        }

        fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
            self.opaque.iter().copied().collect()
        }
    }

    #[test]
    fn clipped_damage_keeps_surface_local_rect_inside_frame() {
        // Smithay gives us damage relative to the surface origin (349,177),
        // adds back the *element* origin (365,187) itself, so the wrapper must
        // hand back a rect relative to the clipped origin. Getting this wrong
        // gives damage tracking amnesia and stale pixels — the compositor has
        // the memory of a goldfish with a hole punch.
        let scale = Scale::from(1.0f64);
        let clip = rect(365, 187, 1190, 795);
        let inner = FakeElement::at(rect(349, 177, 1222, 837))
            .with_damage(rect(0, 0, 1222, 837));
        let element = ClippedElement::with_stretch(inner, clip, scale, Scale::from(1.0f64));

        let damage: Vec<_> = element.damage_since(scale, None).iter().copied().collect();
        assert_eq!(damage, vec![rect(0, 0, 1190, 795)]);
    }

    #[test]
    fn clipped_opaque_region_maps_oversize_buffer_to_frame() {
        let scale = Scale::from(1.0f64);
        let clip = rect(393, 215, 1190, 795);
        let inner = FakeElement::at(rect(393, 215, 1440, 900))
            .with_opaque(rect(0, 0, 1440, 900));
        let element = ClippedElement::with_stretch(inner, clip, scale, Scale::from(1.0f64));

        let opaque: Vec<_> = element.opaque_regions(scale).iter().copied().collect();
        assert_eq!(opaque, vec![rect(0, 0, 1190, 795)]);
    }

    #[test]
    fn clipped_opaque_region_maps_partial_csd_region() {
        let scale = Scale::from(1.0f64);
        let clip = rect(365, 187, 1190, 795);
        let inner = FakeElement::at(rect(349, 177, 1222, 837))
            .with_opaque(rect(100, 50, 200, 100));
        let element = ClippedElement::with_stretch(inner, clip, scale, Scale::from(1.0f64));

        let opaque: Vec<_> = element.opaque_regions(scale).iter().copied().collect();
        assert_eq!(opaque, vec![rect(84, 40, 200, 100)]);
    }

    #[test]
    fn scaled_element_stretches_geometry_to_the_frame() {
        // Craft maps at 1440x900 inside an 1896x1078 tiled frame. The element
        // must report the frame size while keeping the surface origin.
        let scale = Scale { x: 1896.0 / 1440.0, y: 1078.0 / 900.0 };
        let inner = FakeElement::at(rect(12, 80, 1440, 900));
        let element = ScaledElement::new(inner, scale);
        let unit = Scale::from(1.0f64);
        assert_eq!(element.location(unit), Point::from((12, 80)));
        assert_eq!(element.geometry(unit), rect(12, 80, 1896, 1078));
        assert_eq!(element.src(), src(0.0, 0.0, 1440.0, 900.0));
    }

    #[test]
    fn scaled_element_scales_damage_and_opaque_regions() {
        let scale = Scale { x: 2.0, y: 0.5 };
        let inner = FakeElement::at(rect(0, 0, 100, 100))
            .with_damage(rect(10, 20, 30, 40))
            .with_opaque(rect(1, 2, 3, 4));
        let element = ScaledElement::new(inner, scale);
        let unit = Scale::from(1.0f64);

        let damage: Vec<_> = element.damage_since(unit, None).iter().copied().collect();
        assert_eq!(damage, vec![rect(20, 10, 60, 20)]);
        let opaque: Vec<_> = element.opaque_regions(unit).iter().copied().collect();
        assert_eq!(opaque, vec![rect(2, 1, 6, 2)]);
    }

    #[test]
    fn clipped_opaque_region_outside_frame_is_dropped() {
        let scale = Scale::from(1.0f64);
        let clip = rect(365, 187, 1190, 795);
        let inner = FakeElement::at(rect(100, 100, 100, 100))
            .with_opaque(rect(0, 0, 100, 100));
        let element = ClippedElement::with_stretch(inner, clip, scale, Scale::from(1.0f64));

        assert!(element.opaque_regions(scale).iter().next().is_none());
    }
}
