//! NEON DE macOS-Inspired Glass Floating Dock (`zwlr_layer_shell_v1` `Layer::Top`).
//!
//! Supports live-configurable:
//! - **Position**: Bottom (default), Left, or Right edge — adapts layer anchor, orientation,
//!   edge sensor, and exclusive zone live without restart.
//! - **Icon Size**: Tiny (24px), Compact (30px), Small (36px), Medium (42px), Large (52px) (22–64px).
//! - **Magnification**: Toggleable smooth hover magnification (`dock-magnify`).
//! - **Live Pin / Unpin**: Dynamically syncs pinned favorites from `dock_pinned_apps`.
//! - **Visibility Modes**: Always Visible, Auto-Hide, Smart Hide (Intelligent), and Fullscreen Removal.
//! - **Unified macOS Context Menus**: Rich per-app right-click menu (Open, New Window, XDG App
//!   Actions, Pin/Unpin, Launch at Login, Show All Windows, Move to Workspace, Hide, Quit) and
//!   empty dock menu (Position, Size, Magnification, Auto Hide, Dock Settings).

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, DrawingArea,
    EventControllerMotion, GestureClick, Image, Label, Orientation, Overlay, Separator, glib,
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
    desktop_apps::{
        DesktopApp, clear_icon_cache, load_app_icon, resolve_window_desktop_app,
        select_favorites_with_pins,
    },
    dock_preview::{DockPreviewAnchor, DockPreviewPopup, DockPreviewTarget},
    ipc_bridge::IpcCommandSender,
    notification_center::NotificationCenterPanel,
    panel::{
        NetworkIconVisual, PendingTrayMenu, battery_icon_name, bluetooth_status_icon_info,
        build_tray_icon, draw_bluetooth_status_icon, draw_control_center_icon,
        draw_network_status_icon, draw_workspaces_taskview_icon, format_date_short,
        format_time_only, launch_sibling_binary, network_status_visual,
    },
    quick_settings::QuickSettingsPanel,
    workspaces_panel::WorkspacesPanel,
};

type DockWindowSig = (u64, Option<String>, Option<String>, bool, bool);

const DOCK_MAG_CLASSES: [&str; 8] = [
    "dock-mag-1",
    "dock-mag-2",
    "dock-mag-3",
    "dock-mag-4",
    "dock-mag-5",
    "dock-mag-6",
    "dock-mag-7",
    "dock-mag-8",
];

#[derive(Default)]
struct DockWaveState {
    rest_rel_centers: Vec<(Button, f64)>,
    levels: Vec<u8>,
    pitch: f64,
    vertical: bool,
    dirty: bool,
}

fn truncate_dock_label(raw: &str, max_chars: usize) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let mut s: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
        while s.ends_with(' ') {
            s.pop();
        }
        s.push('…');
        s
    }
}

/// Compact Liquid Glass floating app-title pill rendered cleanly ABOVE bottom dock/panel icons
/// (or beside vertical left/right dock icons) so titles never hide below the screen edge.
#[derive(Clone)]
struct DockLabelPopup {
    window: ApplicationWindow,
    label: Label,
    token: Rc<Cell<u64>>,
}

