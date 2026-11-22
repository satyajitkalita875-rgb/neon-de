# NEON DE — Development Roadmap

NEON DE is built in eight verifiable milestones. Every milestone produces a compilable, runnable, and tested system.

| Milestone | Scope | Status |
|---|---|---|
| **M1 — Initial Compositor** | Rust workspace, Smithay `0.7.0`, nested `winit` + GLES backend, Wayland display, client lifecycle, terminal + GUI app support, keyboard/pointer input, click-to-focus, interactive move/resize, automated tests | **Completed (Verified)** |
| **M2 — Window Management** | Full window state machine, intelligent placement, snapping/optional tiling, maximize/minimize/restore, fullscreen, dynamic workspaces, window rules, crate extraction (`neon-window-manager`, `neon-workspaces`) | **Completed (Verified)** |
| **M3 — Rendering Engine** | Retained scene graph (`neon-scene`), modular renderer (`neon-renderer`), damage tracking, rounded corners, shadows, frame-driven animation engine (`neon-effects`) | **Next** |
| **M4 — Desktop Shell** | Typed Unix socket IPC (`neon-ipc`), `zwlr_layer_shell_v1`, GTK4 `neon-shell` (top panel, dock, activities overview, application grid, search, workspace previews) | Planned |
| **M5 — Advanced Compositor** | GPU blur & translucency, touchpad gestures, DRM/KMS bare-metal multi-monitor backend (`neon-output`), hotplugging, fractional scaling, XWayland, `ext_session_lock_v1` lock screen | Planned |
| **M6 — Settings Application** | Versioned XDG config service (`neon-config`), GTK4 macOS-inspired `neon-settings` app with live IPC updates across all categories | Planned |
| **M7 — System Integration** | D-Bus integration (`neon-system`): NetworkManager, PipeWire/WirePlumber, BlueZ, UPower, notifications daemon, polkit, XDG Desktop Portals | Planned |
| **M8 — Production Readiness** | Crash recovery, profiling, accessibility, packaging, session `.desktop` files (`neon-session`), bare-metal QA | Planned |

## Milestone 1 Acceptance Criteria

- [x] Rust Cargo workspace initialized with `neon-compositor` crate.
- [x] Architecture and protocol documentation written.
- [x] Custom Smithay `0.7.0` Wayland compositor compiles cleanly without warnings (`cargo clippy --workspace --all-targets -- -D warnings`).
- [x] Nested `winit` backend runs inside the host Wayland/X11 session without disturbing it.
- [x] Wayland socket is created and exposed to child clients (`WAYLAND_DISPLAY`).
- [x] Terminal emulator (`weston-terminal`) and graphical Wayland app (`weston-simple-shm`) run simultaneously inside NEON DE.
- [x] Keyboard input, pointer motion/buttons/scroll, click-to-focus, and window move/resize implemented and verified.
- [x] Window creation, surface commit, and destruction lifecycle verified by automated tests and live execution.

## Milestone 2 Acceptance Criteria

- [x] Extracted `crates/neon-workspaces` (GNOME-style dynamic workspace manager with trailing empty workspace and automatic pruning).
- [x] Extracted `crates/neon-window-manager` (exclusive `WindowMode` state machine, intelligent placement, transient parent-child centering, smart snapping, focus policies, window rules engine).
- [x] Integrated dynamic workspaces, minimize/restore, maximize/fullscreen, snapping, and rules into `neon-compositor`.
- [x] Verified multi-window lifecycle, workspace switching, window movement, pruning, snapping, minimize/restore, fullscreen exclusivity, and rules via automated unit + real Wayland client integration tests and live nested session execution.
