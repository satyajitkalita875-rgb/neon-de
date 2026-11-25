//! NEON DE macOS-Inspired Glass Floating Dock (`zwlr_layer_shell_v1` `Layer::Top`).
//!
//! Supports live-configurable:
//! - **Position**: Bottom (default), Left, or Right edge — adapts layer anchor, orientation,
//!   edge sensor, and exclusive zone live without restart.
//! - **Icon Size**: Tiny (24px), Compact (30px), Small (36px), Medium (42px), Large (52px) (22–64px).
//! - **Magnification**: Toggleable smooth hover magnification (`dock-magnify`).
//! - **Live Pin / Unpin**: Dynamically syncs pinned favorites from `dock_pinned_apps`.
//! - **Visibility Modes**: Always Visible, Auto-Hide, Smart Hide (Intelligent), and Fullscreen Removal.
//! - **Unified macOS Context Menus**: Rich per-app right-click menu (Open, New Window, XDG App
//!   Actions, Pin/Unpin, Launch at Login, Show All Windows, Move to Workspace, Hide, Quit) and
//!   empty dock menu (Position, Size, Magnification, Auto Hide, Dock Settings).

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerMotion,
    GestureClick, Orientation, Separator, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::{
    context_menu::ContextMenuOverlay,
    desktop_apps::{
        DesktopApp, load_app_icon, resolve_window_desktop_app, select_favorites_with_pins,
    },
    ipc_bridge::IpcCommandSender,
    panel::launch_sibling_binary,
};

type DockWindowSig = (u64, Option<String>, Option<String>, bool, bool);

#[derive(Default)]
struct DockHideState {
    auto_hide: bool,
    smart_hide: bool,
    has_fullscreen: bool,
    overview_open: bool,
    window_overlaps_dock: bool,
    hovered_dock: bool,
    hovered_sensor: bool,
    currently_shown: bool,
    last_exclusive: i32,
    position: String,
    icon_size: u32,
    anim_gen: u64,
}

/// Floating glass dock displaying icon-only pinned apps, running window icons with glowing
/// indicator dots, Settings, and the Applications Grid trigger.
pub struct Dock {
    window: ApplicationWindow,
    sensor_window: ApplicationWindow,
    sensor_strip: GtkBox,
    wrapper: GtkBox,
    container: GtkBox,
    pinned_box: GtkBox,
    running_box: GtkBox,
    separator: Separator,
    sep2: Separator,
    settings_btn: Button,
    grid_btn: Button,
    all_apps: Rc<RefCell<Vec<DesktopApp>>>,
    last_windows: RefCell<Vec<DockWindowSig>>,
    last_position: RefCell<String>,
    last_icon_size: Cell<u32>,
    last_magnification: Cell<bool>,
    last_pinned_ids: RefCell<Vec<String>>,
    hide_state: Rc<RefCell<DockHideState>>,
    context_menu: ContextMenuOverlay,
    ipc: IpcCommandSender,
}

impl Dock {
    pub fn build(
        app: &Application,
        favorites: &[DesktopApp],
        all_apps: Rc<RefCell<Vec<DesktopApp>>>,
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
            window.set_exclusive_zone(68);
        }

        // Invisible 3px screen-edge pointer sensor for Auto-Hide & Smart-Hide reveal.
        let sensor_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dock Sensor")
            .decorated(false)
            .resizable(false)
            .default_height(3)
            .build();
        sensor_window.add_css_class("neon-edge-sensor-window");
        if gtk4_layer_shell::is_supported() {
            sensor_window.init_layer_shell();
            sensor_window.set_namespace(Some("neon-dock-sensor"));
            sensor_window.set_layer(Layer::Top);
            sensor_window.set_anchor(Edge::Bottom, true);
            sensor_window.set_anchor(Edge::Left, true);
            sensor_window.set_anchor(Edge::Right, true);
            sensor_window.set_keyboard_mode(KeyboardMode::None);
            sensor_window.set_exclusive_zone(0);
        }
        let sensor_strip = GtkBox::new(Orientation::Horizontal, 0);
        sensor_strip.add_css_class("neon-edge-sensor-strip");
        sensor_strip.set_size_request(-1, 3);
        sensor_strip.set_hexpand(true);
        sensor_window.set_child(Some(&sensor_strip));
        sensor_window.set_visible(false);