impl DockLabelPopup {
    fn new(app: &Application) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dock Label")
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("neon-dock-tooltip-window");
        window.set_can_target(false);

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-dock-tooltip"));
            window.set_layer(Layer::Overlay);
            window.set_keyboard_mode(KeyboardMode::None);
            window.set_exclusive_zone(-1);
        }

        // Transparent outer margin ensures the pill's rounded corners and drop shadow
        // render with silky-smooth anti-aliasing and never hard-clip at the surface edge.
        let wrap = GtkBox::new(Orientation::Horizontal, 0);
        wrap.add_css_class("neon-dock-tooltip-wrap");
        wrap.set_halign(Align::Center);
        wrap.set_valign(Align::Center);
        wrap.set_can_target(false);

        let pill = GtkBox::new(Orientation::Horizontal, 0);
        pill.add_css_class("neon-dock-tooltip-pill");
        pill.set_halign(Align::Center);
        pill.set_valign(Align::Center);
        pill.set_can_target(false);

        let label = Label::new(None);
        label.add_css_class("neon-dock-tooltip-label");
        label.set_single_line_mode(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::None);
        label.set_can_target(false);

        pill.append(&label);
        wrap.append(&pill);
        window.set_child(Some(&wrap));
        window.set_visible(false);

        Self {
            window,
            label,
            token: Rc::new(Cell::new(0)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn show(
        &self,
        title: &str,
        screen_x: i32,
        screen_y: i32,
        pos: &str,
        band_px: i32,
        combined: bool,
        out_w: i32,
        out_h: i32,
    ) {
        let display = truncate_dock_label(title, 28);
        if display.is_empty() {
            self.hide_immediate();
            return;
        }
        self.token.set(self.token.get().wrapping_add(1));
        self.label.set_text(&display);

        let (text_w, text_h) = self.label.create_pango_layout(Some(&display)).pixel_size();
        // Pill padding (14px * 2) + transparent wrap padding (14px * 2) + 2px border
        let total_w = (text_w + 58).clamp(72, 340);
        let total_h = (text_h + 32).clamp(44, 64);

        if gtk4_layer_shell::is_supported() {
            match pos {
                "left" => {
                    self.window.set_anchor(Edge::Left, true);
                    self.window.set_anchor(Edge::Top, true);
                    self.window.set_anchor(Edge::Right, false);
                    self.window.set_anchor(Edge::Bottom, false);
                    self.window.set_margin(Edge::Left, (band_px + 4).max(0));
                    self.window.set_margin(
                        Edge::Top,
                        (screen_y - total_h / 2).clamp(4, (out_h - total_h - 4).max(4)),
                    );
                    self.window.set_margin(Edge::Right, 0);
                    self.window.set_margin(Edge::Bottom, 0);
                }
                "right" => {
                    self.window.set_anchor(Edge::Right, true);
                    self.window.set_anchor(Edge::Top, true);
                    self.window.set_anchor(Edge::Left, false);
                    self.window.set_anchor(Edge::Bottom, false);
                    self.window.set_margin(Edge::Right, (band_px + 4).max(0));
                    self.window.set_margin(
                        Edge::Top,
                        (screen_y - total_h / 2).clamp(4, (out_h - total_h - 4).max(4)),
                    );
                    self.window.set_margin(Edge::Left, 0);
                    self.window.set_margin(Edge::Bottom, 0);
                }
                _ => {
                    // Wrap already includes 10px bottom shadow padding, so net visual gap is 8px / 12px.
                    let lift = if combined { 0 } else { 4 };
                    self.window.set_anchor(Edge::Bottom, true);
                    self.window.set_anchor(Edge::Left, true);
                    self.window.set_anchor(Edge::Top, false);
                    self.window.set_anchor(Edge::Right, false);
                    self.window.set_margin(Edge::Bottom, (band_px + lift).max(0));
                    self.window.set_margin(
                        Edge::Left,
                        (screen_x - total_w / 2).clamp(4, (out_w - total_w - 4).max(4)),
                    );
                    self.window.set_margin(Edge::Top, 0);
                    self.window.set_margin(Edge::Right, 0);
                }
            }
        }
        self.window.set_default_size(-1, -1);
        self.window.queue_resize();
        self.window.set_visible(true);
        self.window.present();
    }

    fn schedule_hide(&self) {
        let expected = self.token.get();
        let token = Rc::clone(&self.token);
        let win = self.window.clone();
        glib::timeout_add_local_once(Duration::from_millis(55), move || {
            if token.get() == expected && win.is_visible() {
                win.set_visible(false);
            }
        });
    }

    fn hide_immediate(&self) {
        self.token.set(self.token.get().wrapping_add(1));
        if self.window.is_visible() {
            self.window.set_visible(false);
        }
    }
}

#[derive(Default)]
struct DockHideState {
    combined_mode: bool,
    auto_hide: bool,
    smart_hide: bool,
    magnification: bool,
    has_fullscreen: bool,
    overview_open: bool,
    workspaces_open: bool,
    qs_open: bool,
    nc_open: bool,
    preview_open: bool,
    window_overlaps_dock: bool,
    hovered_dock: bool,
    hovered_sensor: bool,
    currently_shown: bool,
    last_exclusive: i32,
    position: String,
    icon_size: u32,
    dock_height: u32,
    anim_gen: u64,
}

type StopRecordingCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// Floating glass dock (and full-width Combined Bottom Taskbar when enabled).
pub struct Dock {
    window: ApplicationWindow,
    sensor_window: ApplicationWindow,
    sensor_strip: GtkBox,
    wrapper: GtkBox,
    taskbar_box: CenterBox,
    left_zone: GtkBox,
    activities_btn: Button,
    ws_icon_area: DrawingArea,
    workspaces_box: GtkBox,
    right_zone: GtkBox,
    rec_btn: Button,
    rec_label: Label,
    on_stop_recording: StopRecordingCallback,
    tray_box: GtkBox,
    tray_sep: Separator,
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
    clock_btn: Button,
    clock_time_label: Label,
    clock_date_label: Label,
    clock_24h: Rc<Cell<bool>>,
    notif_dot: GtkBox,
    show_system_tray: Cell<bool>,
    is_light_theme: Rc<Cell<bool>>,
    last_ws_sig: RefCell<Vec<(usize, bool, usize)>>,
    tray_items: RefCell<Vec<TrayItem>>,
    pending_tray_menu: Rc<RefCell<Option<PendingTrayMenu>>>,
    tray_tx: TrayActionSender,
    container: GtkBox,
    pinned_box: GtkBox,
    running_box: GtkBox,
    separator: Separator,
    sep2: Separator,
    settings_btn: Button,
    grid_icon_area: DrawingArea,
    all_apps: Rc<RefCell<Vec<DesktopApp>>>,
    last_windows: RefCell<Vec<DockWindowSig>>,
    known_window_ids: RefCell<HashSet<u64>>,
    seeded_windows: Cell<bool>,
    last_position: RefCell<String>,
    last_combined_mode: Cell<bool>,
    last_combined_width: Cell<i32>,
    last_icon_size: Cell<u32>,
    last_dock_height: Cell<u32>,
    last_magnification: Cell<bool>,
    last_pinned_ids: RefCell<Vec<String>>,
    overview_elevated: Cell<bool>,
    workspaces_elevated: Cell<bool>,
    last_layer_elevated: Cell<bool>,
    drawer_bg_hidden: Rc<Cell<bool>>,
    hide_state: Rc<RefCell<DockHideState>>,
    wave_state: Rc<RefCell<DockWaveState>>,
    dock_label: DockLabelPopup,
    context_menu: ContextMenuOverlay,
    dock_preview: DockPreviewPopup,
    ipc: IpcCommandSender,
}

impl Dock {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        app: &Application,
        favorites: &[DesktopApp],
        all_apps: Rc<RefCell<Vec<DesktopApp>>>,
        ipc: IpcCommandSender,
        tray_tx: TrayActionSender,
        quick_settings: QuickSettingsPanel,
        notification_center: NotificationCenterPanel,
        workspaces_panel: WorkspacesPanel,
        context_menu: ContextMenuOverlay,
        dock_preview: DockPreviewPopup,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dock")
            .decorated(false)
            .build();
        window.add_css_class("neon-dock-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-dock"));
            window.set_layer(Layer::Top);
            window.set_anchor(Edge::Bottom, true);
            window.set_margin(Edge::Bottom, 0);
            window.set_keyboard_mode(KeyboardMode::None);
            window.set_exclusive_zone(68);
        }

        // Invisible 3px screen-edge pointer sensor for Auto-Hide & Smart-Hide reveal.
        let sensor_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dock Sensor")
            .decorated(false)
            .resizable(false)
            .default_height(3)
            .build();
        sensor_window.add_css_class("neon-edge-sensor-window");
        if gtk4_layer_shell::is_supported() {
            sensor_window.init_layer_shell();
            sensor_window.set_namespace(Some("neon-dock-sensor"));
            sensor_window.set_layer(Layer::Top);
            sensor_window.set_anchor(Edge::Bottom, true);
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

        let wrapper = GtkBox::new(Orientation::Horizontal, 0);
        wrapper.add_css_class("neon-dock-wrapper");
        wrapper.set_hexpand(true);
        wrapper.set_vexpand(true);

        let taskbar_box = CenterBox::new();
        taskbar_box.set_hexpand(true);
        taskbar_box.set_vexpand(true);
        taskbar_box.set_halign(Align::Fill);
        taskbar_box.set_valign(Align::Fill);

        let container = GtkBox::new(Orientation::Horizontal, 4);
        container.add_css_class("neon-dock-container");
        container.add_css_class("dock-magnify");
        container.set_halign(Align::Center);
        container.set_valign(Align::Center);

        let pinned_box = GtkBox::new(Orientation::Horizontal, 4);
        pinned_box.set_halign(Align::Center);
        pinned_box.set_valign(Align::Center);
        container.append(&pinned_box);

        let separator = Separator::new(Orientation::Vertical);
        separator.add_css_class("neon-dock-separator");
        separator.set_halign(Align::Center);
        separator.set_valign(Align::Center);
        separator.set_visible(false);
        container.append(&separator);

        let running_box = GtkBox::new(Orientation::Horizontal, 4);
        running_box.set_halign(Align::Center);
        running_box.set_valign(Align::Center);
        running_box.set_visible(false);
        container.append(&running_box);

        let sep2 = Separator::new(Orientation::Vertical);
        sep2.add_css_class("neon-dock-separator");
        sep2.set_halign(Align::Center);
        sep2.set_valign(Align::Center);
        container.append(&sep2);

        let hide_state = Rc::new(RefCell::new(DockHideState {
            currently_shown: true,
            magnification: true,
            last_exclusive: 68,
            position: "bottom".into(),
            icon_size: 36,
            dock_height: 52,
            ..Default::default()
        }));
        let wave_state = Rc::new(RefCell::new(DockWaveState {
            dirty: true,
            ..Default::default()
        }));
        let dock_label = DockLabelPopup::new(app);

        let is_light_theme = Rc::new(Cell::new(false));
        let drawer_bg_hidden_flag = Rc::new(Cell::new(false));

        // Combined Bottom Taskbar Left Zone (Windows-style Workspaces / Task View button + workspace dots)
        let left_zone = GtkBox::new(Orientation::Horizontal, 8);
        left_zone.add_css_class("neon-taskbar-left");
        left_zone.set_valign(Align::Center);
        left_zone.set_visible(false);

        let ws_icon_area = DrawingArea::new();
        ws_icon_area.set_content_width(20);
        ws_icon_area.set_content_height(16);
        ws_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden_flag);
            ws_icon_area.set_draw_func(move |_, cr, w, h| {
                draw_workspaces_taskview_icon(cr, w, h, light_flag.get(), drawer_flag.get());
            });
        }
        let activities_btn = Button::new();
        activities_btn.add_css_class("neon-ws-taskview-btn");
        activities_btn.set_child(Some(&ws_icon_area));
        activities_btn.set_tooltip_text(Some("Task View & Workspaces (Super+1–0)"));
        {
            let btn_c = activities_btn.clone();
            let st_c = Rc::clone(&hide_state);
            let win_c = window.clone();
            let sen_c = sensor_window.clone();
            let wrap_c = wrapper.clone();
            workspaces_panel.add_on_state_change(move |open| {
                if open {
                    btn_c.add_css_class("active");
                } else {
                    btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().workspaces_open = open;
                sync_dock_visibility(&st_c, &win_c, &sen_c, &wrap_c, false);
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

        left_zone.append(&activities_btn);
        left_zone.append(&workspaces_box);

        // Combined Bottom Taskbar Right Zone (Tray + Battery + Control Center + Clock/Notifications)
        let right_zone = GtkBox::new(Orientation::Horizontal, 3);
        right_zone.add_css_class("neon-taskbar-right");
        right_zone.set_valign(Align::Center);
        right_zone.set_visible(false);

        // Live Screen Recording stop pill for Combined Bottom Taskbar mode
        let rec_inner = GtkBox::new(Orientation::Horizontal, 6);
        rec_inner.set_valign(Align::Center);
        let rec_dot = GtkBox::new(Orientation::Horizontal, 0);
        rec_dot.add_css_class("neon-rec-dot");
        rec_dot.set_valign(Align::Center);
        let rec_label = Label::new(Some("00:00  Stop"));
        rec_inner.append(&rec_dot);
        rec_inner.append(&rec_label);
        let rec_btn = Button::new();
        rec_btn.add_css_class("neon-rec-pill");
        rec_btn.set_child(Some(&rec_inner));
        rec_btn.set_tooltip_text(Some("Stop Screen Recording"));
        rec_btn.set_visible(false);
        let on_stop_recording: StopRecordingCallback = Rc::new(RefCell::new(None));
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
            let drawer_flag = Rc::clone(&drawer_bg_hidden_flag);
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
            let drawer_flag = Rc::clone(&drawer_bg_hidden_flag);
            wifi_icon_area.set_draw_func(move |_, cr, w, h| {
                let v = vis_flag.get();
                draw_network_status_icon(
                    cr,
                    w,
                    h,
                    v.kind,
                    v.strength,
                    light_flag.get(),
                    drawer_flag.get(),
                );
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

        let cc_icon_area = DrawingArea::new();
        cc_icon_area.set_content_width(18);
        cc_icon_area.set_content_height(15);
        cc_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden_flag);
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
            let wrap_c = wrapper.clone();
            quick_settings.add_on_state_change(move |open| {
                if open {
                    ws_close.close_immediate();
                    cc_btn_c.add_css_class("active");
                } else {
                    cc_btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().qs_open = open;
                sync_dock_visibility(&st_c, &win_c, &sen_c, &wrap_c, !open);
            });
        }
        {
            let qs = quick_settings;
            cc_btn.connect_clicked(move |_| qs.toggle());
        }

        let clock_24h = Rc::new(Cell::new(true));
        let clock_inner = GtkBox::new(Orientation::Horizontal, 7);
        clock_inner.set_valign(Align::Center);

        let clock_stack = GtkBox::new(Orientation::Vertical, 0);
        clock_stack.add_css_class("neon-taskbar-clock-stack");
        clock_stack.set_valign(Align::Center);
        clock_stack.set_halign(Align::End);

        let clock_time_label = Label::new(Some(&format_time_only(clock_24h.get())));
        clock_time_label.add_css_class("neon-taskbar-clock-time");
        clock_time_label.set_halign(Align::End);
        clock_time_label.set_xalign(1.0);

        let clock_date_label = Label::new(Some(&format_date_short()));
        clock_date_label.add_css_class("neon-taskbar-clock-date");
        clock_date_label.set_halign(Align::End);
        clock_date_label.set_xalign(1.0);

        clock_stack.append(&clock_time_label);
        clock_stack.append(&clock_date_label);

        let notif_dot = GtkBox::new(Orientation::Horizontal, 0);
        notif_dot.add_css_class("neon-notif-dot");
        notif_dot.set_valign(Align::Center);
        notif_dot.set_visible(false);
        clock_inner.append(&clock_stack);
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
            let wrap_c = wrapper.clone();
            notification_center.add_on_state_change(move |open| {
                if open {
                    ws_close.close_immediate();
                    clk_btn_c.add_css_class("active");
                } else {
                    clk_btn_c.remove_css_class("active");
                }
                st_c.borrow_mut().nc_open = open;
                sync_dock_visibility(&st_c, &win_c, &sen_c, &wrap_c, !open);
            });
        }
        {
            let nc = notification_center;
            clock_btn.connect_clicked(move |_| nc.toggle());
        }
        {
            let time_clone = clock_time_label.clone();
            let date_clone = clock_date_label.clone();
            let clock_24h_c = Rc::clone(&clock_24h);
            let st_clk = Rc::clone(&hide_state);
            glib::timeout_add_local(Duration::from_secs(1), move || {
                if st_clk.borrow().combined_mode {
                    time_clone.set_text(&format_time_only(clock_24h_c.get()));
                    date_clone.set_text(&format_date_short());
                }
                glib::ControlFlow::Continue
            });
        }

        right_zone.append(&rec_btn);
        right_zone.append(&tray_box);
        right_zone.append(&tray_sep);
        right_zone.append(&bt_btn);
        right_zone.append(&wifi_btn);
        right_zone.append(&power_btn);
        right_zone.append(&cc_btn);
        right_zone.append(&clock_btn);

        let settings_btn = make_icon_dock_button(
            Some("preferences-system"),
            "settings",
            "NEON DE Settings",
            false,
            false,
            36,
        );
        {
            let ipc_set = ipc.clone();
            let cm_set = context_menu.clone();
            let cont_set = container.clone();
            let btn_set = settings_btn.clone();
            let st_set = Rc::clone(&hide_state);
            let lbl_set = dock_label.clone();
            settings_btn.connect_clicked(move |_| {
                lbl_set.hide_immediate();
                let pos = st_set.borrow().position.clone();
                let (ox, oy) = dock_button_center_screen_coords(&cm_set, &cont_set, &btn_set, &pos);
                ipc_set.send(IpcRequest::SetLaunchOrigin { x: ox, y: oy });
                trigger_dock_launch_hop(&btn_set);
                launch_sibling_binary("neon-settings");
                ipc_set.send(IpcRequest::SetOverview { open: false });
            });
        }
        {
            let hover_ctrl = EventControllerMotion::new();
            let lbl_in = dock_label.clone();
            let cm_h = context_menu.clone();
            let cont_h = container.clone();
            let tb_h = taskbar_box.clone();
            let btn_h = settings_btn.clone();
            let st_h = Rc::clone(&hide_state);
            hover_ctrl.connect_enter(move |_, _, _| {
                let st = st_h.borrow();
                if st.overview_open || st.workspaces_open || st.qs_open || st.nc_open {
                    return;
                }
                let pos = st.position.clone();
                let combined = st.combined_mode;
                let band = dock_band_px_for_preview(&st, &cont_h, &tb_h);
                drop(st);
                let (out_w, out_h) = cm_h.output_size();
                let (sx, sy) = dock_button_center_screen_coords(&cm_h, &cont_h, &btn_h, &pos);
                lbl_in.show("Settings", sx, sy, &pos, band, combined, out_w, out_h);
            });
            let lbl_out = dock_label.clone();
            hover_ctrl.connect_leave(move |_| {
                lbl_out.schedule_hide();
            });
            settings_btn.add_controller(hover_ctrl);
        }
        container.append(&settings_btn);

        let grid_icon_area = DrawingArea::new();
        grid_icon_area.set_content_width(36);
        grid_icon_area.set_content_height(36);
        grid_icon_area.set_halign(Align::Center);
        grid_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            let drawer_flag = Rc::clone(&drawer_bg_hidden_flag);
            grid_icon_area.set_draw_func(move |_, cr, w, h| {
                draw_app_grid_icon(cr, w, h, light_flag.get(), drawer_flag.get());
            });
        }
        let grid_btn = make_widget_dock_button(
            &grid_icon_area,
            "Applications & Overview (Super+Space)",
            false,
            false,
        );
        {
            let ipc_g = ipc.clone();
            let btn_g = grid_btn.clone();
            let lbl_g = dock_label.clone();
            grid_btn.connect_clicked(move |_| {
                lbl_g.hide_immediate();
                trigger_dock_launch_hop(&btn_g);
                ipc_g.send(IpcRequest::ToggleOverview);
            });
        }
        {
            let hover_ctrl = EventControllerMotion::new();
            let lbl_in = dock_label.clone();
            let cm_h = context_menu.clone();
            let cont_h = container.clone();
            let tb_h = taskbar_box.clone();
            let btn_h = grid_btn.clone();
            let st_h = Rc::clone(&hide_state);
            hover_ctrl.connect_enter(move |_, _, _| {
                let st = st_h.borrow();
                if st.overview_open || st.workspaces_open || st.qs_open || st.nc_open {
                    return;
                }
                let pos = st.position.clone();
                let combined = st.combined_mode;
                let band = dock_band_px_for_preview(&st, &cont_h, &tb_h);
                drop(st);
                let (out_w, out_h) = cm_h.output_size();
                let (sx, sy) = dock_button_center_screen_coords(&cm_h, &cont_h, &btn_h, &pos);
                lbl_in.show("Applications", sx, sy, &pos, band, combined, out_w, out_h);
            });
            let lbl_out = dock_label.clone();
            hover_ctrl.connect_leave(move |_| {
                lbl_out.schedule_hide();
            });
            grid_btn.add_controller(hover_ctrl);
        }
        container.append(&grid_btn);

        // macOS Gaussian parabolic wave magnification across the hovered icon and its neighbors.
        {
            let wave_motion = EventControllerMotion::new();
            {
                let wave_c = Rc::clone(&wave_state);
                let cont_c = container.clone();
                let st_c = Rc::clone(&hide_state);
                wave_motion.connect_enter(move |_, mx, my| {
                    update_dock_wave(&wave_c, &cont_c, &st_c, mx, my);
                });
            }
            {
                let wave_c = Rc::clone(&wave_state);
                let cont_c = container.clone();
                let st_c = Rc::clone(&hide_state);
                wave_motion.connect_motion(move |_, mx, my| {
                    update_dock_wave(&wave_c, &cont_c, &st_c, mx, my);
                });
            }
            {
                let wave_c = Rc::clone(&wave_state);
                let cont_c = container.clone();
                let lbl_c = dock_label.clone();
                wave_motion.connect_leave(move |_| {
                    clear_dock_wave(&wave_c, &cont_c);
                    lbl_c.hide_immediate();
                });
            }
            container.add_controller(wave_motion);
        }

        // Right-clicking empty dock / taskbar area opens context-appropriate settings & toggles.
        let dock_rc = GestureClick::new();
        dock_rc.set_button(3);
        {
            let cm = context_menu.clone();
            let cont = container.clone();
            let tb = taskbar_box.clone();
            let st_rc = Rc::clone(&hide_state);
            let lz_c = left_zone.clone();
            let bt_c = bt_btn.clone();
            let wifi_c = wifi_btn.clone();
            let pwr_c = power_btn.clone();
            let cc_c = cc_btn.clone();
            let clk_c = clock_btn.clone();
            let try_c = tray_box.clone();
            dock_rc.connect_pressed(move |gesture, _, x, y| {
                let (pos, combined) = {
                    let st = st_rc.borrow();
                    (st.position.clone(), st.combined_mode)
                };
                if combined {
                    let (out_w, out_h) = cm.output_size();
                    let bar_h = tb.height().max(52);
                    let menu_y = out_h - bar_h - 8;
                    if let Some(w) = gesture.widget()
                        && let Some(picked) = w.pick(x, y, gtk4::PickFlags::DEFAULT)
                    {
                        let is_in =
                            |target: &gtk4::Widget| picked == *target || picked.is_ancestor(target);
                        if is_in(try_c.upcast_ref()) {
                            return;
                        }
                        let sx = x.round().clamp(16.0, f64::from(out_w - 16)) as i32;
                        if is_in(pwr_c.upcast_ref()) {
                            cm.open_panel_menu(sx, menu_y, PanelMenuZone::Battery);
                            return;
                        } else if is_in(cc_c.upcast_ref())
                            || is_in(bt_c.upcast_ref())
                            || is_in(wifi_c.upcast_ref())
                        {
                            cm.open_panel_menu(sx, menu_y, PanelMenuZone::QuickSettings);
                            return;
                        } else if is_in(clk_c.upcast_ref()) {
                            cm.open_panel_menu(sx, menu_y, PanelMenuZone::Clock);
                            return;
                        } else if is_in(lz_c.upcast_ref()) {
                            cm.open_panel_menu(sx, menu_y, PanelMenuZone::Workspaces);
                            return;
                        }
                    }
                    cm.open_dock_menu(x.round() as i32, menu_y);
                } else {
                    let (sx, sy) = dock_point_screen_coords(&cm, &cont, &pos, x, y);
                    cm.open_dock_menu(sx, sy);
                }
            });
        }
        taskbar_box.add_controller(dock_rc);

        taskbar_box.set_start_widget(Some(&left_zone));
        taskbar_box.set_center_widget(Some(&container));
        taskbar_box.set_end_widget(Some(&right_zone));
        wrapper.append(&taskbar_box);
        window.set_child(Some(&wrapper));
        window.present();

        // Pointer hover tracking on Dock & edge sensor
        {
            let motion = EventControllerMotion::new();
            let st_in = Rc::clone(&hide_state);
            let win_in = window.clone();
            let sen_in = sensor_window.clone();
            let wrap_in = wrapper.clone();
            motion.connect_enter(move |_, _, _| {
                st_in.borrow_mut().hovered_dock = true;
                sync_dock_visibility(&st_in, &win_in, &sen_in, &wrap_in, false);
            });

            let st_out = Rc::clone(&hide_state);
            let win_out = window.clone();
            let sen_out = sensor_window.clone();
            let wrap_out = wrapper.clone();
            motion.connect_leave(move |_| {
                st_out.borrow_mut().hovered_dock = false;
                sync_dock_visibility(&st_out, &win_out, &sen_out, &wrap_out, true);
            });
            wrapper.add_controller(motion);
        }
        {
            let motion = EventControllerMotion::new();
            let st_in = Rc::clone(&hide_state);
            let win_in = window.clone();
            let sen_in = sensor_window.clone();
            let wrap_in = wrapper.clone();
            motion.connect_enter(move |_, _, _| {
                st_in.borrow_mut().hovered_sensor = true;
                sync_dock_visibility(&st_in, &win_in, &sen_in, &wrap_in, false);
            });

            let st_out = Rc::clone(&hide_state);
            let win_out = window.clone();
            let sen_out = sensor_window.clone();
            let wrap_out = wrapper.clone();
            motion.connect_leave(move |_| {
                st_out.borrow_mut().hovered_sensor = false;
                sync_dock_visibility(&st_out, &win_out, &sen_out, &wrap_out, true);
            });
            sensor_strip.add_controller(motion);
        }

        // Keep an auto-hiding Dock open while the user hovers into a Dock preview popup,
        // and immediately dismiss the compact label pill whenever the full preview card opens.
        {
            let st_c = Rc::clone(&hide_state);
            let win_c = window.clone();
            let sen_c = sensor_window.clone();
            let wrap_c = wrapper.clone();
            let lbl_c = dock_label.clone();
            dock_preview.add_on_state_change(move |open| {
                if open {
                    lbl_c.hide_immediate();
                }
                st_c.borrow_mut().preview_open = open;
                sync_dock_visibility(&st_c, &win_c, &sen_c, &wrap_c, !open);
            });
        }

        let dock = Self {
            window,
            sensor_window,
            sensor_strip,
            wrapper,
            taskbar_box,
            left_zone,
            activities_btn,
            ws_icon_area,
            workspaces_box,
            right_zone,
            rec_btn,
            rec_label,
            on_stop_recording,
            tray_box,
            tray_sep,
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
            clock_btn,
            clock_time_label,
            clock_date_label,
            clock_24h,
            notif_dot,
            show_system_tray: Cell::new(true),
            is_light_theme,
            last_ws_sig: RefCell::new(Vec::new()),
            tray_items: RefCell::new(Vec::new()),
            pending_tray_menu: Rc::new(RefCell::new(None)),
            tray_tx,
            container,
            pinned_box,
            running_box,
            separator,
            sep2,
            settings_btn,
            grid_icon_area,
            all_apps,
            last_windows: RefCell::new(Vec::new()),
            known_window_ids: RefCell::new(HashSet::new()),
            seeded_windows: Cell::new(false),
            last_position: RefCell::new("bottom".into()),
            last_combined_mode: Cell::new(false),
            last_combined_width: Cell::new(0),
            last_icon_size: Cell::new(36),
            last_dock_height: Cell::new(52),
            last_magnification: Cell::new(true),
            last_pinned_ids: RefCell::new(Vec::new()),
            overview_elevated: Cell::new(false),
            workspaces_elevated: Cell::new(false),
            last_layer_elevated: Cell::new(false),
            drawer_bg_hidden: drawer_bg_hidden_flag,
            hide_state,
            wave_state,
            dock_label,
            context_menu,
            dock_preview,
            ipc,
        };
        dock.rebuild_pinned_buttons(favorites, 36);
        dock
    }

    fn rebuild_pinned_buttons(&self, favorites: &[DesktopApp], icon_size: i32) {
        invalidate_dock_wave(&self.wave_state, &self.container);
        self.dock_label.hide_immediate();
        while let Some(child) = self.pinned_box.first_child() {
            self.pinned_box.remove(&child);
        }
        for fav in favorites {
            let btn = make_icon_dock_button(
                fav.icon.as_deref(),
                &fav.name,
                &fav.name,
                false,
                false,
                icon_size,
            );
            let app_clone = fav.clone();
            let ipc_fav = self.ipc.clone();
            let cm_fav = self.context_menu.clone();
            let prev_fav_click = self.dock_preview.clone();
            let lbl_fav_click = self.dock_label.clone();
            let cont_fav = self.container.clone();
            let btn_fav = btn.clone();
            let st_fav = Rc::clone(&self.hide_state);
            btn.connect_clicked(move |_| {
                prev_fav_click.hide_immediate();
                lbl_fav_click.hide_immediate();
                let pos = st_fav.borrow().position.clone();
                let (ox, oy) = dock_button_center_screen_coords(&cm_fav, &cont_fav, &btn_fav, &pos);
                ipc_fav.send(IpcRequest::SetLaunchOrigin { x: ox, y: oy });
                trigger_dock_launch_hop(&btn_fav);
                app_clone.launch();
                ipc_fav.send(IpcRequest::SetOverview { open: false });
            });
            // Hover preview + above-icon title pill for pinned apps.
            {
                let hover_ctrl = EventControllerMotion::new();
                let prev_in = self.dock_preview.clone();
                let lbl_in = self.dock_label.clone();
                let cm_h = self.context_menu.clone();
                let cont_h = self.container.clone();
                let tb_h = self.taskbar_box.clone();
                let btn_h = btn.clone();
                let st_h = Rc::clone(&self.hide_state);
                let pin_id = fav.id.clone();
                let pin_name = fav.name.clone();
                hover_ctrl.connect_enter(move |_, _, _| {
                    let st = st_h.borrow();
                    if st.overview_open || st.workspaces_open || st.qs_open || st.nc_open {
                        return;
                    }
                    let pos = st.position.clone();
                    let combined = st.combined_mode;
                    let preview_open = st.preview_open;
                    let band = dock_band_px_for_preview(&st, &cont_h, &tb_h);
                    drop(st);
                    let (out_w, out_h) = cm_h.output_size();
                    let (sx, sy) = dock_button_center_screen_coords(&cm_h, &cont_h, &btn_h, &pos);
                    if !preview_open {
                        lbl_in.show(&pin_name, sx, sy, &pos, band, combined, out_w, out_h);
                    }
                    prev_in.schedule_hover(DockPreviewAnchor {
                        target: DockPreviewTarget::PinnedApp {
                            app_id: pin_id.clone(),
                            app_name: pin_name.clone(),
                        },
                        screen_x: sx,
                        screen_y: sy,
                        dock_position: pos,
                        dock_band_px: band,
                        output_w: out_w,
                        output_h: out_h,
                    });
                });
                let prev_out = self.dock_preview.clone();
                let lbl_out = self.dock_label.clone();
                hover_ctrl.connect_leave(move |_| {
                    prev_out.on_dock_item_leave();
                    lbl_out.schedule_hide();
                });
                btn.add_controller(hover_ctrl);
            }
            let rc_gesture = GestureClick::new();
            rc_gesture.set_button(3);
            {
                let cm = self.context_menu.clone();
                let prev_rc = self.dock_preview.clone();
                let lbl_rc = self.dock_label.clone();
                let app_rc = fav.clone();
                let cont = self.container.clone();
                let btn_c = btn.clone();
                let st_rc = Rc::clone(&self.hide_state);
                rc_gesture.connect_pressed(move |gesture, _, x, y| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    prev_rc.hide_immediate();
                    lbl_rc.hide_immediate();
                    let pos = st_rc.borrow().position.clone();
                    let (sx, sy) = dock_widget_screen_coords(&cm, &cont, &btn_c, &pos, x, y);
                    cm.open_dock_app_menu(sx, sy, Some(app_rc.clone()), None);
                });
            }
            btn.add_controller(rc_gesture);
            self.pinned_box.append(&btn);
        }
    }

    fn apply_position(&self, pos: &str, combined: bool, out_w: i32, dock_height: u32) {
        invalidate_dock_wave(&self.wave_state, &self.container);
        self.dock_label.hide_immediate();
        let was_visible = self.window.is_visible();
        if was_visible {
            self.window.set_visible(false);
        }

        let vertical = !combined && (pos == "left" || pos == "right");
        let orient = if vertical {
            Orientation::Vertical
        } else {
            Orientation::Horizontal
        };
        let sep_orient = if vertical {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        };

        self.wrapper.set_orientation(orient);
        self.taskbar_box.set_orientation(orient);
        self.container.set_orientation(orient);
        self.pinned_box.set_orientation(orient);
        self.running_box.set_orientation(orient);
        self.separator.set_orientation(sep_orient);
        self.sep2.set_orientation(sep_orient);

        if combined {
            self.wrapper.add_css_class("combined-taskbar");
            self.taskbar_box.add_css_class("neon-taskbar-combined");
            self.container.add_css_class("in-taskbar");
            self.container.remove_css_class("drawer-open");
            self.left_zone.set_visible(true);
            self.right_zone.set_visible(true);
            self.window.set_resizable(true);
        } else {
            self.wrapper.remove_css_class("combined-taskbar");
            self.taskbar_box.remove_css_class("neon-taskbar-combined");
            self.taskbar_box.remove_css_class("drawer-open");
            self.container.remove_css_class("in-taskbar");
            self.left_zone.set_visible(false);
            self.right_zone.set_visible(false);
            self.window.set_resizable(false);
        }

        if vertical {
            self.wrapper.add_css_class("dock-vertical");
            self.container.add_css_class("dock-vertical");
        } else {
            self.wrapper.remove_css_class("dock-vertical");
            self.container.remove_css_class("dock-vertical");
        }

        if gtk4_layer_shell::is_supported() {
            if combined {
                self.window.set_anchor(Edge::Bottom, true);
                self.window.set_anchor(Edge::Left, true);
                self.window.set_anchor(Edge::Right, true);
                self.window.set_anchor(Edge::Top, false);

                self.sensor_window.set_anchor(Edge::Bottom, true);
                self.sensor_window.set_anchor(Edge::Top, false);
                self.sensor_window.set_anchor(Edge::Left, true);
                self.sensor_window.set_anchor(Edge::Right, true);
                self.sensor_strip.set_size_request(-1, 3);
                self.sensor_strip.set_hexpand(true);
                self.sensor_strip.set_vexpand(false);
            } else {
                self.window.set_anchor(Edge::Bottom, pos == "bottom");
                self.window.set_anchor(Edge::Left, pos == "left");
                self.window.set_anchor(Edge::Right, pos == "right");
                self.window.set_anchor(Edge::Top, false);

                self.sensor_window
                    .set_anchor(Edge::Bottom, pos == "bottom" || vertical);
                self.sensor_window.set_anchor(Edge::Top, vertical);
                self.sensor_window
                    .set_anchor(Edge::Left, pos == "left" || !vertical);
                self.sensor_window
                    .set_anchor(Edge::Right, pos == "right" || !vertical);
                if vertical {
                    self.sensor_strip.set_size_request(3, -1);
                    self.sensor_strip.set_hexpand(false);
                    self.sensor_strip.set_vexpand(true);
                } else {
                    self.sensor_strip.set_size_request(-1, 3);
                    self.sensor_strip.set_hexpand(true);
                    self.sensor_strip.set_vexpand(false);
                }
            }
        }
        let target_h = dock_height.clamp(36, 88) as i32;
        if combined {
            self.last_combined_width.set(out_w);
            self.taskbar_box.set_size_request(out_w, target_h);
            self.window.set_default_size(out_w, target_h);
        } else {
            self.last_combined_width.set(0);
            self.taskbar_box.set_size_request(-1, -1);
            self.window.set_default_size(1, 1);
        }
        self.sync_drawer_bg_class();
        self.window.queue_resize();
        if was_visible {
            self.window.set_visible(true);
            self.window.present();
        }
    }

    /// Returns the visual height of the Combined Bottom Taskbar for positioning popups cleanly above it.
    pub fn combined_bar_height(&self) -> i32 {
        let h = self.last_dock_height.get().clamp(36, 88) as i32;
        self.taskbar_box.height().max(h)
    }

    /// Forces running window icons on the Dock to be re-resolved on the next IPC snapshot
    /// (called after `.desktop` entries or icon themes refresh dynamically).
    pub fn invalidate_window_icons(&self) {
        clear_icon_cache();
        self.last_windows.borrow_mut().clear();
    }

    fn sync_layer_elevation(&self) {
        let elevated = self.overview_elevated.get() || self.workspaces_elevated.get();
        if self.last_layer_elevated.get() != elevated {
            self.last_layer_elevated.set(elevated);
            if gtk4_layer_shell::is_supported() {
                self.window.set_layer(if elevated {
                    Layer::Overlay
                } else {
                    Layer::Top
                });
            }
            self.window.queue_draw();
            self.container.queue_draw();
        }
    }

    fn sync_drawer_bg_class(&self) {
        let combined = self.hide_state.borrow().combined_mode;
        if self.drawer_bg_hidden.get() && !self.workspaces_elevated.get() {
            if combined {
                self.container.remove_css_class("drawer-open");
                self.taskbar_box.add_css_class("drawer-open");
            } else {
                self.container.add_css_class("drawer-open");
                self.taskbar_box.remove_css_class("drawer-open");
            }
        } else {
            self.container.remove_css_class("drawer-open");
            self.taskbar_box.remove_css_class("drawer-open");
        }
        self.cc_icon_area.queue_draw();
        self.ws_icon_area.queue_draw();
        self.grid_icon_area.queue_draw();
        self.wifi_icon_area.queue_draw();
        self.bt_icon_area.queue_draw();
    }

    /// Binds the callback invoked when the user clicks the recording stop pill in Combined Bottom mode.
    pub fn set_on_stop_recording<F: Fn() + 'static>(&self, cb: F) {
        *self.on_stop_recording.borrow_mut() = Some(Box::new(cb));
    }

    /// Shows or updates the live Screen Recording indicator pill inside Combined Bottom Taskbar mode.
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

    /// Elevates the Dock above the fullscreen App Drawer (`Layer::Overlay`) independently of
    /// Mission Control so switching directly between them never drops the Dock behind either backdrop.
    pub fn set_overview_elevated(&self, elevated: bool) {
        self.overview_elevated.set(elevated);
        self.sync_layer_elevation();
    }

    /// Keeps the floating Dock visible and elevated above the fullscreen Mission Control backdrop.
    pub fn set_workspaces_open(&self, open: bool) {
        self.hide_state.borrow_mut().workspaces_open = open;
        self.workspaces_elevated.set(open);
        self.sync_layer_elevation();
        self.sync_drawer_bg_class();
        sync_dock_visibility(
            &self.hide_state,
            &self.window,
            &self.sensor_window,
            &self.wrapper,
            false,
        );
    }

    pub fn set_drawer_bg_hidden(&self, hidden: bool) {
        self.drawer_bg_hidden.set(hidden);
        self.sync_drawer_bg_class();
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        self.dock_preview.update_snapshot(snapshot);
        let light =
            snapshot.appearance.theme == "neon_light" || snapshot.appearance.theme == "light";
        if self.is_light_theme.get() != light {
            self.is_light_theme.set(light);
            clear_icon_cache();
            self.cc_icon_area.queue_draw();
            self.ws_icon_area.queue_draw();
            self.grid_icon_area.queue_draw();
            self.wifi_icon_area.queue_draw();
            self.bt_icon_area.queue_draw();
        }

        let combined = snapshot
            .appearance
            .panel_mode
            .eq_ignore_ascii_case("combined_bottom");

        let (out_w, out_h) = snapshot
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| snapshot.outputs.first())
            .map(|o| (o.width.max(800), o.height.max(600)))
            .unwrap_or((1920, 1080));

        // 1. Live Dock Position, Panel Mode, Height, Icon Size, Magnification, and Pinned Apps sync
        let desired_pos = snapshot.appearance.dock_position.to_lowercase();
        let norm_pos = match desired_pos.as_str() {
            "left" => "left",
            "right" => "right",
            _ => "bottom",
        };
        let eff_pos = if combined { "bottom" } else { norm_pos };
        let desired_height = snapshot.appearance.dock_height.clamp(36, 88);
        let height_changed = self.last_dock_height.get() != desired_height;
        let combined_width_changed = combined && self.last_combined_width.get() != out_w;
        if self.last_position.borrow().as_str() != norm_pos
            || self.last_combined_mode.get() != combined
            || combined_width_changed
        {
            *self.last_position.borrow_mut() = norm_pos.to_string();
            self.last_combined_mode.set(combined);
            self.last_dock_height.set(desired_height);
            {
                let mut st = self.hide_state.borrow_mut();
                st.position = eff_pos.to_string();
                st.combined_mode = combined;
                st.dock_height = desired_height;
            }
            self.apply_position(norm_pos, combined, out_w, desired_height);
        } else if height_changed {
            self.last_dock_height.set(desired_height);
            self.hide_state.borrow_mut().dock_height = desired_height;
            if combined {
                self.taskbar_box
                    .set_size_request(out_w, desired_height as i32);
                self.window.set_default_size(out_w, desired_height as i32);
            } else {
                self.window.set_default_size(1, 1);
            }
            self.window.queue_resize();
        }

        if combined {
            self.clock_btn.set_visible(snapshot.appearance.show_clock);
            if self.clock_24h.get() != snapshot.appearance.clock_24h {
                self.clock_24h.set(snapshot.appearance.clock_24h);
                self.clock_time_label
                    .set_text(&format_time_only(snapshot.appearance.clock_24h));
                self.clock_date_label.set_text(&format_date_short());
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

        let desired_mag = snapshot.appearance.dock_magnification;
        if self.last_magnification.get() != desired_mag {
            self.last_magnification.set(desired_mag);
            self.hide_state.borrow_mut().magnification = desired_mag;
            if desired_mag {
                self.container.add_css_class("dock-magnify");
            } else {
                self.container.remove_css_class("dock-magnify");
                clear_dock_wave(&self.wave_state, &self.container);
            }
        }

        let requested_icon = snapshot.appearance.dock_icon_size.clamp(22, 64);
        let max_icon_for_height = desired_height.saturating_sub(10).clamp(20, 64);
        let desired_size = requested_icon.min(max_icon_for_height);
        let size_changed = self.last_icon_size.get() != desired_size;
        let pins_changed = *self.last_pinned_ids.borrow() != snapshot.appearance.dock_pinned_apps;
        if size_changed || pins_changed {
            self.last_icon_size.set(desired_size);
            self.hide_state.borrow_mut().icon_size = desired_size;
            *self.last_pinned_ids.borrow_mut() = snapshot.appearance.dock_pinned_apps.clone();
            if desired_size <= 32 {
                self.container.add_css_class("dock-compact");
                self.container.set_spacing(2);
                self.pinned_box.set_spacing(2);
                self.running_box.set_spacing(2);
            } else {
                self.container.remove_css_class("dock-compact");
                self.container.set_spacing(4);
                self.pinned_box.set_spacing(4);
                self.running_box.set_spacing(4);
            }
            let favs = {
                let guard = self.all_apps.borrow();
                select_favorites_with_pins(&guard, &snapshot.appearance.dock_pinned_apps)
            };
            self.rebuild_pinned_buttons(&favs, desired_size as i32);
            if size_changed {
                update_dock_button_icon(
                    &self.settings_btn,
                    Some("preferences-system"),
                    "settings",
                    desired_size as i32,
                );
                self.grid_icon_area.set_content_width(desired_size as i32);
                self.grid_icon_area.set_content_height(desired_size as i32);
                self.grid_icon_area.queue_draw();
                self.last_windows.borrow_mut().clear();
            }
            if combined {
                self.taskbar_box
                    .set_size_request(out_w, desired_height as i32);
                self.window.set_default_size(out_w, desired_height as i32);
            } else {
                self.window.set_default_size(1, 1);
            }
            self.window.queue_resize();
        }

        // 2. Fullscreen, Auto-Hide, and Smart-Hide overlap state (strictly scoped to the active
        //    workspace so an empty workspace never inherits fullscreen state from another desk).
        let dock_w = if combined {
            out_w
        } else {
            self.container.width().max(320)
        };
        let dock_left = if combined { 0 } else { (out_w - dock_w) / 2 };
        let dock_right = if combined { out_w } else { dock_left + dock_w };
        let dock_band = if combined {
            desired_height as i32
        } else {
            (desired_height as i32).max((desired_size as i32) + 20) + 12
        };
        let dock_top_zone = out_h - dock_band;

        let (has_fullscreen, window_overlaps_dock) = compute_dock_workspace_overlap(
            snapshot,
            eff_pos,
            dock_band,
            dock_left,
            dock_right,
            out_w,
            dock_top_zone,
        );

        {
            let mut st = self.hide_state.borrow_mut();
            st.combined_mode = combined;
            st.auto_hide = snapshot.appearance.dock_auto_hide;
            st.smart_hide = snapshot.appearance.dock_smart_hide;
            st.magnification = desired_mag;
            st.has_fullscreen = has_fullscreen;
            st.overview_open = snapshot.overview_open;
            st.window_overlaps_dock = window_overlaps_dock;
            st.icon_size = desired_size;
            st.dock_height = desired_height;
        }
        sync_dock_visibility(
            &self.hide_state,
            &self.window,
            &self.sensor_window,
            &self.wrapper,
            false,
        );

        let sig: Vec<DockWindowSig> = snapshot
            .windows
            .iter()
            .map(|w| {
                (
                    w.id,
                    w.app_id.clone(),
                    w.title.clone(),
                    w.focused,
                    w.minimized,
                )
            })
            .collect();
        if *self.last_windows.borrow() == sig {
            return;
        }
        *self.last_windows.borrow_mut() = sig;

        invalidate_dock_wave(&self.wave_state, &self.container);
        self.dock_label.hide_immediate();
        while let Some(child) = self.running_box.first_child() {
            self.running_box.remove(&child);
        }

        let has_running = !snapshot.windows.is_empty();
        self.separator.set_visible(has_running);
        self.running_box.set_visible(has_running);

        let apps_guard = self.all_apps.borrow();
        let icon_px = self.last_icon_size.get() as i32;
        let prev_known = self.known_window_ids.borrow().clone();
        let allow_pop_in = self.seeded_windows.get();
        let mut next_known: HashSet<u64> = HashSet::with_capacity(snapshot.windows.len());
        let mut anchor_buttons: Vec<(u64, Button)> = Vec::with_capacity(snapshot.windows.len());
        for win in &snapshot.windows {
            next_known.insert(win.id);
            let is_new_window = allow_pop_in && !prev_known.contains(&win.id);
            let display_name = win
                .title
                .as_deref()
                .or(win.app_id.as_deref())
                .unwrap_or("Window");
            let icon_spec =
                resolve_window_icon(win.app_id.as_deref(), win.title.as_deref(), &apps_guard);
            let icon_hint = win.app_id.as_deref().unwrap_or("");

            let btn = make_icon_dock_button(
                icon_spec.as_deref(),
                icon_hint,
                display_name,
                true,
                win.focused,
                icon_px,
            );
            // Rich live window preview card replaces plain text tooltip on running icons.
            btn.set_has_tooltip(false);
            if win.minimized {
                btn.set_opacity(0.55);
            }
            if is_new_window {
                btn.add_css_class("dock-icon-enter");
                btn.add_tick_callback(|w, _| {
                    w.remove_css_class("dock-icon-enter");
                    glib::ControlFlow::Break
                });
            }

            let win_id = win.id;
            let was_focused = win.focused;
            let was_minimized = win.minimized;
            {
                let ipc = self.ipc.clone();
                let cm = self.context_menu.clone();
                let prev_click = self.dock_preview.clone();
                let lbl_click = self.dock_label.clone();
                let cont = self.container.clone();
                let btn_c = btn.clone();
                let st_rc = Rc::clone(&self.hide_state);
                btn.connect_clicked(move |_| {
                    prev_click.hide_immediate();
                    lbl_click.hide_immediate();
                    let pos = st_rc.borrow().position.clone();
                    let (ax, ay) = dock_button_center_screen_coords(&cm, &cont, &btn_c, &pos);
                    ipc.send(IpcRequest::SetWindowDockAnchor {
                        window_id: win_id,
                        x: ax,
                        y: ay,
                    });
                    if was_focused && !was_minimized {
                        ipc.send(IpcRequest::MinimizeWindow { window_id: win_id });
                    } else {
                        if was_minimized {
                            trigger_dock_launch_hop(&btn_c);
                        }
                        ipc.send(IpcRequest::SetOverview { open: false });
                        ipc.send(IpcRequest::FocusWindow { window_id: win_id });
                    }
                });
            }
            // Hover shows an immediate above-icon title pill, then transitions to the live preview card.
            {
                let hover_ctrl = EventControllerMotion::new();
                let prev_in = self.dock_preview.clone();
                let lbl_in = self.dock_label.clone();
                let cm_h = self.context_menu.clone();
                let cont_h = self.container.clone();
                let tb_h = self.taskbar_box.clone();
                let btn_h = btn.clone();
                let st_h = Rc::clone(&self.hide_state);
                let hover_title = display_name.to_string();
                hover_ctrl.connect_enter(move |_, _, _| {
                    let st = st_h.borrow();
                    if st.overview_open || st.workspaces_open || st.qs_open || st.nc_open {
                        return;
                    }
                    let pos = st.position.clone();
                    let combined = st.combined_mode;
                    let preview_open = st.preview_open;
                    let band = dock_band_px_for_preview(&st, &cont_h, &tb_h);
                    drop(st);
                    let (out_w, out_h) = cm_h.output_size();
                    let (sx, sy) = dock_button_center_screen_coords(&cm_h, &cont_h, &btn_h, &pos);
                    if !preview_open {
                        lbl_in.show(&hover_title, sx, sy, &pos, band, combined, out_w, out_h);
                    }
                    prev_in.schedule_hover(DockPreviewAnchor {
                        target: DockPreviewTarget::Window(win_id),
                        screen_x: sx,
                        screen_y: sy,
                        dock_position: pos,
                        dock_band_px: band,
                        output_w: out_w,
                        output_h: out_h,
                    });
                });
                let prev_out = self.dock_preview.clone();
                let lbl_out = self.dock_label.clone();
                hover_ctrl.connect_leave(move |_| {
                    prev_out.on_dock_item_leave();
                    lbl_out.schedule_hide();
                });
                btn.add_controller(hover_ctrl);
            }
            let rc_gesture = GestureClick::new();
            rc_gesture.set_button(3);
            {
                let cm = self.context_menu.clone();
                let prev_rc = self.dock_preview.clone();
                let lbl_rc = self.dock_label.clone();
                let ipc = self.ipc.clone();
                let cont = self.container.clone();
                let btn_c = btn.clone();
                let st_rc = Rc::clone(&self.hide_state);
                rc_gesture.connect_pressed(move |gesture, _, x, y| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    prev_rc.hide_immediate();
                    lbl_rc.hide_immediate();
                    let pos = st_rc.borrow().position.clone();
                    let (ax, ay) = dock_button_center_screen_coords(&cm, &cont, &btn_c, &pos);
                    ipc.send(IpcRequest::SetWindowDockAnchor {
                        window_id: win_id,
                        x: ax,
                        y: ay,
                    });
                    let (sx, sy) = dock_widget_screen_coords(&cm, &cont, &btn_c, &pos, x, y);
                    cm.open_dock_app_menu(sx, sy, None, Some(win_id));
                });
            }
            btn.add_controller(rc_gesture);
            self.running_box.append(&btn);
            anchor_buttons.push((win_id, btn));
        }
        *self.known_window_ids.borrow_mut() = next_known;
        self.seeded_windows.set(true);

        // Report each running window's exact Dock icon screen center once GTK finishes layout
        // so titlebar Minimize/Close and Super+H/Super+Q swoop accurately to/from that icon.
        if !anchor_buttons.is_empty() {
            let cm = self.context_menu.clone();
            let cont = self.container.clone();
            let st_rc = Rc::clone(&self.hide_state);
            let ipc = self.ipc.clone();
            glib::timeout_add_local_once(Duration::from_millis(60), move || {
                let pos = st_rc.borrow().position.clone();
                for (wid, btn) in anchor_buttons {
                    if btn.parent().is_some() {
                        let (ax, ay) = dock_button_center_screen_coords(&cm, &cont, &btn, &pos);
                        ipc.send(IpcRequest::SetWindowDockAnchor {
                            window_id: wid,
                            x: ax,
                            y: ay,
                        });
                    }
                }
            });
        }
    }

    /// Updates active Wi-Fi/Bluetooth icons, battery status, and unread notification dot inside Combined Bottom Taskbar mode.
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

    /// Updates the live System Tray (`StatusNotifierItem`) icons inside Combined Bottom Taskbar mode.
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

            let compute_screen_xy = {
                let cm = self.context_menu.clone();
                let tb = self.taskbar_box.clone();
                let btn_c = btn.clone();
                move || {
                    let (out_w, out_h) = cm.output_size();
                    let bar_h = tb.height().max(52);
                    let x = btn_c
                        .compute_bounds(&tb)
                        .map(|r| r.x().round() as i32)
                        .unwrap_or(out_w - 200);
                    (x.clamp(16, out_w - 16), out_h - bar_h - 6)
                }
            };

            {
                let item_c = item.clone();
                let tx = self.tray_tx.clone();
                let pending = Rc::clone(&self.pending_tray_menu);
                let ctx = self.context_menu.clone();
                let xy_fn = compute_screen_xy.clone();
                btn.connect_clicked(move |_| {
                    let (x, y) = xy_fn();
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
                            y,
                        });
                    }
                });
            }

            let rc = GestureClick::new();
            rc.set_button(3);
            {
                let item_c = item.clone();
                let tx = self.tray_tx.clone();
                let pending = Rc::clone(&self.pending_tray_menu);
                let ctx = self.context_menu.clone();
                let xy_fn = compute_screen_xy.clone();
                rc.connect_pressed(move |gesture, _, _, _| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    let (x, y) = xy_fn();
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
                            y,
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

            let mc = GestureClick::new();
            mc.set_button(2);
            {
                let item_c = item.clone();
                let tx = self.tray_tx.clone();
                mc.connect_pressed(move |gesture, _, _, _| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    let (x, y) = compute_screen_xy();
                    tx.send(TrayAction::SecondaryActivate {
                        bus_name: item_c.bus_name.clone(),
                        object_path: item_c.object_path.clone(),
                        x,
                        y,
                    });
                });
            }
            btn.add_controller(mc);

            self.tray_box.append(&btn);
        }
    }

    pub fn handle_tray_menu_loaded(
        &self,
        key: String,
        bus_name: String,
        menu_path: String,
        items: Vec<TrayMenuItem>,
    ) {
        if !self.hide_state.borrow().combined_mode {
            return;
        }
        let pending = self.pending_tray_menu.borrow_mut().take();
        let Some(req) = pending.filter(|p| p.key == key) else {
            return;
        };
        let (_, out_h) = self.context_menu.output_size();
        let menu_y = out_h - self.combined_bar_height() - 6;
        self.context_menu.open_tray_menu(
            req.anchor_x,
            menu_y,
            &req.app_title,
            &bus_name,
            &req.object_path,
            Some(&menu_path),
            &items,
            self.tray_tx.clone(),
        );
    }
}

