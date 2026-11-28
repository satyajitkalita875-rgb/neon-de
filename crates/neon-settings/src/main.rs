//! Entrypoint for `neon-settings` — clean macOS System Settings–style GTK4 application
//! for NEON DE, exposing only genuine, live-working desktop settings with instant startup.

mod metadata;
mod navigation;
mod pages;
mod state;
mod system_info;
mod ui;

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

use clap::Parser;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, Image, Label,
    ListBox, ListBoxRow, Orientation, PolicyType, ScrolledWindow, SearchEntry, SelectionMode,
    Separator, Stack, StackTransitionType, WindowHandle, gdk, glib, prelude::*,
};
use neon_config::{WindowButtonLayout, default_config_path, load_or_default};
use neon_ipc::default_socket_path;

use crate::{
    metadata::search_settings,
    navigation::{ABOUT_CATEGORY, CategoryDef, NavigationHistory, SIDEBAR_CATEGORIES, find_category},
    state::{SettingsController, apply_system_cursor, apply_window_button_layout},
    ui::{
        boxed_card, row_separator, section_heading,
        style::SettingsTheme,
    },
};

#[derive(Debug, Parser)]
#[command(name = "neon-settings", version, about)]
struct Args {
    /// Open directly to a specific settings category (e.g. appearance, displays, network, sound).
    #[arg(long)]
    page: Option<String>,
    /// Override the NEON DE config file path.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override the neon-compositor IPC socket path.
    #[arg(long)]
    ipc_socket: Option<PathBuf>,
}

fn build_nav_badge(icon_name: &str, badge_class: &str) -> GtkBox {
    let badge = GtkBox::new(Orientation::Horizontal, 0);
    badge.add_css_class("nav-badge");
    badge.add_css_class(badge_class);
    badge.set_size_request(20, 20);
    badge.set_halign(Align::Start);
    badge.set_valign(Align::Center);
    badge.set_hexpand(false);
    badge.set_vexpand(false);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(12);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    icon.set_margin_start(4);
    icon.set_margin_end(4);
    badge.append(&icon);
    badge
}

