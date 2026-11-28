//! NEON DE Frosted-Glass Dock Hover Live Window Preview (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Hovering over any active/running Dock icon (either a running-window button or a pinned app
//! button that has open windows) for ~680ms reveals a GPU-blurred frosted-glass preview card
//! anchored cleanly above/beside that icon:
//! - Live GPU-captured window thumbnail(s) via `IpcRequest::CaptureScreencastPreview`
//! - App icon, window title, workspace badge (when on another Desk), and minimized tag
//! - Click card to focus/restore (auto-switching Desk if needed); click `×` to close window
//! - Zero-flicker dwell/switch/leave timers so gliding across Dock icons never causes popups

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fs,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit,
    EventControllerMotion, GestureClick, Image, Label, Orientation, Overlay, Picture, Separator,
    gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest, ScreencastTarget, WindowInfo};

use crate::{
    desktop_apps::{DesktopApp, load_app_icon, resolve_window_desktop_app},
    ipc_bridge::IpcCommandSender,
};

const INITIAL_HOVER_DELAY_MS: u64 = 420;
const SWITCH_HOVER_DELAY_MS: u64 = 150;
const LEAVE_GRACE_MS: u64 = 240;
const MIN_CARD_WIDTH_PX: i32 = 176;
const MAX_THUMB_W_PX: i32 = 220;
const MAX_THUMB_H_PX: i32 = 132;
const MIN_THUMB_W_PX: i32 = 112;
const MIN_THUMB_H_PX: i32 = 84;
const MAX_PREVIEW_CARDS: usize = 4;

fn compute_thumb_size(win: &WindowInfo) -> (i32, i32) {
    if win.width > 0 && win.height > 0 {
        let aspect = (win.width as f64) / (win.height as f64);
        let h_at_max_w = (MAX_THUMB_W_PX as f64) / aspect;
        if h_at_max_w <= MAX_THUMB_H_PX as f64 {
            let h = h_at_max_w
                .round()
                .clamp(MIN_THUMB_H_PX as f64, MAX_THUMB_H_PX as f64) as i32;
            (MAX_THUMB_W_PX, h)
        } else {
            let w = ((MAX_THUMB_H_PX as f64) * aspect)
                .round()
                .clamp(MIN_THUMB_W_PX as f64, MAX_THUMB_W_PX as f64) as i32;
            (w, MAX_THUMB_H_PX)
        }
    } else {
        (208, 120)
    }
}

type BoolListeners = Rc<RefCell<Vec<Box<dyn Fn(bool)>>>>;

/// Identifies which Dock icon triggered the hover preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockPreviewTarget {
    /// Specific running window icon in `running_box`.
    Window(u64),
    /// Pinned app icon in `pinned_box` (matched against all currently open windows of this app).
    PinnedApp { app_id: String, app_name: String },
}

#[derive(Debug, Clone)]
pub struct DockPreviewAnchor {
    pub target: DockPreviewTarget,
    pub screen_x: i32,
    pub screen_y: i32,
    pub dock_position: String,
    pub dock_band_px: i32,
    pub output_w: i32,
    pub output_h: i32,
}

#[derive(Clone)]
struct PendingThumb {
    window_id: u64,
    path: PathBuf,
    picture: Picture,
    placeholder: GtkBox,
    loaded: bool,
}

/// Frosted-glass live window preview popup anchored to hovered active Dock icons.
#[derive(Clone)]
pub struct DockPreviewPopup {
    window: ApplicationWindow,
    card_strip: GtkBox,
    ipc: IpcCommandSender,
    all_apps: Rc<RefCell<Vec<DesktopApp>>>,
    latest_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    is_open: Rc<Cell<bool>>,
    pointer_inside: Rc<Cell<bool>>,
    hover_epoch: Rc<Cell<u64>>,
    hide_epoch: Rc<Cell<u64>>,
    preview_gen: Rc<Cell<u64>>,
    active_anchor: Rc<RefCell<Option<DockPreviewAnchor>>>,
    thumb_cache: Rc<RefCell<HashMap<u64, gdk::Texture>>>,
    pending_thumbs: Rc<RefCell<Vec<PendingThumb>>>,
    on_state_listeners: BoolListeners,
    preview_dir: PathBuf,
}

