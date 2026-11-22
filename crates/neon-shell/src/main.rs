//! Entrypoint for `neon-shell` — NEON DE's custom Rust + GTK4 Wayland desktop shell.

mod desktop_apps;
mod dock;
mod ipc_bridge;
mod notifications;
mod overview;
mod panel;
mod quick_settings;
mod style;

use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use gtk4::{Application, gio, glib, prelude::*};
use neon_system::{
    notifications::{
        NESTED_BUS_NAME, NotificationEvent, NotificationStore, start_notification_daemon,
    },
    start_system_monitor,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::{
    desktop_apps::{discover_applications, select_default_favorites},
    dock::Dock,
    ipc_bridge::{ShellIpcUpdate, resolve_ipc_socket_path, start_ipc_bridge},
    notifications::NotificationOverlay,
    overview::Overview,
    panel::TopPanel,
    quick_settings::QuickSettingsPanel,
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

    // Start NEON DE's Freedesktop Notifications v1.2 D-Bus service safely
    // (never steals `org.freedesktop.Notifications` from a host desktop in nested mode).
    let notif_daemon = match start_notification_daemon(Arc::clone(&notif_store), None) {
        Ok(handle) => Some(handle),
        Err(err) => {
            warn!(%err, "Notification D-Bus daemon could not bind; continuing with in-process store");
            None
        }
    };
    let active_notif_bus = notif_daemon
        .as_ref()
        .map(|h| h.bus_name.clone())
        .unwrap_or_else(|| NESTED_BUS_NAME.to_string());

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

        let toast_overlay = NotificationOverlay::build(app, Arc::clone(&notif_store));
        let quick_settings = QuickSettingsPanel::build(
            app,
            ipc_sender.clone(),
            sys_tx.clone(),
            Arc::clone(&notif_store),
            active_notif_bus.clone(),
            toast_overlay.clone(),
        );
        let top_panel = TopPanel::build(
            app,
            ipc_sender.clone(),
            sys_tx.clone(),
            quick_settings.clone(),
        );
        let dock = Dock::build(app, &favorites, ipc_sender.clone());
        let overview = Overview::build(app, apps, ipc_sender);

        let sys_rx_local = sys_rx.borrow_mut().take();
        let notif_rx_local = notif_rx.borrow_mut().take();
        let store_for_loop = Arc::clone(&notif_store);
        let mut last_sys = neon_system::SystemSnapshot::default();

        // Drain compositor IPC, system D-Bus, and notification events cleanly on the GTK4 main loop.
        glib::timeout_add_local(Duration::from_millis(33), move || {
            while let Ok(update) = update_rx.try_recv() {
                match update {
                    ShellIpcUpdate::Snapshot(snapshot) => {
                        theme_ctrl.apply_if_changed(&snapshot.appearance);
                        top_panel.update(&snapshot);
                        dock.update(&snapshot);
                        overview.update(&snapshot);
                    }
                    ShellIpcUpdate::OverviewToggled(open) => {
                        overview.set_open(open);
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

    // Keep D-Bus notification daemon alive for the entire lifetime of `neon-shell`.
    let _keep_daemon = notif_daemon;
    app.run_with_args::<&str>(&[]);
}
