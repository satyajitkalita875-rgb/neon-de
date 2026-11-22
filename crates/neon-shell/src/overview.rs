//! NEON DE macOS Launchpad & Mission Control (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Default view is a clean, spacious macOS Launchpad: centered frosted search box,
//! 7×5 grid of crisp 64px app icons, bottom pagination dots, Enter-to-launch, and
//! click-anywhere-empty or Esc to dismiss. Includes a clean Mission Control tab for
//! switching workspaces and open windows.

use std::{cell::RefCell, rc::Rc};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, FlowBox,
    GestureClick, Grid, Image, Label, Orientation, SearchEntry, SelectionMode, Stack,
    StackTransitionType, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::{desktop_apps::DesktopApp, ipc_bridge::IpcCommandSender};

const LAUNCHPAD_COLS: usize = 7;
const LAUNCHPAD_ROWS: usize = 5;
const APPS_PER_PAGE: usize = LAUNCHPAD_COLS * LAUNCHPAD_ROWS;

/// Fullscreen macOS Launchpad + Mission Control overlay.
pub struct Overview {
    pub window: ApplicationWindow,
    mode_stack: Stack,
    launchpad_tab_btn: Button,
    mission_tab_btn: Button,
    search_entry: SearchEntry,
    apps_grid: Grid,
    dots_box: GtkBox,
    workspaces_box: GtkBox,
    windows_flow: FlowBox,
    all_apps: Rc<Vec<DesktopApp>>,
    current_page: Rc<RefCell<usize>>,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    ipc: IpcCommandSender,
}

impl Overview {
    pub fn build(app: &Application, apps: Vec<DesktopApp>, ipc: IpcCommandSender) -> Rc<Self> {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Launchpad")
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
        root.add_css_class("neon-launchpad-scrim");

        // Top centered macOS Launchpad search box + minimalist segmented tab switcher
        let top_area = GtkBox::new(Orientation::Vertical, 12);
        top_area.set_halign(Align::Center);
        top_area.set_margin_top(14);

        let search_entry = SearchEntry::new();
        search_entry.add_css_class("neon-launchpad-search");
        search_entry.set_placeholder_text(Some("Search"));
        search_entry.set_halign(Align::Center);
        top_area.append(&search_entry);

        let mode_bar = GtkBox::new(Orientation::Horizontal, 4);
        mode_bar.add_css_class("neon-segment-track");
        mode_bar.set_halign(Align::Center);

        let launchpad_tab_btn = Button::with_label("Launchpad");
        launchpad_tab_btn.add_css_class("neon-segment-btn");
        launchpad_tab_btn.add_css_class("active");

        let mission_tab_btn = Button::with_label("Mission Control");
        mission_tab_btn.add_css_class("neon-segment-btn");

        mode_bar.append(&launchpad_tab_btn);
        mode_bar.append(&mission_tab_btn);
        top_area.append(&mode_bar);

        root.append(&top_area);

        // Main stack switching cleanly between Launchpad (default) and Mission Control
        let mode_stack = Stack::new();
        mode_stack.set_vexpand(true);
        mode_stack.set_hexpand(true);
        mode_stack.set_transition_type(StackTransitionType::Crossfade);
        mode_stack.set_transition_duration(140);

        // === Page 1: macOS Launchpad ===
        let launchpad_page = GtkBox::new(Orientation::Vertical, 16);
        launchpad_page.set_vexpand(true);
        launchpad_page.set_hexpand(true);

        let grid_holder = GtkBox::new(Orientation::Vertical, 0);
        grid_holder.set_vexpand(true);
        grid_holder.set_valign(Align::Center);
        grid_holder.set_halign(Align::Center);

        let apps_grid = Grid::new();
        apps_grid.set_column_spacing(26);
        apps_grid.set_row_spacing(20);
        apps_grid.set_column_homogeneous(true);
        apps_grid.set_row_homogeneous(true);
        grid_holder.append(&apps_grid);
        launchpad_page.append(&grid_holder);

        let dots_box = GtkBox::new(Orientation::Horizontal, 10);
        dots_box.set_halign(Align::Center);
        dots_box.set_margin_bottom(24);
        launchpad_page.append(&dots_box);

        mode_stack.add_named(&launchpad_page, Some("launchpad"));

        // === Page 2: macOS Mission Control ===
        let mission_page = GtkBox::new(Orientation::Vertical, 22);
        mission_page.set_vexpand(true);
        mission_page.set_hexpand(true);
        mission_page.set_margin_top(10);
        mission_page.set_margin_start(48);
        mission_page.set_margin_end(48);

        let workspaces_box = GtkBox::new(Orientation::Horizontal, 16);
        workspaces_box.set_halign(Align::Center);
        mission_page.append(&workspaces_box);

        let windows_flow = FlowBox::new();
        windows_flow.set_selection_mode(SelectionMode::None);
        windows_flow.set_column_spacing(18);
        windows_flow.set_row_spacing(18);
        windows_flow.set_halign(Align::Center);
        windows_flow.set_valign(Align::Center);
        windows_flow.set_vexpand(true);
        mission_page.append(&windows_flow);

        mode_stack.add_named(&mission_page, Some("mission"));
        mode_stack.set_visible_child_name("launchpad");

        root.append(&mode_stack);
        window.set_child(Some(&root));

        let overview = Rc::new(Self {
            window,
            mode_stack,
            launchpad_tab_btn,
            mission_tab_btn,
            search_entry,
            apps_grid,
            dots_box,
            workspaces_box,
            windows_flow,
            all_apps: Rc::new(apps),
            current_page: Rc::new(RefCell::new(0)),
            last_snapshot: Rc::new(RefCell::new(None)),
            ipc: ipc.clone(),
        });

        // Clicking empty backdrop dismisses Launchpad immediately (macOS behavior)
        let bg_click = GestureClick::new();
        {
            let ipc = ipc.clone();
            let search = overview.search_entry.clone();
            let grid = overview.apps_grid.clone();
            bg_click.connect_released(move |gesture, _, x, y| {
                if let Some(widget) = gesture.widget()
                    && let Some(picked) = widget.pick(x, y, gtk4::PickFlags::DEFAULT)
                {
                    let on_interactive = picked.is_ancestor(&search)
                        || picked == search.clone().upcast::<gtk4::Widget>()
                        || picked.ancestor(Button::static_type()).is_some()
                        || (picked.is_ancestor(&grid)
                            && picked.ancestor(Button::static_type()).is_some());
                    if !on_interactive {
                        ipc.send(IpcRequest::SetOverview { open: false });
                    }
                }
            });
        }
        root.add_controller(bg_click);

        // Mode switcher tabs
        {
            let ov = Rc::clone(&overview);
            overview.launchpad_tab_btn.connect_clicked(move |_| {
                ov.set_mode("launchpad");
            });
        }
        {
            let ov = Rc::clone(&overview);
            overview.mission_tab_btn.connect_clicked(move |_| {
                ov.set_mode("mission");
            });
        }

        // Keyboard navigation: Esc closes, Left/Right flips Launchpad pages, Enter launches top match
        let key_ctrl = EventControllerKey::new();
        {
            let ov = Rc::clone(&overview);
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    ov.ipc.send(IpcRequest::SetOverview { open: false });
                    return glib::Propagation::Stop;
                }
                if ov.search_entry.text().is_empty() {
                    if key == gdk::Key::Right {
                        ov.step_page(1);
                        return glib::Propagation::Stop;
                    }
                    if key == gdk::Key::Left {
                        ov.step_page(-1);
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            });
        }
        overview.window.add_controller(key_ctrl);

