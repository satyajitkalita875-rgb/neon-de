//! Non-blocking IPC bridge connecting `neon-shell`'s GTK4 UI loop to `neon-compositor`.

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use neon_ipc::{
    ContextMenuTarget, DesktopSnapshot, EventTopic, IpcClient, IpcEvent, IpcPayload, IpcRequest,
    IpcResult, IpcServerMessage, default_socket_path,
};
use tracing::{debug, info, warn};

/// Incoming updates delivered from `neon-compositor` to the GTK4 main loop.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ShellIpcUpdate {
    Snapshot(DesktopSnapshot),
    OverviewToggled(bool),
    ScreenshotUiRequested,
    ContextMenuRequested {
        x: i32,
        y: i32,
        target: ContextMenuTarget,
    },
}

/// Handle used by GTK4 widgets to send fire-and-forget commands to `neon-compositor`.
#[derive(Clone)]
pub struct IpcCommandSender {
    tx: Sender<IpcRequest>,
}

impl IpcCommandSender {
    pub fn send(&self, request: IpcRequest) {
        let _ = self.tx.send(request);
    }
}

/// Resolves the NEON DE IPC socket path from CLI override, `NEON_IPC_SOCKET`, or `WAYLAND_DISPLAY`.
pub fn resolve_ipc_socket_path(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    if let Some(env_path) = std::env::var_os("NEON_IPC_SOCKET") {
        return PathBuf::from(env_path);
    }
    let wayland_display =
        std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-1".to_string());
    default_socket_path(&wayland_display)
}

