# Linux Distro-Agnostic Plan

**Date:** 2026-11-28  
**Status:** Draft / planning reference  
**Scope:** Make eswm build, run, and package cleanly across major Linux distributions and be ready for OS/public-repo packaging.

## Goal / Non-goal

- **Goal:** Linux-distro agnostic — same source builds and runs on major Linux distros (Fedora, Arch, Debian/Ubuntu, openSUSE, NixOS, Alpine/musl where feasible) and is ready for distro/public-repo packaging.
- **Non-goal:** OS/platform agnostic beyond Linux. eswm remains a Linux DRM/libseat/Wayland compositor. FreeBSD/other OS support would require separate backend/session/input work and is out of scope for this plan.

## Current distro-friction points

| Area | Current state | Distro friction |
|---|---|---|
| Build deps | `Cargo.toml`: smithay 0.7 `backend_egl,use_system_lib`, wayland-server/protocols, xkbcommon, xcursor, mlua lua54, libc, smithay-drm-extras. Implicit system libs: libudev, libseat, libinput, libdrm, gbm, EGL/GL, libdisplay-info, lua5.4. | Package names differ (`systemd-devel` vs `libudev-dev`, `lua-devel` vs `liblua5.4-dev`, `mesa-libgbm-devel` vs `libgbm-dev`). |
| Runtime/session integration | No session env export yet; planned `systemctl --user import-environment` + `dbus-update-activation-environment` (see `PROPER-DE-PLAN_LATEST.md` Phase 0). | systemd/dbus not universal (elogind, seatd-only, minimal systems). Must be optional and gracefully degraded. |
| Privilege/session backend | Smithay/libseat path (`src/udev.rs` uses `LibSeatSession`). | Works with logind or seatd, but runtime deps/docs differ by distro. |
| Paths/logs | Uses `$XDG_RUNTIME_DIR`, `$XDG_STATE_HOME`, `/run/user/{uid}` fallback (`src/control.rs`, `src/main.rs`, `src/ipc.rs`). | Mostly fine; fallback behavior should be explicit and documented. |
| Config/deps discovery | Lua 5.4 via mlua; pkg-config-based native deps. | Lua pkg-config name varies (`lua54`, `lua5.4`, `lua`). |
| Packaging | No eswm distro packaging files, no `.desktop` session file, no install targets, no release artifacts. | Blocks OS public repos. |
| CI | No visible Linux distro matrix in repo. | Regressions on other distros unnoticed. |

## Plan

### 1. Build/dependency portability

- **Document canonical deps by pkg-config/module, not only distro package names** in `README.md` / `BUILDING.md`:
  - `libudev`, `libseat`, `libinput`, `xkbcommon`, `libdrm`, `gbm`, `egl`, `glesv2` (as required by smithay), `wayland-server`, `libdisplay-info`, `lua5.4/lua54`.
- **Add a `BUILDING.md` distro package mapping table**:

  | Dependency | Fedora | Debian/Ubuntu | Arch | openSUSE | Alpine |
  |---|---|---|---|---|---|
  | libudev | `systemd-devel` | `libudev-dev` | `systemd-libs` | `systemd-devel` | `eudev-dev`/`udev-dev` (verify) |
  | libseat | `libseat-devel` | `libseat-dev` | `seatd` | `libseat-devel` | `seatd-dev` |
  | libinput | `libinput-devel` | `libinput-dev` | `libinput` | `libinput-devel` | `libinput-dev` |
  | libxkbcommon | `libxkbcommon-devel` | `libxkbcommon-dev` | `libxkbcommon` | `libxkbcommon-devel` | `libxkbcommon-dev` |
  | libdrm | `libdrm-devel` | `libdrm-dev` | `libdrm` | `libdrm-devel` | `libdrm-dev` |
  | gbm/EGL/GL | `mesa-libgbm-devel mesa-libEGL-devel mesa-libGL-devel` | `libgbm-dev libegl-dev libgl-dev` | `mesa` | `Mesa-libgbm-devel Mesa-libEGL-devel Mesa-libGL-devel` | `mesa-dev` |
  | libdisplay-info | `libdisplay-info-devel` | `libdisplay-info-dev` | `libdisplay-info` | `libdisplay-info-devel` | `libdisplay-info-dev` (verify) |
  | Lua 5.4 | `lua-devel` | `liblua5.4-dev` | `lua` | `lua54-devel` | `lua5.4-dev` |
  | wayland | `wayland-devel wayland-protocols-devel` | `libwayland-dev wayland-protocols` | `wayland wayland-protocols` | `wayland-devel wayland-protocols-devel` | `wayland-dev wayland-protocols` |

  *Verify package names in CI before treating table as authoritative.*
