//! NEON DE macOS Tahoe ("Liquid Glass") Control Center
//! (`zwlr_layer_shell_v1` `Layer::Overlay` anchored Top+Right)
//!
//! Features live GPU-blurred translucent glass modules, macOS Tahoe connectivity cluster,
//! Focus + Appearance/Overview tiles, Display Brightness & Sound capsule sliders, Power
//! Profile switcher, dynamic Notifications list, and smooth spring open/close animations.

use std::{
    cell::{Cell, RefCell},
    fs,
    path::PathBuf,
    process::Command,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, EventControllerKey,
    Image, Label, Orientation, Overflow, Overlay, Picture, PolicyType, Scale, ScrolledWindow,
    Separator, gdk, gio, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{AppearanceInfo, IpcRequest};
use neon_system::{
    SystemAction, SystemActionSender, SystemSnapshot, notifications::NotificationStore,
};

use crate::{
    ipc_bridge::IpcCommandSender, notifications::NotificationOverlay, panel::launch_sibling_binary,
    wallpaper::resolve_wallpaper_path,
};

type StateChangeCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

/// Live macOS Tahoe Control Center popover anchored under the top-right status cluster.
#[derive(Clone)]
pub struct QuickSettingsPanel {
    window: ApplicationWindow,
    shell_overlay: Overlay,
    blur_picture: Picture,
    current_wp: Rc<RefCell<Option<PathBuf>>>,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    on_state_change: StateChangeCallback,
    syncing: Rc<Cell<bool>>,
    // Connectivity cluster
    wifi_btn: Button,
    wifi_sub: Label,
    wifi_enabled_state: Rc<Cell<bool>>,
    bt_btn: Button,
    bt_sub: Label,
    bt_powered_state: Rc<Cell<bool>>,
    bt_devices_box: GtkBox,
    // Focus & quick tiles
    dnd_btn: Button,
    dnd_sub: Label,
    theme_btn: Button,
    theme_sub: Label,
    theme_icon: Image,
    is_light_theme: Rc<Cell<bool>>,
    // Display Brightness
    brightness_card: GtkBox,
    brightness_pct_label: Label,
    brightness_scale: Scale,
    // Capsule Audio
    sink_label: Label,
    vol_pct_label: Label,
    volume_scale: Scale,
    mute_btn: Button,
    mute_icon: Image,
    // Power
    battery_label: Label,
    profile_saver_btn: Button,
    profile_balanced_btn: Button,
    profile_perf_btn: Button,
    // Notifications (auto-hidden when empty)
    notif_card: GtkBox,
    notif_list_box: GtkBox,
    store: Arc<NotificationStore>,
    sys_tx: SystemActionSender,
    toast_overlay: NotificationOverlay,
}

impl QuickSettingsPanel {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        sys_tx: SystemActionSender,
        store: Arc<NotificationStore>,
        toast_overlay: NotificationOverlay,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Control Center")
            .decorated(false)
            .resizable(false)
            .default_width(372)
            .build();
        window.add_css_class("neon-quicksettings-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-quick-settings"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, 30);
            window.set_margin(Edge::Right, 4);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let syncing = Rc::new(Cell::new(false));
        let wifi_enabled_state = Rc::new(Cell::new(false));
        let bt_powered_state = Rc::new(Cell::new(false));
        let is_light_theme = Rc::new(Cell::new(false));
        let is_open = Rc::new(Cell::new(false));
        let anim_gen = Rc::new(Cell::new(0u64));
        let on_state_change: StateChangeCallback = Rc::new(RefCell::new(None));
        let current_wp: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));

        // Outer padding wrapper so shadows & open/close spring animation never clip
        let wrapper = GtkBox::new(Orientation::Vertical, 0);
        wrapper.add_css_class("neon-tahoe-wrapper");

        // Liquid Glass overlay shell (clipped to rounded 26px corners)
        let shell_overlay = Overlay::new();
        shell_overlay.add_css_class("neon-tahoe-shell");
        shell_overlay.add_css_class("cc-closed");
        shell_overlay.set_overflow(Overflow::Hidden);

        // Background GPU-blurred wallpaper bleed layer
        let blur_picture = Picture::new();
        blur_picture.add_css_class("neon-tahoe-blur-bg");
        blur_picture.set_content_fit(ContentFit::Cover);
        blur_picture.set_can_shrink(true);
        blur_picture.set_size_request(1, 1);
        blur_picture.set_hexpand(true);
        blur_picture.set_vexpand(true);
        if let Some(wp) = resolve_wallpaper_path(None, false) {
            blur_picture.set_file(Some(&gio::File::for_path(&wp)));
            *current_wp.borrow_mut() = Some(wp);
        }

        // Foreground Liquid Glass content stack
        let root = GtkBox::new(Orientation::Vertical, 10);
        root.add_css_class("neon-tahoe-glass");

        // === ROW 1: macOS Tahoe 2-Column Modular Top Cluster ===
        let top_cluster = GtkBox::new(Orientation::Horizontal, 10);
        top_cluster.set_homogeneous(true);

        // Left Module: Connectivity Card (Wi-Fi + Bluetooth stacked in one Liquid Glass pod)
        let conn_card = GtkBox::new(Orientation::Vertical, 2);
        conn_card.add_css_class("neon-tahoe-card");
        conn_card.set_valign(Align::Fill);

        let (wifi_btn, wifi_sub, _) =
            build_conn_row("network-wireless-symbolic", "Wi-Fi", "Checking...");
        {
            let sys_tx = sys_tx.clone();
            let wifi_state = Rc::clone(&wifi_enabled_state);
            wifi_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::SetWifiEnabled(!wifi_state.get()));
            });
        }

        let conn_sep = Separator::new(Orientation::Horizontal);
        conn_sep.add_css_class("neon-tahoe-sep");

        let (bt_btn, bt_sub, _) =
            build_conn_row("bluetooth-active-symbolic", "Bluetooth", "Checking...");
        {
            let sys_tx = sys_tx.clone();
            let bt_state = Rc::clone(&bt_powered_state);
            bt_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::SetBluetoothPowered(!bt_state.get()));
            });
        }

        conn_card.append(&wifi_btn);
        conn_card.append(&conn_sep);
        conn_card.append(&bt_btn);

        // Right Column: Focus pill on top + 2 compact square Liquid Glass tiles below
        let right_col = GtkBox::new(Orientation::Vertical, 10);

        let (dnd_btn, dnd_sub, _) =
            build_focus_pill("weather-clear-night-symbolic", "Focus", "Off");
        {
            let store = Arc::clone(&store);
            let btn_c = dnd_btn.clone();
            let sub_c = dnd_sub.clone();
            dnd_btn.connect_clicked(move |_| {
                let next = !store.is_do_not_disturb();
                store.set_do_not_disturb(next);
                if next {
                    btn_c.add_css_class("active");
                    sub_c.set_text("Do Not Disturb");
                } else {
                    btn_c.remove_css_class("active");
                    sub_c.set_text("Off");
                }
            });
        }

        let mini_row = GtkBox::new(Orientation::Horizontal, 10);
        mini_row.set_homogeneous(true);
        mini_row.set_vexpand(true);

        let (theme_btn, theme_sub, theme_icon) =
            build_mini_tile("weather-clear-night-symbolic", "Dark");
        theme_btn.add_css_class("active");
        {
            let ipc = ipc.clone();
            let is_light = Rc::clone(&is_light_theme);
            theme_btn.connect_clicked(move |_| {
                let next_theme = if is_light.get() {
                    "neon_dark"
                } else {
                    "neon_light"
                };
                ipc.send(IpcRequest::UpdateAppearance {
                    theme: Some(next_theme.to_string()),
                    accent_color: None,
                    dock_transparency: None,
                    panel_transparency: None,
                    panel_visible: None,
                    show_clock: None,
                    show_workspaces: None,
                    high_contrast: None,
                    wallpaper: None,
                });
            });
        }

        let (overview_btn, _, _) = build_mini_tile("view-app-grid-symbolic", "Apps");
        mini_row.append(&theme_btn);
        mini_row.append(&overview_btn);

        right_col.append(&dnd_btn);
        right_col.append(&mini_row);

        top_cluster.append(&conn_card);
        top_cluster.append(&right_col);
        root.append(&top_cluster);

        // Paired Bluetooth devices mini-list (only shown when devices exist)
        let bt_devices_box = GtkBox::new(Orientation::Vertical, 6);
        bt_devices_box.set_visible(false);
        root.append(&bt_devices_box);

        // === ROW 2: Display Brightness Liquid Glass Capsule Slider ===
        let brightness_card = GtkBox::new(Orientation::Vertical, 8);
        brightness_card.add_css_class("neon-tahoe-card");

        let bright_head = GtkBox::new(Orientation::Horizontal, 8);
        let bright_title = Label::new(Some("Display"));
        bright_title.add_css_class("neon-qs-title");
        bright_title.set_hexpand(true);
        bright_title.set_halign(Align::Start);

        let initial_brightness = read_backlight_percent();
        let brightness_pct_label =
            Label::new(Some(&format!("{}%", initial_brightness.unwrap_or(100))));
        brightness_pct_label.add_css_class("neon-qs-muted");
        bright_head.append(&bright_title);
        bright_head.append(&brightness_pct_label);

        let bright_slider_row = GtkBox::new(Orientation::Horizontal, 8);
        let sun_icon = Image::from_icon_name("display-brightness-symbolic");
        sun_icon.set_pixel_size(16);
        sun_icon.add_css_class("neon-tahoe-slider-icon");

        let brightness_scale = Scale::with_range(Orientation::Horizontal, 5.0, 100.0, 1.0);
        brightness_scale.add_css_class("neon-tahoe-capsule");
        brightness_scale.set_draw_value(false);
        brightness_scale.set_hexpand(true);
        brightness_scale.set_value(f64::from(initial_brightness.unwrap_or(100)));
        {
            let syncing = Rc::clone(&syncing);
            let pct_lbl = brightness_pct_label.clone();
            brightness_scale.connect_value_changed(move |s| {
                let pct = s.value().round() as u32;
                pct_lbl.set_text(&format!("{pct}%"));
                if syncing.get() {
                    return;
                }
                set_backlight_percent(pct);
            });
        }

        bright_slider_row.append(&sun_icon);
        bright_slider_row.append(&brightness_scale);
        brightness_card.append(&bright_head);
        brightness_card.append(&bright_slider_row);
        brightness_card.set_visible(initial_brightness.is_some());
        root.append(&brightness_card);

        // === ROW 3: Sound Liquid Glass Capsule Slider (PipeWire) ===
        let audio_card = GtkBox::new(Orientation::Vertical, 8);
        audio_card.add_css_class("neon-tahoe-card");

        let audio_head = GtkBox::new(Orientation::Horizontal, 8);
        let sink_label = Label::new(Some("Sound"));
        sink_label.add_css_class("neon-qs-title");
        sink_label.set_hexpand(true);
        sink_label.set_halign(Align::Start);
        sink_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let vol_pct_label = Label::new(Some("70%"));
        vol_pct_label.add_css_class("neon-qs-muted");
        audio_head.append(&sink_label);
        audio_head.append(&vol_pct_label);

        let slider_row = GtkBox::new(Orientation::Horizontal, 8);
        let mute_icon = Image::from_icon_name("audio-volume-high-symbolic");
        mute_icon.set_pixel_size(16);
        let mute_btn = Button::new();
        mute_btn.add_css_class("neon-tray-btn");
        mute_btn.set_child(Some(&mute_icon));
        mute_btn.set_tooltip_text(Some("Mute / Unmute"));
        {
            let sys_tx = sys_tx.clone();
            mute_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::ToggleSinkMute);
            });
        }

        let volume_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 2.0);
        volume_scale.add_css_class("neon-tahoe-capsule");
        volume_scale.set_draw_value(false);
        volume_scale.set_hexpand(true);
        volume_scale.set_value(70.0);
        {
            let sys_tx = sys_tx.clone();
            let syncing = Rc::clone(&syncing);
            let pct_lbl = vol_pct_label.clone();
            volume_scale.connect_value_changed(move |s| {
                pct_lbl.set_text(&format!("{:.0}%", s.value()));
                if syncing.get() {
                    return;
                }
                let vol = (s.value() as f32) / 100.0;
                sys_tx.send(SystemAction::SetSinkVolume(vol));
            });
        }
        slider_row.append(&mute_btn);
        slider_row.append(&volume_scale);

        audio_card.append(&audio_head);
        audio_card.append(&slider_row);
        root.append(&audio_card);

        // === ROW 4: Battery & Power Profiles ===
        let power_card = GtkBox::new(Orientation::Vertical, 8);
        power_card.add_css_class("neon-tahoe-card");

        let battery_label = Label::new(Some("Battery"));
        battery_label.add_css_class("neon-qs-title");
        battery_label.set_halign(Align::Start);

        let seg_track = GtkBox::new(Orientation::Horizontal, 4);
        seg_track.add_css_class("neon-segment-track");
        seg_track.set_homogeneous(true);

        let profile_saver_btn = Button::with_label("Saver");
        let profile_balanced_btn = Button::with_label("Balanced");
        let profile_perf_btn = Button::with_label("Performance");
        for (btn, profile) in [
            (&profile_saver_btn, "power-saver"),
            (&profile_balanced_btn, "balanced"),
            (&profile_perf_btn, "performance"),
        ] {
            btn.add_css_class("neon-segment-btn");
            let sys_tx = sys_tx.clone();
            let prof = profile.to_string();
            btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::SetPowerProfile(prof.clone()));
            });
            seg_track.append(btn);
        }
        power_card.append(&battery_label);
        power_card.append(&seg_track);
        root.append(&power_card);

        // === ROW 5: Notifications (Auto-hidden when empty; zero Test buttons) ===
        let notif_card = GtkBox::new(Orientation::Vertical, 6);
        notif_card.add_css_class("neon-tahoe-card");
        notif_card.set_visible(false);

        let notif_head = GtkBox::new(Orientation::Horizontal, 6);
        let notif_title = Label::new(Some("Notifications"));
        notif_title.add_css_class("neon-qs-title");
        notif_title.set_hexpand(true);
        notif_title.set_halign(Align::Start);

        let clear_btn = Button::with_label("Clear");
        clear_btn.add_css_class("neon-tray-btn");
        {
            let store = Arc::clone(&store);
            clear_btn.connect_clicked(move |_| {
                store.clear_all();
            });
        }

        notif_head.append(&notif_title);
        notif_head.append(&clear_btn);
        notif_card.append(&notif_head);

        let notif_scroller = ScrolledWindow::builder()
            .max_content_height(128)
            .propagate_natural_height(true)
            .build();
        let notif_list_box = GtkBox::new(Orientation::Vertical, 6);
        notif_scroller.set_child(Some(&notif_list_box));
        notif_card.append(&notif_scroller);
        root.append(&notif_card);

        // === ROW 6: Bottom Liquid Glass Action Bar ===
        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.set_homogeneous(true);

        let shot_btn = build_footer_pill("camera-photo-symbolic", "Capture");
        let settings_btn = build_footer_pill("preferences-system-symbolic", "Settings");
        let lock_btn = build_footer_pill("system-lock-screen-symbolic", "Lock");

        footer.append(&shot_btn);
        footer.append(&settings_btn);
        footer.append(&lock_btn);
        root.append(&footer);

        // Wrap blur_picture in an External ScrolledWindow (propagate_natural=false) so a 4K
        // wallpaper never inflates the Control Center's natural window dimensions.
        let blur_holder = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::External)
            .vscrollbar_policy(PolicyType::External)
            .propagate_natural_width(false)
            .propagate_natural_height(false)
            .can_target(false)
            .focusable(false)
            .child(&blur_picture)
            .build();

        shell_overlay.set_child(Some(&blur_holder));
        shell_overlay.add_overlay(&root);
        shell_overlay.set_measure_overlay(&root, true);
        shell_overlay.set_clip_overlay(&root, false);

        wrapper.append(&shell_overlay);
        window.set_child(Some(&wrapper));
        window.set_visible(false);

        let panel = Self {
            window,
            shell_overlay,
            blur_picture,
            current_wp,
            is_open,
            anim_gen,
            on_state_change,
            syncing,
            wifi_btn,
            wifi_sub,
            wifi_enabled_state,
            bt_btn,
            bt_sub,
            bt_powered_state,
            bt_devices_box,
            dnd_btn,
            dnd_sub,
            theme_btn,
            theme_sub,
            theme_icon,
            is_light_theme,
            brightness_card,
            brightness_pct_label,
            brightness_scale,
            sink_label,
            vol_pct_label,
            volume_scale,
            mute_btn,
            mute_icon,
            battery_label,
            profile_saver_btn,
            profile_balanced_btn,
            profile_perf_btn,
            notif_card,
            notif_list_box,
            store,
            sys_tx,
            toast_overlay,
        };

        // Wire Overview mini tile
        {
            let p = panel.clone();
            let ipc = ipc.clone();
            overview_btn.connect_clicked(move |_| {
                p.close();
                ipc.send(IpcRequest::ToggleOverview);
            });
        }

        // Wire Footer pills with smooth close animation
        {
            let p = panel.clone();
            let ipc = ipc.clone();
            shot_btn.connect_clicked(move |_| {
                p.close();
                let ipc_c = ipc.clone();
                glib::timeout_add_local_once(Duration::from_millis(220), move || {
                    ipc_c.send(IpcRequest::OpenScreenshotUi);
                });
            });
        }
        {
            let p = panel.clone();
            settings_btn.connect_clicked(move |_| {
                p.close();
                launch_sibling_binary("neon-settings");
            });
        }
        {
            let p = panel.clone();
            let ipc = ipc.clone();
            lock_btn.connect_clicked(move |_| {
                p.close();
                ipc.send(IpcRequest::LockSession);
            });
        }

        // Escape key smoothly closes Control Center
        {
            let key_ctrl = EventControllerKey::new();
            let p = panel.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape && p.is_open.get() {
                    p.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            panel.window.add_controller(key_ctrl);
        }

        panel.refresh_notifications();
        panel
    }

    /// Registers a listener notified whenever Control Center opens (`true`) or closes (`false`).
    pub fn set_on_state_change<F: Fn(bool) + 'static>(&self, cb: F) {
        *self.on_state_change.borrow_mut() = Some(Box::new(cb));
    }

    pub fn toggle(&self) {
        if self.is_open.get() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(&self) {
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(true);
        self.notify_state_change(true);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        }

        self.toast_overlay.set_suppressed(true);
        self.sys_tx.send(SystemAction::RefreshNow);
        self.refresh_notifications();
        self.refresh_brightness();

        // Start from closed transform, reveal window, then animate into open state
        self.shell_overlay.remove_css_class("cc-open");
        self.shell_overlay.add_css_class("cc-closed");
        self.window.set_visible(true);
        self.window.present();

        let overlay = self.shell_overlay.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(16), move || {
            if gen_cell.get() == epoch {
                overlay.remove_css_class("cc-closed");
                overlay.add_css_class("cc-open");
            }
        });
    }

    pub fn close(&self) {
        if !self.is_open.get() && !self.window.is_visible() {
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        self.notify_state_change(false);

        self.shell_overlay.remove_css_class("cc-open");
        self.shell_overlay.add_css_class("cc-closed");

        let win = self.window.clone();
        let toast = self.toast_overlay.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(210), move || {
            if gen_cell.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    win.set_keyboard_mode(KeyboardMode::None);
                }
                win.set_visible(false);
                toast.set_suppressed(false);
            }
        });
    }

    fn notify_state_change(&self, open: bool) {
        if let Some(cb) = self.on_state_change.borrow().as_ref() {
            cb(open);
        }
    }

    fn refresh_brightness(&self) {
        if let Some(pct) = read_backlight_percent() {
            self.brightness_card.set_visible(true);
            self.syncing.set(true);
            self.brightness_scale.set_value(f64::from(pct));
            self.brightness_pct_label.set_text(&format!("{pct}%"));
            self.syncing.set(false);
        } else {
            self.brightness_card.set_visible(false);
        }
    }

    pub fn update_appearance(&self, appearance: &AppearanceInfo) {
        let light = appearance.theme == "neon_light" || appearance.theme == "light";
        self.is_light_theme.set(light);
        if light {
            self.theme_sub.set_text("Light");
            self.theme_icon
                .set_icon_name(Some("weather-clear-symbolic"));
        } else {
            self.theme_sub.set_text("Dark");
            self.theme_icon
                .set_icon_name(Some("weather-clear-night-symbolic"));
        }
        self.theme_btn.add_css_class("active");

        if let Some(wp) = resolve_wallpaper_path(appearance.wallpaper.as_deref(), light)
            && self.current_wp.borrow().as_ref() != Some(&wp)
        {
            self.blur_picture.set_file(Some(&gio::File::for_path(&wp)));
            *self.current_wp.borrow_mut() = Some(wp);
        }
    }

    pub fn update_system(&self, snap: &SystemSnapshot) {
        self.syncing.set(true);

        // Wi-Fi
        self.wifi_enabled_state.set(snap.network.wifi_enabled);
        self.wifi_btn.set_sensitive(snap.network.available);
        if snap.network.wifi_enabled {
            self.wifi_btn.add_css_class("active");
            let sub = snap.network.primary_name.as_deref().unwrap_or("On");
            self.wifi_sub.set_text(sub);
        } else {
            self.wifi_btn.remove_css_class("active");
            self.wifi_sub.set_text("Off");
        }

        // Bluetooth
        self.bt_powered_state.set(snap.bluetooth.powered);
        self.bt_btn.set_sensitive(snap.bluetooth.available);
        if snap.bluetooth.powered {
            self.bt_btn.add_css_class("active");
            let n = snap.bluetooth.connected_count();
            if n > 0 {
                self.bt_sub.set_text(&format!("{n} Connected"));
            } else {
                self.bt_sub.set_text("On");
            }
        } else {
            self.bt_btn.remove_css_class("active");
            self.bt_sub.set_text("Off");
        }

        while let Some(child) = self.bt_devices_box.first_child() {
            self.bt_devices_box.remove(&child);
        }
        if snap.bluetooth.powered && !snap.bluetooth.devices.is_empty() {
            self.bt_devices_box.set_visible(true);
            for dev in snap.bluetooth.devices.iter().take(3) {
                let row = GtkBox::new(Orientation::Horizontal, 8);
                row.add_css_class("neon-tahoe-card");
                let icon = Image::from_icon_name("audio-headphones-symbolic");
                icon.set_pixel_size(15);
                let name = Label::new(Some(&dev.name));
                name.add_css_class("neon-qs-title");
                name.set_hexpand(true);
                name.set_halign(Align::Start);
                name.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                let btn = Button::with_label(if dev.connected {
                    "Disconnect"
                } else {
                    "Connect"
                });
                btn.add_css_class("neon-tray-btn");
                let path = dev.object_path.clone();
                let target_connect = !dev.connected;
                let sys_tx = self.sys_tx.clone();
                btn.connect_clicked(move |_| {
                    sys_tx.send(SystemAction::SetBluetoothDeviceConnected {
                        object_path: path.clone(),
                        connect: target_connect,
                    });
                });
                row.append(&icon);
                row.append(&name);
                row.append(&btn);
                self.bt_devices_box.append(&row);
            }
        } else {
            self.bt_devices_box.set_visible(false);
        }

        // Focus / DND
        if self.store.is_do_not_disturb() {
            self.dnd_btn.add_css_class("active");
            self.dnd_sub.set_text("Do Not Disturb");
        } else {
            self.dnd_btn.remove_css_class("active");
            self.dnd_sub.set_text("Off");
        }

        // Audio
        if snap.audio.available {
            let sink = snap
                .audio
                .default_sink_name
                .as_deref()
                .unwrap_or("Sound Output");
            self.sink_label.set_text(sink);
            let pct = snap.audio.volume_percent();
            self.volume_scale.set_value(f64::from(pct));
            self.vol_pct_label.set_text(&format!("{pct}%"));
            let icon = if snap.audio.muted || pct == 0 {
                "audio-volume-muted-symbolic"
            } else if pct < 40 {
                "audio-volume-low-symbolic"
            } else if pct < 75 {
                "audio-volume-medium-symbolic"
            } else {
                "audio-volume-high-symbolic"
            };
            self.mute_icon.set_icon_name(Some(icon));
            if snap.audio.muted {
                self.mute_btn.add_css_class("active");
            } else {
                self.mute_btn.remove_css_class("active");
            }
        } else {
            self.sink_label.set_text("Audio Unavailable");
        }

        // Power
        if snap.power.has_battery {
            self.battery_label.set_text(&format!(
                "Battery · {:.0}% ({})",
                snap.power.battery_percentage,
                snap.power.battery_state.label()
            ));
        } else {
            self.battery_label.set_text("Power · AC Adapter");
        }
        let active_prof = snap.power.active_profile.as_deref().unwrap_or("");
        for (btn, prof) in [
            (&self.profile_saver_btn, "power-saver"),
            (&self.profile_balanced_btn, "balanced"),
            (&self.profile_perf_btn, "performance"),
        ] {
            if active_prof == prof {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }

        self.syncing.set(false);
    }

    pub fn refresh_notifications(&self) {
        while let Some(child) = self.notif_list_box.first_child() {
            self.notif_list_box.remove(&child);
        }

        let history = self.store.history();
        if history.is_empty() {
            self.notif_card.set_visible(false);
            return;
        }

        self.notif_card.set_visible(true);
        for item in history.iter().take(5) {
            let row = GtkBox::new(Orientation::Vertical, 2);
            row.add_css_class("neon-tahoe-notif-item");
            let title = Label::new(Some(&format!("{} · {}", item.app_name, item.summary)));
            title.add_css_class("neon-qs-title");
            title.set_halign(Align::Start);
            title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row.append(&title);

            if !item.body.is_empty() {
                let body = Label::new(Some(&item.body));
                body.add_css_class("neon-qs-muted");
                body.set_halign(Align::Start);
                body.set_wrap(true);
                body.set_max_width_chars(38);
                row.append(&body);
            }
            self.notif_list_box.append(&row);
        }
    }
}

