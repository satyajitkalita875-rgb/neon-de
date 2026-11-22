//! NEON DE Floating Dock (`zwlr_layer_shell_v1` `Layer::Top` anchored Bottom).

use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, Image, Label, Orientation, Separator,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::{desktop_apps::DesktopApp, ipc_bridge::IpcCommandSender};

/// Floating dock displaying pinned favorite apps, live running windows, and the app launcher toggle.
pub struct Dock {
    _window: ApplicationWindow,
    running_box: GtkBox,
    separator: Separator,
    ipc: IpcCommandSender,
}

impl Dock {
    pub fn build(app: &Application, favorites: &[DesktopApp], ipc: IpcCommandSender) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dock")
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("neon-dock-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-dock"));
            window.set_layer(Layer::Top);
            window.set_anchor(Edge::Bottom, true);
            window.set_margin(Edge::Bottom, 10);
            window.set_keyboard_mode(KeyboardMode::None);
            window.auto_exclusive_zone_enable();
        }

        let container = GtkBox::new(Orientation::Horizontal, 8);
        container.add_css_class("neon-dock-container");

        // Pinned favorite applications
        for fav in favorites {
            let btn = make_app_dock_button(fav);
            let app_clone = fav.clone();
            btn.connect_clicked(move |_| {
                app_clone.launch();
            });
            container.append(&btn);
        }

        let separator = Separator::new(Orientation::Vertical);
        separator.add_css_class("neon-dock-separator");
        separator.set_visible(false);
        container.append(&separator);

        let running_box = GtkBox::new(Orientation::Horizontal, 6);
        container.append(&running_box);

        // App Grid / Overview button
        let sep2 = Separator::new(Orientation::Vertical);
        sep2.add_css_class("neon-dock-separator");
        container.append(&sep2);

        let grid_btn = Button::new();
        grid_btn.add_css_class("neon-dock-item");
        grid_btn.set_tooltip_text(Some("Applications & Overview (Super+Space)"));
        let grid_icon = Image::from_icon_name("view-app-grid-symbolic");
        grid_icon.set_pixel_size(22);
        grid_btn.set_child(Some(&grid_icon));
        {
            let ipc = ipc.clone();
            grid_btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::ToggleOverview);
            });
        }
        container.append(&grid_btn);

        window.set_child(Some(&container));
        window.present();

        Self {
            _window: window,
            running_box,
            separator,
            ipc,
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        while let Some(child) = self.running_box.first_child() {
            self.running_box.remove(&child);
        }

        self.separator.set_visible(!snapshot.windows.is_empty());

        for win in &snapshot.windows {
            let display_name = win
                .title
                .as_deref()
                .or(win.app_id.as_deref())
                .unwrap_or("Window");
            let short: String = display_name.chars().take(12).collect();
            let label = Label::new(Some(&short));

            let btn = Button::new();
            btn.add_css_class("neon-dock-item");
            btn.add_css_class("running");
            if win.focused {
                btn.add_css_class("focused");
            }
            if win.minimized {
                btn.set_opacity(0.55);
            }
            btn.set_tooltip_text(Some(display_name));
            btn.set_child(Some(&label));

            let win_id = win.id;
            let ipc = self.ipc.clone();
            btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::FocusWindow { window_id: win_id });
            });
            self.running_box.append(&btn);
        }
    }
}

fn make_app_dock_button(app: &DesktopApp) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-dock-item");
    btn.set_tooltip_text(Some(&app.name));

    let content = GtkBox::new(Orientation::Vertical, 2);
    if let Some(icon_name) = &app.icon {
        let icon = Image::from_icon_name(icon_name);
        icon.set_pixel_size(24);
        content.append(&icon);
    } else {
        let fallback: String = app.name.chars().take(2).collect();
        content.append(&Label::new(Some(&fallback)));
    }
    btn.set_child(Some(&content));
    btn
}
