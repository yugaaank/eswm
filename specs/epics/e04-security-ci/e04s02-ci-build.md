### Story e04s02: Fix Arch CI build (smithay-drm-extras / libdisplay-info)

**type:** fix  
**risk:** P1  
**context:** build  

**Context:** `smithay-drm-extras 0.1` pulls in `libdisplay-info-sys 0.2.2`, which
needs `libdisplay-info < 0.3`, but Arch ships `0.3.0`. `cargo build` fails on a
current Arch install, and Tacit packages on Arch. The review identified this at
udev.rs:1356-1365 where `display_info::for_connector` is used.

## Steps

1. Set `default-features = false` on `smithay-drm-extras` in `Cargo.toml` -> verify: `cargo build`
2. Replace `display_info::for_connector` (udev.rs:1356-1365) with a small `edid.rs`
   that reads the connector's EDID property blob and returns make (bytes 8-9,
   three 5-bit letters) and model (the 0xFC descriptor) -> verify: `cargo build`
3. Add a GitHub Actions job in an `archlinux:latest` container that runs
   `cargo build`, `cargo test` and `cargo clippy` -> verify: CI job passes
4. Unit tests for `edid.rs` with a real 128-byte EDID, plus one with no `0xFC`
   descriptor (model falls back to connector name) -> verify: `cargo test`

## Verification Script

1. Run `cargo build`, `cargo test`, `cargo clippy`; all pass.
2. Confirm GitHub Actions workflow exists at `.github/workflows/ci.yml`.
3. Confirm `edid.rs` unit tests pass with real and missing-0xFC EDID data.
4. Confirm `smithay-drm-extras` has `default-features = false` in Cargo.toml.

## Out of scope

- Supporting other distributions' packaging
- Alternative EDID parsing libraries

## Risks

- EDID parsing must handle connectors with no EDID blob gracefully.

---
