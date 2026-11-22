//! NEON DE macOS Launchpad & Mission Control (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Features a live GPU-blurred desktop wallpaper backdrop, smooth macOS zoom+fade
//! opening and closing animations, horizontal `SlideLeftRight` multi-page app grid
//! controllable via mouse wheel / trackpad scrolling, arrow keys, or pagination dots,
//! and a clean Mission Control workspace switcher tab.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, EventControllerKey,
    EventControllerScroll, EventControllerScrollFlags, FlowBox, GestureClick, Grid, Label,
    Orientation, Overlay, Picture, PolicyType, ScrolledWindow, SearchEntry, SelectionMode, Stack,
    StackTransitionType, gdk, gio, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{AppearanceInfo, DesktopSnapshot, IpcRequest};

use crate::{
    desktop_apps::{DesktopApp, load_app_icon},
    ipc_bridge::IpcCommandSender,
    wallpaper::resolve_wallpaper_path,
};

const LAUNCHPAD_COLS: usize = 7;
const LAUNCHPAD_ROWS: usize = 5;
const APPS_PER_PAGE: usize = LAUNCHPAD_COLS * LAUNCHPAD_ROWS;

/// Fullscreen macOS Launchpad + Mission Control overlay.
pub struct Overview {
    pub window: ApplicationWindow,
    backdrop_overlay: Overlay,
    blur_picture: Picture,
    content_box: GtkBox,
    current_wp: RefCell<Option<PathBuf>>,
    desired_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    mode_stack: Stack,
    launchpad_tab_btn: Button,
    mission_tab_btn: Button,
    search_entry: SearchEntry,
    page_stack: Stack,
    dots_box: GtkBox,
    workspaces_box: GtkBox,
    windows_flow: FlowBox,
    all_apps: Rc<Vec<DesktopApp>>,
    current_page: Rc<Cell<usize>>,
    page_count: Rc<Cell<usize>>,
    dot_buttons: Rc<RefCell<Vec<Button>>>,
    last_scroll_at: Rc<RefCell<Instant>>,
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

        // Fullscreen overlay: live GPU-blurred desktop wallpaper + translucent tint + animated Launchpad
        let backdrop_overlay = Overlay::new();
        backdrop_overlay.add_css_class("neon-launchpad-overlay");
        backdrop_overlay.add_css_class("lp-closed");

        let blur_picture = Picture::new();
        blur_picture.add_css_class("neon-launchpad-blur-bg");
        blur_picture.set_content_fit(ContentFit::Cover);
        blur_picture.set_can_shrink(true);
        blur_picture.set_size_request(1, 1);
        blur_picture.set_hexpand(true);
        blur_picture.set_vexpand(true);

        let initial_wp = resolve_wallpaper_path(None, false);
        if let Some(ref wp) = initial_wp {
            blur_picture.set_file(Some(&gio::File::for_path(wp)));
        }

        let content_box = GtkBox::new(Orientation::Vertical, 18);
        content_box.add_css_class("neon-launchpad-scrim");
        content_box.add_css_class("neon-launchpad-content");
        content_box.set_hexpand(true);
        content_box.set_vexpand(true);

        // Top centered macOS Launchpad search box + minimalist Liquid Glass segmented switcher
        let top_area = GtkBox::new(Orientation::Vertical, 12);
        top_area.set_halign(Align::Center);
        top_area.set_margin_top(16);

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

        content_box.append(&top_area);

        // Main stack switching cleanly between Launchpad (default) and Mission Control
        let mode_stack = Stack::new();
        mode_stack.set_vexpand(true);
        mode_stack.set_hexpand(true);
        mode_stack.set_transition_type(StackTransitionType::Crossfade);
        mode_stack.set_transition_duration(180);

        // === Page 1: macOS Launchpad (smooth horizontal sliding page stack) ===
        let launchpad_page = GtkBox::new(Orientation::Vertical, 14);
        launchpad_page.set_vexpand(true);
        launchpad_page.set_hexpand(true);

        let page_stack = Stack::new();
        page_stack.set_vexpand(true);
        page_stack.set_hexpand(true);
        page_stack.set_halign(Align::Center);
        page_stack.set_valign(Align::Center);
        page_stack.set_transition_type(StackTransitionType::SlideLeftRight);
        page_stack.set_transition_duration(260);
        launchpad_page.append(&page_stack);

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

        content_box.append(&mode_stack);

