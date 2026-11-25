//! NEON DE macOS × GNOME 47 Top Menu Bar (`zwlr_layer_shell_v1` `Layer::Top` anchored Top+Left+Right).
//!
//! Features:
//! - Live workspaces indicator, focused app title, native `StatusNotifierItem` System Tray,
//!   live Screen Recording stop pill (`● 00:00 Stop`), Battery, Control Center, and Clock
//! - **Smart Hide Top Bar**: reserves 0px exclusive zone when enabled, hides smoothly when a
//!   maximized or overlapping window touches the top bar, reveals via a 3px top edge sensor
//! - **Fullscreen Removal**: completely removes both top bar and edge sensor during fullscreen

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    f64::consts::PI,
    rc::Rc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, DrawingArea,
    EventControllerMotion, GestureClick, Image, Label, Orientation, Separator, gdk, gio, glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};
use neon_system::{
    SystemSnapshot,
    power::BatteryState,
    tray::{TrayAction, TrayActionSender, TrayItem, TrayMenuItem},
};

use crate::{
    context_menu::{ContextMenuOverlay, PanelMenuZone},
    ipc_bridge::IpcCommandSender,
    quick_settings::QuickSettingsPanel,
    workspaces_panel::WorkspacesPanel,
};

#[derive(Clone)]
struct PendingTrayMenu {
    key: String,
    app_title: String,
    object_path: String,
    anchor_x: i32,
}

#[derive(Default)]
struct PanelHideState {
    panel_visible: bool,
    smart_hide: bool,
    has_fullscreen: bool,
    overview_open: bool,
    popup_open: bool,
    window_overlaps_panel: bool,
    hovered_panel: bool,
    hovered_sensor: bool,
    currently_shown: bool,
    auto_exclusive_active: bool,
    anim_gen: u64,
}

type StopRecordingCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// Live handles to updatable widgets in the NEON DE Top Menu Bar.
pub struct TopPanel {
    window: ApplicationWindow,
    sensor_window: ApplicationWindow,
    bar: CenterBox,
    activities_btn: Button,
    workspaces_box: GtkBox,
    focused_label: Label,
    rec_btn: Button,
    rec_label: Label,
    on_stop_recording: StopRecordingCallback,
    tray_box: GtkBox,
    tray_sep: Separator,
    clock_btn: Button,
    clock_label: Label,
    clock_24h: Rc<Cell<bool>>,
    power_btn: Button,
    power_icon: Image,
    power_pct_label: Label,
    cc_btn: Button,
    cc_icon_area: DrawingArea,
    show_system_tray: Cell<bool>,
    is_light_theme: Rc<Cell<bool>>,
    drawer_bg_hidden: Rc<Cell<bool>>,
    notif_dot: GtkBox,
    last_ws_sig: RefCell<Vec<(usize, bool, usize)>>,
    tray_items: RefCell<Vec<TrayItem>>,
    pending_tray_menu: Rc<RefCell<Option<PendingTrayMenu>>>,
    hide_state: Rc<RefCell<PanelHideState>>,
    tray_tx: TrayActionSender,
    context_menu: ContextMenuOverlay,
    ipc: IpcCommandSender,
}

