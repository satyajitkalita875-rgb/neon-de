//! Typed NEON DE desktop, window, workspace, appearance, hardware, and application MCP tools.
//!
//! Every tool maps directly onto NEON DE's authenticated IPC (`neon-ipc`) or native Linux
//! D-Bus / PipeWire / XDG APIs (`neon-system`). Zero arbitrary shell execution endpoints exist.

use std::{path::Path, process::Command};

use neon_config::{default_config_path, load_or_default};
use neon_ipc::{IpcPayload, IpcRequest};
use neon_system::{
    SystemAction,
    apps::{InstalledApp, discover_installed_apps},
    execute_system_action,
    notifications::{NotificationUrgency, send_dbus_notification},
    power::read_backlight_percent,
    query_system_snapshot,
};
use serde_json::{Value, json};

use crate::ipc::NeonBridge;

/// Returns the complete MCP `tools/list` schema catalog (25 typed tools).
pub fn list_tool_definitions() -> Vec<Value> {
    vec![
        // --- Inspection (Read-Only) ---
        tool_def(
            "get_desktop_state",
            "Returns the complete live NEON DE desktop snapshot (windows, workspaces, outputs, appearance, effects, and app drawer state).",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            true,
            false,
            true,
        ),
        tool_def(
            "get_diagnostics",
            "Returns live NEON DE compositor diagnostics (backend, uptime, frames rendered, frame time in microseconds, window/workspace/output counts).",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            true,
            false,
            true,
        ),
        tool_def(
            "list_windows",
            "Lists all open toplevel windows across all desktops with IDs, titles, app IDs, bounds, workspace IDs, and focused/minimized/maximized/fullscreen state.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            true,
            false,
            true,
        ),
        tool_def(
            "list_workspaces",
            "Lists all active NEON DE desktops/workspaces (up to 10) with their indices, active state, and member window IDs.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            true,
            false,
            true,
        ),
        tool_def(
            "list_outputs",
            "Lists all connected monitors with logical positions, resolutions, refresh rates, scaling factors, transforms, and primary status.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            true,
            false,
            true,
        ),
        tool_def(
            "get_system_status",
            "Reads live Linux desktop hardware status: Wi-Fi, Bluetooth, PipeWire audio volume/mute/sink, display backlight brightness %, battery & power profile, and MPRIS media playback.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            true,
            false,
            true,
        ),
        // --- Window Control ---
        tool_def(
            "focus_window",
            "Focuses and raises a window by its numeric window_id (automatically switching to its desktop and restoring it if minimized).",
            json!({
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "minimum": 1, "description": "Window ID from list_windows" }
                },
                "required": ["window_id"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "close_window",
            "Gracefully closes a window by window_id (with Material 3 Expressive / Hyprland popout close animation).",
            json!({
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "minimum": 1, "description": "Window ID from list_windows" }
                },
                "required": ["window_id"],
                "additionalProperties": false
            }),
            false,
            true,
            false,
        ),
        tool_def(
            "minimize_window",
            "Minimizes a window smoothly down to the Dock.",
            json!({
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "minimum": 1, "description": "Window ID from list_windows" }
                },
                "required": ["window_id"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "restore_window",
            "Restores a minimized window smoothly up from the Dock and focuses it.",
            json!({
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "minimum": 1, "description": "Window ID from list_windows" }
                },
                "required": ["window_id"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "toggle_maximize_window",
            "Toggles maximized state on a window with smooth Hyprland/M3 spatial spring animation.",
            json!({
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "minimum": 1, "description": "Window ID from list_windows" }
                },
                "required": ["window_id"],
                "additionalProperties": false
            }),
            false,
            false,
            false,
        ),
        tool_def(
            "snap_window",
            "Snaps a window to a screen half or corner quarter.",
            json!({
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "minimum": 1, "description": "Window ID from list_windows" },
                    "region": {
                        "type": "string",
                        "enum": ["left", "right", "top_left", "top_right", "bottom_left", "bottom_right"],
                        "description": "Target snap region"
                    }
                },
                "required": ["window_id", "region"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "move_window_to_workspace",
            "Moves a window to another zero-based workspace_index.",
            json!({
                "type": "object",
                "properties": {
                    "window_id": { "type": "integer", "minimum": 1 },
                    "workspace_index": { "type": "integer", "minimum": 0, "maximum": 9 }
                },
                "required": ["window_id", "workspace_index"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        // --- Workspaces & Shell UI ---
        tool_def(
            "switch_workspace",
            "Switches the active desktop to zero-based index (0..9) with smooth horizontal slide animation.",
            json!({
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "minimum": 0, "maximum": 9, "description": "Zero-based desktop index" }
                },
                "required": ["index"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "add_workspace",
            "Creates a new persistent desktop in NEON DE (up to 10 desktops total).",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            false,
            false,
            false,
        ),
        tool_def(
            "remove_workspace",
            "Removes the desktop at zero-based index (keeps at least 1 desktop; relocates any windows safely).",
            json!({
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "minimum": 0, "maximum": 9 }
                },
                "required": ["index"],
                "additionalProperties": false
            }),
            false,
            true,
            false,
        ),
        tool_def(
            "set_app_drawer",
            "Opens or closes the fullscreen blurred NEON DE App Drawer (Launchpad).",
            json!({
                "type": "object",
                "properties": {
                    "open": { "type": "boolean", "description": "true to open, false to close" }
                },
                "required": ["open"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "take_screenshot",
            "Captures a native hardware-accelerated PNG screenshot of the desktop (or an optional [x, y, width, height] region) and returns the saved path.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Optional absolute PNG output path (defaults to ~/Pictures/Screenshots)" },
                    "region": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "minItems": 4,
                        "maxItems": 4,
                        "description": "Optional logical crop rectangle [x, y, width, height]"
                    }
                },
                "additionalProperties": false
            }),
            false,
            false,
            false,
        ),
        tool_def(
            "lock_session",
            "Locks the NEON DE Wayland session immediately via ext_session_lock_v1.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            false,
            false,
            true,
        ),
        // --- Appearance, Effects & Displays ---
        tool_def(
            "update_appearance",
            "Updates live NEON DE visual appearance settings (theme, accent color, wallpaper, blur reduction, cursor theme/size, panel/dock transparency).",
            json!({
                "type": "object",
                "properties": {
                    "theme": { "type": "string", "enum": ["neon_dark", "neon_light"] },
                    "accent_color": { "type": "string", "description": "Hex color such as #38BDF8" },
                    "wallpaper": { "type": "string", "description": "Absolute path to wallpaper image" },
                    "reduce_blur": { "type": "boolean" },
                    "blur_strength": { "type": "number", "minimum": 0.0, "maximum": 2.0 },
                    "cursor_theme": { "type": "string" },
                    "cursor_size": { "type": "integer", "minimum": 16, "maximum": 48 },
                    "dock_transparency": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "panel_transparency": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "panel_visible": { "type": "boolean" },
                    "show_clock": { "type": "boolean" },
                    "show_workspaces": { "type": "boolean" },
                    "high_contrast": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "update_effects",
            "Updates live NEON DE compositor visual effects and animation speed.",
            json!({
                "type": "object",
                "properties": {
                    "enabled": { "type": "boolean" },
                    "reduced_motion": { "type": "boolean" },
                    "shadows_enabled": { "type": "boolean" },
                    "borders_enabled": { "type": "boolean" },
                    "blur_enabled": { "type": "boolean" },
                    "blur_strength": { "type": "number", "minimum": 0.0, "maximum": 2.0 },
                    "inactive_opacity": { "type": "number", "minimum": 0.4, "maximum": 1.0 },
                    "corner_radius": { "type": "number", "minimum": 0.0, "maximum": 28.0 },
                    "speed_scale": { "type": "number", "minimum": 0.25, "maximum": 4.0 }
                },
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        tool_def(
            "configure_output",
            "Updates live monitor layout, scaling, orientation, or primary selection.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Output name from list_outputs (e.g. EmbeddedDisplayPort-1)" },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "scale": { "type": "number", "minimum": 0.5, "maximum": 4.0 },
                    "primary": { "type": "boolean" },
                    "transform": {
                        "type": "string",
                        "enum": ["normal", "90", "180", "270", "flipped", "flipped-90", "flipped-180", "flipped-270"]
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            false,
            false,
            true,
        ),
        // --- Hardware, System & Notifications ---
        tool_def(
            "control_system",
            "Controls live Linux desktop hardware & services: display brightness (5-100%), PipeWire audio volume (0-150%) or mute, Wi-Fi, Bluetooth, UPower profile, and MPRIS media playback.",
            json!({
                "type": "object",
                "properties": {
                    "brightness_percent": { "type": "integer", "minimum": 5, "maximum": 100 },
                    "volume_percent": { "type": "integer", "minimum": 0, "maximum": 150 },
                    "mute": { "type": "string", "enum": ["mute", "unmute", "toggle"] },
                    "wifi_enabled": { "type": "boolean" },
                    "bluetooth_powered": { "type": "boolean" },
                    "power_profile": { "type": "string", "enum": ["power-saver", "balanced", "performance"] },
                    "media_action": { "type": "string", "enum": ["play_pause", "next", "previous"] }
                },
                "additionalProperties": false
            }),
            false,
            false,
            false,
        ),
        tool_def(
            "send_notification",
            "Displays a native NEON DE desktop notification toast via Freedesktop Notifications D-Bus.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Notification summary/title" },
                    "body": { "type": "string", "description": "Notification body text" },
                    "app_name": { "type": "string", "description": "Optional sender label (default: NEON MCP)" },
                    "urgency": { "type": "string", "enum": ["low", "normal", "critical"] },
                    "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 30000 }
                },
                "required": ["title", "body"],
                "additionalProperties": false
            }),
            false,
            false,
            false,
        ),
        // --- Verified XDG Application Discovery & Launch ---
        tool_def(
            "list_apps",
            "Searches installed XDG desktop applications (returns desktop_id, name, comment, categories).",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Optional case-insensitive filter on app name, ID, or description" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Max results (default 40)" }
                },
                "additionalProperties": false
            }),
            true,
            false,
            true,
        ),
        tool_def(
            "launch_app",
            "Launches a verified installed XDG desktop application (by desktop_id or name, e.g. 'org.neonde.settings.desktop', 'firefox', 'Files') inside the live NEON DE session. Rejects arbitrary shell commands.",
            json!({
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "Exact desktop_id or installed application name from list_apps" }
                },
                "required": ["app"],
                "additionalProperties": false
            }),
            false,
            false,
            false,
        ),
    ]
}

