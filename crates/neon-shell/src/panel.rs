//! NEON DE macOS × GNOME 47 Top Menu Bar (`zwlr_layer_shell_v1` `Layer::Top` anchored Top+Left+Right).

use std::{cell::Cell, f64::consts::PI, rc::Rc, time::Duration};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, DrawingArea, Image,
    Label, Orientation, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};
use neon_system::{
    SystemAction, SystemActionSender, SystemSnapshot, network::ConnectionKind, power::BatteryState,
};

use crate::{ipc_bridge::IpcCommandSender, quick_settings::QuickSettingsPanel};

/// Live handles to updatable widgets in the NEON DE Top Menu Bar.
pub struct TopPanel {
    window: ApplicationWindow,
    activities_btn: Button,
    workspaces_box: GtkBox,
    focused_label: Label,
    clock_btn: Button,
    clock_label: Label,
    net_btn: Button,
    net_icon: Image,
    bt_btn: Button,
    audio_btn: Button,
    audio_icon: Image,
    power_btn: Button,
    power_icon: Image,
    power_pct_label: Label,
    cc_icon_area: DrawingArea,
    is_light_theme: Rc<Cell<bool>>,
    notif_dot: GtkBox,
    ipc: IpcCommandSender,
}

impl TopPanel {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        sys_tx: SystemActionSender,
        quick_settings: QuickSettingsPanel,
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
            window.auto_exclusive_zone_enable();
        }

        let bar = CenterBox::new();
        bar.set_hexpand(true);
        bar.add_css_class("neon-top-panel");

        // Left: NEON brand / Activities button + GNOME 47 workspace dots + focused app name
        let left_box = GtkBox::new(Orientation::Horizontal, 8);
        left_box.set_valign(Align::Center);

        let activities_btn = Button::with_label("NEON");
        activities_btn.add_css_class("neon-brand-button");
        activities_btn.set_tooltip_text(Some("Activities Overview (Super+Space)"));
        {
            let ipc = ipc.clone();
            activities_btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::ToggleOverview);
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

        // Center: Clean GNOME / macOS Date & Time (click toggles Control Center & Notifications)
        let clock_label = Label::new(Some(&format_clock_now()));
        let clock_btn = Button::new();
        clock_btn.add_css_class("neon-clock-btn");
        clock_btn.set_child(Some(&clock_label));
        clock_btn.set_tooltip_text(Some("Date, Time & Notifications"));
        {
            let qs = quick_settings.clone();
            clock_btn.connect_clicked(move |_| qs.toggle());
        }
        {
            let clock_clone = clock_label.clone();
            glib::timeout_add_local(Duration::from_secs(1), move || {
                clock_clone.set_text(&format_clock_now());
                glib::ControlFlow::Continue
            });
        }

        // Right: Crisp symbolic icons (Wi-Fi, Bluetooth, Volume, Battery + %, Control Center)
        let right_box = GtkBox::new(Orientation::Horizontal, 2);
        right_box.set_valign(Align::Center);

        let shot_icon = Image::from_icon_name("camera-photo-symbolic");
        shot_icon.set_pixel_size(15);
        let shot_btn = Button::new();
        shot_btn.add_css_class("neon-tray-btn");
        shot_btn.set_child(Some(&shot_icon));
        shot_btn.set_tooltip_text(Some("Screenshot (Print / Super+Shift+4)"));
        {
            let ipc = ipc.clone();
            shot_btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::OpenScreenshotUi);
            });
        }

        let net_icon = Image::from_icon_name("network-wireless-signal-excellent-symbolic");
        net_icon.set_pixel_size(15);
        let net_btn = Button::new();
        net_btn.add_css_class("neon-tray-btn");
        net_btn.set_child(Some(&net_icon));
        net_btn.set_tooltip_text(Some("Network"));
        {
            let qs = quick_settings.clone();
            net_btn.connect_clicked(move |_| qs.toggle());
        }

        let bt_icon = Image::from_icon_name("bluetooth-active-symbolic");
        bt_icon.set_pixel_size(15);
        let bt_btn = Button::new();
        bt_btn.add_css_class("neon-tray-btn");
        bt_btn.set_child(Some(&bt_icon));
        bt_btn.set_tooltip_text(Some("Bluetooth"));
        bt_btn.set_visible(false);
        {
            let qs = quick_settings.clone();
            bt_btn.connect_clicked(move |_| qs.toggle());
        }

        let audio_icon = Image::from_icon_name("audio-volume-high-symbolic");
        audio_icon.set_pixel_size(15);
        let audio_btn = Button::new();
        audio_btn.add_css_class("neon-tray-btn");
        audio_btn.set_child(Some(&audio_icon));
        audio_btn.set_tooltip_text(Some("Volume — Click to Mute/Unmute"));
        {
            let sys_tx = sys_tx.clone();
            audio_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::ToggleSinkMute);
            });
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

        let is_light_theme = Rc::new(Cell::new(false));

        // Authentic macOS Control Center dual-toggle-switches menu bar icon
        let cc_inner = GtkBox::new(Orientation::Horizontal, 4);
        let cc_icon_area = DrawingArea::new();
        cc_icon_area.set_content_width(18);
        cc_icon_area.set_content_height(15);
        cc_icon_area.set_valign(Align::Center);
        {
            let light_flag = Rc::clone(&is_light_theme);
            cc_icon_area.set_draw_func(move |_, cr, w, h| {
                let (r, g, b) = if light_flag.get() {
                    (0.06, 0.09, 0.16)
                } else {
                    (0.95, 0.97, 1.0)
                };
                let w = f64::from(w);
                let h = f64::from(h);
                let pill_h = 5.4;
                let pill_w = w - 2.0;
                let rx = pill_h * 0.5;

                // Helper to trace a horizontal capsule
                let trace_pill = |cr: &gtk4::cairo::Context, x: f64, y: f64, pw: f64, ph: f64| {
                    let rad = ph * 0.5;
                    cr.new_sub_path();
                    cr.arc(x + pw - rad, y + rad, rad, -PI * 0.5, PI * 0.5);
                    cr.arc(x + rad, y + rad, rad, PI * 0.5, PI * 1.5);
                    cr.close_path();
                };

                // Top pill: outlined track + solid knob on left
                let top_y = 1.2;
                cr.set_source_rgba(r, g, b, 0.92);
                cr.set_line_width(1.25);
                trace_pill(cr, 1.0, top_y, pill_w, pill_h);
                let _ = cr.stroke();
                cr.arc(1.0 + rx, top_y + rx, rx - 0.8, 0.0, 2.0 * PI);
                let _ = cr.fill();

                // Bottom pill: filled track + bright knob on right
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
        cc_btn.set_tooltip_text(Some("Control Center"));
        {
            let cc_btn_c = cc_btn.clone();
            quick_settings.set_on_state_change(move |open| {
                if open {
                    cc_btn_c.add_css_class("active");
                } else {
                    cc_btn_c.remove_css_class("active");
                }
            });
        }
        {
            let qs = quick_settings;
            cc_btn.connect_clicked(move |_| qs.toggle());
        }

        right_box.append(&shot_btn);
        right_box.append(&net_btn);
        right_box.append(&bt_btn);
        right_box.append(&audio_btn);
        right_box.append(&power_btn);
        right_box.append(&cc_btn);
        right_box.append(&clock_btn);

        bar.set_start_widget(Some(&left_box));
        bar.set_end_widget(Some(&right_box));

        window.set_child(Some(&bar));
        window.present();

        Self {
            window,
            activities_btn,
            workspaces_box,
            focused_label,
            clock_btn,
            clock_label,
            net_btn,
            net_icon,
            bt_btn,
            audio_btn,
            audio_icon,
            power_btn,
            power_icon,
            power_pct_label,
            cc_icon_area,
            is_light_theme,
            notif_dot,
            ipc,
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        let light =
            snapshot.appearance.theme == "neon_light" || snapshot.appearance.theme == "light";
        if self.is_light_theme.get() != light {
            self.is_light_theme.set(light);
            self.cc_icon_area.queue_draw();
        }
        self.window.set_visible(snapshot.appearance.panel_visible);
        self.clock_btn.set_visible(snapshot.appearance.show_clock);
        self.clock_label.set_visible(snapshot.appearance.show_clock);
        self.workspaces_box
            .set_visible(snapshot.appearance.show_workspaces);

        if snapshot.overview_open {
            self.activities_btn.add_css_class("active");
        } else {
            self.activities_btn.remove_css_class("active");
        }

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

        let title = snapshot
            .windows
            .iter()
            .find(|w| w.focused)
            .and_then(|w| w.title.as_deref().or(w.app_id.as_deref()))
            .unwrap_or("Desktop");
        self.focused_label.set_text(title);
    }

    pub fn update_system(&self, sys: &SystemSnapshot, unread_notifications: usize) {
        // Network icon + tooltip
        let net_icon_name = if !sys.network.available || !sys.network.connectivity.is_connected() {
            "network-wireless-offline-symbolic"
        } else {
            match sys.network.primary_kind {
                ConnectionKind::Ethernet => "network-wired-symbolic",
                ConnectionKind::Vpn => "network-vpn-symbolic",
                ConnectionKind::Wifi => match sys.network.wifi_strength.unwrap_or(80) {
                    75..=100 => "network-wireless-signal-excellent-symbolic",
                    50..=74 => "network-wireless-signal-good-symbolic",
                    25..=49 => "network-wireless-signal-ok-symbolic",
                    _ => "network-wireless-signal-weak-symbolic",
                },
                _ => "network-transmit-receive-symbolic",
            }
        };
        self.net_icon.set_icon_name(Some(net_icon_name));
        self.net_btn
            .set_tooltip_text(Some(&sys.network.panel_label()));

        // Bluetooth icon visibility
        self.bt_btn
            .set_visible(sys.bluetooth.available && sys.bluetooth.powered);
        self.bt_btn.set_tooltip_text(Some(&format!(
            "Bluetooth ({} connected)",
            sys.bluetooth.connected_count()
        )));

        // Audio symbolic icon + tooltip
        let vol_pct = sys.audio.volume_percent();
        let vol_icon_name = if sys.audio.muted || vol_pct == 0 {
            "audio-volume-muted-symbolic"
        } else if vol_pct < 34 {
            "audio-volume-low-symbolic"
        } else if vol_pct < 67 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.audio_icon.set_icon_name(Some(vol_icon_name));
        self.audio_btn.set_tooltip_text(Some(&format!(
            "{} — Click to toggle mute",
            sys.audio.panel_label()
        )));
        if sys.audio.muted {
            self.audio_btn.add_css_class("active");
        } else {
            self.audio_btn.remove_css_class("active");
        }

        // Power / Battery icon + compact percentage
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

fn format_clock_now() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%a %b %e  %H:%M"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "NEON DE".to_string())
}

pub fn launch_sibling_binary(name: &str) {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(name);
        if sibling.exists() {
            let _ = std::process::Command::new(sibling).spawn();
            return;
        }
    }
    let _ = std::process::Command::new(name).spawn();
}