fn build_conn_row(icon_name: &str, title: &str, subtitle: &str) -> (Button, Label, Image) {
    let btn = Button::new();
    btn.add_css_class("neon-tahoe-conn-row");

    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_valign(Align::Center);

    let badge = GtkBox::new(Orientation::Horizontal, 0);
    badge.add_css_class("neon-cc-badge");
    badge.set_halign(Align::Center);
    badge.set_valign(Align::Center);
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    icon.set_hexpand(true);
    badge.append(&icon);

    let text = GtkBox::new(Orientation::Vertical, 1);
    text.set_valign(Align::Center);
    let title_lbl = Label::new(Some(title));
    title_lbl.add_css_class("neon-qs-title");
    title_lbl.set_halign(Align::Start);
    let sub_lbl = Label::new(Some(subtitle));
    sub_lbl.add_css_class("neon-qs-muted");
    sub_lbl.set_halign(Align::Start);
    sub_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    sub_lbl.set_max_width_chars(13);
    text.append(&title_lbl);
    text.append(&sub_lbl);

    row.append(&badge);
    row.append(&text);
    btn.set_child(Some(&row));

    (btn, sub_lbl, icon)
}

fn build_focus_pill(icon_name: &str, title: &str, subtitle: &str) -> (Button, Label, Image) {
    let (btn, sub_lbl, icon) = build_conn_row(icon_name, title, subtitle);
    btn.remove_css_class("neon-tahoe-conn-row");
    btn.add_css_class("neon-tahoe-focus-pill");
    (btn, sub_lbl, icon)
}