/// Returns MCP `resources/list` definitions.
pub fn list_resource_definitions() -> Vec<Value> {
    vec![
        json!({
            "uri": "neon://desktop/state",
            "name": "NEON DE Live Desktop State",
            "description": "Real-time compositor state: windows, workspaces, outputs, appearance, effects, and overview status.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "neon://system/status",
            "name": "NEON DE Live System & Hardware Status",
            "description": "Real-time Linux subsystem status: Wi-Fi, Bluetooth, PipeWire audio, backlight brightness, battery, power profile, and MPRIS media.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "neon://config",
            "name": "NEON DE Persistent Configuration",
            "description": "Current validated ~/.config/neon-de/config.toml settings.",
            "mimeType": "application/json"
        }),
    ]
}

/// Reads a NEON DE MCP resource URI.
pub fn read_resource(uri: &str, bridge: &NeonBridge) -> Result<Value, String> {
    match uri {
        "neon://desktop/state" => dispatch_tool("get_desktop_state", &json!({}), bridge),
        "neon://system/status" => dispatch_tool("get_system_status", &json!({}), bridge),
        "neon://config" => {
            let cfg = load_or_default(&default_config_path());
            serde_json::to_value(&cfg)
                .map_err(|e| format!("failed to serialize NEON DE config: {e}"))
        }
        other => Err(format!("unknown NEON DE resource URI: {other}")),
    }
}