fn sync_dock_visibility(
    state_rc: &Rc<RefCell<DockHideState>>,
    window: &ApplicationWindow,
    sensor_window: &ApplicationWindow,
    container: &GtkBox,
    delay_hide: bool,
) {
    let (desired_exclusive, should_show, enable_sensor, immediate_fullscreen) = {
        let st = state_rc.borrow();
        let dock_h = st.dock_height.clamp(36, 88) as i32;
        let eff_icon = (st.icon_size as i32).min((dock_h - 10).clamp(20, 64));
        let ex_sz = if st.combined_mode {
            dock_h.max(eff_icon + 12)
        } else {
            dock_h.max(eff_icon + if eff_icon <= 32 { 16 } else { 20 }) + 12
        };
        let popup_open = st.qs_open || st.nc_open || st.preview_open;
        if st.has_fullscreen && !st.overview_open && !st.workspaces_open {
            (0, false, false, true)
        } else if !st.auto_hide && !st.smart_hide {
            (ex_sz, true, false, false)
        } else {
            let show = st.overview_open
                || st.workspaces_open
                || popup_open
                || st.hovered_dock
                || st.hovered_sensor
                || (st.smart_hide && !st.auto_hide && !st.window_overlaps_dock);
            (0, show, !show, false)
        }
    };

    if gtk4_layer_shell::is_supported() {
        let prev_ex = state_rc.borrow().last_exclusive;
        if prev_ex != desired_exclusive {
            state_rc.borrow_mut().last_exclusive = desired_exclusive;
            window.set_exclusive_zone(desired_exclusive);
        }
    }

    if immediate_fullscreen {
        let mut st = state_rc.borrow_mut();
        st.anim_gen = st.anim_gen.wrapping_add(1);
        st.currently_shown = false;
        drop(st);
        sensor_window.set_visible(false);
        container.add_css_class("dock-hidden");
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
            container.add_css_class("dock-hidden");
            window.set_visible(true);
            window.present();
            let cont_c = container.clone();
            let st_c = Rc::clone(state_rc);
            let epoch = state_rc.borrow().anim_gen;
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                if st_c.borrow().anim_gen == epoch {
                    cont_c.remove_css_class("dock-hidden");
                }
            });
        } else {
            container.remove_css_class("dock-hidden");
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

    let leave_wait = if delay_hide { 380 } else { 0 };
    let st_c = Rc::clone(state_rc);
    let win_c = window.clone();
    let sen_c = sensor_window.clone();
    let cont_c = container.clone();

    glib::timeout_add_local_once(Duration::from_millis(leave_wait), move || {
        if st_c.borrow().anim_gen != epoch {
            return;
        }
        cont_c.add_css_class("dock-hidden");
        if enable_sensor {
            sen_c.set_visible(true);
            sen_c.present();
        }
        let st_done = Rc::clone(&st_c);
        let win_done = win_c.clone();
        glib::timeout_add_local_once(Duration::from_millis(235), move || {
            if st_done.borrow().anim_gen == epoch {
                st_done.borrow_mut().currently_shown = false;
                win_done.set_visible(false);
            }
        });
    });
}

