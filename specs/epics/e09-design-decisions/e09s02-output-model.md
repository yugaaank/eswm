### Story e09s02: Output model: position, scale, work area, LayerMap per output; focused output

**type:** refactor  
**risk:** P3  
**context:** output  

**Context:** Two different "primary" outputs: wm.primary_output_id() is first output
(gets every new window, drives window_at/cycling/workspaces/dock);
state.primary_output is last connected output (gets every layer and dock geometry).
Every output sits at (0,0) but pointer bounds add widths together. Work area is one
global; output a layer surface asks for is ignored. HDMI plugged into laptop: bars
land on one screen, windows on another.

## Steps

1. Give each output position, scale, work area, LayerMap -> verify: `cargo build`
2. Track focused output; drop "primary" from placement -> verify: `cargo build`
3. Each output gets its own LayerMap; layer surfaces ask for specific output -> verify: `cargo build`
4. Pointer bounds use per-output positions -> verify: `cargo build`

## Verification Script

1. HDMI plugged into laptop: bars and windows land on correct outputs.
2. Each output has independent scale and work area.

## Out of scope

- Hotplug handling (existing Phase 1 work)

## Risks

- Removing "primary" from placement changes which output new windows go to.

---
