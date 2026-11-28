//! NEON DE Liquid Glass PolicyKit1 Authentication Dialog (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Presents a centered, Material 3 Expressive modal dialog whenever `polkitd` invokes
//! `BeginAuthentication` (e.g. App Center / `snap-store` updates, Flatpak/PackageKit installs,
//! `pkexec`, Firmware Updater, or privileged NEON Settings actions).

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, DropDown, EventControllerKey,
    Expander, Image, Label, Orientation, PasswordEntry, Spinner, StringList, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::EffectsInfo;
use neon_system::polkit::{PolkitAgentState, PolkitAuthRequest, PolkitEvent, PolkitIdentity};

type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// Native GTK4 Liquid Glass modal dialog for PolicyKit1 privilege elevation.
#[derive(Clone)]
pub struct PolkitDialog {
    window: ApplicationWindow,
    backdrop: GtkBox,
    card: GtkBox,
    emblem_icon: Image,
    message_label: Label,
    user_single_pill: GtkBox,
    user_single_label: Label,
    user_dropdown: DropDown,
    password_entry: PasswordEntry,
    status_row: GtkBox,
    status_spinner: Spinner,
    status_label: Label,
    error_box: GtkBox,
    error_label: Label,
    details_expander: Expander,
    details_box: GtkBox,
    cancel_btn: Button,
    auth_btn: Button,
    state: Arc<PolkitAgentState>,
    active_request: Rc<RefCell<Option<PolkitAuthRequest>>>,
    queued_requests: Rc<RefCell<VecDeque<PolkitAuthRequest>>>,
    verifying: Rc<Cell<bool>>,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    motion_enabled: Rc<Cell<bool>>,
    speed_scale: Rc<Cell<f64>>,
    on_before_open: VoidCallback,
}

impl PolkitDialog {
    pub fn build(app: &Application, state: Arc<PolkitAgentState>) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Authentication Required")
            .decorated(false)
            .build();
        window.add_css_class("neon-polkit-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-polkit-dialog"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let backdrop = GtkBox::new(Orientation::Vertical, 0);
        backdrop.add_css_class("neon-polkit-backdrop");
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);
        backdrop.set_halign(Align::Fill);
        backdrop.set_valign(Align::Fill);

        let card = GtkBox::new(Orientation::Vertical, 14);
        card.add_css_class("neon-polkit-card");
        card.add_css_class("pk-closed");
        card.set_halign(Align::Center);
        card.set_valign(Align::Center);
        card.set_hexpand(false);
        card.set_vexpand(false);
        card.set_width_request(420);

        let center_wrap = GtkBox::new(Orientation::Vertical, 0);
        center_wrap.set_hexpand(true);
        center_wrap.set_vexpand(true);
        center_wrap.set_halign(Align::Center);
        center_wrap.set_valign(Align::Center);

        // Header emblem + title + Polkit action message
        let header_box = GtkBox::new(Orientation::Vertical, 8);
        header_box.set_halign(Align::Fill);

        let emblem = GtkBox::new(Orientation::Vertical, 0);
        emblem.add_css_class("neon-polkit-emblem");
        emblem.set_halign(Align::Center);
        emblem.set_valign(Align::Center);
        emblem.set_size_request(54, 54);

        let emblem_icon = Image::from_icon_name("dialog-password-symbolic");
        emblem_icon.set_pixel_size(26);
        emblem_icon.set_halign(Align::Center);
        emblem_icon.set_valign(Align::Center);
        emblem_icon.set_hexpand(true);
        emblem_icon.set_vexpand(true);
        emblem.append(&emblem_icon);

        let title_label = Label::new(Some("Authentication Required"));
        title_label.add_css_class("neon-polkit-title");
        title_label.set_halign(Align::Center);

        let message_label = Label::new(Some(
            "Enter your administrator password to authorize this action.",
        ));
        message_label.add_css_class("neon-polkit-message");
        message_label.set_wrap(true);
        message_label.set_max_width_chars(44);
        message_label.set_justify(gtk4::Justification::Center);
        message_label.set_halign(Align::Center);

