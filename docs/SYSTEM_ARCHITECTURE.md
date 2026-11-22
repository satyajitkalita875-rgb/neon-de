# NEON DE — System Architecture

## 1. Architectural Principles

NEON DE is designed as an independent Wayland desktop environment with strict separation of concerns:

1. **Compositor / Shell Split**: `neon-compositor` owns the Wayland display server, input devices, output lifecycle, window policy, scene graph, and GPU composition. `neon-shell` (GTK4) renders panel, dock, overview, launcher, and quick settings surfaces as unprivileged Wayland clients communicating over a typed IPC protocol.
2. **Recovery by Design**: `neon-compositor` can operate independently without `neon-shell` for debugging, safe mode, and crash recovery.
3. **Incremental Crate Extraction**: Milestone 1 keeps internal subsystems modular inside `crates/neon-compositor/src/` with clear trait/struct boundaries. Crates (`neon-window-manager`, `neon-renderer`, `neon-scene`, `neon-workspaces`, `neon-effects`, `neon-input`, `neon-output`, `neon-ipc`, `neon-config`, `neon-shell`, `neon-settings`, etc.) are extracted only once their interfaces stabilize.
4. **Reuse Mature Linux Plumbing**: Integrate with `systemd-logind`, `libinput`, `udev`, PipeWire/WirePlumber, NetworkManager, UPower, BlueZ, polkit, and XDG Desktop Portals rather than reinventing system daemons.

## 2. Target Process Topology

```text
NEON SESSION (planned — Milestone 8)
├── neon-compositor  [IMPLEMENTED: Milestone 1]
│   ├── Wayland Display Server (wayland-server / Smithay)
│   ├── Event Loop (calloop)
│   ├── Backend Layer
│   │   ├── Nested Winit + GLES/Glow Backend [IMPLEMENTED: Milestone 1]
│   │   ├── Headless Testing Backend         [IMPLEMENTED: Milestone 1 (tests)]
│   │   └── DRM/KMS + GBM + libinput Backend [PLANNED: Milestone 5]
│   ├── Window Manager (floating policy, focus, lifecycle) [M1 basics / M2 full]
│   ├── Input Router (keyboard, pointer, focus, shortcuts) [M1 basics / M5 gestures]
│   ├── Output Manager (logical outputs, modes, scale)     [M1 single / M5 multi]
│   ├── Scene Graph & Renderer (damage tracking, effects)  [M1 Space / M3 Scene]
│   ├── Workspace Manager (dynamic workspaces)             [PLANNED: Milestone 2]
│   └── IPC Server (Unix domain socket)                    [PLANNED: Milestone 4]
├── neon-shell       [PLANNED: Milestone 4] (GTK4 + layer-shell + NEON IPC)
├── neon-settings    [PLANNED: Milestone 6] (GTK4 macOS-inspired settings UI)
├── neon-notifications [PLANNED: Milestone 7] (org.freedesktop.Notifications)
├── neon-lockscreen  [PLANNED: Milestone 5] (ext-session-lock-v1 client)
└── Linux System Services (PipeWire, NetworkManager, UPower, BlueZ, polkit, portals)
```

## 3. Current Workspace Crate Structure (Milestone 5)

```text
crates/
├── neon-workspaces/          # GNOME-inspired dynamic workspace manager (WorkspaceManager, WorkspaceId)
├── neon-window-manager/      # WindowMode state machine, placement, snapping, focus policy, window rules
├── neon-output/              # Multi-monitor discovery, arrangement, fractional scale, rotation, mirroring, hotplug
├── neon-effects/             # Time-based interruptible animations (Bézier, spring, focus/open/workspace slide, blur config)
├── neon-scene/               # Retained Z-layered scene graph (Background..Cursor, shadows, borders, dirty regions)
├── neon-renderer/            # Modular GLES/Glow pipeline, SDF shadow/border/backdrop/frosted-glass shaders, lock curtain
├── neon-ipc/                 # Versioned Unix domain socket IPC + SO_PEERCRED auth + typed snapshots/events
├── neon-compositor/          # Smithay 0.7.0 compositor (layer-shell, session-lock, fractional-scale, gestures, XWayland, DMA-BUF)
├── neon-shell/               # Custom Rust + GTK4 + gtk4-layer-shell Desktop Shell (Top Panel, Dock, Overview, XDG App Grid)
└── neon-lockscreen/          # Standalone ext_session_lock_v1 Wayland lock screen client
```

## 4. Key Architectural Decisions & Trade-offs

| Decision | Choice | Rationale & Trade-off |
|---|---|---|
| Language | Rust (Edition 2024) | Memory safety for a long-lived display server processing untrusted client requests; zero-cost abstractions. Trade-off: longer compile times than C. |
| Compositor Framework | Smithay `0.7.0` | Actively maintained Rust Wayland toolkit with DRM/KMS, EGL/GLES, Winit, and Wayland protocol implementations. Trade-off: Smithay API evolves between minor releases; pinned via `Cargo.lock`. |
| Event Loop | `calloop` (via Smithay) | Single-threaded deterministic dispatch of Wayland sockets, timers, and backend events without lock contention. |
| Initial Backend | Nested `winit` + `GlesRenderer` | Safe development inside an existing desktop session without risking the host display or TTY. DRM/KMS comes later once core window management is stable. |
| Window Policy | Floating-first (`WindowRegistry` + `Space`) | Matches GNOME-like usability out of the box; optional snapping/tiling layers cleanly on top in Milestone 2. |
| Shell Toolkit | GTK4 (Milestone 4+) | Native Wayland accessibility, typography, and modern styling; decoupled via IPC so the compositor never depends on GTK. |

## 5. Hardware & Graphics Strategy

Initial validation targets Intel integrated graphics (CometLake-H UHD Graphics, Mesa EGL/GLES/GBM) before validating NVIDIA proprietary/open drivers (GTX 1650 Mobile) separately. Software fallback (`LIBGL_ALWAYS_SOFTWARE=1`) is supported in nested mode for debugging and headless CI.
