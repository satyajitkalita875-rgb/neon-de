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
    ClipboardToggleRequested,
    DynamicIslandToggleRequested,
    DragStateChanged(bool),
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
                            } else if let IpcResult::Ok {
                                payload: IpcPayload::WindowBehavior { ref behavior },
                            } = res
                            {
                                persist_legacy_window_behavior_update(&req, behavior);
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
                                    IpcEvent::ClipboardToggleRequested => {
                                        if update_tx
                                            .send(ShellIpcUpdate::ClipboardToggleRequested)
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    IpcEvent::DynamicIslandToggleRequested => {
                                        if update_tx
                                            .send(ShellIpcUpdate::DynamicIslandToggleRequested)
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    IpcEvent::DragStateChanged { active } => {
                                        if update_tx
                                            .send(ShellIpcUpdate::DragStateChanged(active))
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
        dock_height,
        dock_magnification,
        dock_pinned_apps,
        panel_mode,
        dynamic_island_enabled,
        panel_visible,
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
        || dock_height.is_some()
        || dock_magnification.is_some()
        || dock_pinned_apps.is_some()
        || panel_mode.is_some()
        || dynamic_island_enabled.is_some()
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
    if let Some(h) = dock_height {
        cfg.dock.height = (*h).clamp(36, 88);
    }
    if let Some(mag) = dock_magnification {
        cfg.dock.magnification = *mag;
    }
    if let Some(pins) = dock_pinned_apps {
        cfg.dock.pinned_apps = pins.clone();
    }
    if let Some(mode) = panel_mode {
        cfg.panel.mode = if mode.eq_ignore_ascii_case("combined_bottom") {
            neon_config::PanelMode::CombinedBottom
        } else {
            neon_config::PanelMode::TopAndDock
        };
    }
    if let Some(v) = panel_visible {
        // Legacy compositors expect panel_visible=false to drop Top Bar work-area when switching
        // into Combined Bottom mode; preserve the user's underlying Top Bar visibility preference.
        if cfg.panel.mode == neon_config::PanelMode::CombinedBottom && !*v {
            cfg.panel.visible = true;
        } else {
            cfg.panel.visible = *v;
        }
    }
    if let Some(v) = dynamic_island_enabled {
        cfg.panel.dynamic_island_enabled = *v;
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
    if appearance.protocol_rev < 2 {
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
    if appearance.protocol_rev < 4 {
        appearance.panel_mode = match cfg.panel.mode {
            neon_config::PanelMode::CombinedBottom => "combined_bottom".into(),
            neon_config::PanelMode::TopAndDock => "top_and_dock".into(),
        };
        appearance.dynamic_island_enabled = cfg.panel.dynamic_island_enabled;
    }
    if appearance.protocol_rev < 5 {
        appearance.dock_height = cfg.dock.height.clamp(36, 88);
    }
}

/// Overlays per-action Material 3 Expressive animation toggles from `NeonConfig` onto a snapshot
/// emitted by a pre-restart `neon-compositor` (`protocol_rev < 3`).
pub fn overlay_legacy_effects(
    effects: &mut neon_ipc::EffectsInfo,
    protocol_rev: u32,
    cfg: &neon_config::NeonConfig,
) {
    if protocol_rev >= 3 {
        return;
    }
    effects.open_animation = cfg.effects.open_animation;
    effects.close_animation = cfg.effects.close_animation;
    effects.minimize_animation = cfg.effects.minimize_animation;
    effects.maximize_animation = cfg.effects.maximize_animation;
    effects.workspace_animation = cfg.effects.workspace_animation;
    effects.overview_animation = cfg.effects.overview_animation;
    effects.dock_animation = cfg.effects.dock_animation;
}

/// Persists newer snapping/auto-tiling window behavior fields directly to XDG `config.toml`
/// when talking to an older pre-restart `neon-compositor` that ignores those fields.
fn persist_legacy_window_behavior_update(req: &IpcRequest, returned: &neon_ipc::WindowBehaviorInfo) {
    let IpcRequest::UpdateWindowBehavior {
        tiling_gap,
        auto_tiling,
        tiling_layout,
        auto_snap,
        snap_layouts_bar,
        ..
    } = req
    else {
        return;
    };
    let mismatch = tiling_gap.is_some_and(|v| v != returned.tiling_gap)
        || auto_tiling.is_some_and(|v| v != returned.auto_tiling)
        || tiling_layout
            .as_deref()
            .is_some_and(|s| !s.eq_ignore_ascii_case(&returned.tiling_layout))
        || auto_snap.is_some_and(|v| v != returned.auto_snap)
        || snap_layouts_bar.is_some_and(|v| v != returned.snap_layouts_bar);
    if !mismatch {
        return;
    }
    let cfg_path = neon_config::default_config_path();
    let mut cfg = neon_config::load_or_default(&cfg_path);
    if let Some(v) = tiling_gap {
        cfg.windows.tiling_gap = *v;
    }
    if let Some(v) = auto_tiling {
        cfg.windows.auto_tiling = *v;
    }
    if let Some(s) = tiling_layout {
        cfg.windows.tiling_layout = match s.to_ascii_lowercase().as_str() {
            "master_stack" | "master-stack" | "master" => neon_config::TilingLayout::MasterStack,
            _ => neon_config::TilingLayout::Dwindle,
        };
    }
    if let Some(v) = auto_snap {
        cfg.windows.auto_snap = *v;
    }
    if let Some(v) = snap_layouts_bar {
        cfg.windows.snap_layouts_bar = *v;
    }
    let _ = neon_config::save_atomic(&cfg_path, &cfg);
}

/// Overlays snapping & auto-tiling fields from `NeonConfig` onto a snapshot emitted by an older
/// pre-restart `neon-compositor` (`protocol_rev < 6`).
pub fn overlay_legacy_window_behavior(
    behavior: &mut neon_ipc::WindowBehaviorInfo,
    protocol_rev: u32,
    cfg: &neon_config::NeonConfig,
) {
    if protocol_rev >= 6 {
        return;
    }
    behavior.auto_tiling = cfg.windows.auto_tiling;
    behavior.tiling_layout = match cfg.windows.tiling_layout {
        neon_config::TilingLayout::Dwindle => "dwindle".into(),
        neon_config::TilingLayout::MasterStack => "master_stack".into(),
    };
    behavior.auto_snap = cfg.windows.auto_snap;
    behavior.snap_layouts_bar = cfg.windows.snap_layouts_bar;
}
