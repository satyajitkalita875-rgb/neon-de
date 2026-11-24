//! Entrypoint for `neon-settings` — NEON DE's complete macOS-inspired GTK4 Settings
//! application with real Linux D-Bus/PipeWire/XDG/IPC integration across 22 categories.

mod metadata;
mod navigation;
mod pages;
mod state;
mod system_info;
mod ui;

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

use clap::Parser;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, EventControllerKey,
    Image, Label, ListBox, ListBoxRow, MenuButton, Orientation, PolicyType, Popover,
    ScrolledWindow, SearchEntry, SelectionMode, Separator, Stack, StackTransitionType,
    WindowHandle, gdk, gio, glib, prelude::*,
};
use neon_config::{ThemeMode, default_config_path, load_or_default};
use neon_ipc::default_socket_path;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::{
    metadata::search_settings,
    navigation::{ABOUT_CATEGORY, CategoryDef, NavigationHistory, SIDEBAR_CATEGORIES, find_category},
    state::{SettingsController, build_appearance_request},
    ui::{
        boxed_card, capability_badge, row_separator, section_heading, style::SettingsTheme,
    },
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

    /// Optional initial settings page to open (e.g. `appearance`, `displays`, `shortcuts`).
    #[arg(long)]
    page: Option<String>,
}