        header_box.append(&emblem);
        header_box.append(&title_label);
        header_box.append(&message_label);
        card.append(&header_box);

        // Identity section (sleek user pill when single user, DropDown when multiple admins)
        let user_single_pill = GtkBox::new(Orientation::Horizontal, 8);
        user_single_pill.add_css_class("neon-polkit-user-pill");
        user_single_pill.set_halign(Align::Center);
        let user_icon = Image::from_icon_name("avatar-default-symbolic");
        user_icon.set_pixel_size(16);
        let user_single_label = Label::new(Some("Administrator"));
        user_single_label.add_css_class("neon-polkit-user-label");
        user_single_pill.append(&user_icon);
        user_single_pill.append(&user_single_label);
        card.append(&user_single_pill);

        let user_dropdown = DropDown::from_strings(&["Administrator"]);
        user_dropdown.add_css_class("neon-polkit-dropdown");
        user_dropdown.set_halign(Align::Fill);
        user_dropdown.set_visible(false);
        card.append(&user_dropdown);

        // Password field
        let password_entry = PasswordEntry::new();
        password_entry.add_css_class("neon-polkit-entry");
        password_entry.set_show_peek_icon(true);
        password_entry.set_placeholder_text(Some("Password"));
        password_entry.set_hexpand(true);
        card.append(&password_entry);

        // Inline authenticating status row
        let status_row = GtkBox::new(Orientation::Horizontal, 8);
        status_row.add_css_class("neon-polkit-status");
        status_row.set_halign(Align::Center);
        status_row.set_visible(false);
        let status_spinner = Spinner::new();
        status_spinner.set_size_request(16, 16);
        let status_label = Label::new(Some("Authenticating…"));
        status_label.add_css_class("neon-polkit-status-label");
        status_row.append(&status_spinner);
        status_row.append(&status_label);
        card.append(&status_row);

        // Inline error banner
        let error_box = GtkBox::new(Orientation::Horizontal, 8);
        error_box.add_css_class("neon-polkit-error");
        error_box.set_halign(Align::Fill);
        error_box.set_visible(false);
        let error_icon = Image::from_icon_name("dialog-warning-symbolic");
        error_icon.set_pixel_size(15);
        let error_label = Label::new(None);
        error_label.add_css_class("neon-polkit-error-label");
        error_label.set_wrap(true);
        error_label.set_max_width_chars(42);
        error_label.set_xalign(0.0);
        error_label.set_hexpand(true);
        error_box.append(&error_icon);
        error_box.append(&error_label);
        card.append(&error_box);

        // Collapsible action details
        let details_box = GtkBox::new(Orientation::Vertical, 4);
        details_box.add_css_class("neon-polkit-details-box");
        let details_expander = Expander::new(Some("Details"));
        details_expander.add_css_class("neon-polkit-expander");
        details_expander.set_child(Some(&details_box));
        details_expander.set_expanded(false);
        card.append(&details_expander);

        // Action buttons row
        let btn_row = GtkBox::new(Orientation::Horizontal, 10);
        btn_row.set_homogeneous(true);
        btn_row.set_halign(Align::Fill);
        btn_row.set_margin_top(4);

        let cancel_btn = Button::with_label("Cancel");
        cancel_btn.add_css_class("neon-polkit-btn");

        let auth_btn = Button::with_label("Authenticate");
        auth_btn.add_css_class("neon-polkit-btn");
        auth_btn.add_css_class("primary");
        auth_btn.set_sensitive(false);

        btn_row.append(&cancel_btn);
        btn_row.append(&auth_btn);
        card.append(&btn_row);

        center_wrap.append(&card);
        backdrop.append(&center_wrap);
        window.set_child(Some(&backdrop));
        window.set_visible(false);