/// Triggers an expressive Material 3 spring launch hop on a clicked Dock icon.
fn trigger_dock_launch_hop(btn: &Button) {
    btn.add_css_class("dock-launch-hop");
    let btn_c = btn.clone();
    glib::timeout_add_local_once(Duration::from_millis(190), move || {
        btn_c.remove_css_class("dock-launch-hop");
    });
}

fn dock_point_screen_coords(
    cm: &ContextMenuOverlay,
    dock_container: &GtkBox,
    pos: &str,
    local_x: f64,
    local_y: f64,
) -> (i32, i32) {
    let (out_w, out_h) = cm.output_size();
    let dock_w = dock_container.width().max(80);
    let dock_h = dock_container.height().max(64);
    match pos {
        "left" => {
            let dock_top = (out_h - dock_h) / 2;
            (dock_w + 18, dock_top + local_y.round() as i32)
        }
        "right" => {
            let dock_top = (out_h - dock_h) / 2;
            (out_w - dock_w - 18, dock_top + local_y.round() as i32)
        }
        _ => {
            let dock_left = (out_w - dock_w) / 2;
            (dock_left + local_x.round() as i32, out_h - dock_h - 14)
        }
    }
}

fn dock_widget_screen_coords(
    cm: &ContextMenuOverlay,
    dock_container: &GtkBox,
    widget: &impl IsA<gtk4::Widget>,
    pos: &str,
    local_x: f64,
    local_y: f64,
) -> (i32, i32) {
    let (rel_x, rel_y) = widget
        .compute_bounds(dock_container)
        .map(|b| (b.x() as f64, b.y() as f64))
        .unwrap_or((0.0, 0.0));
    dock_point_screen_coords(cm, dock_container, pos, rel_x + local_x, rel_y + local_y)
}

