---
bug_id: BUG-2026-11-24-SUPER-DRAG-LEAK
status: fixed
severity: high
scope: input
title: Super+drag move/resize leaks button and motion events to the client
---

## Symptom
In floating mode, `Super+drag` moves the window but the app also receives the
click and drag: e.g. in a file manager it starts selecting files/folders.

## Root cause
The pointer button handler starts a compositor `DragState` (`src/udev.rs:1778-1794`,
titlebar drag `:1795-1823`) but still forwards the initiating `pointer.button`
press to the focused client at the bottom of the handler (`:1909-1919`). The
matching release is forwarded too (`:1903-1907`), and `pointer_focus()`
(`:1408-1418`) deliberately routes motion to the dragged window while drag is
active. The client therefore sees a complete press/motion/release sequence and
runs its own drag-selection alongside eswm's window move.

## Verify steps (TDD)
- RED test: `drag_consumes_button` — only the drag's own button is consumed.
- GREEN: swallow the initiating press and matching release; route motion to no
  surface while a compositor drag is active.
- Regression: popup/dock clicks and non-drag buttons still reach clients.
- Full preflight, then operator live-DRM UAT in a file manager.

## Resolution
**Fixed:** 2026-11-24 on branch `fix/super-drag-leak` (RED `88a63fb`, GREEN `ac0314b`),
local merge pending.
**Root cause:** the compositor started a `DragState` but still delivered the
initiating button press, matching release, and motion to the client, so the app
ran its own drag-selection while eswm moved the window.
**Fix:** `drag_consumes_button()` swallows the drag's own press and release;
`pointer_focus()` returns `None` and clears pointer focus while a compositor
drag is active, so motion cannot leak to the client either. Other buttons,
dock clicks, popups and ordinary clicks still reach clients.
**Hardening:** RED test `super_drag_consumes_only_its_own_button` asserts only
the drag button is consumed (other buttons and no-drag cases pass through).
**Evidence:** `cargo clippy --all-targets && cargo test (198 passed) && cargo build`
green (`/tmp/opencode/super-drag-leak-preflight.log`).
**Live DRM UAT:** pending operator — in floating mode, `Super+drag` over a file
manager must move the window without selecting files/folders; ordinary clicks
and right-click menus must still work.
