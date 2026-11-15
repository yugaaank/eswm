### Story e08s03: Scale, cursors, input settings

**type:** fix  
**risk:** P3  
**context:** correctness  

**Context:** Everything renders at scale 1.0; client cursors ignored (main.rs:366).
Touchpad settings and XKB keymap hardcoded.

## Steps

1. Store CursorImageStatus and draw client's cursor surface -> verify: `cargo build`
2. Store per-output scale; render at that scale -> verify: `cargo build`
3. Take touchpad settings from config -> verify: `cargo build`
4. Take XKB keymap from config -> verify: `cargo build`

## Verification Script

1. Client cursor renders correctly.
2. Output scale respected (HiDPI).
3. Config sets keymap and touchpad settings.

## Out of scope

- Fractional scale (future epic)

## Risks

- Per-output scale requires damage tracking for scale changes.

---
