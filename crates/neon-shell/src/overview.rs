//! NEON DE 60 FPS Fullscreen Blurred App Drawer / Launchpad (`zwlr_layer_shell_v1` `Layer::Top`,
//! `exclusive_zone = -1`).
//!
//! Renders an edge-to-edge **pre-blurred static GPU texture** (zero per-frame blur shader cost,
//! keeping page sliding and animations locked at 60 FPS), minimalist centered Search pill,
//! top-aligned 7×5 paged app icon grid (every page and newly installed app starts from Row 0),
//! centered circular pagination dots above the Dock, and frame-synchronized seamless crossfade
//! with the elevated Top Bar and Dock backgrounds.

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, EventControllerKey,
    EventControllerScroll, EventControllerScrollFlags, GestureClick, Grid, Label, Orientation,
    Overlay, Picture, PolicyType, ScrolledWindow, SearchEntry, Stack, StackTransitionType, gdk,
    gio, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{AppearanceInfo, DesktopSnapshot, IpcRequest};

use crate::{
    desktop_apps::{DesktopApp, load_app_icon},
    ipc_bridge::IpcCommandSender,
    wallpaper::{load_blurred_texture, resolve_wallpaper_path},
};

const LAUNCHPAD_COLS: usize = 7;
const LAUNCHPAD_ROWS: usize = 5;
const APPS_PER_PAGE: usize = LAUNCHPAD_COLS * LAUNCHPAD_ROWS;

type BoolCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

/// Pure 60 FPS fullscreen blurred App Drawer (Launchpad).
pub struct Overview {
    pub window: ApplicationWindow,
    backdrop_overlay: Overlay,
    blur_picture: Picture,
    content_box: GtkBox,
    current_wp_sig: RefCell<Option<(PathBuf, bool, u32)>>,
    desired_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    on_layer_elevate: BoolCallback,
    on_bg_hidden: BoolCallback,
    search_entry: SearchEntry,
    page_stack: Stack,
    dots_box: GtkBox,
    all_apps: Rc<Vec<DesktopApp>>,
    current_page: Rc<Cell<usize>>,
    page_count: Rc<Cell<usize>>,
    dot_buttons: Rc<RefCell<Vec<Button>>>,
    last_scroll_at: Rc<RefCell<Instant>>,
    ipc: IpcCommandSender,
}

impl Overview {
    pub fn build(app: &Application, apps: Vec<DesktopApp>, ipc: IpcCommandSender) -> Rc<Self> {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE App Drawer")
            .decorated(false)
            .build();
        window.add_css_class("neon-overview-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-overview"));
            // Layer::Top with exclusive_zone(-1) spans 100% fullscreen edge-to-edge above all
            // regular windows while sitting cleanly BELOW the elevated Top Bar and Dock.
            window.set_layer(Layer::Top);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        // Fullscreen overlay: pre-blurred static wallpaper texture + subtle veil + 7×5 app grid
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
        let initial_sig = initial_wp.as_ref().map(|p| (p.clone(), false, 100u32));
        if let Some(ref wp) = initial_wp {
            if let Some(tex) = load_blurred_texture(wp, 320, 180, 11, 0.78, 1.36) {
                blur_picture.set_paintable(Some(&tex));
            } else {
                blur_picture.set_file(Some(&gio::File::for_path(wp)));
            }
        }

        let content_box = GtkBox::new(Orientation::Vertical, 18);
        content_box.add_css_class("neon-launchpad-scrim");
        content_box.add_css_class("neon-launchpad-content");
        content_box.set_hexpand(true);
        content_box.set_vexpand(true);

        // Minimalist centered translucent Launchpad Search pill (directly under floating Top Bar)
        let search_entry = SearchEntry::new();
        search_entry.add_css_class("neon-launchpad-search");
        search_entry.set_placeholder_text(Some("Search"));
        search_entry.set_halign(Align::Center);
        search_entry.set_margin_top(8);
        content_box.append(&search_entry);

        // Top-aligned 7×5 paged app grid wrapper: every page (Page 1, Page 2, ...) starts at Row 0!
        let pages_holder = GtkBox::new(Orientation::Vertical, 0);
        pages_holder.set_hexpand(true);
        pages_holder.set_vexpand(true);
        pages_holder.set_halign(Align::Center);
        pages_holder.set_valign(Align::Fill);
        pages_holder.set_margin_top(16);

        let page_stack = Stack::new();
        page_stack.set_hexpand(true);
        page_stack.set_vexpand(true);
        page_stack.set_halign(Align::Center);
        page_stack.set_valign(Align::Start);
        page_stack.set_transition_type(StackTransitionType::SlideLeftRight);
        page_stack.set_transition_duration(240);
        pages_holder.append(&page_stack);
        content_box.append(&pages_holder);