/// Dispatches an MCP `tools/call` invocation by tool name and JSON arguments.
pub fn dispatch_tool(name: &str, args: &Value, bridge: &NeonBridge) -> Result<Value, String> {
    match name {
        "get_desktop_state" => match bridge.request(IpcRequest::GetState)? {
            IpcPayload::State { snapshot } => to_json(snapshot),
            other => to_json(other),
        },
        "get_diagnostics" => match bridge.request(IpcRequest::GetDiagnostics)? {
            IpcPayload::Diagnostics { diagnostics } => to_json(diagnostics),
            other => to_json(other),
        },
        "list_windows" => match bridge.request(IpcRequest::GetWindows)? {
            IpcPayload::Windows { windows } => Ok(json!({
                "count": windows.len(),
                "windows": windows,
            })),
            other => to_json(other),
        },
        "list_workspaces" => match bridge.request(IpcRequest::GetWorkspaces)? {
            IpcPayload::Workspaces { workspaces } => Ok(json!({
                "count": workspaces.len(),
                "max_workspaces": 10,
                "workspaces": workspaces,
            })),
            other => to_json(other),
        },
        "list_outputs" => match bridge.request(IpcRequest::GetOutputs)? {
            IpcPayload::Outputs { outputs } => Ok(json!({
                "count": outputs.len(),
                "outputs": outputs,
            })),
            other => to_json(other),
        },
        "get_system_status" => {
            let snap = query_system_snapshot();
            let brightness = read_backlight_percent();
            Ok(json!({
                "brightness_percent": brightness,
                "network": snap.network,
                "bluetooth": snap.bluetooth,
                "audio": snap.audio,
                "power": snap.power,
                "media": snap.media,
                "notifications": snap.notifications,
            }))
        }
        "focus_window" => {
            let window_id = req_u64(args, "window_id")?;
            bridge.request(IpcRequest::FocusWindow { window_id })?;
            Ok(json!({ "ok": true, "focused_window_id": window_id }))
        }
        "close_window" => {
            let window_id = req_u64(args, "window_id")?;
            bridge.request(IpcRequest::CloseWindow { window_id })?;
            Ok(json!({ "ok": true, "closing_window_id": window_id }))
        }
        "minimize_window" => {
            let window_id = req_u64(args, "window_id")?;
            bridge.request(IpcRequest::MinimizeWindow { window_id })?;
            Ok(json!({ "ok": true, "minimized_window_id": window_id }))
        }
        "restore_window" => {
            let window_id = req_u64(args, "window_id")?;
            bridge.request(IpcRequest::RestoreWindow { window_id })?;
            Ok(json!({ "ok": true, "restored_window_id": window_id }))
        }
        "toggle_maximize_window" => {
            let window_id = req_u64(args, "window_id")?;
            bridge.request(IpcRequest::ToggleMaximizeWindow { window_id })?;
            Ok(json!({ "ok": true, "toggled_maximize_window_id": window_id }))
        }
        "snap_window" => {
            let window_id = req_u64(args, "window_id")?;
            let region = req_str(args, "region")?.to_string();
            bridge.request(IpcRequest::SnapWindow {
                window_id,
                region: region.clone(),
            })?;
            Ok(json!({ "ok": true, "window_id": window_id, "region": region }))
        }
        "move_window_to_workspace" => {
            let window_id = req_u64(args, "window_id")?;
            let workspace_index = req_usize(args, "workspace_index")?;
            bridge.request(IpcRequest::MoveWindowToWorkspace {
                window_id,
                workspace_index,
            })?;
            Ok(json!({ "ok": true, "window_id": window_id, "workspace_index": workspace_index }))
        }
        "switch_workspace" => {
            let index = req_usize(args, "index")?;
            bridge.request(IpcRequest::SwitchWorkspace { index })?;
            Ok(json!({ "ok": true, "active_workspace_index": index }))
        }
        "add_workspace" => {
            bridge.request(IpcRequest::AddWorkspace)?;
            let workspaces = match bridge.request(IpcRequest::GetWorkspaces) {
                Ok(IpcPayload::Workspaces { workspaces }) => Some(workspaces),
                _ => None,
            };
            Ok(json!({
                "ok": true,
                "count": workspaces.as_ref().map(Vec::len),
                "workspaces": workspaces,
            }))
        }
        "remove_workspace" => {
            let index = req_usize(args, "index")?;
            bridge.request(IpcRequest::RemoveWorkspace { index })?;
            Ok(json!({ "ok": true, "removed_workspace_index": index }))
        }
        "set_app_drawer" => {
            let open = args
                .get("open")
                .and_then(Value::as_bool)
                .ok_or_else(|| "missing required boolean parameter: open".to_string())?;
            bridge.request(IpcRequest::SetOverview { open })?;
            Ok(json!({ "ok": true, "app_drawer_open": open }))
        }
        "take_screenshot" => {
            let path = args.get("path").and_then(Value::as_str).map(String::from);
            if let Some(ref p) = path
                && (p.contains("..") || !Path::new(p).is_absolute())
            {
                return Err("screenshot path must be an absolute path without '..'".to_string());
            }
            let region = match args.get("region") {
                None | Some(Value::Null) => None,
                Some(Value::Array(arr)) if arr.len() == 4 => {
                    let mut vals = [0i32; 4];
                    for (i, item) in arr.iter().enumerate() {
                        vals[i] = item
                            .as_i64()
                            .ok_or_else(|| "region elements must be integers".to_string())?
                            as i32;
                    }
                    if vals[2] <= 0 || vals[3] <= 0 {
                        return Err("region width and height must be positive".to_string());
                    }
                    Some(vals)
                }
                _ => return Err("region must be a 4-integer array [x, y, width, height]".to_string()),
            };
            match bridge.request(IpcRequest::TakeScreenshot { path, region })? {
                IpcPayload::Screenshot { path } => Ok(json!({ "ok": true, "path": path })),
                other => to_json(other),
            }
        }
        "lock_session" => {
            bridge.request(IpcRequest::LockSession)?;
            Ok(json!({ "ok": true, "locked": true }))
        }
        "update_appearance" => {
            let req = IpcRequest::UpdateAppearance {
                theme: opt_str(args, "theme"),
                accent_color: opt_str(args, "accent_color"),
                dock_transparency: opt_f32(args, "dock_transparency"),
                panel_transparency: opt_f32(args, "panel_transparency"),
                panel_visible: opt_bool(args, "panel_visible"),
                show_clock: opt_bool(args, "show_clock"),
                show_workspaces: opt_bool(args, "show_workspaces"),
                high_contrast: opt_bool(args, "high_contrast"),
                wallpaper: opt_str(args, "wallpaper"),
                reduce_blur: opt_bool(args, "reduce_blur"),
                blur_strength: opt_f32(args, "blur_strength"),
                cursor_theme: opt_str(args, "cursor_theme"),
                cursor_size: opt_u32(args, "cursor_size"),
            };
            match bridge.request(req)? {
                IpcPayload::Appearance { appearance } => {
                    Ok(json!({ "ok": true, "appearance": appearance }))
                }
                other => to_json(other),
            }
        }
        "update_effects" => {
            let req = IpcRequest::UpdateEffects {
                enabled: opt_bool(args, "enabled"),
                reduced_motion: opt_bool(args, "reduced_motion"),
                shadows_enabled: opt_bool(args, "shadows_enabled"),
                borders_enabled: opt_bool(args, "borders_enabled"),
                blur_enabled: opt_bool(args, "blur_enabled"),
                blur_strength: opt_f32(args, "blur_strength"),
                inactive_opacity: opt_f32(args, "inactive_opacity"),
                corner_radius: opt_f32(args, "corner_radius"),
                speed_scale: opt_f64(args, "speed_scale"),
            };
            match bridge.request(req)? {
                IpcPayload::Effects { effects } => Ok(json!({ "ok": true, "effects": effects })),
                other => to_json(other),
            }
        }
        "configure_output" => {
            let name = req_str(args, "name")?.to_string();
            let req = IpcRequest::ConfigureOutput {
                name,
                x: opt_i32(args, "x"),
                y: opt_i32(args, "y"),
                scale: opt_f64(args, "scale"),
                primary: opt_bool(args, "primary"),
                transform: opt_str(args, "transform"),
            };
            match bridge.request(req)? {
                IpcPayload::Outputs { outputs } => Ok(json!({ "ok": true, "outputs": outputs })),
                other => to_json(other),
            }
        }
        "control_system" => handle_control_system(args),
        "send_notification" => handle_send_notification(args),
        "list_apps" => handle_list_apps(args),
        "launch_app" => handle_launch_app(args, bridge),
        other => Err(format!("unknown MCP tool: {other}")),
    }
}