/// Spawns dedicated background command + subscription threads so GTK4 never blocks on socket I/O.
pub fn start_ipc_bridge(socket_path: PathBuf) -> (IpcCommandSender, Receiver<ShellIpcUpdate>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<IpcRequest>();
    let (update_tx, update_rx) = mpsc::channel::<ShellIpcUpdate>();

    // Command sender thread
    {
        let cmd_socket = socket_path.clone();
        thread::spawn(move || {
            let mut client: Option<IpcClient> = None;
            while let Ok(req) = cmd_rx.recv() {
                if client.is_none() {
                    client = IpcClient::connect(&cmd_socket).ok();
                }
                if let Some(c) = client.as_mut() {
                    match c.request(req.clone()) {
                        Ok(res) => {
                            debug!(?req, ?res, "IPC command completed");
                            if let IpcResult::Ok {
                                payload: IpcPayload::Appearance { ref appearance },
                            } = res
                                && appearance.protocol_rev < neon_ipc::APPEARANCE_PROTOCOL_REV
                            {
                                persist_legacy_appearance_update(&req);
                            }
                        }
                        Err(err) => {
                            warn!(?err, "IPC command failed; reconnecting next time");
                            client = None;
                        }
                    }
                }
            }
        });
    }

    // Subscription / event listener thread with automatic reconnect
    {
        let sub_socket = socket_path;
        thread::spawn(move || {
            loop {
                match IpcClient::connect(&sub_socket) {
                    Ok(mut client) => {
                        info!(path = %sub_socket.display(), "Connected neon-shell IPC subscriber");
                        let sub = client.subscribe(vec![
                            EventTopic::Windows,
                            EventTopic::Workspaces,
                            EventTopic::Outputs,
                            EventTopic::Overview,
                            EventTopic::Effects,
                        ]);
                        if let Ok(IpcResult::Ok {
                            payload: IpcPayload::State { snapshot },
                        }) = sub
                        {
                            let _ = update_tx.send(ShellIpcUpdate::Snapshot(snapshot));
                        }
                        let _ = client.set_read_timeout(None);

                        loop {
                            match client.read_next_message() {
                                Ok(IpcServerMessage::Event { event, .. }) => match event {
                                    IpcEvent::StateChanged { snapshot } => {
                                        if update_tx
                                            .send(ShellIpcUpdate::Snapshot(snapshot))
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    IpcEvent::OverviewToggled { open } => {
                                        if update_tx
                                            .send(ShellIpcUpdate::OverviewToggled(open))
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    IpcEvent::ScreenshotUiRequested => {
                                        if update_tx
                                            .send(ShellIpcUpdate::ScreenshotUiRequested)
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    IpcEvent::ContextMenuRequested { x, y, target } => {
                                        if update_tx
                                            .send(ShellIpcUpdate::ContextMenuRequested {
                                                x,
                                                y,
                                                target,
                                            })
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                },
                                Ok(IpcServerMessage::Response { .. }) => {}
                                Err(err) => {
                                    debug!(?err, "IPC subscriber disconnected; retrying");
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(250));
                    }
                }
            }
        });
    }

    (IpcCommandSender { tx: cmd_tx }, update_rx)
}

/// Persists newer shell appearance fields directly to `config.toml` when talking to an older
/// running `neon-compositor` (`protocol_rev < APPEARANCE_PROTOCOL_REV`) so right-click menu
/// updates take effect immediately without requiring a full session logout.
fn persist_legacy_appearance_update(req: &IpcRequest) {
    let IpcRequest::UpdateAppearance {
        dock_position,
        dock_icon_size,
        dock_magnification,
        dock_pinned_apps,
        clock_24h,
        show_app_title,
        show_system_tray,
        show_battery,
        show_quick_settings,
        show_desktop_icons,
        desktop_sort_by,
        desktop_auto_arrange,
        ..
    } = req
    else {
        return;
    };

    let has_new_field = dock_position.is_some()
        || dock_icon_size.is_some()
        || dock_magnification.is_some()
        || dock_pinned_apps.is_some()
        || clock_24h.is_some()
        || show_app_title.is_some()
        || show_system_tray.is_some()
        || show_battery.is_some()
        || show_quick_settings.is_some()
        || show_desktop_icons.is_some()
        || desktop_sort_by.is_some()
        || desktop_auto_arrange.is_some();
    if !has_new_field {
        return;
    }

    let cfg_path = neon_config::default_config_path();
    let mut cfg = neon_config::load_or_default(&cfg_path);
    if let Some(pos) = dock_position {
        cfg.dock.position = match pos.to_lowercase().as_str() {
            "left" => neon_config::DockPosition::Left,
            "right" => neon_config::DockPosition::Right,
            _ => neon_config::DockPosition::Bottom,
        };
    }
    if let Some(sz) = dock_icon_size {
        cfg.dock.icon_size = (*sz).clamp(22, 64);
    }
    if let Some(mag) = dock_magnification {
        cfg.dock.magnification = *mag;
    }
    if let Some(pins) = dock_pinned_apps {
        cfg.dock.pinned_apps = pins.clone();
    }
    if let Some(v) = clock_24h {
        cfg.panel.clock_24h = *v;
    }
    if let Some(v) = show_app_title {
        cfg.panel.show_app_title = *v;
    }
    if let Some(v) = show_system_tray {
        cfg.panel.show_system_tray = *v;
    }
    if let Some(v) = show_battery {
        cfg.panel.show_battery = *v;
    }
    if let Some(v) = show_quick_settings {
        cfg.panel.show_quick_settings = *v;
    }
    if let Some(v) = show_desktop_icons {
        cfg.desktop.show_desktop_icons = *v;
    }
    if let Some(s) = desktop_sort_by {
        cfg.desktop.desktop_sort_by = s.clone();
    }
    if let Some(v) = desktop_auto_arrange {
        cfg.desktop.desktop_auto_arrange = *v;
    }
    let _ = neon_config::save_atomic(&cfg_path, &cfg);
}

/// Overlays newer shell appearance fields from `NeonConfig` onto a snapshot emitted by an older
/// pre-restart `neon-compositor` (`protocol_rev < APPEARANCE_PROTOCOL_REV`).
pub fn overlay_legacy_appearance(
    appearance: &mut neon_ipc::AppearanceInfo,
    cfg: &neon_config::NeonConfig,
) {
    if appearance.protocol_rev >= neon_ipc::APPEARANCE_PROTOCOL_REV {
        return;
    }
    appearance.dock_position = match cfg.dock.position {
        neon_config::DockPosition::Left => "left".into(),
        neon_config::DockPosition::Right => "right".into(),
        neon_config::DockPosition::Bottom => "bottom".into(),
    };
    appearance.dock_icon_size = cfg.dock.icon_size.clamp(22, 64);
    appearance.dock_magnification = cfg.dock.magnification;
    appearance.dock_pinned_apps = cfg.dock.pinned_apps.clone();
    appearance.clock_24h = cfg.panel.clock_24h;
    appearance.show_app_title = cfg.panel.show_app_title;
    appearance.show_system_tray = cfg.panel.show_system_tray;
    appearance.show_battery = cfg.panel.show_battery;
    appearance.show_quick_settings = cfg.panel.show_quick_settings;
    appearance.show_desktop_icons = cfg.desktop.show_desktop_icons;
    appearance.desktop_sort_by = cfg.desktop.desktop_sort_by.clone();
    appearance.desktop_auto_arrange = cfg.desktop.desktop_auto_arrange;
}
