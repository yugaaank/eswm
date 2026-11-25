//! Rounded window corners via baked alpha in compositor-drawn textures.
//!
//! Hyprland rounds windows in a fragment shader (`rounding.glsl`), but
//! Smithay 0.7's `render_elements!` macro requires every element to implement
//! `RenderElement<R>` for all renderers, which prevents injecting a GLES-only
//! shader override per element without forking smithay. Instead, eswm bakes
//! the corner mask into the alpha channel of the textures it already draws:
//! the titlebar background and the border frame. The client surface stays
//! square, but it is inset inside the rounded frame, so the visible outline
//! is rounded. No GLES-specific code, no shader, no damage-tracking changes.
//!
//! Corner policy (operator spec, 2026-11-24):
//! - titlebar present: round the titlebar's two top corners and the window's
//!   two bottom corners. The surface top is hidden behind the opaque titlebar.
//! - no titlebar: round all four corners.
//! - fullscreen windows and popups are never rounded.
//! - `corner_radius == 0` disables rounding entirely.

/// Radii for the four corners, in pixels. Independent so a titlebar can round
/// only its top edge while the client below rounds only its bottom edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Corners {
    pub tl: u32,
    pub tr: u32,
    pub bl: u32,
    pub br: u32,
}

impl Corners {
    pub fn all(r: u32) -> Self {
        Self { tl: r, tr: r, bl: r, br: r }
    }
}

/// Which corners of a window are rounded for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RingPlan {
    /// Outer silhouette of the border ring.
    pub outer: Corners,
    /// Inner hole of the ring; paints over the client's square corners.
    pub inner: Corners,
    /// Titlebar background; only its top corners are rounded.
    pub titlebar: Corners,
}

/// Compute the corner policy from config and window state.
pub fn ring_plan(
    radius: u32,
    border_size: u32,
    titlebar_height: u32,
    fullscreen: bool,
    popup: bool,
) -> RingPlan {
    if radius == 0 || fullscreen || popup {
        return RingPlan::default();
    }
    let Some(inner_radius) = radius.checked_sub(border_size) else {
        return RingPlan::default();
    };
    let mut inner = Corners::all(inner_radius);
    // A titlebar shorter than the arc owns the top of the hole: clamp both
    // sides to the same limit so the titlebar and the ring meet seamlessly.
    if titlebar_height > 0 && inner_radius > titlebar_height {
        inner.tl = titlebar_height;
        inner.tr = titlebar_height;
    }
    let titlebar = if titlebar_height > 0 {
        Corners { tl: inner.tl, tr: inner.tr, bl: 0, br: 0 }
    } else {
        Corners::default()
    };
    RingPlan { outer: Corners::all(radius), titlebar, inner }
}

/// Coverage of pixel `(x, y)` in a `width x height` rectangle with independent
/// corner radii. Returns 0 outside the rounded shape, 255 well inside, and a
/// linear 2px antialias band at each arc — the baked equivalent of Hyprland's
/// `smoothstep` in `rounding.glsl`.
pub fn rounded_alpha(x: i32, y: i32, width: i32, height: i32, c: Corners) -> u8 {
    if x < 0 || y < 0 || x >= width || y >= height {
        return 0;
    }
    let left = x < width / 2;
    let top = y < height / 2;
    let r = match (left, top) {
        (true, true) => c.tl,
        (false, true) => c.tr,
        (true, false) => c.bl,
        (false, false) => c.br,
    };
    if r == 0 {
        return 255;
    }
    // Distance from the arc's centre, mirrored into the corner quadrant.
    let cx = if left { r as i32 } else { width - 1 - r as i32 };
    let cy = if top { r as i32 } else { height - 1 - r as i32 };
    let dx = x - cx;
    let dy = y - cy;
    let outside_x = if left { dx < 0 } else { dx > 0 };
    let outside_y = if top { dy < 0 } else { dy > 0 };
    if !outside_x || !outside_y {
        return 255;
    }
    let dist = ((dx as f64).powi(2) + (dy as f64).powi(2)).sqrt();
    let r = r as f64;
    if dist <= r - 1.0 {
        255
    } else if dist >= r + 1.0 {
        0
    } else {
        ((r + 1.0 - dist) / 2.0 * 255.0).round().clamp(0.0, 255.0) as u8
    }
}

