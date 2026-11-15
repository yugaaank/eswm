### Story e05s01: Test fixture with no display (UnixStream client)

**type:** feat  
**risk:** P2  
**context:** testing  

**Context:** Every integration test needs a running desktop, and tests/globals.rs
uses the real $XDG_RUNTIME_DIR. Nothing tests the Wayland side. niri's
src/tests/ (fixture.rs, server.rs, client.rs) is the model to copy.

## Steps

1. Build State without a renderer or DRM (udev: None) -> verify: `cargo build`
2. Connect a wayland-client over UnixStream::pair() with
   display.handle().insert_client(server_end, Arc::new(ClientState::default()))
   -> verify: `cargo build`
3. Add a roundtrip() helper that dispatches both sides until neither has work
   -> verify: `cargo build`
4. Point tests/globals.rs at a temp dir -> verify: `cargo test`
5. Test: client binds xdg_wm_base, creates a toplevel, sets app_id and commits.
   The WM then has one window with that app_id -> verify: `cargo test`

## Verification Script

1. Run `cargo test` with WAYLAND_DISPLAY and XDG_RUNTIME_DIR unset; all pass.
2. Confirm fixture client can create a toplevel and the WM sees it.

## Out of scope

- DRM or input device testing in CI
- Full compositor session in tests

## Risks

- Smithay 0.7 client API may differ from niri's version; check Smithay source
  before writing fixtures.

---
