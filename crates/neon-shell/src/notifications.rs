//! NEON DE Desktop Notification Toast Overlay (`zwlr_layer_shell_v1` `Layer::Overlay` Top+Right).

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation, glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_system::notifications::{DesktopNotification, NotificationStore, NotificationUrgency};

/// Live notification toast stack rendered in the top-right corner.
#[derive(Clone)]
pub struct NotificationOverlay {
    window: ApplicationWindow,
    container: GtkBox,
    store: Arc<NotificationStore>,
    visible_toasts: Rc<RefCell<Vec<DesktopNotification>>>,
    suppressed: Rc<RefCell<bool>>,
}

impl NotificationOverlay {
    pub fn build(app: &Application, store: Arc<NotificationStore>) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Notifications")
            .decorated(false)
            .resizable(false)
            .default_width(330)
            .build();
        window.add_css_class("neon-toast-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-notifications"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, 46);
            window.set_margin(Edge::Right, 14);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let container = GtkBox::new(Orientation::Vertical, 8);
        window.set_child(Some(&container));
        window.set_visible(false);

        Self {
            window,
            container,
            store,
            visible_toasts: Rc::new(RefCell::new(Vec::new())),
            suppressed: Rc::new(RefCell::new(false)),
        }
    }

    /// Suppresses floating toasts while Quick Settings / Notification Center is open.
    pub fn set_suppressed(&self, suppressed: bool) {
        *self.suppressed.borrow_mut() = suppressed;
        self.rebuild();
    }

    pub fn push_toast(&self, item: DesktopNotification) {
        if self.store.is_do_not_disturb() && item.urgency != NotificationUrgency::Critical {
            return;
        }
        let id = item.id;
        let timeout_ms = item.expire_timeout_ms;

        {
            let mut list = self.visible_toasts.borrow_mut();
            list.retain(|n| n.id != id);
            list.insert(0, item);
            if list.len() > 4 {
                list.truncate(4);
            }
        }
        self.rebuild();

        // Auto-expire non-critical notifications cleanly after their timeout.
        if timeout_ms > 0 {
            let this = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(timeout_ms as u64), move || {
                this.remove_toast(id);
            });
        }
    }

    pub fn remove_toast(&self, id: u32) {
        let changed = {
            let mut list = self.visible_toasts.borrow_mut();
            let before = list.len();
            list.retain(|n| n.id != id);
            list.len() != before
        };
        if changed {
            self.rebuild();
        }
    }

    pub fn clear_toasts(&self) {
        self.visible_toasts.borrow_mut().clear();
        self.rebuild();
    }

    fn rebuild(&self) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }

        let suppressed = *self.suppressed.borrow();
        let toasts = self.visible_toasts.borrow();
        if suppressed || toasts.is_empty() {
            self.window.set_visible(false);
            return;
        }

        for item in toasts.iter() {
            let card = GtkBox::new(Orientation::Vertical, 4);
            card.add_css_class("neon-toast-card");
            if item.urgency == NotificationUrgency::Critical {
                card.add_css_class("critical");
            }

            let header = GtkBox::new(Orientation::Horizontal, 8);
            let app_lbl = Label::new(Some(if item.app_name.is_empty() {
                "System"
            } else {
                &item.app_name
            }));
            app_lbl.add_css_class("neon-qs-muted");
            app_lbl.set_hexpand(true);
            app_lbl.set_halign(Align::Start);

            let close_btn = Button::with_label("×");
            close_btn.add_css_class("neon-close-button");
            {
                let store = Arc::clone(&self.store);
                let overlay = self.clone();
                let id = item.id;
                close_btn.connect_clicked(move |_| {
                    store.dismiss(id, 2);
                    overlay.remove_toast(id);
                });
            }
            header.append(&app_lbl);
            header.append(&close_btn);
            card.append(&header);

            let summary = Label::new(Some(&item.summary));
            summary.add_css_class("neon-qs-title");
            summary.set_halign(Align::Start);
            summary.set_wrap(true);
            summary.set_max_width_chars(38);
            card.append(&summary);

            if !item.body.is_empty() {
                let body = Label::new(Some(&item.body));
                body.add_css_class("neon-qs-muted");
                body.set_halign(Align::Start);
                body.set_wrap(true);
                body.set_max_width_chars(42);
                card.append(&body);
            }

            self.container.append(&card);
        }

        self.window.set_visible(true);
        self.window.present();
    }
}
