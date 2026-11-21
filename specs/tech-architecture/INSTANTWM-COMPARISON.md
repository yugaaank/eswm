# eswm vs instantWM — deep comparison

**Date:** 2026-11-22
**Scope:** static analysis of `/home/yugaaank/Projects/eswm/instantWM` (v0.8.1, ~128.9k LOC, 489 source files) against eswm (v0.1.0, ~10.2k LOC, 7 source files). No source was copied; instantWM is reference-only per eswm AGENTS.md.
**Purpose:** inform e09 (design decisions: window model, output model, IPC, Lua config) and the longer roadmap before Tacit integration.

---

## 0. Executive summary

| Dimension | eswm | instantWM | Verdict |
|---|---|---|---|
| Maturity | Early (v0.1.0, 144 tests) | Mature (v0.8.1, ~1,359 tests, 2 fuzz systems) | instantWM far ahead in depth/testing |
| Backends | DRM/libseat only (virtual output for tests) | X11 + Wayland (DRM + nested winit) behind capability traits | instantWM's abstraction is a real e09/e10 reference |
| Config | Lua-only, ~40 setters, no validation on some paths, stubs | TOML-only, typed schema, includes, validation, runtime edits | **eswm's Lua wins on expression; instantWM wins on safety/discoverability** |
| Actions | String names matched in match statements; closed list | Macro-generated typed enum + registry, shared by keybinds/hooks/IPC/CLI | **instantWM clearly better** — strong e09s04 input |
| Layout | Master + Equal, recomputed each time | Persistent weighted n-ary tree with 3 presets, manual edits, per-tag slots, keyboard/pointer placement | instantWM much richer; eswm simpler/smaller |
| Workspaces | Workspaces 1..9 per output, one focused window per output | Tags as bitmasks (multi-tag views, sticky, scratchpad), per-monitor ownership | instantWM more expressive; eswm's e09s01/s02 already target the gaps |
| Protocols | xdg-shell, layer-shell, data-device, viewporter, decoration, xdg-output | + screencopy, session-lock, ext-workspace, foreign-toplevel, output-management/power, idle-inhibit, ext-image-capture, commit-timing, syncobj, Xwayland, text-input, gestures, virtual devices | **instantWM far ahead** |
| Rendering | Direct GLES render loop, manual element bands, dirty/redraw/frame-in-flight gates | Scene graph with shared bands, SceneCache, animations, VRR policy, damage sets, split-GPU | instantWM ahead; eswm's e05–e08 fixed the worst pathologies |
| IPC | hyprctl-compatible text commands + `event>>data` stream, JSON snapshot | Typed bincode over Unix socket, content-hashed protocol version, epoll readiness, JSON client-side only | **different goals**: eswm's text/JSON is better for Tacit; instantWM's hardening is better engineering |
| Status bar | External (waybar/quickshell via layer-shell) | Built-in compositor bar with i3bar protocol + SNI tray + clicks | eswm deliberately delegates — right call for Tacit |
| Testing | 144 inline tests, no fuzzing, no coverage tooling | ~1,359 tests, compile-fail borrow contract, E2E script, two fuzz systems | instantWM far ahead; eswm's recent RED/GREEN discipline is healthy but small |
| Code style | Long files (`udev.rs` ~3.9k lines), some stubs | Heavily modular (489 files), extensive tests, some `//BOZO:` self-critique | instantWM better structured; eswm's "grep-unique short names" convention comparable |

**Bottom line:** instantWM is a feature-rich, production-minded compositor with a mature WM policy layer; eswm is a deliberately narrow Tacit-first compositor. The most valuable things for eswm to borrow are **not features** but *design patterns*: typed action registry, declarative validated config conversion, per-output model, persistent z-order projection, and test architecture. The e09 stories already independently converged on several of these.

---

## 1. Architecture & backend abstraction

### instantWM
- `src/backend/mod.rs` defines narrow capability traits: `WindowOps`, `PointerOps`, `InteractionProjectionOps`, `WindowCloseOps`, `LayoutInteractionOps`, `OutputOps`, `OutputPolicyOps`, `BackendState`.
- `WmCtx` is an enum of `WmCtxX11` / `WmCtxWayland` wrappers; traits implemented on wrappers (`contexts.rs`, `backend/wayland/mod.rs:302-505`).
- `WaylandBackend<'a>` is a transparent `&mut WaylandState` newtype; disjoint field borrowing instead of `RefCell`/mutex. A **compile-fail UI test** (`tests/borrow_contract.py`, `tests/ui/native_borrow.rs`) asserts exact error counts `{E0499:5, E0502:2, E0599:1, E0596:1}` — an ownership-architecture contract.
- Shared services below WM: output transactions (`backend/output.rs`), deterministic maximum-matching CRTC allocation (`output/allocation.rs`), output intent across replugs (`output/intent.rs`).
- Caveats: `set_border_width` is a no-op on Wayland (borders compositor-rendered); X11 and Wayland share *policy* but not input/rendering/protocols.

