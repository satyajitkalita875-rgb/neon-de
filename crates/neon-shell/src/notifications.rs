//! NEON DE Clickable Desktop Notification Toasts & Shared Activation Logic.
//!
//! Features:
//! - Standard Freedesktop Notifications v1.2 `ActionInvoked` + `NotificationClosed` D-Bus signaling.
//! - Intelligent window matching & focus (`FocusWindow`) when a notification or action is clicked.
//! - Shared relative timestamp, themed icon lookup, and toast overlay (`Layer::Overlay`)
//!   that adapts between Top-Right (Classic mode) and Bottom-Right (Combined fallback mode),
//!   and cleanly yields to the Dynamic Island when enabled.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, EventControllerMotion, Image,
    Label, Orientation, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{IpcRequest, WindowInfo};
use neon_system::notifications::{
    DesktopNotification, NotificationAction, NotificationStore, NotificationUrgency,
};

use crate::ipc_bridge::IpcCommandSender;

/// Invokes the appropriate D-Bus action (or dismisses if none) and focuses any matching
/// application window so clicking a notification brings the user straight to the app.
pub fn activate_notification(
    item: &DesktopNotification,
    explicit_action: Option<&str>,
    store: &Arc<NotificationStore>,
    windows: &[WindowInfo],
    ipc: &IpcCommandSender,
) {
    let key_to_invoke = explicit_action.or_else(|| {
        item.actions
            .iter()
            .find(|a| a.key == "default")
            .map(|a| a.key.as_str())
    });

    if let Some(key) = key_to_invoke {
        let _ = store.invoke_action(item.id, key);
    } else {
        let _ = store.dismiss(item.id, 2);
    }

    if let Some(win_id) = match_notification_window(item, windows) {
        ipc.send(IpcRequest::SetOverview { open: false });
        ipc.send(IpcRequest::FocusWindow { window_id: win_id });
    }
}

/// Filters non-`"default"` action buttons suitable for rendering as interactive pills.
pub fn visible_notification_actions(actions: &[NotificationAction]) -> Vec<&NotificationAction> {
    actions
        .iter()
        .filter(|a| a.key != "default" && !a.label.trim().is_empty())
        .take(3)
        .collect()
}

