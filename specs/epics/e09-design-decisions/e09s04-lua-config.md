### Story e09s04: Lua config shape: declarative es.config{}, es.bind with function/action

**type:** refactor  
**risk:** P3  
**context:** lua  

**Context:** Tacit config uses ~1900 lines of hl.config{}, hl.bind(keys, fn, opts),
hl.window_rule{}, hl.layer_rule{}, hl.gesture{}, hl.on(event, fn), hl.dsp.*.
Mixed actions and setters; es.dsp mixes setters with actions; binds name closed list
of strings; stubs that lie (define_submap, global, config.*, safe_require succeed
silently; is_file_exists always false); no reload; needless lock (DecorationConfig
through Arc<Mutex> copied every frame, everything on one thread).

## Steps

1. Declarative es.config{} with validation -> verify: `cargo build`
2. es.bind taking Lua function or action; es.on for events -> verify: `cargo build`
3. Rule tables; reload by running config again -> verify: `cargo build`
4. Stubs raise "not implemented" instead of succeeding silently -> verify: `cargo build`
5. is_file_exists returns real result -> verify: `cargo build`
6. Remove needless Arc<Mutex> lock on DecorationConfig (single-threaded) -> verify: `cargo build`

## Verification Script

1. es.bind with Lua function action works.
2. es.config{} declarative block loads and validates.
3. Stub functions (is_file_exists, etc.) return real results or "not implemented" error.

## Out of scope

- Safe require module loading

## Risks

- Changing Lua API breaks existing configs; document migration.

---