        let wrapper = GtkBox::new(Orientation::Horizontal, 0);
        wrapper.add_css_class("neon-dock-wrapper");

        let container = GtkBox::new(Orientation::Horizontal, 4);
        container.add_css_class("neon-dock-container");
        container.add_css_class("dock-magnify");

        let pinned_box = GtkBox::new(Orientation::Horizontal, 4);
        container.append(&pinned_box);

        let separator = Separator::new(Orientation::Vertical);
        separator.add_css_class("neon-dock-separator");
        separator.set_visible(false);
        container.append(&separator);

        let running_box = GtkBox::new(Orientation::Horizontal, 4);
        container.append(&running_box);

        let sep2 = Separator::new(Orientation::Vertical);
        sep2.add_css_class("neon-dock-separator");
        container.append(&sep2);

        let settings_btn = make_icon_dock_button(
            Some("preferences-system"),
            "settings",
            "NEON DE Settings",
            false,
            false,
            36,
        );
        {
            let ipc_set = ipc.clone();
            settings_btn.connect_clicked(move |_| {
                launch_sibling_binary("neon-settings");
                ipc_set.send(IpcRequest::SetOverview { open: false });
            });
        }
        container.append(&settings_btn);

        let grid_btn = make_icon_dock_button(
            Some("view-app-grid-symbolic"),
            "view-app-grid-symbolic",
            "Applications & Overview (Super+Space)",
            false,
            false,
            36,
        );
        {
            let ipc_g = ipc.clone();
            grid_btn.connect_clicked(move |_| {
                ipc_g.send(IpcRequest::ToggleOverview);
            });
        }
        container.append(&grid_btn);

        let hide_state = Rc::new(RefCell::new(DockHideState {
            currently_shown: true,
            last_exclusive: 68,
            position: "bottom".into(),
            icon_size: 36,
            ..Default::default()
        }));

        // Right-clicking empty dock background opens the Dock Settings/Position/Size/Magnification menu.
        let dock_rc = GestureClick::new();
        dock_rc.set_button(3);
        {
            let cm = context_menu.clone();
            let cont = container.clone();
            let st_rc = Rc::clone(&hide_state);
            dock_rc.connect_pressed(move |_, _, x, y| {
                let pos = st_rc.borrow().position.clone();
                let (sx, sy) = dock_point_screen_coords(&cm, &cont, &pos, x, y);
                cm.open_dock_menu(sx, sy);
            });
        }
        container.add_controller(dock_rc);

        wrapper.append(&container);
        window.set_child(Some(&wrapper));
        window.present();

        // Pointer hover tracking on Dock & edge sensor
        {
            let motion = EventControllerMotion::new();
            let st_in = Rc::clone(&hide_state);
            let win_in = window.clone();
            let sen_in = sensor_window.clone();
            let cont_in = container.clone();
            motion.connect_enter(move |_, _, _| {
                st_in.borrow_mut().hovered_dock = true;
                sync_dock_visibility(&st_in, &win_in, &sen_in, &cont_in, false);
            });

            let st_out = Rc::clone(&hide_state);
            let win_out = window.clone();
            let sen_out = sensor_window.clone();
            let cont_out = container.clone();
            motion.connect_leave(move |_| {
                st_out.borrow_mut().hovered_dock = false;
                sync_dock_visibility(&st_out, &win_out, &sen_out, &cont_out, true);
            });
            wrapper.add_controller(motion);
        }
        {
            let motion = EventControllerMotion::new();
            let st_in = Rc::clone(&hide_state);
            let win_in = window.clone();
            let sen_in = sensor_window.clone();
            let cont_in = container.clone();
            motion.connect_enter(move |_, _, _| {
                st_in.borrow_mut().hovered_sensor = true;
                sync_dock_visibility(&st_in, &win_in, &sen_in, &cont_in, false);
            });

            let st_out = Rc::clone(&hide_state);
            let win_out = window.clone();
            let sen_out = sensor_window.clone();
            let cont_out = container.clone();
            motion.connect_leave(move |_| {
                st_out.borrow_mut().hovered_sensor = false;
                sync_dock_visibility(&st_out, &win_out, &sen_out, &cont_out, true);
            });
            sensor_strip.add_controller(motion);
        }