impl DockPreviewPopup {
    pub fn build(
        app: &Application,
        all_apps: Rc<RefCell<Vec<DesktopApp>>>,
        ipc: IpcCommandSender,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Dock Preview")
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("neon-dock-preview-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-dock-preview"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let card_strip = GtkBox::new(Orientation::Horizontal, 8);
        card_strip.add_css_class("neon-dock-preview-card");
        card_strip.set_halign(Align::Center);
        card_strip.set_valign(Align::Center);

        window.set_child(Some(&card_strip));
        window.set_visible(false);

        let preview_dir = std::env::temp_dir().join(format!(
            "neon-dock-previews-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&preview_dir);

        let popup = Self {
            window,
            card_strip,
            ipc,
            all_apps,
            latest_snapshot: Rc::new(RefCell::new(None)),
            is_open: Rc::new(Cell::new(false)),
            pointer_inside: Rc::new(Cell::new(false)),
            hover_epoch: Rc::new(Cell::new(0)),
            hide_epoch: Rc::new(Cell::new(0)),
            preview_gen: Rc::new(Cell::new(0)),
            active_anchor: Rc::new(RefCell::new(None)),
            thumb_cache: Rc::new(RefCell::new(HashMap::new())),
            pending_thumbs: Rc::new(RefCell::new(Vec::new())),
            on_state_listeners: Rc::new(RefCell::new(Vec::new())),
            preview_dir,
        };

        // Keep popup open while pointer glides up from the Dock icon onto the preview card.
        let motion = EventControllerMotion::new();
        {
            let p = popup.clone();
            motion.connect_enter(move |_, _, _| {
                p.pointer_inside.set(true);
                p.hide_epoch.set(p.hide_epoch.get().wrapping_add(1));
            });
        }
        {
            let p = popup.clone();
            motion.connect_leave(move |_| {
                p.pointer_inside.set(false);
                p.schedule_hide(200);
            });
        }
        popup.card_strip.add_controller(motion);

        popup
    }

    pub fn add_on_state_change<F: Fn(bool) + 'static>(&self, cb: F) {
        self.on_state_listeners.borrow_mut().push(Box::new(cb));
    }

    fn notify_state(&self, open: bool) {
        for cb in self.on_state_listeners.borrow().iter() {
            cb(open);
        }
    }

    /// Updates cached desktop state and refreshes the open preview if windows changed or closed.
    pub fn update_snapshot(&self, snapshot: &DesktopSnapshot) {
        // Evict stale textures for closed windows so cache never grows unbounded.
        {
            let live_ids: std::collections::HashSet<u64> =
                snapshot.windows.iter().map(|w| w.id).collect();
            self.thumb_cache
                .borrow_mut()
                .retain(|wid, _| live_ids.contains(wid));
        }

        *self.latest_snapshot.borrow_mut() = Some(snapshot.clone());

        if snapshot.overview_open {
            self.hide_immediate();
            return;
        }

        if self.is_open.get()
            && let Some(anchor) = self.active_anchor.borrow().clone()
        {
            let matching = self.resolve_matching_windows(&anchor.target);
            if matching.is_empty() {
                self.hide_immediate();
            }
        }
    }

    /// Schedules showing the live preview card for an active Dock icon after a smooth dwell delay.
    pub fn schedule_hover(&self, anchor: DockPreviewAnchor) {
        // Cancel any pending leave-hide timer immediately.
        self.hide_epoch.set(self.hide_epoch.get().wrapping_add(1));

        // Only open previews for icons that have at least one active window.
        if self.resolve_matching_windows(&anchor.target).is_empty() {
            self.hover_epoch.set(self.hover_epoch.get().wrapping_add(1));
            if self.is_open.get() {
                self.schedule_hide(LEAVE_GRACE_MS);
            }
            return;
        }

        // If already showing this exact target, nothing to reschedule.
        if self.is_open.get()
            && self
                .active_anchor
                .borrow()
                .as_ref()
                .is_some_and(|cur| cur.target == anchor.target)
        {
            return;
        }

        let epoch = self.hover_epoch.get().wrapping_add(1);
        self.hover_epoch.set(epoch);
        let delay = if self.is_open.get() {
            SWITCH_HOVER_DELAY_MS
        } else {
            INITIAL_HOVER_DELAY_MS
        };

        let this = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if this.hover_epoch.get() == epoch {
                this.show_now(anchor);
            }
        });
    }

