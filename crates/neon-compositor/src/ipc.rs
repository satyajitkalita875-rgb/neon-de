//! Authenticated Unix domain socket IPC server embedded inside `neon-compositor`.

use std::{
    collections::HashSet,
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
};

use neon_config::{DockPosition, FocusMode, PanelMode, ThemeMode, WindowButtonLayout};
use neon_ipc::{
    AppearanceInfo, DesktopSnapshot, DiagnosticsInfo, EffectsInfo, EventTopic, IpcEvent,
    IpcPayload, IpcRequest, IpcRequestEnvelope, IpcResult, IpcServerMessage, MAX_FRAME_BYTES,
    OutputInfo, PROTOCOL_VERSION, WindowBehaviorInfo, WindowInfo, WorkspaceInfo, bind_ipc_listener,
    encode_frame, verify_peer_credentials,
};
use neon_output::{OutputConfigPatch, OutputTransform};
use neon_window_manager::{WindowId, geometry::Point as WmPoint};
use smithay::utils::SERIAL_COUNTER;
use tracing::{debug, info, warn};

use crate::state::NeonState;

struct ConnectedIpcClient {
    stream: UnixStream,
    peer_pid: i32,
    read_buf: Vec<u8>,
    subscriptions: HashSet<EventTopic>,
}

/// Non-blocking IPC server driven inside `neon-compositor`'s main `calloop` event loop.
pub struct IpcServer {
    socket_path: PathBuf,
    listener: UnixListener,
    clients: Vec<ConnectedIpcClient>,
    last_broadcast_snapshot: Option<DesktopSnapshot>,
}

impl IpcServer {
    pub fn bind(socket_path: PathBuf) -> io::Result<Self> {
        let listener = bind_ipc_listener(&socket_path)?;
        info!(path = %socket_path.display(), "Bound NEON DE IPC socket");
        Ok(Self {
            socket_path,
            listener,
            clients: Vec::new(),
            last_broadcast_snapshot: None,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    /// Accepts all pending incoming Unix socket connections after verifying `SO_PEERCRED`.
    pub fn accept_pending(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => match verify_peer_credentials(&stream) {
                    Ok(creds) => {
                        if let Err(err) = stream.set_nonblocking(true) {
                            warn!(?err, "Failed to set IPC client nonblocking");
                            continue;
                        }
                        debug!(
                            pid = creds.pid,
                            uid = creds.uid,
                            "Accepted authenticated IPC client"
                        );
                        self.clients.push(ConnectedIpcClient {
                            stream,
                            peer_pid: creds.pid,
                            read_buf: Vec::with_capacity(4096),
                            subscriptions: HashSet::new(),
                        });
                    }
                    Err(err) => {
                        warn!(?err, "Rejected IPC client failing SO_PEERCRED verification");
                    }
                },
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    warn!(?err, "Error accepting IPC client");
                    break;
                }
            }
        }
    }

    /// Drains incoming JSON-line requests from connected IPC clients and dispatches them.
    pub fn poll_requests(&mut self, state: &mut NeonState) {
        self.accept_pending();

        let mut i = 0;
        while i < self.clients.len() {
            let mut disconnected = false;
            let mut pending_lines: Vec<String> = Vec::new();

            {
                let client = &mut self.clients[i];
                let mut tmp = [0u8; 4096];
                loop {
                    match client.stream.read(&mut tmp) {
                        Ok(0) => {
                            disconnected = true;
                            break;
                        }
                        Ok(n) => {
                            if client.read_buf.len() + n > MAX_FRAME_BYTES {
                                warn!(pid = client.peer_pid, "IPC client exceeded MAX_FRAME_BYTES");
                                disconnected = true;
                                break;
                            }
                            client.read_buf.extend_from_slice(&tmp[..n]);
                        }
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => {
                            disconnected = true;
                            break;
                        }
                    }
                }

                while let Some(pos) = client.read_buf.iter().position(|&b| b == b'\n') {
                    let line_bytes: Vec<u8> = client.read_buf.drain(..=pos).collect();
                    match String::from_utf8(line_bytes) {
                        Ok(s) => pending_lines.push(s),
                        Err(_) => {
                            disconnected = true;
                            break;
                        }
                    }
                }
            }

            for line in pending_lines {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let response = match serde_json::from_str::<IpcRequestEnvelope>(trimmed) {
                    Ok(env) => {
                        if env.version != PROTOCOL_VERSION {
                            IpcServerMessage::Response {
                                version: PROTOCOL_VERSION,
                                id: env.id,
                                result: IpcResult::Err {
                                    code: "unsupported_version".into(),
                                    message: format!(
                                        "Expected IPC protocol version {}, got {}",
                                        PROTOCOL_VERSION, env.version
                                    ),
                                },
                            }
                        } else {
                            let res = handle_ipc_request(
                                state,
                                &mut self.clients[i].subscriptions,
                                env.request,
                            );
                            IpcServerMessage::Response {
                                version: PROTOCOL_VERSION,
                                id: env.id,
                                result: res,
                            }
                        }
                    }
                    Err(err) => IpcServerMessage::Response {
                        version: PROTOCOL_VERSION,
                        id: 0,
                        result: IpcResult::Err {
                            code: "invalid_request".into(),
                            message: err.to_string(),
                        },
                    },
                };

                if let Ok(frame) = encode_frame(&response)
                    && self.clients[i].stream.write_all(&frame).is_err()
                {
                    disconnected = true;
                    break;
                }
            }

            if disconnected {
                let removed = self.clients.swap_remove(i);
                debug!(pid = removed.peer_pid, "IPC client disconnected");
            } else {
                i += 1;
            }
        }

        // Push state diffs to subscribed clients if anything changed.
        self.broadcast_if_changed(state);
    }