fn build_window_controls(window: &ApplicationWindow, right_order: bool) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.set_valign(Align::Center);

    let close_btn = Button::new();
    close_btn.add_css_class("mac-dot");
    close_btn.add_css_class("mac-dot-close");
    close_btn.set_tooltip_text(Some("Close"));
    {
        let win = window.clone();
        close_btn.connect_clicked(move |_| win.close());
    }

    let min_btn = Button::new();
    min_btn.add_css_class("mac-dot");
    min_btn.add_css_class("mac-dot-min");
    min_btn.set_tooltip_text(Some("Minimize"));
    {
        let win = window.clone();
        min_btn.connect_clicked(move |_| win.minimize());
    }

    let max_btn = Button::new();
    max_btn.add_css_class("mac-dot");
    max_btn.add_css_class("mac-dot-max");
    max_btn.set_tooltip_text(Some("Maximize"));
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

    if right_order {
        row.add_css_class("settings-right-controls");
        row.append(&min_btn);
        row.append(&max_btn);
        row.append(&close_btn);
    } else {
        row.add_css_class("settings-sidebar-top");
        row.append(&close_btn);
        row.append(&min_btn);
        row.append(&max_btn);
    }
    row
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "neon_settings=info".into()),
        )
        .init();

    let args = Args::parse();
    let config_path = args.config.unwrap_or_else(default_config_path);
    let ipc_socket = args.ipc_socket.unwrap_or_else(|| {
        std::env::var_os("NEON_IPC_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let wd = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
                default_socket_path(&wd)
            })
    });
    let initial_page_arg = args.page;

    let app = Application::builder()
        .application_id("org.neonde.settings")
        .build();

    app.connect_activate(move |app| {
        if let Some(existing) = app.active_window() {
            existing.present();
            return;
        }

        let cfg = load_or_default(&config_path);
        let initial_page = initial_page_arg
            .as_deref()
            .and_then(find_category)
            .or_else(|| find_category(&cfg.session.last_settings_page))
            .map(|c| c.id.to_string())
            .unwrap_or_else(|| "appearance".to_string());

        let theme = SettingsTheme::install(cfg.appearance.theme, &cfg.appearance.accent_color);
        apply_system_cursor(&cfg.appearance.cursor_theme, cfg.appearance.cursor_size);
        apply_window_button_layout(cfg.windows.button_layout);

        let window = ApplicationWindow::builder()
            .application(app)
            .title("Settings")
            .default_width(820)
            .default_height(580)
            .decorated(false)
            .build();
        window.add_css_class("neon-settings-window");

        // Auto-hiding transient feedback toast (hidden by default — zero permanent green bar)
        let status_bar = GtkBox::new(Orientation::Horizontal, 8);
        status_bar.add_css_class("settings-status-bar");
        status_bar.set_visible(false);
        let status_label = Label::new(None);
        status_label.set_halign(Align::Start);
        status_label.set_hexpand(true);
        status_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        status_bar.append(&status_label);

        let ctrl = Rc::new(SettingsController::new(
            config_path.clone(),
            ipc_socket.clone(),
            cfg,
            theme,
            status_label.clone(),
            status_bar.clone(),
            window.clone(),
        ));

        // ── Unified macOS Split Window (Sidebar | Content Pane) ──────────────
        let split = GtkBox::new(Orientation::Horizontal, 0);
        split.set_hexpand(true);
        split.set_vexpand(true);

        // ── Left Sidebar (Traffic Lights + Search + Compact Colored Icon List) ─
        let sidebar = GtkBox::new(Orientation::Vertical, 0);
        sidebar.add_css_class("settings-sidebar");
        sidebar.set_width_request(222);
        // Explicitly stop hexpand propagation from row labels so the sidebar never steals width.
        sidebar.set_hexpand(false);

        // Draggable sidebar top with macOS traffic lights (shown when Left layout is active)
        let sidebar_handle = WindowHandle::new();
        let sidebar_top_box = GtkBox::new(Orientation::Horizontal, 0);
        sidebar_top_box.set_valign(Align::Center);
        let left_controls = build_window_controls(&window, false);
        let sidebar_top_spacer = GtkBox::new(Orientation::Horizontal, 0);
        sidebar_top_spacer.add_css_class("settings-sidebar-top-spacer");
        sidebar_top_box.append(&left_controls);
        sidebar_top_box.append(&sidebar_top_spacer);
        sidebar_handle.set_child(Some(&sidebar_top_box));
        sidebar.append(&sidebar_handle);

        // Compact macOS Search Field directly under traffic lights
        let search = SearchEntry::new();
        search.add_css_class("settings-search");
        search.set_placeholder_text(Some("Search"));
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

            let row_box = GtkBox::new(Orientation::Horizontal, 9);
            row_box.set_valign(Align::Center);
            row_box.set_halign(Align::Fill);
            let badge = build_nav_badge(cat.icon, cat.badge_class);
            let lbl = Label::new(Some(cat.title));
            lbl.set_halign(Align::Start);
            lbl.set_hexpand(true);
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row_box.append(&badge);
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

        // Pinned "About NEON DE" button at sidebar bottom
        let about_sep = Separator::new(Orientation::Horizontal);
        about_sep.add_css_class("settings-sidebar-sep");
        sidebar.append(&about_sep);

        let about_footer_btn = Button::new();
        about_footer_btn.add_css_class("settings-about-footer-btn");
        about_footer_btn.set_vexpand(false);
        about_footer_btn.set_valign(Align::End);
        let about_row = GtkBox::new(Orientation::Horizontal, 9);
        about_row.set_valign(Align::Center);
        about_row.set_halign(Align::Fill);
        let about_badge = build_nav_badge(ABOUT_CATEGORY.icon, ABOUT_CATEGORY.badge_class);
        let about_lbl = Label::new(Some(ABOUT_CATEGORY.title));
        about_lbl.set_halign(Align::Start);
        about_lbl.set_hexpand(true);
        about_row.append(&about_badge);
        about_row.append(&about_lbl);
        about_footer_btn.set_child(Some(&about_row));
        sidebar.append(&about_footer_btn);

        // ── Right Content Pane (< > Title Header + Stack + Auto-Hiding Toast) ─
        let content_pane = GtkBox::new(Orientation::Vertical, 0);
        content_pane.set_hexpand(true);
        content_pane.set_vexpand(true);

        let content_handle = WindowHandle::new();
        let content_header = GtkBox::new(Orientation::Horizontal, 2);
        content_header.add_css_class("settings-content-header");
        content_header.set_valign(Align::Center);

        let back_btn = Button::from_icon_name("go-previous-symbolic");
        back_btn.add_css_class("nav-chevron-btn");
        back_btn.set_sensitive(false);
        back_btn.set_tooltip_text(Some("Back (Alt+Left)"));

        let fwd_btn = Button::from_icon_name("go-next-symbolic");
        fwd_btn.add_css_class("nav-chevron-btn");
        fwd_btn.set_sensitive(false);
        fwd_btn.set_tooltip_text(Some("Forward (Alt+Right)"));

        let page_title_label = Label::new(Some("Appearance"));
        page_title_label.add_css_class("settings-header-title");
        page_title_label.set_halign(Align::Start);
        page_title_label.set_hexpand(true);

        let right_controls = build_window_controls(&window, true);

        content_header.append(&back_btn);
        content_header.append(&fwd_btn);
        content_header.append(&page_title_label);
        content_header.append(&right_controls);
        content_handle.set_child(Some(&content_header));
        content_pane.append(&content_handle);

        let apply_header_button_layout = {
            let left_c = left_controls.clone();
            let spacer_c = sidebar_top_spacer.clone();
            let right_c = right_controls.clone();
            Rc::new(move |layout: WindowButtonLayout| {
                let is_right = layout == WindowButtonLayout::RightStandard;
                left_c.set_visible(!is_right);
                spacer_c.set_visible(is_right);
                right_c.set_visible(is_right);
            })
        };
        apply_header_button_layout(ctrl.config.borrow().windows.button_layout);
        *ctrl.button_layout_cb.borrow_mut() = Some(apply_header_button_layout);

        let stack = Stack::new();
        stack.set_transition_type(StackTransitionType::Crossfade);
        stack.set_transition_duration(210);
        stack.set_hexpand(true);
        stack.set_vexpand(true);

        // Search results page
        let search_results_outer = GtkBox::new(Orientation::Vertical, 0);
        search_results_outer.set_hexpand(true);
        let search_results_col = GtkBox::new(Orientation::Vertical, 4);
        search_results_col.add_css_class("settings-content-clamp");
        search_results_col.set_hexpand(true);
        search_results_outer.append(&search_results_col);
        let search_results_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&search_results_outer)
            .build();
        stack.add_named(&search_results_scroll, Some("__search__"));

        content_pane.append(&stack);
        content_pane.append(&status_bar);

        let built_pages: Rc<RefCell<HashMap<String, ScrolledWindow>>> =
            Rc::new(RefCell::new(HashMap::new()));
        let history = Rc::new(RefCell::new(NavigationHistory::default()));
        let active_page_id = Rc::new(RefCell::new(String::new()));
        let suppress_nav_signal = Rc::new(RefCell::new(false));
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
            let fwd_btn = fwd_btn.clone();
            let active_id = Rc::clone(&active_page_id);
            let suppress = Rc::clone(&suppress_nav_signal);

            Rc::new(move |raw_id: &str, record_history: bool, highlight: Option<&str>| {
                let Some(cat) = find_category(raw_id) else {
                    return;
                };
                let id = cat.id;

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
                fwd_btn.set_sensitive(history.borrow().can_go_forward());

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

                if ctrl.config.borrow().session.last_settings_page != id {
                    ctrl.config.borrow_mut().session.last_settings_page = id.to_string();
                    ctrl.persist_silent();
                }

                if let Some(setting_name) = highlight {
                    ctrl.set_status_info(&format!("{} → {setting_name}", cat.title));
                }
            })
        };

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
                let ctrl_idle = Rc::clone(&ctrl_c);
                let stack_idle = stack_c.clone();
                let built_idle = Rc::clone(&built_c);
                let active_idle = Rc::clone(&active_c);
                glib::idle_add_local_once(move || {
                    let prev_tr = stack_idle.transition_type();
                    stack_idle.set_transition_type(StackTransitionType::None);
                    if let Some(old) = built_idle.borrow_mut().remove(id) {
                        stack_idle.remove(&old);
                    }
                    let fresh = pages::build_page(id, Rc::clone(&ctrl_idle));
                    stack_idle.add_named(&fresh, Some(id));
                    built_idle.borrow_mut().insert(id.to_string(), fresh);
                    if active_idle.borrow().as_str() == id {
                        stack_idle.set_visible_child_name(id);
                    }
                    stack_idle.set_transition_type(prev_tr);
                });
            }));
        }

        // Sidebar selection
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

        // About footer click
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

        // Back & Forward buttons
        {
            let history_c = Rc::clone(&history);
            let nav_fn = Rc::clone(&navigate_core);
            let back_c = back_btn.clone();
            let fwd_c = fwd_btn.clone();
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
                fwd_c.set_sensitive(history_c.borrow().can_go_forward());
            });
        }
        {
            let history_c = Rc::clone(&history);
            let nav_fn = Rc::clone(&navigate_core);
            let back_c = back_btn.clone();
            let fwd_c = fwd_btn.clone();
            let search_c = search.clone();
            fwd_btn.connect_clicked(move |_| {
                if !search_c.text().is_empty() {
                    search_c.set_text("");
                    return;
                }
                if let Some(next) = history_c.borrow_mut().go_forward() {
                    nav_fn(&next, false, None);
                }
                back_c.set_sensitive(history_c.borrow().can_go_back());
                fwd_c.set_sensitive(history_c.borrow().can_go_forward());
            });
        }

        // Keyboard shortcuts: Ctrl+F, Escape, Alt+Left, Alt+Right
        {
            let key_ctrl = EventControllerKey::new();
            let entry = search.clone();
            let history_c = Rc::clone(&history);
            let nav_fn = Rc::clone(&navigate_core);
            let back_c = back_btn.clone();
            let fwd_c = fwd_btn.clone();
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
                        fwd_c.set_sensitive(history_c.borrow().can_go_forward());
                        return glib::Propagation::Stop;
                    }
                }
                if mods.contains(gdk::ModifierType::ALT_MASK) && key == gdk::Key::Right {
                    if let Some(next) = history_c.borrow_mut().go_forward() {
                        nav_fn(&next, false, None);
                        back_c.set_sensitive(history_c.borrow().can_go_back());
                        fwd_c.set_sensitive(history_c.borrow().can_go_forward());
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            });
            window.add_controller(key_ctrl);
        }

        // Global Search
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
                        "No matching settings found. Try wifi, wallpaper, blur, brightness, sound, or battery.",
                    ));
                    empty.add_css_class("settings-row-desc");
                    empty.set_margin_top(14);
                    empty.set_margin_bottom(14);
                    empty.set_margin_start(14);
                    empty.set_margin_end(14);
                    empty.set_wrap(true);
                    card.append(&empty);
                } else {
                    for (idx, hit) in hits.iter().take(20).enumerate() {
                        if idx > 0 {
                            card.append(&row_separator());
                        }
                        let btn = Button::new();
                        btn.add_css_class("search-hit-btn");

                        let row = GtkBox::new(Orientation::Horizontal, 10);
                        row.set_valign(Align::Center);
                        let text = GtkBox::new(Orientation::Vertical, 2);
                        text.set_hexpand(true);

                        let name_lbl = Label::new(Some(hit.setting.name));
                        name_lbl.add_css_class("settings-row-title");
                        name_lbl.set_halign(Align::Start);

                        let desc_lbl = Label::new(Some(hit.setting.description));
                        desc_lbl.add_css_class("settings-row-desc");
                        desc_lbl.set_halign(Align::Start);
                        desc_lbl.set_wrap(true);

                        text.append(&name_lbl);
                        text.append(&desc_lbl);

                        let cat_pill = Label::new(Some(hit.setting.category_title));
                        cat_pill.add_css_class("settings-badge");
                        cat_pill.set_valign(Align::Center);

                        row.append(&text);
                        row.append(&cat_pill);
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

        split.append(&sidebar);
        split.append(&content_pane);
        window.set_child(Some(&split));

        // Present the window IMMEDIATELY, then build the initial page on the first frame
        // so opening Settings from the Dock or App Drawer feels instantaneous!
        window.present();
        let nav_init = Rc::clone(&navigate_core);
        glib::idle_add_local_once(move || {
            nav_init(&initial_page, true, None);
        });
    });

    app.run_with_args::<&str>(&[]);
}