impl TopPanel {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        tray_tx: TrayActionSender,
        quick_settings: QuickSettingsPanel,
        workspaces_panel: WorkspacesPanel,
        context_menu: ContextMenuOverlay,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Top Bar")
            .decorated(false)
            .default_height(30)
            .build();
        window.add_css_class("neon-panel-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-panel"));
            window.set_layer(Layer::Top);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_keyboard_mode(KeyboardMode::None);
            window.set_exclusive_zone(31);
        }

        // Invisible 3px top-edge hover sensor for Smart-Hide Top Bar reveal.
        let sensor_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Top Bar Sensor")
            .decorated(false)
            .resizable(false)
            .default_height(3)
            .build();
        sensor_window.add_css_class("neon-edge-sensor-window");
        if gtk4_layer_shell::is_supported() {
            sensor_window.init_layer_shell();
            sensor_window.set_namespace(Some("neon-panel-sensor"));
            sensor_window.set_layer(Layer::Top);
            sensor_window.set_anchor(Edge::Top, true);
            sensor_window.set_anchor(Edge::Left, true);
            sensor_window.set_anchor(Edge::Right, true);
            sensor_window.set_keyboard_mode(KeyboardMode::None);
            sensor_window.set_exclusive_zone(0);
        }
        let sensor_strip = GtkBox::new(Orientation::Horizontal, 0);
        sensor_strip.add_css_class("neon-edge-sensor-strip");
        sensor_strip.set_size_request(-1, 3);
        sensor_strip.set_hexpand(true);
        sensor_window.set_child(Some(&sensor_strip));
        sensor_window.set_visible(false);

        let bar = CenterBox::new();
        bar.set_hexpand(true);
        bar.add_css_class("neon-top-panel");

        let hide_state = Rc::new(RefCell::new(PanelHideState {
            panel_visible: true,
            currently_shown: true,
            auto_exclusive_active: true,
            ..Default::default()
        }));

        // Left: NEON brand / Activities button + GNOME 47 workspace dots + focused app name
        let left_box = GtkBox::new(Orientation::Horizontal, 8);
        left_box.set_valign(Align::Center);

        let activities_btn = Button::with_label("NEON");
        activities_btn.add_css_class("neon-brand-button");
        activities_btn.set_tooltip_text(Some(
            "Workspaces Sidebar (Switch with Super+1–0 or Super+Numpad)",
        ));
        {
            let btn_c = activities_btn.clone();
            let st_c = Rc::clone(&hide_state);
            let win_c = window.clone();
            let sen_c = sensor_window.clone();
            let bar_c = bar.clone();
            workspaces_panel.set_on_state_change(move |open| {
                if open {
                    btn_c.add_css_class("active");
                } else {
                    btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().popup_open = open;
                sync_panel_visibility(&st_c, &win_c, &sen_c, &bar_c, !open);
            });
        }
        {
            let ws_panel = workspaces_panel.clone();
            let qs_close = quick_settings.clone();
            let ipc_close_ov = ipc.clone();
            activities_btn.connect_clicked(move |_| {
                qs_close.close();
                ipc_close_ov.send(IpcRequest::SetOverview { open: false });
                ws_panel.toggle();
            });
        }

        let workspaces_box = GtkBox::new(Orientation::Horizontal, 4);
        workspaces_box.set_valign(Align::Center);

        let focused_label = Label::new(Some("Desktop"));
        focused_label.add_css_class("neon-app-title");
        focused_label.set_max_width_chars(24);
        focused_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        left_box.append(&activities_btn);
        left_box.append(&workspaces_box);
        left_box.append(&focused_label);

        // Right: Active Recording Stop Pill + System Tray + Battery + Control Center + Clock
        let right_box = GtkBox::new(Orientation::Horizontal, 2);
        right_box.set_valign(Align::Center);

        let on_stop_recording: StopRecordingCallback = Rc::new(RefCell::new(None));
        let rec_inner = GtkBox::new(Orientation::Horizontal, 5);
        rec_inner.set_valign(Align::Center);
        let rec_dot = GtkBox::new(Orientation::Horizontal, 0);
        rec_dot.add_css_class("neon-rec-dot");
        rec_dot.set_valign(Align::Center);
        let rec_label = Label::new(Some("00:00  Stop"));
        rec_label.add_css_class("neon-rec-label");
        rec_inner.append(&rec_dot);
        rec_inner.append(&rec_label);

        let rec_btn = Button::new();
        rec_btn.add_css_class("neon-rec-pill");
        rec_btn.set_child(Some(&rec_inner));
        rec_btn.set_tooltip_text(Some("Click to stop screen recording"));
        rec_btn.set_visible(false);
        {
            let cb_ref = Rc::clone(&on_stop_recording);
            rec_btn.connect_clicked(move |_| {
                if let Some(cb) = cb_ref.borrow().as_ref() {
                    cb();
                }
            });
        }

        let tray_box = GtkBox::new(Orientation::Horizontal, 2);
        tray_box.add_css_class("neon-tray-apps");
        tray_box.set_valign(Align::Center);
        tray_box.set_visible(false);

        let tray_sep = Separator::new(Orientation::Vertical);
        tray_sep.add_css_class("neon-tray-sep");
        tray_sep.set_valign(Align::Center);
        tray_sep.set_visible(false);

        let power_inner = GtkBox::new(Orientation::Horizontal, 4);
        let power_icon = Image::from_icon_name("battery-full-symbolic");
        power_icon.set_pixel_size(15);
        let power_pct_label = Label::new(Some("100%"));
        power_inner.append(&power_icon);
        power_inner.append(&power_pct_label);
        let power_btn = Button::new();
        power_btn.add_css_class("neon-tray-btn");
        power_btn.set_child(Some(&power_inner));
        power_btn.set_tooltip_text(Some("Battery & Power"));
        {
            let qs = quick_settings.clone();
            power_btn.connect_clicked(move |_| qs.toggle());
        }

        let is_light_theme = Rc::new(Cell::new(false));
        let drawer_bg_hidden = Rc::new(Cell::new(false));

        // Authentic macOS Control Center dual-toggle-switches menu bar icon
        let cc_inner = GtkBox::new(Orientation::Horizontal, 4);
        let cc_icon_area = DrawingArea::new();
        cc_icon_area.set_content_width(18);
        cc_icon_area.set_content_height(15);
        cc_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden);
            cc_icon_area.set_draw_func(move |_, cr, w, h| {
                let (r, g, b) = if light_flag.get() && !drawer_flag.get() {
                    (0.06, 0.09, 0.16)
                } else {
                    (0.95, 0.97, 1.0)
                };
                let w = f64::from(w);
                let h = f64::from(h);
                let pill_h = 5.4;
                let pill_w = w - 2.0;
                let rx = pill_h * 0.5;

                let trace_pill = |cr: &gtk4::cairo::Context, x: f64, y: f64, pw: f64, ph: f64| {
                    let rad = ph * 0.5;
                    cr.new_sub_path();
                    cr.arc(x + pw - rad, y + rad, rad, -PI * 0.5, PI * 0.5);
                    cr.arc(x + rad, y + rad, rad, PI * 0.5, PI * 1.5);
                    cr.close_path();
                };

                let top_y = 1.2;
                cr.set_source_rgba(r, g, b, 0.92);
                cr.set_line_width(1.25);
                trace_pill(cr, 1.0, top_y, pill_w, pill_h);
                let _ = cr.stroke();
                cr.arc(1.0 + rx, top_y + rx, rx - 0.8, 0.0, 2.0 * PI);
                let _ = cr.fill();

                let bot_y = h - pill_h - 1.2;
                cr.set_source_rgba(r, g, b, 0.34);
                trace_pill(cr, 1.0, bot_y, pill_w, pill_h);
                let _ = cr.fill();
                cr.set_source_rgba(r, g, b, 0.96);
                cr.set_line_width(1.1);
                trace_pill(cr, 1.0, bot_y, pill_w, pill_h);
                let _ = cr.stroke();
                cr.arc(1.0 + pill_w - rx, bot_y + rx, rx - 0.5, 0.0, 2.0 * PI);
                let _ = cr.fill();
            });
        }

        let notif_dot = GtkBox::new(Orientation::Horizontal, 0);
        notif_dot.add_css_class("neon-notif-dot");
        notif_dot.set_valign(Align::Center);
        notif_dot.set_visible(false);
        cc_inner.append(&cc_icon_area);
        cc_inner.append(&notif_dot);

        let cc_btn = Button::new();
        cc_btn.add_css_class("neon-tray-btn");
        cc_btn.add_css_class("neon-cc-toggle-btn");
        cc_btn.set_child(Some(&cc_inner));
        cc_btn.set_tooltip_text(Some("Quick Settings"));
        {
            let cc_btn_c = cc_btn.clone();
            let ws_close = workspaces_panel.clone();
            let st_c = Rc::clone(&hide_state);
            let win_c = window.clone();
            let sen_c = sensor_window.clone();
            let bar_c = bar.clone();
            quick_settings.set_on_state_change(move |open| {
                if open {
                    ws_close.close();
                    cc_btn_c.add_css_class("active");
                } else {
                    cc_btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().popup_open = open;
                sync_panel_visibility(&st_c, &win_c, &sen_c, &bar_c, !open);
            });
        }
        {
            let qs = quick_settings.clone();
            cc_btn.connect_clicked(move |_| qs.toggle());
        }

        let clock_24h = Rc::new(Cell::new(true));
        let clock_label = Label::new(Some(&format_clock_now(clock_24h.get())));
        let clock_btn = Button::new();
        clock_btn.add_css_class("neon-clock-btn");
        clock_btn.set_child(Some(&clock_label));
        clock_btn.set_tooltip_text(Some("Date, Time & Quick Settings"));
        {
            let qs = quick_settings;
            clock_btn.connect_clicked(move |_| qs.toggle());
        }
        {
            let clock_clone = clock_label.clone();
            let clock_24h_c = Rc::clone(&clock_24h);
            glib::timeout_add_local(Duration::from_secs(1), move || {
                clock_clone.set_text(&format_clock_now(clock_24h_c.get()));
                glib::ControlFlow::Continue
            });
        }

        right_box.append(&rec_btn);
        right_box.append(&tray_box);
        right_box.append(&tray_sep);
        right_box.append(&power_btn);
        right_box.append(&cc_btn);
        right_box.append(&clock_btn);

        bar.set_start_widget(Some(&left_box));
        bar.set_end_widget(Some(&right_box));

        // Context-aware right-click menus for each Top Bar zone & widget.
        let panel_rc = GestureClick::new();
        panel_rc.set_button(3);
        {
            let ctx = context_menu.clone();
            let tray_box_c = tray_box.clone();
            let ws_box_c = workspaces_box.clone();
            let act_btn_c = activities_btn.clone();
            let pwr_btn_c = power_btn.clone();
            let cc_btn_c = cc_btn.clone();
            let clk_btn_c = clock_btn.clone();
            panel_rc.connect_pressed(move |gesture, _, x, y| {
                let mut zone = PanelMenuZone::General;
                if let Some(w) = gesture.widget()
                    && let Some(picked) = w.pick(x, y, gtk4::PickFlags::DEFAULT)
                {
                    let is_in = |target: &gtk4::Widget| picked == *target || picked.is_ancestor(target);
                    if is_in(tray_box_c.upcast_ref()) {
                        return;
                    }
                    if is_in(pwr_btn_c.upcast_ref()) {
                        zone = PanelMenuZone::Battery;
                    } else if is_in(cc_btn_c.upcast_ref()) {
                        zone = PanelMenuZone::QuickSettings;
                    } else if is_in(clk_btn_c.upcast_ref()) {
                        zone = PanelMenuZone::Clock;
                    } else if is_in(ws_box_c.upcast_ref()) || is_in(act_btn_c.upcast_ref()) {
                        zone = PanelMenuZone::Workspaces;
                    }
                }
                ctx.open_panel_menu(x.round() as i32, 34, zone);
            });
        }
        bar.add_controller(panel_rc);

        // Hover controllers for Smart-Hide Top Bar
        {
            let motion = EventControllerMotion::new();
            let st_in = Rc::clone(&hide_state);
            let win_in = window.clone();
            let sen_in = sensor_window.clone();
            let bar_in = bar.clone();
            motion.connect_enter(move |_, _, _| {
                st_in.borrow_mut().hovered_panel = true;
                sync_panel_visibility(&st_in, &win_in, &sen_in, &bar_in, false);
            });
            let st_out = Rc::clone(&hide_state);
            let win_out = window.clone();
            let sen_out = sensor_window.clone();
            let bar_out = bar.clone();
            motion.connect_leave(move |_| {
                st_out.borrow_mut().hovered_panel = false;
                sync_panel_visibility(&st_out, &win_out, &sen_out, &bar_out, true);
            });
            bar.add_controller(motion);
        }
        {
            let motion = EventControllerMotion::new();
            let st_in = Rc::clone(&hide_state);
            let win_in = window.clone();
            let sen_in = sensor_window.clone();
            let bar_in = bar.clone();
            motion.connect_enter(move |_, _, _| {
                st_in.borrow_mut().hovered_sensor = true;
                sync_panel_visibility(&st_in, &win_in, &sen_in, &bar_in, false);
            });
            let st_out = Rc::clone(&hide_state);
            let win_out = window.clone();
            let sen_out = sensor_window.clone();
            let bar_out = bar.clone();
            motion.connect_leave(move |_| {
                st_out.borrow_mut().hovered_sensor = false;
                sync_panel_visibility(&st_out, &win_out, &sen_out, &bar_out, true);
            });
            sensor_strip.add_controller(motion);
        }

        window.set_child(Some(&bar));
        window.present();

        Self {
            window,
            sensor_window,
            bar,
            activities_btn,
            workspaces_box,
            focused_label,
            rec_btn,
            rec_label,
            on_stop_recording,
            tray_box,
            tray_sep,
            clock_btn,
            clock_label,
            clock_24h,
            power_btn,
            power_icon,
            power_pct_label,
            cc_btn,
            cc_icon_area,
            show_system_tray: Cell::new(true),
            is_light_theme,
            drawer_bg_hidden,
            notif_dot,
            last_ws_sig: RefCell::new(Vec::new()),
            tray_items: RefCell::new(Vec::new()),
            pending_tray_menu: Rc::new(RefCell::new(None)),
            hide_state,
            tray_tx,
            context_menu,
            ipc,
        }
    }

    /// Binds the callback invoked when the user clicks the red Top Bar recording stop pill.
    pub fn set_on_stop_recording<F: Fn() + 'static>(&self, cb: F) {
        *self.on_stop_recording.borrow_mut() = Some(Box::new(cb));
    }

    /// Shows or updates the live red Screen Recording indicator pill in the Top Bar.
    pub fn set_recording_status(&self, active: bool, elapsed_secs: u64) {
        if active {
            let mins = elapsed_secs / 60;
            let secs = elapsed_secs % 60;
            self.rec_label.set_text(&format!("{mins:02}:{secs:02}  Stop"));
            self.rec_btn.set_visible(true);
        } else {
            self.rec_btn.set_visible(false);
        }
    }

    /// Updates the live System Tray (`StatusNotifierItem`) icons in the Top Bar.
    pub fn update_tray_items(&self, items: Vec<TrayItem>) {
        if *self.tray_items.borrow() == items {
            return;
        }
        *self.tray_items.borrow_mut() = items.clone();

        while let Some(child) = self.tray_box.first_child() {
            self.tray_box.remove(&child);
        }

        let has_items = !items.is_empty() && self.show_system_tray.get();
        self.tray_box.set_visible(has_items);
        self.tray_sep.set_visible(has_items);

        for item in items {
            let icon = build_tray_icon(&item);
            let btn = Button::new();
            btn.add_css_class("neon-tray-btn");
            btn.add_css_class("neon-tray-app-btn");
            btn.set_child(Some(&icon));
            btn.set_tooltip_text(Some(item.display_title()));

            let compute_anchor_x = {
                let bar_c = self.bar.clone();
                let btn_c = btn.clone();
                move || {
                    btn_c
                        .compute_bounds(&bar_c)
                        .map(|rect| rect.x().round() as i32)
                        .unwrap_or(1100)
                }
            };

            // Left-click: Activate or open menu if ItemIsMenu.
            {
                let item_c = item.clone();
                let tx = self.tray_tx.clone();
                let pending = Rc::clone(&self.pending_tray_menu);
                let ctx = self.context_menu.clone();
                let anchor_fn = compute_anchor_x.clone();
                btn.connect_clicked(move |_| {
                    let x = anchor_fn();
                    if item_c.item_is_menu
                        && let Some(ref mpath) = item_c.menu_path
                    {
                        *pending.borrow_mut() = Some(PendingTrayMenu {
                            key: item_c.key.clone(),
                            app_title: item_c.display_title().to_string(),
                            object_path: item_c.object_path.clone(),
                            anchor_x: x,
                        });
                        tx.send(TrayAction::FetchMenu {
                            key: item_c.key.clone(),
                            bus_name: item_c.bus_name.clone(),
                            menu_path: mpath.clone(),
                        });
                    } else {
                        ctx.close();
                        tx.send(TrayAction::Activate {
                            bus_name: item_c.bus_name.clone(),
                            object_path: item_c.object_path.clone(),
                            x,
                            y: 30,
                        });
                    }
                });
            }

            // Right-click: Open native DBusMenu.
            let rc = GestureClick::new();
            rc.set_button(3);
            {
                let item_c = item.clone();
                let tx = self.tray_tx.clone();
                let pending = Rc::clone(&self.pending_tray_menu);
                let ctx = self.context_menu.clone();
                let anchor_fn = compute_anchor_x.clone();
                rc.connect_pressed(move |gesture, _, _, _| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    let x = anchor_fn();
                    if let Some(ref mpath) = item_c.menu_path {
                        *pending.borrow_mut() = Some(PendingTrayMenu {
                            key: item_c.key.clone(),
                            app_title: item_c.display_title().to_string(),
                            object_path: item_c.object_path.clone(),
                            anchor_x: x,
                        });
                        tx.send(TrayAction::FetchMenu {
                            key: item_c.key.clone(),
                            bus_name: item_c.bus_name.clone(),
                            menu_path: mpath.clone(),
                        });
                    } else {
                        ctx.open_tray_menu(
                            x,
                            34,
                            item_c.display_title(),
                            &item_c.bus_name,
                            &item_c.object_path,
                            None,
                            &[],
                            tx.clone(),
                        );
                    }
                });
            }
            btn.add_controller(rc);

            // Middle-click: SecondaryActivate.
            let mc = GestureClick::new();
            mc.set_button(2);
            {
                let item_c = item.clone();
                let tx = self.tray_tx.clone();
                let anchor_fn = compute_anchor_x;
                mc.connect_pressed(move |gesture, _, _, _| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    tx.send(TrayAction::SecondaryActivate {
                        bus_name: item_c.bus_name.clone(),
                        object_path: item_c.object_path.clone(),
                        x: anchor_fn(),
                        y: 30,
                    });
                });
            }
            btn.add_controller(mc);

            self.tray_box.append(&btn);
        }
    }

    /// Displays the fetched `com.canonical.dbusmenu` popup when the tray worker responds.
    pub fn handle_tray_menu_loaded(
        &self,
        key: String,
        bus_name: String,
        menu_path: String,
        items: Vec<TrayMenuItem>,
    ) {
        let pending = self.pending_tray_menu.borrow_mut().take();
        let Some(req) = pending.filter(|p| p.key == key) else {
            return;
        };
        self.context_menu.open_tray_menu(
            req.anchor_x,
            34,
            &req.app_title,
            &bus_name,
            &req.object_path,
            Some(&menu_path),
            &items,
            self.tray_tx.clone(),
        );
    }

    /// Elevates or restores the Top Bar's Wayland layer so it floats cleanly above the
    /// fullscreen App Drawer (`Layer::Overlay` while open, `Layer::Top` normally).
    pub fn set_drawer_layer_elevated(&self, elevated: bool) {
        if gtk4_layer_shell::is_supported() {
            self.window.set_layer(if elevated {
                Layer::Overlay
            } else {
                Layer::Top
            });
        }
        self.bar.queue_draw();
    }

    /// Smoothly fades out or fades back in the Top Bar background while keeping icons floating.
    pub fn set_drawer_bg_hidden(&self, hidden: bool) {
        self.drawer_bg_hidden.set(hidden);
        if hidden {
            self.bar.add_css_class("drawer-open");
        } else {
            self.bar.remove_css_class("drawer-open");
        }
        self.cc_icon_area.queue_draw();
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        let light =
            snapshot.appearance.theme == "neon_light" || snapshot.appearance.theme == "light";
        if self.is_light_theme.get() != light {
            self.is_light_theme.set(light);
            self.cc_icon_area.queue_draw();
        }

        // Compute fullscreen & smart-hide overlap on active workspace.
        let active_ws_ids: HashSet<u64> = snapshot
            .workspaces
            .iter()
            .find(|ws| ws.active)
            .map(|ws| ws.window_ids.iter().copied().collect())
            .unwrap_or_default();

        let mut has_fullscreen = false;
        let mut window_overlaps_panel = false;
        for w in &snapshot.windows {
            if w.minimized {
                continue;
            }
            if !active_ws_ids.is_empty() && !active_ws_ids.contains(&w.id) {
                continue;
            }
            if w.fullscreen {
                has_fullscreen = true;
                window_overlaps_panel = true;
            } else if w.maximized || w.y < 34 {
                window_overlaps_panel = true;
            }
        }

        {
            let mut st = self.hide_state.borrow_mut();
            st.panel_visible = snapshot.appearance.panel_visible;
            st.smart_hide = snapshot.appearance.panel_smart_hide;
            st.has_fullscreen = has_fullscreen;
            st.overview_open = snapshot.overview_open;
            st.window_overlaps_panel = window_overlaps_panel;
        }
        sync_panel_visibility(
            &self.hide_state,
            &self.window,
            &self.sensor_window,
            &self.bar,
            false,
        );

        self.clock_btn.set_visible(snapshot.appearance.show_clock);
        self.clock_label.set_visible(snapshot.appearance.show_clock);
        if self.clock_24h.get() != snapshot.appearance.clock_24h {
            self.clock_24h.set(snapshot.appearance.clock_24h);
            self.clock_label
                .set_text(&format_clock_now(snapshot.appearance.clock_24h));
        }
        self.workspaces_box
            .set_visible(snapshot.appearance.show_workspaces);
        self.focused_label
            .set_visible(snapshot.appearance.show_app_title);
        self.power_btn.set_visible(snapshot.appearance.show_battery);
        self.cc_btn
            .set_visible(snapshot.appearance.show_quick_settings);
        self.show_system_tray
            .set(snapshot.appearance.show_system_tray);
        let show_tray_now =
            snapshot.appearance.show_system_tray && !self.tray_items.borrow().is_empty();
        self.tray_box.set_visible(show_tray_now);
        self.tray_sep.set_visible(show_tray_now);

        if snapshot.overview_open {
            self.activities_btn.add_css_class("active");
        } else {
            self.activities_btn.remove_css_class("active");
        }

        let ws_sig: Vec<(usize, bool, usize)> = snapshot
            .workspaces
            .iter()
            .map(|ws| (ws.index, ws.active, ws.window_ids.len()))
            .collect();
        if *self.last_ws_sig.borrow() != ws_sig {
            *self.last_ws_sig.borrow_mut() = ws_sig;
            while let Some(child) = self.workspaces_box.first_child() {
                self.workspaces_box.remove(&child);
            }

            for ws in &snapshot.workspaces {
                let dot = Button::new();
                dot.add_css_class("neon-ws-dot");
                if !ws.window_ids.is_empty() {
                    dot.add_css_class("occupied");
                }
                if ws.active {
                    dot.add_css_class("active");
                }
                dot.set_tooltip_text(Some(&format!(
                    "Workspace {} ({} windows)",
                    ws.index + 1,
                    ws.window_ids.len()
                )));
                let idx = ws.index;
                let ipc = self.ipc.clone();
                dot.connect_clicked(move |_| {
                    ipc.send(IpcRequest::SwitchWorkspace { index: idx });
                });
                self.workspaces_box.append(&dot);
            }
        }

        let title = snapshot
            .windows
            .iter()
            .find(|w| w.focused)
            .and_then(|w| w.title.as_deref().or(w.app_id.as_deref()))
            .unwrap_or("Desktop");
        self.focused_label.set_text(title);
    }

    pub fn update_system(&self, sys: &SystemSnapshot, unread_notifications: usize) {
        if sys.power.has_battery {
            let pct = sys.power.battery_percentage.round() as u32;
            let charging = matches!(
                sys.power.battery_state,
                BatteryState::Charging | BatteryState::PendingCharge
            );
            let bat_icon = match (pct, charging) {
                (90..=100, true) => "battery-full-charging-symbolic",
                (90..=100, false) => "battery-full-symbolic",
                (50..=89, true) => "battery-good-charging-symbolic",
                (50..=89, false) => "battery-good-symbolic",
                (20..=49, true) => "battery-low-charging-symbolic",
                (20..=49, false) => "battery-low-symbolic",
                (_, true) => "battery-caution-charging-symbolic",
                (_, false) => "battery-caution-symbolic",
            };
            self.power_icon.set_icon_name(Some(bat_icon));
            self.power_pct_label.set_text(&format!("{pct}%"));
            self.power_pct_label.set_visible(true);
            self.power_btn.set_tooltip_text(Some(&format!(
                "Battery: {pct}% ({}) · Profile: {}",
                sys.power.battery_state.label(),
                sys.power.active_profile.as_deref().unwrap_or("default")
            )));
        } else {
            self.power_icon.set_icon_name(Some("ac-adapter-symbolic"));
            self.power_pct_label.set_visible(false);
            self.power_btn.set_tooltip_text(Some("AC Power"));
        }

        self.notif_dot.set_visible(unread_notifications > 0);
    }
}