fn dock_band_px_for_preview(
    st: &DockHideState,
    dock_container: &GtkBox,
    taskbar_box: &CenterBox,
) -> i32 {
    let h = st.dock_height.clamp(36, 88) as i32;
    if st.combined_mode {
        taskbar_box.height().max(h)
    } else if st.position == "left" || st.position == "right" {
        dock_container.width().max(h) + 12
    } else {
        dock_container.height().max(h) + 12
    }
}

fn dock_button_center_screen_coords(
    cm: &ContextMenuOverlay,
    dock_container: &GtkBox,
    widget: &impl IsA<gtk4::Widget>,
    pos: &str,
) -> (i32, i32) {
    let (out_w, out_h) = cm.output_size();
    let dock_w = dock_container.width().max(80);
    let dock_h = dock_container.height().max(48);
    let (cx, cy) = widget
        .compute_bounds(dock_container)
        .map(|b| {
            (
                b.x() as f64 + b.width() as f64 * 0.5,
                b.y() as f64 + b.height() as f64 * 0.5,
            )
        })
        .unwrap_or((dock_w as f64 * 0.5, dock_h as f64 * 0.5));
    match pos {
        "left" => {
            let dock_top = (out_h - dock_h) / 2;
            ((dock_w / 2).max(20), dock_top + cy.round() as i32)
        }
        "right" => {
            let dock_top = (out_h - dock_h) / 2;
            (out_w - (dock_w / 2).max(20), dock_top + cy.round() as i32)
        }
        _ => {
            let dock_left = (out_w - dock_w) / 2;
            (dock_left + cx.round() as i32, out_h - (dock_h / 2).max(20))
        }
    }
}