fn build_mini_tile(icon_name: &str, label: &str) -> (Button, Label, Image) {
    let btn = Button::new();
    btn.add_css_class("neon-tahoe-mini-tile");

    let col = GtkBox::new(Orientation::Vertical, 4);
    col.set_halign(Align::Center);
    col.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(18);
    let lbl = Label::new(Some(label));
    lbl.add_css_class("neon-qs-muted");
    col.append(&icon);
    col.append(&lbl);
    btn.set_child(Some(&col));
    (btn, lbl, icon)
}

fn build_footer_pill(icon_name: &str, text: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-tahoe-footer-btn");
    let row = GtkBox::new(Orientation::Horizontal, 6);
    row.set_halign(Align::Center);
    row.set_valign(Align::Center);
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(14);
    let lbl = Label::new(Some(text));
    row.append(&icon);
    row.append(&lbl);
    btn.set_child(Some(&row));
    btn
}

fn read_backlight_percent() -> Option<u32> {
    let entries = fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        let cur: f64 = fs::read_to_string(p.join("brightness"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let max: f64 = fs::read_to_string(p.join("max_brightness"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        if max > 0.0 {
            return Some(((cur / max) * 100.0).round().clamp(1.0, 100.0) as u32);
        }
    }
    None
}

fn set_backlight_percent(pct: u32) {
    let clamped = pct.clamp(5, 100);
    let _ = Command::new("brightnessctl")
        .args(["set", &format!("{clamped}%")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