### eswm
- Single `State` in `main.rs` owns `WindowManager`, `UdevData`, seat, and all protocol state. `src/{control,ipc,lua_state,udev,wm}.rs`.
- DRM/libseat only (`udev.rs`); virtual output exists only for tests (ADR-0002). No abstraction traits — switching backends would require restructuring.
- Everything runs in one thread with `Arc<Mutex<...>>` for shared config snapshots (`decoration`, `input_config`, `dock_entries`).

### Assessment
- **better in them:** genuine backend-neutral policy layer; compile-enforced borrow architecture; reusable output transaction/matching services.
- **better in us:** much smaller surface, one code path to reason about, no cross-backend feature asymmetry; the single-threaded `Arc<Mutex>` design is pragmatic for a one-backend compositor (e09s04 already questions the needless lock).
- **borrow for e09:** e09s01/s02 (split window state, per-output model) are exactly the kind of separation instantWM does well. The output transaction/matching code is an e10 (multi-monitor) reference, not e09 scope.

---

## 2. Configuration & actions

### instantWM — TOML + typed action registry
- Single `$XDG_CONFIG_HOME/instantwm/config.toml`, `serde` + `toml`, `#[serde(default)]` + `deny_unknown_fields` per section, explicit `validated()` per section with field-name-bearing errors.
- `includes` with `~/` expansion, cycle detection, arrays accumulate, includer wins. Theme presets (`catppuccin-mocha` etc.) resolve to base colors with overlays.
- Keybinds: `[[keybinds]] modifiers=["super"] key="Return" action={spawn=["kitty"]}`; `action = "none"` removes defaults; XKB keysym lookups; invalid entries logged and skipped, not fatal.
- `define_named_actions!` macro generates ~90 typed action variants from one table: variant + wire name + doc + arg type + overview policy + run closure. Emits `parse()`, `args()` (round-trip), `execute()`, `action_infos()` for `--list-actions`.
- One vocabulary for keybinds, hooks, CLI convenience commands, and IPC `RunAction`.
- Two reload paths: full reload validates before mutating (bad config leaves live state untouched); runtime `config set` uses serde-JSON reflection with typed validation, returns `ConfigEffect` applied in one place. Runtime edits are **not persisted** (reload discards them).

### eswm — Lua-only
- `~/.config/eswm/config.lua` via `mlua` (Lua 5.4). `es.bind`, `es.bind_cmd`, `es.define_submap`, `es.dsp.*` (~40 setters incl. new `set_output_scale`, `set_keymap`, touchpad), `es.dock.*`, `es.config{}`.
- Config can compute, loop, call functions, read files — far more expressive than any declarative format. Product-goal locked (AGENTS.md "Never add JSON config").
- Weaknesses documented in e09s04: mixed setters/actions in `es.dsp`, closed string action list, stubs that lie (`define_submap`, `global`, `config.*`, `safe_require`, `is_file_exists` always false), no real reload, needless `Arc<Mutex>` locking.

### Assessment
- **better in them:** typed actions with typed args and round-tripping; validation before mutation; includes; auto-generated action listing; one action namespace across all frontends; runtime edits with typed effects.
- **better in us:** Lua expressiveness (conditionals, functions, dynamic config) — impossible in their TOML model; direct compatibility with Tacit's existing ~1,900-line `hl.config{}` style config; no third-party config crate.
- **borrow for e09s04:** keep Lua but adopt their *shape*: `es.config{}` declarative table, `es.bind(keys, fn, opts)` with function/action duality, a generated/typed action catalog (Lua-side validation of action names and args), real reload with validate-before-apply, and replace stubs with honest errors. Do **not** replace Lua with TOML.

---

## 3. Window model, layout, tags/z-order