/// Build an ARGB byte slice for a filled rounded rectangle. Bytes are ordered
/// `[R, G, B, A]` because the buffers upload with `Fourcc::Abgr8888`, which
/// smithay maps to GL `RGBA`. RGB is premultiplied by the final alpha.
pub fn rounded_rect_pixels(
    width: i32,
    height: i32,
    color: [u8; 4],
    corners: Corners,
) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mask = rounded_alpha(x, y, width, height, corners);
            let a = color[3] as u16 * mask as u16 / 255;
            pixels.extend_from_slice(&[
                (color[0] as u16 * a / 255) as u8,
                (color[1] as u16 * a / 255) as u8,
                (color[2] as u16 * a / 255) as u8,
                a as u8,
            ]);
        }
    }
    pixels
}

/// Build one border ring: opaque border colour on the straight edges, a
/// transparent hole for the titlebar and client, and border-coloured wedges
/// over their square corners. The outer silhouette follows `outer`; the hole
/// follows `inner`. Bytes are `[R, G, B, A]`, premultiplied.
#[allow(clippy::too_many_arguments)]
pub fn rounded_border_pixels(
    width: i32,
    height: i32,
    hole_x: i32,
    hole_y: i32,
    hole_width: i32,
    hole_height: i32,
    color: [u8; 4],
    outer: Corners,
    inner: Corners,
) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let outer_mask = rounded_alpha(x, y, width, height, outer);
            let in_hole = x >= hole_x
                && x < hole_x + hole_width
                && y >= hole_y
                && y < hole_y + hole_height;
            let mask = if in_hole {
                // Inside the hole, cover only the part its own rounded mask
                // clips away — that is the wedge over a square client corner.
                // The wedge must stay inside the outer silhouette too: with a
                // thin border the hole's corner sits further from the arc
                // centre than the outer arc does, so an unclipped wedge
                // pokes out past the arc and squares the corner off again.
                // Same trap as painting a round room's corners with a square
                // brush.
                outer_mask.min(255u8.saturating_sub(rounded_alpha(
                    x - hole_x, y - hole_y, hole_width, hole_height, inner,
                )))
            } else {
                outer_mask
            };
            let a = color[3] as u16 * mask as u16 / 255;
            pixels.extend_from_slice(&[
                (color[0] as u16 * a / 255) as u8,
                (color[1] as u16 * a / 255) as u8,
                (color[2] as u16 * a / 255) as u8,
                a as u8,
            ]);
        }
    }
    pixels
}

