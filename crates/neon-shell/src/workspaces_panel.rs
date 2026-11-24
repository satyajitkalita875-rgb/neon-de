//! NEON DE Workspaces & Windows Panel (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Opened from the `NEON` icon in the Top Menu Bar. Displays all active desktops (up to 10),
//! provides one-click `+ Add Desktop` (and remove on extra desktops), shows open windows with
//! focus/restore/close controls, animates smoothly on open/close, and dismisses when clicking
//! anywhere outside or pressing `Escape`.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerKey, FlowBox,
    GestureClick, Image, Label, Orientation, ScrolledWindow, SelectionMode, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::ipc_bridge::IpcCommandSender;

const MAX_DESKTOPS: usize = 10;

type StateChangeCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

/// Floating Liquid Glass Workspaces & Open Windows popover toggled by the Top Bar `NEON` button.
#[derive(Clone)]
pub struct WorkspacesPanel {
    window: ApplicationWindow,
    surface: GtkBox,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    on_state_change: StateChangeCallback,
    count_label: Label,
    header_add_btn: Button,
    desktops_flow: FlowBox,
    windows_flow: FlowBox,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    ipc: IpcCommandSender,
}

impl WorkspacesPanel {
    pub fn build(app: &Application, ipc: IpcCommandSender) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Workspaces")
            .decorated(false)
            .build();
        window.add_css_class("neon-quicksettings-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-workspaces-panel"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let is_open = Rc::new(Cell::new(false));
        let anim_gen = Rc::new(Cell::new(0u64));
        let on_state_change: StateChangeCallback = Rc::new(RefCell::new(None));
        let last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>> = Rc::new(RefCell::new(None));

        let backdrop = GtkBox::new(Orientation::Vertical, 0);
        backdrop.add_css_class("neon-cc-backdrop");
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);

        let surface = GtkBox::new(Orientation::Vertical, 14);
        surface.add_css_class("neon-ws-panel-surface");
        surface.add_css_class("ws-closed");
        surface.set_halign(Align::Start);
        surface.set_valign(Align::Start);
        surface.set_margin_top(40);
        surface.set_margin_start(14);

        // Header: title, live desktop counter, and "+ Add Desktop" action button
        let header = GtkBox::new(Orientation::Horizontal, 10);
        header.set_valign(Align::Center);

        let title_col = GtkBox::new(Orientation::Vertical, 1);
        title_col.set_hexpand(true);
        let title_lbl = Label::new(Some("Workspaces"));
        title_lbl.add_css_class("neon-ws-panel-title");
        title_lbl.set_halign(Align::Start);

        let count_label = Label::new(Some("1 / 10 Desktops"));
        count_label.add_css_class("neon-cc-sub");
        count_label.set_halign(Align::Start);

        title_col.append(&title_lbl);
        title_col.append(&count_label);

        let header_add_btn = Button::new();
        header_add_btn.add_css_class("neon-ws-add-btn");
        let add_row = GtkBox::new(Orientation::Horizontal, 6);
        let plus_icon = Image::from_icon_name("list-add-symbolic");
        plus_icon.set_pixel_size(14);
        let add_lbl = Label::new(Some("Add Desktop"));
        add_row.append(&plus_icon);
        add_row.append(&add_lbl);
        header_add_btn.set_child(Some(&add_row));
        {
            let ipc_add = ipc.clone();
            header_add_btn.connect_clicked(move |_| {
                ipc_add.send(IpcRequest::AddWorkspace);
            });
        }

        header.append(&title_col);
        header.append(&header_add_btn);
        surface.append(&header);

        // Desktops grid (cleanly wraps up to 5 per row -> max 2 rows for 10 desktops)
        let desktops_flow = FlowBox::new();
        desktops_flow.set_selection_mode(SelectionMode::None);
        desktops_flow.set_homogeneous(true);
        desktops_flow.set_min_children_per_line(3);
        desktops_flow.set_max_children_per_line(5);
        desktops_flow.set_column_spacing(10);
        desktops_flow.set_row_spacing(10);
        desktops_flow.set_halign(Align::Start);
        surface.append(&desktops_flow);

        // Open Windows section
        let win_heading = Label::new(Some("Open Windows"));
        win_heading.add_css_class("neon-cc-sub");
        win_heading.set_halign(Align::Start);
        win_heading.set_margin_top(2);
        surface.append(&win_heading);

        let win_scroller = ScrolledWindow::builder()
            .max_content_height(260)
            .propagate_natural_height(true)
            .build();
        let windows_flow = FlowBox::new();
        windows_flow.set_selection_mode(SelectionMode::None);
        windows_flow.set_min_children_per_line(1);
        windows_flow.set_max_children_per_line(3);
        windows_flow.set_column_spacing(10);
        windows_flow.set_row_spacing(10);
        windows_flow.set_halign(Align::Start);
        win_scroller.set_child(Some(&windows_flow));
        surface.append(&win_scroller);

