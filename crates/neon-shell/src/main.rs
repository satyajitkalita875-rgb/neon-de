//! Entrypoint for `neon-shell` — NEON DE's custom Rust + GTK4 Wayland desktop shell.

mod desktop_apps;
mod dock;
mod ipc_bridge;
mod overview;
mod panel;
mod style;

use std::{path::PathBuf, time::Duration};

use clap::Parser;
use gtk4::{Application, gio, glib, prelude::*};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::{
    desktop_apps::{discover_applications, select_default_favorites},
    dock::Dock,
    ipc_bridge::{ShellIpcUpdate, resolve_ipc_socket_path, start_ipc_bridge},
    overview::Overview,
    panel::TopPanel,
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
        .unwrap_or_else(|_| EnvFilter::new("neon_shell=info,neon_ipc=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    let cli = Cli::parse();
    let socket_path = resolve_ipc_socket_path(cli.ipc_socket);
    info!(ipc_socket = %socket_path.display(), "Starting NEON DE Desktop Shell");

    let app = Application::builder()
        .application_id("org.neonde.shell")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let mut theme_ctrl = style::ShellThemeController::init();

        let apps = discover_applications();
        info!(
            app_count = apps.len(),
            "Discovered installed XDG desktop applications"
        );
        let favorites = select_default_favorites(&apps);

        let (ipc_sender, update_rx) = start_ipc_bridge(socket_path.clone());

        let top_panel = TopPanel::build(app, ipc_sender.clone());
        let dock = Dock::build(app, &favorites, ipc_sender.clone());
        let overview = Overview::build(app, apps, ipc_sender);

        // Drain IPC events on the GTK4 main loop cleanly without blocking UI frames.
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
            glib::ControlFlow::Continue
        });
    });

    // Pass empty argv to GTK since `clap` already parsed CLI flags.
    app.run_with_args::<&str>(&[]);
}
