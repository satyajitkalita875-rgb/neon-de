//! Entrypoint for `neon-settings` — NEON DE's macOS Sequoia × GNOME 47 Settings app
//! with KDE-grade customization and security controls.

mod pages;
mod pages_system;
mod style;
mod system_info;

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use clap::Parser;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, EventControllerKey,
    Image, Label, ListBox, ListBoxRow, MenuButton, Orientation, PolicyType, Popover, Revealer,
    RevealerTransitionType, ScrolledWindow, SearchEntry, SelectionMode, Separator, Stack,
    StackTransitionType, WindowHandle, gdk, gio, prelude::*,
};
use neon_config::{ThemeMode, default_config_path, load_or_default};
use neon_ipc::default_socket_path;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::{
    pages::{
        SettingsController, build_appearance_page, build_appearance_request, build_displays_page,
        build_dock_panels_page, build_windows_effects_page,
    },
    pages_system::{
        build_accessibility_page, build_bluetooth_page, build_keyboard_page,
        build_mouse_touchpad_page, build_network_page, build_notifications_page, build_power_page,
        build_privacy_security_page, build_sound_page, build_system_page,
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
    group: u8,
    keywords: &'static str,
}

const CATEGORIES: &[CategoryDef] = &[
    // Group 0: Connectivity
    CategoryDef {
        id: "network",
        title: "Network",
        icon: "network-wireless-symbolic",
        group: 0,
        keywords: "wifi wi-fi ethernet internet ip connection networkmanager",
    },
    CategoryDef {
        id: "bluetooth",
        title: "Bluetooth",
        icon: "bluetooth-active-symbolic",
        group: 0,
        keywords: "wireless headphones speaker mouse keyboard pair bluez",
    },
    // Group 1: Hardware & Power
    CategoryDef {
        id: "displays",
        title: "Displays",
        icon: "video-display-symbolic",
        group: 1,
        keywords: "monitor screen resolution refresh fractional scale rotation",
    },
    CategoryDef {
        id: "sound",
        title: "Sound",
        icon: "audio-speakers-symbolic",
        group: 1,
        keywords: "audio volume speakers headphones microphone pipewire balance mute",
    },
    CategoryDef {
        id: "power",
        title: "Power",
        icon: "battery-good-symbolic",
        group: 1,
        keywords: "battery energy profile saver balanced performance sleep",
    },
    // Group 2: Customization (macOS + KDE-grade controls)
    CategoryDef {
        id: "appearance",
        title: "Appearance",
        icon: "preferences-desktop-appearance-symbolic",
        group: 2,
        keywords: "theme dark light style accent color wallpaper background icon cursor font",
    },
    CategoryDef {
        id: "dock",
        title: "Dock & Panels",
        icon: "view-grid-symbolic",
        group: 2,
        keywords: "dock panel menu bar topbar clock workspaces autohide transparency",
    },
    CategoryDef {
        id: "windows",
        title: "Windows & Effects",
        icon: "preferences-system-windows-symbolic",
        group: 2,
        keywords: "focus snap tiling blur shadow corner radius opacity animation kwin kde",
    },
    CategoryDef {
        id: "notifications",
        title: "Notifications",
        icon: "preferences-system-notifications-symbolic",
        group: 2,
        keywords: "alerts toasts banners do not disturb dbus test",
    },
    // Group 3: Input Devices
    CategoryDef {
        id: "mouse",
        title: "Mouse & Touchpad",
        icon: "input-mouse-symbolic",
        group: 3,
        keywords: "pointer speed acceleration scroll natural traditional tap gesture primary button",
    },
    CategoryDef {
        id: "keyboard",
        title: "Keyboard",
        icon: "input-keyboard-symbolic",
        group: 3,
        keywords: "layout input source shortcuts keybindings super alt repeat",
    },
    // Group 4: Accessibility, Security & System
    CategoryDef {
        id: "accessibility",
        title: "Accessibility",
        icon: "preferences-desktop-accessibility-symbolic",
        group: 4,
        keywords: "high contrast reduced motion vision readability",
    },
    CategoryDef {
        id: "privacy",
        title: "Privacy & Security",
        icon: "security-high-symbolic",
        group: 4,
        keywords: "lock screen password polkit peercred portal sandbox clipboard security",
    },
    CategoryDef {
        id: "system",
        title: "System",
        icon: "computer-symbolic",
        group: 4,
        keywords: "about hardware cpu memory gpu diagnostics backup export reset version",
    },
];

const SIDEBAR_WIDTH: i32 = 248;

fn resolve_ipc_socket(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    if let Some(env_p) = std::env::var_os("NEON_IPC_SOCKET") {
        return PathBuf::from(env_p);
    }
    let wl = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
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
            .title("Settings")
            .default_width(980)
            .default_height(680)
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

        // ── Split Headerbar (macOS Sequoia × GNOME 47) ───────────────────────
        let split_header = GtkBox::new(Orientation::Horizontal, 0);
        split_header.add_css_class("settings-split-header");

        // Left sidebar header: [Search]  Settings  [Menu]
        let sidebar_header = CenterBox::new();
        sidebar_header.add_css_class("settings-sidebar-header");
        sidebar_header.set_width_request(SIDEBAR_WIDTH);

        let search_btn = Button::new();
        search_btn.add_css_class("settings-header-icon-btn");
        search_btn.set_tooltip_text(Some("Search settings (Ctrl+F)"));
        let search_icon = Image::from_icon_name("system-search-symbolic");
        search_icon.set_pixel_size(15);
        search_btn.set_child(Some(&search_icon));
        sidebar_header.set_start_widget(Some(&search_btn));

        let sidebar_title = Label::new(Some("Settings"));
        sidebar_title.add_css_class("settings-header-title");
        sidebar_header.set_center_widget(Some(&sidebar_title));

        let menu_btn = MenuButton::new();
        menu_btn.add_css_class("settings-header-icon-btn");
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.set_tooltip_text(Some("Quick Actions"));

        let menu_box = GtkBox::new(Orientation::Vertical, 4);
        menu_box.set_margin_top(6);
        menu_box.set_margin_bottom(6);
        menu_box.set_margin_start(6);
        menu_box.set_margin_end(6);

        let toggle_theme_btn = Button::with_label("Switch Light / Dark Style");
        toggle_theme_btn.add_css_class("settings-menu-item");
        let sync_btn = Button::with_label("Sync Settings to Compositor");
        sync_btn.add_css_class("settings-menu-item");
        let about_btn = Button::with_label("About NEON DE");
        about_btn.add_css_class("settings-menu-item");
        menu_box.append(&toggle_theme_btn);
        menu_box.append(&sync_btn);
        menu_box.append(&about_btn);

        let popover = Popover::new();
        popover.set_child(Some(&menu_box));
        menu_btn.set_popover(Some(&popover));
        sidebar_header.set_end_widget(Some(&menu_btn));

        // Right content header: [Dynamic Page Title]  [×]
        let content_header = CenterBox::new();
        content_header.add_css_class("settings-content-header");
        content_header.set_hexpand(true);

        let page_title_label = Label::new(Some("Appearance"));
        page_title_label.add_css_class("settings-header-title");
        content_header.set_center_widget(Some(&page_title_label));

        let close_btn = Button::new();
        close_btn.add_css_class("settings-close-circle");
        close_btn.set_tooltip_text(Some("Close Settings"));
        let close_icon = Image::from_icon_name("window-close-symbolic");
        close_icon.set_pixel_size(13);
        close_btn.set_child(Some(&close_icon));
        {
            let win = window.clone();
            close_btn.connect_clicked(move |_| win.close());
        }
        content_header.set_end_widget(Some(&close_btn));

        split_header.append(&sidebar_header);
        split_header.append(&content_header);

        let titlebar_handle = WindowHandle::new();
        titlebar_handle.set_child(Some(&split_header));
        window.set_titlebar(Some(&titlebar_handle));

        // ── Main Split Body ──────────────────────────────────────────────────
        let outer = GtkBox::new(Orientation::Vertical, 0);
        let split = GtkBox::new(Orientation::Horizontal, 0);
        split.set_vexpand(true);

        let sidebar = GtkBox::new(Orientation::Vertical, 4);
        sidebar.add_css_class("settings-sidebar");
        sidebar.set_width_request(SIDEBAR_WIDTH);

        let search_revealer = Revealer::new();
        search_revealer.set_transition_type(RevealerTransitionType::SlideDown);
        search_revealer.set_transition_duration(150);
        search_revealer.set_reveal_child(false);

        let search = SearchEntry::new();
        search.add_css_class("settings-search");
        search.set_placeholder_text(Some("Search settings..."));
        search_revealer.set_child(Some(&search));
        sidebar.append(&search_revealer);

        {
            let rev = search_revealer.clone();
            let entry = search.clone();
            search_btn.connect_clicked(move |_| {
                let next = !rev.reveals_child();
                rev.set_reveal_child(next);
                if next {
                    entry.grab_focus();
                } else {
                    entry.set_text("");
                }
            });
        }

        // Ctrl+F shortcut to open sidebar search, Escape to close it
        {
            let key_ctrl = EventControllerKey::new();
            let rev = search_revealer.clone();
            let entry = search.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, mods| {
                if mods.contains(gdk::ModifierType::CONTROL_MASK)
                    && matches!(key, gdk::Key::f | gdk::Key::F)
                {
                    rev.set_reveal_child(true);
                    entry.grab_focus();
                    return gtk4::glib::Propagation::Stop;
                }
                if key == gdk::Key::Escape && rev.reveals_child() {
                    entry.set_text("");
                    rev.set_reveal_child(false);
                    return gtk4::glib::Propagation::Stop;
                }
                gtk4::glib::Propagation::Proceed
            });
            window.add_controller(key_ctrl);
        }

        let nav_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();

        let nav_list = ListBox::new();
        nav_list.add_css_class("settings-nav-list");
        nav_list.set_selection_mode(SelectionMode::Single);
        nav_list.set_vexpand(true);

        let mut nav_rows: Vec<(ListBoxRow, &'static CategoryDef)> = Vec::new();
        let mut sep_rows: Vec<ListBoxRow> = Vec::new();
        let mut last_group: Option<u8> = None;

        for cat in CATEGORIES {
            if let Some(prev) = last_group
                && prev != cat.group
            {
                let sep = Separator::new(Orientation::Horizontal);
                sep.add_css_class("settings-sidebar-sep");
                let sep_row = ListBoxRow::new();
                sep_row.set_selectable(false);
                sep_row.set_activatable(false);
                sep_row.set_focusable(false);
                sep_row.set_child(Some(&sep));
                nav_list.append(&sep_row);
                sep_rows.push(sep_row);
            }
            last_group = Some(cat.group);

            let row_box = GtkBox::new(Orientation::Horizontal, 11);
            let icon = Image::from_icon_name(cat.icon);
            icon.set_pixel_size(16);
            let lbl = Label::new(Some(cat.title));
            lbl.set_halign(Align::Start);
            row_box.append(&icon);
            row_box.append(&lbl);

            let row = ListBoxRow::new();
            row.add_css_class("settings-nav-row");
            row.set_widget_name(cat.id);
            row.set_child(Some(&row_box));
            nav_list.append(&row);
            nav_rows.push((row, cat));
        }

        nav_scroll.set_child(Some(&nav_list));
        sidebar.append(&nav_scroll);

        // ── Right Detail Stack (14 Pages) ────────────────────────────────────
        let stack = Stack::new();
        stack.set_transition_type(StackTransitionType::Crossfade);
        stack.set_transition_duration(130);
        stack.set_hexpand(true);
        stack.set_vexpand(true);

        stack.add_named(&build_network_page(Rc::clone(&ctrl)), Some("network"));
        stack.add_named(&build_bluetooth_page(Rc::clone(&ctrl)), Some("bluetooth"));
        stack.add_named(&build_displays_page(Rc::clone(&ctrl)), Some("displays"));
        stack.add_named(&build_sound_page(Rc::clone(&ctrl)), Some("sound"));
        stack.add_named(&build_power_page(Rc::clone(&ctrl)), Some("power"));
        stack.add_named(&build_appearance_page(Rc::clone(&ctrl)), Some("appearance"));
        stack.add_named(&build_dock_panels_page(Rc::clone(&ctrl)), Some("dock"));
        stack.add_named(
            &build_windows_effects_page(Rc::clone(&ctrl)),
            Some("windows"),
        );
        stack.add_named(
            &build_notifications_page(Rc::clone(&ctrl)),
            Some("notifications"),
        );
        stack.add_named(&build_mouse_touchpad_page(Rc::clone(&ctrl)), Some("mouse"));
        stack.add_named(&build_keyboard_page(Rc::clone(&ctrl)), Some("keyboard"));
        stack.add_named(
            &build_accessibility_page(Rc::clone(&ctrl)),
            Some("accessibility"),
        );
        stack.add_named(
            &build_privacy_security_page(Rc::clone(&ctrl)),
            Some("privacy"),
        );
        stack.add_named(&build_system_page(Rc::clone(&ctrl)), Some("system"));

        // Wire sidebar selection -> stack page & header title
        {
            let stack_clone = stack.clone();
            let title_clone = page_title_label.clone();
            nav_list.connect_row_selected(move |_, row| {
                if let Some(row) = row {
                    let id = row.widget_name();
                    if let Some(cat) = CATEGORIES.iter().find(|c| c.id == id.as_str()) {
                        stack_clone.set_visible_child_name(cat.id);
                        title_clone.set_text(cat.title);
                    }
                }
            });
        }

        // Wire quick-menu actions
        {
            let ctrl = Rc::clone(&ctrl);
            let pop = popover.clone();
            toggle_theme_btn.connect_clicked(move |_| {
                pop.popdown();
                let next = {
                    let mut cfg = ctrl.config.borrow_mut();
                    let next = if cfg.appearance.theme == ThemeMode::NeonLight {
                        ThemeMode::NeonDark
                    } else {
                        ThemeMode::NeonLight
                    };
                    cfg.appearance.theme = next;
                    next
                };
                let req = build_appearance_request(&ctrl.config.borrow());
                ctrl.persist_and_sync(
                    Some(req),
                    if next == ThemeMode::NeonLight {
                        "Switched to Neon Light"
                    } else {
                        "Switched to Neon Dark"
                    },
                );
            });
        }
        {
            let ctrl = Rc::clone(&ctrl);
            let pop = popover.clone();
            sync_btn.connect_clicked(move |_| {
                pop.popdown();
                ctrl.push_full_config_live();
                ctrl.status_label
                    .set_text("Synchronized full configuration to live compositor");
            });
        }
        {
            let nav_list_c = nav_list.clone();
            let nav_rows_c = nav_rows.clone();
            let pop = popover.clone();
            about_btn.connect_clicked(move |_| {
                pop.popdown();
                if let Some((row, _)) = nav_rows_c.iter().find(|(_, c)| c.id == "system") {
                    nav_list_c.select_row(Some(row));
                }
            });
        }

        // Instant sidebar search filter (hides group separators while filtering)
        {
            let nav_rows_clone = nav_rows.clone();
            let sep_rows_clone = sep_rows.clone();
            search.connect_search_changed(move |entry| {
                let q = entry.text().trim().to_lowercase();
                for (row, cat) in &nav_rows_clone {
                    let visible = q.is_empty()
                        || cat.title.to_lowercase().contains(&q)
                        || cat.keywords.contains(&q);
                    row.set_visible(visible);
                }
                for sep in &sep_rows_clone {
                    sep.set_visible(q.is_empty());
                }
            });
        }

        // Default to Appearance so visual style cards & wallpaper grid greet the user
        if let Some((row, _)) = nav_rows.iter().find(|(_, c)| c.id == "appearance") {
            nav_list.select_row(Some(row));
        } else if let Some((first_row, _)) = nav_rows.first() {
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
