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
    bluetooth::BluetoothStatus,
    network::{ConnectionKind, NetworkStatus},
    power::BatteryState,
    tray::{TrayAction, TrayActionSender, TrayItem, TrayMenuItem},
};

use crate::{
    context_menu::{ContextMenuOverlay, PanelMenuZone},
    ipc_bridge::IpcCommandSender,
    notification_center::NotificationCenterPanel,
    quick_settings::QuickSettingsPanel,
    workspaces_panel::WorkspacesPanel,
};

#[derive(Clone)]
pub(crate) struct PendingTrayMenu {
    pub key: String,
    pub app_title: String,
    pub object_path: String,
    pub anchor_x: i32,
}

#[derive(Default)]
struct PanelHideState {
    panel_visible: bool,
    combined_mode: bool,
    smart_hide: bool,
    has_fullscreen: bool,
    overview_open: bool,
    workspaces_open: bool,
    qs_open: bool,
    nc_open: bool,
    window_overlaps_panel: bool,
    hovered_panel: bool,
    hovered_sensor: bool,
    currently_shown: bool,
    auto_exclusive_active: bool,
    anim_gen: u64,
}

type StopRecordingCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NetworkIconVisual {
    pub kind: ConnectionKind,
    pub strength: u8,
}

impl Default for NetworkIconVisual {
    fn default() -> Self {
        Self {
            kind: ConnectionKind::Wifi,
            strength: 100,
        }
    }
}

/// Live handles to updatable widgets in the NEON DE Top Menu Bar.
pub struct TopPanel {
    window: ApplicationWindow,
    sensor_window: ApplicationWindow,
    bar: CenterBox,
    activities_btn: Button,
    ws_icon_area: DrawingArea,
    workspaces_box: GtkBox,
    rec_btn: Button,
    rec_label: Label,
    on_stop_recording: StopRecordingCallback,
    tray_box: GtkBox,
    tray_sep: Separator,
    clock_btn: Button,
    clock_label: Label,
    clock_24h: Rc<Cell<bool>>,
    bt_btn: Button,
    bt_icon_area: DrawingArea,
    wifi_btn: Button,
    wifi_icon_area: DrawingArea,
    wifi_visual: Rc<Cell<NetworkIconVisual>>,
    power_btn: Button,
    power_icon: Image,
    power_pct_label: Label,
    cc_btn: Button,
    cc_icon_area: DrawingArea,
    show_system_tray: Cell<bool>,
    is_light_theme: Rc<Cell<bool>>,
    drawer_bg_hidden: Rc<Cell<bool>>,
    last_layer_elevated: Cell<bool>,
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
        notification_center: NotificationCenterPanel,
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

        // Left: Windows-style Task View / Workspaces icon button + workspace dots
        let left_box = GtkBox::new(Orientation::Horizontal, 8);
        left_box.set_valign(Align::Center);

        let is_light_theme = Rc::new(Cell::new(false));
        let drawer_bg_hidden = Rc::new(Cell::new(false));