        // Enter in search launches first matching app immediately
        {
            let ov = Rc::clone(&overview);
            overview.search_entry.connect_activate(move |_| {
                let matches = ov.filtered_apps();
                if let Some(first) = matches.first() {
                    first.launch();
                    ov.ipc.send(IpcRequest::SetOverview { open: false });
                }
            });
        }

        // Live search resets to page 0 and filters smoothly
        {
            let ov = Rc::clone(&overview);
            overview.search_entry.connect_search_changed(move |_| {
                *ov.current_page.borrow_mut() = 0;
                ov.rebuild_launchpad();
            });
        }

        overview.rebuild_launchpad();
        overview.window.set_visible(false);
        overview
    }

    pub fn set_mode(&self, mode: &str) {
        self.mode_stack.set_visible_child_name(mode);
        if mode == "mission" {
            self.mission_tab_btn.add_css_class("active");
            self.launchpad_tab_btn.remove_css_class("active");
        } else {
            self.launchpad_tab_btn.add_css_class("active");
            self.mission_tab_btn.remove_css_class("active");
        }
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
            self.set_mode("launchpad");
            *self.current_page.borrow_mut() = 0;
            self.search_entry.set_text("");
            self.rebuild_launchpad();
            self.window.present();
            self.search_entry.grab_focus();
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
        if self.window.is_visible() != snapshot.overview_open {
            self.set_open(snapshot.overview_open);
        }
        self.rebuild_mission_control(snapshot);
    }

    fn filtered_apps(&self) -> Vec<DesktopApp> {
        let query = self.search_entry.text().to_string();
        self.all_apps
            .iter()
            .filter(|a| a.matches_query(&query))
            .cloned()
            .collect()
    }

    fn step_page(&self, delta: isize) {
        let total = self.filtered_apps().len();
        let page_count = total.div_ceil(APPS_PER_PAGE).max(1);
        let cur = *self.current_page.borrow() as isize;
        let next = (cur + delta).clamp(0, (page_count - 1) as isize) as usize;
        if next != *self.current_page.borrow() {
            *self.current_page.borrow_mut() = next;
            self.rebuild_launchpad();
        }
    }

    fn rebuild_launchpad(&self) {
        while let Some(child) = self.apps_grid.first_child() {
            self.apps_grid.remove(&child);
        }
        while let Some(child) = self.dots_box.first_child() {
            self.dots_box.remove(&child);
        }

        let matches = self.filtered_apps();
        let page_count = matches.len().div_ceil(APPS_PER_PAGE).max(1);
        let mut page = *self.current_page.borrow();
        if page >= page_count {
            page = 0;
            *self.current_page.borrow_mut() = 0;
        }

        let start = page * APPS_PER_PAGE;
        let page_slice = matches.iter().skip(start).take(APPS_PER_PAGE);

        for (idx, app) in page_slice.enumerate() {
            let col = (idx % LAUNCHPAD_COLS) as i32;
            let row = (idx / LAUNCHPAD_COLS) as i32;

            let tile = Button::new();
            tile.add_css_class("neon-launchpad-tile");
            if let Some(comment) = &app.comment {
                tile.set_tooltip_text(Some(comment));
            }

            let vbox = GtkBox::new(Orientation::Vertical, 8);
            vbox.set_halign(Align::Center);
            vbox.set_valign(Align::Center);

            let icon_name = app.icon.as_deref().unwrap_or("application-x-executable");
            let icon = Image::from_icon_name(icon_name);
            icon.set_pixel_size(64);
            vbox.append(&icon);

            let name_lbl = Label::new(Some(&app.name));
            name_lbl.add_css_class("neon-launchpad-label");
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

            self.apps_grid.attach(&tile, col, row, 1, 1);
        }

        // macOS Launchpad bottom pagination dots
        if page_count > 1 {
            for p in 0..page_count {
                let dot = Button::new();
                dot.add_css_class("neon-page-dot");
                if p == page {
                    dot.add_css_class("active");
                }
                dot.set_tooltip_text(Some(&format!("Page {}", p + 1)));
                let page_ref = Rc::clone(&self.current_page);
                let grid_ref = self.apps_grid.clone();
                let dots_ref = self.dots_box.clone();
                let apps_ref = Rc::clone(&self.all_apps);
                let search_ref = self.search_entry.clone();
                let ipc_ref = self.ipc.clone();
                dot.connect_clicked(move |_| {
                    *page_ref.borrow_mut() = p;
                    rebuild_launchpad_page(
                        &grid_ref,
                        &dots_ref,
                        &apps_ref,
                        &search_ref,
                        &page_ref,
                        &ipc_ref,
                    );
                });
                self.dots_box.append(&dot);
            }
        }
    }

    fn rebuild_mission_control(&self, snapshot: &DesktopSnapshot) {
        while let Some(child) = self.workspaces_box.first_child() {
            self.workspaces_box.remove(&child);
        }
        while let Some(child) = self.windows_flow.first_child() {
            self.windows_flow.remove(&child);
        }

        for ws in &snapshot.workspaces {
            let card = Button::new();
            card.add_css_class("neon-workspace-card");
            if ws.active {
                card.add_css_class("active");
            }

            let vbox = GtkBox::new(Orientation::Vertical, 4);
            vbox.set_valign(Align::Center);
            let title = Label::new(Some(&format!("Desktop {}", ws.index + 1)));
            title.add_css_class("neon-qs-title");
            let count_txt = match ws.window_ids.len() {
                0 => "Empty".to_string(),
                1 => "1 window".to_string(),
                n => format!("{n} windows"),
            };
            let count_lbl = Label::new(Some(&count_txt));
            count_lbl.add_css_class("neon-qs-muted");
            vbox.append(&title);
            vbox.append(&count_lbl);
            card.set_child(Some(&vbox));

            let idx = ws.index;
            let ipc = self.ipc.clone();
            card.connect_clicked(move |_| {
                ipc.send(IpcRequest::SwitchWorkspace { index: idx });
                ipc.send(IpcRequest::SetOverview { open: false });
            });
            self.workspaces_box.append(&card);
        }

        if snapshot.windows.is_empty() {
            let empty = Label::new(Some("No open windows"));
            empty.add_css_class("neon-qs-muted");
            self.windows_flow.insert(&empty, -1);
            return;
        }

        for win in &snapshot.windows {
            let title = win
                .title
                .as_deref()
                .or(win.app_id.as_deref())
                .unwrap_or("Untitled Window");

            let card = GtkBox::new(Orientation::Vertical, 10);
            card.add_css_class("neon-window-card");
            if win.focused {
                card.add_css_class("focused");
            }

            let top_row = GtkBox::new(Orientation::Horizontal, 8);
            let close_btn = Button::with_label("×");
            close_btn.add_css_class("neon-close-button");
            {
                let win_id = win.id;
                let ipc = self.ipc.clone();
                close_btn.connect_clicked(move |_| {
                    ipc.send(IpcRequest::CloseWindow { window_id: win_id });
                });
            }

            let title_lbl = Label::new(Some(title));
            title_lbl.add_css_class("neon-qs-title");
            title_lbl.set_hexpand(true);
            title_lbl.set_halign(Align::Start);
            title_lbl.set_max_width_chars(24);
            title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

            top_row.append(&close_btn);
            top_row.append(&title_lbl);

            let focus_btn = Button::with_label(if win.minimized {
                "Restore Window"
            } else {
                "Focus Window"
            });
            focus_btn.add_css_class("neon-pill-button");
            {
                let win_id = win.id;
                let ipc = self.ipc.clone();
                focus_btn.connect_clicked(move |_| {
                    ipc.send(IpcRequest::FocusWindow { window_id: win_id });
                    ipc.send(IpcRequest::SetOverview { open: false });
                });
            }

            card.append(&top_row);
            card.append(&focus_btn);
            self.windows_flow.insert(&card, -1);
        }
    }
}

