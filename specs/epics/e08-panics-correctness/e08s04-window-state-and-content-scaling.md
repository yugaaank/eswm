### Story e08s04: Window state flags and content scaling

**type:** fix
**risk:** P3
**context:** presentation

**Context:** `apply_layout` sends a size configure but never sets
`xdg_toplevel::State::Maximized` / `Fullscreen`, so a client cannot tell it is
maximized and keeps its own decorations/window state. A candidate patch in the
working tree also writes the *client's* `ViewportCachedState.dst` from the
compositor so the buffer scales into `target_rect`.

That viewport override was written before BUG-2026-11-19T235000 was found: the
real reason content "stayed small inside the frame" was that the titlebar path
never sent a configure at all. With configures fixed, a conforming client
resizes its buffer itself and the override may be dead weight — and setting
viewport destination on the client's behalf is outside the protocol's intent.

**Tasks**

1. Send `Maximized` / `Fullscreen` in the pending configure and clear them for
   other modes.
2. Prove on a DRM session that a client resizes into a maximized rect on a
   plain size configure (kitty, a GTK4 app, and one Xwayland-free toolkit).
3. Keep the viewport override only if a real client still under-fills; if kept,
   document why and confine it to the surface the compositor owns. Otherwise
   delete it.

**Out of scope**

- Fractional scale (`wp_fractional_scale_v1`) — stays deferred.
- Client-side decoration negotiation.

**Risks**

- Removing the override could regress a client that ignores size configures.
  Task 2 exists to catch exactly that before deletion.