### instantWM
- `ClientMode { placement: Tiling|Floating, presentation: Normal|Maximized|Fullscreen{...} }` — orthogonal persistent policy vs temporary state (e09s01 problem statement mirrors this).
- `TagMask: u32` bitmask — a client can be on multiple tags; a monitor can view multiple tags. Sticky and scratchpad are reserved bits/roles. Per-tag state (`PerTagState`) keyed by exact visible mask holds `LayoutTree`, presentation, master count, bar visibility, stored trees.
- `LayoutTree`: persistent weighted n-ary tree (`Split{axis, children: WeightedNode}`, `Node::Window|Split`), canonicalized, with 3 presets (master/stack, sqrt grid, bottom-stack), per-tag stored slots that survive switches, manual edits, pointer/keyboard placement sessions, snap, scratchpads.
- `ClientZOrder` persistent per-monitor bottom-to-top vector; `sync_z_order` projects into layers `tiled → bars → floating → fullscreen → above → transients → overview` without mutating persistent order.
- `Monitor` owns its clients (no monitor field on client); transfers via `MonitorManager::transfer_client`.

### eswm
- `WindowMode` is a single enum mixing layout and state (`Tiled|Floating|Maximized|Fullscreen`), plus `mapped` doubling as minimized, bool focus cleared per output, global `work_area`, one "primary" output chosen two different ways. These are exactly e09s01/s02 problem statements.
- `TilingLayout: Master|Equal`, recomputed in `arrange_output`; `dirty` and `redraw_needed` gates; server-side border/titlebar rendering with `SolidColorRenderElement`; dock drawn as an element band.
- Workspaces 1..9 per output, one focused window; simple `remove_window`/`focus_window` flows; z-order is essentially `workspace.windows` iteration order.

### Assessment
- **better in them:** orthogonal state model, multi-tag freedom, persistent per-view layout trees, layered z-order projection, per-monitor client ownership, mature drag/drop.
- **better in us:** tiny, predictable tiler; easy to reason about and test; server-side titlebar with clickable buttons (instantWM has no traditional titlebars — the internal bar doubles as a tab strip); dock as a render band.
- **borrow for e09:** e09s01 should split layout vs state flags like their `ClientMode`; e09s02 should adopt per-output geometry/scale/work area/LayerMap and a single notion of focused output. e09 does **not** need the full tree or tag bitmask — keep master/equal, but make the *model* correct first. Persistent z-order projection is a cheap idea worth noting for a later story.

---

## 4. Wayland protocols & rendering

### instantWM
- Broad protocol set: xdg-shell/decoration/dialog/activation/foreign, layer-shell, screencopy + ext-image-capture, ext-session-lock, ext-workspace, wlr-foreign-toplevel, wlr-output-management + power, idle-inhibit + ext-idle-notify, keyboard-shortcuts-inhibit, gestures/constraints/relative-pointer/pointer-warp, virtual keyboard/pointer, text-input/input-method, tablet, viewporter, fractional-scale, presentation-time, commit-timing, FIFO, linux-drm-syncobj, xwayland-shell, data-control.
- DRM: split render/scanout GPUs, explicit sync, CRTC maximum-matching, in-flight-flip deferral, rollback on KMS failure, MST re-probe staging, mirrors as first-class projected outputs, VRR hard blockers.
- Rendering: GLES, scene bands assembled by a shared macro (`overlay → windows → bar → borders → bottom layers`), `SceneCache`, animations driven from presented (not logical) state, damage sets per scanout.

### eswm
- Protocols actually implemented: xdg-shell (+ popup lifecycle from e06), layer-shell, data-device/primary-selection/ext-data-control, viewporter (global only until e08s04 analysis), xdg-decoration, xdg-output, plus basic cursor and Dmabuf feedback.
- DRM: libseat session, GBM allocator, DrmOutputManager, one `frame_in_flight` gate and `redraw_needed`/`dirty` flags (the e05/e08 stutter fixes), output scale from config (e08s03), fullscreen band ordering (e08s02).
- Rendering: direct `render_surface()` with manual vectors (`cursor_elements`, `fullscreen_elements`, `layer_elements_above`, `dock_elements`, `elements`, `layer_elements_below`) assembled by `render_band_order`.

### Assessment
- **better in them:** protocol breadth (session lock, screencopy, output management, explicit sync), robust hotplug/VRR, scene caching, animations.
- **better in us:** our render path, while long, is linear and now well-tested; e08 fixed the pathological re-render/callback deadlocks with focused tests. Their DRM init is explicitly marked fragile ("random ass segfaults").
- **borrow later (not e09):** screencopy/session-lock for Tacit shell, output-management for display settings, scene-cache ideas when rendering becomes a bottleneck. Do not chase protocol parity now.

