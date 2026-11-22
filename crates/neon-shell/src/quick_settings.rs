//! NEON DE Control Center / Quick Settings & Notification Center (`zwlr_layer_shell_v1` `Layer::Overlay`).

use std::{cell::Cell, rc::Rc, sync::Arc};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation, Scale,
    ScrolledWindow, Separator, Switch, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::IpcRequest;
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
    // Audio
    sink_label: Label,
    volume_scale: Scale,
    mute_btn: Button,
    // Network
    wifi_label: Label,
    wifi_switch: Switch,
    // Bluetooth
    bt_label: Label,
    bt_switch: Switch,
    bt_devices_box: GtkBox,
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
            .default_width(360)
            .build();
        window.add_css_class("neon-quicksettings-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-quick-settings"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, 44);
            window.set_margin(Edge::Right, 12);
            window.set_keyboard_mode(KeyboardMode::OnDemand);
        }

        let syncing = Rc::new(Cell::new(false));

        let root = GtkBox::new(Orientation::Vertical, 10);
        root.add_css_class("neon-quicksettings-panel");

        // Header
        let header = GtkBox::new(Orientation::Horizontal, 8);
        let title = Label::new(Some("Control Center"));
        title.add_css_class("neon-qs-title");
        title.set_hexpand(true);
        title.set_halign(Align::Start);

        let close_btn = Button::with_label("×");
        close_btn.add_css_class("neon-close-button");
        {
            let win = window.clone();
            let overlay = toast_overlay.clone();
            close_btn.connect_clicked(move |_| {
                win.set_visible(false);
                overlay.set_suppressed(false);
            });
        }
        header.append(&title);
        header.append(&close_btn);
        root.append(&header);

        // --- Audio Card (PipeWire / WirePlumber) ---
        let audio_card = GtkBox::new(Orientation::Vertical, 6);
        audio_card.add_css_class("neon-qs-card");
        let audio_head = GtkBox::new(Orientation::Horizontal, 8);
        let sink_label = Label::new(Some("Audio Output"));
        sink_label.add_css_class("neon-qs-title");
        sink_label.set_hexpand(true);
        sink_label.set_halign(Align::Start);
        sink_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let mute_btn = Button::with_label("Mute");
        mute_btn.add_css_class("neon-status-pill");
        {
            let sys_tx = sys_tx.clone();
            mute_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::ToggleSinkMute);
            });
        }
        audio_head.append(&sink_label);
        audio_head.append(&mute_btn);

        let volume_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 2.0);
        volume_scale.set_draw_value(true);
        volume_scale.set_value(70.0);
        {
            let sys_tx = sys_tx.clone();
            let syncing = Rc::clone(&syncing);
            volume_scale.connect_value_changed(move |s| {
                if syncing.get() {
                    return;
                }
                let vol = (s.value() as f32) / 100.0;
                sys_tx.send(SystemAction::SetSinkVolume(vol));
            });
        }
        audio_card.append(&audio_head);
        audio_card.append(&volume_scale);
        root.append(&audio_card);

        // --- Connectivity Card (NetworkManager + BlueZ) ---
        let conn_card = GtkBox::new(Orientation::Vertical, 8);
        conn_card.add_css_class("neon-qs-card");

        let wifi_row = GtkBox::new(Orientation::Horizontal, 8);
        let wifi_label = Label::new(Some("Wi-Fi: Checking..."));
        wifi_label.set_hexpand(true);
        wifi_label.set_halign(Align::Start);
        let wifi_switch = Switch::new();
        wifi_switch.set_valign(Align::Center);
        {
            let sys_tx = sys_tx.clone();
            let syncing = Rc::clone(&syncing);
            wifi_switch.connect_state_set(move |_, active| {
                if !syncing.get() {
                    sys_tx.send(SystemAction::SetWifiEnabled(active));
                }
                gtk4::glib::Propagation::Proceed
            });
        }
        wifi_row.append(&wifi_label);
        wifi_row.append(&wifi_switch);

        let bt_row = GtkBox::new(Orientation::Horizontal, 8);
        let bt_label = Label::new(Some("Bluetooth: Checking..."));
        bt_label.set_hexpand(true);
        bt_label.set_halign(Align::Start);
        let bt_switch = Switch::new();
        bt_switch.set_valign(Align::Center);
        {
            let sys_tx = sys_tx.clone();
            let syncing = Rc::clone(&syncing);
            bt_switch.connect_state_set(move |_, active| {
                if !syncing.get() {
                    sys_tx.send(SystemAction::SetBluetoothPowered(active));
                }
                gtk4::glib::Propagation::Proceed
            });
        }
        bt_row.append(&bt_label);
        bt_row.append(&bt_switch);

        let bt_devices_box = GtkBox::new(Orientation::Vertical, 4);

        conn_card.append(&wifi_row);
        conn_card.append(&Separator::new(Orientation::Horizontal));
        conn_card.append(&bt_row);
        conn_card.append(&bt_devices_box);
        root.append(&conn_card);

        // --- Battery & Power Profiles Card (UPower) ---
        let power_card = GtkBox::new(Orientation::Vertical, 8);
        power_card.add_css_class("neon-qs-card");
        let battery_label = Label::new(Some("Power: Checking..."));
        battery_label.add_css_class("neon-qs-title");
        battery_label.set_halign(Align::Start);

        let profiles_row = GtkBox::new(Orientation::Horizontal, 6);
        let profile_saver_btn = Button::with_label("Saver");
        let profile_balanced_btn = Button::with_label("Balanced");
        let profile_perf_btn = Button::with_label("Perf");
        for (btn, profile) in [
            (&profile_saver_btn, "power-saver"),
            (&profile_balanced_btn, "balanced"),
            (&profile_perf_btn, "performance"),
        ] {
            btn.add_css_class("neon-status-pill");
            btn.set_hexpand(true);
            let sys_tx = sys_tx.clone();
            let prof = profile.to_string();
            btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::SetPowerProfile(prof.clone()));
            });
            profiles_row.append(btn);
        }
        power_card.append(&battery_label);
        power_card.append(&profiles_row);
        root.append(&power_card);

        // --- Notifications Center Card ---
        let notif_card = GtkBox::new(Orientation::Vertical, 6);
        notif_card.add_css_class("neon-qs-card");

        let notif_head = GtkBox::new(Orientation::Horizontal, 6);
        let notif_title = Label::new(Some("Notifications"));
        notif_title.add_css_class("neon-qs-title");
        notif_title.set_hexpand(true);
        notif_title.set_halign(Align::Start);

        let dnd_lbl = Label::new(Some("DND"));
        dnd_lbl.add_css_class("neon-qs-muted");
        let dnd_switch = Switch::new();
        dnd_switch.set_valign(Align::Center);
        {
            let store = Arc::clone(&store);
            dnd_switch.connect_state_set(move |_, active| {
                store.set_do_not_disturb(active);
                gtk4::glib::Propagation::Proceed
            });
        }

        let test_btn = Button::with_label("Test");
        test_btn.add_css_class("neon-status-pill");
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
        clear_btn.add_css_class("neon-status-pill");
        {
            let store = Arc::clone(&store);
            clear_btn.connect_clicked(move |_| {
                store.clear_all();
            });
        }

        notif_head.append(&notif_title);
        notif_head.append(&dnd_lbl);
        notif_head.append(&dnd_switch);
        notif_head.append(&test_btn);
        notif_head.append(&clear_btn);
        notif_card.append(&notif_head);

        let notif_scroller = ScrolledWindow::builder()
            .max_content_height(160)
            .propagate_natural_height(true)
            .build();
        let notif_list_box = GtkBox::new(Orientation::Vertical, 6);
        notif_scroller.set_child(Some(&notif_list_box));
        notif_card.append(&notif_scroller);
        root.append(&notif_card);

        // --- Footer Quick Actions ---
        let footer = GtkBox::new(Orientation::Horizontal, 8);
        let settings_btn = Button::with_label("Open Settings");
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

        let lock_btn = Button::with_label("Lock Screen");
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

        footer.append(&settings_btn);
        footer.append(&lock_btn);
        root.append(&footer);

        window.set_child(Some(&root));
        window.set_visible(false);

        let panel = Self {
            window,
            syncing,
            sink_label,
            volume_scale,
            mute_btn,
            wifi_label,
            wifi_switch,
            bt_label,
            bt_switch,
            bt_devices_box,
            battery_label,
            profile_saver_btn,
            profile_balanced_btn,
            profile_perf_btn,
            notif_list_box,
            store,
            sys_tx,
            toast_overlay,
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

    pub fn update_system(&self, snap: &SystemSnapshot) {
        self.syncing.set(true);

        // Audio
        if snap.audio.available {
            let sink = snap
                .audio
                .default_sink_name
                .as_deref()
                .unwrap_or("PipeWire Audio");
            self.sink_label.set_text(sink);
            self.volume_scale
                .set_value(f64::from(snap.audio.volume_percent()));
            self.mute_btn
                .set_label(if snap.audio.muted { "Unmute" } else { "Mute" });
            if snap.audio.muted {
                self.mute_btn.add_css_class("active");
            } else {
                self.mute_btn.remove_css_class("active");
            }
        } else {
            self.sink_label.set_text("Audio Unavailable");
        }

        // Network
        self.wifi_label.set_text(&snap.network.panel_label());
        self.wifi_switch.set_sensitive(snap.network.available);
        self.wifi_switch.set_active(snap.network.wifi_enabled);

        // Bluetooth
        let bt_text = if !snap.bluetooth.available {
            "Bluetooth: Unavailable".to_string()
        } else if snap.bluetooth.powered {
            let n = snap.bluetooth.connected_count();
            if n > 0 {
                format!("Bluetooth: On ({n} connected)")
            } else {
                "Bluetooth: On".to_string()
            }
        } else {
            "Bluetooth: Off".to_string()
        };
        self.bt_label.set_text(&bt_text);
        self.bt_switch.set_sensitive(snap.bluetooth.available);
        self.bt_switch.set_active(snap.bluetooth.powered);

        while let Some(child) = self.bt_devices_box.first_child() {
            self.bt_devices_box.remove(&child);
        }
        for dev in snap.bluetooth.devices.iter().take(4) {
            let row = GtkBox::new(Orientation::Horizontal, 6);
            let name = Label::new(Some(&dev.name));
            name.add_css_class("neon-qs-muted");
            name.set_hexpand(true);
            name.set_halign(Align::Start);
            let btn = Button::with_label(if dev.connected {
                "Disconnect"
            } else {
                "Connect"
            });
            btn.add_css_class("neon-status-pill");
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

        // Power & Profiles
        if snap.power.has_battery {
            self.battery_label.set_text(&format!(
                "Battery: {:.0}% · {}",
                snap.power.battery_percentage,
                snap.power.battery_state.label()
            ));
        } else {
            self.battery_label.set_text("Power: AC Adapter");
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
            let empty = Label::new(Some("No notifications"));
            empty.add_css_class("neon-qs-muted");
            empty.set_halign(Align::Start);
            self.notif_list_box.append(&empty);
            return;
        }

        for item in history.iter().take(8) {
            let row = GtkBox::new(Orientation::Vertical, 2);
            let top = GtkBox::new(Orientation::Horizontal, 6);
            let title = Label::new(Some(&format!("{} — {}", item.app_name, item.summary)));
            title.add_css_class("neon-qs-title");
            title.set_hexpand(true);
            title.set_halign(Align::Start);
            title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            top.append(&title);
            row.append(&top);

            if !item.body.is_empty() {
                let body = Label::new(Some(&item.body));
                body.add_css_class("neon-qs-muted");
                body.set_halign(Align::Start);
                body.set_wrap(true);
                body.set_max_width_chars(40);
                row.append(&body);
            }
            self.notif_list_box.append(&row);
        }
    }
}
