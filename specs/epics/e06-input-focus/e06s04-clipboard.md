### Story e06s04: Clipboard and primary selection (data_device focus)

**type:** feat  
**risk:** P2  
**context:** input  

**Context:** Smithay offers selection only to the client set with
set_data_device_focus, and no client is ever set, so copy/paste does nothing.

## Steps

1. In focus_changed, find client of focused surface; call set_data_device_focus and
   set_primary_focus with it (anvil's focus_changed pattern) -> verify: `cargo build`
2. Add primary-selection global -> verify: `cargo build`
3. Add ext-data-control -> verify: `cargo build`

## Verification Script

1. wl-copy hi in one terminal; wl-paste in another -> "hi".
2. wl-paste --watch cat prints on every copy.

## Out of scope

- Clipboard history manager

## Risks

- Smithay data_device API focus semantics must match anvil.

---