/// Identity of a cached decoration buffer. `scale` is part of the key so a
/// monitor-scale change rebuilds the rasterised arc instead of stretching a
/// buffer made for the old scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DecorationBufferKey {
    pub window: u64,
    pub width: i32,
    pub height: i32,
    pub radius: u32,
    pub border: u32,
    pub titlebar: u32,
    pub color: [u8; 4],
    pub scale: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_plan_rounds_titlebar_top_and_window_bottom() {
        // Titlebar present: the titlebar is rounded at the top only, and the
        // client's visible bottom corners follow the ring's bottom arc.
        let plan = ring_plan(12, 2, 24, false, false);
        assert_eq!(
            plan.outer,
            Corners { tl: 12, tr: 12, bl: 12, br: 12 }
        );
        assert_eq!(
            plan.inner,
            Corners { tl: 10, tr: 10, bl: 10, br: 10 }
        );
        assert_eq!(plan.titlebar, Corners { tl: 10, tr: 10, bl: 0, br: 0 });
    }

    #[test]
    fn ring_plan_rounds_all_four_without_titlebar() {
        let plan = ring_plan(12, 2, 0, false, false);
        assert_eq!(
            plan.inner,
            Corners { tl: 10, tr: 10, bl: 10, br: 10 }
        );
        assert_eq!(plan.titlebar, Corners::default());
    }

    #[test]
    fn buffer_cache_key_tracks_output_scale() {
        let base = DecorationBufferKey {
            window: 1, width: 100, height: 40, radius: 12,
            border: 2, titlebar: 24, color: [200, 200, 200, 255], scale: 1,
        };
        assert_eq!(base, DecorationBufferKey { scale: 1, ..base });
        assert_ne!(base, DecorationBufferKey { scale: 2, ..base });
        assert_ne!(base, DecorationBufferKey { radius: 8, ..base });
    }

    #[test]
    fn corner_radii_are_independent() {
        // A titlebar mask: top corners rounded, bottom corners square.
        let c = Corners { tl: 5, tr: 5, bl: 0, br: 0 };
        assert_eq!(rounded_alpha(0, 0, 20, 20, c), 0);
        assert_eq!(rounded_alpha(19, 0, 20, 20, c), 0);
        assert_eq!(rounded_alpha(0, 19, 20, 20, c), 255);
        assert_eq!(rounded_alpha(19, 19, 20, 20, c), 255);
    }

    #[test]
    fn rounded_alpha_keeps_the_centre_and_straight_edges_opaque() {
        let c = Corners::all(5);
        assert_eq!(rounded_alpha(10, 10, 20, 20, c), 255);
        assert_eq!(rounded_alpha(10, 0, 20, 20, c), 255);
        assert_eq!(rounded_alpha(0, 10, 20, 20, c), 255);
    }

    #[test]
    fn rounded_rect_pixels_are_premultiplied_rgba() {
        // An antialiased pixel must store rgb * a / 255, not raw rgb: smithay
        // blends with ONE, ONE_MINUS_SRC_ALPHA.
        let px = rounded_rect_pixels(20, 20, [200, 100, 50, 255], Corners::all(5));
        let a = rounded_alpha(1, 1, 20, 20, Corners::all(5));
        assert!(a > 0 && a < 255, "expected a partial pixel, got {a}");
        let idx = 21 * 4;
        assert_eq!(px[idx + 3], a);
        assert_eq!(px[idx], (200u16 * a as u16 / 255) as u8);
        assert_eq!(px[idx + 1], (100u16 * a as u16 / 255) as u8);
        assert_eq!(px[idx + 2], (50u16 * a as u16 / 255) as u8);
    }

    #[test]
    fn rounded_border_pixels_paints_client_corner_and_leaves_hole_clear() {
        // 20x20 frame with a 12x12 hole at (4,4): the ring is 4px thick.
        let px = rounded_border_pixels(
            20, 20, 4, 4, 12, 12, [255, 0, 0, 255], Corners::all(6), Corners::all(2),
        );
        let at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 20 + x) * 4;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        // Centre of the hole shows the client: fully transparent.
        assert_eq!(at(10, 10), [0, 0, 0, 0]);
        // Straight border edge is opaque border colour.
        assert_eq!(at(2, 10), [255, 0, 0, 255]);
        // Inside the hole but outside its rounded mask is the wedge that
        // covers the client's square corner. The exact arc pixel is
        // antialiased; one step further out is fully opaque border colour.
        let corner = at(4, 4);
        assert_eq!(corner[1], 0);
        assert_eq!(corner[2], 0);
        assert!(corner[3] > 0, "wedge must paint the client corner: {corner:?}");
        // The straight part of the ring is fully opaque.
        assert_eq!(at(2, 4), [255, 0, 0, 255]);
        // Outside the outer arc: transparent.
        assert_eq!(at(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn thin_border_wedge_cannot_poke_outside_the_arc() {
        // Operator config: radius 12 over a 1px border. The hole's square
        // corner sits further from the arc centre than the outer arc does, so
        // an unclipped wedge spills past the silhouette and squares the corner
        // off again -- rounded arcs with square corners still showing.
        let (w, h, bs, tb, r) = (90i32, 50i32, 1i32, 20i32, 12u32);
        let fw = w + 2 * bs;
        let fh = h + tb + 2 * bs;
        let px = rounded_border_pixels(
            fw, fh, bs, bs, w, h + tb, [255, 0, 0, 255],
            Corners::all(r), Corners::all(r - bs as u32),
        );
        let alpha = |x: i32, y: i32| px[((y * fw + x) * 4 + 3) as usize];
        for y in 0..fh {
            for x in 0..fw {
                if rounded_alpha(x, y, fw, fh, Corners::all(r)) == 0 {
                    assert_eq!(
                        alpha(x, y), 0,
                        "({x},{y}) is outside the outer arc but still painted",
                    );
                }
            }
        }
        // The corner border must still close over the client's square corner.
        assert!(alpha(4, 4) > 0, "corner border must cover the client corner");
        // ...while the square corner tip itself stays cut away.
        assert_eq!(alpha(1, 1), 0, "corner tip must be rounded away");
    }

    #[test]
    fn rounded_alpha_fades_across_the_arc() {
        let c = Corners::all(5);
        // The 2px band around the arc must contain partial coverage, not a
        // binary jump from opaque to transparent.
        let mut partial = false;
        for y in 0..6 {
            for x in 0..6 {
                let a = rounded_alpha(x, y, 20, 20, c);
                if a > 0 && a < 255 { partial = true; }
            }
        }
        assert!(partial, "corner arc must have antialiased pixels");
    }
}