        // Pure centered pagination dots right above the floating Dock (no extra corner buttons)
        let dots_box = GtkBox::new(Orientation::Horizontal, 10);
        dots_box.set_halign(Align::Center);
        dots_box.set_valign(Align::End);
        dots_box.set_margin_bottom(6);
        content_box.append(&dots_box);

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
            current_wp_sig: RefCell::new(initial_sig),
            desired_open: Rc::new(Cell::new(false)),
            anim_gen: Rc::new(Cell::new(0)),
            on_layer_elevate: Rc::new(RefCell::new(None)),
            on_bg_hidden: Rc::new(RefCell::new(None)),
            search_entry,
            page_stack,
            dots_box,
            all_apps: Rc::new(apps),
            current_page: Rc::new(Cell::new(0)),
            page_count: Rc::new(Cell::new(1)),
            dot_buttons: Rc::new(RefCell::new(Vec::new())),
            last_scroll_at: Rc::new(RefCell::new(
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .unwrap_or_else(Instant::now),
            )),
            ipc,
        });

        // 60 FPS smooth mouse wheel & trackpad horizontal/vertical page turning
        let scroll_ctrl = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
        {
            let ov = Rc::clone(&overview);
            scroll_ctrl.connect_scroll(move |_, dx, dy| {
                let primary = if dy.abs() >= dx.abs() { dy } else { dx };
                if primary.abs() < 0.08 {
                    return glib::Propagation::Proceed;
                }
                let now = Instant::now();
                if now.duration_since(*ov.last_scroll_at.borrow()) < Duration::from_millis(220) {
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

        // Clicking empty blurred space closes the App Drawer smoothly
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

        // Enter in search launches first matching app and smoothly closes drawer
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

    /// Registers callbacks that synchronize Top Bar & Dock layer elevation (`Layer::Overlay`
    /// while drawer is open) and frame-locked background crossfade transitions.
    pub fn set_chrome_hooks<F, G>(&self, on_layer_elevate: F, on_bg_hidden: G)
    where
        F: Fn(bool) + 'static,
        G: Fn(bool) + 'static,
    {
        *self.on_layer_elevate.borrow_mut() = Some(Box::new(on_layer_elevate));
        *self.on_bg_hidden.borrow_mut() = Some(Box::new(on_bg_hidden));
    }

    pub fn update_appearance(&self, appearance: &AppearanceInfo) {
        let light = appearance.theme == "neon_light" || appearance.theme == "light";
        let radius = if appearance.high_contrast {
            0
        } else if appearance.reduce_blur {
            6
        } else {
            ((11.0 * appearance.blur_strength.clamp(0.4, 2.0)).round() as usize).clamp(4, 18)
        };
        let sig_key = radius as u32;

        self.blur_picture.set_visible(!appearance.high_contrast);
        if let Some(wp) = resolve_wallpaper_path(appearance.wallpaper.as_deref(), light) {
            let next_sig = (wp.clone(), light, sig_key);
            if self.current_wp_sig.borrow().as_ref() != Some(&next_sig) {
                let (brightness, saturation) = if light { (0.90, 1.25) } else { (0.78, 1.36) };
                if let Some(tex) =
                    load_blurred_texture(&wp, 320, 180, radius, brightness, saturation)
                {
                    self.blur_picture.set_paintable(Some(&tex));
                } else {
                    self.blur_picture.set_file(Some(&gio::File::for_path(&wp)));
                }
                *self.current_wp_sig.borrow_mut() = Some(next_sig);
            }
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
            // 1. Elevate Top Bar & Dock above the App Drawer immediately so they stay on top
            if let Some(cb) = self.on_layer_elevate.borrow().as_ref() {
                cb(true);
            }

            if gtk4_layer_shell::is_supported() {
                self.window.set_keyboard_mode(KeyboardMode::Exclusive);
            }
            if !self.search_entry.text().is_empty() {
                self.current_page.set(0);
                self.search_entry.set_text("");
            } else {
                self.go_to_page(0);
            }

            // 2. Present in closed zoomed-out state, wait 2 frame ticks so Wayland commits the
            //    initial frame cleanly, then simultaneously fade in the fullscreen blur AND fade
            //    out the Top Bar & Dock backgrounds on the exact same frame (280ms seamless swap).
            self.backdrop_overlay.remove_css_class("lp-open");
            self.backdrop_overlay.add_css_class("lp-closed");
            self.window.set_visible(true);
            self.window.present();
            self.search_entry.grab_focus();

            let triggered = Rc::new(Cell::new(false));
            let ticks = Rc::new(Cell::new(0u8));
            {
                let gen_cell = Rc::clone(&self.anim_gen);
                let trig = Rc::clone(&triggered);
                let ticks_c = Rc::clone(&ticks);
                let bg_hook = Rc::clone(&self.on_bg_hidden);
                self.backdrop_overlay.add_tick_callback(move |widget, _| {
                    if gen_cell.get() != epoch || trig.get() {
                        return glib::ControlFlow::Break;
                    }
                    let next = ticks_c.get().saturating_add(1);
                    ticks_c.set(next);
                    if next >= 2 {
                        trig.set(true);
                        widget.remove_css_class("lp-closed");
                        widget.add_css_class("lp-open");
                        if let Some(cb) = bg_hook.borrow().as_ref() {
                            cb(true);
                        }
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                });
            }

            let overlay = self.backdrop_overlay.clone();
            let gen_cell = Rc::clone(&self.anim_gen);
            let bg_hook = Rc::clone(&self.on_bg_hidden);
            glib::timeout_add_local_once(Duration::from_millis(60), move || {
                if gen_cell.get() == epoch && !triggered.get() {
                    triggered.set(true);
                    overlay.remove_css_class("lp-closed");
                    overlay.add_css_class("lp-open");
                    if let Some(cb) = bg_hook.borrow().as_ref() {
                        cb(true);
                    }
                }
            });
        } else {
            // Simultaneously fade out the fullscreen blur AND fade in the Top Bar & Dock
            // backgrounds on the exact same frame with the exact same 280ms duration.
            self.backdrop_overlay.remove_css_class("lp-open");
            self.backdrop_overlay.add_css_class("lp-closed");
            if let Some(cb) = self.on_bg_hidden.borrow().as_ref() {
                cb(false);
            }

            let win = self.window.clone();
            let gen_cell = Rc::clone(&self.anim_gen);
            let layer_hook = Rc::clone(&self.on_layer_elevate);
            glib::timeout_add_local_once(Duration::from_millis(290), move || {
                if gen_cell.get() == epoch {
                    if gtk4_layer_shell::is_supported() {
                        win.set_keyboard_mode(KeyboardMode::None);
                    }
                    win.set_visible(false);
                    if let Some(cb) = layer_hook.borrow().as_ref() {
                        cb(false);
                    }
                }
            });
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        self.update_appearance(&snapshot.appearance);
        if self.desired_open.get() != snapshot.overview_open {
            self.set_open(snapshot.overview_open);
        }
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

    fn rebuild_launchpad(self: &Rc<Self>) {
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
            // Outer wrapper ensures every page (even with 1-7 apps) anchors cleanly to Top-Left!
            let page_frame = GtkBox::new(Orientation::Vertical, 0);
            page_frame.set_halign(Align::Center);
            page_frame.set_valign(Align::Start);
            page_frame.set_vexpand(true);

            let apps_grid = Grid::new();
            apps_grid.set_column_spacing(34);
            apps_grid.set_row_spacing(18);
            apps_grid.set_column_homogeneous(true);
            apps_grid.set_row_homogeneous(false);
            apps_grid.set_halign(Align::Start);
            apps_grid.set_valign(Align::Start);

            // Insert invisible placeholder spacers on empty columns of Row 0 for sparse pages
            // so a 2-app Page 2 keeps identical 7-column horizontal alignment with Page 1!
            let start = p * APPS_PER_PAGE;
            let page_slice: Vec<&DesktopApp> = matches.iter().skip(start).take(APPS_PER_PAGE).collect();
            for (idx, app) in page_slice.iter().enumerate() {
                let col = (idx % LAUNCHPAD_COLS) as i32;
                let row = (idx / LAUNCHPAD_COLS) as i32;

                let tile = Button::new();
                tile.add_css_class("neon-launchpad-tile");
                if let Some(comment) = &app.comment {
                    tile.set_tooltip_text(Some(comment));
                }

                let vbox = GtkBox::new(Orientation::Vertical, 7);
                vbox.set_halign(Align::Center);
                vbox.set_valign(Align::Center);

                let icon = load_app_icon(app.icon.as_deref(), &app.name, 72);
                icon.set_halign(Align::Center);
                icon.set_valign(Align::Center);
                vbox.append(&icon);

                let name_lbl = Label::new(Some(&app.name));
                name_lbl.add_css_class("neon-launchpad-label");
                name_lbl.set_halign(Align::Center);
                name_lbl.set_max_width_chars(14);
                name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                vbox.append(&name_lbl);

                tile.set_child(Some(&vbox));

                let app_clone = (*app).clone();
                let ov = Rc::clone(self);
                tile.connect_clicked(move |_| {
                    app_clone.launch();
                    ov.request_close();
                });

                apps_grid.attach(&tile, col, row, 1, 1);
            }

            // Fill remaining empty columns in Row 0 so sparse pages keep exact 7-column width
            if page_slice.len() < LAUNCHPAD_COLS {
                for empty_col in page_slice.len()..LAUNCHPAD_COLS {
                    let spacer = GtkBox::new(Orientation::Vertical, 0);
                    spacer.set_size_request(112, 1);
                    spacer.set_can_target(false);
                    apps_grid.attach(&spacer, empty_col as i32, 0, 1, 1);
                }
            }

            page_frame.append(&apps_grid);
            self.page_stack
                .add_named(&page_frame, Some(&format!("page-{p}")));
        }

        self.page_stack
            .set_visible_child_name(&format!("page-{active_page}"));

        // macOS Launchpad bottom pagination dots (clickable, 60 FPS slide)
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
}
