//! Entrypoint for `neon-settings` — NEON DE's macOS-inspired GTK4 Settings application.

mod pages;
mod style;
mod system_info;

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use clap::Parser;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Image, Label, ListBox, ListBoxRow,
    Orientation, SearchEntry, SelectionMode, Stack, StackTransitionType, gio, prelude::*,
};
use neon_config::{default_config_path, load_or_default};
use neon_ipc::default_socket_path;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::{
    pages::{
        SettingsController, build_appearance_page, build_displays_page, build_dock_panels_page,
        build_effects_page, build_input_page, build_privacy_accessibility_page, build_system_page,
        build_windows_desktop_page,
    },
    style::SettingsTheme,
};

#[derive(Debug, Parser)]
#[command(name = "neon-settings", version, about)]
struct Cli {
    /// Explicit path to `neon-compositor`'s IPC Unix socket.
    #[arg(long)]
    ipc_socket: Option<PathBuf>,

    /// Explicit path to `neon-de/config.toml`.
    #[arg(long)]
    config: Option<PathBuf>,
}

struct CategoryDef {
    id: &'static str,
    title: &'static str,
    icon: &'static str,
    keywords: &'static str,
}

const CATEGORIES: &[CategoryDef] = &[
    CategoryDef {
        id: "appearance",
        title: "Appearance",
        icon: "preferences-desktop-theme-symbolic",
        keywords: "theme dark light accent color font typography wallpaper",
    },
    CategoryDef {
        id: "displays",
        title: "Displays",
        icon: "video-display-symbolic",
        keywords: "monitor screen resolution refresh fractional scale rotation",
    },
    CategoryDef {
        id: "desktop",
        title: "Desktop & Windows",
        icon: "preferences-system-windows-symbolic",
        keywords: "focus snap tiling workspace overview floating placement",
    },
    CategoryDef {
        id: "dock",
        title: "Dock & Panels",
        icon: "view-grid-symbolic",
        keywords: "dock panel bar clock transparency position autohide",
    },
    CategoryDef {
        id: "effects",
        title: "Effects",
        icon: "applications-graphics-symbolic",
        keywords: "blur shadow rounded corner opacity animation speed motion",
    },
    CategoryDef {
        id: "input",
        title: "Input & Shortcuts",
        icon: "input-keyboard-symbolic",
        keywords: "keyboard mouse touchpad scroll gestures shortcuts",
    },
    CategoryDef {
        id: "privacy",
        title: "Privacy & Accessibility",
        icon: "system-lock-screen-symbolic",
        keywords: "lock security password contrast accessibility screen",
    },
    CategoryDef {
        id: "system",
        title: "System",
        icon: "computer-symbolic",
        keywords: "about version hardware cpu memory audio network bluetooth power reset export",
    },
];

fn resolve_ipc_socket(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    if let Some(env_p) = std::env::var_os("NEON_IPC_SOCKET") {
        return PathBuf::from(env_p);
    }
    let wl = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-1".into());
    default_socket_path(&wl)
}

fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("neon_settings=info,neon_config=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(default_config_path);
    let ipc_socket = resolve_ipc_socket(cli.ipc_socket);
    info!(
        config = %config_path.display(),
        ipc = %ipc_socket.display(),
        "Launching NEON DE Settings"
    );

    let app = Application::builder()
        .application_id("org.neonde.settings")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_activate(move |app| {
        let loaded = load_or_default(&config_path);
        let theme = SettingsTheme::init(loaded.appearance.theme, &loaded.appearance.accent_color);

        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Settings")
            .default_width(940)
            .default_height(640)
            .build();
        window.add_css_class("neon-settings-window");

        let status_label = Label::new(Some("Connected to NEON DE configuration & live IPC"));
        status_label.set_halign(Align::Start);

        let ctrl = Rc::new(SettingsController {
            config_path: config_path.clone(),
            ipc_socket: ipc_socket.clone(),
            config: RefCell::new(loaded),
            theme,
            status_label: status_label.clone(),
        });

        let outer = GtkBox::new(Orientation::Vertical, 0);
        let split = GtkBox::new(Orientation::Horizontal, 0);
        split.set_vexpand(true);

        // Left macOS-inspired sidebar with instant search
        let sidebar = GtkBox::new(Orientation::Vertical, 6);
        sidebar.add_css_class("settings-sidebar");
        sidebar.set_width_request(240);

        let search = SearchEntry::new();
        search.add_css_class("settings-search");
        search.set_placeholder_text(Some("Search settings..."));
        sidebar.append(&search);

        let nav_list = ListBox::new();
        nav_list.set_selection_mode(SelectionMode::Single);
        nav_list.set_vexpand(true);

        let mut nav_rows: Vec<(ListBoxRow, &'static CategoryDef)> = Vec::new();
        for cat in CATEGORIES {
            let row_box = GtkBox::new(Orientation::Horizontal, 10);
            let icon = Image::from_icon_name(cat.icon);
            icon.set_pixel_size(18);
            let lbl = Label::new(Some(cat.title));
            lbl.set_halign(Align::Start);
            row_box.append(&icon);
            row_box.append(&lbl);

            let row = ListBoxRow::new();
            row.add_css_class("settings-nav-row");
            row.set_child(Some(&row_box));
            nav_list.append(&row);
            nav_rows.push((row, cat));
        }
        sidebar.append(&nav_list);

        // Right detail stack with smooth crossfade transitions
        let stack = Stack::new();
        stack.set_transition_type(StackTransitionType::Crossfade);
        stack.set_transition_duration(140);
        stack.set_hexpand(true);
        stack.set_vexpand(true);

        stack.add_named(&build_appearance_page(Rc::clone(&ctrl)), Some("appearance"));
        stack.add_named(&build_displays_page(Rc::clone(&ctrl)), Some("displays"));
        stack.add_named(
            &build_windows_desktop_page(Rc::clone(&ctrl)),
            Some("desktop"),
        );
        stack.add_named(&build_dock_panels_page(Rc::clone(&ctrl)), Some("dock"));
        stack.add_named(&build_effects_page(Rc::clone(&ctrl)), Some("effects"));
        stack.add_named(&build_input_page(Rc::clone(&ctrl)), Some("input"));
        stack.add_named(
            &build_privacy_accessibility_page(Rc::clone(&ctrl)),
            Some("privacy"),
        );
        stack.add_named(&build_system_page(Rc::clone(&ctrl)), Some("system"));

        {
            let stack_clone = stack.clone();
            nav_list.connect_row_selected(move |_, row| {
                if let Some(row) = row
                    && let Some(cat) = CATEGORIES.get(row.index() as usize)
                {
                    stack_clone.set_visible_child_name(cat.id);
                }
            });
        }

        // Instant sidebar search filter
        {
            let nav_rows_clone = nav_rows.clone();
            search.connect_search_changed(move |entry| {
                let q = entry.text().trim().to_lowercase();
                for (row, cat) in &nav_rows_clone {
                    let visible = q.is_empty()
                        || cat.title.to_lowercase().contains(&q)
                        || cat.keywords.contains(&q);
                    row.set_visible(visible);
                }
            });
        }

        if let Some((first_row, _)) = nav_rows.first() {
            nav_list.select_row(Some(first_row));
        }

        split.append(&sidebar);
        split.append(&stack);

        let status_bar = GtkBox::new(Orientation::Horizontal, 8);
        status_bar.add_css_class("settings-status-bar");
        status_bar.append(&status_label);

        outer.append(&split);
        outer.append(&status_bar);

        window.set_child(Some(&outer));
        window.present();
    });

    app.run_with_args::<&str>(&[]);
}