fn rebuild_launchpad_page(
    apps_grid: &Grid,
    dots_box: &GtkBox,
    all_apps: &Rc<Vec<DesktopApp>>,
    search_entry: &SearchEntry,
    current_page: &Rc<RefCell<usize>>,
    ipc: &IpcCommandSender,
) {
    while let Some(child) = apps_grid.first_child() {
        apps_grid.remove(&child);
    }
    while let Some(child) = dots_box.first_child() {
        dots_box.remove(&child);
    }

    let query = search_entry.text().to_string();
    let matches: Vec<DesktopApp> = all_apps
        .iter()
        .filter(|a| a.matches_query(&query))
        .cloned()
        .collect();
    let page_count = matches.len().div_ceil(APPS_PER_PAGE).max(1);
    let page = (*current_page.borrow()).min(page_count - 1);
    *current_page.borrow_mut() = page;

    let start = page * APPS_PER_PAGE;
    for (idx, app) in matches.iter().skip(start).take(APPS_PER_PAGE).enumerate() {
        let col = (idx % LAUNCHPAD_COLS) as i32;
        let row = (idx / LAUNCHPAD_COLS) as i32;

        let tile = Button::new();
        tile.add_css_class("neon-launchpad-tile");
        if let Some(comment) = &app.comment {
            tile.set_tooltip_text(Some(comment));
        }
        let vbox = GtkBox::new(Orientation::Vertical, 8);
        vbox.set_halign(Align::Center);
        vbox.set_valign(Align::Center);

        let icon = Image::from_icon_name(app.icon.as_deref().unwrap_or("application-x-executable"));
        icon.set_pixel_size(64);
        vbox.append(&icon);

        let lbl = Label::new(Some(&app.name));
        lbl.add_css_class("neon-launchpad-label");
        lbl.set_max_width_chars(14);
        lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        vbox.append(&lbl);

        tile.set_child(Some(&vbox));
        let app_clone = app.clone();
        let ipc_c = ipc.clone();
        tile.connect_clicked(move |_| {
            app_clone.launch();
            ipc_c.send(IpcRequest::SetOverview { open: false });
        });
        apps_grid.attach(&tile, col, row, 1, 1);
    }

    if page_count > 1 {
        for p in 0..page_count {
            let dot = Button::new();
            dot.add_css_class("neon-page-dot");
            if p == page {
                dot.add_css_class("active");
            }
            let page_ref = Rc::clone(current_page);
            let grid_ref = apps_grid.clone();
            let dots_ref = dots_box.clone();
            let apps_ref = Rc::clone(all_apps);
            let search_ref = search_entry.clone();
            let ipc_ref = ipc.clone();
            dot.connect_clicked(move |_| {
                *page_ref.borrow_mut() = p;
                rebuild_launchpad_page(
                    &grid_ref,
                    &dots_ref,
                    &apps_ref,
                    &search_ref,
                    &page_ref,
                    &ipc_ref,
                );
            });
            dots_box.append(&dot);
        }
    }
}