fn sync_panel_visibility(
    state_rc: &Rc<RefCell<PanelHideState>>,
    window: &ApplicationWindow,
    sensor_window: &ApplicationWindow,
    bar: &CenterBox,
    delay_hide: bool,
) {
    let (want_auto_exclusive, should_show, enable_sensor, immediate_hide) = {
        let st = state_rc.borrow();
        if !st.panel_visible || (st.has_fullscreen && !st.overview_open) {
            (false, false, false, true)
        } else if !st.smart_hide {
            (true, true, false, false)
        } else {
            let show = st.overview_open
                || st.popup_open
                || st.hovered_panel
                || st.hovered_sensor
                || !st.window_overlaps_panel;
            (false, show, !show, false)
        }
    };

    if gtk4_layer_shell::is_supported() {
        let prev = state_rc.borrow().auto_exclusive_active;
        if prev != want_auto_exclusive {
            state_rc.borrow_mut().auto_exclusive_active = want_auto_exclusive;
            if want_auto_exclusive {
                let h = window.height().max(31);
                window.set_exclusive_zone(h);
            } else {
                window.set_exclusive_zone(0);
            }
        }
    }

    if immediate_hide {
        let mut st = state_rc.borrow_mut();
        st.anim_gen = st.anim_gen.wrapping_add(1);
        st.currently_shown = false;
        drop(st);
        sensor_window.set_visible(false);
        bar.add_css_class("panel-hidden");
        window.set_visible(false);
        return;
    }

    if should_show {
        let was_shown = {
            let mut st = state_rc.borrow_mut();
            st.anim_gen = st.anim_gen.wrapping_add(1);
            let prev = st.currently_shown;
            st.currently_shown = true;
            prev
        };
        sensor_window.set_visible(false);
        if !was_shown || !window.is_visible() {
            bar.add_css_class("panel-hidden");
            window.set_visible(true);
            window.present();
            let bar_c = bar.clone();
            let st_c = Rc::clone(state_rc);
            let epoch = state_rc.borrow().anim_gen;
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                if st_c.borrow().anim_gen == epoch {
                    bar_c.remove_css_class("panel-hidden");
                }
            });
        } else {
            bar.remove_css_class("panel-hidden");
        }
        return;
    }

    let epoch = {
        let mut st = state_rc.borrow_mut();
        if !st.currently_shown && !window.is_visible() {
            if sensor_window.is_visible() != enable_sensor {
                sensor_window.set_visible(enable_sensor);
                if enable_sensor {
                    sensor_window.present();
                }
            }
            return;
        }
        st.anim_gen = st.anim_gen.wrapping_add(1);
        st.anim_gen
    };

    let leave_wait = if delay_hide { 360 } else { 0 };
    let st_c = Rc::clone(state_rc);
    let win_c = window.clone();
    let sen_c = sensor_window.clone();
    let bar_c = bar.clone();

    glib::timeout_add_local_once(Duration::from_millis(leave_wait), move || {
        if st_c.borrow().anim_gen != epoch {
            return;
        }
        bar_c.add_css_class("panel-hidden");
        if enable_sensor {
            sen_c.set_visible(true);
            sen_c.present();
        }
        let st_done = Rc::clone(&st_c);
        let win_done = win_c.clone();
        glib::timeout_add_local_once(Duration::from_millis(200), move || {
            if st_done.borrow().anim_gen == epoch {
                st_done.borrow_mut().currently_shown = false;
                win_done.set_visible(false);
            }
        });
    });
}