---

## 5. Input

### instantWM
- Keyboard generic over `InputBackend` (physical + virtual), per-device `KeyboardSource` interception, suppression priority (`SessionLocked > Grab > Inhibitor > ExclusiveLayer`), VT switching before grabs, modifier→X11 mask reuse, and a **recovery gesture** (Logo+Shift+Escape held 2s breaks grabs/inhibitors with progress UI).
- Pointer: constraints (`Locked`/`Confined`), gestures, warps deferred to tick so protocol focus and rendered position agree, virtual pointer.
- Touch (824 lines): mapping targets, compositor claim before native delivery, pointer emulation, slot bookkeeping.
- Cursor: `ResolvedCursor::{Hidden, Named, Surface, DndIcon}`, Xcursor theme with animation frames and fallback, client surface hotspots, XSettings projection to XWayland.

### eswm
- Keyboard: xkb keysym parsing at bind time, raw-sym matching, Caps/Num-Lock normalization, always-on emergency quit + VT switch, symmetric press/release interception (e07).
- Pointer: libinput with config-driven tap/natural-scroll/speed (e08s03), focus/hit-test with input regions (e06), titlebar button hit-testing (BUG fix).
- Cursor: Xcursor images + client surfaces with hotspots (e08s03), `Hidden`/`Named`/`Surface` policy helper.

### Assessment
- **better in them:** grab/inhibitor recovery, input-method correctness, touch, constraint/warp discipline, cursor theme animation.
- **better in us:** our keybind parsing/matching is layout-robust and tested; config-driven touchpad/keymap is simpler. Emergency quit is hardcoded and cannot be broken by config.
- **borrow for e09:** none directly; recovery gesture and touch are later-story candidates.

---

## 6. IPC

### instantWM
- Unix socket, **bincode** request/response, client-side JSON presentation only. epoll readiness fd covering listener + accepted clients, 1 MiB request cap, 128 clients, 30s idle timeout, never blocks render loop. Protocol version = hash of IPC schema sources + git commit (generated in `build.rs`), mismatch rejected unless overridden. CLI generated from the same clap enums as wire types.

### eswm
- Unix socket, **hyprctl-compatible text** commands (`dispatch`, `reload`, etc.) returning JSON snapshots; `event>>data` stream to subscribers. `CompCmd` enum for main-loop effects. 0600 socket, line-length cap, SO_PEERCRED logging proposed in e09s03.
- Goal: Tacit already speaks hyprctl's dialect; eswm's compatibility is a product requirement, not an accident.

