//! NEON DE macOS Sonoma-Inspired Control Center & Notification Center
//! (`zwlr_layer_shell_v1` `Layer::Overlay` anchored Top+Right).

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, Grid, Image, Label, Orientation,
    Scale, ScrolledWindow, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{AppearanceInfo, IpcRequest};
use neon_system::{
    SystemAction, SystemActionSender, SystemSnapshot, notifications::NotificationStore,
};

use crate::{
    ipc_bridge::IpcCommandSender, notifications::NotificationOverlay, panel::launch_sibling_binary,
};

/// Live Control Center popover anchored under the top-right status cluster.
#[derive(Clone)]
pub struct QuickSettingsPanel {
    window: ApplicationWindow,
    syncing: Rc<Cell<bool>>,
    // 2x2 Connectivity & Mode Tiles
    wifi_tile: Button,
    wifi_sub: Label,
    wifi_enabled_state: Rc<Cell<bool>>,
    bt_tile: Button,
    bt_sub: Label,
    bt_powered_state: Rc<Cell<bool>>,
    bt_devices_box: GtkBox,
    dnd_tile: Button,
    dnd_sub: Label,
    theme_tile: Button,
    theme_sub: Label,
    theme_icon: Image,
    is_light_theme: Rc<Cell<bool>>,
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
    // Notifications
    notif_list_box: GtkBox,
    store: Arc<NotificationStore>,
    sys_tx: SystemActionSender,
    toast_overlay: NotificationOverlay,
    last_appearance: Rc<RefCell<Option<AppearanceInfo>>>,
}

