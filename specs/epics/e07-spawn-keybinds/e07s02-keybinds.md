### Story e07s02: Keybinds: xkb keysym names, raw sym matching, emergency quit always on

**type:** fix  
**risk:** P3  
**context:** input  

**Context:** Only ~35 key names exist; Super+r example never fires. Matching on
modified_sym() means Super+Shift+1 arrives as exclam, so move-to-workspace binds
never match. Caps Lock and AZERTY break binds. No built-in binds means a broken
config leaves no VT switch or quit. xkb::keysym_from_name(name,
KEYSYM_CASE_INSENSITIVE) parses key names at bind time; unknown name is a Lua error.
Match on raw_latin_sym_or_raw_current_sym(), dropping Caps/Num Lock. Keep
Ctrl+Alt+F1..F12 and emergency quit always on; config can't remove them.

## Steps

1. Parse key names at bind time with xkb::keysym_from_name(name, KEYSYM_CASE_INSENSITIVE);
   unknown name -> Lua error from es.bind -> verify: `cargo build`
2. Match on raw_latin_sym_or_raw_current_sym(), comparing modifiers with Caps/Num
   Lock dropped -> verify: `cargo build`
3. Keep Ctrl+Alt+F1..F12 and emergency quit always on; config can't remove them -> verify: `cargo build`
4. Intercept release for any intercepted press -> verify: `cargo build`

## Verification Script

1. Super+r matches.
2. Super+Shift+1 matches (raw 1 with logo and shift).
3. Same with Caps Lock on matches.
4. Super+a on AZERTY layout matches.
5. Unknown bind name -> error.
6. quit_keybind_matches_super_alt_backspace test still passes.

## Out of scope

- Key name autocomplete or suggestions

## Risks

- xkbcommon API for keysym_from_name may vary by version; check Smithay's usage.

---