        let dock = Self {
            window,
            sensor_window,
            sensor_strip,
            wrapper,
            container,
            pinned_box,
            running_box,
            separator,
            sep2,
            settings_btn,
            grid_btn,
            all_apps,
            last_windows: RefCell::new(Vec::new()),
            last_position: RefCell::new("bottom".into()),
            last_icon_size: Cell::new(36),
            last_magnification: Cell::new(true),
            last_pinned_ids: RefCell::new(Vec::new()),
            hide_state,
            context_menu,
            ipc,
        };
        dock.rebuild_pinned_buttons(favorites, 36);
        dock
    }

    fn rebuild_pinned_buttons(&self, favorites: &[DesktopApp], icon_size: i32) {
        while let Some(child) = self.pinned_box.first_child() {
            self.pinned_box.remove(&child);
        }
        for fav in favorites {
            let btn = make_icon_dock_button(
                fav.icon.as_deref(),
                &fav.name,
                &fav.name,
                false,
                false,
                icon_size,
            );
            let app_clone = fav.clone();
            let ipc_fav = self.ipc.clone();
            btn.connect_clicked(move |_| {
                app_clone.launch();
                ipc_fav.send(IpcRequest::SetOverview { open: false });
            });
            let rc_gesture = GestureClick::new();
            rc_gesture.set_button(3);
            {
                let cm = self.context_menu.clone();
                let app_rc = fav.clone();
                let cont = self.container.clone();
                let btn_c = btn.clone();
                let st_rc = Rc::clone(&self.hide_state);
                rc_gesture.connect_pressed(move |gesture, _, x, y| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    let pos = st_rc.borrow().position.clone();
                    let (sx, sy) = dock_widget_screen_coords(&cm, &cont, &btn_c, &pos, x, y);
                    cm.open_dock_app_menu(sx, sy, Some(app_rc.clone()), None);
                });
            }
            btn.add_controller(rc_gesture);
            self.pinned_box.append(&btn);
        }
    }

    fn apply_position(&self, pos: &str) {
        let vertical = pos == "left" || pos == "right";
        let orient = if vertical {
            Orientation::Vertical
        } else {
            Orientation::Horizontal
        };
        let sep_orient = if vertical {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        };

        self.wrapper.set_orientation(orient);
        self.container.set_orientation(orient);
        self.pinned_box.set_orientation(orient);
        self.running_box.set_orientation(orient);
        self.separator.set_orientation(sep_orient);
        self.sep2.set_orientation(sep_orient);

        if vertical {
            self.wrapper.add_css_class("dock-vertical");
            self.container.add_css_class("dock-vertical");
        } else {
            self.wrapper.remove_css_class("dock-vertical");
            self.container.remove_css_class("dock-vertical");
        }

        if gtk4_layer_shell::is_supported() {
            self.window.set_anchor(Edge::Bottom, pos == "bottom");
            self.window.set_anchor(Edge::Left, pos == "left");
            self.window.set_anchor(Edge::Right, pos == "right");
            self.window.set_anchor(Edge::Top, false);

            self.sensor_window.set_anchor(Edge::Bottom, pos == "bottom" || vertical);
            self.sensor_window.set_anchor(Edge::Top, vertical);
            self.sensor_window.set_anchor(Edge::Left, pos == "left" || !vertical);
            self.sensor_window.set_anchor(Edge::Right, pos == "right" || !vertical);
            if vertical {
                self.sensor_strip.set_size_request(3, -1);
                self.sensor_strip.set_hexpand(false);
                self.sensor_strip.set_vexpand(true);
            } else {
                self.sensor_strip.set_size_request(-1, 3);
                self.sensor_strip.set_hexpand(true);
                self.sensor_strip.set_vexpand(false);
            }
        }
        self.window.set_default_size(1, 1);
    }

    /// Forces running window icons on the Dock to be re-resolved on the next IPC snapshot
    /// (called after `.desktop` entries or icon themes refresh dynamically).
    pub fn invalidate_window_icons(&self) {
        self.last_windows.borrow_mut().clear();
    }

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

    pub fn set_drawer_bg_hidden(&self, hidden: bool) {
        if hidden {
            self.container.add_css_class("drawer-open");
        } else {
            self.container.remove_css_class("drawer-open");
        }
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        // 1. Live Dock Position, Size, Magnification, and Pinned Apps sync
        let desired_pos = snapshot.appearance.dock_position.to_lowercase();
        let norm_pos = match desired_pos.as_str() {
            "left" => "left",
            "right" => "right",
            _ => "bottom",
        };
        if self.last_position.borrow().as_str() != norm_pos {
            *self.last_position.borrow_mut() = norm_pos.to_string();
            self.hide_state.borrow_mut().position = norm_pos.to_string();
            self.apply_position(norm_pos);
        }

        let desired_mag = snapshot.appearance.dock_magnification;
        if self.last_magnification.get() != desired_mag {
            self.last_magnification.set(desired_mag);
            if desired_mag {
                self.container.add_css_class("dock-magnify");
            } else {
                self.container.remove_css_class("dock-magnify");
            }
        }

        let desired_size = snapshot.appearance.dock_icon_size.clamp(22, 64);
        let size_changed = self.last_icon_size.get() != desired_size;
        let pins_changed = *self.last_pinned_ids.borrow() != snapshot.appearance.dock_pinned_apps;
        if size_changed || pins_changed {
            self.last_icon_size.set(desired_size);
            self.hide_state.borrow_mut().icon_size = desired_size;
            *self.last_pinned_ids.borrow_mut() = snapshot.appearance.dock_pinned_apps.clone();
            if desired_size <= 32 {
                self.container.add_css_class("dock-compact");
                self.container.set_spacing(2);
                self.pinned_box.set_spacing(2);
                self.running_box.set_spacing(2);
            } else {
                self.container.remove_css_class("dock-compact");
                self.container.set_spacing(4);
                self.pinned_box.set_spacing(4);
                self.running_box.set_spacing(4);
            }
            let favs = {
                let guard = self.all_apps.borrow();
                select_favorites_with_pins(&guard, &snapshot.appearance.dock_pinned_apps)
            };
            self.rebuild_pinned_buttons(&favs, desired_size as i32);
            if size_changed {
                update_dock_button_icon(
                    &self.settings_btn,
                    Some("preferences-system"),
                    "settings",
                    desired_size as i32,
                );
                update_dock_button_icon(
                    &self.grid_btn,
                    Some("view-app-grid-symbolic"),
                    "view-app-grid-symbolic",
                    desired_size as i32,
                );
                self.last_windows.borrow_mut().clear();
            }
            self.window.set_default_size(1, 1);
        }

        // 2. Fullscreen, Auto-Hide, and Smart-Hide overlap state
        let active_ws_ids: HashSet<u64> = snapshot
            .workspaces
            .iter()
            .find(|ws| ws.active)
            .map(|ws| ws.window_ids.iter().copied().collect())
            .unwrap_or_default();

        let (out_w, out_h) = snapshot
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| snapshot.outputs.first())
            .map(|o| (o.width.max(800), o.height.max(600)))
            .unwrap_or((1920, 1080));

        let dock_w = self.container.width().max(320);
        let dock_left = (out_w - dock_w) / 2;
        let dock_right = dock_left + dock_w;
        let dock_band = (desired_size as i32) + if desired_size <= 32 { 22 } else { 30 };
        let dock_top_zone = out_h - dock_band;

        let mut has_fullscreen = false;
        let mut window_overlaps_dock = false;

        for w in &snapshot.windows {
            if w.minimized {
                continue;
            }
            if !active_ws_ids.is_empty() && !active_ws_ids.contains(&w.id) {
                continue;
            }
            if w.fullscreen {
                has_fullscreen = true;
                window_overlaps_dock = true;
            } else if w.maximized {
                window_overlaps_dock = true;
            } else {
                let overlaps = match norm_pos {
                    "left" => w.x < dock_band,
                    "right" => (w.x + w.width) > (out_w - dock_band),
                    _ => w.x < dock_right && (w.x + w.width) > dock_left && (w.y + w.height) > dock_top_zone,
                };
                if overlaps {
                    window_overlaps_dock = true;
                }
            }
        }

        {
            let mut st = self.hide_state.borrow_mut();
            st.auto_hide = snapshot.appearance.dock_auto_hide;
            st.smart_hide = snapshot.appearance.dock_smart_hide;
            st.has_fullscreen = has_fullscreen;
            st.overview_open = snapshot.overview_open;
            st.window_overlaps_dock = window_overlaps_dock;
            st.icon_size = desired_size;
        }
        sync_dock_visibility(
            &self.hide_state,
            &self.window,
            &self.sensor_window,
            &self.container,
            false,
        );

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

        let apps_guard = self.all_apps.borrow();
        let icon_px = self.last_icon_size.get() as i32;
        let mut anchor_buttons: Vec<(u64, Button)> = Vec::with_capacity(snapshot.windows.len());
        for win in &snapshot.windows {
            let display_name = win
                .title
                .as_deref()
                .or(win.app_id.as_deref())
                .unwrap_or("Window");
            let icon_spec =
                resolve_window_icon(win.app_id.as_deref(), win.title.as_deref(), &apps_guard);
            let icon_hint = win.app_id.as_deref().unwrap_or("");

            let btn = make_icon_dock_button(
                icon_spec.as_deref(),
                icon_hint,
                display_name,
                true,
                win.focused,
                icon_px,
            );
            if win.minimized {
                btn.set_opacity(0.55);
            }

            let win_id = win.id;
            let was_focused = win.focused;
            let was_minimized = win.minimized;
            {
                let ipc = self.ipc.clone();
                let cm = self.context_menu.clone();
                let cont = self.container.clone();
                let btn_c = btn.clone();
                let st_rc = Rc::clone(&self.hide_state);
                btn.connect_clicked(move |_| {
                    let pos = st_rc.borrow().position.clone();
                    let (ax, ay) = dock_button_center_screen_coords(&cm, &cont, &btn_c, &pos);
                    ipc.send(IpcRequest::SetWindowDockAnchor {
                        window_id: win_id,
                        x: ax,
                        y: ay,
                    });
                    if was_focused && !was_minimized {
                        ipc.send(IpcRequest::MinimizeWindow { window_id: win_id });
                    } else {
                        ipc.send(IpcRequest::SetOverview { open: false });
                        ipc.send(IpcRequest::FocusWindow { window_id: win_id });
                    }
                });
            }
            let rc_gesture = GestureClick::new();
            rc_gesture.set_button(3);
            {
                let cm = self.context_menu.clone();
                let ipc = self.ipc.clone();
                let cont = self.container.clone();
                let btn_c = btn.clone();
                let st_rc = Rc::clone(&self.hide_state);
                rc_gesture.connect_pressed(move |gesture, _, x, y| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    let pos = st_rc.borrow().position.clone();
                    let (ax, ay) = dock_button_center_screen_coords(&cm, &cont, &btn_c, &pos);
                    ipc.send(IpcRequest::SetWindowDockAnchor {
                        window_id: win_id,
                        x: ax,
                        y: ay,
                    });
                    let (sx, sy) = dock_widget_screen_coords(&cm, &cont, &btn_c, &pos, x, y);
                    cm.open_dock_app_menu(sx, sy, None, Some(win_id));
                });
            }
            btn.add_controller(rc_gesture);
            self.running_box.append(&btn);
            anchor_buttons.push((win_id, btn));
        }

        // Report each running window's exact Dock icon screen center once GTK finishes layout
        // so titlebar Minimize/Close and Super+H/Super+Q swoop accurately to/from that icon.
        if !anchor_buttons.is_empty() {
            let cm = self.context_menu.clone();
            let cont = self.container.clone();
            let st_rc = Rc::clone(&self.hide_state);
            let ipc = self.ipc.clone();
            glib::timeout_add_local_once(Duration::from_millis(60), move || {
                let pos = st_rc.borrow().position.clone();
                for (wid, btn) in anchor_buttons {
                    if btn.parent().is_some() {
                        let (ax, ay) = dock_button_center_screen_coords(&cm, &cont, &btn, &pos);
                        ipc.send(IpcRequest::SetWindowDockAnchor {
                            window_id: wid,
                            x: ax,
                            y: ay,
                        });
                    }
                }
            });
        }
    }
}