        let ws_icon_area = DrawingArea::new();
        ws_icon_area.set_content_width(20);
        ws_icon_area.set_content_height(16);
        ws_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden);
            ws_icon_area.set_draw_func(move |_, cr, w, h| {
                draw_workspaces_taskview_icon(cr, w, h, light_flag.get(), drawer_flag.get());
            });
        }
        let activities_btn = Button::new();
        activities_btn.add_css_class("neon-ws-taskview-btn");
        activities_btn.set_child(Some(&ws_icon_area));
        activities_btn.set_tooltip_text(Some(
            "Task View & Workspaces (Switch with Super+1–0 or drag windows between Desks)",
        ));
        {
            let btn_c = activities_btn.clone();
            let st_c = Rc::clone(&hide_state);
            let win_c = window.clone();
            let sen_c = sensor_window.clone();
            let bar_c = bar.clone();
            workspaces_panel.add_on_state_change(move |open| {
                if open {
                    btn_c.add_css_class("active");
                } else {
                    btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().workspaces_open = open;
                sync_panel_visibility(&st_c, &win_c, &sen_c, &bar_c, false);
            });
        }
        {
            let ws_panel = workspaces_panel.clone();
            let qs_close = quick_settings.clone();
            let nc_close = notification_center.clone();
            activities_btn.connect_clicked(move |_| {
                qs_close.close_immediate();
                nc_close.close_immediate();
                ws_panel.toggle();
            });
        }

        let workspaces_box = GtkBox::new(Orientation::Horizontal, 4);
        workspaces_box.set_valign(Align::Center);

        left_box.append(&activities_btn);
        left_box.append(&workspaces_box);

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

        let bt_icon_area = DrawingArea::new();
        bt_icon_area.set_content_width(15);
        bt_icon_area.set_content_height(15);
        bt_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden);
            bt_icon_area.set_draw_func(move |_, cr, w, h| {
                draw_bluetooth_status_icon(cr, w, h, light_flag.get(), drawer_flag.get());
            });
        }
        let bt_btn = Button::new();
        bt_btn.add_css_class("neon-tray-btn");
        bt_btn.add_css_class("neon-status-icon-btn");
        bt_btn.set_child(Some(&bt_icon_area));
        bt_btn.set_tooltip_text(Some("Bluetooth Connected"));
        bt_btn.set_visible(false);
        {
            let qs = quick_settings.clone();
            bt_btn.connect_clicked(move |_| qs.toggle());
        }

        let wifi_visual = Rc::new(Cell::new(NetworkIconVisual::default()));
        let wifi_icon_area = DrawingArea::new();
        wifi_icon_area.set_content_width(18);
        wifi_icon_area.set_content_height(15);
        wifi_icon_area.set_valign(Align::Center);
        {
            let vis_flag = Rc::clone(&wifi_visual);
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden);
            wifi_icon_area.set_draw_func(move |_, cr, w, h| {
                let v = vis_flag.get();
                draw_network_status_icon(cr, w, h, v.kind, v.strength, light_flag.get(), drawer_flag.get());
            });
        }
        let wifi_btn = Button::new();
        wifi_btn.add_css_class("neon-tray-btn");
        wifi_btn.add_css_class("neon-status-icon-btn");
        wifi_btn.set_child(Some(&wifi_icon_area));
        wifi_btn.set_tooltip_text(Some("Wi-Fi Connected"));
        wifi_btn.set_visible(false);
        {
            let qs = quick_settings.clone();
            wifi_btn.connect_clicked(move |_| qs.toggle());
        }

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

        // Authentic macOS Control Center dual-toggle-switches menu bar icon
        let cc_icon_area = DrawingArea::new();
        cc_icon_area.set_content_width(18);
        cc_icon_area.set_content_height(15);
        cc_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden);
            cc_icon_area.set_draw_func(move |_, cr, w, h| {
                draw_control_center_icon(cr, w, h, light_flag.get(), drawer_flag.get());
            });
        }

        let cc_btn = Button::new();
        cc_btn.add_css_class("neon-tray-btn");
        cc_btn.add_css_class("neon-cc-toggle-btn");
        cc_btn.set_child(Some(&cc_icon_area));
        cc_btn.set_tooltip_text(Some("Quick Settings"));
        {
            let cc_btn_c = cc_btn.clone();
            let ws_close = workspaces_panel.clone();
            let st_c = Rc::clone(&hide_state);
            let win_c = window.clone();
            let sen_c = sensor_window.clone();
            let bar_c = bar.clone();
            quick_settings.add_on_state_change(move |open| {
                if open {
                    ws_close.close_immediate();
                    cc_btn_c.add_css_class("active");
                } else {
                    cc_btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().qs_open = open;
                sync_panel_visibility(&st_c, &win_c, &sen_c, &bar_c, !open);
            });
        }
        {
            let qs = quick_settings.clone();
            cc_btn.connect_clicked(move |_| qs.toggle());
        }

        let clock_24h = Rc::new(Cell::new(true));
        let clock_inner = GtkBox::new(Orientation::Horizontal, 6);
        clock_inner.set_valign(Align::Center);
        let clock_label = Label::new(Some(&format_clock_now(clock_24h.get())));
        let notif_dot = GtkBox::new(Orientation::Horizontal, 0);
        notif_dot.add_css_class("neon-notif-dot");
        notif_dot.set_valign(Align::Center);
        notif_dot.set_visible(false);
        clock_inner.append(&clock_label);
        clock_inner.append(&notif_dot);

        let clock_btn = Button::new();
        clock_btn.add_css_class("neon-clock-btn");
        clock_btn.set_child(Some(&clock_inner));
        clock_btn.set_tooltip_text(Some("Notifications & Calendar"));
        {
            let clk_btn_c = clock_btn.clone();
            let ws_close = workspaces_panel;
            let st_c = Rc::clone(&hide_state);
            let win_c = window.clone();
            let sen_c = sensor_window.clone();
            let bar_c = bar.clone();
            notification_center.add_on_state_change(move |open| {
                if open {
                    ws_close.close_immediate();
                    clk_btn_c.add_css_class("active");
                } else {
                    clk_btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().nc_open = open;
                sync_panel_visibility(&st_c, &win_c, &sen_c, &bar_c, !open);
            });
        }
        {
            let nc = notification_center;
            clock_btn.connect_clicked(move |_| nc.toggle());
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
        right_box.append(&bt_btn);
        right_box.append(&wifi_btn);
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
            let bt_btn_c = bt_btn.clone();
            let wifi_btn_c = wifi_btn.clone();
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
                    } else if is_in(cc_btn_c.upcast_ref())
                        || is_in(bt_btn_c.upcast_ref())
                        || is_in(wifi_btn_c.upcast_ref())
                    {
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
            ws_icon_area,
            workspaces_box,
            rec_btn,
            rec_label,
            on_stop_recording,
            tray_box,
            tray_sep,
            clock_btn,
            clock_label,
            clock_24h,
            bt_btn,
            bt_icon_area,
            wifi_btn,
            wifi_icon_area,
            wifi_visual,
            power_btn,
            power_icon,
            power_pct_label,
            cc_btn,
            cc_icon_area,
            show_system_tray: Cell::new(true),
            is_light_theme,
            drawer_bg_hidden,
            last_layer_elevated: Cell::new(false),
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
        if self.hide_state.borrow().combined_mode {
            return;
        }
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
        if self.last_layer_elevated.get() == elevated {
            return;
        }
        self.last_layer_elevated.set(elevated);
        if gtk4_layer_shell::is_supported() {
            self.window.set_layer(if elevated {
                Layer::Overlay
            } else {
                Layer::Top
            });
        }
        self.window.queue_draw();
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
        self.ws_icon_area.queue_draw();
        self.wifi_icon_area.queue_draw();
        self.bt_icon_area.queue_draw();
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        let light =
            snapshot.appearance.theme == "neon_light" || snapshot.appearance.theme == "light";
        if self.is_light_theme.get() != light {
            self.is_light_theme.set(light);
            self.cc_icon_area.queue_draw();
            self.ws_icon_area.queue_draw();
            self.wifi_icon_area.queue_draw();
            self.bt_icon_area.queue_draw();
        }

        // Compute fullscreen & smart-hide overlap strictly on the active workspace (an empty
        // active workspace must NEVER inherit fullscreen/overlap state from another workspace).
        let (has_fullscreen, window_overlaps_panel) = compute_panel_workspace_overlap(snapshot);
        let combined_mode = snapshot
            .appearance
            .panel_mode
            .eq_ignore_ascii_case("combined_bottom");

        {
            let mut st = self.hide_state.borrow_mut();
            st.panel_visible = snapshot.appearance.panel_visible;
            st.combined_mode = combined_mode;
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
        self.activities_btn
            .set_visible(snapshot.appearance.show_workspaces);
        self.power_btn.set_visible(snapshot.appearance.show_battery);
        self.cc_btn
            .set_visible(snapshot.appearance.show_quick_settings);
        self.show_system_tray
            .set(snapshot.appearance.show_system_tray);
        let show_tray_now =
            snapshot.appearance.show_system_tray && !self.tray_items.borrow().is_empty();
        self.tray_box.set_visible(show_tray_now);
        self.tray_sep.set_visible(show_tray_now);

        if snapshot.overview_open || self.hide_state.borrow().workspaces_open {
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
    }

    pub fn update_system(&self, sys: &SystemSnapshot, unread_notifications: usize) {
        if let Some((_, tooltip)) = bluetooth_status_icon_info(&sys.bluetooth) {
            self.bt_btn.set_tooltip_text(Some(&tooltip));
            self.bt_btn.set_visible(true);
            self.bt_icon_area.queue_draw();
        } else {
            self.bt_btn.set_visible(false);
        }

        if let Some((vis, tooltip)) = network_status_visual(&sys.network) {
            let changed = self.wifi_visual.get() != vis;
            self.wifi_visual.set(vis);
            self.wifi_btn.set_tooltip_text(Some(&tooltip));
            self.wifi_btn.set_visible(true);
            if changed {
                self.wifi_icon_area.queue_draw();
            }
        } else {
            self.wifi_btn.set_visible(false);
        }

        if sys.power.has_battery {
            let pct = sys.power.battery_percentage.round() as u32;
            let charging = matches!(
                sys.power.battery_state,
                BatteryState::Charging | BatteryState::PendingCharge
            );
            let bat_icon = battery_icon_name(pct, charging);
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

pub(crate) fn wifi_status_icon_info(net: &NetworkStatus) -> Option<(&'static str, String)> {
    if !net.available || !net.connectivity.is_connected() {
        return None;
    }
    let kind = if net.primary_kind == ConnectionKind::None
        && net.wifi_enabled
        && net.wifi_strength.is_some()
    {
        ConnectionKind::Wifi
    } else {
        net.primary_kind
    };
    match kind {
        ConnectionKind::Wifi => {
            let icon = match net.wifi_strength {
                Some(80..=100) => "network-wireless-signal-excellent-symbolic",
                Some(55..=79) => "network-wireless-signal-good-symbolic",
                Some(30..=54) => "network-wireless-signal-ok-symbolic",
                Some(1..=29) => "network-wireless-signal-weak-symbolic",
                _ => "network-wireless-symbolic",
            };
            let tip = match (&net.primary_name, net.wifi_strength) {
                (Some(name), Some(pct)) => format!("Wi-Fi: {name} ({pct}%)"),
                (Some(name), None) => format!("Wi-Fi: {name}"),
                (None, Some(pct)) => format!("Wi-Fi Connected ({pct}%)"),
                (None, None) => "Wi-Fi Connected".to_string(),
            };
            Some((icon, tip))
        }
        ConnectionKind::Ethernet => {
            let tip = net
                .primary_name
                .as_deref()
                .map(|n| format!("Ethernet: {n}"))
                .unwrap_or_else(|| "Wired Connected".to_string());
            Some(("network-wired-symbolic", tip))
        }
        ConnectionKind::Vpn => {
            let tip = net
                .primary_name
                .as_deref()
                .map(|n| format!("VPN: {n}"))
                .unwrap_or_else(|| "VPN Connected".to_string());
            Some(("network-vpn-symbolic", tip))
        }
        ConnectionKind::Other => {
            let tip = net
                .primary_name
                .as_deref()
                .map(|n| format!("Network: {n}"))
                .unwrap_or_else(|| "Network Connected".to_string());
            Some(("network-transmit-receive-symbolic", tip))
        }
        ConnectionKind::None => None,
    }
}

pub(crate) fn bluetooth_status_icon_info(bt: &BluetoothStatus) -> Option<(&'static str, String)> {
    if !bt.available || !bt.powered {
        return None;
    }
    let connected_names: Vec<&str> = bt
        .devices
        .iter()
        .filter(|d| d.connected)
        .map(|d| d.name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    if bt.connected_count() == 0 {
        return None;
    }
    let tip = if connected_names.is_empty() {
        "Bluetooth Connected".to_string()
    } else {
        format!("Bluetooth: {}", connected_names.join(", "))
    };
    Some(("bluetooth-active-symbolic", tip))
}

pub(crate) fn network_status_visual(net: &NetworkStatus) -> Option<(NetworkIconVisual, String)> {
    let (_, tip) = wifi_status_icon_info(net)?;
    let kind = if net.primary_kind == ConnectionKind::None
        && net.wifi_enabled
        && net.wifi_strength.is_some()
    {
        ConnectionKind::Wifi
    } else {
        net.primary_kind
    };
    let strength = net.wifi_strength.unwrap_or(100).min(100);
    Some((NetworkIconVisual { kind, strength }, tip))
}

/// Crisp anti-aliased Cairo vector Network status icon (Wi-Fi signal arcs, Ethernet, or VPN).
pub(crate) fn draw_network_status_icon(
    cr: &gtk4::cairo::Context,
    w: i32,
    h: i32,
    kind: ConnectionKind,
    strength: u8,
    light: bool,
    drawer_hidden: bool,
) {
    let (r, g, b) = if light && !drawer_hidden {
        (0.06, 0.09, 0.16)
    } else {
        (0.95, 0.97, 1.0)
    };
    let wf = f64::from(w);
    let hf = f64::from(h);
    let sx = (wf / 18.0).min(hf / 15.0);
    let ox = (wf - 18.0 * sx) * 0.5;
    let oy = (hf - 15.0 * sx) * 0.5;
    let _ = cr.save();
    cr.set_antialias(gtk4::cairo::Antialias::Best);
    cr.translate(ox, oy);
    cr.scale(sx, sx);

    match kind {
        ConnectionKind::Wifi => {
            let cx = 9.0;
            let cy = 12.6;
            let a_start = -PI * 0.74;
            let a_end = -PI * 0.26;
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            cr.set_line_width(1.65);

            // Outer arc (>= 68% strength)
            cr.set_source_rgba(r, g, b, if strength >= 68 { 0.96 } else { 0.28 });
            cr.new_sub_path();
            cr.arc(cx, cy, 10.2, a_start, a_end);
            let _ = cr.stroke();

            // Middle arc (>= 36% strength)
            cr.set_source_rgba(r, g, b, if strength >= 36 { 0.96 } else { 0.28 });
            cr.new_sub_path();
            cr.arc(cx, cy, 7.0, a_start, a_end);
            let _ = cr.stroke();

            // Inner arc (connected)
            cr.set_source_rgba(r, g, b, if strength >= 1 { 0.96 } else { 0.36 });
            cr.new_sub_path();
            cr.arc(cx, cy, 3.85, a_start, a_end);
            let _ = cr.stroke();

            // Crisp base dot
            cr.set_source_rgba(r, g, b, 0.98);
            cr.new_sub_path();
            cr.arc(cx, cy, 1.45, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        ConnectionKind::Ethernet => {
            cr.set_source_rgba(r, g, b, 0.95);
            cr.set_line_width(1.35);
            cr.set_line_join(gtk4::cairo::LineJoin::Round);
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            // Outer port shell
            cr.rectangle(2.5, 2.5, 13.0, 10.0);
            let _ = cr.stroke();
            // Connector pins
            for px in [5.5, 9.0, 12.5] {
                cr.move_to(px, 2.5);
                cr.line_to(px, 6.2);
                let _ = cr.stroke();
            }
            cr.move_to(6.2, 10.0);
            cr.line_to(11.8, 10.0);
            let _ = cr.stroke();
        }
        ConnectionKind::Vpn => {
            cr.set_source_rgba(r, g, b, 0.95);
            cr.set_line_width(1.4);
            cr.set_line_join(gtk4::cairo::LineJoin::Round);
            cr.move_to(9.0, 1.5);
            cr.line_to(15.0, 3.8);
            cr.line_to(14.5, 8.6);
            cr.curve_to(14.2, 11.4, 11.6, 13.0, 9.0, 13.8);
            cr.curve_to(6.4, 13.0, 3.8, 11.4, 3.5, 8.6);
            cr.line_to(3.0, 3.8);
            cr.close_path();
            let _ = cr.stroke();
            cr.arc(9.0, 7.6, 1.55, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        _ => {
            cr.set_source_rgba(r, g, b, 0.95);
            cr.set_line_width(1.45);
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            cr.set_line_join(gtk4::cairo::LineJoin::Round);
            // Up/down transfer arrows
            cr.move_to(6.0, 12.5);
            cr.line_to(6.0, 2.5);
            cr.move_to(3.5, 5.0);
            cr.line_to(6.0, 2.5);
            cr.line_to(8.5, 5.0);
            cr.move_to(12.0, 2.5);
            cr.line_to(12.0, 12.5);
            cr.move_to(9.5, 10.0);
            cr.line_to(12.0, 12.5);
            cr.line_to(14.5, 10.0);
            let _ = cr.stroke();
        }
    }

    let _ = cr.restore();
}

/// Crisp anti-aliased Cairo vector connected-Bluetooth icon (rune + twin active connection dots).
pub(crate) fn draw_bluetooth_status_icon(
    cr: &gtk4::cairo::Context,
    w: i32,
    h: i32,
    light: bool,
    drawer_hidden: bool,
) {
    let (r, g, b) = if light && !drawer_hidden {
        (0.06, 0.09, 0.16)
    } else {
        (0.95, 0.97, 1.0)
    };
    let wf = f64::from(w);
    let hf = f64::from(h);
    let sx = (wf / 15.0).min(hf / 15.0);
    let ox = (wf - 15.0 * sx) * 0.5;
    let oy = (hf - 15.0 * sx) * 0.5;
    let _ = cr.save();
    cr.set_antialias(gtk4::cairo::Antialias::Best);
    cr.translate(ox, oy);
    cr.scale(sx, sx);

    let cx = 7.5;
    cr.set_source_rgba(r, g, b, 0.96);
    cr.set_line_width(1.42);
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    cr.set_line_join(gtk4::cairo::LineJoin::Round);

    // Classic Bluetooth rune
    cr.move_to(cx - 3.0, 4.7);
    cr.line_to(cx + 3.2, 9.9);
    cr.line_to(cx, 13.0);
    cr.line_to(cx, 2.0);
    cr.line_to(cx + 3.2, 5.1);
    cr.line_to(cx - 3.0, 10.3);
    let _ = cr.stroke();

    // Twin connected-device indicator dots (left & right)
    cr.new_sub_path();
    cr.arc(1.8, 7.5, 1.15, 0.0, 2.0 * PI);
    cr.new_sub_path();
    cr.arc(13.2, 7.5, 1.15, 0.0, 2.0 * PI);
    let _ = cr.fill();

    let _ = cr.restore();
}

pub(crate) fn battery_icon_name(pct: u32, charging: bool) -> &'static str {
    match (pct, charging) {
        (90..=100, true) => "battery-full-charging-symbolic",
        (90..=100, false) => "battery-full-symbolic",
        (50..=89, true) => "battery-good-charging-symbolic",
        (50..=89, false) => "battery-good-symbolic",
        (20..=49, true) => "battery-low-charging-symbolic",
        (20..=49, false) => "battery-low-symbolic",
        (_, true) => "battery-caution-charging-symbolic",
        (_, false) => "battery-caution-symbolic",
    }
}

pub(crate) fn draw_workspaces_taskview_icon(
    cr: &gtk4::cairo::Context,
    w: i32,
    h: i32,
    light: bool,
    drawer_hidden: bool,
) {
    let (r, g, b) = if light && !drawer_hidden {
        (0.06, 0.09, 0.16)
    } else {
        (0.95, 0.97, 1.0)
    };
    let w = f64::from(w);
    let h = f64::from(h);
    let sx = (w / 20.0).min(h / 16.0);
    let ox = (w - 20.0 * sx) * 0.5;
    let oy = (h - 16.0 * sx) * 0.5;
    let _ = cr.save();
    cr.set_antialias(gtk4::cairo::Antialias::Best);
    cr.translate(ox, oy);
    cr.scale(sx, sx);

    let rounded_rect = |cr: &gtk4::cairo::Context, x: f64, y: f64, rw: f64, rh: f64, rad: f64| {
        let rad = rad.min(rw * 0.5).min(rh * 0.5);
        cr.new_sub_path();
        cr.arc(x + rw - rad, y + rad, rad, -PI * 0.5, 0.0);
        cr.arc(x + rw - rad, y + rh - rad, rad, 0.0, PI * 0.5);
        cr.arc(x + rad, y + rh - rad, rad, PI * 0.5, PI);
        cr.arc(x + rad, y + rad, rad, PI, PI * 1.5);
        cr.close_path();
    };

    // Back desktop card (top-right offset, Windows 11 Task View style)
    rounded_rect(cr, 7.0, 1.5, 11.2, 8.6, 2.1);
    cr.set_source_rgba(r, g, b, 0.24);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(r, g, b, 0.62);
    cr.set_line_width(1.25);
    let _ = cr.stroke();

    // Front desktop card (bottom-left foreground)
    rounded_rect(cr, 1.8, 5.4, 11.8, 9.0, 2.2);
    cr.set_source_rgba(r, g, b, 0.44);
    let _ = cr.fill_preserve();
    cr.set_source_rgba(r, g, b, 0.96);
    cr.set_line_width(1.4);
    let _ = cr.stroke();

    // Crisp subtle inner mini-bar detail inside front workspace card
    cr.set_source_rgba(r, g, b, 0.82);
    cr.set_line_width(1.2);
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    cr.move_to(4.6, 11.8);
    cr.line_to(10.8, 11.8);
    let _ = cr.stroke();

    let _ = cr.restore();
}

pub(crate) fn draw_control_center_icon(
    cr: &gtk4::cairo::Context,
    w: i32,
    h: i32,
    light: bool,
    drawer_hidden: bool,
) {
    let (r, g, b) = if light && !drawer_hidden {
        (0.06, 0.09, 0.16)
    } else {
        (0.95, 0.97, 1.0)
    };
    cr.set_antialias(gtk4::cairo::Antialias::Best);
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
        let popup_open = st.qs_open || st.nc_open;
        if st.combined_mode || !st.panel_visible || (st.has_fullscreen && !st.overview_open) {
            (false, false, false, true)
        } else if st.workspaces_open && !st.overview_open {
            // Hide the Top Bar visually while fullscreen Mission Control is open, while preserving
            // its exclusive zone so maximized windows underneath never resize/flicker.
            (!st.smart_hide, false, false, true)
        } else if !st.smart_hide {
            (true, true, false, false)
        } else {
            let show = st.overview_open
                || popup_open
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

pub(crate) fn build_tray_icon(item: &TrayItem) -> Image {
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

pub(crate) fn format_clock_now(clock_24h: bool) -> String {
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

pub(crate) fn format_time_only(clock_24h: bool) -> String {
    let fmt = if clock_24h { "%H:%M" } else { "%-I:%M %p" };
    glib::DateTime::now_local()
        .and_then(|dt| dt.format(fmt))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "12:00".to_string())
}

pub(crate) fn format_date_short() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%a, %b %-e"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "Today".to_string())
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

fn compute_panel_workspace_overlap(snapshot: &DesktopSnapshot) -> (bool, bool) {
    let active_ws = snapshot
        .workspaces
        .iter()
        .find(|ws| ws.active)
        .or_else(|| snapshot.workspaces.get(snapshot.active_workspace_index));
    let active_ws_info: Option<(u64, HashSet<u64>)> =
        active_ws.map(|ws| (ws.id, ws.window_ids.iter().copied().collect()));

    let mut has_fullscreen = false;
    let mut window_overlaps_panel = false;
    for w in &snapshot.windows {
        if w.minimized {
            continue;
        }
        if let Some((ws_id, ref ws_win_ids)) = active_ws_info
            && w.workspace_id != ws_id
            && !ws_win_ids.contains(&w.id)
        {
            continue;
        }
        if w.fullscreen {
            has_fullscreen = true;
            window_overlaps_panel = true;
        } else if w.maximized || w.y < 34 {
            window_overlaps_panel = true;
        }
    }
    (has_fullscreen, window_overlaps_panel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use neon_ipc::{WindowInfo, WorkspaceInfo};

    #[test]
    fn empty_active_workspace_ignores_fullscreen_video_on_other_workspace() {
        let fs_video = WindowInfo {
            id: 10,
            workspace_id: 1,
            app_id: Some("google-chrome".into()),
            title: Some("Video".into()),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            focused: false,
            minimized: false,
            maximized: false,
            fullscreen: true,
            tiled: false,
            always_on_top: false,
        };
        let mut snap = DesktopSnapshot {
            active_workspace_index: 1,
            overview_open: false,
            locked: false,
            workspaces: vec![
                WorkspaceInfo {
                    id: 1,
                    index: 0,
                    active: false,
                    pinned: false,
                    window_ids: vec![10],
                },
                WorkspaceInfo {
                    id: 2,
                    index: 1,
                    active: true,
                    pinned: false,
                    window_ids: vec![],
                },
            ],
            windows: vec![fs_video],
            outputs: vec![],
            effects: neon_ipc::EffectsInfo::default(),
            appearance: neon_ipc::AppearanceInfo::default(),
            window_behavior: neon_ipc::WindowBehaviorInfo {
                focus_follows_mouse: false,
                snap_gap: 12,
                button_layout: "left_traffic_lights".into(),
                ..Default::default()
            },
            xwayland_display: None,
        };

        // Empty Workspace 2 must NOT inherit fullscreen or panel overlap from Workspace 1.
        assert_eq!(compute_panel_workspace_overlap(&snap), (false, false));

        // Switching back to Workspace 1 detects the fullscreen video immediately.
        snap.active_workspace_index = 0;
        snap.workspaces[0].active = true;
        snap.workspaces[1].active = false;
        assert_eq!(compute_panel_workspace_overlap(&snap), (true, true));
    }

    #[test]
    fn wifi_status_icon_shows_only_when_connected() {
        use neon_system::network::NetworkConnectivity;

        let mut net = NetworkStatus::default();
        assert!(wifi_status_icon_info(&net).is_none());

        net.available = true;
        net.wifi_enabled = true;
        net.connectivity = NetworkConnectivity::Offline;
        assert!(wifi_status_icon_info(&net).is_none());

        net.connectivity = NetworkConnectivity::ConnectedGlobal;
        net.primary_kind = ConnectionKind::Wifi;
        net.primary_name = Some("HomeWiFi".into());
        net.wifi_strength = Some(86);
        let (icon, tip) = wifi_status_icon_info(&net).expect("connected Wi-Fi shows icon");
        assert_eq!(icon, "network-wireless-signal-excellent-symbolic");
        assert_eq!(tip, "Wi-Fi: HomeWiFi (86%)");

        net.primary_kind = ConnectionKind::Ethernet;
        net.primary_name = Some("Wired 1".into());
        let (eth_icon, eth_tip) =
            wifi_status_icon_info(&net).expect("connected Ethernet shows icon");
        assert_eq!(eth_icon, "network-wired-symbolic");
        assert_eq!(eth_tip, "Ethernet: Wired 1");
    }

    #[test]
    fn bluetooth_status_icon_shows_only_when_device_connected() {
        use neon_system::bluetooth::BluetoothDevice;

        let mut bt = BluetoothStatus {
            available: true,
            powered: true,
            discovering: false,
            adapter_path: Some("/org/bluez/hci0".into()),
            adapter_alias: Some("NEON".into()),
            devices: vec![BluetoothDevice {
                object_path: "/org/bluez/hci0/dev_1".into(),
                name: "AirPods Pro".into(),
                address: "AA:BB:CC:DD:EE:FF".into(),
                paired: true,
                connected: false,
                icon: None,
            }],
        };
        assert!(bluetooth_status_icon_info(&bt).is_none());

        bt.devices[0].connected = true;
        let (icon, tip) =
            bluetooth_status_icon_info(&bt).expect("connected BT device shows icon");
        assert_eq!(icon, "bluetooth-active-symbolic");
        assert_eq!(tip, "Bluetooth: AirPods Pro");

        bt.powered = false;
        assert!(bluetooth_status_icon_info(&bt).is_none());
    }
}
