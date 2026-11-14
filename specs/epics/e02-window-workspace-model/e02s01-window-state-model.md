### Story e02s01: Window state model: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `wm::Window` already exists in `src/wm.rs` with surface, role, output, workspace, mode, rect, target_rect, focus, mapped, min/max/fullscreen, app_id, title. This story audits that implementation against the spec and adds test coverage for any uncovered behavior. No new fields or structs are added unless the audit finds a genuine gap.

## Steps

1. Audit `src/wm.rs` `Window` struct against SCOPE_LATEST.yaml fields -> verify: `cargo test --lib wm::tests`
2. Add unit test for `Window` min/max/fullscreen mode transitions if not covered -> verify: `cargo test --lib wm::tests`
3. Add unit test for `set_window_meta` mirroring app_id/title from xdg surface -> verify: `cargo test --lib wm::tests`

## Verification Script (Step-by-Step)

1. Run `cargo test --lib wm::tests` and confirm all window-state tests pass.
2. Confirm `Window` has all fields named in SCOPE_LATEST.yaml in `src/wm.rs`.

## Out of scope

- New `Window` fields not in SCOPE_LATEST.yaml
- Rendering or border drawing changes

## Risks

- Audit finds a field missing (e.g. role) -> add field + test in a follow-up task; do not block this story.

---
