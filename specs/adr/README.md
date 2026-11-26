# Architecture Decision Records

One file per decision that would be expensive to rediscover: `ADR-NNNN-slug.md`.
An ADR records *why*, not *how* — the code and `specs/tech-architecture/` cover
the how. Supersede an ADR by writing a new one and marking the old one
`Superseded by ADR-NNNN`; never rewrite history in place.

Template:

```markdown
# ADR-NNNN: Title

- Status: Accepted | Superseded by ADR-NNNN | Rejected
- Date: YYYY-MM-DD
- Deciders: <who/which session>

## Context
What forced a decision.

## Decision
The choice, stated plainly.

## Consequences
What gets easier, what gets harder, what is now forbidden.
```

Index:

| ADR | Decision |
|-----|----------|
| [0001](ADR-0001-lua-only-configuration.md) | Configuration is Lua only; there is no JSON path |
| [0002](ADR-0002-drm-libseat-only-runtime.md) | DRM/libseat is the only shipped backend; the virtual output exists for tests |
| [0003](ADR-0003-inline-tests.md) | Tests live inline in `src/`, not in `tests/` |
| [0004](ADR-0004-popup-positioner-parity.md) | Popup placement follows Hyprland's positioner semantics |
| [0005](ADR-0005-specs-are-contracts.md) | `specs/` is a gated contract, checked by `scripts/spec-lint.py` |
| [0006](ADR-0006-shader-rounding-and-gpl.md) | GPL-3.0-only relicensing; COSMIC-style shader clipping of real client corners |
