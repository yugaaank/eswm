---
bug_id: BUG-2026-11-24-DOCK-RESTORE
status: open
severity: high
scope: input
title: Clicking a minimized window's dock icon does not restore it
---

## Symptom
Minimize a window, then left-click its dock icon: the window stays hidden.

## Reproduction
1. Focus a window, `Super+M` (or right-click dock → minimize).
2. Left-click its dock icon.
3. Expected: window reappears and takes focus. Actual: window stays hidden.

## Root cause
`WindowManager::focus_window_or_restore()` (`src/wm.rs:1316`) decides whether to
restore by reading `window.mapped`. Minimizing never clears `mapped` — it sets
the separate `minimized` flag (`src/wm.rs:1276-1291`, test `src/wm.rs:2586-2602`).
So the restore branch never runs and `minimized` remains `true`; `placed_windows`
(`src/main.rs:294`) and `surface_under` (`src/wm.rs:968`) keep skipping it.

Secondary: the dock click handler (`src/udev.rs:1861-1866`) does not call
`apply_layout` after `focus_window_or_restore`, so even a restored window would
not get a fresh configure / Activated state from this path.

## Verify steps (TDD)
- RED test: minimized window → `focus_window_or_restore` → `minimized == false`,
  `focused == true`, `focused_id() == Some(id)`.
- GREEN: use `was_minimized` as the restore trigger; clear the flag; arrange the
  output so layout/configure runs; call `apply_layout` from the dock handler.
- Regression: dock entry state flips from minimized to active; repeated clicks
  behave as restore/focus, not toggle.

## Resolution
Pending.
