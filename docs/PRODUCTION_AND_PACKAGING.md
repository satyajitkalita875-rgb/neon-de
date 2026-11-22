# NEON DE — Production Readiness, Crash Recovery & Packaging

Milestone 8 completes NEON DE's production architecture across bare-metal DRM/KMS execution, supervised crash recovery, profiling diagnostics, accessibility, and clean Linux packaging.

## 1. Dual Display Backends (`neon-compositor`)

`neon-compositor` supports automatic backend selection (`--backend auto|winit|drm`):

- **Nested Winit + GLES (`--backend winit`)**: Used automatically whenever `WAYLAND_DISPLAY` or `DISPLAY` is set in the environment. Runs safely inside an existing desktop without touching host sessions.
- **Bare-Metal DRM/KMS (`--backend drm`)**: Used automatically when launched from a TTY or display manager (SDDM / GDM). Integrates directly with:
  - `libseat` (`LibSeatSession`) for unprivileged seat & VT management (`PauseSession` / `ActivateSession` on VT switch)
  - `udev` (`UdevBackend`) for GPU node discovery (`/dev/dri/card*`)
  - `DRM/KMS` + `GBM` + `EGL/GLES` (`DrmOutputManager`, `GbmAllocator`, `GbmFramebufferExporter`, `GlesRenderer`) with VBlank-synchronized scanout and DMA-BUF zero-copy client rendering
  - `libinput` (`LibinputInputBackend`) for hardware keyboard, mouse, and touchpad gestures

## 2. Session Supervisor & Crash Recovery (`neon-session`)

`neon-session` is the canonical entrypoint referenced by `/usr/share/wayland-sessions/neon-de.desktop`:

- **Compositor Health Wait**: Waits for `neon-compositor` to bind its authenticated IPC socket and answer `GetDiagnostics` before launching `neon-shell`.
- **Automatic `neon-shell` Crash Recovery**: If `neon-shell` exits unexpectedly while `neon-compositor` is running, `neon-session` restarts it automatically with progressive backoff and a crash-loop guard (max 5 restarts per 30 seconds). If the crash-loop limit is reached, `neon-compositor` stays alive in recovery mode so open user applications are never lost.
- **Safe Mode (`--safe-mode`)**: Launches `neon-compositor` with blur, shadows, and animations disabled (`NEON DE (Safe Mode)` session entry).
- **Diagnostics (`--diagnostics`)**: Reports binary locations, `/dev/dri` DRM nodes, config schema health, NetworkManager/PipeWire/BlueZ/UPower/Polkit/Portal status, and live compositor frame telemetry.

## 3. Profiling & Accessibility

- **Frame Telemetry (`IpcRequest::GetDiagnostics`)**: Tracks `backend`, `uptime_secs`, `frames_rendered`, `idle_frames_skipped`, `last_frame_time_us`, `window_count`, `workspace_count`, `output_count`, and `safe_mode`, displayed live in `neon-settings` (*System*) and `neon-session --diagnostics`.
- **High-Contrast Mode & Reduced Motion**: Toggling *High Contrast* in `neon-settings` (*Privacy & Accessibility*) applies opaque, high-contrast surfaces and borders live across `neon-shell` via IPC; *Reduce Motion* disables transitions immediately.

## 4. Installing & Uninstalling NEON DE

```bash
# System-wide release install (adds NEON DE to SDDM/GDM alongside existing desktops)
./scripts/install.sh

# Or user-only install under ~/.local (no sudo needed)
./scripts/install.sh --user

# Verify health after install
neon-session --diagnostics

# Launch safely in nested mode anytime
neon-session --nested

# Clean uninstall
./scripts/uninstall.sh
```

Neither `install.sh` nor `uninstall.sh` modifies existing Hyprland/GNOME/KDE configurations or SDDM default session settings.