fn handle_control_system(args: &Value) -> Result<Value, String> {
    let mut applied: Vec<String> = Vec::new();

    if let Some(pct) = opt_u32(args, "brightness_percent") {
        let clamped = pct.clamp(5, 100);
        execute_system_action(&SystemAction::SetBrightness(clamped))?;
        applied.push(format!("brightness={clamped}%"));
    }
    if let Some(pct) = opt_u32(args, "volume_percent") {
        let clamped = pct.clamp(0, 150);
        execute_system_action(&SystemAction::SetSinkVolume(clamped as f32 / 100.0))?;
        applied.push(format!("volume={clamped}%"));
    }
    if let Some(mute_cmd) = opt_str(args, "mute") {
        match mute_cmd.as_str() {
            "mute" => execute_system_action(&SystemAction::SetSinkMute(true))?,
            "unmute" => execute_system_action(&SystemAction::SetSinkMute(false))?,
            "toggle" => execute_system_action(&SystemAction::ToggleSinkMute)?,
            other => return Err(format!("invalid mute action: {other}")),
        }
        applied.push(format!("mute={mute_cmd}"));
    }
    if let Some(wifi) = opt_bool(args, "wifi_enabled") {
        execute_system_action(&SystemAction::SetWifiEnabled(wifi))?;
        applied.push(format!("wifi={wifi}"));
    }
    if let Some(bt) = opt_bool(args, "bluetooth_powered") {
        execute_system_action(&SystemAction::SetBluetoothPowered(bt))?;
        applied.push(format!("bluetooth={bt}"));
    }
    if let Some(prof) = opt_str(args, "power_profile") {
        if !matches!(prof.as_str(), "power-saver" | "balanced" | "performance") {
            return Err(format!("invalid power_profile: {prof}"));
        }
        execute_system_action(&SystemAction::SetPowerProfile(prof.clone()))?;
        applied.push(format!("power_profile={prof}"));
    }
    if let Some(media) = opt_str(args, "media_action") {
        let method = match media.as_str() {
            "play_pause" => "PlayPause",
            "next" => "Next",
            "previous" => "Previous",
            other => return Err(format!("invalid media_action: {other}")),
        };
        execute_system_action(&SystemAction::MediaCommand {
            bus_name: None,
            method: method.to_string(),
        })?;
        applied.push(format!("media={media}"));
    }

    if applied.is_empty() {
        return Err(
            "provide at least one field: brightness_percent, volume_percent, mute, wifi_enabled, bluetooth_powered, power_profile, or media_action".to_string(),
        );
    }

    Ok(json!({ "ok": true, "applied": applied }))
}

