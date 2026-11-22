//! Authenticated Unix domain socket IPC server embedded inside `neon-compositor`.

use std::{
    collections::HashSet,
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
};

use neon_config::{FocusMode, ThemeMode};
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
    pub fn broadcast_if_changed(&mut self, state: &NeonState) {
        if !self.clients.iter().any(|c| !c.subscriptions.is_empty()) {
            return;
        }
        let snapshot = build_snapshot(state);
        if self.last_broadcast_snapshot.as_ref() == Some(&snapshot) {
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
            if state.wm.workspaces().len() > index {
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
            // Move window directly to requested workspace index
            let cur_idx = state
                .wm
                .get(id)
                .and_then(|w| state.wm.workspaces().index_of(w.workspace_id));
            if let Some(mut idx) = cur_idx {
                while idx < workspace_index {
                    if state.wm.move_window_to_next_workspace(id, false).is_none() {
                        break;
                    }
                    idx += 1;
                }
                while idx > workspace_index {
                    if state.wm.move_window_to_prev_workspace(id, false).is_none() {
                        break;
                    }
                    idx -= 1;
                }
                state.sync_active_workspace_space(serial);
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
                let work_area = state.work_area();
                if let Some(update) = state.wm.restore_window(id, work_area) {
                    state.apply_layout_update(&update);
                }
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
            if let Some(window) = state.find_window_by_id(id)
                && let Some(toplevel) = window.toplevel()
            {
                toplevel.send_close();
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
            let work_area = state.work_area();
            if let Some(update) = state.wm.restore_window(id, work_area) {
                state.apply_layout_update(&update);
                if let Some(window) = state.find_window_by_id(id) {
                    state.focus_window(Some(&window), serial);
                }
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
        IpcRequest::TakeScreenshot { path } => {
            let dest = state.request_screenshot(path.map(std::path::PathBuf::from));
            IpcResult::Ok {
                payload: IpcPayload::Screenshot {
                    path: dest.to_string_lossy().into_owned(),
                },
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
        } => {
            if let Some(v) = enabled {
                state.effects.config.enabled = v;
            }
            if let Some(v) = reduced_motion {
                state.effects.config.reduced_motion = v;
            }
            if let Some(v) = shadows_enabled {
                state.effects.config.shadows_enabled = v;
            }
            if let Some(v) = borders_enabled {
                state.effects.config.borders_enabled = v;
            }
            if let Some(v) = blur_enabled {
                state.effects.config.blur_enabled = v;
            }
            if let Some(v) = blur_strength {
                state.effects.config.blur_strength = v.clamp(0.0, 2.0);
            }
            if let Some(v) = inactive_opacity {
                state.effects.config.inactive_opacity = v.clamp(0.4, 1.0);
            }
            if let Some(v) = corner_radius {
                state.effects.config.corner_radius = v.clamp(0.0, 32.0);
            }
            if let Some(v) = speed_scale {
                state.effects.config.speed_scale = v.clamp(0.1, 5.0);
            }
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
            panel_transparency,
            panel_visible,
            show_clock,
            show_workspaces,
            high_contrast,
            wallpaper,
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
            if let Some(v) = panel_transparency {
                state.config.panel.transparency = v;
            }
            if let Some(v) = panel_visible {
                state.config.panel.visible = v;
            }
            if let Some(v) = show_clock {
                state.config.panel.show_clock = v;
            }
            if let Some(v) = show_workspaces {
                state.config.panel.show_workspaces = v;
            }
            if let Some(v) = high_contrast {
                state.config.privacy_accessibility.high_contrast = v;
            }
            if let Some(w) = wallpaper {
                state.config.appearance.wallpaper_path = if w.is_empty() { None } else { Some(w) };
            }
            state.config.validate_and_clamp();
            IpcResult::Ok {
                payload: IpcPayload::Appearance {
                    appearance: build_appearance(state),
                },
            }
        }
        IpcRequest::UpdateWindowBehavior {
            focus_follows_mouse,
            snap_gap,
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
            if let Some(gap) = snap_gap {
                let clamped = gap.clamp(0, 48);
                state.config.windows.snap_gap = clamped;
                state.wm.set_snap_gap(clamped);
                state.relayout_non_floating_windows();
            }
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
    }
}

fn build_appearance(state: &NeonState) -> AppearanceInfo {
    let theme = match state.config.appearance.theme {
        ThemeMode::NeonDark => "neon_dark",
        ThemeMode::NeonLight => "neon_light",
        ThemeMode::Auto => "auto",
    };
    AppearanceInfo {
        theme: theme.into(),
        accent_color: state.config.appearance.accent_color.clone(),
        dock_transparency: state.config.dock.transparency,
        panel_transparency: state.config.panel.transparency,
        panel_visible: state.config.panel.visible,
        show_clock: state.config.panel.show_clock,
        show_workspaces: state.config.panel.show_workspaces,
        high_contrast: state.config.privacy_accessibility.high_contrast,
        wallpaper: state.config.appearance.wallpaper_path.clone(),
    }
}

fn build_window_behavior(state: &NeonState) -> WindowBehaviorInfo {
    WindowBehaviorInfo {
        focus_follows_mouse: state.config.windows.focus_mode == FocusMode::FocusFollowsMouse,
        snap_gap: state.config.windows.snap_gap,
    }
}