        backdrop.append(&surface);
        window.set_child(Some(&backdrop));
        window.set_visible(false);

        let panel = Self {
            window,
            surface,
            is_open,
            anim_gen,
            on_state_change,
            count_label,
            header_add_btn,
            desktops_flow,
            windows_flow,
            last_snapshot,
            ipc,
        };

        // Click anywhere outside closes smoothly
        {
            let outside_click = GestureClick::new();
            outside_click.set_button(0);
            let p = panel.clone();
            let surf = panel.surface.clone();
            outside_click.connect_pressed(move |gesture, _, x, y| {
                if !p.is_open.get() {
                    return;
                }
                let inside = gesture
                    .widget()
                    .and_then(|w| w.pick(x, y, gtk4::PickFlags::DEFAULT))
                    .is_some_and(|picked| {
                        picked == surf.clone().upcast::<gtk4::Widget>() || picked.is_ancestor(&surf)
                    });
                if !inside {
                    p.close();
                }
            });
            backdrop.add_controller(outside_click);
        }

        // Escape closes smoothly
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

        panel
    }

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
        if let Some(ref snap) = *self.last_snapshot.borrow() {
            self.rebuild(snap);
        }

        self.surface.remove_css_class("ws-open");
        self.surface.add_css_class("ws-closed");
        self.window.set_visible(true);
        self.window.present();

        let triggered = Rc::new(Cell::new(false));
        let ticks = Rc::new(Cell::new(0u8));
        {
            let gen_cell = Rc::clone(&self.anim_gen);
            let trig = Rc::clone(&triggered);
            let ticks_c = Rc::clone(&ticks);
            self.surface.add_tick_callback(move |widget, _| {
                if gen_cell.get() != epoch || trig.get() {
                    return glib::ControlFlow::Break;
                }
                let next = ticks_c.get().saturating_add(1);
                ticks_c.set(next);
                if next >= 2 {
                    trig.set(true);
                    widget.remove_css_class("ws-closed");
                    widget.add_css_class("ws-open");
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        }

        let surf = self.surface.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(55), move || {
            if gen_cell.get() == epoch && !triggered.get() {
                triggered.set(true);
                surf.remove_css_class("ws-closed");
                surf.add_css_class("ws-open");
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

        self.surface.remove_css_class("ws-open");
        self.surface.add_css_class("ws-closed");

        let win = self.window.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(220), move || {
            if gen_cell.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    win.set_keyboard_mode(KeyboardMode::None);
                }
                win.set_visible(false);
            }
        });
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
        if self.is_open.get() {
            self.rebuild(snapshot);
        }
    }

    fn notify_state_change(&self, open: bool) {
        if let Some(cb) = self.on_state_change.borrow().as_ref() {
            cb(open);
        }
    }

    fn rebuild(&self, snapshot: &DesktopSnapshot) {
        while let Some(child) = self.desktops_flow.first_child() {
            self.desktops_flow.remove(&child);
        }
        while let Some(child) = self.windows_flow.first_child() {
            self.windows_flow.remove(&child);
        }

        let total = snapshot.workspaces.len();
        self.count_label
            .set_text(&format!("{total} / {MAX_DESKTOPS} Desktops"));
        let can_add = total < MAX_DESKTOPS;
        self.header_add_btn.set_sensitive(can_add);
        self.header_add_btn.set_tooltip_text(Some(if can_add {
            "Add a new desktop (up to 10)"
        } else {
            "Maximum of 10 desktops reached"
        }));

        for ws in &snapshot.workspaces {
            let card_box = GtkBox::new(Orientation::Vertical, 6);
            card_box.add_css_class("neon-ws-desktop-card");
            if ws.active {
                card_box.add_css_class("active");
            }
            card_box.set_size_request(132, 74);

            let top_row = GtkBox::new(Orientation::Horizontal, 4);
            let title = Label::new(Some(&format!("Desktop {}", ws.index + 1)));
            title.add_css_class("neon-cc-title");
            title.set_hexpand(true);
            title.set_halign(Align::Start);
            top_row.append(&title);

            if total > 1 {
                let rm_btn = Button::with_label("×");
                rm_btn.add_css_class("neon-ws-remove-btn");
                rm_btn.set_tooltip_text(Some("Remove Desktop"));
                let idx = ws.index;
                let ipc_rm = self.ipc.clone();
                rm_btn.connect_clicked(move |_| {
                    ipc_rm.send(IpcRequest::RemoveWorkspace { index: idx });
                });
                top_row.append(&rm_btn);
            }

            let count_txt = match ws.window_ids.len() {
                0 => "Empty".to_string(),
                1 => "1 window".to_string(),
                n => format!("{n} windows"),
            };
            let sub = Label::new(Some(&count_txt));
            sub.add_css_class("neon-cc-sub");
            sub.set_halign(Align::Start);
            sub.set_vexpand(true);
            sub.set_valign(Align::End);

            card_box.append(&top_row);
            card_box.append(&sub);

            // Clicking the desktop card body switches cleanly to that desktop
            let click = GestureClick::new();
            let idx = ws.index;
            let ipc_sw = self.ipc.clone();
            let p_close = self.clone();
            click.connect_released(move |gesture, _, x, y| {
                let on_remove = gesture
                    .widget()
                    .and_then(|w| w.pick(x, y, gtk4::PickFlags::DEFAULT))
                    .and_then(|picked| picked.ancestor(Button::static_type()))
                    .is_some();
                if !on_remove {
                    ipc_sw.send(IpcRequest::SwitchWorkspace { index: idx });
                    p_close.close();
                }
            });
            card_box.add_controller(click);

            self.desktops_flow.insert(&card_box, -1);
        }

        // Inline "+" Add Desktop card right at the end of the desktops strip (up to 10)
        if can_add {
            let plus_card = Button::new();
            plus_card.add_css_class("neon-ws-plus-card");
            plus_card.set_size_request(132, 74);
            plus_card.set_tooltip_text(Some(&format!(
                "Create Desktop {} (max {MAX_DESKTOPS})",
                total + 1
            )));

            let col = GtkBox::new(Orientation::Vertical, 4);
            col.set_halign(Align::Center);
            col.set_valign(Align::Center);
            let ic = Image::from_icon_name("list-add-symbolic");
            ic.set_pixel_size(20);
            let lbl = Label::new(Some("New Desktop"));
            lbl.add_css_class("neon-cc-sub");
            col.append(&ic);
            col.append(&lbl);
            plus_card.set_child(Some(&col));

            let ipc_add = self.ipc.clone();
            plus_card.connect_clicked(move |_| {
                ipc_add.send(IpcRequest::AddWorkspace);
            });
            self.desktops_flow.insert(&plus_card, -1);
        }

        if snapshot.windows.is_empty() {
            let empty = Label::new(Some("No open windows"));
            empty.add_css_class("neon-cc-sub");
            empty.set_margin_top(4);
            empty.set_margin_bottom(4);
            self.windows_flow.insert(&empty, -1);
            return;
        }

        for win in &snapshot.windows {
            let ws_num = snapshot
                .workspaces
                .iter()
                .find(|w| w.id == win.workspace_id)
                .map(|w| w.index + 1)
                .unwrap_or(1);
            let title = win
                .title
                .as_deref()
                .or(win.app_id.as_deref())
                .unwrap_or("Untitled Window");

            let card = GtkBox::new(Orientation::Vertical, 8);
            card.add_css_class("neon-ws-window-card");
            if win.focused {
                card.add_css_class("focused");
            }
            card.set_size_request(210, -1);

            let top_row = GtkBox::new(Orientation::Horizontal, 8);
            let close_btn = Button::with_label("×");
            close_btn.add_css_class("neon-close-button");
            {
                let win_id = win.id;
                let ipc = self.ipc.clone();
                close_btn.connect_clicked(move |_| {
                    ipc.send(IpcRequest::CloseWindow { window_id: win_id });
                });
            }

            let title_lbl = Label::new(Some(title));
            title_lbl.add_css_class("neon-cc-title");
            title_lbl.set_hexpand(true);
            title_lbl.set_halign(Align::Start);
            title_lbl.set_max_width_chars(20);
            title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

            let desk_badge = Label::new(Some(&format!("D{ws_num}")));
            desk_badge.add_css_class("neon-ws-badge");

            top_row.append(&close_btn);
            top_row.append(&title_lbl);
            top_row.append(&desk_badge);

            let focus_btn = Button::with_label(if win.minimized {
                "Restore Window"
            } else {
                "Focus Window"
            });
            focus_btn.add_css_class("neon-pill-button");
            {
                let win_id = win.id;
                let ipc = self.ipc.clone();
                let p = self.clone();
                focus_btn.connect_clicked(move |_| {
                    ipc.send(IpcRequest::FocusWindow { window_id: win_id });
                    p.close();
                });
            }

            card.append(&top_row);
            card.append(&focus_btn);
            self.windows_flow.insert(&card, -1);
        }
    }
}