fn handle_send_notification(args: &Value) -> Result<Value, String> {
    let title = req_str(args, "title")?;
    let body = req_str(args, "body")?;
    let app_name = opt_str(args, "app_name").unwrap_or_else(|| "NEON MCP".to_string());
    let urgency = match opt_str(args, "urgency").as_deref() {
        Some("low") => NotificationUrgency::Low,
        Some("critical") => NotificationUrgency::Critical,
        _ => NotificationUrgency::Normal,
    };
    let timeout_ms = opt_i32(args, "timeout_ms").unwrap_or(4500).clamp(1000, 30000);

    for bus in ["org.freedesktop.Notifications", "org.neonde.Notifications"] {
        if let Ok(id) = send_dbus_notification(bus, &app_name, title, body, urgency, timeout_ms) {
            return Ok(json!({ "ok": true, "notification_id": id, "bus": bus }));
        }
    }
    Err("no active desktop notification daemon responded on session D-Bus".to_string())
}

fn handle_list_apps(args: &Value) -> Result<Value, String> {
    let query = opt_str(args, "query").unwrap_or_default().to_lowercase();
    let limit = opt_u32(args, "limit").unwrap_or(40).clamp(1, 200) as usize;
    let apps = discover_installed_apps();
    let matched: Vec<Value> = apps
        .into_iter()
        .filter(|a| {
            if query.is_empty() {
                return true;
            }
            a.name.to_lowercase().contains(&query)
                || a.desktop_id.to_lowercase().contains(&query)
                || a.comment
                    .as_deref()
                    .map(|c| c.to_lowercase().contains(&query))
                    .unwrap_or(false)
        })
        .take(limit)
        .map(|a| {
            json!({
                "desktop_id": a.desktop_id,
                "name": a.name,
                "comment": a.comment,
                "categories": a.categories,
            })
        })
        .collect();
    Ok(json!({ "count": matched.len(), "apps": matched }))
}