/// Matches a notification's `desktop_entry`, `app_name`, and `app_icon` against open windows
/// and returns the highest-scoring window ID if a confident match exists.
pub fn match_notification_window(item: &DesktopNotification, windows: &[WindowInfo]) -> Option<u64> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(ref de) = item.desktop_entry {
        let cleaned = de.trim().trim_end_matches(".desktop").to_ascii_lowercase();
        if cleaned.len() >= 2 {
            candidates.push(cleaned);
        }
    }
    let app_lower = item.app_name.trim().to_ascii_lowercase();
    if app_lower.len() >= 2 {
        candidates.push(app_lower.clone());
        let compact: String = app_lower.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if compact.len() >= 3 && compact != app_lower {
            candidates.push(compact);
        }
    }
    let icon_clean = item
        .app_icon
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".desktop")
        .trim_end_matches(".png")
        .trim_end_matches(".svg")
        .to_ascii_lowercase();
    if icon_clean.len() >= 3
        && !icon_clean.starts_with("dialog-")
        && !icon_clean.starts_with("preferences-")
        && !icon_clean.starts_with("notification")
    {
        candidates.push(icon_clean);
    }

    if candidates.is_empty() {
        return None;
    }

    let mut best: Option<(i32, u64)> = None;
    for win in windows {
        let aid = win
            .app_id
            .as_deref()
            .unwrap_or("")
            .trim_end_matches(".desktop")
            .to_ascii_lowercase();
        let aid_compact: String = aid.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let title = win.title.as_deref().unwrap_or("").to_ascii_lowercase();

        let mut score = 0;
        for cand in &candidates {
            if !aid.is_empty() {
                if aid == *cand || aid_compact == *cand || aid.ends_with(&format!(".{cand}")) {
                    score = score.max(100);
                } else if aid.contains(cand.as_str()) || (!aid_compact.is_empty() && cand.contains(&aid_compact)) {
                    score = score.max(65);
                }
            }
            if !title.is_empty() && cand.len() >= 4 && title.contains(cand.as_str()) {
                score = score.max(40);
            }
        }
        if score > 0 {
            if !win.minimized {
                score += 5;
            }
            if best.is_none_or(|(bscore, _)| score > bscore) {
                best = Some((score, win.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Formats a human-readable relative time label (`Just now`, `2m ago`, `3h ago`, etc.).
pub fn format_relative_time(received_unix_secs: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(received_unix_secs);
    format_relative_delta(now.saturating_sub(received_unix_secs))
}

fn format_relative_delta(delta_secs: u64) -> String {
    if delta_secs < 45 {
        "Just now".into()
    } else if delta_secs < 3600 {
        format!("{}m ago", (delta_secs / 60).max(1))
    } else if delta_secs < 86400 {
        format!("{}h ago", delta_secs / 3600)
    } else {
        format!("{}d ago", delta_secs / 86400)
    }
}

/// Builds a crisp icon for a notification from `app_icon` path, icon theme, or `desktop_entry`.
pub fn build_notification_icon(item: &DesktopNotification, pixel_size: i32) -> Image {
    let raw_icon = item.app_icon.trim();
    if !raw_icon.is_empty() {
        let local_path = raw_icon.strip_prefix("file://").unwrap_or(raw_icon);
        if local_path.starts_with('/') && Path::new(local_path).is_file() {
            let img = Image::from_file(local_path);
            img.set_pixel_size(pixel_size);
            return img;
        }
    }

    let theme = gdk::Display::default().map(|d| gtk4::IconTheme::for_display(&d));
    let mut candidates: Vec<&str> = Vec::new();
    if !raw_icon.is_empty() {
        candidates.push(raw_icon);
    }
    if let Some(ref de) = item.desktop_entry {
        let trimmed = de.trim().trim_end_matches(".desktop");
        if !trimmed.is_empty() {
            candidates.push(trimmed);
        }
    }
    if let Some(ref th) = theme {
        for cand in candidates {
            if th.has_icon(cand) {
                let img = Image::from_icon_name(cand);
                img.set_pixel_size(pixel_size);
                return img;
            }
        }
    }

    let fallback = match item.urgency {
        NotificationUrgency::Critical => "dialog-warning-symbolic",
        _ => "preferences-system-notifications-symbolic",
    };
    let img = Image::from_icon_name(fallback);
    img.set_pixel_size(pixel_size);
    img
}

/// Live notification toast stack rendered cleanly with Material 3 Expressive motion.
#[derive(Clone)]
pub struct NotificationOverlay {
    window: ApplicationWindow,
    container: GtkBox,
    store: Arc<NotificationStore>,
    ipc: IpcCommandSender,
    windows: Rc<RefCell<Vec<WindowInfo>>>,
    visible_toasts: Rc<RefCell<Vec<DesktopNotification>>>,
    seen_ids: Rc<RefCell<HashSet<u32>>>,
    cards_by_id: Rc<RefCell<HashMap<u32, GtkBox>>>,
    suppressed: Rc<Cell<bool>>,
    island_routed: Rc<Cell<bool>>,
    hovered: Rc<Cell<bool>>,
    anchor_bottom: Rc<Cell<bool>>,
    bottom_margin: Rc<Cell<i32>>,
}

impl NotificationOverlay {
    pub fn build(
        app: &Application,
        store: Arc<NotificationStore>,
        ipc: IpcCommandSender,
        windows: Rc<RefCell<Vec<WindowInfo>>>,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Notifications")
            .decorated(false)
            .resizable(false)
            .default_width(340)
            .build();
        window.add_css_class("neon-toast-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-notifications"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, false);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, 46);
            window.set_margin(Edge::Right, 14);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let container = GtkBox::new(Orientation::Vertical, 8);
        window.set_child(Some(&container));
        window.set_visible(false);

        let hovered = Rc::new(Cell::new(false));
        let overlay = Self {
            window,
            container,
            store,
            ipc,
            windows,
            visible_toasts: Rc::new(RefCell::new(Vec::new())),
            seen_ids: Rc::new(RefCell::new(HashSet::new())),
            cards_by_id: Rc::new(RefCell::new(HashMap::new())),
            suppressed: Rc::new(Cell::new(false)),
            island_routed: Rc::new(Cell::new(false)),
            hovered: Rc::clone(&hovered),
            anchor_bottom: Rc::new(Cell::new(false)),
            bottom_margin: Rc::new(Cell::new(82)),
        };

        let motion = EventControllerMotion::new();
        {
            let hov_in = Rc::clone(&hovered);
            motion.connect_enter(move |_, _, _| {
                hov_in.set(true);
            });
            let hov_out = hovered;
            motion.connect_leave(move |_| {
                hov_out.set(false);
            });
        }
        overlay.container.add_controller(motion);
        overlay
    }

    /// Suppresses floating toasts while Quick Settings / Notification Center is open.
    pub fn set_suppressed(&self, suppressed: bool) {
        if self.suppressed.get() == suppressed {
            return;
        }
        self.suppressed.set(suppressed);
        self.rebuild();
    }

    /// Hides floating corner toasts when notifications are routed into the Dynamic Island.
    pub fn set_island_routed(&self, routed: bool) {
        if self.island_routed.get() == routed {
            return;
        }
        self.island_routed.set(routed);
        self.rebuild();
    }

    /// Switches anchor edge between Top-Right (Classic mode) and Bottom-Right (Combined fallback).
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
                self.window.set_margin(Edge::Top, 46);
            }
            self.window.set_anchor(Edge::Right, true);
            self.window.set_margin(Edge::Right, 14);
        }
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

        // Auto-expire banner popup cleanly after timeout while keeping the entry in Notification
        // Center until explicitly clicked or dismissed.
        if timeout_ms > 0 {
            let this = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(timeout_ms as u64), move || {
                this.expire_or_defer_toast(id);
            });
        }
    }

    fn expire_or_defer_toast(&self, id: u32) {
        if self.hovered.get() {
            let this = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(900), move || {
                this.expire_or_defer_toast(id);
            });
            return;
        }
        self.remove_toast(id);
    }

    pub fn remove_toast(&self, id: u32) {
        if let Some(card) = self.cards_by_id.borrow().get(&id).cloned()
            && !card.has_css_class("toast-exit")
        {
            card.add_css_class("toast-exit");
            let this = self.clone();
            glib::timeout_add_local_once(Duration::from_millis(190), move || {
                this.remove_toast_immediate(id);
            });
            return;
        }
        self.remove_toast_immediate(id);
    }

    fn remove_toast_immediate(&self, id: u32) {
        self.seen_ids.borrow_mut().remove(&id);
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
        self.seen_ids.borrow_mut().clear();
        self.rebuild();
    }

    fn rebuild(&self) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        self.cards_by_id.borrow_mut().clear();

        let suppressed = self.suppressed.get() || self.island_routed.get();
        let toasts = self.visible_toasts.borrow();
        if suppressed || toasts.is_empty() {
            self.window.set_visible(false);
            return;
        }

        let mut seen = self.seen_ids.borrow_mut();
        for item in toasts.iter() {
            let is_new = seen.insert(item.id);
            let card = GtkBox::new(Orientation::Vertical, 6);
            card.add_css_class("neon-toast-card");
            if item.urgency == NotificationUrgency::Critical {
                card.add_css_class("critical");
            }
            if is_new {
                card.add_css_class("toast-enter");
            }

            let header = GtkBox::new(Orientation::Horizontal, 8);
            header.set_valign(Align::Center);
            let icon = build_notification_icon(item, 18);
            let app_lbl = Label::new(Some(if item.app_name.is_empty() {
                "System"
            } else {
                &item.app_name
            }));
            app_lbl.add_css_class("neon-qs-muted");
            app_lbl.set_hexpand(true);
            app_lbl.set_halign(Align::Start);
            app_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);

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
            header.append(&icon);
            header.append(&app_lbl);
            header.append(&close_btn);
            card.append(&header);

            // Clickable summary + body -> triggers D-Bus ActionInvoked + FocusWindow
            let body_btn = Button::new();
            body_btn.add_css_class("neon-toast-body-btn");
            let body_col = GtkBox::new(Orientation::Vertical, 3);
            let summary = Label::new(Some(&item.summary));
            summary.add_css_class("neon-qs-title");
            summary.set_halign(Align::Start);
            summary.set_xalign(0.0);
            summary.set_wrap(true);
            summary.set_max_width_chars(38);
            body_col.append(&summary);

            if !item.body.is_empty() {
                let body = Label::new(Some(&item.body));
                body.add_css_class("neon-qs-muted");
                body.set_halign(Align::Start);
                body.set_xalign(0.0);
                body.set_wrap(true);
                body.set_max_width_chars(42);
                body.set_lines(3);
                body.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                body_col.append(&body);
            }
            body_btn.set_child(Some(&body_col));
            {
                let item_c = item.clone();
                let store = Arc::clone(&self.store);
                let wins = Rc::clone(&self.windows);
                let ipc = self.ipc.clone();
                let overlay = self.clone();
                body_btn.connect_clicked(move |_| {
                    activate_notification(&item_c, None, &store, &wins.borrow(), &ipc);
                    overlay.remove_toast(item_c.id);
                });
            }
            card.append(&body_btn);

            let actions = visible_notification_actions(&item.actions);
            if !actions.is_empty() {
                let act_row = GtkBox::new(Orientation::Horizontal, 6);
                act_row.add_css_class("neon-toast-actions-row");
                for act in actions {
                    let abtn = Button::with_label(&act.label);
                    abtn.add_css_class("neon-notif-action-btn");
                    let item_c = item.clone();
                    let key_c = act.key.clone();
                    let store = Arc::clone(&self.store);
                    let wins = Rc::clone(&self.windows);
                    let ipc = self.ipc.clone();
                    let overlay = self.clone();
                    abtn.connect_clicked(move |_| {
                        activate_notification(
                            &item_c,
                            Some(&key_c),
                            &store,
                            &wins.borrow(),
                            &ipc,
                        );
                        overlay.remove_toast(item_c.id);
                    });
                    act_row.append(&abtn);
                }
                card.append(&act_row);
            }

            self.cards_by_id.borrow_mut().insert(item.id, card.clone());
            self.container.append(&card);
            if is_new {
                card.add_tick_callback(|w, _| {
                    w.remove_css_class("toast-enter");
                    glib::ControlFlow::Break
                });
            }
        }
        drop(seen);

        self.window.set_visible(true);
        self.window.present();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_win(id: u64, app_id: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id,
            workspace_id: 1,
            app_id: Some(app_id.into()),
            title: Some(title.into()),
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            focused: false,
            minimized: false,
            maximized: false,
            fullscreen: false,
            tiled: false,
            always_on_top: false,
        }
    }

    #[test]
    fn notification_window_matching_prefers_exact_app_or_title() {
        let windows = vec![
            mk_win(10, "kitty", "bash"),
            mk_win(20, "google-chrome", "ChatGPT - Google Chrome"),
            mk_win(30, "org.gnome.Nautilus", "Files"),
        ];

        let chrome_notif = DesktopNotification {
            id: 1,
            app_name: "Google Chrome".into(),
            app_icon: "google-chrome".into(),
            summary: "Reply from ChatGPT".into(),
            body: "".into(),
            urgency: NotificationUrgency::Normal,
            expire_timeout_ms: 5000,
            received_unix_secs: 0,
            dismissed: false,
            actions: Vec::new(),
            desktop_entry: Some("google-chrome".into()),
            resident: false,
        };
        assert_eq!(match_notification_window(&chrome_notif, &windows), Some(20));

        let files_notif = DesktopNotification {
            id: 2,
            app_name: "Nautilus".into(),
            app_icon: "".into(),
            summary: "Copy Complete".into(),
            body: "".into(),
            urgency: NotificationUrgency::Normal,
            expire_timeout_ms: 5000,
            received_unix_secs: 0,
            dismissed: false,
            actions: Vec::new(),
            desktop_entry: None,
            resident: false,
        };
        assert_eq!(match_notification_window(&files_notif, &windows), Some(30));
    }

    #[test]
    fn relative_time_formats_cleanly() {
        assert_eq!(format_relative_delta(5), "Just now");
        assert_eq!(format_relative_delta(125), "2m ago");
        assert_eq!(format_relative_delta(7200), "2h ago");
        assert_eq!(format_relative_delta(172_800), "2d ago");
    }
}