    /// Broadcasts `IpcEvent::StateChanged` to subscribed clients whenever the desktop snapshot changes.
    pub fn broadcast_if_changed(&mut self, state: &mut NeonState) {
        if !self.clients.iter().any(|c| !c.subscriptions.is_empty()) {
            return;
        }
        let screenshot_ui_frame = if state.pending_screenshot_ui {
            state.pending_screenshot_ui = false;
            Some(encode_frame(&IpcServerMessage::Event {
                version: PROTOCOL_VERSION,
                event: IpcEvent::ScreenshotUiRequested,
            }))
        } else {
            None
        };
        let clipboard_frame = if state.pending_clipboard_toggle {
            state.pending_clipboard_toggle = false;
            Some(encode_frame(&IpcServerMessage::Event {
                version: PROTOCOL_VERSION,
                event: IpcEvent::ClipboardToggleRequested,
            }))
        } else {
            None
        };
        let island_frame = if state.pending_island_toggle {
            state.pending_island_toggle = false;
            Some(encode_frame(&IpcServerMessage::Event {
                version: PROTOCOL_VERSION,
                event: IpcEvent::DynamicIslandToggleRequested,
            }))
        } else {
            None
        };
        let drag_frame = state.pending_drag_state.take().map(|active| {
            encode_frame(&IpcServerMessage::Event {
                version: PROTOCOL_VERSION,
                event: IpcEvent::DragStateChanged { active },
            })
        });
        let context_menu_frame =
            state
                .pending_context_menu
                .take()
                .map(|(x, y, target)| {
                    encode_frame(&IpcServerMessage::Event {
                        version: PROTOCOL_VERSION,
                        event: IpcEvent::ContextMenuRequested { x, y, target },
                    })
                });

        let snapshot = build_snapshot(state);
        if screenshot_ui_frame.is_none()
            && clipboard_frame.is_none()
            && island_frame.is_none()
            && drag_frame.is_none()
            && context_menu_frame.is_none()
            && self.last_broadcast_snapshot.as_ref() == Some(&snapshot)
        {
            return;
        }

        let overview_changed = self
            .last_broadcast_snapshot
            .as_ref()
            .map(|prev| prev.overview_open != snapshot.overview_open)
            .unwrap_or(true);

        let state_frame = encode_frame(&IpcServerMessage::Event {
            version: PROTOCOL_VERSION,
            event: IpcEvent::StateChanged {
                snapshot: snapshot.clone(),
            },
        });
        let overview_frame = overview_changed.then(|| {
            encode_frame(&IpcServerMessage::Event {
                version: PROTOCOL_VERSION,
                event: IpcEvent::OverviewToggled {
                    open: snapshot.overview_open,
                },
            })
        });

        let mut i = 0;
        while i < self.clients.len() {
            let client = &mut self.clients[i];
            let mut failed = false;

            if !client.subscriptions.is_empty()
                && let Ok(bytes) = &state_frame
                && client.stream.write_all(bytes).is_err()
            {
                failed = true;
            }

            if !failed
                && client.subscriptions.contains(&EventTopic::Overview)
                && let Some(Ok(bytes)) = &overview_frame
                && client.stream.write_all(bytes).is_err()
            {
                failed = true;
            }

            if !failed
                && !client.subscriptions.is_empty()
                && let Some(Ok(bytes)) = &screenshot_ui_frame
                && client.stream.write_all(bytes).is_err()
            {
                failed = true;
            }

            if !failed
                && !client.subscriptions.is_empty()
                && let Some(Ok(bytes)) = &clipboard_frame
                && client.stream.write_all(bytes).is_err()
            {
                failed = true;
            }

            if !failed
                && !client.subscriptions.is_empty()
                && let Some(Ok(bytes)) = &island_frame
                && client.stream.write_all(bytes).is_err()
            {
                failed = true;
            }

            if !failed
                && !client.subscriptions.is_empty()
                && let Some(Ok(bytes)) = &drag_frame
                && client.stream.write_all(bytes).is_err()
            {
                failed = true;
            }

            if !failed
                && !client.subscriptions.is_empty()
                && let Some(Ok(bytes)) = &context_menu_frame
                && client.stream.write_all(bytes).is_err()
            {
                failed = true;
            }

            if failed {
                self.clients.swap_remove(i);
            } else {
                i += 1;
            }
        }

        self.last_broadcast_snapshot = Some(snapshot);
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

fn handle_ipc_request(
    state: &mut NeonState,
    subscriptions: &mut HashSet<EventTopic>,
    request: IpcRequest,
) -> IpcResult {
    let serial = SERIAL_COUNTER.next_serial();
    match request {
        IpcRequest::GetState => IpcResult::Ok {
            payload: IpcPayload::State {
                snapshot: build_snapshot(state),
            },
        },
        IpcRequest::GetDiagnostics => IpcResult::Ok {
            payload: IpcPayload::Diagnostics {
                diagnostics: DiagnosticsInfo {
                    backend: state.backend_name.clone(),
                    uptime_secs: state.started_at.elapsed().as_secs(),
                    frames_rendered: state.frames_rendered,
                    idle_frames_skipped: state.idle_frames_skipped,
                    last_frame_time_us: state.last_frame_time_us,
                    window_count: state.managed_windows.len(),
                    workspace_count: state.wm.workspaces().len(),
                    output_count: state.output_manager.all_outputs().len(),
                    safe_mode: state.safe_mode,
                },
            },
        },
        IpcRequest::GetWindows => IpcResult::Ok {
            payload: IpcPayload::Windows {
                windows: build_windows(state),
            },
        },
        IpcRequest::GetWorkspaces => IpcResult::Ok {
            payload: IpcPayload::Workspaces {
                workspaces: build_workspaces(state),
            },
        },
        IpcRequest::GetOutputs => IpcResult::Ok {
            payload: IpcPayload::Outputs {
                outputs: build_outputs(state),
            },
        },
        IpcRequest::SwitchWorkspace { index } => {
            if index < neon_workspaces::MAX_WORKSPACES {
                state.switch_workspace_index(index, serial);
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "invalid_workspace".into(),
                    message: format!("Workspace index {index} out of bounds"),
                }
            }
        }
        IpcRequest::AddWorkspace => {
            if state.add_workspace().is_some() {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "workspace_limit".into(),
                    message: "Maximum of 10 desktops reached".into(),
                }
            }
        }
        IpcRequest::RemoveWorkspace { index } => {
            if state.remove_workspace(index, serial) {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "invalid_workspace".into(),
                    message: format!("Cannot remove desktop at index {index}"),
                }
            }
        }
        IpcRequest::TogglePinWorkspace { index } => {
            if state.toggle_pin_workspace(index, serial).is_some() {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "invalid_workspace".into(),
                    message: format!("Cannot pin desktop at index {index}"),
                }
            }
        }
        IpcRequest::MoveWindowToWorkspace {
            window_id,
            workspace_index,
        } => {
            let id = WindowId(window_id);
            if state.wm.get(id).is_none() {
                return IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                };
            }
            if state
                .wm
                .workspaces()
                .workspaces()
                .get(workspace_index)
                .is_none()
            {
                return IpcResult::Err {
                    code: "invalid_workspace".into(),
                    message: format!("Workspace index {workspace_index} out of bounds"),
                };
            }
            // Move window directly to requested workspace index (no intermediate dynamic pruning).
            let old_ws = state.wm.get(id).map(|w| w.workspace_id);
            if let Some(new_ws) = state
                .wm
                .move_window_to_workspace_index(id, workspace_index, false)
            {
                if let Some(old_ws) = old_ws {
                    state.relayout_auto_tiling_for_workspace(old_ws);
                }
                state.relayout_auto_tiling_for_workspace(new_ws);
                state.sync_active_workspace_space(serial);
                state.needs_redraw = true;
            }
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::FocusWindow { window_id } => {
            let id = WindowId(window_id);
            let Some(record) = state.wm.get(id).cloned() else {
                return IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                };
            };
            // Switch to the window's workspace if needed, and restore if minimized!
            if let Some(ws_idx) = state.wm.workspaces().index_of(record.workspace_id)
                && ws_idx != state.wm.workspaces().active_index()
            {
                state.switch_workspace_index(ws_idx, serial);
            }
            if record.mode.is_minimized() {
                state.restore_minimized_window(id, serial);
            }
            if let Some(window) = state.find_window_by_id(id) {
                state.set_overview_open(false, serial);
                state.focus_window(Some(&window), serial);
            }
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::CloseWindow { window_id } => {
            let id = WindowId(window_id);
            if state.close_window_by_id(id) {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            }
        }
        IpcRequest::MinimizeWindow { window_id } => {
            let id = WindowId(window_id);
            if state.wm.get(id).is_some() {
                state.minimize_window(id, serial);
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            }
        }
        IpcRequest::RestoreWindow { window_id } => {
            let id = WindowId(window_id);
            if state.restore_minimized_window(id, serial) {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "not_minimized".into(),
                    message: format!("Window {window_id} is not minimized"),
                }
            }
        }
        IpcRequest::ToggleMaximizeWindow { window_id } => {
            let id = WindowId(window_id);
            if state.wm.get(id).is_some() {
                state.toggle_maximize_window(id);
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            }
        }
        IpcRequest::ToggleFullscreenWindow { window_id } => {
            let id = WindowId(window_id);
            if state.wm.get(id).is_some() {
                state.toggle_fullscreen_window(id);
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            }
        }
        IpcRequest::SnapWindow { window_id, region } => {
            let id = WindowId(window_id);
            let parsed = crate::window::SnapRegion::from_name(&region);
            if state.wm.get(id).is_none() {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            } else if let Some(snap_region) = parsed {
                state.snap_window_by_id(id, snap_region);
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "invalid_region".into(),
                    message: format!("Unknown snap region '{region}'"),
                }
            }
        }
        IpcRequest::ToggleWindowFloat { window_id } => {
            let target = window_id.map(WindowId);
            if let Some(id) = target
                && state.wm.get(id).is_none()
            {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {} does not exist", id.0),
                }
            } else {
                state.toggle_window_float(target);
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            }
        }
        IpcRequest::ToggleAlwaysOnTop { window_id } => {
            let id = WindowId(window_id);
            if state.toggle_always_on_top_by_id(id, serial).is_some() {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            }
        }
        IpcRequest::CenterWindow { window_id } => {
            let id = WindowId(window_id);
            if state.center_window_by_id(id) {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            }
        }
        IpcRequest::SetFloatingWindow { window_id } => {
            let id = WindowId(window_id);
            if state.wm.get(id).is_some() {
                let _ = state.set_floating_window_by_id(id);
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            }
        }
        IpcRequest::MoveWindowToOutput {
            window_id,
            output_name,
        } => {
            let id = WindowId(window_id);
            if state.wm.get(id).is_none() {
                IpcResult::Err {
                    code: "window_not_found".into(),
                    message: format!("Window {window_id} does not exist"),
                }
            } else if state.move_window_to_output_by_name(id, &output_name) {
                IpcResult::Ok {
                    payload: IpcPayload::Ack,
                }
            } else {
                IpcResult::Err {
                    code: "output_not_found".into(),
                    message: format!("Output '{output_name}' not found"),
                }
            }
        }
        IpcRequest::ToggleOverview => {
            let open = state.toggle_overview(serial);
            IpcResult::Ok {
                payload: IpcPayload::Overview { open },
            }
        }
        IpcRequest::SetOverview { open } => {
            state.set_overview_open(open, serial);
            IpcResult::Ok {
                payload: IpcPayload::Overview { open },
            }
        }
        IpcRequest::LockSession => {
            state.lock_session();
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::OpenScreenshotUi => {
            state.pending_screenshot_ui = true;
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::ToggleClipboard => {
            state.pending_clipboard_toggle = true;
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::ToggleDynamicIsland => {
            state.pending_island_toggle = true;
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::AnimateOverlay { namespace, opening } => {
            state.pending_overlay_anims.push((namespace, opening));
            state.needs_redraw = true;
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::SetWindowDockAnchor { window_id, x, y } => {
            state
                .window_dock_anchors
                .insert(WindowId(window_id), (x, y).into());
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::SetLaunchOrigin { x, y } => {
            state.recent_launch_origin = Some(((x, y).into(), std::time::Instant::now()));
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::TakeScreenshot { path, region } => {
            let dest = state.request_screenshot(path.map(std::path::PathBuf::from), region);
            IpcResult::Ok {
                payload: IpcPayload::Screenshot {
                    path: dest.to_string_lossy().into_owned(),
                },
            }
        }
        IpcRequest::GetScreencastTarget => IpcResult::Ok {
            payload: IpcPayload::ScreencastTarget {
                target: state.screencast_target.clone(),
            },
        },
        IpcRequest::SetScreencastTarget { target } => {
            info!(?target, "Updated active NEON DE screencast capture target");
            state.screencast_target = target.clone();
            state.needs_redraw = true;
            IpcResult::Ok {
                payload: IpcPayload::ScreencastTarget { target },
            }
        }
        IpcRequest::CaptureScreencastPreview {
            target,
            path,
            max_width,
            max_height,
        } => {
            let p = std::path::PathBuf::from(&path);
            if !p.is_absolute() || path.contains("..") || !path.ends_with(".png") {
                return IpcResult::Err {
                    code: "invalid_path".into(),
                    message: "preview path must be an absolute .png path without '..'".into(),
                };
            }
            let mw = max_width.unwrap_or(320).clamp(64, 960);
            let mh = max_height.unwrap_or(200).clamp(64, 720);
            state
                .pending_screencast_previews
                .push(crate::state::PendingPreviewCapture {
                    target,
                    path: p,
                    max_width: mw,
                    max_height: mh,
                });
            state.needs_redraw = true;
            IpcResult::Ok {
                payload: IpcPayload::Ack,
            }
        }
        IpcRequest::ConfigureOutput {
            name,
            x,
            y,
            scale,
            primary,
            transform,
        } => {
            let parsed_transform =
                transform
                    .as_deref()
                    .and_then(|s| match s.to_lowercase().as_str() {
                        "normal" => Some(OutputTransform::Normal),
                        "90" | "rotate90" | "rotate_90" => Some(OutputTransform::Rotate90),
                        "180" | "rotate180" | "rotate_180" => Some(OutputTransform::Rotate180),
                        "270" | "rotate270" | "rotate_270" => Some(OutputTransform::Rotate270),
                        "flipped" => Some(OutputTransform::Flipped),
                        _ => None,
                    });
            let position = match (x, y) {
                (Some(px), Some(py)) => Some(WmPoint::new(px, py)),
                _ => None,
            };
            let patch = OutputConfigPatch {
                position,
                scale,
                primary,
                transform: parsed_transform,
                ..Default::default()
            };
            if state.configure_output_by_name(&name, &patch).is_some() {
                IpcResult::Ok {
                    payload: IpcPayload::Outputs {
                        outputs: build_outputs(state),
                    },
                }
            } else {
                IpcResult::Err {
                    code: "output_not_found".into(),
                    message: format!("Output '{name}' not found"),
                }
            }
        }
        IpcRequest::UpdateEffects {
            enabled,
            reduced_motion,
            shadows_enabled,
            borders_enabled,
            blur_enabled,
            blur_strength,
            inactive_opacity,
            corner_radius,
            speed_scale,
            open_animation,
            close_animation,
            minimize_animation,
            maximize_animation,
            workspace_animation,
            overview_animation,
            dock_animation,
        } => {
            if let Some(v) = enabled {
                state.effects.config.enabled = v;
                state.config.effects.enabled = v;
            }
            if let Some(v) = reduced_motion {
                state.effects.config.reduced_motion = v;
                state.config.effects.reduced_motion = v;
            }
            if let Some(v) = shadows_enabled {
                state.effects.config.shadows_enabled = v;
                state.config.effects.shadows_enabled = v;
            }
            if let Some(v) = borders_enabled {
                state.effects.config.borders_enabled = v;
                state.config.effects.borders_enabled = v;
            }
            if let Some(v) = blur_enabled {
                state.effects.config.blur_enabled = v;
                state.config.effects.blur_enabled = v;
                if v {
                    state.config.appearance.reduce_blur = false;
                }
            }
            if let Some(v) = blur_strength {
                let clamped = v.clamp(0.0, 2.0);
                state.effects.config.blur_strength = clamped;
                state.config.effects.blur_strength = clamped;
                if blur_enabled.is_none() {
                    let on = clamped > 0.01;
                    state.effects.config.blur_enabled = on;
                    state.config.effects.blur_enabled = on;
                    state.config.appearance.reduce_blur = !on;
                }
            }
            if let Some(v) = inactive_opacity {
                let clamped = v.clamp(0.4, 1.0);
                state.effects.config.inactive_opacity = clamped;
                state.config.effects.inactive_opacity = clamped;
            }
            if let Some(v) = corner_radius {
                let clamped = v.clamp(0.0, 32.0);
                state.effects.config.corner_radius = clamped;
                state.config.effects.corner_radius = clamped;
            }
            if let Some(v) = speed_scale {
                let clamped = v.clamp(0.1, 5.0);
                state.effects.config.speed_scale = clamped;
                state.config.effects.speed_scale = clamped;
            }
            if let Some(v) = open_animation {
                state.effects.config.open_animation = v;
                state.config.effects.open_animation = v;
            }
            if let Some(v) = close_animation {
                state.effects.config.close_animation = v;
                state.config.effects.close_animation = v;
            }
            if let Some(v) = minimize_animation {
                state.effects.config.minimize_animation = v;
                state.config.effects.minimize_animation = v;
            }
            if let Some(v) = maximize_animation {
                state.effects.config.maximize_animation = v;
                state.config.effects.maximize_animation = v;
            }
            if let Some(v) = workspace_animation {
                state.effects.config.workspace_animation = v;
                state.config.effects.workspace_animation = v;
            }
            if let Some(v) = overview_animation {
                state.effects.config.overview_animation = v;
                state.config.effects.overview_animation = v;
            }
            if let Some(v) = dock_animation {
                state.effects.config.dock_animation = v;
                state.config.effects.dock_animation = v;
            }
            let any_action_explicit = open_animation.is_some()
                || close_animation.is_some()
                || minimize_animation.is_some()
                || maximize_animation.is_some()
                || workspace_animation.is_some()
                || overview_animation.is_some()
                || dock_animation.is_some();
            if enabled == Some(true)
                && !any_action_explicit
                && !state.effects.config.open_animation
                && !state.effects.config.close_animation
                && !state.effects.config.minimize_animation
                && !state.effects.config.maximize_animation
                && !state.effects.config.workspace_animation
                && !state.effects.config.overview_animation
                && !state.effects.config.dock_animation
            {
                state.effects.config.open_animation = true;
                state.effects.config.close_animation = true;
                state.effects.config.minimize_animation = true;
                state.effects.config.maximize_animation = true;
                state.effects.config.workspace_animation = true;
                state.effects.config.overview_animation = true;
                state.effects.config.dock_animation = true;
                state.config.effects.open_animation = true;
                state.config.effects.close_animation = true;
                state.config.effects.minimize_animation = true;
                state.config.effects.maximize_animation = true;
                state.config.effects.workspace_animation = true;
                state.config.effects.overview_animation = true;
                state.config.effects.dock_animation = true;
            }
            state.config.validate_and_clamp();
            if state.backend_name != "headless" {
                let cfg_path = neon_config::default_config_path();
                if let Err(err) = neon_config::save_atomic(&cfg_path, &state.config) {
                    warn!(?err, "Failed to persist live effects config update");
                }
            }
            state.needs_redraw = true;
            IpcResult::Ok {
                payload: IpcPayload::Effects {
                    effects: build_effects(state),
                },
            }
        }
        IpcRequest::UpdateAppearance {
            theme,
            accent_color,
            dock_transparency,
            dock_auto_hide,
            dock_smart_hide,
            dock_position,
            dock_icon_size,
            dock_height,
            dock_magnification,
            dock_pinned_apps,
            panel_transparency,
            panel_visible,
            panel_smart_hide,
            panel_mode,
            dynamic_island_enabled,
            show_clock,
            clock_24h,
            show_workspaces,
            show_app_title,
            show_system_tray,
            show_battery,
            show_quick_settings,
            show_desktop_icons,
            desktop_sort_by,
            desktop_auto_arrange,
            high_contrast,
            wallpaper,
            reduce_blur,
            blur_strength,
            cursor_theme,
            cursor_size,
        } => {
            if let Some(t) = theme {
                state.config.appearance.theme = match t.as_str() {
                    "neon_light" | "light" => ThemeMode::NeonLight,
                    "auto" => ThemeMode::Auto,
                    _ => ThemeMode::NeonDark,
                };
            }
            if let Some(hex) = accent_color {
                state.config.appearance.accent_color = hex;
            }
            if let Some(v) = dock_transparency {
                state.config.dock.transparency = v;
            }
            let mut work_area_policy_changed = false;
            if let Some(v) = dock_auto_hide {
                work_area_policy_changed |= state.config.dock.auto_hide != v;
                state.config.dock.auto_hide = v;
            }
            if let Some(v) = dock_smart_hide {
                work_area_policy_changed |= state.config.dock.intelligent_hide != v;
                state.config.dock.intelligent_hide = v;
            }
            if let Some(pos) = dock_position {
                let parsed = match pos.to_lowercase().as_str() {
                    "left" => DockPosition::Left,
                    "right" => DockPosition::Right,
                    _ => DockPosition::Bottom,
                };
                work_area_policy_changed |= state.config.dock.position != parsed;
                state.config.dock.position = parsed;
            }
            if let Some(sz) = dock_icon_size {
                state.config.dock.icon_size = sz.clamp(22, 64);
            }
            if let Some(h) = dock_height {
                let clamped = h.clamp(36, 88);
                work_area_policy_changed |= state.config.dock.height != clamped;
                state.config.dock.height = clamped;
            }
            if let Some(mag) = dock_magnification {
                state.config.dock.magnification = mag;
            }
            if let Some(pinned) = dock_pinned_apps {
                state.config.dock.pinned_apps = pinned;
            }
            if let Some(v) = panel_transparency {
                state.config.panel.transparency = v;
            }
            if let Some(ref mode_str) = panel_mode {
                let next_mode = if mode_str.eq_ignore_ascii_case("combined_bottom") {
                    PanelMode::CombinedBottom
                } else {
                    PanelMode::TopAndDock
                };
                work_area_policy_changed |= state.config.panel.mode != next_mode;
                state.config.panel.mode = next_mode;
            }
            if let Some(v) = dynamic_island_enabled {
                state.config.panel.dynamic_island_enabled = v;
            }
            if let Some(v) = panel_visible
                && (state.config.panel.mode == PanelMode::TopAndDock || v)
            {
                work_area_policy_changed |= state.config.panel.visible != v;
                state.config.panel.visible = v;
            }
            if let Some(v) = panel_smart_hide {
                work_area_policy_changed |= state.config.panel.smart_hide != v;
                state.config.panel.smart_hide = v;
            }
            if work_area_policy_changed {
                state.relayout_non_floating_windows();
            }
            if let Some(v) = show_clock {
                state.config.panel.show_clock = v;
            }
            if let Some(v) = clock_24h {
                state.config.panel.clock_24h = v;
            }
            if let Some(v) = show_workspaces {
                state.config.panel.show_workspaces = v;
            }
            if let Some(v) = show_app_title {
                state.config.panel.show_app_title = v;
            }
            if let Some(v) = show_system_tray {
                state.config.panel.show_system_tray = v;
            }
            if let Some(v) = show_battery {
                state.config.panel.show_battery = v;
            }
            if let Some(v) = show_quick_settings {
                state.config.panel.show_quick_settings = v;
            }
            if let Some(v) = show_desktop_icons {
                state.config.desktop.show_desktop_icons = v;
            }
            if let Some(s) = desktop_sort_by {
                state.config.desktop.desktop_sort_by = s;
            }
            if let Some(v) = desktop_auto_arrange {
                state.config.desktop.desktop_auto_arrange = v;
            }
            if let Some(v) = high_contrast {
                state.config.privacy_accessibility.high_contrast = v;
            }
            if let Some(w) = wallpaper {
                state.config.appearance.wallpaper_path = if w.is_empty() { None } else { Some(w) };
            }
            if let Some(rb) = reduce_blur {
                state.config.appearance.reduce_blur = rb;
                if rb && blur_strength.is_none() {
                    state.config.effects.blur_enabled = false;
                    state.effects.config.blur_enabled = false;
                }
            }
            if let Some(bs) = blur_strength {
                let clamped = bs.clamp(0.0, 2.0);
                state.config.effects.blur_strength = clamped;
                state.effects.config.blur_strength = clamped;
                let on = clamped > 0.01 && !reduce_blur.unwrap_or(false);
                state.config.effects.blur_enabled = on;
                state.effects.config.blur_enabled = on;
                if reduce_blur.is_none() {
                    state.config.appearance.reduce_blur = !on;
                }
            }
            if let Some(ct) = cursor_theme
                && !ct.trim().is_empty()
            {
                state.config.appearance.cursor_theme = ct;
            }
            if let Some(cs) = cursor_size {
                state.config.appearance.cursor_size = cs.clamp(16, 48);
            }
            state.config.validate_and_clamp();
            if state.backend_name != "headless" {
                let cfg_path = neon_config::default_config_path();
                if let Err(err) = neon_config::save_atomic(&cfg_path, &state.config) {
                    warn!(?err, "Failed to persist live appearance config update");
                }
            }
            state.needs_redraw = true;
            IpcResult::Ok {
                payload: IpcPayload::Appearance {
                    appearance: build_appearance(state),
                },
            }
        }
        IpcRequest::UpdateWindowBehavior {
            focus_follows_mouse,
            snap_gap,
            tiling_gap,
            button_layout,
            auto_tiling,
            tiling_layout,
            auto_snap,
            snap_layouts_bar,
        } => {
            if let Some(ffm) = focus_follows_mouse {
                state.config.windows.focus_mode = if ffm {
                    FocusMode::FocusFollowsMouse
                } else {
                    FocusMode::ClickToFocus
                };
                state.wm.set_focus_policy(if ffm {
                    crate::window::FocusPolicy::FocusFollowsMouse
                } else {
                    crate::window::FocusPolicy::ClickToFocus
                });
            }
            let mut gap_changed = false;
            if let Some(gap) = snap_gap {
                let clamped = gap.clamp(0, 48);
                state.config.windows.snap_gap = clamped;
                state.wm.set_snap_gap(clamped);
                gap_changed = true;
            }
            if let Some(gap) = tiling_gap {
                let clamped = gap.clamp(0, 24);
                state.config.windows.tiling_gap = clamped;
                state.wm.set_tiling_gap(clamped);
                gap_changed = true;
            }
            if gap_changed {
                state.relayout_non_floating_windows();
            }
            if let Some(layout) = button_layout {
                state.config.windows.button_layout = match layout.to_lowercase().as_str() {
                    "right" | "right_standard" => WindowButtonLayout::RightStandard,
                    _ => WindowButtonLayout::LeftTrafficLights,
                };
                state.needs_redraw = true;
            }
            if let Some(v) = auto_snap {
                state.config.windows.auto_snap = v;
            }
            if let Some(v) = snap_layouts_bar {
                state.config.windows.snap_layouts_bar = v;
            }
            if let Some(layout_str) = tiling_layout {
                let parsed = neon_config::TilingLayout::from_str_name(&layout_str);
                state.set_tiling_layout(parsed, false);
            }
            if let Some(enabled) = auto_tiling {
                state.set_auto_tiling(enabled, false);
            }
            state.config.validate_and_clamp();
            if state.backend_name != "headless" {
                let cfg_path = neon_config::default_config_path();
                if let Err(err) = neon_config::save_atomic(&cfg_path, &state.config) {
                    warn!(?err, "Failed to persist live window behavior update");
                }
            }
            state.needs_redraw = true;
            IpcResult::Ok {
                payload: IpcPayload::WindowBehavior {
                    behavior: build_window_behavior(state),
                },
            }
        }
        IpcRequest::Subscribe { topics } => {
            for topic in topics {
                subscriptions.insert(topic);
            }
            IpcResult::Ok {
                payload: IpcPayload::State {
                    snapshot: build_snapshot(state),
                },
            }
        }
    }
}

pub fn build_snapshot(state: &NeonState) -> DesktopSnapshot {
    DesktopSnapshot {
        active_workspace_index: state.wm.workspaces().active_index(),
        overview_open: state.overview_open,
        locked: state.is_locked(),
        windows: build_windows(state),
        workspaces: build_workspaces(state),
        outputs: build_outputs(state),
        effects: build_effects(state),
        appearance: build_appearance(state),
        window_behavior: build_window_behavior(state),
        xwayland_display: state.xwayland_display.map(|d| format!(":{d}")),
    }
}

fn build_windows(state: &NeonState) -> Vec<WindowInfo> {
    let mut ids = state.wm.all_window_ids();
    ids.sort_by_key(|id| id.0);
    ids.into_iter()
        .filter_map(|id| {
            let w = state.wm.get(id)?;
            Some(WindowInfo {
                id: w.id.0,
                workspace_id: w.workspace_id.0,
                app_id: w.app_id.clone(),
                title: w.title.clone(),
                x: w.current_geometry.loc.x,
                y: w.current_geometry.loc.y,
                width: w.current_geometry.size.w,
                height: w.current_geometry.size.h,
                focused: w.focused,
                minimized: w.mode.is_minimized(),
                maximized: w.mode.is_maximized(),
                fullscreen: w.mode.is_fullscreen(),
                always_on_top: w.always_on_top,
                tiled: w.mode.is_tiled(),
            })
        })
        .collect()
}

fn build_workspaces(state: &NeonState) -> Vec<WorkspaceInfo> {
    let active_idx = state.wm.workspaces().active_index();
    state
        .wm
        .workspaces()
        .workspaces()
        .iter()
        .enumerate()
        .map(|(index, ws)| WorkspaceInfo {
            id: ws.id.0,
            index,
            active: index == active_idx,
            pinned: ws.pinned,
            window_ids: ws.windows().iter().map(|w| w.0).collect(),
        })
        .collect()
}

fn build_outputs(state: &NeonState) -> Vec<OutputInfo> {
    state
        .output_manager
        .all_outputs()
        .into_iter()
        .map(|o| {
            let logical = o.logical_size();
            OutputInfo {
                name: o.name.clone(),
                x: o.position.x,
                y: o.position.y,
                width: logical.w,
                height: logical.h,
                scale: o.scale,
                refresh_mhz: o.current_mode.refresh_mhz,
                primary: o.primary,
                transform: format!("{:?}", o.transform).to_lowercase(),
            }
        })
        .collect()
}

fn build_effects(state: &NeonState) -> EffectsInfo {
    let cfg = state.effects.config;
    EffectsInfo {
        enabled: cfg.enabled,
        reduced_motion: cfg.reduced_motion,
        shadows_enabled: cfg.shadows_enabled,
        borders_enabled: cfg.borders_enabled,
        blur_enabled: cfg.blur_enabled,
        blur_strength: cfg.blur_strength,
        inactive_opacity: cfg.inactive_opacity,
        corner_radius: cfg.corner_radius,
        speed_scale: cfg.speed_scale,
        open_animation: cfg.open_animation,
        close_animation: cfg.close_animation,
        minimize_animation: cfg.minimize_animation,
        maximize_animation: cfg.maximize_animation,
        workspace_animation: cfg.workspace_animation,
        overview_animation: cfg.overview_animation,
        dock_animation: cfg.dock_animation,
    }
}

fn build_appearance(state: &NeonState) -> AppearanceInfo {
    let theme = match state.config.appearance.theme {
        ThemeMode::NeonDark => "neon_dark",
        ThemeMode::NeonLight => "neon_light",
        ThemeMode::Auto => "auto",
    };
    let dock_position = match state.config.dock.position {
        DockPosition::Bottom => "bottom",
        DockPosition::Left => "left",
        DockPosition::Right => "right",
    };
    AppearanceInfo {
        protocol_rev: neon_ipc::APPEARANCE_PROTOCOL_REV,
        theme: theme.into(),
        accent_color: state.config.appearance.accent_color.clone(),
        dock_transparency: state.config.dock.transparency,
        dock_auto_hide: state.config.dock.auto_hide,
        dock_smart_hide: state.config.dock.intelligent_hide,
        dock_position: dock_position.into(),
        dock_icon_size: state.config.dock.icon_size,
        dock_height: state.config.dock.height,
        dock_magnification: state.config.dock.magnification,
        dock_pinned_apps: state.config.dock.pinned_apps.clone(),
        panel_transparency: state.config.panel.transparency,
        panel_visible: state.config.panel.visible,
        panel_smart_hide: state.config.panel.smart_hide,
        panel_mode: match state.config.panel.mode {
            PanelMode::CombinedBottom => "combined_bottom".into(),
            PanelMode::TopAndDock => "top_and_dock".into(),
        },
        dynamic_island_enabled: state.config.panel.dynamic_island_enabled,
        show_clock: state.config.panel.show_clock,
        clock_24h: state.config.panel.clock_24h,
        show_workspaces: state.config.panel.show_workspaces,
        show_app_title: state.config.panel.show_app_title,
        show_system_tray: state.config.panel.show_system_tray,
        show_battery: state.config.panel.show_battery,
        show_quick_settings: state.config.panel.show_quick_settings,
        show_desktop_icons: state.config.desktop.show_desktop_icons,
        desktop_sort_by: state.config.desktop.desktop_sort_by.clone(),
        desktop_auto_arrange: state.config.desktop.desktop_auto_arrange,
        high_contrast: state.config.privacy_accessibility.high_contrast,
        wallpaper: state.config.appearance.wallpaper_path.clone(),
        reduce_blur: state.config.appearance.reduce_blur,
        blur_strength: state.effects.config.blur_strength,
        cursor_theme: Some(state.config.appearance.cursor_theme.clone()),
        cursor_size: Some(state.config.appearance.cursor_size),
    }
}

fn build_window_behavior(state: &NeonState) -> WindowBehaviorInfo {
    let button_layout = match state.config.windows.button_layout {
        WindowButtonLayout::RightStandard => "right_standard",
        WindowButtonLayout::LeftTrafficLights => "left_traffic_lights",
    };
    WindowBehaviorInfo {
        focus_follows_mouse: state.config.windows.focus_mode == FocusMode::FocusFollowsMouse,
        snap_gap: state.config.windows.snap_gap,
        tiling_gap: state.config.windows.tiling_gap,
        button_layout: button_layout.into(),
        auto_tiling: state.wm.is_auto_tiling(),
        tiling_layout: state.wm.tiling_layout().as_str().into(),
        auto_snap: state.config.windows.auto_snap,
        snap_layouts_bar: state.config.windows.snap_layouts_bar,
    }
}
