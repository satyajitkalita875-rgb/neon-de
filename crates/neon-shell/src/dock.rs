//! NEON DE macOS-Inspired Glass Floating Dock (`zwlr_layer_shell_v1` `Layer::Top` anchored Bottom).

use std::rc::Rc;

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, Image, Orientation, Separator,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::{desktop_apps::DesktopApp, ipc_bridge::IpcCommandSender, panel::launch_sibling_binary};

/// Floating glass dock displaying icon-only pinned apps, running window icons with glowing
/// macOS/GNOME indicator dots, Settings, and the Applications Grid trigger.
pub struct Dock {
    _window: ApplicationWindow,
    running_box: GtkBox,
    separator: Separator,
    all_apps: Rc<Vec<DesktopApp>>,
    ipc: IpcCommandSender,
}

impl Dock {
    pub fn build(
        app: &Application,
        favorites: &[DesktopApp],
        all_apps: Vec<DesktopApp>,
        ipc: IpcCommandSender,
    ) -> Self {
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

        let container = GtkBox::new(Orientation::Horizontal, 4);
        container.add_css_class("neon-dock-container");

        // Pinned favorite applications (pure crisp 38px icons with tooltips)
        for fav in favorites {
            let btn = make_icon_dock_button(
                fav.icon.as_deref().unwrap_or("application-x-executable"),
                &fav.name,
                false,
                false,
            );
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

        let running_box = GtkBox::new(Orientation::Horizontal, 4);
        container.append(&running_box);

        // Utility section on the right: Settings + App Grid
        let sep2 = Separator::new(Orientation::Vertical);
        sep2.add_css_class("neon-dock-separator");
        container.append(&sep2);

        let settings_btn =
            make_icon_dock_button("preferences-system", "NEON DE Settings", false, false);
        settings_btn.connect_clicked(move |_| {
            launch_sibling_binary("neon-settings");
        });
        container.append(&settings_btn);

        let grid_btn = make_icon_dock_button(
            "view-app-grid-symbolic",
            "Applications & Overview (Super+Space)",
            false,
            false,
        );
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
            all_apps: Rc::new(all_apps),
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
            let icon_name =
                resolve_window_icon(win.app_id.as_deref(), win.title.as_deref(), &self.all_apps);

            let btn = make_icon_dock_button(&icon_name, display_name, true, win.focused);
            if win.minimized {
                btn.set_opacity(0.55);
            }

            let win_id = win.id;
            let ipc = self.ipc.clone();
            btn.connect_clicked(move |_| {
                ipc.send(IpcRequest::FocusWindow { window_id: win_id });
            });
            self.running_box.append(&btn);
        }
    }
}

fn make_icon_dock_button(icon_name: &str, tooltip: &str, running: bool, focused: bool) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-dock-item");
    if focused {
        btn.add_css_class("focused");
    }
    btn.set_tooltip_text(Some(tooltip));

    let vbox = GtkBox::new(Orientation::Vertical, 0);
    vbox.set_halign(Align::Center);
    vbox.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(38);
    vbox.append(&icon);

    let dot = GtkBox::new(Orientation::Horizontal, 0);
    dot.add_css_class("neon-dock-dot");
    dot.set_halign(Align::Center);
    if running {
        dot.add_css_class("running");
    }
    if focused {
        dot.add_css_class("focused");
    }
    vbox.append(&dot);

    btn.set_child(Some(&vbox));
    btn
}

fn resolve_window_icon(
    app_id: Option<&str>,
    title: Option<&str>,
    all_apps: &[DesktopApp],
) -> String {
    let needle_id = app_id.unwrap_or("").to_lowercase();
    let needle_title = title.unwrap_or("").to_lowercase();

    if needle_id.contains("neonde.settings") || needle_title.contains("neon de settings") {
        return "preferences-system".to_string();
    }
    if needle_id.contains("terminal")
        || needle_id.contains("alacritty")
        || needle_id.contains("kitty")
        || needle_id.contains("foot")
        || needle_title.contains("terminal")
    {
        return "utilities-terminal".to_string();
    }

    for app in all_apps {
        let name_l = app.name.to_lowercase();
        let exec_l = app.exec_program.to_lowercase();
        let matched = (!needle_id.is_empty()
            && (exec_l.contains(&needle_id) || name_l.contains(&needle_id)))
            || (!needle_title.is_empty() && needle_title.contains(&name_l));
        if matched && let Some(ref icon) = app.icon {
            return icon.clone();
        }
    }

    "application-x-executable".to_string()
}
