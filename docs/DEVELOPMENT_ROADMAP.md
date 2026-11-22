# NEON DE — Development Roadmap

NEON DE is built in eight verifiable milestones. Every milestone produces a compilable, runnable, and tested system.

| Milestone | Scope | Status |
|---|---|---|
| **M1 — Initial Compositor** | Rust workspace, Smithay `0.7.0`, nested `winit` + GLES backend, Wayland display, client lifecycle, terminal + GUI app support, keyboard/pointer input, click-to-focus, interactive move/resize, automated tests | **Completed (Verified)** |
| **M2 — Window Management** | Full window state machine, intelligent placement, snapping/optional tiling, maximize/minimize/restore, fullscreen, dynamic workspaces, window rules, crate extraction (`neon-window-manager`, `neon-workspaces`) | **Completed (Verified)** |
| **M3 — Rendering Engine** | Retained scene graph (`neon-scene`), modular renderer (`neon-renderer`), damage tracking, rounded corners, shadows, frame-driven animation engine (`neon-effects`), `zwp_linux_dmabuf_v1` | **Completed (Verified)** |
| **M4 — Desktop Shell** | Typed Unix socket IPC (`neon-ipc`), `zwlr_layer_shell_v1`, GTK4 `neon-shell` (top panel, dock, activities overview, application grid, search, workspace previews) | **Completed (Verified)** |
| **M5 — Advanced Compositor** | GPU blur & translucency, touchpad gestures, DRM/KMS bare-metal multi-monitor backend (`neon-output`), hotplugging, fractional scaling, XWayland, `ext_session_lock_v1` lock screen | **Next** |
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

## Milestone 3 Acceptance Criteria

- [x] Extracted `crates/neon-effects` (time-based interruptible animations with linear/cubic/Bézier/spring easing, chaining, cancellation, reduced motion).
- [x] Extracted `crates/neon-scene` (retained Z-layered scene graph with shadow/border descriptors and dirty-region tracking).
- [x] Extracted `crates/neon-renderer` (modular GLES renderer with compiled analytical shadow, rounded focus ring, and ambient backdrop shaders + cached damage-tracked elements).
- [x] Integrated `zwp_linux_dmabuf_v1`, scene graph sync, animations, and zero-damage idle frame skipping into `neon-compositor`.
- [x] Verified shader compilation, DMA-BUF EGL client (`weston-simple-egl`) + SHM client + terminal rendering in live nested session, plus automated unit & integration tests.

## Milestone 4 Acceptance Criteria

- [x] Extracted `crates/neon-ipc` (versioned Unix socket IPC with `0700`/`0600` permissions, `SO_PEERCRED` UID auth, typed window/workspace/overview/effects requests & event subscriptions).
- [x] Implemented `zwlr_layer_shell_v1` in `neon-compositor` + `neon-renderer` (exclusive zone work-area reservation, upper/lower layer compositing Z-order, `None`/`OnDemand`/`Exclusive` keyboard interactivity).
- [x] Built `crates/neon-shell` in Rust + GTK4 + `gtk4-layer-shell`: Top Panel (Activities, dynamic workspace pills, live clock, active title), Floating Dock (XDG favorites, running window indicators, app grid trigger), Activities Overview (live search, workspace previews, open window cards, XDG Desktop Entry application grid with categories).
- [x] Verified full nested NEON DE session running `neon-compositor --shell` + `neon-shell` + real Wayland apps simultaneously, plus automated unit & IPC integration tests.