fn sync_dock_visibility(
    state_rc: &Rc<RefCell<DockHideState>>,
    window: &ApplicationWindow,
    sensor_window: &ApplicationWindow,
    container: &GtkBox,
    delay_hide: bool,
) {
    let (desired_exclusive, should_show, enable_sensor, immediate_fullscreen) = {
        let st = state_rc.borrow();
        let ex_sz = (st.icon_size as i32) + if st.icon_size <= 32 { 24 } else { 32 };
        if st.has_fullscreen && !st.overview_open {
            (0, false, false, true)
        } else if !st.auto_hide && !st.smart_hide {
            (ex_sz, true, false, false)
        } else {
            let show = st.overview_open
                || st.hovered_dock
                || st.hovered_sensor
                || (st.smart_hide && !st.auto_hide && !st.window_overlaps_dock);
            (0, show, !show, false)
        }
    };

    if gtk4_layer_shell::is_supported() {
        let prev_ex = state_rc.borrow().last_exclusive;
        if prev_ex != desired_exclusive {
            state_rc.borrow_mut().last_exclusive = desired_exclusive;
            window.set_exclusive_zone(desired_exclusive);
        }
    }

    if immediate_fullscreen {
        let mut st = state_rc.borrow_mut();
        st.anim_gen = st.anim_gen.wrapping_add(1);
        st.currently_shown = false;
        drop(st);
        sensor_window.set_visible(false);
        container.add_css_class("dock-hidden");
        window.set_visible(false);
        return;
    }

    if should_show {
        let was_shown = {
            let mut st = state_rc.borrow_mut();
            st.anim_gen = st.anim_gen.wrapping_add(1);
            let prev = st.currently_shown;
            st.currently_shown = true;
            prev
        };
        sensor_window.set_visible(false);
        if !was_shown || !window.is_visible() {
            container.add_css_class("dock-hidden");
            window.set_visible(true);
            window.present();
            let cont_c = container.clone();
            let st_c = Rc::clone(state_rc);
            let epoch = state_rc.borrow().anim_gen;
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                if st_c.borrow().anim_gen == epoch {
                    cont_c.remove_css_class("dock-hidden");
                }
            });
        } else {
            container.remove_css_class("dock-hidden");
        }
        return;
    }

    let epoch = {
        let mut st = state_rc.borrow_mut();
        if !st.currently_shown && !window.is_visible() {
            if sensor_window.is_visible() != enable_sensor {
                sensor_window.set_visible(enable_sensor);
                if enable_sensor {
                    sensor_window.present();
                }
            }
            return;
        }
        st.anim_gen = st.anim_gen.wrapping_add(1);
        st.anim_gen
    };

    let leave_wait = if delay_hide { 380 } else { 0 };
    let st_c = Rc::clone(state_rc);
    let win_c = window.clone();
    let sen_c = sensor_window.clone();
    let cont_c = container.clone();

    glib::timeout_add_local_once(Duration::from_millis(leave_wait), move || {
        if st_c.borrow().anim_gen != epoch {
            return;
        }
        cont_c.add_css_class("dock-hidden");
        if enable_sensor {
            sen_c.set_visible(true);
            sen_c.present();
        }
        let st_done = Rc::clone(&st_c);
        let win_done = win_c.clone();
        glib::timeout_add_local_once(Duration::from_millis(210), move || {
            if st_done.borrow().anim_gen == epoch {
                st_done.borrow_mut().currently_shown = false;
                win_done.set_visible(false);
            }
        });
    });
}

