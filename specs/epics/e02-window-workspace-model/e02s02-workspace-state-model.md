### Story e02s02: Workspace state model with mode/layout/focus history: Implementation Steps

**type:** feat  
**risk:** P1  
**context:** domain  

**Context:** `wm::Workspace` currently has `id`, `name`, `output`, `windows`, `focused`. The review model adds `mode` (Tiling/Floating), `layout` (Master/Dwindle/EqualSplit), `tiling_order` (Vec<WindowId>), `focus_history` (MRU Vec<WindowId>), and per-layout state (`master_ratio`/`master_count`/`master_position`, `dwindle_tree`, `equal_orientation`). All per-layout state is kept simultaneously and never discarded on layout switch.

## Steps

1. Add `mode: LayoutMode`, `layout: TilingAlgorithm`, `tiling_order: Vec<WindowId>`, `focus_history: Vec<WindowId>` to `Workspace` -> verify: `cargo build`
2. Add `master_ratio: f32`, `master_count: usize`, `master_position: MasterPos` (Master state), `dwindle_tree: DwindleTree` (Dwindle state), `equal_orientation: EqualOrient` (EqualSplit state) -> verify: `cargo build`
3. Add default workspace per output on startup; assign created windows to active workspace of their output -> verify: `cargo test --bin eswm wm::tests`
4. Unit test: focus_history is MRU; focus switch updates MRU -> verify: `cargo test --bin eswm wm::tests`
5. Unit test: per-layout state preserved across layout switches -> verify: `cargo test --bin eswm wm::tests`

## Verification Script (Step-by-Step)

1. Run `cargo test --bin eswm wm::tests` and confirm all workspace tests pass.
2. Confirm `Workspace` has mode, layout, tiling_order, focus_history, and per-layout state fields.

## Out of scope

- Multi-output active workspace sync beyond `active_workspace` per output (exists)
- Workspace creation from IPC (already via `workspace.focus` command)

## Risks

- `focus_history` must be updated on every focus change; missing an update causes stale MRU.

---