        let blur_holder = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::External)
            .vscrollbar_policy(PolicyType::External)
            .propagate_natural_width(false)
            .propagate_natural_height(false)
            .can_target(false)
            .focusable(false)
            .child(&blur_picture)
            .build();

        backdrop_overlay.set_child(Some(&blur_holder));
        backdrop_overlay.add_overlay(&content_box);
        window.set_child(Some(&backdrop_overlay));

        let overview = Rc::new(Self {
            window,
            backdrop_overlay,
            blur_picture,
            content_box,
            current_wp: RefCell::new(initial_wp),
            desired_open: Rc::new(Cell::new(false)),
            anim_gen: Rc::new(Cell::new(0)),
            mode_stack,
            launchpad_tab_btn,
            mission_tab_btn,
            search_entry,
            page_stack,
            dots_box,
            workspaces_box,
            windows_flow,
            all_apps: Rc::new(apps),
            current_page: Rc::new(Cell::new(0)),
            page_count: Rc::new(Cell::new(1)),
            dot_buttons: Rc::new(RefCell::new(Vec::new())),
            last_scroll_at: Rc::new(RefCell::new(
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .unwrap_or_else(Instant::now),
            )),
            last_snapshot: Rc::new(RefCell::new(None)),
            ipc: ipc.clone(),
        });

        // Mouse wheel & touchpad scrolling turns Launchpad pages smoothly!
        let scroll_ctrl = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
        {
            let ov = Rc::clone(&overview);
            scroll_ctrl.connect_scroll(move |_, dx, dy| {
                if ov.mode_stack.visible_child_name().as_deref() != Some("launchpad") {
                    return glib::Propagation::Proceed;
                }
                let primary = if dy.abs() >= dx.abs() { dy } else { dx };
                if primary.abs() < 0.08 {
                    return glib::Propagation::Proceed;
                }
                let now = Instant::now();
                if now.duration_since(*ov.last_scroll_at.borrow()) < Duration::from_millis(200) {
                    return glib::Propagation::Stop;
                }
                *ov.last_scroll_at.borrow_mut() = now;
                if primary > 0.0 {
                    ov.step_page(1);
                } else {
                    ov.step_page(-1);
                }
                glib::Propagation::Stop
            });
        }
        overview.content_box.add_controller(scroll_ctrl);

        // Clicking empty backdrop dismisses Launchpad smoothly (macOS behavior)
        let bg_click = GestureClick::new();
        {
            let ov = Rc::clone(&overview);
            let search = overview.search_entry.clone();
            bg_click.connect_released(move |gesture, _, x, y| {
                if let Some(widget) = gesture.widget()
                    && let Some(picked) = widget.pick(x, y, gtk4::PickFlags::DEFAULT)
                {
                    let on_interactive = picked.is_ancestor(&search)
                        || picked == search.clone().upcast::<gtk4::Widget>()
                        || picked.ancestor(Button::static_type()).is_some();
                    if !on_interactive {
                        ov.request_close();
                    }
                }
            });
        }
        overview.content_box.add_controller(bg_click);

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

        // Keyboard navigation: Esc closes, Left/Right/PageUp/PageDown turns Launchpad pages
        let key_ctrl = EventControllerKey::new();
        {
            let ov = Rc::clone(&overview);
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    ov.request_close();
                    return glib::Propagation::Stop;
                }
                if ov.search_entry.text().is_empty() {
                    if matches!(key, gdk::Key::Right | gdk::Key::Page_Down) {
                        ov.step_page(1);
                        return glib::Propagation::Stop;
                    }
                    if matches!(key, gdk::Key::Left | gdk::Key::Page_Up) {
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
                    ov.request_close();
                }
            });
        }

        // Live search resets to page 0 and filters smoothly
        {
            let ov = Rc::clone(&overview);
            overview.search_entry.connect_search_changed(move |_| {
                ov.current_page.set(0);
                ov.rebuild_launchpad();
            });
        }

        overview.rebuild_launchpad();
        overview.window.set_visible(false);
        overview
    }

    pub fn update_appearance(&self, appearance: &AppearanceInfo) {
        let light = appearance.theme == "neon_light" || appearance.theme == "light";
        if let Some(wp) = resolve_wallpaper_path(appearance.wallpaper.as_deref(), light)
            && self.current_wp.borrow().as_ref() != Some(&wp)
        {
            self.blur_picture.set_file(Some(&gio::File::for_path(&wp)));
            *self.current_wp.borrow_mut() = Some(wp);
        }
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

    pub fn request_close(&self) {
        self.set_open(false);
        self.ipc.send(IpcRequest::SetOverview { open: false });
    }

    pub fn set_open(&self, open: bool) {
        if self.desired_open.get() == open && self.window.is_visible() == open {
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.desired_open.set(open);

        if open {
            if gtk4_layer_shell::is_supported() {
                self.window.set_keyboard_mode(KeyboardMode::Exclusive);
            }
            self.set_mode("launchpad");
            self.current_page.set(0);
            self.search_entry.set_text("");
            self.rebuild_launchpad();

            // Reveal in closed zoomed-out state, then smoothly animate into open state
            self.backdrop_overlay.remove_css_class("lp-open");
            self.backdrop_overlay.add_css_class("lp-closed");
            self.window.set_visible(true);
            self.window.present();
            self.search_entry.grab_focus();

            let overlay = self.backdrop_overlay.clone();
            let gen_cell = Rc::clone(&self.anim_gen);
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                if gen_cell.get() == epoch {
                    overlay.remove_css_class("lp-closed");
                    overlay.add_css_class("lp-open");
                }
            });
        } else {
            // Play smooth macOS zoom-out + blur fade-out before hiding the layer surface
            self.backdrop_overlay.remove_css_class("lp-open");
            self.backdrop_overlay.add_css_class("lp-closed");

            let win = self.window.clone();
            let gen_cell = Rc::clone(&self.anim_gen);
            glib::timeout_add_local_once(Duration::from_millis(210), move || {
                if gen_cell.get() == epoch {
                    if gtk4_layer_shell::is_supported() {
                        win.set_keyboard_mode(KeyboardMode::None);
                    }
                    win.set_visible(false);
                }
            });
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
        self.update_appearance(&snapshot.appearance);
        if self.desired_open.get() != snapshot.overview_open {
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

    fn go_to_page(&self, target: usize) {
        let total = self.page_count.get().max(1);
        let next = target.min(total - 1);
        if next == self.current_page.get() {
            return;
        }
        self.current_page.set(next);
        self.page_stack
            .set_visible_child_name(&format!("page-{next}"));
        for (idx, dot) in self.dot_buttons.borrow().iter().enumerate() {
            if idx == next {
                dot.add_css_class("active");
            } else {
                dot.remove_css_class("active");
            }
        }
    }

    fn step_page(&self, delta: isize) {
        let total = self.page_count.get().max(1) as isize;
        let cur = self.current_page.get() as isize;
        let next = (cur + delta).clamp(0, total - 1) as usize;
        self.go_to_page(next);
    }

    fn rebuild_launchpad(&self) {
        while let Some(child) = self.page_stack.first_child() {
            self.page_stack.remove(&child);
        }
        while let Some(child) = self.dots_box.first_child() {
            self.dots_box.remove(&child);
        }
        self.dot_buttons.borrow_mut().clear();

        let matches = self.filtered_apps();
        let total_pages = matches.len().div_ceil(APPS_PER_PAGE).max(1);
        self.page_count.set(total_pages);
        let active_page = self.current_page.get().min(total_pages - 1);
        self.current_page.set(active_page);

        for p in 0..total_pages {
            let apps_grid = Grid::new();
            apps_grid.set_column_spacing(26);
            apps_grid.set_row_spacing(20);
            apps_grid.set_column_homogeneous(true);
            apps_grid.set_row_homogeneous(true);
            apps_grid.set_halign(Align::Center);
            apps_grid.set_valign(Align::Center);

            let start = p * APPS_PER_PAGE;
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

                let icon = load_app_icon(app.icon.as_deref(), &app.name, 68);
                vbox.append(&icon);

                let name_lbl = Label::new(Some(&app.name));
                name_lbl.add_css_class("neon-launchpad-label");
                name_lbl.set_max_width_chars(14);
                name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                vbox.append(&name_lbl);

                tile.set_child(Some(&vbox));

                let app_clone = app.clone();
                let ipc = self.ipc.clone();
                let overlay = self.backdrop_overlay.clone();
                let win = self.window.clone();
                let desired = Rc::clone(&self.desired_open);
                let gen_cell = Rc::clone(&self.anim_gen);
                tile.connect_clicked(move |_| {
                    app_clone.launch();
                    let epoch = gen_cell.get().wrapping_add(1);
                    gen_cell.set(epoch);
                    desired.set(false);
                    overlay.remove_css_class("lp-open");
                    overlay.add_css_class("lp-closed");
                    ipc.send(IpcRequest::SetOverview { open: false });
                    let win_c = win.clone();
                    let gen_c = Rc::clone(&gen_cell);
                    glib::timeout_add_local_once(Duration::from_millis(200), move || {
                        if gen_c.get() == epoch {
                            if gtk4_layer_shell::is_supported() {
                                win_c.set_keyboard_mode(KeyboardMode::None);
                            }
                            win_c.set_visible(false);
                        }
                    });
                });

                apps_grid.attach(&tile, col, row, 1, 1);
            }

            self.page_stack
                .add_named(&apps_grid, Some(&format!("page-{p}")));
        }

        self.page_stack
            .set_visible_child_name(&format!("page-{active_page}"));

        // macOS Launchpad bottom pagination dots (clickable, smooth slide)
        if total_pages > 1 {
            let mut dots_vec = Vec::with_capacity(total_pages);
            for p in 0..total_pages {
                let dot = Button::new();
                dot.add_css_class("neon-page-dot");
                if p == active_page {
                    dot.add_css_class("active");
                }
                dot.set_tooltip_text(Some(&format!("Page {}", p + 1)));
                dots_vec.push(dot.clone());
                self.dots_box.append(&dot);
            }
            *self.dot_buttons.borrow_mut() = dots_vec;

            for (p, dot) in self.dot_buttons.borrow().iter().enumerate() {
                let page_cell = Rc::clone(&self.current_page);
                let stack_ref = self.page_stack.clone();
                let all_dots = Rc::clone(&self.dot_buttons);
                dot.connect_clicked(move |_| {
                    page_cell.set(p);
                    stack_ref.set_visible_child_name(&format!("page-{p}"));
                    for (idx, d) in all_dots.borrow().iter().enumerate() {
                        if idx == p {
                            d.add_css_class("active");
                        } else {
                            d.remove_css_class("active");
                        }
                    }
                });
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