fn make_widget_dock_button(
    child_widget: &impl IsA<gtk4::Widget>,
    tooltip: &str,
    running: bool,
    focused: bool,
) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-dock-item");
    btn.set_halign(Align::Center);
    btn.set_valign(Align::Center);
    if focused {
        btn.add_css_class("focused");
    }
    let _ = tooltip;
    // Custom DockLabelPopup renders above bottom dock/panel icons so titles never clip off-screen.
    btn.set_has_tooltip(false);

    // Overlay keeps the app icon dead-center (horizontally & vertically) inside symmetric slot
    // padding while the running/focused indicator dot sits cleanly at the bottom edge without
    // displacing the icon upward.
    let overlay = Overlay::new();
    overlay.set_halign(Align::Center);
    overlay.set_valign(Align::Center);

    let icon_slot = GtkBox::new(Orientation::Vertical, 0);
    icon_slot.add_css_class("neon-dock-icon-slot");
    icon_slot.set_halign(Align::Center);
    icon_slot.set_valign(Align::Center);
    icon_slot.append(child_widget);
    overlay.set_child(Some(&icon_slot));

    let dot = GtkBox::new(Orientation::Horizontal, 0);
    dot.add_css_class("neon-dock-dot");
    dot.set_halign(Align::Center);
    dot.set_valign(Align::End);
    dot.set_can_target(false);
    if running {
        dot.add_css_class("running");
    }
    if focused {
        dot.add_css_class("focused");
    }
    overlay.add_overlay(&dot);

    btn.set_child(Some(&overlay));
    btn
}