fn handle_launch_app(args: &Value, bridge: &NeonBridge) -> Result<Value, String> {
    let target = req_str(args, "app")?.trim();
    if target.is_empty() {
        return Err("app parameter cannot be empty".to_string());
    }

    let apps = discover_installed_apps();
    let lower = target.to_lowercase();
    let found: InstalledApp = apps
        .iter()
        .find(|a| a.desktop_id.eq_ignore_ascii_case(target))
        .or_else(|| apps.iter().find(|a| a.name.eq_ignore_ascii_case(target)))
        .or_else(|| {
            apps.iter().find(|a| {
                a.desktop_id
                    .strip_suffix(".desktop")
                    .map(|s| s.eq_ignore_ascii_case(target))
                    .unwrap_or(false)
            })
        })
        .or_else(|| {
            apps.iter()
                .find(|a| a.name.to_lowercase().contains(&lower))
        })
        .cloned()
        .ok_or_else(|| {
            format!("no installed XDG desktop application matched '{target}' (use list_apps first)")
        })?;

    let (program, cmd_args) = parse_desktop_exec(&found.exec)
        .ok_or_else(|| format!("application '{}' has an invalid Exec line", found.name))?;

    let mut cmd = Command::new(&program);
    cmd.args(&cmd_args);

    // Wire the launched app cleanly onto the live NEON DE Wayland, IPC, and XWayland sockets.
    if let Ok(sock) = bridge.resolve_socket_path() {
        cmd.env("NEON_IPC_SOCKET", &sock);
        if let Some(wl) = NeonBridge::infer_wayland_display(&sock) {
            cmd.env("WAYLAND_DISPLAY", wl);
        }
    }
    if let Ok(IpcPayload::State { snapshot }) = bridge.request(IpcRequest::GetState) {
        if let Some(ref disp) = snapshot.xwayland_display {
            cmd.env("DISPLAY", disp);
        }
        if let Some(ref ct) = snapshot.appearance.cursor_theme {
            cmd.env("XCURSOR_THEME", ct);
        }
        if let Some(cs) = snapshot.appearance.cursor_size {
            cmd.env("XCURSOR_SIZE", cs.to_string());
        }
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch '{}' ({program}): {e}", found.name))?;

    Ok(json!({
        "ok": true,
        "desktop_id": found.desktop_id,
        "name": found.name,
        "pid": child.id(),
    }))
}

/// Safely splits an XDG Desktop Entry `Exec=` value into `(program, args)`, stripping standard
/// XDG field codes (`%u`, `%U`, `%f`, `%F`, `%i`, `%c`, `%k`, etc.) without invoking a shell.
pub fn parse_desktop_exec(exec: &str) -> Option<(String, Vec<String>)> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in exec.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }

    let filtered: Vec<String> = tokens
        .into_iter()
        .filter(|tok| {
            !(tok.starts_with('%') && tok.len() == 2 && tok.as_bytes()[1].is_ascii_alphabetic())
        })
        .collect();

    let mut iter = filtered.into_iter();
    let prog = iter.next()?;
    if prog.is_empty() {
        return None;
    }
    Some((prog, iter.collect()))
}

