# ADR-0004: Popup placement follows Hyprland's positioner semantics

- Status: Accepted
- Date: 2026-11-19
- Deciders: eswm maintainer, e06-input-focus

## Context

`xdg_positioner` is under-specified in practice. The protocol defines anchor
rect, anchor, gravity, offset, constraint adjustment, flip and slide; it does
not say how aggressively a compositor should push a popup back inside the work
area, nor which order flip/slide/resize apply. Clients (GTK4 menus, fuzzel,
qt) are tuned against the compositors people actually run. Getting this "close
enough by the spec" produces menus that hang off screen edges or jump when they
meet a panel — BUG-2026-11-18T160939 was exactly that: only the offset was
applied, so the anchor/gravity half of the request was ignored.

Tacit is being ported from a Hyprland session, so matching Hyprland is the
cheapest way to make the migration invisible to users.

## Decision

Popup geometry is resolved the way Hyprland's `CXDGPositionerRules::getPosition`
does it: compute the unconstrained rect from anchor rect + anchor + gravity +
offset, then apply flip/slide/resize against the output work area, in that
order. The result is recomputed every frame from the parent's current geometry,
so a popup follows its window.

Smithay exposes this as `get_unconstrained_geometry`; `src/main.rs` wraps it in
`popup_constraint()` / `popup_geometry()` and sends the result in the initial
popup configure.

## Consequences

- Menus land where a Hyprland user expects them, including at screen edges and
  next to layer-shell bars.
- Popup rects are parent-relative and recomputed per frame; no stale cached
  position to invalidate.
- Behaviour is pinned by `popup_positioner_*` tests. Changing the resolution
  order means changing those tests deliberately, not by accident.
- Diverging from Hyprland for a future protocol improvement is allowed, but it
  is a new ADR because it changes what users see in every app.
