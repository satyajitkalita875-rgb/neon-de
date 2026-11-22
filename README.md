# NEON DE

**NEON DE** is an independent, next-generation Linux desktop environment built in **Rust** with **Smithay** and **Wayland**.

> **Project Status**: **Milestone 1 (Initial Compositor)** — Custom Wayland compositor running on a nested `winit` + OpenGL ES (`glow`) backend with XDG Shell client support, keyboard and mouse input, floating window management basics, and click-to-focus. Desktop shell, dynamic workspaces, effects engine, settings application, and DRM/KMS bare-metal session are planned for subsequent milestones.

## Vision

NEON DE combines:

- GNOME-inspired spatial workflow and clean visual aesthetics
- macOS-inspired settings clarity and polish
- KDE-grade configurability without visual clutter
- GPU-accelerated Wayland compositing written in Rust on top of Smithay

NEON DE is **not** a desktop theme, GNOME extension, KDE customization, or wrapper around another compositor.

## Architecture Overview

NEON DE separates responsibilities cleanly across processes and modules:

- **`neon-compositor`** *(implemented — Milestone 1)*: Custom Wayland display server, window lifecycle, input routing, focus management, output handling, and GPU rendering via Smithay.
- **`neon-shell`** *(planned — Milestone 4)*: GTK4 desktop shell (top panel, dock, activities overview, launcher, quick settings).
- **`neon-settings`** *(planned — Milestone 6)*: GTK4 macOS-inspired settings application communicating over typed IPC.
- **`neon-ipc` / `neon-config`** *(planned — Milestones 4–6)*: Versioned Unix socket IPC and centralized XDG configuration.

See [docs/SYSTEM_ARCHITECTURE.md](docs/SYSTEM_ARCHITECTURE.md) for the complete architecture.

## Quick Start (Nested Development Session)

NEON DE Milestone 1 runs strictly inside a nested window on your existing X11 or Wayland session — it never modifies your host session or login manager.

```bash
# Build the compositor
cargo build -p neon-compositor

# Run nested and automatically launch a terminal inside NEON DE
./scripts/run-nested.sh

# Or run manually with a custom client command
cargo run -p neon-compositor -- --command weston-terminal
```

## Documentation

- [System Architecture](docs/SYSTEM_ARCHITECTURE.md)
- [Development Roadmap](docs/DEVELOPMENT_ROADMAP.md)
- [Wayland Protocols](docs/WAYLAND_PROTOCOLS.md)
- [Rendering Design](docs/RENDERING_DESIGN.md)
- [Window Manager Design](docs/WINDOW_MANAGER_DESIGN.md)
- [IPC Specification](docs/IPC_SPECIFICATION.md)
- [Configuration Specification](docs/CONFIGURATION_SPECIFICATION.md)
- [Security Model](docs/SECURITY_MODEL.md)
- [Build Instructions](docs/BUILD_INSTRUCTIONS.md)
- [Testing Guide](docs/TESTING_GUIDE.md)
- [Contributing](docs/CONTRIBUTING.md)
