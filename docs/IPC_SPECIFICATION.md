# NEON DE — IPC Specification

> **Status**: **Planned (Milestone 4)**. Not yet implemented in Milestone 1.

## 1. Transport & Addressing

- **Transport**: Unix domain stream socket (`AF_UNIX`, `SOCK_STREAM`) located at `$XDG_RUNTIME_DIR/neon-de/ipc-<wayland_display>.sock` with `0600` permissions.
- **Framing**: Length-prefixed UTF-8 JSON (or versioned binary framing) with explicit protocol version handshake (`protocol_version: 1`).
- **Authentication**: Kernel peer credentials (`SO_PEERCRED`) verified by `neon-compositor` to ensure same-UID access and capability checks before privileged requests.

## 2. Planned Request / Event Categories

- **Query**: `GetWindows`, `GetWorkspaces`, `GetOutputs`, `GetConfig`
- **Actions**: `SwitchWorkspace`, `MoveWindowToWorkspace`, `FocusWindow`, `CloseWindow`, `OpenOverview`, `CloseOverview`, `UpdateSetting`
- **Subscriptions**: `WindowEvents`, `WorkspaceEvents`, `OutputEvents`, `ConfigEvents`

## 3. Non-Goals & Security Boundaries

- **No arbitrary shell command execution** over IPC.
- System services (networking, Bluetooth, audio, power, authentication) use standard Linux D-Bus interfaces directly rather than duplicating them inside NEON IPC.