    /// Called when the pointer leaves a Dock icon; gives a brief grace window to enter the popup.
    pub fn on_dock_item_leave(&self) {
        self.hover_epoch.set(self.hover_epoch.get().wrapping_add(1));
        if self.is_open.get() {
            self.schedule_hide(LEAVE_GRACE_MS);
        }
    }

    fn schedule_hide(&self, delay_ms: u64) {
        let epoch = self.hide_epoch.get().wrapping_add(1);
        self.hide_epoch.set(epoch);
        let this = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
            if this.hide_epoch.get() == epoch && !this.pointer_inside.get() {
                this.hide_immediate();
            }
        });
    }

    /// Immediately hides the preview popup and cancels any pending dwell or capture timers.
    pub fn hide_immediate(&self) {
        self.hover_epoch.set(self.hover_epoch.get().wrapping_add(1));
        self.hide_epoch.set(self.hide_epoch.get().wrapping_add(1));
        self.preview_gen.set(self.preview_gen.get().wrapping_add(1));
        self.pointer_inside.set(false);
        self.pending_thumbs.borrow_mut().clear();
        *self.active_anchor.borrow_mut() = None;

        let was_open = self.is_open.replace(false);
        if self.window.is_visible() {
            self.window.set_visible(false);
        }
        if was_open {
            self.notify_state(false);
        }
    }

    /// Opens the preview popup immediately for the first open window (used by `--open-dock-preview-test`).
    pub fn open_first_window_for_test(&self) -> bool {
        let snap = self.latest_snapshot.borrow().clone();
        let Some(snap) = snap else {
            return false;
        };
        let Some(first_win) = snap.windows.first() else {
            return false;
        };
        let (out_w, out_h) = snap
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| snap.outputs.first())
            .map(|o| (o.width.max(800), o.height.max(600)))
            .unwrap_or((1280, 720));
        let anchor = DockPreviewAnchor {
            target: DockPreviewTarget::Window(first_win.id),
            screen_x: out_w / 2,
            screen_y: out_h - 32,
            dock_position: "bottom".into(),
            dock_band_px: 72,
            output_w: out_w,
            output_h: out_h,
        };
        self.show_now(anchor);
        true
    }

    fn resolve_matching_windows(&self, target: &DockPreviewTarget) -> Vec<WindowInfo> {
        let guard = self.latest_snapshot.borrow();
        let Some(ref snap) = *guard else {
            return Vec::new();
        };
        let apps = self.all_apps.borrow();
        let mut matched: Vec<WindowInfo> = match target {
            DockPreviewTarget::Window(wid) => snap
                .windows
                .iter()
                .filter(|w| w.id == *wid)
                .cloned()
                .collect(),
            DockPreviewTarget::PinnedApp { app_id, app_name } => snap
                .windows
                .iter()
                .filter(|w| window_matches_pinned_app(w, app_id, app_name, &apps))
                .cloned()
                .collect(),
        };
        // Prioritize focused window first, then preserve order, capped cleanly at MAX_PREVIEW_CARDS.
        matched.sort_by_key(|w| (!w.focused, w.minimized));
        matched.truncate(MAX_PREVIEW_CARDS);
        matched
    }

    fn show_now(&self, anchor: DockPreviewAnchor) {
        let windows = self.resolve_matching_windows(&anchor.target);
        if windows.is_empty() {
            self.hide_immediate();
            return;
        }

        let snap_opt = self.latest_snapshot.borrow().clone();
        let active_ws_id = snap_opt
            .as_ref()
            .and_then(|s| s.workspaces.iter().find(|ws| ws.active).map(|ws| ws.id));
        let ws_index_map: HashMap<u64, usize> = snap_opt
            .as_ref()
            .map(|s| s.workspaces.iter().map(|ws| (ws.id, ws.index + 1)).collect())
            .unwrap_or_default();

        while let Some(child) = self.card_strip.first_child() {
            self.card_strip.remove(&child);
        }
        let is_multi = windows.len() > 1;
        if is_multi {
            self.card_strip.add_css_class("multi");
        } else {
            self.card_strip.remove_css_class("multi");
        }

        let gen_id = self.preview_gen.get().wrapping_add(1);
        self.preview_gen.set(gen_id);
        let mut pending = Vec::with_capacity(windows.len());
        let apps = self.all_apps.borrow();
        let cache = self.thumb_cache.borrow();
        let mut total_est_w = 16i32;
        let mut max_est_h = 0i32;

        for (idx, win) in windows.iter().enumerate() {
            if idx > 0 {
                let sep = Separator::new(Orientation::Vertical);
                sep.add_css_class("neon-dock-preview-sep");
                self.card_strip.append(&sep);
                total_est_w += 17;
            }
            let ws_badge = if Some(win.workspace_id) != active_ws_id {
                ws_index_map
                    .get(&win.workspace_id)
                    .map(|n| format!("Desk {n}"))
            } else {
                None
            };
            let (thumb_w, thumb_h) = compute_thumb_size(win);
            let card_w = thumb_w.max(MIN_CARD_WIDTH_PX);
            total_est_w += card_w + if is_multi { 8 } else { 0 };
            max_est_h = max_est_h.max(thumb_h + 48);

            let cached_tex = cache.get(&win.id).cloned();
            let (card, pic, placeholder) = self.build_window_card(
                win,
                &apps,
                ws_badge.as_deref(),
                cached_tex.as_ref(),
                thumb_w,
                thumb_h,
                card_w,
            );
            self.card_strip.append(&card);

            let path = self
                .preview_dir
                .join(format!("dock-win-{}-{gen_id}.png", win.id));
            let _ = fs::remove_file(&path);
            self.ipc.send(IpcRequest::CaptureScreencastPreview {
                target: ScreencastTarget::Window { window_id: win.id },
                path: path.to_string_lossy().into_owned(),
                max_width: Some((thumb_w * 2) as u32),
                max_height: Some((thumb_h * 2) as u32),
            });
            pending.push(PendingThumb {
                window_id: win.id,
                path,
                picture: pic,
                placeholder,
                loaded: false,
            });
        }
        drop(cache);
        drop(apps);

        *self.pending_thumbs.borrow_mut() = pending;
        *self.active_anchor.borrow_mut() = Some(anchor.clone());

        self.apply_anchor_position(&anchor, total_est_w, max_est_h.max(140));

        let was_open = self.is_open.replace(true);
        self.window.set_default_size(1, 1);
        self.window.queue_resize();
        self.window.set_visible(true);
        self.window.present();
        if !was_open {
            self.notify_state(true);
        }

        self.start_thumbnail_poller(gen_id);
    }

    fn apply_anchor_position(&self, anchor: &DockPreviewAnchor, est_w: i32, est_h: i32) {
        if !gtk4_layer_shell::is_supported() {
            return;
        }
        let out_w = anchor.output_w.max(640);
        let out_h = anchor.output_h.max(480);
        let gap = 10;

        match anchor.dock_position.as_str() {
            "left" => {
                let top = (anchor.screen_y - est_h / 2).clamp(40, (out_h - est_h - 14).max(40));
                self.window.set_anchor(Edge::Bottom, false);
                self.window.set_anchor(Edge::Right, false);
                self.window.set_anchor(Edge::Top, true);
                self.window.set_anchor(Edge::Left, true);
                self.window
                    .set_margin(Edge::Left, anchor.dock_band_px + gap);
                self.window.set_margin(Edge::Top, top);
                self.window.set_margin(Edge::Bottom, 0);
                self.window.set_margin(Edge::Right, 0);
            }
            "right" => {
                let top = (anchor.screen_y - est_h / 2).clamp(40, (out_h - est_h - 14).max(40));
                self.window.set_anchor(Edge::Bottom, false);
                self.window.set_anchor(Edge::Left, false);
                self.window.set_anchor(Edge::Top, true);
                self.window.set_anchor(Edge::Right, true);
                self.window
                    .set_margin(Edge::Right, anchor.dock_band_px + gap);
                self.window.set_margin(Edge::Top, top);
                self.window.set_margin(Edge::Bottom, 0);
                self.window.set_margin(Edge::Left, 0);
            }
            _ => {
                let left = (anchor.screen_x - est_w / 2).clamp(12, (out_w - est_w - 12).max(12));
                self.window.set_anchor(Edge::Top, false);
                self.window.set_anchor(Edge::Right, false);
                self.window.set_anchor(Edge::Bottom, true);
                self.window.set_anchor(Edge::Left, true);
                self.window
                    .set_margin(Edge::Bottom, anchor.dock_band_px + gap);
                self.window.set_margin(Edge::Left, left);
                self.window.set_margin(Edge::Top, 0);
                self.window.set_margin(Edge::Right, 0);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_window_card(
        &self,
        win: &WindowInfo,
        apps: &[DesktopApp],
        ws_badge: Option<&str>,
        cached_tex: Option<&gdk::Texture>,
        thumb_w: i32,
        thumb_h: i32,
        card_w: i32,
    ) -> (GtkBox, Picture, GtkBox) {
        let card = GtkBox::new(Orientation::Vertical, 6);
        card.add_css_class("neon-dock-preview-item");
        card.set_size_request(card_w, -1);

        // Header: Clickable app icon + title + optional workspace badge | clean close button
        let header = GtkBox::new(Orientation::Horizontal, 6);
        header.add_css_class("neon-dock-preview-header");
        header.set_valign(Align::Center);

        let title_area = GtkBox::new(Orientation::Horizontal, 6);
        title_area.set_hexpand(true);
        title_area.set_valign(Align::Center);

        let resolved_app =
            resolve_window_desktop_app(win.app_id.as_deref(), win.title.as_deref(), apps);
        let icon_spec = resolved_app
            .and_then(|a| a.icon.as_deref())
            .or(win.app_id.as_deref());
        let icon_hint = win.app_id.as_deref().unwrap_or("");
        let app_icon = load_app_icon(icon_spec, icon_hint, 15);
        app_icon.set_valign(Align::Center);
        title_area.append(&app_icon);

        let title_str = win
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .or(resolved_app.map(|a| a.name.as_str()))
            .or(win.app_id.as_deref())
            .unwrap_or("Window");
        let title_lbl = Label::new(Some(title_str));
        title_lbl.add_css_class("neon-dock-preview-title");
        title_lbl.set_hexpand(true);
        title_lbl.set_halign(Align::Start);
        title_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        title_lbl.set_max_width_chars(18);
        title_area.append(&title_lbl);

        if let Some(badge) = ws_badge {
            let badge_lbl = Label::new(Some(badge));
            badge_lbl.add_css_class("neon-dock-preview-badge");
            title_area.append(&badge_lbl);
        } else if win.minimized {
            let min_lbl = Label::new(Some("Min"));
            min_lbl.add_css_class("neon-dock-preview-badge");
            title_area.append(&min_lbl);
        }

        let header_click = GestureClick::new();
        header_click.set_button(1);
        {
            let ipc_c = self.ipc.clone();
            let wid = win.id;
            let popup_c = self.clone();
            header_click.connect_released(move |_, _, _, _| {
                popup_c.hide_immediate();
                ipc_c.send(IpcRequest::SetOverview { open: false });
                ipc_c.send(IpcRequest::FocusWindow { window_id: wid });
            });
        }
        title_area.add_controller(header_click);
        header.append(&title_area);

        let close_btn = Button::new();
        close_btn.add_css_class("neon-dock-preview-close");
        close_btn.set_tooltip_text(Some("Close window"));
        let close_ic = Image::from_icon_name("window-close-symbolic");
        close_ic.set_pixel_size(12);
        close_btn.set_child(Some(&close_ic));
        {
            let ipc_c = self.ipc.clone();
            let wid = win.id;
            let popup_c = self.clone();
            close_btn.connect_clicked(move |_| {
                ipc_c.send(IpcRequest::CloseWindow { window_id: wid });
                popup_c.hide_immediate();
            });
        }
        header.append(&close_btn);
        card.append(&header);

        // Aspect-matched rounded live window preview (zero dark letterbox box behind the app).
        let thumb_frame = GtkBox::new(Orientation::Vertical, 0);
        thumb_frame.add_css_class("neon-dock-preview-thumb-frame");
        thumb_frame.set_overflow(gtk4::Overflow::Hidden);
        thumb_frame.set_halign(Align::Center);
        thumb_frame.set_valign(Align::Center);
        thumb_frame.set_size_request(thumb_w, thumb_h);

        let overlay = Overlay::new();
        overlay.set_size_request(thumb_w, thumb_h);

        let picture = Picture::new();
        picture.set_content_fit(ContentFit::Contain);
        picture.set_can_shrink(true);
        picture.set_size_request(thumb_w, thumb_h);
        if let Some(tex) = cached_tex {
            picture.set_paintable(Some(tex));
        }
        overlay.set_child(Some(&picture));

        let placeholder = GtkBox::new(Orientation::Vertical, 6);
        placeholder.add_css_class("neon-dock-preview-placeholder");
        placeholder.set_halign(Align::Center);
        placeholder.set_valign(Align::Center);
        let ph_icon = load_app_icon(icon_spec, icon_hint, 32);
        ph_icon.set_halign(Align::Center);
        let ph_lbl = Label::new(Some(if win.minimized {
            "Minimized"
        } else {
            "Loading…"
        }));
        ph_lbl.add_css_class("neon-dock-preview-placeholder-lbl");
        placeholder.append(&ph_icon);
        placeholder.append(&ph_lbl);
        placeholder.set_visible(cached_tex.is_none());
        overlay.add_overlay(&placeholder);

        thumb_frame.append(&overlay);
        card.append(&thumb_frame);

        // Clicking the thumbnail or title area focuses/restores the window cleanly.
        let click = GestureClick::new();
        click.set_button(1);
        {
            let ipc_c = self.ipc.clone();
            let wid = win.id;
            let popup_c = self.clone();
            click.connect_released(move |_, _, _, _| {
                popup_c.hide_immediate();
                ipc_c.send(IpcRequest::SetOverview { open: false });
                ipc_c.send(IpcRequest::FocusWindow { window_id: wid });
            });
        }
        thumb_frame.add_controller(click);

        (card, picture, placeholder)
    }

    fn start_thumbnail_poller(&self, gen_id: u64) {
        let slots_c = Rc::clone(&self.pending_thumbs);
        let cache_c = Rc::clone(&self.thumb_cache);
        let gen_c = Rc::clone(&self.preview_gen);
        let open_c = Rc::clone(&self.is_open);
        let started = Instant::now();

        glib::timeout_add_local(Duration::from_millis(25), move || {
            if !open_c.get() || gen_c.get() != gen_id {
                return glib::ControlFlow::Break;
            }
            let mut slots = slots_c.borrow_mut();
            let mut remaining = 0usize;
            for slot in slots.iter_mut() {
                if slot.loaded {
                    continue;
                }
                if slot.path.exists()
                    && let Ok(tex) = gdk::Texture::from_filename(&slot.path)
                {
                    slot.picture.set_paintable(Some(&tex));
                    slot.placeholder.set_visible(false);
                    cache_c.borrow_mut().insert(slot.window_id, tex);
                    let _ = fs::remove_file(&slot.path);
                    slot.loaded = true;
                    continue;
                }
                remaining += 1;
            }
            if remaining == 0 || started.elapsed() > Duration::from_millis(2500) {
                for slot in slots.iter() {
                    let _ = fs::remove_file(&slot.path);
                }
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}

/// Checks whether an open compositor window belongs to a pinned Dock `.desktop` application.
pub fn window_matches_pinned_app(
    win: &WindowInfo,
    pinned_id: &str,
    pinned_name: &str,
    all_apps: &[DesktopApp],
) -> bool {
    if let Some(resolved) =
        resolve_window_desktop_app(win.app_id.as_deref(), win.title.as_deref(), all_apps)
        && resolved.id.eq_ignore_ascii_case(pinned_id)
    {
        return true;
    }
    let norm_pin = pinned_id
        .trim_end_matches(".desktop")
        .to_ascii_lowercase();
    if let Some(aid) = win.app_id.as_deref() {
        let low = aid.to_ascii_lowercase();
        if low == norm_pin || low.ends_with(&format!(".{norm_pin}")) || norm_pin.ends_with(&format!(".{low}")) {
            return true;
        }
    }
    if let Some(title) = win.title.as_deref()
        && !pinned_name.is_empty()
        && title.eq_ignore_ascii_case(pinned_name)
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_pinned_app_by_app_id_suffix_or_title() {
        let win = WindowInfo {
            id: 7,
            workspace_id: 1,
            app_id: Some("org.gnome.Nautilus".into()),
            title: Some("Home".into()),
            x: 100,
            y: 100,
            width: 800,
            height: 600,
            focused: true,
            minimized: false,
            maximized: false,
            fullscreen: false,
            tiled: false,
            always_on_top: false,
        };
        assert!(window_matches_pinned_app(
            &win,
            "org.gnome.Nautilus.desktop",
            "Files",
            &[]
        ));
        assert!(!window_matches_pinned_app(
            &win,
            "firefox.desktop",
            "Firefox",
            &[]
        ));
    }

    #[test]
    fn compute_thumb_size_preserves_landscape_and_tiled_aspect_ratios() {
        let mut win = WindowInfo {
            id: 1,
            workspace_id: 1,
            app_id: Some("brave".into()),
            title: Some("Brave".into()),
            x: 0,
            y: 0,
            width: 1920,
            height: 1032,
            focused: true,
            minimized: false,
            maximized: true,
            fullscreen: false,
            tiled: false,
            always_on_top: false,
        };
        let (lw, lh) = compute_thumb_size(&win);
        assert_eq!(lw, MAX_THUMB_W_PX);
        assert!(lh >= 110 && lh <= 125);

        // Half-screen tiled window (951x1020) fits cleanly at MAX_THUMB_H_PX with proportional width.
        win.width = 951;
        win.height = 1020;
        let (tw, th) = compute_thumb_size(&win);
        assert_eq!(th, MAX_THUMB_H_PX);
        assert!(tw >= 115 && tw <= 130);
    }
}
