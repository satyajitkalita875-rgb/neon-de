//! NEON DE Frosted-Glass Quick Settings Panel (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Architecture:
//! - Uses a dedicated tight-fitting `"neon-quick-settings"` layer surface anchored Top+Right
//!   (or Bottom+Right in Combined Bottom mode) so `neon-renderer` renders a true real-time GPU
//!   Gaussian-blurred desktop backdrop strictly inside Quick Settings' rounded bounds.
//! - Paired with a transparent `"neon-quick-settings-backdrop"` click-catcher so clicking
//!   anywhere outside or pressing `Esc` dismisses Quick Settings cleanly.
//! - Clean, focused system control layout:
//!   1. Header: Battery indicator (left) + Power Menu trigger (right -> fullscreen centered Power Menu)
//!   2. Equal-length Wi-Fi & Bluetooth split pills with live expandable nearby networks / devices dropdowns
//!   3. Unified pixel-aligned Display Brightness & Sound Volume slider stack
//!   4. 3-button Power Mode segmented bar (`Power Saver | Balanced | Performance`)
//!   5. Equal-length `Dark Mode` toggle + `Screenshot` trigger

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, GestureClick,
    Image, Label, Orientation, PasswordEntry, Revealer, RevealerTransitionType, Scale, Separator,
    gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{AppearanceInfo, EffectsInfo, IpcRequest, WindowBehaviorInfo};
use neon_system::{
    SystemAction, SystemActionSender, SystemSnapshot,
    bluetooth::{
        BluetoothDevice, list_all_bluetooth_devices, pair_device, set_bluetooth_discovery,
        set_device_connected,
    },
    network::{WifiAccessPoint, connect_wifi, list_wifi_networks},
    power::{BatteryState, read_backlight_percent},
};

use crate::{
    ipc_bridge::IpcCommandSender,
    notifications::NotificationOverlay,
    power_menu::PowerMenuOverlay,
};

const QS_WIDTH: i32 = 348;

type StateChangeListeners = Rc<RefCell<Vec<Box<dyn Fn(bool)>>>>;
type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveDropdown {
    None,
    Wifi,
    Bluetooth,
}

enum DropdownBgEvent {
    WifiList(Vec<WifiAccessPoint>),
    WifiConnectResult {
        ssid: String,
        secured: bool,
        res: Result<(), String>,
    },
    BtList(Vec<BluetoothDevice>),
    BtActionResult {
        name: String,
        res: Result<(), String>,
    },
}

/// Live frosted-glass Quick Settings panel anchored beneath the top-right status cluster.
#[derive(Clone)]
pub struct QuickSettingsPanel {
    backdrop_window: ApplicationWindow,
    window: ApplicationWindow,
    ipc: IpcCommandSender,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    motion_enabled: Rc<Cell<bool>>,
    speed_scale: Rc<Cell<f64>>,
    on_before_open: VoidCallback,
    on_state_listeners: StateChangeListeners,
    syncing: Rc<Cell<bool>>,
    // Header
    header_battery_box: GtkBox,
    header_battery_icon: Image,
    header_battery_lbl: Label,
    header_title_lbl: Label,
    // Row 1: Equal-length Wi-Fi & Bluetooth split pills
    wifi_pill: GtkBox,
    wifi_main_btn: Button,
    wifi_sub: Label,
    wifi_arrow_icon: Image,
    wifi_enabled_state: Rc<Cell<bool>>,
    bt_pill: GtkBox,
    bt_main_btn: Button,
    bt_sub: Label,
    bt_arrow_icon: Image,
    bt_powered_state: Rc<Cell<bool>>,
    // Expandable dropdown drawer (Material 3 Expressive Revealer + spring card)
    active_dropdown: Rc<Cell<ActiveDropdown>>,
    dropdown_revealer: Revealer,
    dropdown_box: GtkBox,
    dropdown_title: Label,
    dropdown_status: Label,
    dropdown_list_box: GtkBox,
    wifi_pw_revealer: Revealer,
    wifi_pw_entry: PasswordEntry,
    wifi_pw_ssid: Rc<RefCell<Option<String>>>,
    bg_tx: mpsc::Sender<DropdownBgEvent>,
    // Row 2: Unified Display & Sound stack
    brightness_card: GtkBox,
    sliders_divider: Separator,
    brightness_pct_label: Label,
    brightness_scale: Scale,
    audio_card: GtkBox,
    vol_pct_label: Label,
    volume_scale: Scale,
    mute_btn: Button,
    mute_icon: Image,
    // Row 3: 3-button Power Mode segmented bar
    pm_saver_btn: Button,
    pm_balanced_btn: Button,
    pm_perf_btn: Button,
    // Row 4: Hyprland Auto-Tiling split pill (toggle + Dwindle/Master switcher)
    tiling_pill: GtkBox,
    tiling_sub: Label,
    tiling_layout_lbl: Label,
    auto_tiling_enabled: Rc<Cell<bool>>,
    tiling_layout_name: Rc<RefCell<String>>,
    // Row 5: Dark Mode + Screenshot
    dark_mode_btn: Button,
    dark_mode_icon: Image,
    is_light_theme: Rc<Cell<bool>>,
    anchor_bottom: Rc<Cell<bool>>,
    bottom_margin: Rc<Cell<i32>>,
    sys_tx: SystemActionSender,
    toast_overlay: NotificationOverlay,
}