### Assessment
- **better in them:** engineering hardening (epoll, bounds, content-hashed versioning, typed round-trips, no ambiguous JSON parsing).
- **better in us:** text+JSON is exactly what Tacit consumes; third-party tooling can use `socat`/`jq`; event stream needed by ~20 Hyprland events.
- **borrow for e09s03:** adopt their *hardening* (1 MiB cap, client caps, idle pruning, 0600 socket, SO_PEERCRED, nonblocking writes that never delay dispatch, hello with version + error codes — all already in e09s03's problem statement) while keeping our wire format. Their per-schema content hash is overkill for a text protocol; a simple version string in `hello` suffices.

---

## 7. Testing & quality

### instantWM
- ~1,359 `#[test]` functions; `TestWm`/`MonitorBuilder` backend-neutral fixtures; compile-fail ownership contract with exact error counts; i3bar/SNI unit tests; 2 fuzz systems (libFuzzer scenario over real Wayland wire + headless watchdog campaign with resource limits and panic-signature minimization); E2E nested-compositor script with geometry/overlap assertions; CI runs fmt, clippy `--all-targets --all-features`, borrow contract, tests, build, X11 ignored tests, CLI smoke.
- No coverage instrumentation; E2E/fuzz not in CI.

### eswm
- 144 inline tests (up from 116 at session start), all fast (0.44s), RED-first two-commit TDD across e08, verification evidence per story, audit reports. Tests live inline in `src/*.rs` per CONVENTIONS (single binary crate; integration tests can't see private `State`).
- No fuzzing, no coverage tooling, no E2E script; helper scripts referenced by skills (`bp-timing`, `plan-consistency-check`, `verify-tdd-red-commit`, etc.) are absent.

### Assessment
- **better in them:** depth, fuzzing, architecture contracts, E2E, CI breadth.
- **better in us:** tight RED/GREEN discipline and per-story evidence/audit artifacts are a strong process; tests are fast and behavior-focused.
- **borrow:** add a small headless protocol fuzz target (their `IWMF`-style scenario runner is a model), a borrow/ownership contract test if we modularize, and an E2E nested-compositor script once Tacit integration starts. Do not attempt test-count parity.

---

## 8. Features feature-by-feature

| Feature | eswm | instantWM | Note |
|---|---|---|---|
| Tiling layouts | Master + Equal | Master/stack, grid, bottom-stack + manual tree | ours sufficient for Tacit |
| Multiple workspaces | 9 per output | Tags (multi-view, sticky, scratchpad) | e09 models must grow |
| Floating windows | Yes, cascaded spawn | Full subsystem: snap, drag/drop, scratchpads | e08s01 hardened our placement |
| Animations | None | Window transitions, layout preview, cursor animation | deferred in SCOPE |
| Status bar | External | Built-in i3bar + SNI tray | our delegation is intentional |
| Start menu | External (fuzzel) | Built-in layout menu + instantMENU integration | our path is Tacit's |
| Screen sharing | Via portals (external) | Ships portal config + GlobalShortcuts portal | later story |
| Fullscreen | Band above all layers (e08s02) | True/fake fullscreen, VRR blockers | ours simple but correct |
| CSD geometry | Honored via xdg_offset (e08s02) | `decor_hints` + ServerSide decoration | ours more protocol-correct |
| Output scale | `es.dsp.set_output_scale(name, f32)` (e08s03) | TOML + runtime `config set`, transforms, VRR, mirror | theirs broader; ours adequate |
| Keymap/touchpad | Lua setters (e08s03) | TOML + runtime + device-specific settings | comparable surface |
| IPC security | Planned (e09s03) | Implemented (bounds, caps, peer checks) | borrow |
| Reload | Stub in Lua | Validated atomic reload + runtime edits | e09s04 |

---

## 9. Concrete recommendations for e09 (and just after)

1. **e09s01 (window state model):** follow instantWM's `ClientMode` split — separate `layout: Tiling|Floating` from `state_flags: {maximized, fullscreen, minimized}`; stop using `mapped` as minimized; per-window focus rather than per-output bool. Our e08s04 `mode_sent` marker is a proto-version of their presentation-state tracking.
2. **e09s02 (output model):** adopt per-output `{position, scale, work_area, LayerMap, focused}`; one notion of focused output; remove the dual "primary" definitions. Their `Monitor` owning clients is a clean invariant; consider it if multi-output work follows.
3. **e09s03 (IPC):** keep text+JSON hyprctl compatibility; borrow their socket hardening (caps, timeouts, nonblocking writes, peer creds, versioned hello) and their "one effect enum applied in one place" pattern (`CompCmd` already similar).
4. **e09s04 (Lua config):** keep Lua; replace lying stubs with errors; add `es.config{}` shape and `es.bind(keys, fn, opts)`; add a typed action catalog with arg validation and self-documentation; implement validate-before-apply reload; drop the per-frame `Arc<Mutex<DecorationConfig>>` copy if it is provably single-threaded.
5. **Post-e09 backlog (ordered):** E2E nested-compositor test; headless protocol fuzz; session-lock + screencopy for Tacit shell; output-management; persistent z-order projection; animations; backend abstraction traits only if an X11 backend becomes real.

---

## 10. What we should NOT copy

- X11 support — out of scope; DRM/libseat is product-locked.
- TOML config — Lua is product-locked; copying their config format would break Tacit compatibility.
- Bincode IPC — Tacit needs hyprctl text/JSON; do not break wire compatibility for engineering elegance.
- The persistent manual tiling tree — huge complexity for little Tacit value; master/equal is enough.
- Pinned smithay git fork — maintenance risk; stay on crates.io 0.7.
- Built-in status bar / start menu — Tacit owns the shell; eswm should stay a compositor.

---

## 11. Closing note

instantWM demonstrates that a compositor can be both broad and well-engineered; eswm demonstrates that a narrow, test-driven compositor can ship steadily. The right synthesis for e09 is **their models, our product constraints**: borrow the *shapes* (state/output/IPC/config/effects) without importing the features that eswm deliberately delegates to Tacit. The repo's own e09 problem statements already anticipate most of this — this comparison mainly supplies concrete, proven implementations to learn from and traps to avoid.
