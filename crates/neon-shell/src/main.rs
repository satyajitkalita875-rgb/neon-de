//! Entrypoint for `neon-shell` — NEON DE's custom Rust + GTK4 Wayland desktop shell.

mod context_menu;
mod desktop_apps;
mod dock;
mod dock_preview;
mod dynamic_island;
mod file_ops;
mod ipc_bridge;
mod island_data;
mod island_visuals;
mod notification_center;
mod notifications;
mod overview;
mod panel;
mod polkit_dialog;
mod power_menu;
mod quick_settings;
mod screenshot_ui;
mod style;
mod wallpaper;
mod workspaces_panel;

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use clap::Parser;
use gtk4::{Application, gio, glib, prelude::*};
use neon_system::{
    clipboard::{ClipboardStore, start_clipboard_monitor},
    notifications::{
        NotificationAction, NotificationEvent, NotificationStore, NotificationUrgency,
        start_notification_daemon,
    },
    polkit::{PolkitAgentState, start_polkit_agent},
    start_system_monitor,
    tray::{TrayEvent, start_tray_service},
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::{
    context_menu::ContextMenuOverlay,
    desktop_apps::{
        discover_applications, select_default_favorites, set_live_cursor_config,
        set_live_xwayland_display,
    },
    dock::Dock,
    dock_preview::DockPreviewPopup,
    dynamic_island::DynamicIsland,
    ipc_bridge::{ShellIpcUpdate, resolve_ipc_socket_path, start_ipc_bridge},
    notification_center::NotificationCenterPanel,
    notifications::NotificationOverlay,
    overview::Overview,
    panel::TopPanel,
    polkit_dialog::PolkitDialog,
    power_menu::PowerMenuOverlay,
    quick_settings::QuickSettingsPanel,
    screenshot_ui::ScreenshotOverlay,
    wallpaper::WallpaperLayer,
    workspaces_panel::WorkspacesPanel,
};

#[derive(Debug, Parser)]
#[command(name = "neon-shell", version, about)]
struct Cli {
    /// Explicit path to `neon-compositor`'s IPC Unix socket.
    #[arg(long)]
    ipc_socket: Option<PathBuf>,

    /// Open Quick Settings immediately on startup (for automated UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    open_quick_settings: bool,

    /// Open a Quick Settings dropdown ("wifi" or "bluetooth") on startup (for UI verification).
    #[arg(long, hide = true)]
    open_qs_dropdown: Option<String>,

    /// Open fullscreen Power Menu immediately on startup (for automated UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    open_power_menu: bool,

    /// Open Screenshot & Screen Record overlay on startup (for automated UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    open_screenshot_ui: bool,

    /// Switch Screenshot overlay into Screen Record mode on startup (for UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    screenshot_record_mode: bool,

    /// Open left-edge Workspaces Sidebar immediately on startup (for automated UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    open_workspaces: bool,

    /// Open a specific context menu on startup for automated UI verification
    /// (`desktop`, `desktop-sub`, `file`, `dock`, `dock-app`, `window`, `panel`, `panel-sub`).
    #[arg(long, hide = true)]
    open_context_menu: Option<String>,

    /// Open a preview PolicyKit authentication dialog on startup (for automated UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    open_polkit_test: bool,

    /// Open Notification Center on startup with sample notifications (for UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    open_notifications: bool,

    /// Force Combined Bottom Taskbar + Dynamic Island mode (for automated UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    force_combined_mode: bool,

    /// Preview a Dynamic Island state (`compact`, `media`, `notif`, `recording`, `idle`).
    #[arg(long, hide = true)]
    open_island_test: Option<String>,

    /// Open the frosted-glass Dock Hover Live Window Preview on startup (for UI verification).
    #[arg(long, hide = true, default_value_t = false)]
    open_dock_preview_test: bool,

    /// Disable registering the PolicyKit1 AuthenticationAgent with polkitd (for nested tests).
    #[arg(long, hide = true, default_value_t = false)]
    no_polkit: bool,
}

fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("neon_shell=info,neon_ipc=info,neon_system=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    let cli = Cli::parse();
    let open_qs_on_start = cli.open_quick_settings;
    let open_qs_dropdown = cli.open_qs_dropdown.clone();
    let open_pm_on_start = cli.open_power_menu;
    let open_shot_on_start = cli.open_screenshot_ui;
    let shot_record_mode = cli.screenshot_record_mode;
    let open_ws_on_start = cli.open_workspaces;
    let open_ctx_on_start = cli.open_context_menu.clone();
    let open_polkit_on_start = cli.open_polkit_test;
    let open_notif_on_start = cli.open_notifications;
    let force_combined_mode = cli.force_combined_mode;
    let open_island_on_start = cli.open_island_test.clone();
    let open_dock_preview_on_start = cli.open_dock_preview_test;
    let no_polkit = cli.no_polkit;
    let socket_path = resolve_ipc_socket_path(cli.ipc_socket);
    info!(ipc_socket = %socket_path.display(), "Starting NEON DE Desktop Shell");

    let notif_store = Arc::new(NotificationStore::new());
    let notif_rx = notif_store.subscribe();

    let notif_daemon = match start_notification_daemon(Arc::clone(&notif_store), None) {
        Ok(handle) => Some(handle),
        Err(err) => {
            warn!(%err, "Notification D-Bus daemon could not bind; continuing with in-process store");
            None
        }
    };

    let polkit_state = Arc::new(PolkitAgentState::new());
    let polkit_rx = polkit_state.subscribe();
    let polkit_agent = if no_polkit || open_polkit_on_start {
        None
    } else {
        match start_polkit_agent(Arc::clone(&polkit_state)) {
            Ok(handle) => Some(handle),
            Err(err) => {
                warn!(%err, "PolicyKit1 AuthenticationAgent did not register; privileged prompts may be unavailable");
                None
            }
        }
    };

    let (sys_tx, sys_rx) = start_system_monitor(Duration::from_secs(2));
    let (tray_tx, tray_rx) = start_tray_service();
    let clipboard_store = Arc::new(ClipboardStore::new());
    let clipboard_rx = start_clipboard_monitor(Arc::clone(&clipboard_store));

    let app = Application::builder()
        .application_id("org.neonde.shell")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let notif_rx = std::cell::RefCell::new(Some(notif_rx));
    let polkit_rx = std::cell::RefCell::new(Some(polkit_rx));
    let polkit_state_ui = Arc::clone(&polkit_state);
    let sys_rx = std::cell::RefCell::new(Some(sys_rx));
    let tray_rx = std::cell::RefCell::new(Some(tray_rx));
    let clipboard_rx = std::cell::RefCell::new(Some(clipboard_rx));
    let clipboard_store_ui = Arc::clone(&clipboard_store);

    app.connect_activate(move |app| {
        let mut theme_ctrl = style::ShellThemeController::init();

        let initial_apps = discover_applications();
        info!(
            app_count = initial_apps.len(),
            "Discovered installed XDG desktop applications"
        );
        let favorites = select_default_favorites(&initial_apps);
        let shared_apps = Rc::new(RefCell::new(initial_apps));

        let (ipc_sender, update_rx) = start_ipc_bridge(socket_path.clone());

        // System-wide macOS right-click context menu overlay (Desktop, Files, Dock, Panel, Windows).
        let context_menu = ContextMenuOverlay::build(
            app,
            Rc::clone(&shared_apps),
            ipc_sender.clone(),
            sys_tx.clone(),
        );

        let latest_windows: Rc<RefCell<Vec<neon_ipc::WindowInfo>>> = Rc::new(RefCell::new(Vec::new()));

        // Background wallpaper layer renders behind all desktop windows & panels.
        let wallpaper_layer =
            WallpaperLayer::build(app, context_menu.clone(), ipc_sender.clone());
        let screenshot_overlay =
            ScreenshotOverlay::build(app, ipc_sender.clone(), Arc::clone(&notif_store));
        let toast_overlay = NotificationOverlay::build(
            app,
            Arc::clone(&notif_store),
            ipc_sender.clone(),
            Rc::clone(&latest_windows),
        );
        let notification_center = NotificationCenterPanel::build(
            app,
            ipc_sender.clone(),
            Arc::clone(&notif_store),
            toast_overlay.clone(),
            Rc::clone(&latest_windows),
        );
        let dynamic_island = DynamicIsland::build(
            app,
            ipc_sender.clone(),
            sys_tx.clone(),
            Arc::clone(&notif_store),
            Arc::clone(&clipboard_store_ui),
            Rc::clone(&latest_windows),
        );
        let polkit_dialog = PolkitDialog::build(app, Arc::clone(&polkit_state_ui));
        let power_menu = PowerMenuOverlay::build(app, ipc_sender.clone(), sys_tx.clone());
        let quick_settings = QuickSettingsPanel::build(
            app,
            ipc_sender.clone(),
            sys_tx.clone(),
            toast_overlay.clone(),
            power_menu.clone(),
        );
        if open_qs_on_start || open_qs_dropdown.is_some() {
            let qs_open = quick_settings.clone();
            let dd_choice = open_qs_dropdown.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                qs_open.open();
                if let Some(which) = dd_choice {
                    let qs_dd = qs_open.clone();
                    glib::timeout_add_local_once(Duration::from_millis(260), move || {
                        qs_dd.open_dropdown_for_test(&which);
                    });
                }
            });
        }
        if open_notif_on_start {
            let nc_open = notification_center.clone();
            let st_seed = Arc::clone(&notif_store);
            glib::timeout_add_local_once(Duration::from_millis(650), move || {
                st_seed.push_with_meta(
                    0,
                    "Google Chrome".into(),
                    "google-chrome".into(),
                    "ChatGPT response ready".into(),
                    "Finished generating implementation notes for NEON DE.".into(),
                    NotificationUrgency::Normal,
                    60000,
                    vec![
                        NotificationAction {
                            key: "default".into(),
                            label: "Open".into(),
                        },
                        NotificationAction {
                            key: "open_tab".into(),
                            label: "Open Tab".into(),
                        },
                    ],
                    Some("google-chrome".into()),
                    false,
                );
                st_seed.push_with_meta(
                    0,
                    "App Center".into(),
                    "system-software-update".into(),
                    "3 app updates available".into(),
                    "Firefox, Firmware Updater, and core24 are ready to install.".into(),
                    NotificationUrgency::Normal,
                    60000,
                    vec![NotificationAction {
                        key: "update".into(),
                        label: "Update All".into(),
                    }],
                    Some("snap-store".into()),
                    false,
                );
                nc_open.open();
            });
        }
        if let Some(ref isl_mode) = open_island_on_start {
            let mode_s = isl_mode.clone();
            let isl = dynamic_island.clone();
            glib::timeout_add_local_once(Duration::from_millis(650), move || {
                isl.open_for_test(&mode_s);
            });
        }
        if open_pm_on_start {
            let pm_open = power_menu.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                pm_open.open();
            });
        }
        if open_polkit_on_start {
            let pk_open = polkit_dialog.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                pk_open.open_for_test();
            });
        }
        if open_shot_on_start {
            let shot_open = screenshot_overlay.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                shot_open.open();
                if shot_record_mode {
                    shot_open.set_record_mode_for_test();
                }
            });
        }

        let workspaces_panel = WorkspacesPanel::build(
            app,
            Rc::clone(&shared_apps),
            ipc_sender.clone(),
            context_menu.clone(),
        );
        if open_ws_on_start {
            let ws_open = workspaces_panel.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || {
                ws_open.open();
            });
        }
        let top_panel = Rc::new(TopPanel::build(
            app,
            ipc_sender.clone(),
            tray_tx.clone(),
            quick_settings.clone(),
            notification_center.clone(),
            workspaces_panel.clone(),
            context_menu.clone(),
        ));
        {
            let shot_stop = screenshot_overlay.clone();
            top_panel.set_on_stop_recording(move || {
                shot_stop.stop_recording();
            });
            let shot_stop2 = screenshot_overlay.clone();
            dynamic_island.set_on_stop_recording(move || {
                shot_stop2.stop_recording();
            });
            let nc_open = notification_center.clone();
            dynamic_island.set_on_open_notifications(move || {
                nc_open.open();
            });
            let shot_open_isl = screenshot_overlay.clone();
            dynamic_island.set_on_open_screenshot(move |record_mode| {
                shot_open_isl.open();
                if record_mode {
                    shot_open_isl.set_record_mode_for_test();
                }
            });
        }

        let dock_preview = DockPreviewPopup::build(
            app,
            Rc::clone(&shared_apps),
            ipc_sender.clone(),
        );
        if open_dock_preview_on_start {
            let dp_test = dock_preview.clone();
            glib::timeout_add_local(Duration::from_millis(400), move || {
                if dp_test.open_first_window_for_test() {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        }
        let dock = Rc::new(Dock::build(
            app,
            &favorites,
            Rc::clone(&shared_apps),
            ipc_sender.clone(),
            tray_tx.clone(),
            quick_settings.clone(),
            notification_center.clone(),
            workspaces_panel.clone(),
            context_menu.clone(),
            dock_preview.clone(),
        ));
        {
            let shot_stop_dock = screenshot_overlay.clone();
            dock.set_on_stop_recording(move || {
                shot_stop_dock.stop_recording();
            });
            let dk_ws = Rc::clone(&dock);
            workspaces_panel.set_on_dock_elevate(move |open| {
                dk_ws.set_workspaces_open(open);
            });
            let isl_ws = dynamic_island.clone();
            workspaces_panel.add_on_state_change(move |open| {
                isl_ws.set_workspaces_open(open);
            });
            let isl_nc = dynamic_island.clone();
            notification_center.add_on_state_change(move |open| {
                isl_nc.set_suppressed(open);
            });
        }
        let overview = Overview::build(app, Rc::clone(&shared_apps), ipc_sender, context_menu.clone());
        {
            let wl_ref = wallpaper_layer.clone();
            let ov_ref = Rc::clone(&overview);
            context_menu.set_on_desktop_refresh(move || {
                wl_ref.force_show_and_refresh();
                let _ = ov_ref.refresh_apps();
            });
            let ws_open = workspaces_panel.clone();
            context_menu.set_on_open_workspaces(move || {
                ws_open.open();
            });
            let qs_open = quick_settings.clone();
            context_menu.set_on_open_quick_settings(move || {
                qs_open.open();
            });
        }
        if let Some(ref ctx_mode) = open_ctx_on_start {
            let mode = ctx_mode.clone();
            let cm_test = context_menu.clone();
            let apps_test = Rc::clone(&shared_apps);
            glib::timeout_add_local_once(Duration::from_millis(850), move || match mode.as_str() {
                "desktop" => cm_test.open_desktop_menu(340, 180),
                "desktop-sub" => {
                    cm_test.open_desktop_menu(300, 160);
                    let cm2 = cm_test.clone();
                    glib::timeout_add_local_once(Duration::from_millis(220), move || {
                        cm2.open_submenu_for_row(2, true);
                    });
                }
                "file" => {
                    let desk = crate::file_ops::desktop_dir();
                    if let Ok(mut rd) = std::fs::read_dir(&desk)
                        && let Some(Ok(ent)) = rd.find(|e| {
                            e.as_ref()
                                .ok()
                                .is_some_and(|x| !x.file_name().to_string_lossy().starts_with('.'))
                        })
                    {
                        cm_test.open_file_menu(240, 180, ent.path());
                    }
                }
                "dock" => cm_test.open_dock_menu(640, 710),
                "dock-app" => {
                    let apps = apps_test.borrow();
                    let chrome = apps
                        .iter()
                        .find(|a| a.id.eq_ignore_ascii_case("google-chrome"))
                        .cloned()
                        .or_else(|| {
                            crate::desktop_apps::select_default_favorites(&apps)
                                .into_iter()
                                .next()
                        });
                    cm_test.open_dock_app_menu(520, 710, chrome, None);
                }
                "window" => cm_test.open_window_menu(520, 220, 1),
                "panel" => cm_test.open_panel_menu(640, 34, crate::context_menu::PanelMenuZone::General),
                "panel-sub" => {
                    cm_test.open_panel_menu(420, 34, crate::context_menu::PanelMenuZone::General);
                    let cm2 = cm_test.clone();
                    glib::timeout_add_local_once(Duration::from_millis(220), move || {
                        cm2.open_submenu_for_row(0, true);
                    });
                }
                _ => {}
            });
        }
        {
            let dk_refresh = Rc::clone(&dock);
            overview.set_on_apps_refreshed(move || {
                dk_refresh.invalidate_window_icons();
            });
        }
        {
            let ov_poll = Rc::clone(&overview);
            glib::timeout_add_local(Duration::from_secs(3), move || {
                let _ = ov_poll.refresh_apps();
                glib::ControlFlow::Continue
            });
        }
        {
            let wl_poll = wallpaper_layer.clone();
            glib::timeout_add_local(Duration::from_millis(1500), move || {
                wl_poll.poll_dir_if_visible();
                glib::ControlFlow::Continue
            });
        }
        {
            let tp_layer = Rc::clone(&top_panel);
            let dk_layer = Rc::clone(&dock);
            let tp_bg = Rc::clone(&top_panel);
            let dk_bg = Rc::clone(&dock);
            overview.set_chrome_hooks(
                move |elevated| {
                    tp_layer.set_drawer_layer_elevated(elevated);
                    dk_layer.set_overview_elevated(elevated);
                },
                move |hidden| {
                    tp_bg.set_drawer_bg_hidden(hidden);
                    dk_bg.set_drawer_bg_hidden(hidden);
                },
            );
        }
        // Mutual exclusion handoffs: opening any shell overlay immediately dismisses any other
        // active overlay with zero delayed-timer layer races.
        {
            let cm = context_menu.clone();
            let dp = dock_preview.clone();
            let qs = quick_settings.clone();
            let nc = notification_center.clone();
            let pm = power_menu.clone();
            let shot = screenshot_overlay.clone();
            let ov = Rc::clone(&overview);
            workspaces_panel.set_on_before_open(move || {
                cm.close_immediate();
                dp.hide_immediate();
                qs.close_immediate();
                nc.close_immediate();
                pm.close_immediate();
                shot.close_immediate();
                ov.set_open_immediate(false);
            });
        }
        {
            let cm = context_menu.clone();
            let dp = dock_preview.clone();
            let nc = notification_center.clone();
            let pm = power_menu.clone();
            let ws = workspaces_panel.clone();
            let shot = screenshot_overlay.clone();
            let ov = Rc::clone(&overview);
            let isl = dynamic_island.clone();
            quick_settings.set_on_before_open(move || {
                cm.close_immediate();
                dp.hide_immediate();
                nc.close_immediate();
                pm.close_immediate();
                ws.close_immediate();
                shot.close_immediate();
                ov.set_open_immediate(false);
                isl.close_clipboard();
            });
        }
        {
            let cm = context_menu.clone();
            let dp = dock_preview.clone();
            let qs = quick_settings.clone();
            let pm = power_menu.clone();
            let ws = workspaces_panel.clone();
            let shot = screenshot_overlay.clone();
            let ov = Rc::clone(&overview);
            let isl = dynamic_island.clone();
            notification_center.set_on_before_open(move || {
                cm.close_immediate();
                dp.hide_immediate();
                qs.close_immediate();
                pm.close_immediate();
                ws.close_immediate();
                shot.close_immediate();
                ov.set_open_immediate(false);
                isl.clear_notifications();
                isl.close_clipboard();
            });
        }
        {
            let cm = context_menu.clone();
            let dp = dock_preview.clone();
            let qs = quick_settings.clone();
            let nc = notification_center.clone();
            let ws = workspaces_panel.clone();
            let shot = screenshot_overlay.clone();
            let ov = Rc::clone(&overview);
            let isl = dynamic_island.clone();
            power_menu.set_on_before_open(move || {
                cm.close_immediate();
                dp.hide_immediate();
                qs.close_immediate();
                nc.close_immediate();
                ws.close_immediate();
                shot.close_immediate();
                ov.set_open_immediate(false);
                isl.close_clipboard();
            });
        }
        {
            let cm = context_menu.clone();
            let dp = dock_preview.clone();
            let qs = quick_settings.clone();
            let nc = notification_center.clone();
            let pm = power_menu.clone();
            let ws = workspaces_panel.clone();
            let shot = screenshot_overlay.clone();
            let ov = Rc::clone(&overview);
            let isl = dynamic_island.clone();
            polkit_dialog.set_on_before_open(move || {
                cm.close_immediate();
                dp.hide_immediate();
                qs.close_immediate();
                nc.close_immediate();
                pm.close_immediate();
                ws.close_immediate();
                shot.close_immediate();
                ov.set_open_immediate(false);
                isl.close_clipboard();
            });
        }

        let sys_rx_local = sys_rx.borrow_mut().take();
        let notif_rx_local = notif_rx.borrow_mut().take();
        let polkit_rx_local = polkit_rx.borrow_mut().take();
        let tray_rx_local = tray_rx.borrow_mut().take();
        let clipboard_rx_local = clipboard_rx.borrow_mut().take();
        let store_for_loop = Arc::clone(&notif_store);
        let mut last_sys = neon_system::SystemSnapshot::default();
        let mut last_rec_state: Option<u64> = None;
        let cfg_path = neon_config::default_config_path();
        let mut legacy_cfg_mtime: Option<std::time::SystemTime> = None;
        let mut legacy_cfg: Option<neon_config::NeonConfig> = None;
        let mut last_raw_snapshot: Option<neon_ipc::DesktopSnapshot> = None;
        let mut legacy_poll_ticks: u32 = 0;

        let open_island_timer = open_island_on_start.clone();
        glib::timeout_add_local(Duration::from_millis(33), move || {
            let mut apply_snapshot =
                |mut snapshot: neon_ipc::DesktopSnapshot, only_if_cfg_changed: bool| {
                    if snapshot.appearance.protocol_rev < neon_ipc::APPEARANCE_PROTOCOL_REV {
                        let cur_mtime = std::fs::metadata(&cfg_path)
                            .ok()
                            .and_then(|m| m.modified().ok());
                        let changed = legacy_cfg.is_none() || cur_mtime != legacy_cfg_mtime;
                        if only_if_cfg_changed && !changed {
                            return;
                        }
                        if changed {
                            legacy_cfg_mtime = cur_mtime;
                            legacy_cfg = Some(neon_config::load_or_default(&cfg_path));
                        }
                        if let Some(ref cfg) = legacy_cfg {
                            let rev = snapshot.appearance.protocol_rev;
                            ipc_bridge::overlay_legacy_appearance(&mut snapshot.appearance, cfg);
                            ipc_bridge::overlay_legacy_effects(&mut snapshot.effects, rev, cfg);
                            ipc_bridge::overlay_legacy_window_behavior(
                                &mut snapshot.window_behavior,
                                rev,
                                cfg,
                            );
                            if rev < 2 {
                                snapshot.window_behavior.button_layout =
                                    match cfg.windows.button_layout {
                                        neon_config::WindowButtonLayout::RightStandard => {
                                            "right_standard".into()
                                        }
                                        neon_config::WindowButtonLayout::LeftTrafficLights => {
                                            "left_traffic_lights".into()
                                        }
                                    };
                            }
                        }
                    } else if only_if_cfg_changed {
                        return;
                    }
                    if force_combined_mode {
                        snapshot.appearance.panel_mode = "combined_bottom".into();
                        snapshot.appearance.panel_visible = false;
                    }
                    *latest_windows.borrow_mut() = snapshot.windows.clone();
                    let combined_mode = snapshot
                        .appearance
                        .panel_mode
                        .eq_ignore_ascii_case("combined_bottom");
                    let island_active =
                        combined_mode && snapshot.appearance.dynamic_island_enabled;

                    set_live_xwayland_display(snapshot.xwayland_display.as_deref());
                    set_live_cursor_config(
                        snapshot.appearance.cursor_theme.as_deref(),
                        snapshot.appearance.cursor_size,
                    );
                    theme_ctrl.apply_if_changed(&snapshot.appearance, &snapshot.effects);
                    theme_ctrl
                        .sync_button_layout_if_changed(&snapshot.window_behavior.button_layout);
                    wallpaper_layer.update(&snapshot.appearance);
                    quick_settings.update_appearance(&snapshot.appearance);
                    quick_settings.update_effects(&snapshot.effects);
                    quick_settings.update_window_behavior(&snapshot.window_behavior);
                    notification_center.update_effects(&snapshot.effects);
                    power_menu.update_effects(&snapshot.effects);
                    polkit_dialog.update_effects(&snapshot.effects);
                    context_menu.update_snapshot(&snapshot);
                    screenshot_overlay.update_snapshot(&snapshot);
                    top_panel.update(&snapshot);
                    workspaces_panel.update(&snapshot);
                    dock.update(&snapshot);
                    overview.update(&snapshot);

                    let bar_margin = dock.combined_bar_height() + 10;
                    quick_settings.set_anchor_bottom(combined_mode, bar_margin);
                    notification_center.set_anchor_bottom(combined_mode, bar_margin);
                    toast_overlay.set_anchor_bottom(combined_mode, bar_margin);
                    toast_overlay.set_island_routed(island_active);
                    dynamic_island.sync_compositor_backdrop_caps(&snapshot);
                    if open_island_timer.is_none() {
                        dynamic_island.update_desktop(&snapshot, combined_mode);
                    }
                };

            while let Ok(update) = update_rx.try_recv() {
                match update {
                    ShellIpcUpdate::Snapshot(snapshot) => {
                        if snapshot.appearance.protocol_rev < neon_ipc::APPEARANCE_PROTOCOL_REV {
                            last_raw_snapshot = Some(snapshot.clone());
                        } else {
                            last_raw_snapshot = None;
                        }
                        apply_snapshot(snapshot, false);
                    }
                    ShellIpcUpdate::OverviewToggled(open) => {
                        context_menu.close_immediate();
                        dock_preview.hide_immediate();
                        if open {
                            quick_settings.close_immediate();
                            notification_center.close_immediate();
                            power_menu.close_immediate();
                            screenshot_overlay.close_immediate();
                            workspaces_panel.close_immediate();
                        }
                        overview.set_open(open);
                        dynamic_island.set_overview_open(open);
                    }
                    ShellIpcUpdate::ScreenshotUiRequested => {
                        context_menu.close_immediate();
                        dock_preview.hide_immediate();
                        quick_settings.close_immediate();
                        notification_center.close_immediate();
                        power_menu.close_immediate();
                        workspaces_panel.close_immediate();
                        overview.set_open_immediate(false);
                        dynamic_island.close_clipboard();
                        screenshot_overlay.open();
                    }
                    ShellIpcUpdate::ClipboardToggleRequested => {
                        context_menu.close_immediate();
                        dock_preview.hide_immediate();
                        quick_settings.close_immediate();
                        notification_center.close_immediate();
                        power_menu.close_immediate();
                        workspaces_panel.close_immediate();
                        screenshot_overlay.close_immediate();
                        overview.set_open_immediate(false);
                        dynamic_island.toggle_clipboard();
                    }
                    ShellIpcUpdate::DynamicIslandToggleRequested => {
                        context_menu.close_immediate();
                        dock_preview.hide_immediate();
                        quick_settings.close_immediate();
                        notification_center.close_immediate();
                        power_menu.close_immediate();
                        workspaces_panel.close_immediate();
                        screenshot_overlay.close_immediate();
                        overview.set_open_immediate(false);
                        dynamic_island.toggle_island();
                    }
                    ShellIpcUpdate::DragStateChanged(active) => {
                        dynamic_island.set_desktop_drag_active(active);
                    }
                    ShellIpcUpdate::ContextMenuRequested { x, y, target } => {
                        context_menu.handle_compositor_request(x, y, target);
                    }
                }
            }

            // When talking to a pre-logout legacy compositor, re-check config.toml mtime every
            // ~400ms so Settings / Context Menu edits (Dock size, Panel mode, Desktop icons) apply live!
            legacy_poll_ticks = legacy_poll_ticks.wrapping_add(1);
            if legacy_poll_ticks.is_multiple_of(12)
                && let Some(ref raw_snap) = last_raw_snapshot
            {
                apply_snapshot(raw_snap.clone(), true);
            }

            let cur_rec = screenshot_overlay.poll_recording_elapsed();
            if cur_rec != last_rec_state {
                last_rec_state = cur_rec;
                match cur_rec {
                    Some(secs) => {
                        top_panel.set_recording_status(true, secs);
                        dock.set_recording_status(true, secs);
                        if open_island_timer.is_none() {
                            dynamic_island.set_recording_status(true, secs);
                        }
                    }
                    None => {
                        top_panel.set_recording_status(false, 0);
                        dock.set_recording_status(false, 0);
                        if open_island_timer.is_none() {
                            dynamic_island.set_recording_status(false, 0);
                        }
                    }
                }
            }

            if let Some(rx) = sys_rx_local.as_ref() {
                while let Ok(sys_snap) = rx.try_recv() {
                    last_sys = sys_snap;
                    let unread = store_for_loop.active_count();
                    let dnd = store_for_loop.is_do_not_disturb();
                    top_panel.update_system(&last_sys, unread);
                    dock.update_system(&last_sys, unread);
                    quick_settings.update_system(&last_sys);
                    context_menu.update_system_snapshot(&last_sys);
                    if open_island_timer.is_none() {
                        dynamic_island.update_system(&last_sys, unread, dnd);
                    }
                }
            }

            if let Some(rx) = notif_rx_local.as_ref() {
                let mut notif_changed = false;
                while let Ok(evt) = rx.try_recv() {
                    notif_changed = true;
                    match evt {
                        NotificationEvent::Added(item) => {
                            if open_island_timer.is_none() {
                                dynamic_island.push_notification(item.clone());
                            }
                            toast_overlay.push_toast(item);
                        }
                        NotificationEvent::Closed { id, .. } => {
                            toast_overlay.remove_toast(id);
                            if open_island_timer.is_none() {
                                dynamic_island.dismiss_notification(id);
                            }
                        }
                        NotificationEvent::Cleared => {
                            toast_overlay.clear_toasts();
                            if open_island_timer.is_none() {
                                dynamic_island.clear_notifications();
                            }
                        }
                        NotificationEvent::DndChanged(_) => {}
                    }
                }
                if notif_changed {
                    let unread = store_for_loop.active_count();
                    let dnd = store_for_loop.is_do_not_disturb();
                    notification_center.refresh_notifications();
                    top_panel.update_system(&last_sys, unread);
                    dock.update_system(&last_sys, unread);
                    if open_island_timer.is_none() {
                        dynamic_island.update_notification_summary(unread, dnd);
                    }
                }
            }

            if let Some(rx) = clipboard_rx_local.as_ref() {
                let mut clip_changed = false;
                while rx.try_recv().is_ok() {
                    clip_changed = true;
                }
                if clip_changed {
                    dynamic_island.on_clipboard_updated();
                }
            }

            if let Some(rx) = polkit_rx_local.as_ref() {
                while let Ok(evt) = rx.try_recv() {
                    polkit_dialog.handle_event(evt);
                }
            }

            if let Some(rx) = tray_rx_local.as_ref() {
                while let Ok(evt) = rx.try_recv() {
                    match evt {
                        TrayEvent::ItemsUpdated(items) => {
                            top_panel.update_tray_items(items.clone());
                            dock.update_tray_items(items);
                        }
                        TrayEvent::MenuLoaded {
                            key,
                            bus_name,
                            menu_path,
                            items,
                        } => {
                            top_panel.handle_tray_menu_loaded(
                                key.clone(),
                                bus_name.clone(),
                                menu_path.clone(),
                                items.clone(),
                            );
                            dock.handle_tray_menu_loaded(key, bus_name, menu_path, items);
                        }
                    }
                }
            }

            glib::ControlFlow::Continue
        });
    });

    let _keep_daemon = notif_daemon;
    let _keep_polkit = polkit_agent;
    app.run_with_args::<&str>(&[]);
}
