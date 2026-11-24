//! Entrypoint for `neon-shell` — NEON DE's custom Rust + GTK4 Wayland desktop shell.

mod context_menu;
mod desktop_apps;
mod dock;
mod ipc_bridge;
mod notifications;
mod overview;
mod panel;
mod quick_settings;
mod screenshot_ui;
mod style;
mod wallpaper;
mod workspaces_panel;

use std::{path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use clap::Parser;
use gtk4::{Application, gio, glib, prelude::*};
use neon_system::{
    notifications::{NotificationEvent, NotificationStore, start_notification_daemon},
    start_system_monitor,
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
    ipc_bridge::{ShellIpcUpdate, resolve_ipc_socket_path, start_ipc_bridge},
    notifications::NotificationOverlay,
    overview::Overview,
    panel::TopPanel,
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
}

fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("neon_shell=info,neon_ipc=info,neon_system=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    let cli = Cli::parse();
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

    let (sys_tx, sys_rx) = start_system_monitor(Duration::from_secs(2));

    let app = Application::builder()
        .application_id("org.neonde.shell")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let notif_rx = std::cell::RefCell::new(Some(notif_rx));
    let sys_rx = std::cell::RefCell::new(Some(sys_rx));

    app.connect_activate(move |app| {
        let mut theme_ctrl = style::ShellThemeController::init();

        let apps = discover_applications();
        info!(
            app_count = apps.len(),
            "Discovered installed XDG desktop applications"
        );
        let favorites = select_default_favorites(&apps);

        let (ipc_sender, update_rx) = start_ipc_bridge(socket_path.clone());

        // System-wide macOS right-click context menu overlay (Desktop, Dock, Panel, Windows).
        let context_menu = ContextMenuOverlay::build(app, apps.clone(), ipc_sender.clone());

        // Background wallpaper layer renders behind all desktop windows & panels.
        let wallpaper_layer = WallpaperLayer::build(app, context_menu.clone());
        let screenshot_overlay = ScreenshotOverlay::build(app, ipc_sender.clone());
        let toast_overlay = NotificationOverlay::build(app, Arc::clone(&notif_store));
        let quick_settings = QuickSettingsPanel::build(
            app,
            ipc_sender.clone(),
            sys_tx.clone(),
            Arc::clone(&notif_store),
            toast_overlay.clone(),
        );
        let workspaces_panel = WorkspacesPanel::build(app, ipc_sender.clone());
        let top_panel = Rc::new(TopPanel::build(
            app,
            ipc_sender.clone(),
            sys_tx.clone(),
            quick_settings.clone(),
            workspaces_panel.clone(),
            context_menu.clone(),
        ));
        let dock = Rc::new(Dock::build(
            app,
            &favorites,
            apps.clone(),
            ipc_sender.clone(),
            context_menu.clone(),
        ));
        let overview = Overview::build(app, apps, ipc_sender);
        {
            let tp_layer = Rc::clone(&top_panel);
            let dk_layer = Rc::clone(&dock);
            let tp_bg = Rc::clone(&top_panel);
            let dk_bg = Rc::clone(&dock);
            overview.set_chrome_hooks(
                move |elevated| {
                    tp_layer.set_drawer_layer_elevated(elevated);
                    dk_layer.set_drawer_layer_elevated(elevated);
                },
                move |hidden| {
                    tp_bg.set_drawer_bg_hidden(hidden);
                    dk_bg.set_drawer_bg_hidden(hidden);
                },
            );
        }

        let sys_rx_local = sys_rx.borrow_mut().take();
        let notif_rx_local = notif_rx.borrow_mut().take();
        let store_for_loop = Arc::clone(&notif_store);
        let mut last_sys = neon_system::SystemSnapshot::default();

        glib::timeout_add_local(Duration::from_millis(33), move || {
            while let Ok(update) = update_rx.try_recv() {
                match update {
                    ShellIpcUpdate::Snapshot(snapshot) => {
                        set_live_xwayland_display(snapshot.xwayland_display.as_deref());
                        set_live_cursor_config(
                            snapshot.appearance.cursor_theme.as_deref(),
                            snapshot.appearance.cursor_size,
                        );
                        theme_ctrl.apply_if_changed(&snapshot.appearance);
                        wallpaper_layer.update(&snapshot.appearance);
                        quick_settings.update_appearance(&snapshot.appearance);
                        context_menu.update_snapshot(&snapshot);
                        screenshot_overlay.update_snapshot(&snapshot);
                        top_panel.update(&snapshot);
                        workspaces_panel.update(&snapshot);
                        dock.update(&snapshot);
                        overview.update(&snapshot);
                    }
                    ShellIpcUpdate::OverviewToggled(open) => {
                        context_menu.close();
                        if open {
                            quick_settings.close();
                            workspaces_panel.close();
                        }
                        overview.set_open(open);
                    }
                    ShellIpcUpdate::ScreenshotUiRequested => {
                        context_menu.close();
                        screenshot_overlay.open();
                    }
                    ShellIpcUpdate::ContextMenuRequested { x, y, target } => {
                        context_menu.handle_compositor_request(x, y, target);
                    }
                }
            }

            if let Some(rx) = sys_rx_local.as_ref() {
                while let Ok(sys_snap) = rx.try_recv() {
                    last_sys = sys_snap;
                    top_panel.update_system(&last_sys, store_for_loop.active_count());
                    quick_settings.update_system(&last_sys);
                }
            }

            if let Some(rx) = notif_rx_local.as_ref() {
                let mut notif_changed = false;
                while let Ok(evt) = rx.try_recv() {
                    notif_changed = true;
                    match evt {
                        NotificationEvent::Added(item) => toast_overlay.push_toast(item),
                        NotificationEvent::Closed { id, .. } => toast_overlay.remove_toast(id),
                        NotificationEvent::Cleared => toast_overlay.clear_toasts(),
                    }
                }
                if notif_changed {
                    quick_settings.refresh_notifications();
                    top_panel.update_system(&last_sys, store_for_loop.active_count());
                }
            }

            glib::ControlFlow::Continue
        });
    });

    let _keep_daemon = notif_daemon;
    app.run_with_args::<&str>(&[]);
}