const SIDEBAR_WIDTH: i32 = 258;

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
    let cli_page = cli.page;

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
        let initial_page = cli_page
            .clone()
            .filter(|p| find_category(p).is_some())
            .or_else(|| {
                let saved = loaded.session.last_settings_page.clone();
                find_category(&saved).map(|c| c.id.to_string())
            })
            .unwrap_or_else(|| "appearance".to_string());

        let theme = SettingsTheme::init(loaded.appearance.theme, &loaded.appearance.accent_color);

        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Settings")
            .default_width(1040)
            .default_height(720)
            .build();
        window.add_css_class("neon-settings-window");

        let status_label = Label::new(Some("Connected to NEON DE configuration & system services"));
        status_label.set_halign(Align::Start);
        let status_bar = GtkBox::new(Orientation::Horizontal, 8);
        status_bar.add_css_class("settings-status-bar");
        status_bar.append(&status_label);

        let ctrl = Rc::new(SettingsController {
            config_path: config_path.clone(),
            ipc_socket: ipc_socket.clone(),
            config: RefCell::new(loaded),
            theme,
            status_label: status_label.clone(),
            status_bar: status_bar.clone(),
            window: window.clone(),
            navigate_cb: RefCell::new(None),
            rebuild_page_cb: RefCell::new(None),
        });

        // ── Split Headerbar ──────────────────────────────────────────────────
        let split_header = GtkBox::new(Orientation::Horizontal, 0);
        split_header.add_css_class("settings-split-header");

        let sidebar_header = CenterBox::new();
        sidebar_header.add_css_class("settings-sidebar-header");
        sidebar_header.set_width_request(SIDEBAR_WIDTH);

        let traffic_lights = GtkBox::new(Orientation::Horizontal, 8);
        traffic_lights.add_css_class("macos-traffic-lights");
        traffic_lights.set_valign(Align::Center);

        let close_btn = Button::with_label("×");
        close_btn.add_css_class("macos-tl-btn");
        close_btn.add_css_class("macos-tl-close");
        close_btn.set_tooltip_text(Some("Close"));
        {
            let win = window.clone();
            close_btn.connect_clicked(move |_| win.close());
        }

        let min_btn = Button::with_label("−");
        min_btn.add_css_class("macos-tl-btn");
        min_btn.add_css_class("macos-tl-min");
        min_btn.set_tooltip_text(Some("Minimize"));
        {
            let win = window.clone();
            min_btn.connect_clicked(move |_| win.minimize());
        }

        let max_btn = Button::with_label("+");
        max_btn.add_css_class("macos-tl-btn");
        max_btn.add_css_class("macos-tl-max");
        max_btn.set_tooltip_text(Some("Maximize / Restore"));
        {
            let win = window.clone();
            max_btn.connect_clicked(move |_| {
                if win.is_maximized() {
                    win.unmaximize();
                } else {
                    win.maximize();
                }
            });
        }

        traffic_lights.append(&close_btn);
        traffic_lights.append(&min_btn);
        traffic_lights.append(&max_btn);
        sidebar_header.set_start_widget(Some(&traffic_lights));

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

        let toggle_theme_btn = Button::with_label("Toggle Light / Dark Style");
        toggle_theme_btn.add_css_class("settings-menu-item");
        let sync_btn = Button::with_label("Sync Full Config to Compositor");
        sync_btn.add_css_class("settings-menu-item");
        let shortcuts_menu_btn = Button::with_label("Keyboard Shortcuts");
        shortcuts_menu_btn.add_css_class("settings-menu-item");
        let about_menu_btn = Button::with_label("About NEON DE");
        about_menu_btn.add_css_class("settings-menu-item");
        menu_box.append(&toggle_theme_btn);
        menu_box.append(&sync_btn);
        menu_box.append(&shortcuts_menu_btn);
        menu_box.append(&about_menu_btn);

        let popover = Popover::new();
        popover.set_child(Some(&menu_box));
        menu_btn.set_popover(Some(&popover));
        sidebar_header.set_end_widget(Some(&menu_btn));

        // Content header: Back button + active category title
        let content_header = CenterBox::new();
        content_header.add_css_class("settings-content-header");
        content_header.set_hexpand(true);

        let back_btn = Button::new();
        back_btn.add_css_class("settings-header-icon-btn");
        back_btn.set_tooltip_text(Some("Back (Alt+Left)"));
        let back_icon = Image::from_icon_name("go-previous-symbolic");
        back_icon.set_pixel_size(16);
        back_btn.set_child(Some(&back_icon));
        back_btn.set_sensitive(false);
        content_header.set_start_widget(Some(&back_btn));

        let page_title_label = Label::new(Some("Appearance"));
        page_title_label.add_css_class("settings-header-title");
        content_header.set_center_widget(Some(&page_title_label));

        split_header.append(&sidebar_header);
        split_header.append(&content_header);

        let titlebar_handle = WindowHandle::new();
        titlebar_handle.set_child(Some(&split_header));
        window.set_titlebar(Some(&titlebar_handle));

        // ── Main Two-Panel Layout ────────────────────────────────────────────
        let outer = GtkBox::new(Orientation::Vertical, 0);
        let split = GtkBox::new(Orientation::Horizontal, 0);
        split.set_vexpand(true);

        let sidebar = GtkBox::new(Orientation::Vertical, 4);
        sidebar.add_css_class("settings-sidebar");
        sidebar.set_width_request(SIDEBAR_WIDTH);

        // NEON DE Brand Header
        let brand_row = GtkBox::new(Orientation::Horizontal, 10);
        brand_row.add_css_class("neon-brand-card");
        brand_row.set_valign(Align::Center);
        let emblem = Label::new(Some("N"));
        emblem.add_css_class("neon-brand-emblem");
        emblem.set_halign(Align::Center);
        emblem.set_valign(Align::Center);
        let brand_text = GtkBox::new(Orientation::Vertical, 1);
        let brand_name = Label::new(Some("NEON DE"));
        brand_name.add_css_class("neon-brand-title");
        brand_name.set_halign(Align::Start);
        let brand_sub = Label::new(Some("System Settings"));
        brand_sub.add_css_class("neon-brand-sub");
        brand_sub.set_halign(Align::Start);
        brand_text.append(&brand_name);
        brand_text.append(&brand_sub);
        brand_row.append(&emblem);
        brand_row.append(&brand_text);
        sidebar.append(&brand_row);

        // Persistent Search Field
        let search = SearchEntry::new();
        search.add_css_class("settings-search");
        search.set_placeholder_text(Some("Search settings (Ctrl+F)"));
        sidebar.append(&search);

        // Scrollable Category List
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

        for cat in SIDEBAR_CATEGORIES {
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

            let row_box = GtkBox::new(Orientation::Horizontal, 10);
            let icon = Image::from_icon_name(cat.icon);
            icon.set_pixel_size(16);
            let lbl = Label::new(Some(cat.title));
            lbl.set_halign(Align::Start);
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
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

        // Pinned "About NEON DE" button at the bottom of the sidebar
        let about_sep = Separator::new(Orientation::Horizontal);
        about_sep.add_css_class("settings-sidebar-sep");
        sidebar.append(&about_sep);

        let about_footer_btn = Button::new();
        about_footer_btn.add_css_class("settings-about-footer-btn");
        let about_row = GtkBox::new(Orientation::Horizontal, 10);
        let about_icon = Image::from_icon_name(ABOUT_CATEGORY.icon);
        about_icon.set_pixel_size(16);
        let about_lbl = Label::new(Some(ABOUT_CATEGORY.title));
        about_lbl.set_halign(Align::Start);
        about_row.append(&about_icon);
        about_row.append(&about_lbl);
        about_footer_btn.set_child(Some(&about_row));
        sidebar.append(&about_footer_btn);

        // ── Main Content Stack (Lazy-Built Pages + Search Results View) ──────
        let stack = Stack::new();
        stack.set_transition_type(StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.set_hexpand(true);
        stack.set_vexpand(true);

        // Search results page
        let search_results_outer = GtkBox::new(Orientation::Vertical, 0);
        search_results_outer.set_halign(Align::Center);
        search_results_outer.set_hexpand(true);
        let search_results_col = GtkBox::new(Orientation::Vertical, 6);
        search_results_col.add_css_class("settings-content-clamp");
        search_results_col.set_width_request(620);
        search_results_outer.append(&search_results_col);
        let search_results_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&search_results_outer)
            .build();
        stack.add_named(&search_results_scroll, Some("__search__"));

        let built_pages: Rc<RefCell<HashMap<String, ScrolledWindow>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let history = Rc::new(RefCell::new(NavigationHistory::default()));
        let active_page_id = Rc::new(RefCell::new(String::new()));
        let suppress_nav_signal = Rc::new(RefCell::new(false));

        // Central navigation routine (lazy-builds pages, updates history, saves last page)
        let nav_rows_rc = Rc::new(nav_rows);
        let navigate_core: Rc<dyn Fn(&str, bool, Option<&str>)> = {
            let ctrl = Rc::clone(&ctrl);
            let stack = stack.clone();
            let title_lbl = page_title_label.clone();
            let nav_list = nav_list.clone();
            let nav_rows = Rc::clone(&nav_rows_rc);
            let about_btn_c = about_footer_btn.clone();
            let built_pages = Rc::clone(&built_pages);
            let history = Rc::clone(&history);
            let back_btn = back_btn.clone();
            let active_id = Rc::clone(&active_page_id);
            let suppress = Rc::clone(&suppress_nav_signal);

            Rc::new(move |raw_id: &str, record_history: bool, highlight: Option<&str>| {
                let Some(cat) = find_category(raw_id) else {
                    return;
                };
                let id = cat.id;

                // Lazy-build page on first visit
                if !built_pages.borrow().contains_key(id) {
                    let widget = pages::build_page(id, Rc::clone(&ctrl));
                    stack.add_named(&widget, Some(id));
                    built_pages.borrow_mut().insert(id.to_string(), widget);
                }

                stack.set_visible_child_name(id);
                title_lbl.set_text(cat.title);
                *active_id.borrow_mut() = id.to_string();

                if record_history {
                    history.borrow_mut().visit(id);
                }
                back_btn.set_sensitive(history.borrow().can_go_back());

                // Sync sidebar active state without re-triggering signal
                *suppress.borrow_mut() = true;
                if id == ABOUT_CATEGORY.id {
                    nav_list.unselect_all();
                    about_btn_c.add_css_class("active");
                } else {
                    about_btn_c.remove_css_class("active");
                    if let Some((row, _)) = nav_rows.iter().find(|(_, c)| c.id == id) {
                        nav_list.select_row(Some(row));
                    }
                }
                *suppress.borrow_mut() = false;

                // Persist last opened page quietly
                if ctrl.config.borrow().session.last_settings_page != id {
                    ctrl.config.borrow_mut().session.last_settings_page = id.to_string();
                    ctrl.persist_silent();
                }

                if let Some(setting_name) = highlight {
                    ctrl.set_status_info(&format!("Opened {} → {setting_name}", cat.title));
                }
            })
        };

        // Wire programmatic navigate & rebuild callbacks into SettingsController
        {
            let nav_fn = Rc::clone(&navigate_core);
            let search_c = search.clone();
            *ctrl.navigate_cb.borrow_mut() = Some(Rc::new(move |id, highlight| {
                if !search_c.text().is_empty() {
                    search_c.set_text("");
                }
                nav_fn(id, true, highlight);
            }));
        }
        {
            let ctrl_c = Rc::clone(&ctrl);
            let stack_c = stack.clone();
            let built_c = Rc::clone(&built_pages);
            let active_c = Rc::clone(&active_page_id);
            *ctrl.rebuild_page_cb.borrow_mut() = Some(Rc::new(move |raw_id| {
                let Some(cat) = find_category(raw_id) else {
                    return;
                };
                let id = cat.id;
                if let Some(old) = built_c.borrow_mut().remove(id) {
                    stack_c.remove(&old);
                }
                let fresh = pages::build_page(id, Rc::clone(&ctrl_c));
                stack_c.add_named(&fresh, Some(id));
                built_c.borrow_mut().insert(id.to_string(), fresh);
                if active_c.borrow().as_str() == id {
                    stack_c.set_visible_child_name(id);
                }
            }));
        }

        // Sidebar selection handler
        {
            let nav_fn = Rc::clone(&navigate_core);
            let suppress = Rc::clone(&suppress_nav_signal);
            let search_c = search.clone();
            nav_list.connect_row_selected(move |_, row| {
                if *suppress.borrow() {
                    return;
                }
                if let Some(row) = row {
                    let id = row.widget_name();
                    if !search_c.text().is_empty() {
                        search_c.set_text("");
                    }
                    nav_fn(id.as_str(), true, None);
                }
            });
        }

        // Pinned About NEON DE click handler
        {
            let nav_fn = Rc::clone(&navigate_core);
            let search_c = search.clone();
            about_footer_btn.connect_clicked(move |_| {
                if !search_c.text().is_empty() {
                    search_c.set_text("");
                }
                nav_fn(ABOUT_CATEGORY.id, true, None);
            });
        }

        // Back button handler
        {
            let history_c = Rc::clone(&history);
            let nav_fn = Rc::clone(&navigate_core);
            let back_c = back_btn.clone();
            let search_c = search.clone();
            back_btn.connect_clicked(move |_| {
                if !search_c.text().is_empty() {
                    search_c.set_text("");
                    return;
                }
                if let Some(prev) = history_c.borrow_mut().go_back() {
                    nav_fn(&prev, false, None);
                }
                back_c.set_sensitive(history_c.borrow().can_go_back());
            });
        }

        // Keyboard shortcuts: Ctrl+F (focus search), Escape (clear search), Alt+Left (Back)
        {
            let key_ctrl = EventControllerKey::new();
            let entry = search.clone();
            let history_c = Rc::clone(&history);
            let nav_fn = Rc::clone(&navigate_core);
            let back_c = back_btn.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, mods| {
                if mods.contains(gdk::ModifierType::CONTROL_MASK)
                    && matches!(key, gdk::Key::f | gdk::Key::F)
                {
                    entry.grab_focus();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Escape && !entry.text().is_empty() {
                    entry.set_text("");
                    return glib::Propagation::Stop;
                }
                if mods.contains(gdk::ModifierType::ALT_MASK) && key == gdk::Key::Left {
                    if let Some(prev) = history_c.borrow_mut().go_back() {
                        nav_fn(&prev, false, None);
                        back_c.set_sensitive(history_c.borrow().can_go_back());
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            });
            window.add_controller(key_ctrl);
        }

        // Global search (filters sidebar AND displays direct setting-level hits in main pane)
        {
            let nav_rows_c = Rc::clone(&nav_rows_rc);
            let sep_rows_c = sep_rows.clone();
            let stack_c = stack.clone();
            let title_c = page_title_label.clone();
            let results_col = search_results_col.clone();
            let active_c = Rc::clone(&active_page_id);
            let nav_fn = Rc::clone(&navigate_core);
            let search_entry_c = search.clone();

            search.connect_search_changed(move |entry| {
                let raw = entry.text();
                let q = raw.trim().to_ascii_lowercase();

                for (row, cat) in nav_rows_c.iter() {
                    let visible = q.is_empty()
                        || cat.title.to_ascii_lowercase().contains(&q)
                        || cat.description.to_ascii_lowercase().contains(&q)
                        || cat.keywords.contains(&q);
                    row.set_visible(visible);
                }
                for sep in &sep_rows_c {
                    sep.set_visible(q.is_empty());
                }

                // Clear previous search result widgets
                while let Some(child) = results_col.first_child() {
                    results_col.remove(&child);
                }

                if q.is_empty() {
                    let active = active_c.borrow().clone();
                    if let Some(cat) = find_category(&active) {
                        stack_c.set_visible_child_name(cat.id);
                        title_c.set_text(cat.title);
                    }
                    return;
                }

                title_c.set_text(&format!("Search: {}", raw.trim()));
                let hits = search_settings(&q);

                let heading = section_heading(&format!(
                    "{} matching setting{}",
                    hits.len(),
                    if hits.len() == 1 { "" } else { "s" }
                ));
                results_col.append(&heading);

                let card = boxed_card();
                if hits.is_empty() {
                    let empty = Label::new(Some(
                        "No matching settings found. Try keywords like blur, wi-fi, scaling, shortcuts, battery, or timezone.",
                    ));
                    empty.add_css_class("settings-row-desc");
                    empty.set_margin_top(16);
                    empty.set_margin_bottom(16);
                    empty.set_margin_start(16);
                    empty.set_margin_end(16);
                    empty.set_wrap(true);
                    card.append(&empty);
                } else {
                    for (idx, hit) in hits.iter().take(24).enumerate() {
                        if idx > 0 {
                            card.append(&row_separator());
                        }
                        let btn = Button::new();
                        btn.add_css_class("search-hit-btn");

                        let row = GtkBox::new(Orientation::Horizontal, 12);
                        row.set_valign(Align::Center);
                        let text = GtkBox::new(Orientation::Vertical, 2);
                        text.set_hexpand(true);

                        let title_row = GtkBox::new(Orientation::Horizontal, 8);
                        let name_lbl = Label::new(Some(hit.setting.name));
                        name_lbl.add_css_class("settings-row-title");
                        name_lbl.set_halign(Align::Start);
                        let cat_pill = Label::new(Some(hit.setting.category_title));
                        cat_pill.add_css_class("settings-badge");
                        title_row.append(&name_lbl);
                        title_row.append(&cat_pill);

                        let desc_lbl = Label::new(Some(&format!(
                            "{}  •  {}",
                            hit.setting.description, hit.setting.service
                        )));
                        desc_lbl.add_css_class("settings-row-desc");
                        desc_lbl.set_halign(Align::Start);
                        desc_lbl.set_wrap(true);

                        text.append(&title_row);
                        text.append(&desc_lbl);

                        row.append(&text);
                        row.append(&capability_badge(hit.setting.capability));
                        btn.set_child(Some(&row));

                        let nav_click = Rc::clone(&nav_fn);
                        let entry_click = search_entry_c.clone();
                        let target_cat = hit.setting.category_id;
                        let target_name = hit.setting.name;
                        btn.connect_clicked(move |_| {
                            entry_click.set_text("");
                            nav_click(target_cat, true, Some(target_name));
                        });
                        card.append(&btn);
                    }
                }
                results_col.append(&card);
                stack_c.set_visible_child_name("__search__");
            });
        }

        // Quick-menu actions
        {
            let ctrl_c = Rc::clone(&ctrl);
            let pop = popover.clone();
            toggle_theme_btn.connect_clicked(move |_| {
                pop.popdown();
                let next = {
                    let mut cfg = ctrl_c.config.borrow_mut();
                    let next = if cfg.appearance.theme == ThemeMode::NeonLight {
                        ThemeMode::NeonDark
                    } else {
                        ThemeMode::NeonLight
                    };
                    cfg.appearance.theme = next;
                    next
                };
                let req = build_appearance_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(
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
            let ctrl_c = Rc::clone(&ctrl);
            let pop = popover.clone();
            sync_btn.connect_clicked(move |_| {
                pop.popdown();
                ctrl_c.push_full_config_live();
                ctrl_c.set_status_ok("Synchronized full configuration to live compositor");
            });
        }
        {
            let nav_fn = Rc::clone(&navigate_core);
            let pop = popover.clone();
            shortcuts_menu_btn.connect_clicked(move |_| {
                pop.popdown();
                nav_fn("shortcuts", true, None);
            });
        }
        {
            let nav_fn = Rc::clone(&navigate_core);
            let pop = popover.clone();
            about_menu_btn.connect_clicked(move |_| {
                pop.popdown();
                nav_fn(ABOUT_CATEGORY.id, true, None);
            });
        }

        // Open initial page lazily
        navigate_core(&initial_page, true, None);

        split.append(&sidebar);
        split.append(&stack);
        outer.append(&split);
        outer.append(&status_bar);

        window.set_child(Some(&outer));
        window.present();
    });

    app.run_with_args::<&str>(&[]);
}