        let dialog = Self {
            window,
            backdrop,
            card,
            emblem_icon,
            message_label,
            user_single_pill,
            user_single_label,
            user_dropdown,
            password_entry,
            status_row,
            status_spinner,
            status_label,
            error_box,
            error_label,
            details_expander,
            details_box,
            cancel_btn,
            auth_btn,
            state,
            active_request: Rc::new(RefCell::new(None)),
            queued_requests: Rc::new(RefCell::new(VecDeque::new())),
            verifying: Rc::new(Cell::new(false)),
            is_open: Rc::new(Cell::new(false)),
            anim_gen: Rc::new(Cell::new(0)),
            motion_enabled: Rc::new(Cell::new(true)),
            speed_scale: Rc::new(Cell::new(1.0)),
            on_before_open: Rc::new(RefCell::new(None)),
        };

        // Enable Authenticate button as soon as non-empty text is entered, and clear stale errors.
        {
            let dlg = dialog.clone();
            dialog.password_entry.connect_changed(move |entry| {
                let has_text = !entry.text().is_empty();
                dlg.auth_btn.set_sensitive(has_text && !dlg.verifying.get());
                if has_text && dlg.error_box.is_visible() {
                    dlg.error_box.set_visible(false);
                }
            });
        }

        // Pressing Enter submits authentication immediately.
        {
            let dlg = dialog.clone();
            dialog.password_entry.connect_activate(move |_| {
                dlg.submit_current();
            });
        }

        // Authenticate button click.
        {
            let dlg = dialog.clone();
            dialog.auth_btn.connect_clicked(move |_| {
                dlg.submit_current();
            });
        }

        // Cancel button click.
        {
            let dlg = dialog.clone();
            dialog.cancel_btn.connect_clicked(move |_| {
                dlg.cancel_current();
            });
        }

        // Escape cancels immediately.
        {
            let key_ctrl = EventControllerKey::new();
            let dlg = dialog.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape && dlg.is_open.get() {
                    dlg.cancel_current();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            dialog.window.add_controller(key_ctrl);
        }

        dialog
    }