- **Make Lua pkg-config discovery robust**. Confirm `mlua` build behavior across distros; document `PKG_CONFIG_PATH` or feature selection if needed.
- **Add optional `scripts/check-build-deps.sh`** that checks `pkg-config --exists` for required modules and prints distro-agnostic hints.

**Acceptance:** fresh Fedora and Debian/Ubuntu containers can build from documented deps without tribal knowledge.

### 2. Runtime portability guards

- **Systemd/dbus integration optional, never required** (pairs with `PROPER-DE-PLAN_LATEST.md` Phase 0):
  - Detect `systemctl --user` and `dbus-update-activation-environment` availability at runtime; log and continue if missing.
  - Gate sd_notify behind systemd availability/feature.
  - Add opt-out env vars: `ESWM_NO_SYSTEMD=1`, `ESWM_NO_DBUS_ENV=1`.
- **Session backend expectations**: support logind and seatd via libseat; document both. No hard dependency on systemd-logind semantics beyond libseat.
- **Path handling**: keep XDG base dirs; avoid hardcoding `/run/user/$UID` except as fallback; ensure runtime dirs created with safe perms.
- **Audio/portal/clipboard**: don’t hardcode PulseAudio/PipeWire/systemd specifics in compositor core; portals handle desktop integration.

**Acceptance:** eswm starts on systemd+logind, elogind/seatd setups, and minimal environments with degraded-but-logged optional integrations.

### 3. Packaging & repo-install readiness

Add distro-neutral packaging artifacts in repo:

- **Packaging artifacts**:
  - `resources/eswm.desktop` (Wayland session entry for display managers/session pickers)
  - `resources/eswm-portals.conf` (once Phase 0 lands)
  - systemd user units only if needed; keep optional
  - man page / `--help` / `--version`
- **Install layout**:
  - binary: `/usr/bin/eswm`
  - session desktop: `/usr/share/wayland-sessions/eswm.desktop`
  - portal conf: `/usr/share/xdg-desktop-portal/eswm-portals.conf`
  - docs/license: `/usr/share/doc/eswm/`
- **Packaging templates** (CI-validated where possible):
  - Arch `PKGBUILD`
  - Fedora `.spec`
  - Debian `debian/` (optional later)
  - Nix flake/package (good for reproducibility; catches hidden deps)
- **Release artifacts**: versioned tarballs, checksums, changelog; `cargo install` path remains supported for devs.

**Acceptance:** a packager can build/install eswm without patching source; at least one external packaging format builds in CI.

### 4. CI matrix for distro coverage

- **GitHub Actions matrix** (container jobs):
  - Fedora latest
  - Ubuntu LTS / Debian stable
  - Arch Linux
  - optional: Alpine (musl) as “best effort”
- **Steps per distro**: install deps -> `cargo build` -> `cargo test` -> `cargo clippy` (as project policy allows).
- **Cache cargo/target** carefully; keep logs on failure.
- **Add a “build deps changed” check**: when `Cargo.toml` or system deps change, update `BUILDING.md`.

**Acceptance:** CI catches missing pkg-config/lib naming issues before merge.

### 5. Feature/compatibility policy

- **Explicit support tiers**:
  - Tier 1: Linux DRM/libseat + systemd/logind distros (Fedora/Arch/Debian/Ubuntu)
  - Tier 2: seatd/elogind/non-systemd Linux
  - Tier 3: musl/minimal distros (best effort)
- **Avoid Linux-distro-specific commands in core path**; isolate them behind small adapters (`session_env/systemd.rs`, `session_env/dbus.rs` or equivalent).
- **No compile-time hard systemd dependency** unless feature-gated and justified.

**Acceptance:** docs clearly state what “distro agnostic” means; unsupported cases fail with clear logs.

## Suggested execution order

1. **Docs + dep matrix + CI containers** — fast, exposes real gaps; would have caught the Fedora missing-deps case.
2. **Runtime optional guards** for systemd/dbus/session export — pairs with `PROPER-DE-PLAN_LATEST.md` Phase 0.
3. **Session `.desktop` + install layout + install script/Makefile helper**.
4. **Packaging templates** — Arch/Fedora first.
5. **Nix flake** as reproducibility canary (optional but high value).

## Immediate next step

Create a small planning epic (e.g. `e12-distro-agnostic-packaging`; e11 is reserved for proper-DE parity) with stories for: build dep docs, CI distro matrix, optional systemd/dbus guards, and session/portal packaging files. Start by adding `BUILDING.md` + a Fedora/Ubuntu CI build job.

## Related plans

- `specs/epics/e11-proper-de/PROPER-DE-PLAN_LATEST.md` — Phase 0 session env export has direct portability implications; implement with optional guards from this plan.
- `notes/roadmap.md` — packaging/distribution is a cross-cutting product concern; align epics when scoped.