impl QuickSettingsPanel {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        sys_tx: SystemActionSender,
        toast_overlay: NotificationOverlay,
        power_menu: PowerMenuOverlay,
    ) -> Self {
        // 1. Fullscreen transparent click-catcher backdrop
        let backdrop_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Quick Settings Backdrop")
            .decorated(false)
            .build();
        backdrop_window.add_css_class("neon-quicksettings-window");
        if gtk4_layer_shell::is_supported() {
            backdrop_window.init_layer_shell();
            backdrop_window.set_namespace(Some("neon-quick-settings-backdrop"));
            backdrop_window.set_layer(Layer::Overlay);
            backdrop_window.set_anchor(Edge::Top, true);
            backdrop_window.set_anchor(Edge::Bottom, true);
            backdrop_window.set_anchor(Edge::Left, true);
            backdrop_window.set_anchor(Edge::Right, true);
            backdrop_window.set_exclusive_zone(-1);
            backdrop_window.set_keyboard_mode(KeyboardMode::None);
        }
        let backdrop_box = GtkBox::new(Orientation::Vertical, 0);
        backdrop_box.add_css_class("neon-cc-backdrop");
        backdrop_box.set_hexpand(true);
        backdrop_box.set_vexpand(true);
        backdrop_window.set_child(Some(&backdrop_box));
        backdrop_window.set_visible(false);

        // 2. Tight-fitting Quick Settings surface anchored Top+Right so neon-renderer blurs
        //    its exact rounded rectangle in real time (even as dropdowns expand/collapse!).
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Quick Settings")
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("neon-quicksettings-window");
        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-quick-settings"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, 38);
            window.set_margin(Edge::Right, 12);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let syncing = Rc::new(Cell::new(false));
        let wifi_enabled_state = Rc::new(Cell::new(false));
        let bt_powered_state = Rc::new(Cell::new(false));
        let is_light_theme = Rc::new(Cell::new(false));
        let is_open = Rc::new(Cell::new(false));
        let anim_gen = Rc::new(Cell::new(0u64));
        let on_state_listeners: StateChangeListeners = Rc::new(RefCell::new(Vec::new()));
        let active_dropdown = Rc::new(Cell::new(ActiveDropdown::None));
        let wifi_pw_ssid: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let (bg_tx, bg_rx) = mpsc::channel::<DropdownBgEvent>();

        // Unified frosted-glass container card (open/close Material 3 Expressive spring is
        // driven at locked 60 FPS on the GPU by `neon-renderer` with zero GTK buffer reallocation).
        let card = GtkBox::new(Orientation::Vertical, 12);
        card.add_css_class("neon-qs-panel-card");
        card.set_size_request(QS_WIDTH, -1);

        // =========================================================================
        // HEADER: Battery Status (left) + Power Menu Button (right)
        // =========================================================================
        let header_row = GtkBox::new(Orientation::Horizontal, 8);
        header_row.set_valign(Align::Center);

        let header_battery_box = GtkBox::new(Orientation::Horizontal, 6);
        header_battery_box.add_css_class("neon-qs-status-pill");
        header_battery_box.set_valign(Align::Center);
        let header_battery_icon = Image::from_icon_name("battery-full-symbolic");
        header_battery_icon.set_pixel_size(15);
        let header_battery_lbl = Label::new(Some("100%"));
        header_battery_lbl.add_css_class("neon-cc-title");
        header_battery_box.append(&header_battery_icon);
        header_battery_box.append(&header_battery_lbl);

        let header_title_lbl = Label::new(Some("Quick Settings"));
        header_title_lbl.add_css_class("neon-cc-title");
        header_title_lbl.set_visible(false);

        let header_spacer = GtkBox::new(Orientation::Horizontal, 0);
        header_spacer.set_hexpand(true);

        let power_menu_btn = Button::new();
        power_menu_btn.add_css_class("neon-qs-header-power-btn");
        power_menu_btn.set_tooltip_text(Some("Power Menu"));
        let pm_btn_inner = GtkBox::new(Orientation::Horizontal, 6);
        pm_btn_inner.set_valign(Align::Center);
        let pm_btn_icon = Image::from_icon_name("system-shutdown-symbolic");
        pm_btn_icon.set_pixel_size(15);
        let pm_btn_lbl = Label::new(Some("Power"));
        pm_btn_lbl.add_css_class("neon-qs-header-btn-lbl");
        pm_btn_inner.append(&pm_btn_icon);
        pm_btn_inner.append(&pm_btn_lbl);
        power_menu_btn.set_child(Some(&pm_btn_inner));

        header_row.append(&header_battery_box);
        header_row.append(&header_title_lbl);
        header_row.append(&header_spacer);
        header_row.append(&power_menu_btn);
        card.append(&header_row);

        // =========================================================================
        // ROW 1: Equal-Length Wi-Fi & Bluetooth Split Pills with Dropdown Arrows
        // =========================================================================
        let conn_row = GtkBox::new(Orientation::Horizontal, 10);
        conn_row.set_homogeneous(true);
        conn_row.set_hexpand(true);

        let (wifi_pill, wifi_main_btn, wifi_sub, wifi_arrow_btn, wifi_arrow_icon) =
            build_split_connectivity_pill("network-wireless-symbolic", "Wi-Fi", "Checking…");
        {
            let sys_tx_w = sys_tx.clone();
            let wifi_state = Rc::clone(&wifi_enabled_state);
            wifi_main_btn.connect_clicked(move |_| {
                sys_tx_w.send(SystemAction::SetWifiEnabled(!wifi_state.get()));
            });
        }

        let (bt_pill, bt_main_btn, bt_sub, bt_arrow_btn, bt_arrow_icon) =
            build_split_connectivity_pill("bluetooth-active-symbolic", "Bluetooth", "Checking…");
        {
            let sys_tx_b = sys_tx.clone();
            let bt_state = Rc::clone(&bt_powered_state);
            bt_main_btn.connect_clicked(move |_| {
                sys_tx_b.send(SystemAction::SetBluetoothPowered(!bt_state.get()));
            });
        }

        conn_row.append(&wifi_pill);
        conn_row.append(&bt_pill);
        card.append(&conn_row);

        // =========================================================================
        // EXPANDABLE DROPDOWN DRAWER (Nearby Wi-Fi Networks / Bluetooth Devices)
        // =========================================================================
        let dropdown_revealer = Revealer::new();
        dropdown_revealer.set_transition_type(RevealerTransitionType::SlideDown);
        dropdown_revealer.set_transition_duration(210);
        dropdown_revealer.set_reveal_child(false);

        let dropdown_box = GtkBox::new(Orientation::Vertical, 8);
        dropdown_box.add_css_class("neon-qs-dropdown-card");
        dropdown_box.add_css_class("dd-closed");

        let dd_head = GtkBox::new(Orientation::Horizontal, 8);
        dd_head.set_valign(Align::Center);

        let dropdown_title = Label::new(Some("Wi-Fi Networks"));
        dropdown_title.add_css_class("neon-cc-title");
        dropdown_title.set_halign(Align::Start);

        let dropdown_status = Label::new(Some(""));
        dropdown_status.add_css_class("neon-cc-sub");
        dropdown_status.set_hexpand(true);
        dropdown_status.set_halign(Align::End);
        dropdown_status.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let dd_refresh_btn = Button::new();
        dd_refresh_btn.add_css_class("neon-cc-inline-icon-btn");
        dd_refresh_btn.set_tooltip_text(Some("Rescan"));
        let refresh_icon = Image::from_icon_name("view-refresh-symbolic");
        refresh_icon.set_pixel_size(14);
        dd_refresh_btn.set_child(Some(&refresh_icon));

        dd_head.append(&dropdown_title);
        dd_head.append(&dropdown_status);
        dd_head.append(&dd_refresh_btn);
        dropdown_box.append(&dd_head);

        // Inline Wi-Fi password bar wrapped in a smooth Revealer
        let wifi_pw_revealer = Revealer::new();
        wifi_pw_revealer.set_transition_type(RevealerTransitionType::SlideDown);
        wifi_pw_revealer.set_transition_duration(240);
        wifi_pw_revealer.set_reveal_child(false);

        let wifi_pw_box = GtkBox::new(Orientation::Horizontal, 6);
        let wifi_pw_entry = PasswordEntry::new();
        wifi_pw_entry.add_css_class("neon-qs-pw-entry");
        wifi_pw_entry.set_hexpand(true);
        wifi_pw_entry.set_show_peek_icon(true);
        wifi_pw_entry.set_placeholder_text(Some("Wi-Fi Password"));
        let wifi_pw_connect_btn = Button::with_label("Connect");
        wifi_pw_connect_btn.add_css_class("neon-power-confirm-btn");
        wifi_pw_box.append(&wifi_pw_entry);
        wifi_pw_box.append(&wifi_pw_connect_btn);
        wifi_pw_revealer.set_child(Some(&wifi_pw_box));
        dropdown_box.append(&wifi_pw_revealer);

        let dd_scroller = gtk4::ScrolledWindow::builder()
            .max_content_height(192)
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .build();
        let dropdown_list_box = GtkBox::new(Orientation::Vertical, 4);
        dd_scroller.set_child(Some(&dropdown_list_box));
        dropdown_box.append(&dd_scroller);
        dropdown_revealer.set_child(Some(&dropdown_box));
        card.append(&dropdown_revealer);

        // =========================================================================
        // ROW 2: Unified Pixel-Aligned Display & Sound Sliders Stack
        // =========================================================================
        let sliders_stack = GtkBox::new(Orientation::Vertical, 10);
        sliders_stack.add_css_class("neon-qs-inner-card");

        // --- Display Brightness ---
        let brightness_card = GtkBox::new(Orientation::Vertical, 5);
        let bright_head = GtkBox::new(Orientation::Horizontal, 8);
        bright_head.set_valign(Align::Center);
        let bright_title = Label::new(Some("Display"));
        bright_title.add_css_class("neon-cc-title");
        bright_title.set_hexpand(true);
        bright_title.set_halign(Align::Start);

        let initial_brightness = read_backlight_percent();
        let brightness_pct_label =
            Label::new(Some(&format!("{}%", initial_brightness.unwrap_or(100))));
        brightness_pct_label.add_css_class("neon-cc-sub");
        bright_head.append(&bright_title);
        bright_head.append(&brightness_pct_label);

        let bright_row = GtkBox::new(Orientation::Horizontal, 8);
        bright_row.set_valign(Align::Center);
        let dim_sun = centered_slot_icon("display-brightness-symbolic", 15, 24, true);
        let brightness_scale = Scale::with_range(Orientation::Horizontal, 5.0, 100.0, 1.0);
        brightness_scale.add_css_class("neon-cc-slider");
        brightness_scale.set_draw_value(false);
        brightness_scale.set_hexpand(true);
        brightness_scale.set_valign(Align::Center);
        brightness_scale.set_value(f64::from(initial_brightness.unwrap_or(100)));
        {
            let sys_tx_br = sys_tx.clone();
            let syncing_c = Rc::clone(&syncing);
            let pct_lbl = brightness_pct_label.clone();
            brightness_scale.connect_value_changed(move |s| {
                let pct = s.value().round() as u32;
                pct_lbl.set_text(&format!("{pct}%"));
                if syncing_c.get() {
                    return;
                }
                sys_tx_br.send(SystemAction::SetBrightness(pct));
            });
        }
        let max_sun = centered_slot_icon("display-brightness-symbolic", 17, 24, false);
        bright_row.append(&dim_sun);
        bright_row.append(&brightness_scale);
        bright_row.append(&max_sun);

        brightness_card.append(&bright_head);
        brightness_card.append(&bright_row);
        brightness_card.set_visible(initial_brightness.is_some());

        let sliders_divider = Separator::new(Orientation::Horizontal);
        sliders_divider.add_css_class("neon-cc-slider-divider");
        sliders_divider.set_visible(initial_brightness.is_some());

        // --- Sound (exact same 24px icon slots for pixel-perfect slider alignment) ---
        let audio_card = GtkBox::new(Orientation::Vertical, 5);
        let audio_head = GtkBox::new(Orientation::Horizontal, 8);
        audio_head.set_valign(Align::Center);
        let sound_title = Label::new(Some("Sound"));
        sound_title.add_css_class("neon-cc-title");
        sound_title.set_hexpand(true);
        sound_title.set_halign(Align::Start);

        let vol_pct_label = Label::new(Some("70%"));
        vol_pct_label.add_css_class("neon-cc-sub");
        audio_head.append(&sound_title);
        audio_head.append(&vol_pct_label);

        let sound_row = GtkBox::new(Orientation::Horizontal, 8);
        sound_row.set_valign(Align::Center);

        let mute_icon = Image::from_icon_name("audio-volume-low-symbolic");
        mute_icon.set_pixel_size(15);
        mute_icon.set_halign(Align::Center);
        mute_icon.set_valign(Align::Center);
        let mute_btn = Button::new();
        mute_btn.add_css_class("neon-cc-inline-icon-btn");
        mute_btn.set_size_request(24, 24);
        mute_btn.set_hexpand(false);
        mute_btn.set_vexpand(false);
        mute_btn.set_halign(Align::Center);
        mute_btn.set_valign(Align::Center);
        mute_btn.set_child(Some(&mute_icon));
        mute_btn.set_tooltip_text(Some("Mute / Unmute"));
        {
            let sys_tx_m = sys_tx.clone();
            mute_btn.connect_clicked(move |_| {
                sys_tx_m.send(SystemAction::ToggleSinkMute);
            });
        }

        let volume_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 2.0);
        volume_scale.add_css_class("neon-cc-slider");
        volume_scale.set_draw_value(false);
        volume_scale.set_hexpand(true);
        volume_scale.set_valign(Align::Center);
        volume_scale.set_value(70.0);
        {
            let sys_tx_v = sys_tx.clone();
            let syncing_c = Rc::clone(&syncing);
            let pct_lbl = vol_pct_label.clone();
            volume_scale.connect_value_changed(move |s| {
                pct_lbl.set_text(&format!("{:.0}%", s.value()));
                if syncing_c.get() {
                    return;
                }
                let vol = (s.value() as f32) / 100.0;
                sys_tx_v.send(SystemAction::SetSinkVolume(vol));
            });
        }
        let max_vol = centered_slot_icon("audio-volume-high-symbolic", 16, 24, false);

        sound_row.append(&mute_btn);
        sound_row.append(&volume_scale);
        sound_row.append(&max_vol);

        audio_card.append(&audio_head);
        audio_card.append(&sound_row);

        sliders_stack.append(&brightness_card);
        sliders_stack.append(&sliders_divider);
        sliders_stack.append(&audio_card);
        card.append(&sliders_stack);

        // =========================================================================
        // ROW 3: Power Mode — 3-Button Segmented Pill (matching Settings)
        // =========================================================================
        let pm_section = GtkBox::new(Orientation::Vertical, 6);
        pm_section.add_css_class("neon-qs-inner-card");

        let pm_hdr = Label::new(Some("Power Mode"));
        pm_hdr.add_css_class("neon-cc-title");
        pm_hdr.set_halign(Align::Start);

        let pm_track = GtkBox::new(Orientation::Horizontal, 4);
        pm_track.add_css_class("neon-qs-seg-track");
        pm_track.set_homogeneous(true);

        let pm_saver_btn = build_power_seg_btn("Power Saver");
        let pm_balanced_btn = build_power_seg_btn("Balanced");
        pm_balanced_btn.add_css_class("active");
        let pm_perf_btn = build_power_seg_btn("Performance");

        for (btn, profile) in [
            (&pm_saver_btn, "power-saver"),
            (&pm_balanced_btn, "balanced"),
            (&pm_perf_btn, "performance"),
        ] {
            let tx = sys_tx.clone();
            let b_saver = pm_saver_btn.clone();
            let b_bal = pm_balanced_btn.clone();
            let b_perf = pm_perf_btn.clone();
            let prof = profile.to_string();
            btn.connect_clicked(move |_| {
                set_active_power_seg(&b_saver, &b_bal, &b_perf, &prof);
                tx.send(SystemAction::SetPowerProfile(prof.clone()));
            });
        }

        pm_track.append(&pm_saver_btn);
        pm_track.append(&pm_balanced_btn);
        pm_track.append(&pm_perf_btn);

        pm_section.append(&pm_hdr);
        pm_section.append(&pm_track);
        card.append(&pm_section);

        // =========================================================================
        // ROW 4: Hyprland Auto-Tiling Split Pill (Global Toggle + Layout Switcher)
        // =========================================================================
        let auto_tiling_enabled = Rc::new(Cell::new(false));
        let tiling_layout_name = Rc::new(RefCell::new("dwindle".to_string()));
        let (tiling_pill, tiling_main_btn, tiling_sub, tiling_layout_btn, tiling_layout_lbl) =
            build_split_tiling_pill();
        {
            let ipc_t = ipc.clone();
            let enabled_c = Rc::clone(&auto_tiling_enabled);
            let layout_c = Rc::clone(&tiling_layout_name);
            let pill_c = tiling_pill.clone();
            let sub_c = tiling_sub.clone();
            let lbl_c = tiling_layout_lbl.clone();
            tiling_main_btn.connect_clicked(move |_| {
                let next = !enabled_c.get();
                enabled_c.set(next);
                sync_tiling_pill_ui(&pill_c, &sub_c, &lbl_c, next, &layout_c.borrow());
                ipc_t.send(IpcRequest::UpdateWindowBehavior {
                    focus_follows_mouse: None,
                    snap_gap: None,
                    tiling_gap: None,
                    button_layout: None,
                    auto_tiling: Some(next),
                    tiling_layout: None,
                    auto_snap: None,
                    snap_layouts_bar: None,
                });
            });
        }
        {
            let ipc_t = ipc.clone();
            let enabled_c = Rc::clone(&auto_tiling_enabled);
            let layout_c = Rc::clone(&tiling_layout_name);
            let pill_c = tiling_pill.clone();
            let sub_c = tiling_sub.clone();
            let lbl_c = tiling_layout_lbl.clone();
            tiling_layout_btn.connect_clicked(move |_| {
                let next_layout = if layout_c.borrow().eq_ignore_ascii_case("master_stack") {
                    "dwindle"
                } else {
                    "master_stack"
                };
                *layout_c.borrow_mut() = next_layout.to_string();
                sync_tiling_pill_ui(&pill_c, &sub_c, &lbl_c, enabled_c.get(), next_layout);
                ipc_t.send(IpcRequest::UpdateWindowBehavior {
                    focus_follows_mouse: None,
                    snap_gap: None,
                    tiling_gap: None,
                    button_layout: None,
                    auto_tiling: None,
                    tiling_layout: Some(next_layout.to_string()),
                    auto_snap: None,
                    snap_layouts_bar: None,
                });
            });
        }
        card.append(&tiling_pill);

        // =========================================================================
        // ROW 5: Equal-Length Dark Mode Toggle + Screenshot Pill
        // =========================================================================
        let actions_row = GtkBox::new(Orientation::Horizontal, 10);
        actions_row.set_homogeneous(true);
        actions_row.set_hexpand(true);

        let (dark_mode_btn, dark_mode_icon) =
            build_action_pill("weather-clear-night-symbolic", "Dark Mode");
        dark_mode_btn.add_css_class("active");
        {
            let ipc_t = ipc.clone();
            let is_light = Rc::clone(&is_light_theme);
            dark_mode_btn.connect_clicked(move |_| {
                let next_theme = if is_light.get() {
                    "neon_dark"
                } else {
                    "neon_light"
                };
                ipc_t.send(IpcRequest::from(neon_ipc::AppearanceUpdate {
                    theme: Some(next_theme.to_string()),
                    ..Default::default()
                }));
            });
        }

        let (shot_btn, _) = build_action_pill("camera-photo-symbolic", "Screenshot");
        shot_btn.set_tooltip_text(Some("Screenshot & Screen Record"));

        actions_row.append(&dark_mode_btn);
        actions_row.append(&shot_btn);
        card.append(&actions_row);

        window.set_child(Some(&card));
        window.set_visible(false);

        let panel = Self {
            backdrop_window,
            window,
            ipc: ipc.clone(),
            is_open,
            anim_gen,
            motion_enabled: Rc::new(Cell::new(true)),
            speed_scale: Rc::new(Cell::new(1.0)),
            on_before_open: Rc::new(RefCell::new(None)),
            on_state_listeners,
            syncing,
            header_battery_box,
            header_battery_icon,
            header_battery_lbl,
            header_title_lbl,
            wifi_pill,
            wifi_main_btn,
            wifi_sub,
            wifi_arrow_icon,
            wifi_enabled_state,
            bt_pill,
            bt_main_btn,
            bt_sub,
            bt_arrow_icon,
            bt_powered_state,
            active_dropdown,
            dropdown_revealer,
            dropdown_box,
            dropdown_title,
            dropdown_status,
            dropdown_list_box,
            wifi_pw_revealer,
            wifi_pw_entry,
            wifi_pw_ssid,
            bg_tx,
            brightness_card,
            sliders_divider,
            brightness_pct_label,
            brightness_scale,
            audio_card,
            vol_pct_label,
            volume_scale,
            mute_btn,
            mute_icon,
            pm_saver_btn,
            pm_balanced_btn,
            pm_perf_btn,
            tiling_pill,
            tiling_sub,
            tiling_layout_lbl,
            auto_tiling_enabled,
            tiling_layout_name,
            dark_mode_btn,
            dark_mode_icon,
            is_light_theme,
            anchor_bottom: Rc::new(Cell::new(false)),
            bottom_margin: Rc::new(Cell::new(82)),
            sys_tx,
            toast_overlay,
        };

        // Wire Power Menu button -> closes Quick Settings cleanly and opens Power Menu
        {
            let p = panel.clone();
            let pm = power_menu;
            power_menu_btn.connect_clicked(move |_| {
                p.close_immediate();
                pm.open();
            });
        }

        // Wire Screenshot button -> closes Quick Settings cleanly and opens Screenshot UI
        {
            let p = panel.clone();
            let ipc_s = ipc;
            shot_btn.connect_clicked(move |_| {
                p.close_immediate();
                ipc_s.send(IpcRequest::OpenScreenshotUi);
            });
        }

        // Wire Wi-Fi & Bluetooth dropdown toggle arrows
        {
            let p = panel.clone();
            wifi_arrow_btn.connect_clicked(move |_| {
                p.toggle_dropdown(ActiveDropdown::Wifi);
            });
        }
        {
            let p = panel.clone();
            bt_arrow_btn.connect_clicked(move |_| {
                p.toggle_dropdown(ActiveDropdown::Bluetooth);
            });
        }

        // Wire dropdown refresh / scan button
        {
            let p = panel.clone();
            dd_refresh_btn.connect_clicked(move |_| match p.active_dropdown.get() {
                ActiveDropdown::Wifi => p.load_wifi_networks(true),
                ActiveDropdown::Bluetooth => p.load_bluetooth_devices(true),
                ActiveDropdown::None => {}
            });
        }

        // Wire inline Wi-Fi password submit (button click + Enter key)
        {
            let p = panel.clone();
            wifi_pw_connect_btn.connect_clicked(move |_| p.submit_wifi_password());
        }
        {
            let p = panel.clone();
            panel
                .wifi_pw_entry
                .connect_activate(move |_| p.submit_wifi_password());
        }

        // Clicking anywhere on the fullscreen backdrop closes Quick Settings immediately
        {
            let outside_click = GestureClick::new();
            outside_click.set_button(0);
            outside_click.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let p = panel.clone();
            outside_click.connect_pressed(move |_, _, _, _| {
                if p.is_open.get() {
                    p.close();
                }
            });
            panel.backdrop_window.add_controller(outside_click);
        }

        // Escape key on either window closes Quick Settings
        for w in [&panel.window, &panel.backdrop_window] {
            let key_ctrl = EventControllerKey::new();
            let p = panel.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape && p.is_open.get() {
                    p.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            w.add_controller(key_ctrl);
        }

        // Poll background Wi-Fi / Bluetooth worker responses cleanly on the GTK main loop
        {
            let p = panel.clone();
            glib::timeout_add_local(Duration::from_millis(80), move || {
                while let Ok(evt) = bg_rx.try_recv() {
                    p.handle_bg_event(evt);
                }
                glib::ControlFlow::Continue
            });
        }

        panel
    }

    pub fn set_on_before_open<F: Fn() + 'static>(&self, cb: F) {
        *self.on_before_open.borrow_mut() = Some(Box::new(cb));
    }

    pub fn add_on_state_change<F: Fn(bool) + 'static>(&self, cb: F) {
        self.on_state_listeners.borrow_mut().push(Box::new(cb));
    }

    pub fn update_effects(&self, effects: &EffectsInfo) {
        self.motion_enabled
            .set(effects.enabled && !effects.reduced_motion);
        self.speed_scale.set(effects.speed_scale.clamp(0.5, 2.5));
    }

    /// Switches Quick Settings anchor between Top-Right and Bottom-Right (Combined Taskbar mode).
    pub fn set_anchor_bottom(&self, bottom: bool, bottom_margin: i32) {
        let m = bottom_margin.clamp(48, 160);
        if self.anchor_bottom.get() == bottom && (!bottom || self.bottom_margin.get() == m) {
            return;
        }
        self.anchor_bottom.set(bottom);
        self.bottom_margin.set(m);
        if gtk4_layer_shell::is_supported() {
            if bottom {
                self.window.set_anchor(Edge::Top, false);
                self.window.set_anchor(Edge::Bottom, true);
                self.window.set_margin(Edge::Bottom, m);
            } else {
                self.window.set_anchor(Edge::Bottom, false);
                self.window.set_anchor(Edge::Top, true);
                self.window.set_margin(Edge::Top, 38);
            }
            self.window.set_anchor(Edge::Right, true);
            self.window.set_margin(Edge::Right, 12);
        }
    }

    pub fn toggle(&self) {
        if self.is_open.get() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(&self) {
        if let Some(cb) = self.on_before_open.borrow().as_ref() {
            cb();
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(true);
        self.notify_state_change(true);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        }

        self.toast_overlay.set_suppressed(true);
        self.sys_tx.send(SystemAction::RefreshNow);
        self.refresh_brightness();

        // Trigger compositor-driven 60 FPS GPU Material 3 Expressive entrance spring.
        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-quick-settings".into(),
            opening: true,
        });

        // Always map backdrop first, then Quick Settings surface on top at fixed size (zero resize churn).
        self.backdrop_window.set_visible(true);
        self.backdrop_window.present();
        self.backdrop_window.queue_draw();
        self.window.set_visible(true);
        self.window.present();
    }

    /// Immediately closes Quick Settings with zero delayed hide timers (used when handing off
    /// directly to another overlay or when animations are disabled).
    pub fn close_immediate(&self) {
        let was_active = self.is_open.get() || self.window.is_visible();
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        self.collapse_dropdown_immediate();
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::None);
        }
        self.backdrop_window.set_visible(false);
        self.window.set_visible(false);
        self.toast_overlay.set_suppressed(false);
        if was_active {
            self.notify_state_change(false);
        }
    }

    pub fn close(&self) {
        if !self.is_open.get() && !self.window.is_visible() {
            return;
        }
        if !self.motion_enabled.get() {
            self.close_immediate();
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        self.notify_state_change(false);

        // Immediately release the fullscreen click-catcher backdrop so desktop/app clicks
        // aren't blocked while the Quick Settings card finishes its exit glide.
        self.backdrop_window.set_visible(false);

        // Trigger smooth 60 FPS GPU glide + fade exit in neon-renderer.
        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-quick-settings".into(),
            opening: false,
        });

        let speed = self.speed_scale.get().clamp(0.5, 2.5);
        let hide_ms = ((235.0 / speed).round() as u64).clamp(90, 500);

        let this = self.clone();
        let gen_c = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(hide_ms), move || {
            if gen_c.get() == epoch {
                this.collapse_dropdown_immediate();
                if gtk4_layer_shell::is_supported() {
                    this.window.set_keyboard_mode(KeyboardMode::None);
                }
                this.window.set_visible(false);
                this.backdrop_window.set_visible(false);
                this.toast_overlay.set_suppressed(false);
            }
        });
    }

    fn collapse_dropdown_immediate(&self) {
        self.active_dropdown.set(ActiveDropdown::None);
        self.dropdown_revealer.set_transition_duration(0);
        self.dropdown_revealer.set_reveal_child(false);
        self.dropdown_box.remove_css_class("dd-open");
        self.dropdown_box.remove_css_class("dd-swap");
        self.dropdown_box.add_css_class("dd-closed");
        self.wifi_pw_revealer.set_reveal_child(false);
        *self.wifi_pw_ssid.borrow_mut() = None;
        self.wifi_arrow_icon.remove_css_class("expanded");
        self.bt_arrow_icon.remove_css_class("expanded");
    }

    fn collapse_dropdown(&self) {
        self.active_dropdown.set(ActiveDropdown::None);
        self.dropdown_revealer.set_transition_duration(170);
        self.dropdown_revealer.set_reveal_child(false);
        self.dropdown_box.remove_css_class("dd-open");
        self.dropdown_box.remove_css_class("dd-swap");
        self.dropdown_box.add_css_class("dd-closed");
        self.wifi_pw_revealer.set_reveal_child(false);
        *self.wifi_pw_ssid.borrow_mut() = None;
        self.wifi_arrow_icon.remove_css_class("expanded");
        self.bt_arrow_icon.remove_css_class("expanded");
        if gtk4_layer_shell::is_supported() && self.is_open.get() {
            self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        }
    }

    fn toggle_dropdown(&self, target: ActiveDropdown) {
        let prev = self.active_dropdown.get();
        if prev == target {
            self.collapse_dropdown();
            return;
        }

        let was_open = prev != ActiveDropdown::None;
        self.active_dropdown.set(target);
        self.wifi_pw_revealer.set_reveal_child(false);
        *self.wifi_pw_ssid.borrow_mut() = None;

        match target {
            ActiveDropdown::Wifi => {
                self.wifi_arrow_icon.add_css_class("expanded");
                self.bt_arrow_icon.remove_css_class("expanded");
                self.dropdown_title.set_text("Wi-Fi Networks");
                self.load_wifi_networks(false);
                self.load_wifi_networks(true);
            }
            ActiveDropdown::Bluetooth => {
                self.bt_arrow_icon.add_css_class("expanded");
                self.wifi_arrow_icon.remove_css_class("expanded");
                self.dropdown_title.set_text("Bluetooth Devices");
                self.load_bluetooth_devices(false);
            }
            ActiveDropdown::None => {
                self.collapse_dropdown();
                return;
            }
        }

        self.dropdown_revealer.set_transition_duration(210);
        self.dropdown_revealer.set_reveal_child(true);

        if was_open {
            // Quick expressive swap pulse when switching directly between Wi-Fi and Bluetooth
            self.dropdown_box.remove_css_class("dd-open");
            self.dropdown_box.add_css_class("dd-swap");
            let dd = self.dropdown_box.clone();
            glib::timeout_add_local_once(Duration::from_millis(24), move || {
                dd.remove_css_class("dd-swap");
                dd.add_css_class("dd-open");
            });
        } else {
            self.dropdown_box.remove_css_class("dd-open");
            self.dropdown_box.add_css_class("dd-closed");
            let dd = self.dropdown_box.clone();
            glib::timeout_add_local_once(Duration::from_millis(18), move || {
                dd.remove_css_class("dd-closed");
                dd.add_css_class("dd-open");
            });
        }
    }

    /// Programmatically opens the Wi-Fi or Bluetooth dropdown (used for UI verification).
    pub fn open_dropdown_for_test(&self, which: &str) {
        match which {
            "wifi" => self.toggle_dropdown(ActiveDropdown::Wifi),
            "bluetooth" => self.toggle_dropdown(ActiveDropdown::Bluetooth),
            _ => {}
        }
    }

    fn load_wifi_networks(&self, rescan: bool) {
        if !rescan {
            self.clear_dropdown_list();
            self.append_dropdown_placeholder("Loading nearby networks…");
        }
        self.dropdown_status.set_text(if rescan {
            "Scanning…"
        } else {
            "Loading…"
        });
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let nets = list_wifi_networks(rescan);
            let _ = tx.send(DropdownBgEvent::WifiList(nets));
        });
    }

    fn load_bluetooth_devices(&self, discover: bool) {
        if !discover {
            self.clear_dropdown_list();
            self.append_dropdown_placeholder("Loading Bluetooth devices…");
        }
        self.dropdown_status.set_text(if discover {
            "Scanning…"
        } else {
            "Loading…"
        });
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            if discover {
                let _ = set_bluetooth_discovery(true);
                std::thread::sleep(Duration::from_millis(1600));
            }
            let devs = list_all_bluetooth_devices();
            let _ = tx.send(DropdownBgEvent::BtList(devs));
            if discover {
                std::thread::sleep(Duration::from_millis(2800));
                let devs2 = list_all_bluetooth_devices();
                let _ = tx.send(DropdownBgEvent::BtList(devs2));
                let _ = set_bluetooth_discovery(false);
            }
        });
    }

    fn submit_wifi_password(&self) {
        let Some(ssid) = self.wifi_pw_ssid.borrow().clone() else {
            return;
        };
        let pw = self.wifi_pw_entry.text().to_string();
        if pw.is_empty() {
            return;
        }
        self.wifi_pw_entry.set_text("");
        self.wifi_pw_revealer.set_reveal_child(false);
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        }
        self.dropdown_status.set_text(&format!("Connecting to {ssid}…"));
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let res = connect_wifi(&ssid, Some(&pw));
            let _ = tx.send(DropdownBgEvent::WifiConnectResult {
                ssid,
                secured: true,
                res,
            });
        });
    }

    fn handle_bg_event(&self, evt: DropdownBgEvent) {
        match evt {
            DropdownBgEvent::WifiList(aps) => {
                if self.active_dropdown.get() != ActiveDropdown::Wifi {
                    return;
                }
                self.dropdown_status.set_text(&format!("{} found", aps.len()));
                self.clear_dropdown_list();
                if !self.wifi_enabled_state.get() {
                    self.append_dropdown_placeholder("Wi-Fi is turned off");
                    return;
                }
                if aps.is_empty() {
                    self.append_dropdown_placeholder("No nearby Wi-Fi networks found");
                    return;
                }
                for ap in aps {
                    let row_btn = Button::new();
                    row_btn.add_css_class("neon-qs-dd-item");
                    if ap.in_use {
                        row_btn.add_css_class("connected");
                    }

                    let hbox = GtkBox::new(Orientation::Horizontal, 8);
                    hbox.set_valign(Align::Center);

                    let sig_icon = Image::from_icon_name(wifi_signal_icon(ap.strength));
                    sig_icon.set_pixel_size(15);

                    let ssid_lbl = Label::new(Some(&ap.ssid));
                    ssid_lbl.add_css_class("neon-qs-dd-name");
                    ssid_lbl.set_hexpand(true);
                    ssid_lbl.set_halign(Align::Start);
                    ssid_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    ssid_lbl.set_max_width_chars(22);

                    hbox.append(&sig_icon);
                    hbox.append(&ssid_lbl);

                    let secured = !ap.security.eq_ignore_ascii_case("open");
                    if secured && !ap.in_use {
                        let lock_ic = Image::from_icon_name("network-wireless-encrypted-symbolic");
                        lock_ic.set_pixel_size(12);
                        lock_ic.set_opacity(0.70);
                        hbox.append(&lock_ic);
                    }

                    if ap.in_use {
                        let badge = Label::new(Some("Connected"));
                        badge.add_css_class("neon-qs-dd-badge");
                        hbox.append(&badge);
                    }

                    row_btn.set_child(Some(&hbox));

                    let ssid = ap.ssid.clone();
                    let in_use = ap.in_use;
                    let p = self.clone();
                    row_btn.connect_clicked(move |_| {
                        if in_use {
                            return;
                        }
                        p.wifi_pw_revealer.set_reveal_child(false);
                        p.dropdown_status.set_text(&format!("Connecting to {ssid}…"));
                        let tx = p.bg_tx.clone();
                        let s_clone = ssid.clone();
                        std::thread::spawn(move || {
                            let res = connect_wifi(&s_clone, None);
                            let _ = tx.send(DropdownBgEvent::WifiConnectResult {
                                ssid: s_clone,
                                secured,
                                res,
                            });
                        });
                    });

                    self.dropdown_list_box.append(&row_btn);
                }
            }
            DropdownBgEvent::WifiConnectResult { ssid, secured, res } => {
                if self.active_dropdown.get() != ActiveDropdown::Wifi {
                    return;
                }
                match res {
                    Ok(()) => {
                        self.dropdown_status.set_text(&format!("Connected to {ssid}"));
                        self.wifi_pw_revealer.set_reveal_child(false);
                        *self.wifi_pw_ssid.borrow_mut() = None;
                        self.sys_tx.send(SystemAction::RefreshNow);
                        self.load_wifi_networks(false);
                    }
                    Err(err) => {
                        if secured {
                            *self.wifi_pw_ssid.borrow_mut() = Some(ssid.clone());
                            self.wifi_pw_entry
                                .set_placeholder_text(Some(&format!("Password for {ssid}")));
                            self.wifi_pw_revealer.set_reveal_child(true);
                            if gtk4_layer_shell::is_supported() {
                                self.window.set_keyboard_mode(KeyboardMode::Exclusive);
                            }
                            self.wifi_pw_entry.grab_focus();
                            self.dropdown_status.set_text("Enter password");
                        } else {
                            let short: String = err.chars().take(28).collect();
                            self.dropdown_status.set_text(&short);
                        }
                    }
                }
            }
            DropdownBgEvent::BtList(devs) => {
                if self.active_dropdown.get() != ActiveDropdown::Bluetooth {
                    return;
                }
                self.dropdown_status.set_text(&format!("{} devices", devs.len()));
                self.clear_dropdown_list();
                if !self.bt_powered_state.get() {
                    self.append_dropdown_placeholder("Bluetooth is turned off");
                    return;
                }
                if devs.is_empty() {
                    self.append_dropdown_placeholder("No Bluetooth devices — click refresh to scan");
                    return;
                }
                for dev in devs {
                    let row_btn = Button::new();
                    row_btn.add_css_class("neon-qs-dd-item");
                    if dev.connected {
                        row_btn.add_css_class("connected");
                    }

                    let hbox = GtkBox::new(Orientation::Horizontal, 8);
                    hbox.set_valign(Align::Center);

                    let icon_name = bt_device_icon(dev.icon.as_deref());
                    let ic = Image::from_icon_name(icon_name);
                    ic.set_pixel_size(15);

                    let name_lbl = Label::new(Some(&dev.name));
                    name_lbl.add_css_class("neon-qs-dd-name");
                    name_lbl.set_hexpand(true);
                    name_lbl.set_halign(Align::Start);
                    name_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                    name_lbl.set_max_width_chars(20);

                    let state_text = if dev.connected {
                        "Connected"
                    } else if dev.paired {
                        "Paired"
                    } else {
                        "Pair"
                    };
                    let state_lbl = Label::new(Some(state_text));
                    state_lbl.add_css_class(if dev.connected {
                        "neon-qs-dd-badge"
                    } else {
                        "neon-cc-sub"
                    });

                    hbox.append(&ic);
                    hbox.append(&name_lbl);
                    hbox.append(&state_lbl);
                    row_btn.set_child(Some(&hbox));

                    let obj_path = dev.object_path.clone();
                    let name = dev.name.clone();
                    let connected = dev.connected;
                    let paired = dev.paired;
                    let p = self.clone();
                    row_btn.connect_clicked(move |_| {
                        let verb = if connected {
                            "Disconnecting"
                        } else if paired {
                            "Connecting"
                        } else {
                            "Pairing"
                        };
                        p.dropdown_status.set_text(&format!("{verb} {name}…"));
                        let tx = p.bg_tx.clone();
                        let path_c = obj_path.clone();
                        let name_c = name.clone();
                        std::thread::spawn(move || {
                            let res = if connected {
                                set_device_connected(&path_c, false)
                            } else if paired {
                                set_device_connected(&path_c, true)
                            } else {
                                pair_device(&path_c)
                                    .and_then(|()| set_device_connected(&path_c, true))
                            };
                            let _ = tx.send(DropdownBgEvent::BtActionResult { name: name_c, res });
                        });
                    });

                    self.dropdown_list_box.append(&row_btn);
                }
            }
            DropdownBgEvent::BtActionResult { name, res } => {
                if self.active_dropdown.get() != ActiveDropdown::Bluetooth {
                    return;
                }
                match res {
                    Ok(()) => self.dropdown_status.set_text(&format!("Updated {name}")),
                    Err(_) => self.dropdown_status.set_text("Connection failed"),
                }
                self.sys_tx.send(SystemAction::RefreshNow);
                self.load_bluetooth_devices(false);
            }
        }
    }

    fn clear_dropdown_list(&self) {
        while let Some(child) = self.dropdown_list_box.first_child() {
            self.dropdown_list_box.remove(&child);
        }
    }

    fn append_dropdown_placeholder(&self, text: &str) {
        let lbl = Label::new(Some(text));
        lbl.add_css_class("neon-cc-sub");
        lbl.set_margin_top(6);
        lbl.set_margin_bottom(6);
        lbl.set_halign(Align::Center);
        self.dropdown_list_box.append(&lbl);
    }

    fn notify_state_change(&self, open: bool) {
        for cb in self.on_state_listeners.borrow().iter() {
            cb(open);
        }
    }

    fn refresh_brightness(&self) {
        if let Some(pct) = read_backlight_percent() {
            self.brightness_card.set_visible(true);
            self.sliders_divider.set_visible(true);
            self.syncing.set(true);
            self.brightness_scale.set_value(f64::from(pct));
            self.brightness_pct_label.set_text(&format!("{pct}%"));
            self.syncing.set(false);
        } else {
            self.brightness_card.set_visible(false);
            self.sliders_divider.set_visible(false);
        }
    }

    pub fn update_appearance(&self, appearance: &AppearanceInfo) {
        let light = appearance.theme == "neon_light" || appearance.theme == "light";
        self.is_light_theme.set(light);
        if light {
            self.dark_mode_btn.remove_css_class("active");
            self.dark_mode_icon
                .set_icon_name(Some("weather-clear-night-symbolic"));
        } else {
            self.dark_mode_btn.add_css_class("active");
            self.dark_mode_icon
                .set_icon_name(Some("weather-clear-night-symbolic"));
        }
        self.dark_mode_btn.set_tooltip_text(Some("Dark Mode"));
    }

    /// Syncs the Hyprland Auto-Tiling Quick Settings split pill with live compositor state.
    pub fn update_window_behavior(&self, wb: &WindowBehaviorInfo) {
        let layout = if wb.tiling_layout.eq_ignore_ascii_case("master_stack") {
            "master_stack"
        } else {
            "dwindle"
        };
        self.auto_tiling_enabled.set(wb.auto_tiling);
        *self.tiling_layout_name.borrow_mut() = layout.to_string();
        sync_tiling_pill_ui(
            &self.tiling_pill,
            &self.tiling_sub,
            &self.tiling_layout_lbl,
            wb.auto_tiling,
            layout,
        );
    }

    pub fn update_system(&self, snap: &SystemSnapshot) {
        self.syncing.set(true);

        // Header battery
        if snap.power.has_battery {
            let pct = snap.power.battery_percentage.round() as u32;
            let charging = matches!(
                snap.power.battery_state,
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
            self.header_battery_icon.set_icon_name(Some(bat_icon));
            self.header_battery_lbl.set_text(&format!("{pct}%"));
            self.header_battery_box.set_visible(true);
            self.header_title_lbl.set_visible(false);
        } else {
            self.header_battery_box.set_visible(false);
            self.header_title_lbl.set_visible(true);
        }

        // Wi-Fi pill
        self.wifi_enabled_state.set(snap.network.wifi_enabled);
        self.wifi_main_btn.set_sensitive(snap.network.available);
        if snap.network.wifi_enabled {
            self.wifi_pill.add_css_class("active");
            let sub = snap.network.primary_name.as_deref().unwrap_or("On");
            self.wifi_sub.set_text(sub);
        } else {
            self.wifi_pill.remove_css_class("active");
            self.wifi_sub.set_text("Off");
        }

        // Bluetooth pill
        self.bt_powered_state.set(snap.bluetooth.powered);
        self.bt_main_btn.set_sensitive(snap.bluetooth.available);
        if snap.bluetooth.powered {
            self.bt_pill.add_css_class("active");
            let connected: Vec<&str> = snap
                .bluetooth
                .devices
                .iter()
                .filter(|d| d.connected)
                .map(|d| d.name.as_str())
                .collect();
            let sub = match connected.len() {
                0 => "On".to_string(),
                1 => connected[0].to_string(),
                n => format!("{n} Connected"),
            };
            self.bt_sub.set_text(&sub);
        } else {
            self.bt_pill.remove_css_class("active");
            self.bt_sub.set_text("Off");
        }

        // Audio
        if snap.audio.available {
            let sink = snap
                .audio
                .default_sink_name
                .as_deref()
                .unwrap_or("Sound Output");
            self.audio_card.set_tooltip_text(Some(sink));
            let pct = snap.audio.volume_percent();
            self.volume_scale.set_value(f64::from(pct));
            self.vol_pct_label.set_text(&format!("{pct}%"));
            let icon = if snap.audio.muted || pct == 0 {
                "audio-volume-muted-symbolic"
            } else {
                "audio-volume-low-symbolic"
            };
            self.mute_icon.set_icon_name(Some(icon));
            if snap.audio.muted {
                self.mute_btn.add_css_class("muted");
            } else {
                self.mute_btn.remove_css_class("muted");
            }
        } else {
            self.vol_pct_label.set_text("—");
        }

        // 3-Button Power Mode Segmented Control
        let active_prof = snap
            .power
            .active_profile
            .as_deref()
            .unwrap_or("balanced");
        set_active_power_seg(
            &self.pm_saver_btn,
            &self.pm_balanced_btn,
            &self.pm_perf_btn,
            active_prof,
        );

        self.syncing.set(false);
    }
}

