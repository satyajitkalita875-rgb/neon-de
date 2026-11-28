//! NEON DE Dedicated Liquid Glass Notification Center (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Opens cleanly when clicking the Top Bar or Combined Bottom Taskbar Clock/Date button:
//! - Header: Current formatted date + interactive **Do Not Disturb** toggle + **Clear All**
//! - Compact Liquid Glass **Mini Calendar**
//! - Scrollable list of **clickable notification cards** with app icons, relative timestamps,
//!   Freedesktop action buttons, and instant dismiss (`×`)

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, Calendar, EventControllerKey,
    GestureClick, Image, Label, Orientation, PolicyType, ScrolledWindow, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{EffectsInfo, IpcRequest, WindowInfo};
use neon_system::notifications::NotificationStore;

use crate::{
    ipc_bridge::IpcCommandSender,
    notifications::{
        NotificationOverlay, activate_notification, build_notification_icon, format_relative_time,
        visible_notification_actions,
    },
};

const NC_WIDTH: i32 = 364;

type StateChangeListeners = Rc<RefCell<Vec<Box<dyn Fn(bool)>>>>;
type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// Live frosted-glass Notification Center anchored beneath the Top Bar clock (or above the
/// Combined Bottom Taskbar clock).
#[derive(Clone)]
pub struct NotificationCenterPanel {
    backdrop_window: ApplicationWindow,
    window: ApplicationWindow,
    ipc: IpcCommandSender,
    store: Arc<NotificationStore>,
    toast_overlay: NotificationOverlay,
    windows: Rc<RefCell<Vec<WindowInfo>>>,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    motion_enabled: Rc<Cell<bool>>,
    speed_scale: Rc<Cell<f64>>,
    anchor_bottom: Rc<Cell<bool>>,
    bottom_margin: Rc<Cell<i32>>,
    on_before_open: VoidCallback,
    on_state_listeners: StateChangeListeners,
    date_label: Label,
    dnd_btn: Button,
    dnd_icon: Image,
    dnd_label: Label,
    count_pill: Label,
    clear_btn: Button,
    calendar: Calendar,
    list_box: GtkBox,
    scroller: ScrolledWindow,
    empty_box: GtkBox,
}