impl QuickSettingsPanel {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        sys_tx: SystemActionSender,
        store: Arc<NotificationStore>,
        notification_bus_name: String,
        toast_overlay: NotificationOverlay,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Control Center")
            .decorated(false)
            .resizable(false)
            .default_width(356)
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
            window.set_keyboard_mode(KeyboardMode::OnDemand);
        }

        let syncing = Rc::new(Cell::new(false));
        let wifi_enabled_state = Rc::new(Cell::new(false));
        let bt_powered_state = Rc::new(Cell::new(false));
        let is_light_theme = Rc::new(Cell::new(false));
        let last_appearance: Rc<RefCell<Option<AppearanceInfo>>> = Rc::new(RefCell::new(None));

        let root = GtkBox::new(Orientation::Vertical, 10);
        root.add_css_class("neon-quicksettings-panel");

        // --- 2x2 macOS Sonoma Interactive Toggle Tiles ---
        let tile_grid = Grid::new();
        tile_grid.set_column_spacing(8);
        tile_grid.set_row_spacing(8);
        tile_grid.set_column_homogeneous(true);

        let (wifi_tile, wifi_sub, _wifi_icon) =
            build_cc_tile("network-wireless-symbolic", "Wi-Fi", "Checking...");
        {
            let sys_tx = sys_tx.clone();
            let wifi_state = Rc::clone(&wifi_enabled_state);
            wifi_tile.connect_clicked(move |_| {
                let next = !wifi_state.get();
                sys_tx.send(SystemAction::SetWifiEnabled(next));
            });
        }

        let (bt_tile, bt_sub, _bt_icon) =
            build_cc_tile("bluetooth-active-symbolic", "Bluetooth", "Checking...");
        {
            let sys_tx = sys_tx.clone();
            let bt_state = Rc::clone(&bt_powered_state);
            bt_tile.connect_clicked(move |_| {
                let next = !bt_state.get();
                sys_tx.send(SystemAction::SetBluetoothPowered(next));
            });
        }

        let (dnd_tile, dnd_sub, _dnd_icon) =
            build_cc_tile("notifications-disabled-symbolic", "Focus", "Off");
        {
            let store = Arc::clone(&store);
            let tile_c = dnd_tile.clone();
            let sub_c = dnd_sub.clone();
            dnd_tile.connect_clicked(move |_| {
                let next = !store.is_do_not_disturb();
                store.set_do_not_disturb(next);
                if next {
                    tile_c.add_css_class("active");
                    sub_c.set_text("Do Not Disturb");
                } else {
                    tile_c.remove_css_class("active");
                    sub_c.set_text("Off");
                }
            });
        }

        let (theme_tile, theme_sub, theme_icon) =
            build_cc_tile("weather-clear-night-symbolic", "Appearance", "Neon Dark");
        theme_tile.add_css_class("active");
        {
            let ipc = ipc.clone();
            let is_light = Rc::clone(&is_light_theme);
            theme_tile.connect_clicked(move |_| {
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

        tile_grid.attach(&wifi_tile, 0, 0, 1, 1);
        tile_grid.attach(&bt_tile, 1, 0, 1, 1);
        tile_grid.attach(&dnd_tile, 0, 1, 1, 1);
        tile_grid.attach(&theme_tile, 1, 1, 1, 1);
        root.append(&tile_grid);

        // Paired Bluetooth devices mini-list (hidden when empty)
        let bt_devices_box = GtkBox::new(Orientation::Vertical, 4);
        bt_devices_box.set_visible(false);
        root.append(&bt_devices_box);

        // --- Capsule Sound Card (PipeWire / WirePlumber) ---
        let audio_card = GtkBox::new(Orientation::Vertical, 8);
        audio_card.add_css_class("neon-qs-card");

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
        volume_scale.add_css_class("neon-capsule-scale");
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

        // --- Battery & Segmented Power Profile Card (UPower) ---
        let power_card = GtkBox::new(Orientation::Vertical, 8);
        power_card.add_css_class("neon-qs-card");

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

        // --- Notification Center Card ---
        let notif_card = GtkBox::new(Orientation::Vertical, 6);
        notif_card.add_css_class("neon-qs-card");

        let notif_head = GtkBox::new(Orientation::Horizontal, 6);
        let notif_title = Label::new(Some("Notifications"));
        notif_title.add_css_class("neon-qs-title");
        notif_title.set_hexpand(true);
        notif_title.set_halign(Align::Start);

        let test_btn = Button::with_label("Test");
        test_btn.add_css_class("neon-tray-btn");
        {
            let sys_tx = sys_tx.clone();
            let bus_name = notification_bus_name;
            test_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::SendTestNotification {
                    bus_name: bus_name.clone(),
                });
            });
        }

        let clear_btn = Button::with_label("Clear");
        clear_btn.add_css_class("neon-tray-btn");
        {
            let store = Arc::clone(&store);
            clear_btn.connect_clicked(move |_| {
                store.clear_all();
            });
        }

        notif_head.append(&notif_title);
        notif_head.append(&test_btn);
        notif_head.append(&clear_btn);
        notif_card.append(&notif_head);

        let notif_scroller = ScrolledWindow::builder()
            .max_content_height(130)
            .propagate_natural_height(true)
            .build();
        let notif_list_box = GtkBox::new(Orientation::Vertical, 6);
        notif_scroller.set_child(Some(&notif_list_box));
        notif_card.append(&notif_scroller);
        root.append(&notif_card);

        // --- Footer Quick Actions (Settings + Lock + Close) ---
        let footer = GtkBox::new(Orientation::Horizontal, 8);
        let settings_btn = Button::with_label("Settings");
        settings_btn.add_css_class("neon-pill-button");
        settings_btn.set_hexpand(true);
        {
            let win = window.clone();
            let overlay = toast_overlay.clone();
            settings_btn.connect_clicked(move |_| {
                win.set_visible(false);
                overlay.set_suppressed(false);
                launch_sibling_binary("neon-settings");
            });
        }

        let lock_btn = Button::with_label("Lock");
        lock_btn.add_css_class("neon-pill-button");
        lock_btn.set_hexpand(true);
        {
            let win = window.clone();
            let overlay = toast_overlay.clone();
            let ipc = ipc.clone();
            lock_btn.connect_clicked(move |_| {
                win.set_visible(false);
                overlay.set_suppressed(false);
                ipc.send(IpcRequest::LockSession);
            });
        }

        let close_btn = Button::with_label("Done");
        close_btn.add_css_class("neon-pill-button");
        {
            let win = window.clone();
            let overlay = toast_overlay.clone();
            close_btn.connect_clicked(move |_| {
                win.set_visible(false);
                overlay.set_suppressed(false);
            });
        }

        footer.append(&settings_btn);
        footer.append(&lock_btn);
        footer.append(&close_btn);
        root.append(&footer);

        window.set_child(Some(&root));
        window.set_visible(false);

        let panel = Self {
            window,
            syncing,
            wifi_tile,
            wifi_sub,
            wifi_enabled_state,
            bt_tile,
            bt_sub,
            bt_powered_state,
            bt_devices_box,
            dnd_tile,
            dnd_sub,
            theme_tile,
            theme_sub,
            theme_icon,
            is_light_theme,
            sink_label,
            vol_pct_label,
            volume_scale,
            mute_btn,
            mute_icon,
            battery_label,
            profile_saver_btn,
            profile_balanced_btn,
            profile_perf_btn,
            notif_list_box,
            store,
            sys_tx,
            toast_overlay,
            last_appearance,
        };
        panel.refresh_notifications();
        panel
    }

    pub fn toggle(&self) {
        let next = !self.window.is_visible();
        self.window.set_visible(next);
        self.toast_overlay.set_suppressed(next);
        if next {
            self.sys_tx.send(SystemAction::RefreshNow);
            self.refresh_notifications();
            self.window.present();
        }
    }

    pub fn update_appearance(&self, appearance: &AppearanceInfo) {
        *self.last_appearance.borrow_mut() = Some(appearance.clone());
        let light = appearance.theme == "neon_light" || appearance.theme == "light";
        self.is_light_theme.set(light);
        if light {
            self.theme_sub.set_text("Neon Light");
            self.theme_icon
                .set_icon_name(Some("weather-clear-symbolic"));
        } else {
            self.theme_sub.set_text("Neon Dark");
            self.theme_icon
                .set_icon_name(Some("weather-clear-night-symbolic"));
        }
        self.theme_tile.add_css_class("active");
    }

    pub fn update_system(&self, snap: &SystemSnapshot) {
        self.syncing.set(true);

        // Wi-Fi tile
        self.wifi_enabled_state.set(snap.network.wifi_enabled);
        self.wifi_tile.set_sensitive(snap.network.available);
        if snap.network.wifi_enabled {
            self.wifi_tile.add_css_class("active");
            let sub = snap.network.primary_name.as_deref().unwrap_or("On");
            self.wifi_sub.set_text(sub);
        } else {
            self.wifi_tile.remove_css_class("active");
            self.wifi_sub.set_text("Off");
        }

        // Bluetooth tile + paired devices
        self.bt_powered_state.set(snap.bluetooth.powered);
        self.bt_tile.set_sensitive(snap.bluetooth.available);
        if snap.bluetooth.powered {
            self.bt_tile.add_css_class("active");
            let n = snap.bluetooth.connected_count();
            if n > 0 {
                self.bt_sub.set_text(&format!("{n} Connected"));
            } else {
                self.bt_sub.set_text("On");
            }
        } else {
            self.bt_tile.remove_css_class("active");
            self.bt_sub.set_text("Off");
        }

        while let Some(child) = self.bt_devices_box.first_child() {
            self.bt_devices_box.remove(&child);
        }
        if snap.bluetooth.powered && !snap.bluetooth.devices.is_empty() {
            self.bt_devices_box.set_visible(true);
            for dev in snap.bluetooth.devices.iter().take(3) {
                let row = GtkBox::new(Orientation::Horizontal, 8);
                row.add_css_class("neon-qs-card");
                let name = Label::new(Some(&dev.name));
                name.add_css_class("neon-qs-muted");
                name.set_hexpand(true);
                name.set_halign(Align::Start);
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
                row.append(&name);
                row.append(&btn);
                self.bt_devices_box.append(&row);
            }
        } else {
            self.bt_devices_box.set_visible(false);
        }

        // DND sync
        if self.store.is_do_not_disturb() {
            self.dnd_tile.add_css_class("active");
            self.dnd_sub.set_text("Do Not Disturb");
        } else {
            self.dnd_tile.remove_css_class("active");
            self.dnd_sub.set_text("Off");
        }

        // Audio capsule
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

        // Power & segmented profiles
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
            let empty = Label::new(Some("No new notifications"));
            empty.add_css_class("neon-qs-muted");
            empty.set_halign(Align::Start);
            self.notif_list_box.append(&empty);
            return;
        }

        for item in history.iter().take(6) {
            let row = GtkBox::new(Orientation::Vertical, 2);
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

fn build_cc_tile(icon_name: &str, title: &str, subtitle: &str) -> (Button, Label, Image) {
    let btn = Button::new();
    btn.add_css_class("neon-cc-tile");

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
    sub_lbl.set_max_width_chars(14);
    text.append(&title_lbl);
    text.append(&sub_lbl);

    row.append(&badge);
    row.append(&text);
    btn.set_child(Some(&row));

    (btn, sub_lbl, icon)
}