fn make_icon_dock_button(
    icon_spec: Option<&str>,
    icon_hint: &str,
    tooltip: &str,
    running: bool,
    focused: bool,
    pixel_size: i32,
) -> Button {
    let icon = load_app_icon(icon_spec, icon_hint, pixel_size);
    icon.set_size_request(pixel_size, pixel_size);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    make_widget_dock_button(&icon, tooltip, running, focused)
}

fn update_dock_button_icon(btn: &Button, icon_spec: Option<&str>, icon_hint: &str, pixel_size: i32) {
    if let Some(overlay) = btn.child().and_then(|w| w.downcast::<Overlay>().ok())
        && let Some(icon_slot) = overlay.child().and_then(|w| w.downcast::<GtkBox>().ok())
        && let Some(first) = icon_slot.first_child()
    {
        let next_icon = load_app_icon(icon_spec, icon_hint, pixel_size);
        next_icon.set_size_request(pixel_size, pixel_size);
        next_icon.set_halign(Align::Center);
        next_icon.set_valign(Align::Center);
        icon_slot.insert_child_after(&next_icon, Some(&first));
        icon_slot.remove(&first);
    }
}

/// Crisp optical-matched 3×3 App Drawer grid vector icon (occupies 76% of the icon slot so it
/// aligns cleanly in visual weight alongside normalized app icons at every Dock size).
fn draw_app_grid_icon(
    cr: &gtk4::cairo::Context,
    w: i32,
    h: i32,
    light: bool,
    drawer_hidden: bool,
) {
    let (r, g, b) = if light && !drawer_hidden {
        (0.08, 0.11, 0.18)
    } else {
        (0.95, 0.97, 1.0)
    };
    cr.set_antialias(gtk4::cairo::Antialias::Best);
    let dim = f64::from(w.min(h)).max(16.0);
    let ox = (f64::from(w) - dim) * 0.5;
    let oy = (f64::from(h) - dim) * 0.5;
    let grid_span = dim * 0.74;
    let cell = grid_span / 3.0;
    let dot_r = (cell * 0.30).max(1.5);
    let start_x = ox + (dim - grid_span) * 0.5 + cell * 0.5;
    let start_y = oy + (dim - grid_span) * 0.5 + cell * 0.5;

    cr.set_source_rgba(r, g, b, 0.94);
    for row in 0..3 {
        for col in 0..3 {
            let cx = start_x + f64::from(col) * cell;
            let cy = start_y + f64::from(row) * cell;
            cr.new_sub_path();
            cr.arc(cx, cy, dot_r, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }
}

fn resolve_window_icon(
    app_id: Option<&str>,
    title: Option<&str>,
    all_apps: &[DesktopApp],
) -> Option<String> {
    let needle_id = app_id.unwrap_or("").trim().to_lowercase();
    let needle_title = title.unwrap_or("").trim().to_lowercase();

    if needle_id == "org.neonde.settings"
        || needle_id.contains("neonde.settings")
        || (needle_id.is_empty() && needle_title == "neon de settings")
    {
        return Some("preferences-system".to_string());
    }

    if let Some(app) = resolve_window_desktop_app(app_id, title, all_apps)
        && let Some(ref icon) = app.icon
        && !icon.trim().is_empty()
    {
        return Some(icon.clone());
    }

    if matches!(
        needle_id.as_str(),
        "alacritty"
            | "kitty"
            | "foot"
            | "footclient"
            | "wezterm"
            | "org.wezfurlong.wezterm"
            | "com.mitchellh.ghostty"
            | "ghostty"
            | "weston-terminal"
            | "org.gnome.terminal"
            | "org.gnome.console"
            | "kgx"
            | "konsole"
            | "xterm"
    ) {
        return Some("utilities-terminal".to_string());
    }

    None
}

fn compute_dock_workspace_overlap(
    snapshot: &DesktopSnapshot,
    norm_pos: &str,
    dock_band: i32,
    dock_left: i32,
    dock_right: i32,
    out_w: i32,
    dock_top_zone: i32,
) -> (bool, bool) {
    let active_ws = snapshot
        .workspaces
        .iter()
        .find(|ws| ws.active)
        .or_else(|| snapshot.workspaces.get(snapshot.active_workspace_index));
    let active_ws_info: Option<(u64, HashSet<u64>)> =
        active_ws.map(|ws| (ws.id, ws.window_ids.iter().copied().collect()));

    let mut has_fullscreen = false;
    let mut window_overlaps_dock = false;

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
            window_overlaps_dock = true;
        } else if w.maximized {
            window_overlaps_dock = true;
        } else {
            let overlaps = match norm_pos {
                "left" => w.x < dock_band,
                "right" => (w.x + w.width) > (out_w - dock_band),
                _ => {
                    w.x < dock_right
                        && (w.x + w.width) > dock_left
                        && (w.y + w.height) > dock_top_zone
                }
            };
            if overlaps {
                window_overlaps_dock = true;
            }
        }
    }
    (has_fullscreen, window_overlaps_dock)
}

