//! NEON DE macOS-Inspired Glass Floating Dock (`zwlr_layer_shell_v1` `Layer::Top` anchored Bottom).

use std::{cell::RefCell, rc::Rc};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, GestureClick, Orientation,
    Separator, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::{
    context_menu::ContextMenuOverlay,
    desktop_apps::{DesktopApp, load_app_icon},
    ipc_bridge::IpcCommandSender,
    panel::launch_sibling_binary,
};

type DockWindowSig = (u64, Option<String>, Option<String>, bool, bool);

/// Floating glass dock displaying icon-only pinned apps, running window icons with glowing
/// macOS/GNOME indicator dots, Settings, and the Applications Grid trigger.
pub struct Dock {
    window: ApplicationWindow,
    container: GtkBox,
    running_box: GtkBox,
    separator: Separator,
    all_apps: Rc<Vec<DesktopApp>>,
    last_windows: RefCell<Vec<DockWindowSig>>,
    context_menu: ContextMenuOverlay,
    ipc: IpcCommandSender,
}

impl Dock {
    pub fn build(
        app: &Application,
        favorites: &[DesktopApp],
        all_apps: Vec<DesktopApp>,
        ipc: IpcCommandSender,
        context_menu: ContextMenuOverlay,
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
            window.set_margin(Edge::Bottom, 0);
            window.set_keyboard_mode(KeyboardMode::None);
            window.set_exclusive_zone(74);
        }

        // Transparent outer wrapper provides unclipped breathing room for the dock's
        // soft drop shadow and hover-lift animation so no rectangular box ever clips around corners.
        let wrapper = GtkBox::new(Orientation::Horizontal, 0);
        wrapper.add_css_class("neon-dock-wrapper");

        let container = GtkBox::new(Orientation::Horizontal, 4);
        container.add_css_class("neon-dock-container");

        // Pinned favorite applications (pure crisp 38px icons with tooltips + right-click menu)
        for fav in favorites {
            let btn = make_icon_dock_button(
                fav.icon.as_deref().unwrap_or("application-x-executable"),
                &fav.name,
                false,
                false,
            );
            let app_clone = fav.clone();
            let ipc_fav = ipc.clone();
            btn.connect_clicked(move |_| {
                app_clone.launch();
                ipc_fav.send(IpcRequest::SetOverview { open: false });
            });
            let rc_gesture = GestureClick::new();
            rc_gesture.set_button(3);
            {
                let cm = context_menu.clone();
                let app_rc = fav.clone();
                let cont = container.clone();
                let btn_c = btn.clone();
                rc_gesture.connect_pressed(move |gesture, _, x, _y| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    let (sx, sy) = dock_widget_screen_coords(&cm, &cont, &btn_c, x);
                    cm.open_pinned_app_menu(sx, sy, app_rc.clone());
                });
            }
            btn.add_controller(rc_gesture);
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
        {
            let ipc_set = ipc.clone();
            settings_btn.connect_clicked(move |_| {
                launch_sibling_binary("neon-settings");
                ipc_set.send(IpcRequest::SetOverview { open: false });
            });
        }
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

        // Right-clicking empty dock background opens Dock & Appearance settings menu.
        let dock_rc = GestureClick::new();
        dock_rc.set_button(3);
        {
            let cm = context_menu.clone();
            let cont = container.clone();
            dock_rc.connect_pressed(move |_, _, x, _y| {
                let (out_w, out_h) = cm.output_size();
                let dock_w = cont.width().max(200);
                let dock_left = (out_w - dock_w) / 2;
                cm.open_dock_menu(dock_left + x.round() as i32, out_h - 240);
            });
        }
        container.add_controller(dock_rc);

        wrapper.append(&container);
        window.set_child(Some(&wrapper));
        window.present();

        Self {
            window,
            container,
            running_box,
            separator,
            all_apps: Rc::new(all_apps),
            last_windows: RefCell::new(Vec::new()),
            context_menu,
            ipc,
        }
    }

    /// Elevates or restores the Dock's Wayland layer so it stays interactive and cleanly above
    /// the fullscreen blurred App Drawer while open.
    pub fn set_drawer_layer_elevated(&self, elevated: bool) {
        if gtk4_layer_shell::is_supported() {
            self.window.set_layer(if elevated {
                Layer::Overlay
            } else {
                Layer::Top
            });
        }
        self.container.queue_draw();
    }

    /// Smoothly fades out or fades back in the Dock's glass pill background while keeping
    /// app icons floating cleanly above the fullscreen blurred App Drawer.
    pub fn set_drawer_bg_hidden(&self, hidden: bool) {
        if hidden {
            self.container.add_css_class("drawer-open");
        } else {
            self.container.remove_css_class("drawer-open");
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
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
                ipc.send(IpcRequest::SetOverview { open: false });
                ipc.send(IpcRequest::FocusWindow { window_id: win_id });
            });
            let rc_gesture = GestureClick::new();
            rc_gesture.set_button(3);
            {
                let cm = self.context_menu.clone();
                let cont = self.container.clone();
                let btn_c = btn.clone();
                rc_gesture.connect_pressed(move |gesture, _, x, _y| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    let (sx, sy) = dock_widget_screen_coords(&cm, &cont, &btn_c, x);
                    cm.open_window_menu(sx, sy, win_id);
                });
            }
            btn.add_controller(rc_gesture);
            self.running_box.append(&btn);
        }
    }
}

fn dock_widget_screen_coords(
    cm: &ContextMenuOverlay,
    dock_container: &GtkBox,
    widget: &impl IsA<gtk4::Widget>,
    local_x: f64,
) -> (i32, i32) {
    let (out_w, out_h) = cm.output_size();
    let dock_w = dock_container.width().max(200);
    let dock_left = (out_w - dock_w) / 2;
    let rel_x = widget
        .compute_bounds(dock_container)
        .map(|b| b.x() as f64)
        .unwrap_or(0.0);
    let screen_x = dock_left + (rel_x + local_x).round() as i32;
    let screen_y = out_h - 340;
    (screen_x, screen_y)
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

    let icon = load_app_icon(Some(icon_name), tooltip, 42);
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
