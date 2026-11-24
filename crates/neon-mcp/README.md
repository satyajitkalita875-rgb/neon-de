# `neon-mcp` — NEON DE Model Context Protocol (MCP) Control Server

`neon-mcp` is a lightweight, security-hardened **MCP stdio JSON-RPC 2.0 server** built into NEON DE that lets AI agents (Antigravity, Claude Desktop/Code, Cursor, etc.) inspect and control the live NEON DE desktop safely via NEON DE's authenticated Unix socket IPC (`neon-ipc`) and native Linux D-Bus / PipeWire APIs (`neon-system`).

## Security Model
- **Same-UID authenticated IPC**: Connects to `neon-compositor`'s `0600` Unix socket verified by Linux `SO_PEERCRED`.
- **Zero arbitrary shell execution**: Every action is strictly schema-validated; `launch_app` only launches verified installed XDG `.desktop` entries with field codes stripped (never `sh -c`).
- **Auto-discovery**: Automatically finds the live `$XDG_RUNTIME_DIR/neon-de/ipc-*.sock` socket even when launched outside the desktop session tree (override with `--socket <path>` or `NEON_IPC_SOCKET`).

## Exposed MCP Tools (25)
- **Inspect (read-only)**: `get_desktop_state`, `get_diagnostics`, `list_windows`, `list_workspaces`, `list_outputs`, `get_system_status`, `list_apps`
- **Windows**: `focus_window`, `close_window`, `minimize_window`, `restore_window`, `toggle_maximize_window`, `snap_window`, `move_window_to_workspace`
- **Workspaces & Shell**: `switch_workspace`, `add_workspace`, `remove_workspace`, `set_app_drawer`, `take_screenshot`, `lock_session`
- **Appearance, Effects & Monitors**: `update_appearance`, `update_effects`, `configure_output`
- **System, Audio, Brightness, Media & Notifications**: `control_system`, `send_notification`
- **Apps**: `launch_app`

## Exposed MCP Resources (3)
- `neon://desktop/state`
- `neon://system/status`
- `neon://config`

## Client Configuration Example (Antigravity / Claude / Cursor)

```json
{
  "mcpServers": {
    "neon-de": {
      "command": "/usr/local/bin/neon-mcp"
    }
  }
}
```