/// Gaussian macOS-style dock wave magnification level (`0..=8`).
///
/// The hovered icon reaches level 8, immediate left/right neighbors reach ~6,
/// second neighbors swell gently (~2), and distant icons stay at rest (`0`).
pub(crate) fn compute_wave_level(cursor_axis: f64, btn_center: f64, pitch: f64) -> u8 {
    let pitch = pitch.max(24.0);
    let dist = (cursor_axis - btn_center).abs() / pitch;
    if dist >= 3.2 {
        return 0;
    }
    let sigma = 1.24;
    let weight = (-0.5 * (dist / sigma).powi(2)).exp();
    (weight * 8.0).round().clamp(0.0, 8.0) as u8
}

fn collect_dock_item_buttons(container: &GtkBox) -> Vec<Button> {
    let mut out = Vec::new();
    let mut child = container.first_child();
    while let Some(w) = child {
        if w.is_visible() {
            if let Ok(btn) = w.clone().downcast::<Button>() {
                if btn.has_css_class("neon-dock-item") {
                    out.push(btn);
                }
            } else if let Ok(inner_box) = w.clone().downcast::<GtkBox>() {
                let mut sub = inner_box.first_child();
                while let Some(sw) = sub {
                    if sw.is_visible()
                        && let Ok(btn) = sw.clone().downcast::<Button>()
                        && btn.has_css_class("neon-dock-item")
                    {
                        out.push(btn);
                    }
                    sub = sw.next_sibling();
                }
            }
        }
        child = w.next_sibling();
    }
    out
}

fn strip_wave_classes(btn: &Button) {
    for cls in DOCK_MAG_CLASSES {
        btn.remove_css_class(cls);
    }
}

fn clear_dock_wave(wave: &Rc<RefCell<DockWaveState>>, container: &GtkBox) {
    let mut w = wave.borrow_mut();
    for (btn, _) in &w.rest_rel_centers {
        strip_wave_classes(btn);
    }
    for lvl in &mut w.levels {
        *lvl = 0;
    }
    if w.rest_rel_centers.is_empty() {
        for btn in collect_dock_item_buttons(container) {
            strip_wave_classes(&btn);
        }
    }
}

fn invalidate_dock_wave(wave: &Rc<RefCell<DockWaveState>>, container: &GtkBox) {
    clear_dock_wave(wave, container);
    let mut w = wave.borrow_mut();
    w.rest_rel_centers.clear();
    w.levels.clear();
    w.dirty = true;
}

fn update_dock_wave(
    wave: &Rc<RefCell<DockWaveState>>,
    container: &GtkBox,
    hide_state: &Rc<RefCell<DockHideState>>,
    mx: f64,
    my: f64,
) {
    let (mag, vertical, icon_size) = {
        let st = hide_state.borrow();
        let vertical = !st.combined_mode && (st.position == "left" || st.position == "right");
        (st.magnification, vertical, st.icon_size)
    };
    if !mag {
        clear_dock_wave(wave, container);
        return;
    }

    let mut w = wave.borrow_mut();
    let live_span = if vertical {
        f64::from(container.height())
    } else {
        f64::from(container.width())
    };
    if live_span < 20.0 {
        return;
    }

    if w.dirty || w.vertical != vertical || w.rest_rel_centers.is_empty() {
        let buttons = collect_dock_item_buttons(container);
        for btn in &buttons {
            strip_wave_classes(btn);
        }
        let half_span = live_span * 0.5;
        let mut centers = Vec::with_capacity(buttons.len());
        let mut raw_coords = Vec::with_capacity(buttons.len());
        for btn in buttons {
            if let Some(bounds) = btn.compute_bounds(container) {
                let c = if vertical {
                    f64::from(bounds.y()) + f64::from(bounds.height()) * 0.5
                } else {
                    f64::from(bounds.x()) + f64::from(bounds.width()) * 0.5
                };
                raw_coords.push(c);
                centers.push((btn, c - half_span));
            }
        }
        if centers.is_empty() {
            return;
        }
        let mut deltas: Vec<f64> = raw_coords
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .filter(|&d| d > 16.0 && d < 120.0)
            .collect();
        let pitch = if !deltas.is_empty() {
            deltas.sort_by(|a, b| a.total_cmp(b));
            deltas[deltas.len() / 2]
        } else {
            f64::from(icon_size.clamp(22, 64)) + 12.0
        };
        w.levels = vec![0; centers.len()];
        w.rest_rel_centers = centers;
        w.pitch = pitch;
        w.vertical = vertical;
        w.dirty = false;
    }

    let cursor_rel = (if vertical { my } else { mx }) - live_span * 0.5;
    let pitch = w.pitch;
    let DockWaveState {
        rest_rel_centers,
        levels,
        ..
    } = &mut *w;
    for ((btn, rel_center), cur_lvl) in rest_rel_centers.iter().zip(levels.iter_mut()) {
        let next_lvl = compute_wave_level(cursor_rel, *rel_center, pitch);
        if *cur_lvl != next_lvl {
            if *cur_lvl > 0 {
                btn.remove_css_class(DOCK_MAG_CLASSES[(*cur_lvl - 1) as usize]);
            }
            if next_lvl > 0 {
                btn.add_css_class(DOCK_MAG_CLASSES[(next_lvl - 1) as usize]);
            }
            *cur_lvl = next_lvl;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neon_ipc::{WindowInfo, WorkspaceInfo};

    #[test]
    fn empty_active_workspace_keeps_dock_visible_when_other_workspace_has_fullscreen_video() {
        let fs_video = WindowInfo {
            id: 42,
            workspace_id: 1,
            app_id: Some("brave-browser".into()),
            title: Some("Fullscreen Video".into()),
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
                    window_ids: vec![42],
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

        // Empty Workspace 2 must keep Dock visible (no fullscreen, no overlap).
        assert_eq!(
            compute_dock_workspace_overlap(&snap, "bottom", 78, 600, 1320, 1920, 1002),
            (false, false)
        );

        // Switching back to Workspace 1 hides Dock for the fullscreen video.
        snap.active_workspace_index = 0;
        snap.workspaces[0].active = true;
        snap.workspaces[1].active = false;
        assert_eq!(
            compute_dock_workspace_overlap(&snap, "bottom", 78, 600, 1320, 1920, 1002),
            (true, true)
        );
    }

    #[test]
    fn dock_wave_magnification_forms_gaussian_neighbor_bell_curve() {
        let pitch = 48.0;
        let center = 200.0;
        let peak = compute_wave_level(center, center, pitch);
        let neighbor_1 = compute_wave_level(center, center + pitch, pitch);
        let neighbor_1_left = compute_wave_level(center, center - pitch, pitch);
        let neighbor_2 = compute_wave_level(center, center + 2.0 * pitch, pitch);
        let far = compute_wave_level(center, center + 4.0 * pitch, pitch);

        assert_eq!(peak, 8);
        assert_eq!(neighbor_1, neighbor_1_left);
        assert!(neighbor_1 >= 5 && neighbor_1 < peak);
        assert!(neighbor_2 >= 1 && neighbor_2 < neighbor_1);
        assert_eq!(far, 0);
    }
}