fn tool_def(
    name: &str,
    description: &str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": idempotent,
            "openWorldHint": false
        }
    })
}

fn to_json<T: serde::Serialize>(val: T) -> Result<Value, String> {
    serde_json::to_value(val).map_err(|e| format!("failed to serialize response: {e}"))
}

fn req_u64(args: &Value, key: &str) -> Result<u64, String> {
    args.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing or invalid integer parameter: {key}"))
}

fn req_usize(args: &Value, key: &str) -> Result<usize, String> {
    req_u64(args, key).map(|v| v as usize)
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or invalid string parameter: {key}"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(String::from)
}

fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

fn opt_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(Value::as_u64).map(|v| v as u32)
}

fn opt_i32(args: &Value, key: &str) -> Option<i32> {
    args.get(key).and_then(Value::as_i64).map(|v| v as i32)
}

fn opt_f32(args: &Value, key: &str) -> Option<f32> {
    args.get(key).and_then(Value::as_f64).map(|v| v as f32)
}

fn opt_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufReader, Cursor, Write},
        thread,
    };

    use neon_ipc::{
        IpcRequestEnvelope, IpcResult, IpcServerMessage, PROTOCOL_VERSION, WorkspaceInfo,
        bind_ipc_listener, encode_frame, read_frame,
    };

    use crate::protocol::run_stdio_loop;

    #[test]
    fn parse_desktop_exec_strips_xdg_field_codes_and_preserves_quoted_args() {
        let (prog, args) = parse_desktop_exec(r#"/usr/bin/firefox --new-window "My Profile" %U"#)
            .expect("valid exec");
        assert_eq!(prog, "/usr/bin/firefox");
        assert_eq!(args, vec!["--new-window", "My Profile"]);
        assert!(parse_desktop_exec("   ").is_none());
    }

    #[test]
    fn tool_and_resource_catalogs_are_valid_and_unique() {
        let tools = list_tool_definitions();
        assert!(tools.len() >= 25);
        let mut names = std::collections::HashSet::new();
        for t in &tools {
            let name = t["name"].as_str().expect("tool name");
            assert!(names.insert(name), "duplicate MCP tool name: {name}");
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        assert_eq!(list_resource_definitions().len(), 3);
    }

    #[test]
    fn mcp_stdio_roundtrip_initialize_tools_and_mock_ipc_calls() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("ipc-wayland-test.sock");
        let listener = bind_ipc_listener(&sock_path).unwrap();
        listener.set_nonblocking(false).unwrap();

        // Spawn a lightweight mock NEON DE IPC compositor handling 2 requests.
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let env: IpcRequestEnvelope = read_frame(&mut reader).unwrap();
                let payload = match env.request {
                    IpcRequest::GetWorkspaces => IpcPayload::Workspaces {
                        workspaces: vec![
                            WorkspaceInfo {
                                id: 1,
                                index: 0,
                                active: true,
                                window_ids: vec![42],
                            },
                            WorkspaceInfo {
                                id: 2,
                                index: 1,
                                active: false,
                                window_ids: vec![],
                            },
                        ],
                    },
                    IpcRequest::SetOverview { open } => IpcPayload::Overview { open },
                    other => panic!("unexpected mock IPC request: {other:?}"),
                };
                let resp = IpcServerMessage::Response {
                    version: PROTOCOL_VERSION,
                    id: env.id,
                    result: IpcResult::Ok { payload },
                };
                stream.write_all(&encode_frame(&resp).unwrap()).unwrap();
                stream.flush().unwrap();
            }
        });

        let input = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}).to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_workspaces","arguments":{}}}).to_string(),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"set_app_drawer","arguments":{"open":true}}}).to_string(),
            json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"take_screenshot","arguments":{"path":"relative/evil.png"}}}).to_string(),
        ]
        .join("\n")
            + "\n";

        let mut out = Vec::new();
        let bridge = NeonBridge::new(Some(sock_path));
        run_stdio_loop(Cursor::new(input.as_bytes()), &mut out, &bridge).unwrap();
        server.join().unwrap();

        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        // 5 responses (notifications/initialized produces no response).
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "neon-mcp");
        assert!(lines[1]["result"]["tools"].as_array().unwrap().len() >= 25);
        assert_eq!(lines[2]["result"]["isError"], false);
        assert_eq!(lines[2]["result"]["structuredContent"]["count"], 2);
        assert_eq!(lines[3]["result"]["isError"], false);
        assert_eq!(lines[3]["result"]["structuredContent"]["app_drawer_open"], true);
        assert_eq!(lines[4]["result"]["isError"], true);
    }
}
