//! NEON DE Top Panel (`zwlr_layer_shell_v1` `Layer::Top` anchored Top+Left+Right).

use std::time::Duration;

use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, Label, Orientation, glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::ipc_bridge::IpcCommandSender;

/// Live handles to updatable widgets in the NEON DE Top Panel.
pub struct TopPanel {
    _window: ApplicationWindow,
    activities_btn: Button,
    workspaces_box: GtkBox,
    focused_label: Label,
    ipc: IpcCommandSender,
}

impl TopPanel {
    pub fn build(app: &Application, ipc: IpcCommandSender) -> Self {
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

        // Right: Active window title + NEON badge
        let right_box = GtkBox::new(Orientation::Horizontal, 10);
        let focused_label = Label::new(Some("Desktop"));
        focused_label.add_css_class("neon-focused-title");
        focused_label.set_max_width_chars(32);
        focused_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let brand_btn = Button::with_label("NEON DE");
        brand_btn.add_css_class("neon-pill-button");
        {
            let ipc = ipc.clone();
            brand_btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::ToggleOverview);
            });
        }

        right_box.append(&focused_label);
        right_box.append(&brand_btn);

        bar.set_start_widget(Some(&left_box));
        bar.set_center_widget(Some(&clock_label));
        bar.set_end_widget(Some(&right_box));

        window.set_child(Some(&bar));
        window.present();

        Self {
            _window: window,
            activities_btn,
            workspaces_box,
            focused_label,
            ipc,
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        if snapshot.overview_open {
            self.activities_btn.add_css_class("active");
        } else {
            self.activities_btn.remove_css_class("active");
        }

        // Rebuild dynamic workspace pills cleanly
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
}

fn format_clock_now() -> String {
    glib::DateTime::now_local()
        .and_then(|dt| dt.format("%a %b %e  %H:%M"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "NEON DE".to_string())
}
