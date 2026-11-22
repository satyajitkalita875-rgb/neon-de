# NEON DE — IPC Specification

> **Status**: **Implemented (Milestone 4 — Protocol Version 1)** in `crates/neon-ipc` and `crates/neon-compositor/src/ipc.rs`.

## 1. Transport, Addressing & Security

- **Transport**: Unix domain stream socket (`AF_UNIX`, `SOCK_STREAM`).
- **Path**: `${XDG_RUNTIME_DIR:-/run/user/$UID}/neon-de/ipc-<sanitized_wayland_display>.sock`.
  - Parent directory created with mode `0700`; socket file created with mode `0600`.
  - Exported automatically to spawned shell/settings clients via `NEON_IPC_SOCKET`.
- **Framing**: Newline-delimited UTF-8 JSON frames bounded strictly by `MAX_FRAME_BYTES = 262144` (256 KiB).
- **Versioning**: Every request and server message includes `version: 1` (`PROTOCOL_VERSION`). Mismatched versions receive structured `unsupported_version` error responses without crashing the server.
- **Peer Authentication**: Every accepted connection is verified via Linux `SO_PEERCRED` (`verify_peer_credentials`) to reject non-matching UIDs immediately.
- **No Arbitrary Shell Execution**: The IPC protocol exposes strictly typed desktop operations only; no raw shell or arbitrary command-execution endpoints exist.

## 2. Implemented Operations (`IpcRequest`)

| Request | Description |
|---|---|
| `get_state` | Returns full `DesktopSnapshot` (windows, workspaces, outputs, overview state, effects config) |
| `get_windows` | Lists all tracked `WindowInfo` records across workspaces |
| `get_workspaces` | Lists all ordered dynamic `WorkspaceInfo` records |
| `get_outputs` | Lists connected logical `OutputInfo` records |
| `switch_workspace { index }` | Switches active workspace to zero-based `index` |
| `move_window_to_workspace { window_id, workspace_index }` | Moves window to target workspace |
| `focus_window { window_id }` | Switches workspace if needed, restores if minimized, dismisses overview, and focuses window |
| `close_window { window_id }` | Sends graceful `xdg_toplevel.close` |
| `minimize_window { window_id }` | Minimizes window via state machine |
| `restore_window { window_id }` | Restores minimized window to its prior mode |
| `toggle_overview` / `set_overview { open }` | Controls Activities Overview state |
| `update_effects { ... }` | Live-updates animation, reduced-motion, shadow, border, and corner-radius settings |
| `subscribe { topics }` | Subscribes connection to `StateChanged` and `OverviewToggled` event pushes |

## 3. Server Events (`IpcEvent`)

- `state_changed { snapshot }` — pushed automatically whenever desktop state changes.
- `overview_toggled { open }` — pushed to `EventTopic::Overview` subscribers whenever Activities Overview opens or closes.