fn dock_point_screen_coords(
    cm: &ContextMenuOverlay,
    dock_container: &GtkBox,
    pos: &str,
    local_x: f64,
    local_y: f64,
) -> (i32, i32) {
    let (out_w, out_h) = cm.output_size();
    let dock_w = dock_container.width().max(80);
    let dock_h = dock_container.height().max(64);
    match pos {
        "left" => {
            let dock_top = (out_h - dock_h) / 2;
            (dock_w + 18, dock_top + local_y.round() as i32)
        }
        "right" => {
            let dock_top = (out_h - dock_h) / 2;
            (out_w - dock_w - 18, dock_top + local_y.round() as i32)
        }
        _ => {
            let dock_left = (out_w - dock_w) / 2;
            (dock_left + local_x.round() as i32, out_h - dock_h - 14)
        }
    }
}

fn dock_widget_screen_coords(
    cm: &ContextMenuOverlay,
    dock_container: &GtkBox,
    widget: &impl IsA<gtk4::Widget>,
    pos: &str,
    local_x: f64,
    local_y: f64,
) -> (i32, i32) {
    let (rel_x, rel_y) = widget
        .compute_bounds(dock_container)
        .map(|b| (b.x() as f64, b.y() as f64))
        .unwrap_or((0.0, 0.0));
    dock_point_screen_coords(cm, dock_container, pos, rel_x + local_x, rel_y + local_y)
}