    pub fn set_on_before_open<F: Fn() + 'static>(&self, cb: F) {
        *self.on_before_open.borrow_mut() = Some(Box::new(cb));
    }

    pub fn update_effects(&self, effects: &EffectsInfo) {
        self.motion_enabled
            .set(effects.enabled && !effects.reduced_motion);
        self.speed_scale.set(effects.speed_scale.clamp(0.5, 2.5));
    }

    /// Processes a single event received from [`PolkitAgentState`].
    pub fn handle_event(&self, event: PolkitEvent) {
        match event {
            PolkitEvent::Begin(req) => {
                if self.active_request.borrow().is_some() {
                    let mut q = self.queued_requests.borrow_mut();
                    q.retain(|r| r.cookie != req.cookie);
                    q.push_back(req);
                } else {
                    self.present_request(req);
                }
            }
            PolkitEvent::Cancel { cookie } => {
                self.queued_requests
                    .borrow_mut()
                    .retain(|r| r.cookie != cookie);
                let is_active = self
                    .active_request
                    .borrow()
                    .as_ref()
                    .is_some_and(|r| r.cookie == cookie);
                if is_active {
                    self.finish_active_and_advance();
                }
            }
            PolkitEvent::AuthSucceeded { cookie } => {
                let is_active = self
                    .active_request
                    .borrow()
                    .as_ref()
                    .is_some_and(|r| r.cookie == cookie);
                if is_active {
                    self.finish_active_and_advance();
                }
            }
            PolkitEvent::AuthFailed { cookie, message } => {
                let is_active = self
                    .active_request
                    .borrow()
                    .as_ref()
                    .is_some_and(|r| r.cookie == cookie);
                if is_active {
                    self.set_verifying(false);
                    self.password_entry.set_text("");
                    self.show_error(&message);
                    self.password_entry.grab_focus();
                }
            }
        }
    }

    /// Opens a mock PolicyKit dialog for automated visual verification (`--open-polkit-test`)
    /// without registering with `polkitd`.
    pub fn open_for_test(&self) {
        let user = neon_system::polkit::current_user_identity().unwrap_or(PolkitIdentity {
            uid: 1000,
            username: "satyajit".into(),
            full_name: "Satyajit".into(),
        });
        self.present_request(PolkitAuthRequest {
            action_id: "io.snapcraft.snapd.manage".into(),
            message: "Authentication is required to refresh snaps".into(),
            icon_name: "system-software-update".into(),
            details: vec![
                ("program".into(), "/usr/bin/snap".into()),
                ("vendor".into(), "Canonical Snapd".into()),
            ],
            cookie: "preview-cookie".into(),
            identities: vec![user],
            cancelled: false,
        });
    }

    fn present_request(&self, req: PolkitAuthRequest) {
        if let Some(cb) = self.on_before_open.borrow().as_ref() {
            cb();
        }

        // Update icon: keep crisp symbolic lock icon unless a valid themed icon is provided.
        let icon_name = if !req.icon_name.trim().is_empty() {
            let trimmed = req.icon_name.trim();
            if let Some(display) = gdk::Display::default() {
                let theme = gtk4::IconTheme::for_display(&display);
                if theme.has_icon(trimmed) {
                    trimmed
                } else {
                    "dialog-password-symbolic"
                }
            } else {
                "dialog-password-symbolic"
            }
        } else {
            "dialog-password-symbolic"
        };
        self.emblem_icon.set_icon_name(Some(icon_name));

        let msg = if req.message.trim().is_empty() {
            "Authentication is required to perform this administrative action."
        } else {
            req.message.trim()
        };
        self.message_label.set_text(msg);

        // Populate identity selector
        if req.identities.len() <= 1 {
            let label = req
                .identities
                .first()
                .map(PolkitIdentity::display_label)
                .unwrap_or_else(|| "Administrator".to_string());
            self.user_single_label.set_text(&label);
            self.user_single_pill.set_visible(true);
            self.user_dropdown.set_visible(false);
        } else {
            let labels: Vec<String> = req
                .identities
                .iter()
                .map(PolkitIdentity::display_label)
                .collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let model = StringList::new(&refs);
            self.user_dropdown.set_model(Some(&model));
            self.user_dropdown.set_selected(0);
            self.user_single_pill.set_visible(false);
            self.user_dropdown.set_visible(true);
        }

        // Populate collapsible Details section
        while let Some(child) = self.details_box.first_child() {
            self.details_box.remove(&child);
        }
        let mut has_details = false;
        if !req.action_id.trim().is_empty() {
            self.append_detail_row("Action", &req.action_id);
            has_details = true;
        }
        for (k, v) in &req.details {
            self.append_detail_row(k, v);
            has_details = true;
        }
        self.details_expander.set_visible(has_details);
        self.details_expander.set_expanded(false);

        *self.active_request.borrow_mut() = Some(req);
        self.set_verifying(false);
        self.error_box.set_visible(false);
        self.password_entry.set_text("");
        self.auth_btn.set_sensitive(false);

        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(true);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        }

        self.backdrop.remove_css_class("pk-closing");
        self.card.remove_css_class("pk-open");
        self.card.remove_css_class("pk-closing");
        if self.motion_enabled.get() {
            self.card.add_css_class("pk-closed");
        } else {
            self.card.remove_css_class("pk-closed");
            self.card.add_css_class("pk-open");
        }

        self.window.set_visible(true);
        self.window.present();

        let entry = self.password_entry.clone();
        let card = self.card.clone();
        let gen_c = Rc::clone(&self.anim_gen);
        let motion_on = self.motion_enabled.get();
        // Advance from pk-closed -> pk-open on the next frame (with a tiny timer fallback
        // so first-ever layer-shell map is guaranteed to open even if frame clock pauses).
        let open_now = move || {
            if gen_c.get() == epoch {
                if motion_on {
                    card.remove_css_class("pk-closed");
                    card.add_css_class("pk-open");
                }
                entry.grab_focus();
            }
        };
        let open_cb = Rc::new(open_now);
        let open_tick = Rc::clone(&open_cb);
        self.card.add_tick_callback(move |_, _| {
            open_tick();
            glib::ControlFlow::Break
        });
        let open_timer = Rc::clone(&open_cb);
        glib::timeout_add_local_once(Duration::from_millis(32), move || {
            open_timer();
        });
    }

    fn append_detail_row(&self, key: &str, val: &str) {
        let row = GtkBox::new(Orientation::Horizontal, 8);
        let k = Label::new(Some(&format!("{key}:")));
        k.add_css_class("neon-polkit-detail-key");
        k.set_halign(Align::Start);
        k.set_valign(Align::Start);

        let v = Label::new(Some(val));
        v.add_css_class("neon-polkit-detail-val");
        v.set_halign(Align::Start);
        v.set_xalign(0.0);
        v.set_wrap(true);
        v.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        v.set_hexpand(true);

        row.append(&k);
        row.append(&v);
        self.details_box.append(&row);
    }

    fn selected_username(&self, req: &PolkitAuthRequest) -> String {
        if req.identities.len() > 1 {
            let idx = self.user_dropdown.selected() as usize;
            if let Some(id) = req.identities.get(idx) {
                return id.username.clone();
            }
        }
        req.identities
            .first()
            .map(|id| id.username.clone())
            .or_else(|| neon_system::polkit::current_user_identity().map(|id| id.username))
            .unwrap_or_else(|| "root".to_string())
    }

    fn submit_current(&self) {
        if self.verifying.get() {
            return;
        }
        let password = self.password_entry.text().to_string();
        if password.is_empty() {
            return;
        }
        let Some(req) = self.active_request.borrow().clone() else {
            return;
        };
        let username = self.selected_username(&req);

        self.error_box.set_visible(false);
        self.set_verifying(true);

        if req.cookie == "preview-cookie" {
            // Test preview mode: show feedback without contacting system PAM.
            let dlg = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(350), move || {
                dlg.set_verifying(false);
                dlg.show_error("Preview mode — live authentication is disabled.");
            });
            return;
        }

        self.state.submit_password(req.cookie, username, password);
    }

    fn cancel_current(&self) {
        let cookie = self
            .active_request
            .borrow()
            .as_ref()
            .map(|r| r.cookie.clone());
        if let Some(cookie) = cookie {
            self.state.cancel_by_user(&cookie);
        }
        self.finish_active_and_advance();
    }

    fn finish_active_and_advance(&self) {
        *self.active_request.borrow_mut() = None;
        self.set_verifying(false);
        self.password_entry.set_text("");

        if let Some(next) = self.queued_requests.borrow_mut().pop_front() {
            self.present_request(next);
        } else {
            self.close();
        }
    }

    fn set_verifying(&self, active: bool) {
        self.verifying.set(active);
        self.password_entry.set_sensitive(!active);
        self.user_dropdown.set_sensitive(!active);
        self.auth_btn
            .set_sensitive(!active && !self.password_entry.text().is_empty());
        self.status_row.set_visible(active);
        if active {
            self.status_label.set_text("Authenticating…");
            self.status_spinner.start();
        } else {
            self.status_spinner.stop();
        }
    }

    fn show_error(&self, msg: &str) {
        self.error_label.set_text(msg);
        self.error_box.set_visible(true);
    }

    fn close_immediate(&self) {
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.is_open.set(false);
        self.status_spinner.stop();
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::None);
        }
        self.backdrop.remove_css_class("pk-closing");
        self.card.remove_css_class("pk-open");
        self.card.remove_css_class("pk-closing");
        self.card.add_css_class("pk-closed");
        self.window.set_visible(false);
    }

    fn close(&self) {
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
        self.status_spinner.stop();

        self.backdrop.add_css_class("pk-closing");
        self.card.remove_css_class("pk-open");
        self.card.add_css_class("pk-closing");

        let speed = self.speed_scale.get().clamp(0.5, 2.5);
        let hide_ms = ((210.0 / speed).round() as u64).clamp(80, 440);

        let win = self.window.clone();
        let backdrop = self.backdrop.clone();
        let card = self.card.clone();
        let gen_c = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(hide_ms), move || {
            if gen_c.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    win.set_keyboard_mode(KeyboardMode::None);
                }
                backdrop.remove_css_class("pk-closing");
                card.remove_css_class("pk-closing");
                card.add_css_class("pk-closed");
                win.set_visible(false);
            }
        });
    }
}
