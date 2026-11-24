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
}
