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
