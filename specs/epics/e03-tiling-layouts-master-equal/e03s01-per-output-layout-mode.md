### Story e03s01: Per-output wm mode + tiling layout config: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `wm::LayoutMode` existed with `Hyprland`/`Windows` variants but controlled spawn default (tiled vs floating). Renamed to `WmMode` (Tiling/Floating) for clarity. `Output` struct gained `tiling_layout: TilingLayout` field (Master/Equal). No keybind for toggling; wm mode and tiling layout are set via `es.dsp.set_wm_mode` / `es.dsp.set_tiling_layout` in `config.lua`, requiring logout/login to change.

## Steps

1. Rename `LayoutMode` enum to `WmMode` (Tiling/Floating) in `src/wm.rs` -> verify: `cargo build`
2. Add `TilingLayout` enum (`Master`, `Equal`) to `src/wm.rs` -> verify: `cargo build`
3. Add `tiling_layout: TilingLayout` field to `Output` struct, default `Master` -> verify: `cargo build`
4. Add `tiling_layout` field to `DecorationConfig` -> verify: `cargo build`
5. Add `WindowManager::set_tiling_layout` setting primary output's tiling_layout -> verify: `cargo build`
6. Sync `wm_mode` and `tiling_layout` from `DecorationConfig` to `WindowManager` in `apply_decoration_to_wm` -> verify: `cargo build`
7. Add `es.dsp.set_wm_mode` and `es.dsp.set_tiling_layout` Lua APIs in `src/lua_state.rs` -> verify: `cargo build`
8. Rename `toggle_layout_mode` keybind to `toggle_wm_mode` in `src/udev.rs`, then remove it (config-only) -> verify: `cargo test --bin eswm wm::tests`
9. Update `~/.config/eswm/config.lua`: `set_wm_mode("tiling")`, `set_tiling_layout("master")` -> verify: `test -f ~/.config/eswm/config.lua`
10. Update `config.default.lua` with renamed keybinds -> verify: `test -f config.default.lua`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all WM tests pass.
2. Confirm `~/.config/eswm/config.lua` contains `set_wm_mode` and `set_tiling_layout`.
3. Confirm no `toggle_wm_mode` or `toggle_layout_mode` keybind exists in `src/udev.rs`.

## Out of scope

- Keybind-based layout toggle (user decided: config-only, logout/login to change)
- Equal layout algorithm (e03s02)
- Wiring equal into `arrange_output` (e03s03)

## Risks

- `set_tiling_layout` sets the layout on the primary output only; multi-output per-output layout via Lua not yet supported.

---