/// Builds an equal-width split connectivity pill (left toggle + divider + right dropdown arrow).
fn build_split_connectivity_pill(
    icon_name: &str,
    title: &str,
    subtitle: &str,
) -> (GtkBox, Button, Label, Button, Image) {
    let pill = GtkBox::new(Orientation::Horizontal, 0);
    pill.add_css_class("neon-qs-split-pill");
    pill.set_hexpand(true);

    let main_btn = Button::new();
    main_btn.add_css_class("neon-qs-split-main");
    main_btn.set_hexpand(true);

    let main_row = GtkBox::new(Orientation::Horizontal, 8);
    main_row.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(18);
    icon.add_css_class("neon-qs-split-icon");

    let text = GtkBox::new(Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(Align::Center);

    let title_lbl = Label::new(Some(title));
    title_lbl.add_css_class("neon-qs-pill-title");
    title_lbl.set_halign(Align::Start);

    let sub_lbl = Label::new(Some(subtitle));
    sub_lbl.add_css_class("neon-qs-pill-sub");
    sub_lbl.set_halign(Align::Start);
    sub_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    sub_lbl.set_max_width_chars(10);

    text.append(&title_lbl);
    text.append(&sub_lbl);

    main_row.append(&icon);
    main_row.append(&text);
    main_btn.set_child(Some(&main_row));

    let sep = Separator::new(Orientation::Vertical);
    sep.add_css_class("neon-qs-split-sep");

    let arrow_btn = Button::new();
    arrow_btn.add_css_class("neon-qs-split-arrow");
    arrow_btn.set_tooltip_text(Some(&format!("Show {title} menu")));
    let arrow_icon = Image::from_icon_name("pan-down-symbolic");
    arrow_icon.add_css_class("neon-qs-chevron");
    arrow_icon.set_pixel_size(14);
    arrow_btn.set_child(Some(&arrow_icon));

    pill.append(&main_btn);
    pill.append(&sep);
    pill.append(&arrow_btn);

    (pill, main_btn, sub_lbl, arrow_btn, arrow_icon)
}

/// Builds the full-width Hyprland Auto-Tiling split pill (left global toggle + right layout switcher).
fn build_split_tiling_pill() -> (GtkBox, Button, Label, Button, Label) {
    let pill = GtkBox::new(Orientation::Horizontal, 0);
    pill.add_css_class("neon-qs-split-pill");
    pill.add_css_class("neon-qs-tiling-pill");
    pill.set_hexpand(true);

    let main_btn = Button::new();
    main_btn.add_css_class("neon-qs-split-main");
    main_btn.set_hexpand(true);
    main_btn.set_tooltip_text(Some("Toggle Hyprland Auto-Tiling globally (Super+T)"));

    let main_row = GtkBox::new(Orientation::Horizontal, 10);
    main_row.set_valign(Align::Center);

    let icon = Image::from_icon_name("view-grid-symbolic");
    icon.set_pixel_size(18);
    icon.add_css_class("neon-qs-split-icon");

    let text = GtkBox::new(Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(Align::Center);

    let title_lbl = Label::new(Some("Auto-Tiling"));
    title_lbl.add_css_class("neon-qs-pill-title");
    title_lbl.set_halign(Align::Start);

    let sub_lbl = Label::new(Some("Off · Drag Snap Active"));
    sub_lbl.add_css_class("neon-qs-pill-sub");
    sub_lbl.set_halign(Align::Start);
    sub_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

    text.append(&title_lbl);
    text.append(&sub_lbl);

    main_row.append(&icon);
    main_row.append(&text);
    main_btn.set_child(Some(&main_row));

    let sep = Separator::new(Orientation::Vertical);
    sep.add_css_class("neon-qs-split-sep");

    let layout_btn = Button::new();
    layout_btn.add_css_class("neon-qs-split-layout-btn");
    layout_btn.set_tooltip_text(Some("Switch tiling layout (Dwindle ↔ Master–Stack)"));
    let layout_row = GtkBox::new(Orientation::Horizontal, 5);
    layout_row.set_valign(Align::Center);
    layout_row.set_halign(Align::Center);
    let layout_lbl = Label::new(Some("Dwindle"));
    layout_lbl.add_css_class("neon-qs-tiling-layout-lbl");
    let cycle_icon = Image::from_icon_name("object-flip-horizontal-symbolic");
    cycle_icon.set_pixel_size(13);
    cycle_icon.add_css_class("neon-qs-split-icon");
    layout_row.append(&layout_lbl);
    layout_row.append(&cycle_icon);
    layout_btn.set_child(Some(&layout_row));

    pill.append(&main_btn);
    pill.append(&sep);
    pill.append(&layout_btn);

    (pill, main_btn, sub_lbl, layout_btn, layout_lbl)
}

fn sync_tiling_pill_ui(
    pill: &GtkBox,
    sub_lbl: &Label,
    layout_lbl: &Label,
    enabled: bool,
    layout: &str,
) {
    let is_master = layout.eq_ignore_ascii_case("master_stack");
    let short_name = if is_master { "Master" } else { "Dwindle" };
    let full_name = if is_master { "Master–Stack" } else { "Dwindle" };
    layout_lbl.set_text(short_name);
    if enabled {
        pill.add_css_class("active");
        sub_lbl.set_text(&format!("On · {full_name} · Super+T"));
    } else {
        pill.remove_css_class("active");
        sub_lbl.set_text("Off · Drag Snap Active");
    }
}

fn build_action_pill(icon_name: &str, label: &str) -> (Button, Image) {
    let btn = Button::new();
    btn.add_css_class("neon-qs-action-pill");
    btn.set_hexpand(true);

    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.set_halign(Align::Center);
    row.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    let lbl = Label::new(Some(label));
    lbl.add_css_class("neon-qs-pill-title");

    row.append(&icon);
    row.append(&lbl);
    btn.set_child(Some(&row));

    (btn, icon)
}

fn build_power_seg_btn(label: &str) -> Button {
    let btn = Button::with_label(label);
    btn.add_css_class("neon-qs-seg-btn");
    btn.set_hexpand(true);
    btn
}

fn set_active_power_seg(saver: &Button, balanced: &Button, perf: &Button, active: &str) {
    for (btn, key) in [
        (saver, "power-saver"),
        (balanced, "balanced"),
        (perf, "performance"),
    ] {
        if active == key {
            btn.add_css_class("active");
        } else {
            btn.remove_css_class("active");
        }
    }
}

fn centered_slot_icon(icon_name: &str, pixel_size: i32, slot_px: i32, muted: bool) -> GtkBox {
    let slot = GtkBox::new(Orientation::Horizontal, 0);
    slot.set_size_request(slot_px, slot_px);
    slot.set_hexpand(false);
    slot.set_vexpand(false);
    slot.set_halign(Align::Center);
    slot.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(pixel_size);
    icon.set_size_request(slot_px, slot_px);
    icon.set_hexpand(false);
    icon.set_vexpand(false);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    icon.add_css_class(if muted {
        "neon-cc-slider-icon-muted"
    } else {
        "neon-cc-slider-icon"
    });
    slot.append(&icon);
    slot
}

fn wifi_signal_icon(strength: u8) -> &'static str {
    match strength {
        75..=100 => "network-wireless-signal-excellent-symbolic",
        50..=74 => "network-wireless-signal-good-symbolic",
        25..=49 => "network-wireless-signal-ok-symbolic",
        _ => "network-wireless-signal-weak-symbolic",
    }
}

fn bt_device_icon(icon: Option<&str>) -> &'static str {
    match icon.unwrap_or("") {
        "audio-headphones" | "audio-headset" => "audio-headphones-symbolic",
        "audio-card" | "audio-speakers" => "audio-speakers-symbolic",
        "input-mouse" => "input-mouse-symbolic",
        "input-keyboard" => "input-keyboard-symbolic",
        "input-gaming" => "input-gaming-symbolic",
        "phone" => "phone-symbolic",
        _ => "bluetooth-active-symbolic",
    }
}
