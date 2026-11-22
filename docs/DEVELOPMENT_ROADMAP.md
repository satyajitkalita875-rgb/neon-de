# NEON DE — Development Roadmap

NEON DE is built in eight verifiable milestones. Every milestone produces a compilable, runnable, and tested system.

| Milestone | Scope | Status |
|---|---|---|
| **M1 — Initial Compositor** | Rust workspace, Smithay `0.7.0`, nested `winit` + GLES backend, Wayland display, client lifecycle, terminal + GUI app support, keyboard/pointer input, click-to-focus, interactive move/resize, automated tests | **Completed (Verified)** |
| **M2 — Window Management** | Full window state machine, intelligent placement, snapping/optional tiling, maximize/minimize/restore, fullscreen, dynamic workspaces, window rules, crate extraction (`neon-window-manager`, `neon-workspaces`) | **Completed (Verified)** |
| **M3 — Rendering Engine** | Retained scene graph (`neon-scene`), modular renderer (`neon-renderer`), damage tracking, rounded corners, shadows, frame-driven animation engine (`neon-effects`), `zwp_linux_dmabuf_v1` | **Completed (Verified)** |
| **M4 — Desktop Shell** | Typed Unix socket IPC (`neon-ipc`), `zwlr_layer_shell_v1`, GTK4 `neon-shell` (top panel, dock, activities overview, application grid, search, workspace previews) | **Completed (Verified)** |
| **M5 — Advanced Compositor** | GPU frosted-glass blur & translucency, touchpad gestures (`zwp_pointer_gestures_v1`), multi-monitor (`neon-output`) & hotplugging, fractional scaling (`wp_fractional_scale_v1` + `wp_viewporter`), XWayland (`X11Wm`), `ext_session_lock_v1` + `neon-lockscreen` | **Completed (Verified)** |
| **M6 — Settings Application** | Versioned XDG config service (`neon-config`), GTK4 macOS-inspired `neon-settings` app with live IPC updates across Appearance, Displays, Desktop/Windows, Dock/Panels, Effects, Input, Privacy/Accessibility, and System | **Completed (Verified)** |
| **M7 — System Integration** | D-Bus integration (`neon-system`): NetworkManager, PipeWire/WirePlumber, BlueZ, UPower, notifications daemon, polkit, XDG Desktop Portals | **Completed (Verified)** |
| **M8 — Production Readiness** | Bare-metal DRM/KMS backend (`libseat` + `libinput` + `udev` + `GBM`), `neon-session` supervisor with crash recovery & `--safe-mode`, frame diagnostics telemetry, high-contrast accessibility, Wayland `.desktop` sessions, systemd user units, release `install.sh`/`uninstall.sh` | **Completed (Verified)** |

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

## Milestone 5 Acceptance Criteria

- [x] Extracted `crates/neon-output` (multi-monitor discovery, horizontal auto-layout, fractional scale + rotation geometry, display mirroring, safe hotplug disconnect with primary fallback & window migration).
- [x] Added GPU frosted-glass blur & translucency shader (`FROSTED_GLASS_FRAGMENT_SHADER`), live IPC blur/opacity toggles, and `--no-blur` fallback.
- [x] Implemented `wp_fractional_scale_v1`, `wp_viewporter`, `wp_presentation`, and `zwp_pointer_gestures_v1` (+ 3-finger swipe workspace/overview navigation).
- [x] Implemented `ext_session_lock_v1` with strict focus & render isolation + standalone `crates/neon-lockscreen` binary (`Super + L` / IPC `LockSession`).
- [x] Implemented XWayland (`XWaylandShellState` + `X11Wm`) for legacy X11 client compatibility.
- [x] Verified all automated unit, protocol, multi-monitor, IPC, and lock-screen tests plus live nested execution of frosted-glass shaders, XWayland, and `neon-lockscreen`.

## Milestone 6 Acceptance Criteria

- [x] Extracted `crates/neon-config` (versioned TOML schema v1, bounds clamping, corrupt-file backup & safe fallback, schema migration, atomic write-and-rename, export/import).
- [x] Built `crates/neon-settings` in Rust + GTK4 with macOS-inspired searchable sidebar, Neon Dark & Neon Light themes, grouped cards, and live IPC sync.
- [x] Wired live `UpdateAppearance` (Neon Dark / Neon Light, accent color, panel/dock transparency, clock/workspace toggles) and `UpdateWindowBehavior` (focus policy, snap gap) across `neon-compositor`, `neon-shell`, and `neon-settings`, plus one-click Settings & Lock buttons in the Top Panel.
- [x] Verified config unit tests, live IPC appearance/window-behavior tests, and live nested launch of `neon-settings` inside NEON DE.

## Milestone 7 Acceptance Criteria

- [x] Created `crates/neon-system` integrating NetworkManager (`org.freedesktop.NetworkManager`), PipeWire/WirePlumber (`wpctl`), BlueZ (`org.bluez`), UPower & PowerProfiles (`org.freedesktop.UPower`), Freedesktop Notifications v1.2 D-Bus server, PolicyKit1 `AuthenticationAgent` interface, and XDG Desktop Portals (`data/xdg-desktop-portal/neon-de-portals.conf`).
- [x] Integrated live Top Panel status pills (Network, PipeWire Volume, UPower Battery), Control Center / Quick Settings overlay, and live Notification Toast overlay into `neon-shell` off the UI thread.
- [x] Upgraded `neon-settings` *System* page with live PipeWire audio, NetworkManager Wi-Fi, BlueZ Bluetooth, UPower Power Profiles, Notification test, PolicyKit1, and XDG Portal status & controls.
- [x] Verified unit tests, live D-Bus `org.freedesktop.Notifications` v1.2 roundtrip test, live Linux subsystem snapshot probe, and live nested NEON DE session with `neon-shell` + `neon-settings` + D-Bus notification toast delivery.

## Milestone 8 Acceptance Criteria

- [x] Implemented bare-metal DRM/KMS + GBM + EGL/GLES + `libseat` + `libinput` + `udev` backend in `crates/neon-compositor/src/backend/drm.rs` alongside nested `winit` with `--backend auto|winit|drm` and `--safe-mode`.
- [x] Built `crates/neon-session` production supervisor with `neon-shell` automatic crash recovery, crash-loop protection, `--safe-mode`, and `--diagnostics`.
- [x] Added live compositor frame-timing telemetry (`IpcRequest::GetDiagnostics` / `DiagnosticsInfo`) and live high-contrast accessibility mode across `neon-compositor`, `neon-shell`, and `neon-settings`.
- [x] Packaged Wayland session `.desktop` entries (`neon-de.desktop`, `neon-de-safe.desktop`), desktop launchers, XDG portal config, systemd user units, and `scripts/install.sh` + `scripts/uninstall.sh`.
- [x] Verified full release build, zero clippy warnings, all unit & integration tests, live `neon-session` crash recovery test (killing `neon-shell` and confirming automatic respawn while `neon-compositor` stays alive), and system installation.
