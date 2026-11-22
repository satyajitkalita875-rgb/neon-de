//! NEON DE Activities Overview, Workspace Previews, and XDG Application Grid
//! (`zwlr_layer_shell_v1` `Layer::Overlay`).

use std::{cell::RefCell, rc::Rc};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, FlowBox,
    Image, Label, Orientation, PolicyType, ScrolledWindow, SearchEntry, SelectionMode, gdk,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::{desktop_apps::DesktopApp, ipc_bridge::IpcCommandSender};

const CATEGORIES: &[&str] = &[
    "All",
    "Utilities",
    "Development",
    "Internet",
    "System",
    "Graphics",
    "Office",
];

/// Fullscreen Activities Overview overlay combining search, workspace previews, open windows, and app grid.
pub struct Overview {
    pub window: ApplicationWindow,
    search_entry: SearchEntry,
    workspaces_box: GtkBox,
    windows_flow: FlowBox,
    apps_flow: FlowBox,
    all_apps: Rc<Vec<DesktopApp>>,
    selected_category: Rc<RefCell<String>>,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    ipc: IpcCommandSender,
}

impl Overview {
    pub fn build(app: &Application, apps: Vec<DesktopApp>, ipc: IpcCommandSender) -> Rc<Self> {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Activities Overview")
            .decorated(false)
            .build();
        window.add_css_class("neon-overview-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-overview"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let root = GtkBox::new(Orientation::Vertical, 18);
        root.add_css_class("neon-overview-scrim");

        // Top search bar
        let header_row = GtkBox::new(Orientation::Horizontal, 12);
        header_row.set_halign(Align::Center);
        let search_entry = SearchEntry::new();
        search_entry.add_css_class("neon-search-entry");
        search_entry.set_placeholder_text(Some("Search windows and applications..."));

        let close_btn = Button::with_label("Close (Esc)");
        close_btn.add_css_class("neon-pill-button");
        {
            let ipc = ipc.clone();
            close_btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::SetOverview { open: false });
            });
        }
        header_row.append(&search_entry);
        header_row.append(&close_btn);
        root.append(&header_row);

        // Workspace previews strip
        let ws_heading = Label::new(Some("WORKSPACES"));
        ws_heading.add_css_class("neon-section-heading");
        ws_heading.set_halign(Align::Start);
        root.append(&ws_heading);

        let workspaces_box = GtkBox::new(Orientation::Horizontal, 12);
        root.append(&workspaces_box);

        // Open windows preview grid
        let win_heading = Label::new(Some("OPEN WINDOWS"));
        win_heading.add_css_class("neon-section-heading");
        win_heading.set_halign(Align::Start);
        root.append(&win_heading);

        let windows_flow = FlowBox::new();
        windows_flow.set_selection_mode(SelectionMode::None);
        windows_flow.set_column_spacing(12);
        windows_flow.set_row_spacing(12);
        windows_flow.set_homogeneous(false);
        root.append(&windows_flow);

        // Application grid + category pills
        let apps_header = GtkBox::new(Orientation::Horizontal, 8);
        let apps_heading = Label::new(Some("APPLICATIONS"));
        apps_heading.add_css_class("neon-section-heading");
        apps_header.append(&apps_heading);

        let categories_box = GtkBox::new(Orientation::Horizontal, 6);
        categories_box.set_margin_start(12);
        apps_header.append(&categories_box);
        root.append(&apps_header);

        let apps_flow = FlowBox::new();
        apps_flow.set_selection_mode(SelectionMode::None);
        apps_flow.set_column_spacing(12);
        apps_flow.set_row_spacing(12);
        apps_flow.set_max_children_per_line(7);
        apps_flow.set_homogeneous(true);

        let scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .child(&apps_flow)
            .build();
        root.append(&scroller);

        window.set_child(Some(&root));

        let overview = Rc::new(Self {
            window,
            search_entry,
            workspaces_box,
            windows_flow,
            apps_flow,
            all_apps: Rc::new(apps),
            selected_category: Rc::new(RefCell::new("All".to_string())),
            last_snapshot: Rc::new(RefCell::new(None)),
            ipc: ipc.clone(),
        });

        // Escape key dismisses overview cleanly
        let key_ctrl = EventControllerKey::new();
        {
            let ipc = ipc.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    ipc.send(IpcRequest::SetOverview { open: false });
                    return gtk4::glib::Propagation::Stop;
                }
                gtk4::glib::Propagation::Proceed
            });
        }
        overview.window.add_controller(key_ctrl);

        // Category filter buttons
        for &cat in CATEGORIES {
            let btn = Button::with_label(cat);
            btn.add_css_class("neon-pill-button");
            let ov = Rc::clone(&overview);
            let cat_str = cat.to_string();
            btn.connect_clicked(move |_| {
                *ov.selected_category.borrow_mut() = cat_str.clone();
                ov.rebuild_filtered_content();
            });
            categories_box.append(&btn);
        }

        // Live search filtering
        {
            let ov = Rc::clone(&overview);
            overview.search_entry.connect_search_changed(move |_| {
                ov.rebuild_filtered_content();
            });
        }

        overview.rebuild_filtered_content();
        overview.window.set_visible(false);
        overview
    }

    pub fn set_open(&self, open: bool) {
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(if open {
                KeyboardMode::Exclusive
            } else {
                KeyboardMode::None
            });
        }
        self.window.set_visible(open);
        if open {
            self.search_entry.set_text("");
            self.search_entry.grab_focus();
            self.rebuild_filtered_content();
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
        if self.window.is_visible() != snapshot.overview_open {
            self.set_open(snapshot.overview_open);
        }
        self.rebuild_workspaces(snapshot);
        self.rebuild_filtered_content();
    }

    fn rebuild_workspaces(&self, snapshot: &DesktopSnapshot) {
        while let Some(child) = self.workspaces_box.first_child() {
            self.workspaces_box.remove(&child);
        }

        for ws in &snapshot.workspaces {
            let card = Button::new();
            card.add_css_class("neon-workspace-card");
            if ws.active {
                card.add_css_class("active");
            }

            let vbox = GtkBox::new(Orientation::Vertical, 4);
            let title = Label::new(Some(&format!("Workspace {}", ws.index + 1)));
            title.set_halign(Align::Start);
            let count_text = match ws.window_ids.len() {
                0 => "Empty".to_string(),
                1 => "1 window".to_string(),
                n => format!("{n} windows"),
            };
            let subtitle = Label::new(Some(&count_text));
            subtitle.add_css_class("neon-focused-title");
            subtitle.set_halign(Align::Start);

            vbox.append(&title);
            vbox.append(&subtitle);
            card.set_child(Some(&vbox));

            let idx = ws.index;
            let ipc = self.ipc.clone();
            card.connect_clicked(move |_| {
                ipc.send(IpcRequest::SwitchWorkspace { index: idx });
            });
            self.workspaces_box.append(&card);
        }
    }

    fn rebuild_filtered_content(&self) {
        let query = self.search_entry.text().to_string();
        let category = self.selected_category.borrow().clone();

        // 1. Rebuild open windows grid filtered by query
        while let Some(child) = self.windows_flow.first_child() {
            self.windows_flow.remove(&child);
        }
        if let Some(snapshot) = self.last_snapshot.borrow().as_ref() {
            let q = query.trim().to_lowercase();
            for win in &snapshot.windows {
                let title = win
                    .title
                    .as_deref()
                    .or(win.app_id.as_deref())
                    .unwrap_or("Untitled Window");
                let app_id = win.app_id.as_deref().unwrap_or("");
                if !q.is_empty()
                    && !title.to_lowercase().contains(&q)
                    && !app_id.to_lowercase().contains(&q)
                {
                    continue;
                }

                let card = GtkBox::new(Orientation::Vertical, 6);
                card.add_css_class("neon-window-card");
                if win.focused {
                    card.add_css_class("focused");
                }

                let top_row = GtkBox::new(Orientation::Horizontal, 6);
                let title_lbl = Label::new(Some(title));
                title_lbl.set_hexpand(true);
                title_lbl.set_halign(Align::Start);
                title_lbl.set_max_width_chars(24);
                title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

                let close_btn = Button::with_label("×");
                close_btn.add_css_class("neon-close-button");
                {
                    let win_id = win.id;
                    let ipc = self.ipc.clone();
                    close_btn.connect_clicked(move |_| {
                        ipc.send(IpcRequest::CloseWindow { window_id: win_id });
                    });
                }
                top_row.append(&title_lbl);
                top_row.append(&close_btn);

                let focus_btn = Button::with_label(if win.minimized {
                    "Restore & Focus"
                } else {
                    "Switch to Window"
                });
                focus_btn.add_css_class("neon-pill-button");
                {
                    let win_id = win.id;
                    let ipc = self.ipc.clone();
                    focus_btn.connect_clicked(move |_| {
                        ipc.send(IpcRequest::FocusWindow { window_id: win_id });
                    });
                }

                card.append(&top_row);
                card.append(&focus_btn);
                self.windows_flow.insert(&card, -1);
            }
        }

        // 2. Rebuild installed XDG applications grid filtered by query & category
        while let Some(child) = self.apps_flow.first_child() {
            self.apps_flow.remove(&child);
        }

        let mut shown = 0usize;
        for app in self.all_apps.iter() {
            if !app.matches_category(&category) || !app.matches_query(&query) {
                continue;
            }
            let tile = Button::new();
            tile.add_css_class("neon-app-tile");
            if let Some(comment) = &app.comment {
                tile.set_tooltip_text(Some(comment));
            }

            let vbox = GtkBox::new(Orientation::Vertical, 6);
            vbox.set_halign(Align::Center);
            vbox.set_valign(Align::Center);

            if let Some(icon_name) = &app.icon {
                let icon = Image::from_icon_name(icon_name);
                icon.set_pixel_size(32);
                vbox.append(&icon);
            }

            let name_lbl = Label::new(Some(&app.name));
            name_lbl.set_max_width_chars(14);
            name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            vbox.append(&name_lbl);

            tile.set_child(Some(&vbox));

            let app_clone = app.clone();
            let ipc = self.ipc.clone();
            tile.connect_clicked(move |_| {
                app_clone.launch();
                ipc.send(IpcRequest::SetOverview { open: false });
            });

            self.apps_flow.insert(&tile, -1);
            shown += 1;
            if shown >= 42 {
                break;
            }
        }
    }
}