fn dock_button_center_screen_coords(
    cm: &ContextMenuOverlay,
    dock_container: &GtkBox,
    widget: &impl IsA<gtk4::Widget>,
    pos: &str,
) -> (i32, i32) {
    let (out_w, out_h) = cm.output_size();
    let dock_w = dock_container.width().max(80);
    let dock_h = dock_container.height().max(48);
    let (cx, cy) = widget
        .compute_bounds(dock_container)
        .map(|b| {
            (
                b.x() as f64 + b.width() as f64 * 0.5,
                b.y() as f64 + b.height() as f64 * 0.5,
            )
        })
        .unwrap_or((dock_w as f64 * 0.5, dock_h as f64 * 0.5));
    match pos {
        "left" => {
            let dock_top = (out_h - dock_h) / 2;
            ((dock_w / 2).max(20), dock_top + cy.round() as i32)
        }
        "right" => {
            let dock_top = (out_h - dock_h) / 2;
            (out_w - (dock_w / 2).max(20), dock_top + cy.round() as i32)
        }
        _ => {
            let dock_left = (out_w - dock_w) / 2;
            (dock_left + cx.round() as i32, out_h - (dock_h / 2).max(20))
        }
    }
}

fn make_icon_dock_button(
    icon_spec: Option<&str>,
    icon_hint: &str,
    tooltip: &str,
    running: bool,
    focused: bool,
    pixel_size: i32,
) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-dock-item");
    if focused {
        btn.add_css_class("focused");
    }
    btn.set_tooltip_text(Some(tooltip));

    let vbox = GtkBox::new(Orientation::Vertical, 0);
    vbox.set_halign(Align::Center);
    vbox.set_valign(Align::Center);

    let icon = load_app_icon(icon_spec, icon_hint, pixel_size);
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

