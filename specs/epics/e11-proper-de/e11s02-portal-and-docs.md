# e11s02 — Portal selection config and TTY launch documentation

**type:** feat
**risk:** P2
**context:** infra
**delta:** ADDED

## Context

Even with a correct session identity, portals need a backend selection that
matches what eswm actually supports. This story ships the conservative default
config (GTK for FileChooser/OpenURI/Settings, no ScreenCast claim) and documents
the TTY launch flow so users know how to get the session export and portal
backend working together. ScreenCast stays unclaimed until a capture protocol
exists in a later phase.

## Zoom-out (existing modules)

- **`e11s01`** — purpose: export eswm session identity. Caller: `udev.rs`
  startup. Contract: opt-out and ownership-safe cleanup; e11s02 documents those
  controls but does not change them.
- **Portal config location** — `/usr/share/xdg-desktop-portal/<desktop>-portals.conf`
  is the observed system layout (Hyprland config inspected). Package layout is
  out of scope (deferred to e12); this story ships the file and its install path
  in docs.

## Requirements

#### ADDED: Ship a conservative portal config
`resources/eswm-portals.conf` selects the GTK backend for FileChooser, OpenURI
and Settings and deliberately makes no ScreenCast or Screenshot claim.

#### ADDED: Document the TTY session flow
`docs/session.md` explains how to launch eswm on a TTY, which identity variables
are exported, how to opt out, and which portal backends are required.

## Reason for depth

No new abstraction: two static artifacts plus inline assertions that keep them
from silently drifting.

## Steps

1. Add `resources/eswm-portals.conf` with `[ Preferred ]` GTK defaults and no
   ScreenCast entry; assert the shipped content in an inline test → verify: `cargo test --bin eswm tests::portals_conf_defaults_to_gtk`
2. Add `docs/session.md` covering TTY launch, the four identity variables,
   `ESWM_NO_SD_VARS`, and portal backend requirements; assert the doc mentions
   each variable → verify: `cargo test --bin eswm tests::session_docs_cover_tty_launch`
3. Run the operator UAT below and record each row → verify: `verify-script: specs/epics/e11-proper-de/e11s02-portal-and-docs.md`

## Verification script (operator, live DRM only)

1. Install/copy `eswm-portals.conf` to the documented system path.
2. Start eswm from a TTY with session export enabled.
3. Confirm `systemctl --user show-environment` lists the eswm identity.
4. Open a GTK/Qt file picker and confirm it stays on eswm.
5. Confirm a screen-share request does not claim a portal backend eswm lacks.

## Out of scope

- Portal ScreenCast/Screenshot support (Phase 2).
- Packaging rules that install the file automatically (e12).
- Writing a custom eswm portal backend.

## Risks

- GTK-only default may not satisfy Qt applications on some distributions:
  documented as a known limitation; no claim beyond what is observed.
- Portal backend availability varies by distro; docs must state requirements
  without making them hard dependencies.
