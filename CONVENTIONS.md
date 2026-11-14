# eswm — Conventions

## § Always Green / Shift Left
Preflight (`cargo clippy && cargo test && cargo build`) and CI must be green before forward work. The 1-10-100 rule: a defect caught at commit costs 1, at CI 10, at deploy 100. Shift every check left to the commit.

## § Discovered Defects
Reproducible gate failures require fix-or-log: quick-fix -> fix-bug — even when "outside" the story scope. Log separate fixes in separate commits; never dismiss a red gate as pre-existing or unrelated.

## § Banned Dismissive Phrases
| Phrase | Why banned |
|--------|------------|
| "pre-existing" | Dismisses without investigation |
| "unrelated to session" | Assumes causation without evidence |
| "not introduced by my changes" | Shifts blame, skips root cause |
| "out of scope" | Waives Always Green without fix-or-log |

## § specs/ Output Convention
All planning documents, specifications, investigations, and verification outputs live in `specs/`. No planning docs written to project root.

## § Defensive Code Categories
- Timeout — client IPC hangs, DRM pause/resume stalls
- Graceful degradation — Winit dev-backend fallback, output hotplug failure
- Retry — DRM device reopen, client reconnect
- Rate limit — IPC message flooding from clients
- Circuit breaker — not applied (overhead not justified at this scale)

## § Naming (eswm easy brand)
eswm is the brand; "easy" means short, grep-unique names — no clever abbreviations. Each name should return fewer than 5 grep hits.

## § File Organization
- Source under `src/`
- Tests under `tests/`
- All planning under `specs/`
- Config is Lua-only at `~/.config/eswm/config.lua`

## § Never
- Never hand-edit Cargo.lock — cargo add / cargo update only
- Never add JSON config — Lua-only config path is product-goal locked
- Never run DRM/libseat backend from inside an existing Wayland/X11 session — VT grab
- Never copy Pinnacle source, assets, or distinctive implementation (reference only)
- Never skip preflight before forward work — red gate = fix-or-log
