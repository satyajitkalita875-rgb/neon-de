//! NEON DE Top Panel (`zwlr_layer_shell_v1` `Layer::Top` anchored Top+Left+Right).

use std::time::Duration;

use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, Label, Orientation, glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};
use neon_system::{SystemAction, SystemActionSender, SystemSnapshot};

use crate::{ipc_bridge::IpcCommandSender, quick_settings::QuickSettingsPanel};

/// Live handles to updatable widgets in the NEON DE Top Panel.
pub struct TopPanel {
    window: ApplicationWindow,
    activities_btn: Button,
    workspaces_box: GtkBox,
    clock_label: Label,
    focused_label: Label,
    net_btn: Button,
    audio_btn: Button,
    power_btn: Button,
    control_center_btn: Button,
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
            .title("NEON DE Top Panel")
            .decorated(false)
            .resizable(false)
            .default_height(36)
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
        bar.add_css_class("neon-top-panel");

        // Left: Activities toggle + dynamic workspace indicator strip
        let left_box = GtkBox::new(Orientation::Horizontal, 8);
        let activities_btn = Button::with_label("Activities");
        activities_btn.add_css_class("neon-pill-button");
        {
            let ipc = ipc.clone();
            activities_btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::ToggleOverview);
            });
        }

        let workspaces_box = GtkBox::new(Orientation::Horizontal, 4);
        left_box.append(&activities_btn);
        left_box.append(&workspaces_box);

        // Center: Live clock & date
        let clock_label = Label::new(Some(&format_clock_now()));
        clock_label.add_css_class("neon-clock");
        {
            let clock_clone = clock_label.clone();
            glib::timeout_add_local(Duration::from_secs(1), move || {
                clock_clone.set_text(&format_clock_now());
                glib::ControlFlow::Continue
            });
        }

        // Right: Active window title + Live System Tray (Network / PipeWire Audio / UPower Battery) + Control Center + Settings + Lock
        let right_box = GtkBox::new(Orientation::Horizontal, 6);
        let focused_label = Label::new(Some("Desktop"));
        focused_label.add_css_class("neon-focused-title");
        focused_label.set_max_width_chars(22);
        focused_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let net_btn = Button::with_label("Net: --");
        net_btn.add_css_class("neon-status-pill");
        net_btn.set_tooltip_text(Some("NetworkManager Status — Click for Control Center"));
        {
            let qs = quick_settings.clone();
            net_btn.connect_clicked(move |_| qs.toggle());
        }

        let audio_btn = Button::with_label("Vol: --");
        audio_btn.add_css_class("neon-status-pill");
        audio_btn.set_tooltip_text(Some("PipeWire Volume — Click to Mute/Unmute"));
        {
            let sys_tx = sys_tx.clone();
            audio_btn.connect_clicked(move |_| {
                sys_tx.send(SystemAction::ToggleSinkMute);
            });
        }

        let power_btn = Button::with_label("Pwr: --");
        power_btn.add_css_class("neon-status-pill");
        power_btn.set_tooltip_text(Some("UPower Battery & Profile — Click for Control Center"));
        {
            let qs = quick_settings.clone();
            power_btn.connect_clicked(move |_| qs.toggle());
        }

        let control_center_btn = Button::with_label("System");
        control_center_btn.add_css_class("neon-pill-button");
        control_center_btn.set_tooltip_text(Some("Open Control Center & Notifications"));
        {
            let qs = quick_settings;
            control_center_btn.connect_clicked(move |_| qs.toggle());
        }

        let settings_btn = Button::with_label("Settings");
        settings_btn.add_css_class("neon-pill-button");
        settings_btn.set_tooltip_text(Some("Open NEON DE Settings"));
        settings_btn.connect_clicked(move |_| {
            launch_sibling_binary("neon-settings");
        });

        let lock_btn = Button::with_label("Lock");
        lock_btn.add_css_class("neon-pill-button");
        lock_btn.set_tooltip_text(Some("Lock Session (Super+L)"));
        {
            let ipc = ipc.clone();
            lock_btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::LockSession);
            });
        }

        right_box.append(&focused_label);
        right_box.append(&net_btn);
        right_box.append(&audio_btn);
        right_box.append(&power_btn);
        right_box.append(&control_center_btn);
        right_box.append(&settings_btn);
        right_box.append(&lock_btn);

        bar.set_start_widget(Some(&left_box));
        bar.set_center_widget(Some(&clock_label));
        bar.set_end_widget(Some(&right_box));

        window.set_child(Some(&bar));
        window.present();

        Self {
            window,
            activities_btn,
            workspaces_box,
            clock_label,
            focused_label,
            net_btn,
            audio_btn,
            power_btn,
            control_center_btn,
            ipc,
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        self.window.set_visible(snapshot.appearance.panel_visible);
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
            let label = if ws.window_ids.is_empty() {
                format!("{}", ws.index + 1)
            } else {
                format!("{} · {}", ws.index + 1, ws.window_ids.len())
            };
            let pill = Button::with_label(&label);
            pill.add_css_class("neon-ws-pill");
            if ws.active {
                pill.add_css_class("active");
            }
            let idx = ws.index;
            let ipc = self.ipc.clone();
            pill.connect_clicked(move |_| {
                ipc.send(IpcRequest::SwitchWorkspace { index: idx });
            });
            self.workspaces_box.append(&pill);
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
        self.net_btn.set_label(&sys.network.panel_label());
        self.audio_btn.set_label(&sys.audio.panel_label());
        if sys.audio.muted {
            self.audio_btn.add_css_class("active");
        } else {
            self.audio_btn.remove_css_class("active");
        }
        self.power_btn.set_label(&sys.power.panel_label());
        if unread_notifications > 0 {
            self.control_center_btn
                .set_label(&format!("System ({unread_notifications})"));
        } else {
            self.control_center_btn.set_label("System");
        }
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
