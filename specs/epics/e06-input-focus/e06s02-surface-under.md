### Story e06s02: surface_under with input regions for pointer focus

**type:** feat  
**risk:** P3  
**context:** input  

**Context:** pointer_focus hit-tests only WM windows, so bar/notch/panel clicks go
nowhere. Tacit's shell is all layer surfaces. Input regions are never checked; a
hit-test on geometry alone swallows every click. anvil's surface_under pattern is
the model.

## Steps

1. Write surface_under(pos) checking, in order: overlay layers; fullscreen window;
   top layers; active workspace windows front-to-back (popups/subsurfaces included);
   bottom and background layers -> verify: `cargo build`
2. Use layer_map_for_output(..).layer_under(..) and surface_under(..,
   WindowSurfaceType::ALL) for layers; under_from_surface_tree for windows; both
   honor input regions -> verify: `cargo build`
3. Pointer focus, click-to-focus and dock all call surface_under -> verify: `cargo build`

## Verification Script

1. Full-screen overlay layer with 100x100 input region at top left: click at (500,500)
   reaches window below; click at (50,50) reaches layer.

## Out of scope

- Touch input (pointer only)

## Risks

- Smithay layer_under / surface_under APIs may differ; check Smithay source.

---