fn update_dock_button_icon(btn: &Button, icon_spec: Option<&str>, icon_hint: &str, pixel_size: i32) {
    if let Some(child) = btn.child().and_then(|w| w.downcast::<GtkBox>().ok())
        && let Some(first) = child.first_child()
    {
        let next_icon = load_app_icon(icon_spec, icon_hint, pixel_size);
        child.insert_child_after(&next_icon, Some(&first));
        child.remove(&first);
    }
}

fn resolve_window_icon(
    app_id: Option<&str>,
    title: Option<&str>,
    all_apps: &[DesktopApp],
) -> Option<String> {
    let needle_id = app_id.unwrap_or("").trim().to_lowercase();
    let needle_title = title.unwrap_or("").trim().to_lowercase();

    if needle_id == "org.neonde.settings"
        || needle_id.contains("neonde.settings")
        || (needle_id.is_empty() && needle_title == "neon de settings")
    {
        return Some("preferences-system".to_string());
    }

    if let Some(app) = resolve_window_desktop_app(app_id, title, all_apps)
        && let Some(ref icon) = app.icon
        && !icon.trim().is_empty()
    {
        return Some(icon.clone());
    }

    if matches!(
        needle_id.as_str(),
        "alacritty"
            | "kitty"
            | "foot"
            | "footclient"
            | "wezterm"
            | "org.wezfurlong.wezterm"
            | "com.mitchellh.ghostty"
            | "ghostty"
            | "weston-terminal"
            | "org.gnome.terminal"
            | "org.gnome.console"
            | "kgx"
            | "konsole"
            | "xterm"
    ) {
        return Some("utilities-terminal".to_string());
    }

    None
}
