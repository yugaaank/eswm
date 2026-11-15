### Story e05s02: Event-driven loop with calloop sources

**type:** refactor  
**risk:** P3  
**context:** core loop  

**Context:** The display fd and listening socket are not calloop sources, so one
client is accepted per iteration and clients are dispatched only after a 16 ms
poll. The loop renders every iteration on top of vblank timers. On dispatch error
the session exits with status 0 and no log line. anvil's src/state.rs does exactly
this pattern.

## Steps

1. Insert a ListeningSocketSource whose callback calls insert_client -> verify: `cargo build`
2. Insert a calloop Generic source on the display's poll fd
   (Interest::READ, Mode::Level) whose callback calls dispatch_clients -> verify: `cargo build`
3. Dispatch with no fixed timeout; call flush_clients after each dispatch -> verify: `cargo build`
4. Log per-client errors and carry on; on loop error, log and exit status 1 -> verify: `cargo build`

## Verification Script

1. In the fixture (e05s01), connect 10 clients before one dispatch; all 10 get
   their globals in a single roundtrip.
2. An idle session shows about 0% CPU in top.

## Out of scope

- vblank timer integration (part of e05s04)
- Damage tracking (part of e05s04)

## Risks

- calloop Generic source API may differ from Smithay version; check Smithay source.

---