fn build_tray_icon(item: &TrayItem) -> Image {
    let img = Image::new();
    img.set_pixel_size(16);

    if let Some(ref pix) = item.icon_rgba {
        let bytes = glib::Bytes::from(&pix.rgba);
        let tex = gdk::MemoryTexture::new(
            pix.width,
            pix.height,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            (pix.width as usize) * 4,
        );
        img.set_paintable(Some(&tex));
        return img;
    }

    if let Some(ref path) = item.icon_file
        && let Ok(tex) = gdk::Texture::from_file(&gio::File::for_path(path))
    {
        img.set_paintable(Some(&tex));
        return img;
    }

    if let Some(ref name) = item.icon_name
        && !name.is_empty()
    {
        img.set_icon_name(Some(name));
        return img;
    }

    img.set_icon_name(Some("application-x-executable-symbolic"));
    img
}

fn format_clock_now(clock_24h: bool) -> String {
    let fmt = if clock_24h {
        "%a %b %-e  %H:%M"
    } else {
        "%a %b %-e  %-I:%M %p"
    };
    glib::DateTime::now_local()
        .and_then(|dt| dt.format(fmt))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "NEON DE".to_string())
}

pub fn launch_sibling_binary(name: &str) {
    launch_sibling_binary_with_args(name, &[]);
}

pub fn launch_sibling_binary_with_args(name: &str, args: &[&str]) {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(name);
        if sibling.exists() {
            let _ = std::process::Command::new(sibling).args(args).spawn();
            return;
        }
    }
    let _ = std::process::Command::new(name).args(args).spawn();
}