impl NotificationCenterPanel {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        store: Arc<NotificationStore>,
        toast_overlay: NotificationOverlay,
        windows: Rc<RefCell<Vec<WindowInfo>>>,
    ) -> Self {
        let backdrop_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Notification Center Backdrop")
            .decorated(false)
            .build();
        backdrop_window.add_css_class("neon-notification-center-window");
        if gtk4_layer_shell::is_supported() {
            backdrop_window.init_layer_shell();
            backdrop_window.set_namespace(Some("neon-notification-center-backdrop"));
            backdrop_window.set_layer(Layer::Overlay);
            backdrop_window.set_anchor(Edge::Top, true);
            backdrop_window.set_anchor(Edge::Bottom, true);
            backdrop_window.set_anchor(Edge::Left, true);
            backdrop_window.set_anchor(Edge::Right, true);
            backdrop_window.set_exclusive_zone(-1);
            backdrop_window.set_keyboard_mode(KeyboardMode::None);
        }
        let backdrop_box = GtkBox::new(Orientation::Vertical, 0);
        backdrop_box.add_css_class("neon-cc-backdrop");
        backdrop_box.set_hexpand(true);
        backdrop_box.set_vexpand(true);
        backdrop_window.set_child(Some(&backdrop_box));
        backdrop_window.set_visible(false);

        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Notification Center")
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("neon-notification-center-window");
        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-notification-center"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, false);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, 38);
            window.set_margin(Edge::Right, 12);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let card = GtkBox::new(Orientation::Vertical, 12);
        card.add_css_class("neon-nc-panel-card");
        card.set_size_request(NC_WIDTH, -1);

        // =========================================================================
        // Header: Formatted Date + Do Not Disturb Toggle + Clear All
        // =========================================================================
        let header = GtkBox::new(Orientation::Horizontal, 8);
        header.set_valign(Align::Center);

        let date_label = Label::new(Some(&format_full_date_now()));
        date_label.add_css_class("neon-nc-date-title");
        date_label.set_halign(Align::Start);
        date_label.set_hexpand(true);
        date_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        let dnd_btn = Button::new();
        dnd_btn.add_css_class("neon-nc-dnd-btn");
        dnd_btn.set_tooltip_text(Some("Do Not Disturb (Silence popups)"));
        let dnd_inner = GtkBox::new(Orientation::Horizontal, 5);
        dnd_inner.set_valign(Align::Center);
        let dnd_icon = Image::from_icon_name("notifications-disabled-symbolic");
        dnd_icon.set_pixel_size(14);
        let dnd_label = Label::new(Some("DND"));
        dnd_label.add_css_class("neon-cc-small-lbl");
        dnd_inner.append(&dnd_icon);
        dnd_inner.append(&dnd_label);
        dnd_btn.set_child(Some(&dnd_inner));
        {
            let store_c = Arc::clone(&store);
            dnd_btn.connect_clicked(move |_| {
                let next = !store_c.is_do_not_disturb();
                store_c.set_do_not_disturb(next);
            });
        }

        let clear_btn = Button::with_label("Clear All");
        clear_btn.add_css_class("neon-cc-small-btn");
        clear_btn.set_visible(false);
        {
            let store_c = Arc::clone(&store);
            clear_btn.connect_clicked(move |_| {
                store_c.clear_all();
            });
        }

        header.append(&date_label);
        header.append(&dnd_btn);
        header.append(&clear_btn);
        card.append(&header);

        // =========================================================================
        // Compact Liquid Glass Mini Calendar
        // =========================================================================
        let cal_card = GtkBox::new(Orientation::Vertical, 4);
        cal_card.add_css_class("neon-nc-calendar-card");
        let calendar = Calendar::new();
        calendar.add_css_class("neon-nc-calendar");
        calendar.set_show_heading(true);
        calendar.set_show_day_names(true);
        calendar.set_show_week_numbers(false);
        cal_card.append(&calendar);
        card.append(&cal_card);

        // =========================================================================
        // Notifications Section Header + Scrollable Clickable List / Empty State
        // =========================================================================
        let notif_head = GtkBox::new(Orientation::Horizontal, 8);
        notif_head.set_valign(Align::Center);
        let notif_title = Label::new(Some("Notifications"));
        notif_title.add_css_class("neon-cc-title");
        notif_title.set_halign(Align::Start);

        let count_pill = Label::new(Some("0"));
        count_pill.add_css_class("neon-nc-count-pill");
        count_pill.set_visible(false);

        let head_spacer = GtkBox::new(Orientation::Horizontal, 0);
        head_spacer.set_hexpand(true);

        notif_head.append(&notif_title);
        notif_head.append(&count_pill);
        notif_head.append(&head_spacer);
        card.append(&notif_head);

        let list_box = GtkBox::new(Orientation::Vertical, 8);
        let scroller = ScrolledWindow::builder()
            .max_content_height(280)
            .propagate_natural_height(true)
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .build();
        scroller.add_css_class("neon-nc-scroller");
        scroller.set_child(Some(&list_box));
        card.append(&scroller);

        let empty_box = GtkBox::new(Orientation::Vertical, 6);
        empty_box.add_css_class("neon-nc-empty");
        empty_box.set_halign(Align::Center);
        empty_box.set_valign(Align::Center);
        let empty_icon = Image::from_icon_name("preferences-system-notifications-symbolic");
        empty_icon.set_pixel_size(28);
        empty_icon.add_css_class("neon-nc-empty-icon");
        let empty_title = Label::new(Some("No Notifications"));
        empty_title.add_css_class("neon-cc-title");
        let empty_sub = Label::new(Some("You're all caught up"));
        empty_sub.add_css_class("neon-cc-sub");
        empty_box.append(&empty_icon);
        empty_box.append(&empty_title);
        empty_box.append(&empty_sub);
        card.append(&empty_box);

        window.set_child(Some(&card));
        window.set_visible(false);

        let panel = Self {
            backdrop_window,
            window,
            ipc,
            store,
            toast_overlay,
            windows,
            is_open: Rc::new(Cell::new(false)),
            anim_gen: Rc::new(Cell::new(0)),
            motion_enabled: Rc::new(Cell::new(true)),
            speed_scale: Rc::new(Cell::new(1.0)),
            anchor_bottom: Rc::new(Cell::new(false)),
            bottom_margin: Rc::new(Cell::new(82)),
            on_before_open: Rc::new(RefCell::new(None)),
            on_state_listeners: Rc::new(RefCell::new(Vec::new())),
            date_label,
            dnd_btn,
            dnd_icon,
            dnd_label,
            count_pill,
            clear_btn,
            calendar,
            list_box,
            scroller,
            empty_box,
        };

        // Clicking outside dismisses cleanly.
        {
            let click = GestureClick::new();
            click.set_button(0);
            let p = panel.clone();
            click.connect_pressed(move |_, _, _, _| {
                p.close();
            });
            backdrop_box.add_controller(click);
        }

        // Escape dismisses cleanly from either window.
        for w in [&panel.window, &panel.backdrop_window] {
            let key_ctrl = EventControllerKey::new();
            let p = panel.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape && p.is_open.get() {
                    p.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            w.add_controller(key_ctrl);
        }

        panel.refresh_notifications();
        panel
    }

    pub fn set_on_before_open<F: Fn() + 'static>(&self, cb: F) {
        *self.on_before_open.borrow_mut() = Some(Box::new(cb));
    }

    pub fn add_on_state_change<F: Fn(bool) + 'static>(&self, cb: F) {
        self.on_state_listeners.borrow_mut().push(Box::new(cb));
    }

    pub fn update_effects(&self, effects: &EffectsInfo) {
        self.motion_enabled
            .set(effects.enabled && !effects.reduced_motion);
        self.speed_scale.set(effects.speed_scale.clamp(0.5, 2.5));
    }

    /// Switches panel anchor between Top-Right (Classic Top Bar) and Bottom-Right (Combined Taskbar).
    pub fn set_anchor_bottom(&self, bottom: bool, bottom_margin: i32) {
        let m = bottom_margin.clamp(48, 160);
        if self.anchor_bottom.get() == bottom && (!bottom || self.bottom_margin.get() == m) {
            return;
        }
        self.anchor_bottom.set(bottom);
        self.bottom_margin.set(m);
        if gtk4_layer_shell::is_supported() {
            if bottom {
                self.window.set_anchor(Edge::Top, false);
                self.window.set_anchor(Edge::Bottom, true);
                self.window.set_margin(Edge::Bottom, m);
            } else {
                self.window.set_anchor(Edge::Bottom, false);
                self.window.set_anchor(Edge::Top, true);
                self.window.set_margin(Edge::Top, 38);
            }
            self.window.set_anchor(Edge::Right, true);
            self.window.set_margin(Edge::Right, 12);
        }
    }

    pub fn toggle(&self) {
        if self.is_open.get() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(&self) {
        if let Some(cb) = self.on_before_open.borrow().as_ref() {
            cb();
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(true);
        self.notify_state_change(true);

        self.date_label.set_text(&format_full_date_now());
        if let Ok(now) = glib::DateTime::now_local() {
            self.calendar.select_day(&now);
        }

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::OnDemand);
        }

        self.toast_overlay.set_suppressed(true);
        self.refresh_notifications();

        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-notification-center".into(),
            opening: true,
        });

        // Map click-catcher backdrop FIRST, then the Notification Center card on top.
        self.backdrop_window.set_visible(true);
        self.backdrop_window.present();
        self.backdrop_window.queue_draw();
        self.window.set_visible(true);
        self.window.present();
    }

    pub fn close_immediate(&self) {
        let was_active = self.is_open.get() || self.window.is_visible();
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::None);
        }
        self.backdrop_window.set_visible(false);
        self.window.set_visible(false);
        self.toast_overlay.set_suppressed(false);
        if was_active {
            self.notify_state_change(false);
        }
    }

    pub fn close(&self) {
        if !self.is_open.get() && !self.window.is_visible() {
            return;
        }
        if !self.motion_enabled.get() {
            self.close_immediate();
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        self.notify_state_change(false);

        self.backdrop_window.set_visible(false);
        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-notification-center".into(),
            opening: false,
        });

        let speed = self.speed_scale.get().clamp(0.5, 2.5);
        let hide_ms = ((235.0 / speed).round() as u64).clamp(90, 500);

        let this = self.clone();
        let gen_c = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(hide_ms), move || {
            if gen_c.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    this.window.set_keyboard_mode(KeyboardMode::None);
                }
                this.window.set_visible(false);
                this.backdrop_window.set_visible(false);
                this.toast_overlay.set_suppressed(false);
            }
        });
    }

    /// Rebuilds the Notification Center list and DND badge from live [`NotificationStore`].
    pub fn refresh_notifications(&self) {
        let dnd = self.store.is_do_not_disturb();
        if dnd {
            self.dnd_btn.add_css_class("active");
            self.dnd_icon
                .set_icon_name(Some("notifications-disabled-symbolic"));
            self.dnd_label.set_text("DND On");
        } else {
            self.dnd_btn.remove_css_class("active");
            self.dnd_icon
                .set_icon_name(Some("preferences-system-notifications-symbolic"));
            self.dnd_label.set_text("DND");
        }

        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let history: Vec<_> = self
            .store
            .history()
            .into_iter()
            .filter(|n| !n.dismissed)
            .collect();
        let count = history.len();

        self.clear_btn.set_visible(count > 0);
        self.count_pill.set_visible(count > 0);
        self.count_pill.set_text(&count.to_string());
        self.scroller.set_visible(count > 0);
        self.empty_box.set_visible(count == 0);

        for item in history {
            let row_card = GtkBox::new(Orientation::Vertical, 6);
            row_card.add_css_class("neon-nc-item");

            // Top meta line: App icon + app name + relative timestamp + dismiss ×
            let meta = GtkBox::new(Orientation::Horizontal, 7);
            meta.set_valign(Align::Center);
            let icon = build_notification_icon(&item, 18);
            let app_lbl = Label::new(Some(if item.app_name.is_empty() {
                "System"
            } else {
                &item.app_name
            }));
            app_lbl.add_css_class("neon-qs-muted");
            app_lbl.set_hexpand(true);
            app_lbl.set_halign(Align::Start);
            app_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

            let time_lbl = Label::new(Some(&format_relative_time(item.received_unix_secs)));
            time_lbl.add_css_class("neon-nc-time");

            let dismiss_btn = Button::with_label("×");
            dismiss_btn.add_css_class("neon-close-button");
            {
                let store_c = Arc::clone(&self.store);
                let nc = self.clone();
                let id = item.id;
                dismiss_btn.connect_clicked(move |_| {
                    store_c.dismiss(id, 2);
                    nc.refresh_notifications();
                });
            }

            meta.append(&icon);
            meta.append(&app_lbl);
            meta.append(&time_lbl);
            meta.append(&dismiss_btn);
            row_card.append(&meta);

            // Clickable notification body button -> invokes D-Bus ActionInvoked + FocusWindow
            let body_btn = Button::new();
            body_btn.add_css_class("neon-nc-item-body-btn");
            let col = GtkBox::new(Orientation::Vertical, 2);
            let sum_lbl = Label::new(Some(&item.summary));
            sum_lbl.add_css_class("neon-qs-title");
            sum_lbl.set_halign(Align::Start);
            sum_lbl.set_xalign(0.0);
            sum_lbl.set_wrap(true);
            sum_lbl.set_max_width_chars(38);
            col.append(&sum_lbl);

            if !item.body.is_empty() {
                let body_lbl = Label::new(Some(&item.body));
                body_lbl.add_css_class("neon-qs-muted");
                body_lbl.set_halign(Align::Start);
                body_lbl.set_xalign(0.0);
                body_lbl.set_wrap(true);
                body_lbl.set_max_width_chars(42);
                body_lbl.set_lines(3);
                body_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                col.append(&body_lbl);
            }
            body_btn.set_child(Some(&col));
            {
                let item_c = item.clone();
                let store_c = Arc::clone(&self.store);
                let wins = Rc::clone(&self.windows);
                let ipc = self.ipc.clone();
                let nc = self.clone();
                body_btn.connect_clicked(move |_| {
                    activate_notification(&item_c, None, &store_c, &wins.borrow(), &ipc);
                    nc.close();
                });
            }
            row_card.append(&body_btn);

            let actions = visible_notification_actions(&item.actions);
            if !actions.is_empty() {
                let act_row = GtkBox::new(Orientation::Horizontal, 6);
                act_row.add_css_class("neon-toast-actions-row");
                for act in actions {
                    let abtn = Button::with_label(&act.label);
                    abtn.add_css_class("neon-notif-action-btn");
                    let item_c = item.clone();
                    let key_c = act.key.clone();
                    let store_c = Arc::clone(&self.store);
                    let wins = Rc::clone(&self.windows);
                    let ipc = self.ipc.clone();
                    let nc = self.clone();
                    abtn.connect_clicked(move |_| {
                        activate_notification(
                            &item_c,
                            Some(&key_c),
                            &store_c,
                            &wins.borrow(),
                            &ipc,
                        );
                        nc.close();
                    });
                    act_row.append(&abtn);
                }
                row_card.append(&act_row);
            }

            self.list_box.append(&row_card);
        }
    }

    fn notify_state_change(&self, open: bool) {
        for cb in self.on_state_listeners.borrow().iter() {
            cb(open);
        }
    }
}

fn format_full_date_now() -> String {
    glib::DateTime::now_local()
        .ok()
        .and_then(|dt| dt.format("%A, %b %-d").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Today".to_string())
}
