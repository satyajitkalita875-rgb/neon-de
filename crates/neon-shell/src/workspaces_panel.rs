//! NEON DE Fullscreen Mission Control / Workspaces Exposé (`zwlr_layer_shell_v1` `Layer::Top`,
//! `exclusive_zone = -1`).
//!
//! Features:
//! - Fullscreen edge-to-edge pre-blurred desktop wallpaper synced to the unified System Blur
//!   slider and Light/Dark theme, with the live floating Dock elevated cleanly at the bottom.
//! - Centered top horizontal strip of Workspace / Desk cards (`Desk 1`, `Desk 2`, … + `+` card)
//!   with live proportional miniature window previews, active bright outline, hover remove (`×`),
//!   and persistent Pin toggle.
//! - Proportional, non-overlapping center Exposé spread of all windows on the active Desk using
//!   real GPU-captured window previews (zero letterboxing, exact aspect ratios), hover title pill,
//!   hover close (`×`), and right-click Window Context Menu.
//! - 60 FPS drag-and-drop of any window preview directly onto any `Desk N` card (or onto `+` to
//!   create a new Desk and move the window straight into it).

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, DrawingArea,
    EventControllerKey, EventControllerMotion, EventControllerScroll, EventControllerScrollFlags,
    Fixed, GestureClick, GestureDrag, Image, Label, Orientation, Overlay, Picture, PolicyType,
    Revealer, RevealerTransitionType, ScrolledWindow, Separator, gdk, gio, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{AppearanceInfo, DesktopSnapshot, IpcRequest, ScreencastTarget, WindowInfo, WorkspaceInfo};

use crate::{
    context_menu::ContextMenuOverlay,
    desktop_apps::{DesktopApp, load_app_icon, resolve_window_desktop_app},
    ipc_bridge::IpcCommandSender,
    wallpaper::{load_blurred_texture, load_scaled_texture, resolve_wallpaper_path},
};

const MAX_DESKTOPS: usize = 10;
const DESK_THUMB_W: i32 = 148;
const DESK_THUMB_H: i32 = 88;
const ADD_CARD_W: i32 = 84;
const DRAG_THRESHOLD_PX: f64 = 6.0;

type BoolCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;
type BoolListeners = Rc<RefCell<Vec<Box<dyn Fn(bool)>>>>;
type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

#[derive(Clone)]
struct WindowPreviewSlot {
    window_id: u64,
    path: PathBuf,
    pictures: Vec<Picture>,
    placeholders: Vec<GtkBox>,
    loaded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropChoice {
    Desk(usize),
    AddNewDesk,
}

#[derive(Clone)]
struct DropTargetWidget {
    choice: DropChoice,
    widget: gtk4::Widget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExposeSlot {
    pub window_id: u64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

struct ActiveDragState {
    window_id: u64,
    source_widget: gtk4::Widget,
    card_x: f64,
    card_y: f64,
    grab_x: f64,
    grab_y: f64,
    card_w: f64,
    card_h: f64,
    dragging: bool,
    ghost_box: Option<GtkBox>,
    ghost_w: f64,
    ghost_h: f64,
    hovered: Option<DropChoice>,
}

/// Fullscreen Mission Control / Workspaces Exposé toggled from the Top Bar `NEON` button or menu.
#[derive(Clone)]
pub struct WorkspacesPanel {
    window: ApplicationWindow,
    backdrop_overlay: Overlay,
    blur_picture: Picture,
    content_box: GtkBox,
    desk_tray: GtkBox,
    desk_strip_scroll: ScrolledWindow,
    desk_row: GtkBox,
    stage_holder: GtkBox,
    stage_fixed: Fixed,
    drag_layer: Fixed,
    is_open: Rc<Cell<bool>>,
    is_dragging: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    preview_gen: Rc<Cell<u64>>,
    motion_enabled: Rc<Cell<bool>>,
    speed_scale: Rc<Cell<f64>>,
    on_before_open: VoidCallback,
    on_state_listeners: BoolListeners,
    on_dock_elevate: BoolCallback,
    all_apps: Rc<RefCell<Vec<DesktopApp>>>,
    context_menu: ContextMenuOverlay,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    last_structure_sig: Rc<RefCell<Option<String>>>,
    last_active_ws_index: Rc<Cell<Option<usize>>>,
    known_ws_ids: Rc<RefCell<HashSet<u64>>>,
    removing_ws_ids: Rc<RefCell<HashSet<u64>>>,
    current_bg_sig: Rc<RefCell<Option<(PathBuf, bool, u32)>>>,
    thumb_wp_path: Rc<RefCell<Option<PathBuf>>>,
    thumb_wp_tex: Rc<RefCell<Option<gdk::Texture>>>,
    window_preview_cache: Rc<RefCell<HashMap<u64, gdk::Texture>>>,
    pending_slots: Rc<RefCell<Vec<WindowPreviewSlot>>>,
    drop_targets: Rc<RefCell<Vec<DropTargetWidget>>>,
    active_drag: Rc<RefCell<Option<ActiveDragState>>>,
    last_scroll_at: Rc<RefCell<Instant>>,
    preview_dir: PathBuf,
    ipc: IpcCommandSender,
}

impl WorkspacesPanel {
    pub fn build(
        app: &Application,
        apps: Rc<RefCell<Vec<DesktopApp>>>,
        ipc: IpcCommandSender,
        context_menu: ContextMenuOverlay,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Workspaces")
            .decorated(false)
            .build();
        window.add_css_class("neon-workspaces-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-workspaces-panel"));
            // Fullscreen Layer::Top spans edge-to-edge above desktop windows while sitting cleanly
            // beneath the elevated Dock and Layer::Overlay context menus / toasts.
            window.set_layer(Layer::Top);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let backdrop_overlay = Overlay::new();
        backdrop_overlay.add_css_class("neon-ws-mc-overlay");
        backdrop_overlay.add_css_class("ws-closed");
        backdrop_overlay.set_hexpand(true);
        backdrop_overlay.set_vexpand(true);

        let blur_picture = Picture::new();
        blur_picture.add_css_class("neon-ws-mc-blur-bg");
        blur_picture.set_content_fit(ContentFit::Cover);
        blur_picture.set_can_shrink(true);
        blur_picture.set_size_request(1, 1);
        blur_picture.set_hexpand(true);
        blur_picture.set_vexpand(true);

        let initial_wp = resolve_wallpaper_path(None, false);
        let initial_bg_sig = initial_wp.as_ref().map(|p| (p.clone(), false, 10u32));
        if let Some(ref wp) = initial_wp {
            if let Some(tex) = load_blurred_texture(wp, 320, 180, 10, 0.86, 1.32) {
                blur_picture.set_paintable(Some(&tex));
            } else {
                blur_picture.set_file(Some(&gio::File::for_path(wp)));
            }
        }
        let initial_thumb_tex = initial_wp
            .as_deref()
            .and_then(|p| load_scaled_texture(p, DESK_THUMB_W * 2, DESK_THUMB_H * 2));

        let blur_holder = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::External)
            .vscrollbar_policy(PolicyType::External)
            .propagate_natural_width(false)
            .propagate_natural_height(false)
            .can_target(false)
            .focusable(false)
            .child(&blur_picture)
            .build();

        let content_box = GtkBox::new(Orientation::Vertical, 0);
        content_box.add_css_class("neon-ws-mc-content");
        content_box.set_hexpand(true);
        content_box.set_vexpand(true);

        // ── 1. Top Floating Liquid Glass Desk Capsule (visual twin of the Dock) ───────
        let desk_tray = GtkBox::new(Orientation::Horizontal, 0);
        desk_tray.add_css_class("neon-ws-desk-tray");
        desk_tray.set_halign(Align::Center);
        desk_tray.set_valign(Align::Start);
        desk_tray.set_hexpand(false);
        desk_tray.set_vexpand(false);
        desk_tray.set_margin_top(14);
        desk_tray.set_margin_bottom(6);
        desk_tray.set_margin_start(28);
        desk_tray.set_margin_end(28);

        let desk_strip_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Never)
            .overlay_scrolling(true)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .max_content_width(1180)
            .hexpand(false)
            .vexpand(false)
            .build();
        desk_strip_scroll.add_css_class("neon-ws-desk-scroll");

        let desk_row = GtkBox::new(Orientation::Horizontal, 0);
        desk_row.add_css_class("neon-ws-desk-row");
        desk_row.set_halign(Align::Center);
        desk_row.set_valign(Align::Start);
        desk_strip_scroll.set_child(Some(&desk_row));
        desk_tray.append(&desk_strip_scroll);
        content_box.append(&desk_tray);

        // ── 2. Center Exposé Stage (Proportional Non-Overlapping Live Window Spread) ──
        let stage_holder = GtkBox::new(Orientation::Vertical, 0);
        stage_holder.add_css_class("neon-ws-expose-stage");
        stage_holder.set_hexpand(true);
        stage_holder.set_vexpand(true);
        stage_holder.set_halign(Align::Fill);
        stage_holder.set_valign(Align::Fill);

        let stage_fixed = Fixed::new();
        stage_fixed.set_hexpand(true);
        stage_fixed.set_vexpand(true);
        stage_fixed.set_halign(Align::Fill);
        stage_fixed.set_valign(Align::Fill);
        stage_holder.append(&stage_fixed);
        content_box.append(&stage_holder);

        // ── 3. Top-most Non-Intercepting Drag Layer (60 FPS floating dragged window) ──
        let drag_layer = Fixed::new();
        drag_layer.set_can_target(false);
        drag_layer.set_focusable(false);
        drag_layer.set_hexpand(true);
        drag_layer.set_vexpand(true);

        backdrop_overlay.set_child(Some(&blur_holder));
        backdrop_overlay.add_overlay(&content_box);
        backdrop_overlay.add_overlay(&drag_layer);
        window.set_child(Some(&backdrop_overlay));
        window.set_visible(false);

        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let preview_dir = runtime_dir.join("neon-de").join("ws-previews");
        let _ = fs::create_dir_all(&preview_dir);

        let panel = Self {
            window,
            backdrop_overlay,
            blur_picture,
            content_box,
            desk_tray: desk_tray.clone(),
            desk_strip_scroll: desk_strip_scroll.clone(),
            desk_row,
            stage_holder,
            stage_fixed,
            drag_layer,
            is_open: Rc::new(Cell::new(false)),
            is_dragging: Rc::new(Cell::new(false)),
            anim_gen: Rc::new(Cell::new(0u64)),
            preview_gen: Rc::new(Cell::new(0u64)),
            motion_enabled: Rc::new(Cell::new(true)),
            speed_scale: Rc::new(Cell::new(1.0)),
            on_before_open: Rc::new(RefCell::new(None)),
            on_state_listeners: Rc::new(RefCell::new(Vec::new())),
            on_dock_elevate: Rc::new(RefCell::new(None)),
            all_apps: apps,
            context_menu,
            last_snapshot: Rc::new(RefCell::new(None)),
            last_structure_sig: Rc::new(RefCell::new(None)),
            last_active_ws_index: Rc::new(Cell::new(None)),
            known_ws_ids: Rc::new(RefCell::new(HashSet::new())),
            removing_ws_ids: Rc::new(RefCell::new(HashSet::new())),
            current_bg_sig: Rc::new(RefCell::new(initial_bg_sig)),
            thumb_wp_path: Rc::new(RefCell::new(initial_wp)),
            thumb_wp_tex: Rc::new(RefCell::new(initial_thumb_tex)),
            window_preview_cache: Rc::new(RefCell::new(HashMap::new())),
            pending_slots: Rc::new(RefCell::new(Vec::new())),
            drop_targets: Rc::new(RefCell::new(Vec::new())),
            active_drag: Rc::new(RefCell::new(None)),
            last_scroll_at: Rc::new(RefCell::new(
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .unwrap_or_else(Instant::now),
            )),
            preview_dir,
            ipc,
        };

        // Clicking empty blurred wallpaper dismisses Mission Control cleanly.
        {
            let bg_click = GestureClick::new();
            bg_click.set_button(1);
            let p = panel.clone();
            bg_click.connect_released(move |gesture, _, x, y| {
                if !p.is_open.get() || p.is_dragging.get() {
                    return;
                }
                if let Some(widget) = gesture.widget()
                    && let Some(picked) = widget.pick(x, y, gtk4::PickFlags::DEFAULT)
                {
                    if is_inside_interactive_mc_widget(&picked) {
                        return;
                    }
                    p.close();
                }
            });
            panel.content_box.add_controller(bg_click);
        }

        // Scroll wheel over the top Desk strip switches Desks smoothly.
        {
            let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
            let p = panel.clone();
            scroll.connect_scroll(move |_, dx, dy| {
                if !p.is_open.get() || p.is_dragging.get() {
                    return glib::Propagation::Proceed;
                }
                let primary = if dy.abs() >= dx.abs() { dy } else { dx };
                if primary.abs() < 0.12 {
                    return glib::Propagation::Proceed;
                }
                let now = Instant::now();
                if now.duration_since(*p.last_scroll_at.borrow()) < Duration::from_millis(180) {
                    return glib::Propagation::Stop;
                }
                *p.last_scroll_at.borrow_mut() = now;
                if let Some(ref snap) = *p.last_snapshot.borrow() {
                    let cur = snap.active_workspace_index;
                    if primary > 0.0 && cur + 1 < snap.workspaces.len() {
                        p.ipc.send(IpcRequest::SwitchWorkspace { index: cur + 1 });
                    } else if primary < 0.0 && cur > 0 {
                        p.ipc.send(IpcRequest::SwitchWorkspace { index: cur - 1 });
                    }
                }
                glib::Propagation::Stop
            });
            desk_strip_scroll.add_controller(scroll);
        }

        // Keyboard controls: Escape closes, Left/Right switches Desks, 1–9 jumps directly, + adds Desk.
        {
            let key_ctrl = EventControllerKey::new();
            let p = panel.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if !p.is_open.get() {
                    return glib::Propagation::Proceed;
                }
                match key {
                    gdk::Key::Escape => {
                        if p.is_dragging.get() {
                            p.cancel_active_drag();
                        } else {
                            p.close();
                        }
                        glib::Propagation::Stop
                    }
                    gdk::Key::Left | gdk::Key::Up | gdk::Key::h => {
                        if let Some(ref snap) = *p.last_snapshot.borrow()
                            && snap.active_workspace_index > 0
                        {
                            p.ipc.send(IpcRequest::SwitchWorkspace {
                                index: snap.active_workspace_index - 1,
                            });
                        }
                        glib::Propagation::Stop
                    }
                    gdk::Key::Right | gdk::Key::Down | gdk::Key::l => {
                        if let Some(ref snap) = *p.last_snapshot.borrow()
                            && snap.active_workspace_index + 1 < snap.workspaces.len()
                        {
                            p.ipc.send(IpcRequest::SwitchWorkspace {
                                index: snap.active_workspace_index + 1,
                            });
                        }
                        glib::Propagation::Stop
                    }
                    gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => {
                        if let Some(ref snap) = *p.last_snapshot.borrow()
                            && snap.workspaces.len() < MAX_DESKTOPS
                        {
                            p.ipc.send(IpcRequest::AddWorkspace);
                        }
                        glib::Propagation::Stop
                    }
                    gdk::Key::_1
                    | gdk::Key::_2
                    | gdk::Key::_3
                    | gdk::Key::_4
                    | gdk::Key::_5
                    | gdk::Key::_6
                    | gdk::Key::_7
                    | gdk::Key::_8
                    | gdk::Key::_9 => {
                        let idx = match key {
                            gdk::Key::_1 => 0,
                            gdk::Key::_2 => 1,
                            gdk::Key::_3 => 2,
                            gdk::Key::_4 => 3,
                            gdk::Key::_5 => 4,
                            gdk::Key::_6 => 5,
                            gdk::Key::_7 => 6,
                            gdk::Key::_8 => 7,
                            _ => 8,
                        };
                        if let Some(ref snap) = *p.last_snapshot.borrow()
                            && idx < snap.workspaces.len()
                        {
                            p.ipc.send(IpcRequest::SwitchWorkspace { index: idx });
                        }
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            panel.window.add_controller(key_ctrl);
        }

        panel
    }

    pub fn set_on_before_open<F: Fn() + 'static>(&self, cb: F) {
        *self.on_before_open.borrow_mut() = Some(Box::new(cb));
    }

    pub fn add_on_state_change<F: Fn(bool) + 'static>(&self, cb: F) {
        self.on_state_listeners.borrow_mut().push(Box::new(cb));
    }

    pub fn set_on_dock_elevate<F: Fn(bool) + 'static>(&self, cb: F) {
        *self.on_dock_elevate.borrow_mut() = Some(Box::new(cb));
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
        self.cancel_active_drag();

        // Hide Top Bar cleanly & elevate Dock above fullscreen Mission Control backdrop.
        self.notify_state_change(true);
        self.notify_dock_elevate(true);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        }

        self.removing_ws_ids.borrow_mut().clear();

        if let Some(ref snap) = *self.last_snapshot.borrow() {
            self.update_backdrop_texture(&snap.appearance);
            *self.last_structure_sig.borrow_mut() = Some(compute_snapshot_sig(snap));
            self.last_active_ws_index
                .set(Some(snap.active_workspace_index));
            *self.known_ws_ids.borrow_mut() = snap.workspaces.iter().map(|w| w.id).collect();
            // Build immediately and kick off async GPU window captures at t = 0 so fresh previews
            // drop directly into the existing cards without disrupting the M3 Expressive IN spring.
            self.rebuild(snap, true, false);
        }

        self.stage_holder.remove_css_class("ws-stage-enter");
        self.backdrop_overlay.remove_css_class("ws-open");
        self.backdrop_overlay.remove_css_class("ws-closing");
        self.backdrop_overlay.add_css_class("ws-closed");
        self.window.set_visible(true);
        self.window.present();

        // Trigger Material 3 Expressive IN spring once Wayland commits the poised initial frame.
        let triggered = Rc::new(Cell::new(false));
        let ticks = Rc::new(Cell::new(0u8));
        {
            let gen_cell = Rc::clone(&self.anim_gen);
            let trig = Rc::clone(&triggered);
            let ticks_c = Rc::clone(&ticks);
            self.backdrop_overlay.add_tick_callback(move |widget, _| {
                if gen_cell.get() != epoch || trig.get() {
                    return glib::ControlFlow::Break;
                }
                let next = ticks_c.get().saturating_add(1);
                ticks_c.set(next);
                if next >= 2 {
                    trig.set(true);
                    widget.remove_css_class("ws-closed");
                    widget.remove_css_class("ws-closing");
                    widget.add_css_class("ws-open");
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            });
        }
        {
            let overlay = self.backdrop_overlay.clone();
            let gen_cell = Rc::clone(&self.anim_gen);
            let trig = Rc::clone(&triggered);
            glib::timeout_add_local_once(Duration::from_millis(48), move || {
                if gen_cell.get() == epoch && !trig.get() {
                    trig.set(true);
                    overlay.remove_css_class("ws-closed");
                    overlay.remove_css_class("ws-closing");
                    overlay.add_css_class("ws-open");
                }
            });
        }
    }

    fn flush_pending_workspace_removals(&self) {
        let pending: Vec<u64> = self.removing_ws_ids.borrow_mut().drain().collect();
        if pending.is_empty() {
            return;
        }
        if let Some(ref snap) = *self.last_snapshot.borrow() {
            // Remove highest index first so indices stay valid when multiple deletes were queued.
            let mut indices: Vec<usize> = pending
                .iter()
                .filter_map(|wid| snap.workspaces.iter().find(|w| w.id == *wid).map(|w| w.index))
                .collect();
            indices.sort_unstable_by(|a, b| b.cmp(a));
            for idx in indices {
                self.ipc.send(IpcRequest::RemoveWorkspace { index: idx });
            }
        }
    }

    /// Immediately closes Mission Control with zero delayed timers (used when switching directly
    /// to another shell overlay like App Drawer or Quick Settings, or when animations are disabled).
    pub fn close_immediate(&self) {
        self.flush_pending_workspace_removals();
        let was_active = self.is_open.get() || self.window.is_visible();
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.preview_gen.set(self.preview_gen.get().wrapping_add(1));
        self.cancel_active_drag();
        self.is_open.set(false);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::None);
        }
        self.backdrop_overlay.remove_css_class("ws-open");
        self.backdrop_overlay.remove_css_class("ws-closing");
        self.backdrop_overlay.add_css_class("ws-closed");
        self.window.set_visible(false);

        if was_active {
            self.notify_state_change(false);
            self.notify_dock_elevate(false);
        }
    }

    pub fn close(&self) {
        self.flush_pending_workspace_removals();
        if !self.is_open.get() && !self.window.is_visible() {
            return;
        }
        if !self.motion_enabled.get() {
            self.close_immediate();
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.preview_gen.set(self.preview_gen.get().wrapping_add(1));
        self.cancel_active_drag();
        self.is_open.set(false);

        // Material 3 Expressive OUT: smooth mirror of the IN spring choreography so the top Desk
        // capsule and Exposé cards glide expressively out before the blurred backdrop dissolves.
        self.backdrop_overlay.remove_css_class("ws-open");
        self.backdrop_overlay.remove_css_class("ws-closed");
        self.backdrop_overlay.add_css_class("ws-closing");

        let speed = self.speed_scale.get().clamp(0.5, 2.5);
        let topbar_reveal_ms = ((140.0 / speed).round() as u64).clamp(55, 290);
        let close_done_ms = ((400.0 / speed).round() as u64).clamp(160, 820);

        // Reveal Top Bar cleanly once the top Desk capsule clears the top edge.
        {
            let gen_cell = Rc::clone(&self.anim_gen);
            let listeners = Rc::clone(&self.on_state_listeners);
            glib::timeout_add_local_once(Duration::from_millis(topbar_reveal_ms), move || {
                if gen_cell.get() == epoch {
                    for cb in listeners.borrow().iter() {
                        cb(false);
                    }
                }
            });
        }

        let win = self.window.clone();
        let overlay = self.backdrop_overlay.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        let dock_cb = Rc::clone(&self.on_dock_elevate);
        glib::timeout_add_local_once(Duration::from_millis(close_done_ms), move || {
            if gen_cell.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    win.set_keyboard_mode(KeyboardMode::None);
                }
                win.set_visible(false);
                overlay.remove_css_class("ws-closing");
                overlay.add_css_class("ws-closed");
                if let Some(cb) = dock_cb.borrow().as_ref() {
                    cb(false);
                }
            }
        });
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        self.motion_enabled.set(
            snapshot.effects.enabled
                && !snapshot.effects.reduced_motion
                && snapshot.effects.overview_animation,
        );
        self.speed_scale
            .set(snapshot.effects.speed_scale.clamp(0.5, 2.5));
        self.update_backdrop_texture(&snapshot.appearance);
        let wp_changed = self.refresh_thumb_wallpaper_cache(snapshot);
        let sig = compute_snapshot_sig(snapshot);
        let changed = self.last_structure_sig.borrow().as_deref() != Some(sig.as_str());
        let ws_switched = self.last_active_ws_index.get() != Some(snapshot.active_workspace_index);
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());

        // Avoid tearing down the widget tree mid-drag or mid-workspace-delete collapse animation;
        // rebuild automatically right after drop or collapse finishes.
        if self.is_open.get()
            && !self.is_dragging.get()
            && self.removing_ws_ids.borrow().is_empty()
            && (changed || wp_changed)
        {
            *self.last_structure_sig.borrow_mut() = Some(sig);
            self.last_active_ws_index
                .set(Some(snapshot.active_workspace_index));
            let need_capture = snapshot
                .windows
                .iter()
                .any(|w| !self.window_preview_cache.borrow().contains_key(&w.id));
            self.rebuild(snapshot, need_capture, true);

            // When switching Desks inside Mission Control, spring the new Desk's stage in
            // using Material 3 Expressive Fast Spatial physics.
            if ws_switched {
                let stage = self.stage_holder.clone();
                stage.add_css_class("ws-stage-enter");
                let ticks = Rc::new(Cell::new(0u8));
                stage.add_tick_callback(move |w, _| {
                    let n = ticks.get().saturating_add(1);
                    ticks.set(n);
                    if n >= 2 {
                        w.remove_css_class("ws-stage-enter");
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                });
            }
        }
    }

    fn update_backdrop_texture(&self, appearance: &AppearanceInfo) {
        let light = appearance.theme == "neon_light" || appearance.theme == "light";
        let radius = if appearance.high_contrast
            || appearance.reduce_blur
            || appearance.blur_strength <= 0.01
        {
            0
        } else {
            ((9.5 * appearance.blur_strength.clamp(0.0, 2.0)).round() as usize).clamp(1, 18)
        };
        let sig_key = radius as u32;
        self.blur_picture.set_visible(!appearance.high_contrast);

        if let Some(wp) = resolve_wallpaper_path(appearance.wallpaper.as_deref(), light) {
            let next_sig = (wp.clone(), light, sig_key);
            if self.current_bg_sig.borrow().as_ref() != Some(&next_sig) {
                // Keep wallpaper vibrant & luminous with Light Mode airy brightness and Dark Mode depth.
                let (brightness, saturation) = if light { (0.96, 1.22) } else { (0.84, 1.32) };
                let (w, h) = if radius == 0 { (1280, 720) } else { (320, 180) };
                if let Some(tex) =
                    load_blurred_texture(&wp, w, h, radius, brightness, saturation)
                {
                    self.blur_picture.set_paintable(Some(&tex));
                } else {
                    self.blur_picture.set_file(Some(&gio::File::for_path(&wp)));
                }
                *self.current_bg_sig.borrow_mut() = Some(next_sig);
            }
        }
    }

    fn refresh_thumb_wallpaper_cache(&self, snapshot: &DesktopSnapshot) -> bool {
        let is_light =
            snapshot.appearance.theme == "neon_light" || snapshot.appearance.theme == "light";
        let resolved = resolve_wallpaper_path(snapshot.appearance.wallpaper.as_deref(), is_light);
        if resolved != *self.thumb_wp_path.borrow() {
            let tex = resolved
                .as_deref()
                .and_then(|p| load_scaled_texture(p, DESK_THUMB_W * 2, DESK_THUMB_H * 2));
            *self.thumb_wp_path.borrow_mut() = resolved;
            *self.thumb_wp_tex.borrow_mut() = tex;
            return true;
        }
        false
    }

    fn notify_state_change(&self, open: bool) {
        for cb in self.on_state_listeners.borrow().iter() {
            cb(open);
        }
    }

    fn notify_dock_elevate(&self, open: bool) {
        if let Some(cb) = self.on_dock_elevate.borrow().as_ref() {
            cb(open);
        }
    }

    fn cancel_active_drag(&self) {
        self.is_dragging.set(false);
        self.desk_tray.remove_css_class("drag-active");
        self.clear_drop_highlights();
        if let Some(drag) = self.active_drag.borrow_mut().take() {
            drag.source_widget.remove_css_class("dragging-source");
            if let Some(ghost) = drag.ghost_box {
                self.drag_layer.remove(&ghost);
            }
        }
    }

    fn clear_drop_highlights(&self) {
        for dt in self.drop_targets.borrow().iter() {
            dt.widget.remove_css_class("drop-target");
        }
    }

    fn update_drop_hover(&self, cursor_x: f64, cursor_y: f64) -> Option<DropChoice> {
        let mut found: Option<DropChoice> = None;
        for dt in self.drop_targets.borrow().iter() {
            let hit = dt
                .widget
                .compute_bounds(&self.backdrop_overlay)
                .is_some_and(|b| {
                    let pad = 10.0;
                    cursor_x >= f64::from(b.x()) - pad
                        && cursor_x <= f64::from(b.x() + b.width()) + pad
                        && cursor_y >= f64::from(b.y()) - pad
                        && cursor_y <= f64::from(b.y() + b.height()) + pad
                });
            if hit && found.is_none() {
                found = Some(dt.choice);
                dt.widget.add_css_class("drop-target");
            } else {
                dt.widget.remove_css_class("drop-target");
            }
        }
        found
    }

    fn rebuild(
        &self,
        snapshot: &DesktopSnapshot,
        request_live_previews: bool,
        animate_new_desks: bool,
    ) {
        while let Some(child) = self.desk_row.first_child() {
            self.desk_row.remove(&child);
        }
        while let Some(child) = self.stage_fixed.first_child() {
            self.stage_fixed.remove(&child);
        }
        self.drop_targets.borrow_mut().clear();
        self.pending_slots.borrow_mut().clear();

        let prev_known_ws: HashSet<u64> = self.known_ws_ids.borrow().clone();
        *self.known_ws_ids.borrow_mut() = snapshot.workspaces.iter().map(|w| w.id).collect();
        let motion_on = self.motion_enabled.get();
        let speed = self.speed_scale.get().clamp(0.5, 2.5);
        let reveal_ms = if motion_on {
            ((280.0 / speed).round() as u32).clamp(110, 560)
        } else {
            0
        };

        // Prune stale cached window textures for closed windows.
        {
            let live_wids: HashSet<u64> = snapshot.windows.iter().map(|w| w.id).collect();
            self.window_preview_cache
                .borrow_mut()
                .retain(|id, _| live_wids.contains(id));
        }

        let (out_w, out_h) = snapshot
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| snapshot.outputs.first())
            .map(|o| (o.width.max(800), o.height.max(540)))
            .unwrap_or((1280, 800));
        self.desk_strip_scroll
            .set_max_content_width((out_w - 84).max(320));

        let is_light =
            snapshot.appearance.theme == "neon_light" || snapshot.appearance.theme == "light";
        let accent_rgb = parse_hex_rgb(&snapshot.appearance.accent_color).unwrap_or((0.04, 0.52, 1.0));
        let wp_tex = self.thumb_wp_tex.borrow().clone();
        let total_ws = snapshot.workspaces.len();

        // Track every GTK Picture/placeholder per `window_id` so a single GPU capture updates
        // both its Exposé card and its miniature Desk thumbnail instantaneously.
        let mut pics_by_win: HashMap<u64, Vec<Picture>> = HashMap::new();
        let mut placeholders_by_win: HashMap<u64, Vec<GtkBox>> = HashMap::new();

        // ── 1. Build Top Horizontal Desk Strip (Desk 1, Desk 2, ..., +) ──────────────
        let mut any_new_desk = false;
        for ws in &snapshot.workspaces {
            let ws_windows: Vec<WindowInfo> = snapshot
                .windows
                .iter()
                .filter(|w| w.workspace_id == ws.id)
                .cloned()
                .collect();
            let is_new_desk = animate_new_desks
                && motion_on
                && !prev_known_ws.is_empty()
                && !prev_known_ws.contains(&ws.id);
            if is_new_desk {
                any_new_desk = true;
            }

            let revealer = Revealer::new();
            revealer.set_transition_type(RevealerTransitionType::SlideRight);
            revealer.set_halign(Align::Center);
            revealer.set_valign(Align::Start);

            let desk_col = self.build_desk_card(
                ws,
                total_ws,
                &ws_windows,
                out_w,
                out_h,
                is_light,
                accent_rgb,
                wp_tex.as_ref(),
                &revealer,
                &mut pics_by_win,
            );
            revealer.set_child(Some(&desk_col));

            if is_new_desk {
                revealer.set_overflow(gtk4::Overflow::Hidden);
                revealer.set_transition_duration(reveal_ms);
                revealer.set_reveal_child(false);
                desk_col.add_css_class("ws-desk-enter");

                let triggered = Rc::new(Cell::new(false));
                let ticks = Rc::new(Cell::new(0u8));
                let rev_t = revealer.clone();
                let col_t = desk_col.clone();
                let trig_t = Rc::clone(&triggered);
                let ticks_t = Rc::clone(&ticks);
                let settle_ms = u64::from(reveal_ms) + 45;
                desk_col.add_tick_callback(move |_, _| {
                    if trig_t.get() {
                        return glib::ControlFlow::Break;
                    }
                    let n = ticks_t.get().saturating_add(1);
                    ticks_t.set(n);
                    if n >= 2 {
                        trig_t.set(true);
                        rev_t.set_reveal_child(true);
                        col_t.remove_css_class("ws-desk-enter");
                        let rev_done = rev_t.clone();
                        glib::timeout_add_local_once(
                            Duration::from_millis(settle_ms),
                            move || {
                                if rev_done.reveals_child() {
                                    rev_done.set_overflow(gtk4::Overflow::Visible);
                                }
                            },
                        );
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                });
                let rev_fb = revealer.clone();
                let col_fb = desk_col.clone();
                let trig_fb = Rc::clone(&triggered);
                glib::timeout_add_local_once(Duration::from_millis(45), move || {
                    if !trig_fb.get() {
                        trig_fb.set(true);
                        rev_fb.set_reveal_child(true);
                        col_fb.remove_css_class("ws-desk-enter");
                        let rev_done = rev_fb.clone();
                        glib::timeout_add_local_once(
                            Duration::from_millis(settle_ms),
                            move || {
                                if rev_done.reveals_child() {
                                    rev_done.set_overflow(gtk4::Overflow::Visible);
                                }
                            },
                        );
                    }
                });
            } else {
                revealer.set_transition_duration(0);
                revealer.set_reveal_child(true);
                revealer.set_overflow(gtk4::Overflow::Visible);
                revealer.set_transition_duration(reveal_ms);
            }

            self.drop_targets.borrow_mut().push(DropTargetWidget {
                choice: DropChoice::Desk(ws.index),
                widget: desk_col.clone().upcast(),
            });
            self.desk_row.append(&revealer);
        }

        if any_new_desk {
            let scroller = self.desk_strip_scroll.clone();
            glib::timeout_add_local_once(Duration::from_millis(60), move || {
                let adj = scroller.hadjustment();
                let max_val = (adj.upper() - adj.page_size()).max(0.0);
                if max_val > 0.0 {
                    adj.set_value(max_val);
                }
            });
        }

        // Liquid Glass `+` card separated by a hairline divider (mirroring the floating Dock).
        if total_ws < MAX_DESKTOPS {
            let sep = Separator::new(Orientation::Vertical);
            sep.add_css_class("neon-ws-tray-sep");
            sep.set_valign(Align::Fill);
            sep.set_margin_start(6);
            sep.set_margin_end(6);
            self.desk_row.append(&sep);

            let add_col = GtkBox::new(Orientation::Vertical, 4);
            add_col.add_css_class("neon-ws-add-col");
            add_col.set_halign(Align::Center);
            add_col.set_valign(Align::Start);
            add_col.set_margin_start(6);
            add_col.set_margin_end(6);

            // Outer ring matches `.neon-ws-desk-ring` inset so `+` aligns pixel-for-pixel
            // with the top, bottom, and label baseline of `Desk 1..N`.
            let add_ring = GtkBox::new(Orientation::Vertical, 0);
            add_ring.add_css_class("neon-ws-add-ring");
            add_ring.set_halign(Align::Center);
            add_ring.set_valign(Align::Start);

            let add_btn = Button::new();
            add_btn.add_css_class("neon-ws-desk-add-btn");
            add_btn.set_size_request(ADD_CARD_W, DESK_THUMB_H);
            add_btn.set_halign(Align::Center);
            add_btn.set_valign(Align::Start);
            add_btn.set_vexpand(false);
            add_btn.set_tooltip_text(Some(&format!(
                "New Desk ({total_ws} / {MAX_DESKTOPS}) — or drag a window here"
            )));
            let plus = Image::from_icon_name("list-add-symbolic");
            plus.set_pixel_size(22);
            plus.set_halign(Align::Center);
            plus.set_valign(Align::Center);
            add_btn.set_child(Some(&plus));
            {
                let ipc_add = self.ipc.clone();
                let btn_c = add_btn.clone();
                add_btn.connect_clicked(move |_| {
                    btn_c.add_css_class("add-pulse");
                    let b = btn_c.clone();
                    glib::timeout_add_local_once(Duration::from_millis(160), move || {
                        b.remove_css_class("add-pulse");
                    });
                    ipc_add.send(IpcRequest::AddWorkspace);
                });
            }
            add_ring.append(&add_btn);

            let add_lbl = Label::new(Some("New"));
            add_lbl.add_css_class("neon-ws-desk-label");
            add_lbl.add_css_class("neon-ws-add-label");
            add_lbl.set_halign(Align::Center);

            let add_dot = GtkBox::new(Orientation::Horizontal, 0);
            add_dot.add_css_class("neon-ws-desk-indicator");
            add_dot.set_halign(Align::Center);

            add_col.append(&add_ring);
            add_col.append(&add_lbl);
            add_col.append(&add_dot);

            self.drop_targets.borrow_mut().push(DropTargetWidget {
                choice: DropChoice::AddNewDesk,
                widget: add_col.clone().upcast(),
            });
            self.desk_row.append(&add_col);
        }

        // ── 2. Build Center Exposé Spread for the Active Workspace ───────────────────
        let dock_pos = snapshot.appearance.dock_position.to_lowercase();
        let dock_sz = snapshot.appearance.dock_icon_size.clamp(22, 64) as i32;
        let dock_band = dock_sz + if dock_sz <= 32 { 40 } else { 50 };
        let (pad_left, pad_right, pad_bottom) = match dock_pos.as_str() {
            "left" => (dock_band + 16, 36, 26),
            "right" => (36, dock_band + 16, 26),
            _ => (36, 36, dock_band + 16),
        };
        let top_strip_h = 166;
        let stage_w = (out_w - pad_left - pad_right).max(400);
        let stage_h = (out_h - top_strip_h - pad_bottom).max(240);

        self.stage_holder.set_margin_start(pad_left);
        self.stage_holder.set_margin_end(pad_right);
        self.stage_holder.set_margin_top(6);
        self.stage_holder.set_margin_bottom(pad_bottom);

        let active_ws_id = snapshot
            .workspaces
            .get(snapshot.active_workspace_index)
            .map(|w| w.id)
            .or_else(|| snapshot.workspaces.first().map(|w| w.id))
            .unwrap_or(0);

        let mut active_windows: Vec<WindowInfo> = snapshot
            .windows
            .iter()
            .filter(|w| w.workspace_id == active_ws_id)
            .cloned()
            .collect();
        // Keep visible windows first and stable order by window id so cards don't jump around.
        active_windows.sort_by_key(|w| (w.minimized, w.id));

        if !active_windows.is_empty() {
            let slots = compute_expose_layout(&active_windows, stage_w, stage_h);
            let win_map: HashMap<u64, &WindowInfo> =
                active_windows.iter().map(|w| (w.id, w)).collect();

            for (slot_idx, slot) in slots.into_iter().enumerate() {
                if let Some(win) = win_map.get(&slot.window_id) {
                    let card = self.build_expose_window_card(
                        win,
                        &slot,
                        slot_idx,
                        snapshot.active_workspace_index,
                        is_light,
                        &mut pics_by_win,
                        &mut placeholders_by_win,
                    );
                    self.stage_fixed
                        .put(&card, f64::from(slot.x), f64::from(slot.y));
                }
            }
        }

        // ── 3. Request Fresh Live GPU Window Captures (Single Capture Per Window!) ───
        if request_live_previews && !snapshot.windows.is_empty() {
            let gen_id = self.preview_gen.get().wrapping_add(1);
            self.preview_gen.set(gen_id);
            cleanup_stale_previews(&self.preview_dir);

            let mut pending = Vec::new();
            for win in &snapshot.windows {
                let pics = pics_by_win.remove(&win.id).unwrap_or_default();
                let phs = placeholders_by_win.remove(&win.id).unwrap_or_default();
                if pics.is_empty() {
                    continue;
                }
                let path = self
                    .preview_dir
                    .join(format!("win-{}-{gen_id}.png", win.id));
                let is_active = win.workspace_id == active_ws_id;
                let (mw, mh) = if is_active { (760, 520) } else { (360, 240) };
                self.ipc.send(IpcRequest::CaptureScreencastPreview {
                    target: ScreencastTarget::Window { window_id: win.id },
                    path: path.to_string_lossy().into_owned(),
                    max_width: Some(mw),
                    max_height: Some(mh),
                });
                pending.push(WindowPreviewSlot {
                    window_id: win.id,
                    path,
                    pictures: pics,
                    placeholders: phs,
                    loaded: false,
                });
            }
            if !pending.is_empty() {
                *self.pending_slots.borrow_mut() = pending;
                self.start_preview_poller(gen_id);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_desk_card(
        &self,
        ws: &WorkspaceInfo,
        total_workspaces: usize,
        ws_windows: &[WindowInfo],
        out_w: i32,
        out_h: i32,
        is_light: bool,
        accent_rgb: (f64, f64, f64),
        wp_tex: Option<&gdk::Texture>,
        revealer: &Revealer,
        pics_by_win: &mut HashMap<u64, Vec<Picture>>,
    ) -> GtkBox {
        let col = GtkBox::new(Orientation::Vertical, 4);
        col.add_css_class("neon-ws-desk-col");
        if ws.active {
            col.add_css_class("active");
        }
        col.set_halign(Align::Center);
        col.set_valign(Align::Start);
        col.set_margin_start(6);
        col.set_margin_end(6);

        let shortcut_num = if ws.index < 9 {
            format!("{}", ws.index + 1)
        } else {
            "0".to_string()
        };
        let visible_count = ws_windows.iter().filter(|w| !w.minimized).count();
        let tooltip = if visible_count == 0 {
            format!("Desk {} · Empty · Super+{shortcut_num}", ws.index + 1)
        } else {
            format!(
                "Desk {} · {} window{} · Super+{shortcut_num}",
                ws.index + 1,
                visible_count,
                if visible_count == 1 { "" } else { "s" }
            )
        };
        col.set_tooltip_text(Some(&tooltip));

        // Outer active ring wrapper (luminous accent highlight echoing NEON DE active controls).
        let frame_ring = GtkBox::new(Orientation::Vertical, 0);
        frame_ring.add_css_class("neon-ws-desk-ring");
        if ws.active {
            frame_ring.add_css_class("active");
        }
        frame_ring.set_halign(Align::Center);
        frame_ring.set_valign(Align::Start);

        let thumb_frame = GtkBox::new(Orientation::Vertical, 0);
        thumb_frame.add_css_class("neon-ws-desk-thumb");
        thumb_frame.set_size_request(DESK_THUMB_W, DESK_THUMB_H);
        thumb_frame.set_halign(Align::Center);
        thumb_frame.set_valign(Align::Start);
        thumb_frame.set_hexpand(false);
        thumb_frame.set_vexpand(false);
        thumb_frame.set_overflow(gtk4::Overflow::Hidden);

        let overlay = Overlay::new();
        overlay.set_size_request(DESK_THUMB_W, DESK_THUMB_H);
        overlay.set_hexpand(false);
        overlay.set_vexpand(false);

        // Wallpaper base layer wrapped in an External ScrolledWindow so 2x HiDPI textures
        // never inflate the Desk tray's natural width or thumbnail height.
        let wp_pic = Picture::new();
        wp_pic.set_content_fit(ContentFit::Cover);
        wp_pic.set_can_shrink(true);
        wp_pic.set_size_request(1, 1);
        if let Some(wp) = wp_tex {
            wp_pic.set_paintable(Some(wp));
        }
        let wp_holder = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::External)
            .vscrollbar_policy(PolicyType::External)
            .propagate_natural_width(false)
            .propagate_natural_height(false)
            .can_target(false)
            .focusable(false)
            .child(&wp_pic)
            .build();
        wp_holder.set_size_request(DESK_THUMB_W, DESK_THUMB_H);
        overlay.set_child(Some(&wp_holder));

        // Live proportional miniature windows rendered directly on the Desk thumbnail!
        let mini_fixed = Fixed::new();
        mini_fixed.set_size_request(DESK_THUMB_W, DESK_THUMB_H);
        mini_fixed.set_can_target(false);

        let ow = f64::from(out_w.max(1));
        let oh = f64::from(out_h.max(1));
        let tw = f64::from(DESK_THUMB_W);
        let th = f64::from(DESK_THUMB_H);
        let cache = self.window_preview_cache.borrow();

        let mut mini_wins: Vec<&WindowInfo> =
            ws_windows.iter().filter(|w| !w.minimized).collect();
        // Render focused window last so it sits cleanly on top inside the miniature preview.
        mini_wins.sort_by_key(|w| (w.focused, w.id));

        for win in mini_wins {
            let rx = (f64::from(win.x) / ow * tw).clamp(2.0, tw - 16.0);
            let ry = (f64::from(win.y) / oh * th).clamp(2.0, th - 12.0);
            let rw = (f64::from(win.width) / ow * tw).clamp(18.0, tw - rx - 2.0);
            let rh = (f64::from(win.height) / oh * th).clamp(12.0, th - ry - 2.0);

            let mini_card = GtkBox::new(Orientation::Vertical, 0);
            mini_card.add_css_class("neon-ws-desk-mini-win");
            mini_card.set_size_request(rw.round() as i32, rh.round() as i32);
            mini_card.set_overflow(gtk4::Overflow::Hidden);

            let mini_pic = Picture::new();
            mini_pic.set_content_fit(ContentFit::Cover);
            mini_pic.set_can_shrink(true);
            mini_pic.set_size_request(rw.round() as i32, rh.round() as i32);
            if let Some(tex) = cache.get(&win.id) {
                mini_pic.set_paintable(Some(tex));
            }
            mini_card.append(&mini_pic);
            pics_by_win.entry(win.id).or_default().push(mini_pic);
            mini_fixed.put(&mini_card, rx.round(), ry.round());
        }
        drop(cache);
        overlay.add_overlay(&mini_fixed);

        // Discreet hover remove `×` button (silky anti-aliased Cairo vector circle, zero 4-dot border artifacts)
        if total_workspaces > 1 {
            let rm_btn = build_vector_close_button(24, is_light, "neon-ws-thumb-close", "Remove Desk");
            rm_btn.set_halign(Align::End);
            rm_btn.set_valign(Align::Start);
            rm_btn.set_margin_top(2);
            rm_btn.set_margin_end(2);
            let idx = ws.index;
            let ws_id = ws.id;
            let panel_rm = self.clone();
            let col_rm = col.clone();
            let rev_rm = revealer.clone();
            let btn_rm = rm_btn.clone();
            rm_btn.connect_clicked(move |_| {
                if !panel_rm.motion_enabled.get() {
                    panel_rm.ipc.send(IpcRequest::RemoveWorkspace { index: idx });
                    return;
                }
                if !panel_rm.removing_ws_ids.borrow_mut().insert(ws_id) {
                    return;
                }
                btn_rm.set_sensitive(false);
                col_rm.add_css_class("ws-desk-removing");

                let speed = panel_rm.speed_scale.get().clamp(0.5, 2.5);
                let collapse_ms = ((240.0 / speed).round() as u32).clamp(95, 480);
                rev_rm.set_overflow(gtk4::Overflow::Hidden);
                rev_rm.set_transition_type(RevealerTransitionType::SlideLeft);
                rev_rm.set_transition_duration(collapse_ms);
                rev_rm.set_reveal_child(false);

                let p_done = panel_rm.clone();
                glib::timeout_add_local_once(
                    Duration::from_millis(u64::from(collapse_ms) + 18),
                    move || {
                        let still_pending = p_done.removing_ws_ids.borrow_mut().remove(&ws_id);
                        if !still_pending {
                            return;
                        }
                        *p_done.last_structure_sig.borrow_mut() = None;
                        let live_idx = p_done
                            .last_snapshot
                            .borrow()
                            .as_ref()
                            .and_then(|s| s.workspaces.iter().find(|w| w.id == ws_id))
                            .map(|w| w.index)
                            .unwrap_or(idx);
                        p_done
                            .ipc
                            .send(IpcRequest::RemoveWorkspace { index: live_idx });
                    },
                );
            });
            overlay.add_overlay(&rm_btn);
        }

        // Discreet Pin button (silky anti-aliased Cairo vector circle, visible on hover or when pinned)
        {
            let pin_btn = build_vector_pin_button(24, ws.pinned, is_light, accent_rgb);
            if ws.pinned {
                pin_btn.add_css_class("pinned");
                pin_btn.set_tooltip_text(Some("Pinned Desk (click to unpin)"));
            } else {
                pin_btn.set_tooltip_text(Some("Pin Desk (keep even when empty)"));
            }
            pin_btn.set_halign(Align::End);
            pin_btn.set_valign(Align::End);
            pin_btn.set_margin_bottom(2);
            pin_btn.set_margin_end(2);
            let idx = ws.index;
            let ipc_pin = self.ipc.clone();
            pin_btn.connect_clicked(move |_| {
                ipc_pin.send(IpcRequest::TogglePinWorkspace { index: idx });
            });
            overlay.add_overlay(&pin_btn);
        }

        thumb_frame.append(&overlay);
        frame_ring.append(&thumb_frame);
        col.append(&frame_ring);

        let name_lbl = Label::new(Some(&format!("Desk {}", ws.index + 1)));
        name_lbl.add_css_class("neon-ws-desk-label");
        if ws.active {
            name_lbl.add_css_class("active");
        }
        name_lbl.set_halign(Align::Center);
        col.append(&name_lbl);

        // Sleek Dock-mirrored indicator bar beneath each Desk label (glowing accent pill when active).
        let indicator = GtkBox::new(Orientation::Horizontal, 0);
        indicator.add_css_class("neon-ws-desk-indicator");
        if ws.active {
            indicator.add_css_class("active");
        } else if !ws_windows.is_empty() {
            indicator.add_css_class("occupied");
        }
        indicator.set_halign(Align::Center);
        col.append(&indicator);

        // Single-click an inactive Desk switches to it (staying in Mission Control so you can
        // browse & organize Desks); clicking the active Desk or double-clicking enters it!
        let click = GestureClick::new();
        click.set_button(1);
        let idx = ws.index;
        let is_active = ws.active;
        let ipc_sw = self.ipc.clone();
        let panel_c = self.clone();
        click.connect_released(move |gesture, n_press, x, y| {
            let on_button = gesture
                .widget()
                .and_then(|w| w.pick(x, y, gtk4::PickFlags::DEFAULT))
                .and_then(|picked| picked.ancestor(Button::static_type()))
                .is_some();
            if on_button {
                return;
            }
            if !is_active {
                ipc_sw.send(IpcRequest::SwitchWorkspace { index: idx });
                if n_press >= 2 {
                    panel_c.close();
                }
            } else {
                panel_c.close();
            }
        });
        col.add_controller(click);

        col
    }

    #[allow(clippy::too_many_arguments)]
    fn build_expose_window_card(
        &self,
        win: &WindowInfo,
        slot: &ExposeSlot,
        slot_idx: usize,
        active_ws_index: usize,
        is_light: bool,
        pics_by_win: &mut HashMap<u64, Vec<Picture>>,
        placeholders_by_win: &mut HashMap<u64, Vec<GtkBox>>,
    ) -> Overlay {
        let card = Overlay::new();
        card.add_css_class("neon-ws-expose-card");
        card.add_css_class(&format!("neon-ws-stagger-{}", slot_idx.min(5)));
        if win.focused {
            card.add_css_class("focused");
        }
        if win.minimized {
            card.add_css_class("minimized");
        }
        card.set_size_request(slot.w, slot.h);

        let display_title = win
            .title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or(win.app_id.as_deref())
            .unwrap_or("Window")
            .to_string();
        let icon_spec = {
            let apps = self.all_apps.borrow();
            resolve_window_desktop_app(win.app_id.as_deref(), win.title.as_deref(), &apps)
                .and_then(|a| a.icon.clone())
        };
        let icon_hint = win.app_id.as_deref().unwrap_or(&display_title).to_string();

        // Rounded clipped preview frame
        let frame = GtkBox::new(Orientation::Vertical, 0);
        frame.add_css_class("neon-ws-expose-frame");
        frame.set_size_request(slot.w, slot.h);
        frame.set_overflow(gtk4::Overflow::Hidden);

        let frame_overlay = Overlay::new();
        frame_overlay.set_size_request(slot.w, slot.h);

        let cached_tex = self.window_preview_cache.borrow().get(&win.id).cloned();

        let pic = Picture::new();
        pic.set_content_fit(ContentFit::Contain);
        pic.set_can_shrink(true);
        pic.set_size_request(slot.w, slot.h);
        pic.set_hexpand(true);
        pic.set_vexpand(true);
        if let Some(ref tex) = cached_tex {
            pic.set_paintable(Some(tex));
        }
        frame_overlay.set_child(Some(&pic));
        pics_by_win.entry(win.id).or_default().push(pic);

        // Instant sleek placeholder shown only until the first live GPU screenshot arrives (~30ms).
        let placeholder = GtkBox::new(Orientation::Vertical, 8);
        placeholder.add_css_class("neon-ws-expose-placeholder");
        placeholder.set_halign(Align::Fill);
        placeholder.set_valign(Align::Fill);
        placeholder.set_hexpand(true);
        placeholder.set_vexpand(true);
        placeholder.set_can_target(false);
        let ph_inner = GtkBox::new(Orientation::Vertical, 8);
        ph_inner.set_halign(Align::Center);
        ph_inner.set_valign(Align::Center);
        ph_inner.set_vexpand(true);
        let ph_icon = load_app_icon(icon_spec.as_deref(), &icon_hint, 44);
        let ph_lbl = Label::new(Some(&display_title));
        ph_lbl.add_css_class("neon-ws-expose-placeholder-title");
        ph_lbl.set_max_width_chars(24);
        ph_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        ph_inner.append(&ph_icon);
        ph_inner.append(&ph_lbl);
        placeholder.append(&ph_inner);
        placeholder.set_visible(cached_tex.is_none());
        frame_overlay.add_overlay(&placeholder);
        placeholders_by_win
            .entry(win.id)
            .or_default()
            .push(placeholder);

        frame.append(&frame_overlay);
        card.set_child(Some(&frame));

        // Floating glass title pill (shown smoothly on hover, or always subtly if minimized)
        let title_pill = GtkBox::new(Orientation::Horizontal, 6);
        title_pill.add_css_class("neon-ws-expose-title-pill");
        if win.minimized {
            title_pill.add_css_class("always-visible");
        }
        title_pill.set_halign(Align::Center);
        title_pill.set_valign(Align::End);
        title_pill.set_margin_bottom(10);
        title_pill.set_margin_start(12);
        title_pill.set_margin_end(12);
        title_pill.set_can_target(false);

        let pill_icon = load_app_icon(icon_spec.as_deref(), &icon_hint, 16);
        let pill_lbl = Label::new(Some(&display_title));
        pill_lbl.add_css_class("neon-ws-expose-title-text");
        pill_lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        pill_lbl.set_max_width_chars(26);
        title_pill.append(&pill_icon);
        title_pill.append(&pill_lbl);
        if win.minimized {
            let min_tag = Label::new(Some("Minimized"));
            min_tag.add_css_class("neon-ws-expose-min-tag");
            title_pill.append(&min_tag);
        }
        card.add_overlay(&title_pill);

        // Hover close `×` button at top-right corner of window card (silky Cairo vector circle)
        let close_btn =
            build_vector_close_button(30, is_light, "neon-ws-expose-close", "Close Window");
        close_btn.set_halign(Align::End);
        close_btn.set_valign(Align::Start);
        close_btn.set_margin_top(6);
        close_btn.set_margin_end(6);
        {
            let wid = win.id;
            let ipc_cl = self.ipc.clone();
            close_btn.connect_clicked(move |_| {
                ipc_cl.send(IpcRequest::CloseWindow { window_id: wid });
            });
        }
        card.add_overlay(&close_btn);

        // Right-click opens our macOS Window Context Menu
        {
            let rc = GestureClick::new();
            rc.set_button(3);
            let wid = win.id;
            let cm = self.context_menu.clone();
            let card_c = card.clone();
            let root_ov = self.backdrop_overlay.clone();
            rc.connect_pressed(move |gesture, _, x, y| {
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                let (rx, ry) = card_c
                    .compute_bounds(&root_ov)
                    .map(|b| (f64::from(b.x()) + x, f64::from(b.y()) + y))
                    .unwrap_or((x, y));
                cm.open_window_menu(rx.round() as i32, ry.round() as i32, wid);
            });
            card.add_controller(rc);
        }

        // Left-click (focus & close) + 60 FPS Drag-and-Drop onto any Desk card or `+` button!
        {
            let drag = GestureDrag::new();
            drag.set_button(1);

            let wid = win.id;
            let card_w = f64::from(slot.w);
            let card_h = f64::from(slot.h);
            let title_for_ghost = display_title;
            let icon_spec_ghost = icon_spec;
            let icon_hint_ghost = icon_hint;

            {
                let panel_b = self.clone();
                let card_b = card.clone();
                drag.connect_drag_begin(move |gesture, start_x, start_y| {
                    // Never hijack clicks on the close `×` button.
                    let on_btn = gesture
                        .widget()
                        .and_then(|w| w.pick(start_x, start_y, gtk4::PickFlags::DEFAULT))
                        .and_then(|picked| picked.ancestor(Button::static_type()))
                        .is_some();
                    if on_btn {
                        gesture.set_state(gtk4::EventSequenceState::Denied);
                        return;
                    }
                    let (cx, cy) = card_b
                        .compute_bounds(&panel_b.backdrop_overlay)
                        .map(|b| (f64::from(b.x()), f64::from(b.y())))
                        .unwrap_or((0.0, 0.0));
                    *panel_b.active_drag.borrow_mut() = Some(ActiveDragState {
                        window_id: wid,
                        source_widget: card_b.clone().upcast(),
                        card_x: cx,
                        card_y: cy,
                        grab_x: start_x,
                        grab_y: start_y,
                        card_w,
                        card_h,
                        dragging: false,
                        ghost_box: None,
                        ghost_w: card_w,
                        ghost_h: card_h,
                        hovered: None,
                    });
                });
            }

            {
                let panel_u = self.clone();
                drag.connect_drag_update(move |_, dx, dy| {
                    let mut guard = panel_u.active_drag.borrow_mut();
                    let Some(st) = guard.as_mut() else {
                        return;
                    };
                    if !st.dragging && dx.hypot(dy) < DRAG_THRESHOLD_PX {
                        return;
                    }
                    let cursor_x = st.card_x + st.grab_x + dx;
                    let cursor_y = st.card_y + st.grab_y + dy;

                    if !st.dragging {
                        st.dragging = true;
                        panel_u.is_dragging.set(true);
                        panel_u.desk_tray.add_css_class("drag-active");
                        st.source_widget.add_css_class("dragging-source");

                        let aspect = (st.card_w / st.card_h.max(1.0)).clamp(0.45, 2.35);
                        let gh = (st.card_h * 0.56).clamp(96.0, 180.0);
                        let gw = (gh * aspect).clamp(92.0, 290.0);
                        st.ghost_w = gw;
                        st.ghost_h = gh;

                        let ghost = GtkBox::new(Orientation::Vertical, 0);
                        ghost.add_css_class("neon-ws-drag-ghost");
                        ghost.set_size_request(gw.round() as i32, gh.round() as i32);
                        ghost.set_overflow(gtk4::Overflow::Hidden);
                        ghost.set_can_target(false);

                        if let Some(tex) = panel_u.window_preview_cache.borrow().get(&st.window_id)
                        {
                            let gpic = Picture::new();
                            gpic.set_content_fit(ContentFit::Contain);
                            gpic.set_can_shrink(true);
                            gpic.set_size_request(gw.round() as i32, gh.round() as i32);
                            gpic.set_paintable(Some(tex));
                            ghost.append(&gpic);
                        } else {
                            let fb = GtkBox::new(Orientation::Vertical, 6);
                            fb.add_css_class("neon-ws-expose-placeholder");
                            fb.set_hexpand(true);
                            fb.set_vexpand(true);
                            fb.set_halign(Align::Center);
                            fb.set_valign(Align::Center);
                            let ic = load_app_icon(
                                icon_spec_ghost.as_deref(),
                                &icon_hint_ghost,
                                32,
                            );
                            let lb = Label::new(Some(&title_for_ghost));
                            lb.add_css_class("neon-ws-expose-title-text");
                            lb.set_max_width_chars(18);
                            lb.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                            fb.append(&ic);
                            fb.append(&lb);
                            ghost.append(&fb);
                        }

                        panel_u.drag_layer.put(
                            &ghost,
                            (cursor_x - gw * 0.5).round(),
                            (cursor_y - gh * 0.5).round(),
                        );
                        st.ghost_box = Some(ghost);
                    }

                    if let Some(ref ghost) = st.ghost_box {
                        // Subtle shrink as the dragged window approaches the top Desk strip!
                        let near_strip = ((cursor_y - 90.0) / 160.0).clamp(0.0, 1.0);
                        let shrink = 0.72 + 0.28 * near_strip;
                        let draw_w = (st.ghost_w * shrink).round().max(72.0);
                        let draw_h = (st.ghost_h * shrink).round().max(56.0);
                        ghost.set_size_request(draw_w as i32, draw_h as i32);
                        panel_u.drag_layer.move_(
                            ghost,
                            (cursor_x - draw_w * 0.5).round(),
                            (cursor_y - draw_h * 0.5).round(),
                        );
                    }
                    let hovered = panel_u.update_drop_hover(cursor_x, cursor_y);
                    st.hovered = hovered;
                });
            }

            {
                let panel_e = self.clone();
                drag.connect_drag_end(move |_, _dx, _dy| {
                    let Some(st) = panel_e.active_drag.borrow_mut().take() else {
                        return;
                    };
                    panel_e.is_dragging.set(false);
                    panel_e.desk_tray.remove_css_class("drag-active");
                    panel_e.clear_drop_highlights();
                    if let Some(ghost) = st.ghost_box {
                        panel_e.drag_layer.remove(&ghost);
                    }

                    if !st.dragging {
                        // Clean click: focus this window and exit Mission Control!
                        panel_e
                            .ipc
                            .send(IpcRequest::FocusWindow { window_id: wid });
                        panel_e.close();
                        return;
                    }

                    match st.hovered {
                        Some(DropChoice::Desk(target_idx)) if target_idx != active_ws_index => {
                            panel_e.ipc.send(IpcRequest::MoveWindowToWorkspace {
                                window_id: wid,
                                workspace_index: target_idx,
                            });
                            // Optimistically update local snapshot so the card moves from the
                            // active Exposé spread into the target Desk thumbnail with 0ms lag.
                            let updated = {
                                let mut snap_guard = panel_e.last_snapshot.borrow_mut();
                                if let Some(snap) = snap_guard.as_mut()
                                    && let Some(target_ws_id) =
                                        snap.workspaces.get(target_idx).map(|w| w.id)
                                {
                                    for w in &mut snap.windows {
                                        if w.id == wid {
                                            w.workspace_id = target_ws_id;
                                        }
                                    }
                                    for ws in &mut snap.workspaces {
                                        ws.window_ids.retain(|&id| id != wid);
                                        if ws.id == target_ws_id {
                                            ws.window_ids.push(wid);
                                        }
                                    }
                                    Some(snap.clone())
                                } else {
                                    None
                                }
                            };
                            if let Some(snap) = updated {
                                *panel_e.last_structure_sig.borrow_mut() =
                                    Some(compute_snapshot_sig(&snap));
                                panel_e.rebuild(&snap, false, false);
                            }
                        }
                        Some(DropChoice::AddNewDesk) => {
                            let new_idx = panel_e
                                .last_snapshot
                                .borrow()
                                .as_ref()
                                .map(|s| s.workspaces.len())
                                .unwrap_or(0);
                            if new_idx < MAX_DESKTOPS {
                                panel_e.ipc.send(IpcRequest::AddWorkspace);
                                panel_e.ipc.send(IpcRequest::MoveWindowToWorkspace {
                                    window_id: wid,
                                    workspace_index: new_idx,
                                });
                            } else {
                                st.source_widget.remove_css_class("dragging-source");
                            }
                        }
                        _ => {
                            st.source_widget.remove_css_class("dragging-source");
                            // Refresh if an IPC snapshot arrived while dragging.
                            let snap_opt = panel_e.last_snapshot.borrow().clone();
                            if let Some(snap) = snap_opt {
                                let sig = compute_snapshot_sig(&snap);
                                if panel_e.last_structure_sig.borrow().as_deref()
                                    != Some(sig.as_str())
                                {
                                    *panel_e.last_structure_sig.borrow_mut() = Some(sig);
                                    panel_e.rebuild(&snap, false, true);
                                }
                            }
                        }
                    }
                });
            }

            card.add_controller(drag);
        }

        card
    }

    fn start_preview_poller(&self, gen_id: u64) {
        let slots_c = Rc::clone(&self.pending_slots);
        let cache_c = Rc::clone(&self.window_preview_cache);
        let gen_c = Rc::clone(&self.preview_gen);
        let open_c = Rc::clone(&self.is_open);
        let started = Instant::now();

        glib::timeout_add_local(Duration::from_millis(30), move || {
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
                    for pic in &slot.pictures {
                        pic.set_paintable(Some(&tex));
                    }
                    for ph in &slot.placeholders {
                        ph.set_visible(false);
                    }
                    cache_c.borrow_mut().insert(slot.window_id, tex);
                    let _ = fs::remove_file(&slot.path);
                    slot.loaded = true;
                    continue;
                }
                remaining += 1;
            }
            if remaining == 0 || started.elapsed() > Duration::from_secs(3) {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}

/// Computes a balanced, aspect-ratio-preserving, non-overlapping Mission Control Exposé layout
/// inside `(stage_w, stage_h)` (matching macOS / Cutefish spread geometry for 1 to 20+ windows).
pub fn compute_expose_layout(
    windows: &[WindowInfo],
    stage_w: i32,
    stage_h: i32,
) -> Vec<ExposeSlot> {
    if windows.is_empty() || stage_w <= 60 || stage_h <= 60 {
        return Vec::new();
    }

    let sw = f64::from(stage_w);
    let sh = f64::from(stage_h);
    let gap_x = if windows.len() >= 7 { 22.0 } else { 28.0 };
    let gap_y = if windows.len() >= 7 { 24.0 } else { 30.0 };

    let aspects: Vec<f64> = windows
        .iter()
        .map(|w| {
            if w.width > 0 && w.height > 0 {
                (f64::from(w.width) / f64::from(w.height)).clamp(0.45, 2.35)
            } else {
                1.45
            }
        })
        .collect();

    let max_rows = if windows.len() <= 3 {
        windows.len()
    } else if windows.len() <= 9 {
        3.min(windows.len())
    } else {
        4.min(windows.len())
    };

    let mut best_score = -1.0_f64;
    let mut best_rows: Vec<Vec<usize>> = Vec::new();
    let mut best_row_h = 100.0_f64;

    for num_rows in 1..=max_rows {
        let rows = partition_balanced_rows(&aspects, num_rows);
        if rows.iter().any(Vec::is_empty) {
            continue;
        }
        let max_h_vert = ((sh - gap_y * (num_rows as f64 - 1.0)) / num_rows as f64).max(40.0);
        // Elegantly frame 1–2 windows instead of stretching them wall-to-wall.
        let single_cap = if windows.len() == 1 {
            sh * 0.72
        } else if num_rows == 1 {
            sh * 0.66
        } else {
            max_h_vert
        };
        let mut h = max_h_vert.min(single_cap);

        for row in &rows {
            let sum_ar: f64 = row.iter().map(|&idx| aspects[idx]).sum();
            let avail_w = (sw - gap_x * (row.len() as f64 - 1.0)).max(60.0);
            h = h.min(avail_w / sum_ar.max(0.1));
        }

        // Total visual area across all cards (prefer largest crisp previews).
        let mut total_area = 0.0;
        for row in &rows {
            for &idx in row {
                let win = &windows[idx];
                let nat_h = if win.height > 0 {
                    f64::from(win.height)
                } else {
                    600.0
                };
                let ch = h.min((nat_h * 1.05).max(180.0));
                let cw = ch * aspects[idx];
                total_area += cw * ch;
            }
        }
        // Slight penalty for unnecessary extra rows when 1–2 rows already have plenty of area.
        let score = total_area * (1.0 - 0.015 * (num_rows as f64 - 1.0));
        if score > best_score {
            best_score = score;
            best_rows = rows;
            best_row_h = h;
        }
    }

    // Compute exact per-row dimensions & center everything cleanly inside `(stage_w, stage_h)`.
    let mut row_items: Vec<Vec<(usize, i32, i32)>> = Vec::with_capacity(best_rows.len());
    let mut row_heights: Vec<i32> = Vec::with_capacity(best_rows.len());

    for row in &best_rows {
        let mut items = Vec::with_capacity(row.len());
        let mut max_ch = 0_i32;
        for &idx in row {
            let win = &windows[idx];
            let nat_h = if win.height > 0 {
                f64::from(win.height)
            } else {
                600.0
            };
            let ch = best_row_h.min((nat_h * 1.05).max(180.0)).max(84.0);
            let cw = (ch * aspects[idx]).max(84.0);
            let iw = cw.round() as i32;
            let ih = ch.round() as i32;
            max_ch = max_ch.max(ih);
            items.push((idx, iw, ih));
        }
        // Safety clamp in case rounding pushed a row slightly wider than stage_w.
        let sum_w: i32 = items.iter().map(|x| x.1).sum();
        let total_gap = (gap_x.round() as i32) * (items.len().saturating_sub(1) as i32);
        if sum_w + total_gap > stage_w && sum_w > 0 {
            let shrink = f64::from((stage_w - total_gap).max(80)) / f64::from(sum_w);
            max_ch = 0;
            for item in &mut items {
                item.1 = (f64::from(item.1) * shrink).floor().max(64.0) as i32;
                item.2 = (f64::from(item.2) * shrink).floor().max(64.0) as i32;
                max_ch = max_ch.max(item.2);
            }
        }
        row_heights.push(max_ch);
        row_items.push(items);
    }

    let gap_xi = gap_x.round() as i32;
    let gap_yi = gap_y.round() as i32;
    let total_h: i32 = row_heights.iter().sum::<i32>()
        + gap_yi * (row_heights.len().saturating_sub(1) as i32);
    let mut cur_y = ((stage_h - total_h) / 2).max(0);

    let mut out = Vec::with_capacity(windows.len());
    for (r_idx, items) in row_items.into_iter().enumerate() {
        let rh = row_heights[r_idx];
        let row_w: i32 = items.iter().map(|x| x.1).sum::<i32>()
            + gap_xi * (items.len().saturating_sub(1) as i32);
        let mut cur_x = ((stage_w - row_w) / 2).max(0);
        for (win_idx, w, h) in items {
            let y_off = (rh - h) / 2;
            out.push(ExposeSlot {
                window_id: windows[win_idx].id,
                x: cur_x,
                y: cur_y + y_off,
                w,
                h,
            });
            cur_x += w + gap_xi;
        }
        cur_y += rh + gap_yi;
    }

    out
}

/// Partitions `aspects` into `num_rows` contiguous rows minimizing the widest row's aspect sum
/// so narrow windows (e.g. Calculator) and wide windows (e.g. Terminal / Blender) pack cleanly.
fn partition_balanced_rows(aspects: &[f64], num_rows: usize) -> Vec<Vec<usize>> {
    let n = aspects.len();
    if num_rows <= 1 || n <= 1 {
        return vec![(0..n).collect()];
    }
    let k = num_rows.min(n);

    // Exact DP linear partition for tiny N (N <= 24): O(N^2 * K) takes < 5 microseconds!
    let mut prefix = vec![0.0_f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + aspects[i];
    }
    let mut dp = vec![vec![f64::INFINITY; k + 1]; n + 1];
    let mut split = vec![vec![0usize; k + 1]; n + 1];
    for i in 1..=n {
        dp[i][1] = prefix[i];
    }
    for r in 2..=k {
        for i in r..=n {
            for j in (r - 1)..i {
                let cost = dp[j][r - 1].max(prefix[i] - prefix[j]);
                if cost < dp[i][r] {
                    dp[i][r] = cost;
                    split[i][r] = j;
                }
            }
        }
    }

    let mut rows = vec![Vec::new(); k];
    let mut end = n;
    for r in (1..=k).rev() {
        let start = if r == 1 { 0 } else { split[end][r] };
        rows[r - 1] = (start..end).collect();
        end = start;
    }
    rows
}

fn is_inside_interactive_mc_widget(widget: &gtk4::Widget) -> bool {
    let mut cur = Some(widget.clone());
    while let Some(w) = cur {
        if w.has_css_class("neon-ws-expose-card")
            || w.has_css_class("neon-ws-desk-col")
            || w.has_css_class("neon-ws-desk-tray")
            || w.has_css_class("neon-ws-desk-add-btn")
            || w.is::<Button>()
        {
            return true;
        }
        cur = w.parent();
    }
    false
}

/// Draws a smooth, anti-aliased Liquid Glass circular badge with soft analytical drop shadow
/// and seamless continuous hairline rim — avoiding GTK4/GSK's 4-cardinal-dot `GskBorderNode`
/// artifact on small `border-radius: 999px` buttons.
fn draw_glass_circle_badge(
    cr: &gtk4::cairo::Context,
    w: f64,
    h: f64,
    fill_rgba: (f64, f64, f64, f64),
    rim_rgba: (f64, f64, f64, f64),
    gloss_alpha: f64,
    shadow_alpha: f64,
) -> (f64, f64, f64) {
    let cx = w * 0.5;
    let cy = h * 0.5;
    let r = (w.min(h) * 0.5 - 2.2).max(6.0);
    let tau = std::f64::consts::TAU;

    cr.set_antialias(gtk4::cairo::Antialias::Best);

    // 1. Soft analytical radial drop shadow (zero hard edges).
    if shadow_alpha > 0.001 {
        let shadow = gtk4::cairo::RadialGradient::new(cx, cy + 0.8, r * 0.68, cx, cy + 0.8, r + 2.1);
        shadow.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, shadow_alpha);
        shadow.add_color_stop_rgba(0.65, 0.0, 0.0, 0.0, shadow_alpha * 0.45);
        shadow.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.0);
        let _ = cr.set_source(&shadow);
        cr.arc(cx, cy + 0.8, r + 2.1, 0.0, tau);
        let _ = cr.fill();
    }

    // 2. Smooth anti-aliased circular base fill.
    cr.set_source_rgba(fill_rgba.0, fill_rgba.1, fill_rgba.2, fill_rgba.3);
    cr.arc(cx, cy, r, 0.0, tau);
    let _ = cr.fill();

    // 3. Subtle top-to-bottom Liquid Glass specular sheen clipped to the circle.
    if gloss_alpha > 0.001 {
        cr.save().ok();
        cr.arc(cx, cy, r, 0.0, tau);
        cr.clip();
        let sheen = gtk4::cairo::LinearGradient::new(0.0, cy - r, 0.0, cy + r);
        sheen.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, gloss_alpha);
        sheen.add_color_stop_rgba(0.55, 1.0, 1.0, 1.0, gloss_alpha * 0.15);
        sheen.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
        let _ = cr.set_source(&sheen);
        cr.rectangle(cx - r, cy - r, r * 2.0, r * 2.0);
        let _ = cr.fill();
        cr.restore().ok();
    }

    // 4. Seamless continuous circular rim (single 360° arc, zero cardinal seam dots).
    cr.set_source_rgba(rim_rgba.0, rim_rgba.1, rim_rgba.2, rim_rgba.3);
    cr.set_line_width(0.95);
    cr.arc(cx, cy, r - 0.45, 0.0, tau);
    let _ = cr.stroke();

    (cx, cy, r)
}

fn build_vector_close_button(
    size: i32,
    is_light: bool,
    css_class: &str,
    tooltip: &str,
) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-ws-vec-circle-btn");
    btn.add_css_class(css_class);
    btn.set_tooltip_text(Some(tooltip));
    btn.set_size_request(size, size);

    let hovered = Rc::new(Cell::new(false));
    let area = DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_halign(Align::Center);
    area.set_valign(Align::Center);

    {
        let hov = Rc::clone(&hovered);
        area.set_draw_func(move |_, cr, w, h| {
            let is_hov = hov.get();
            let (fill, rim, gloss, shadow) = if is_hov {
                (
                    (0.937, 0.267, 0.267, 0.98),
                    (1.0, 1.0, 1.0, 0.36),
                    0.22,
                    0.34,
                )
            } else if is_light {
                (
                    (0.965, 0.976, 1.0, 0.90),
                    (0.06, 0.09, 0.16, 0.16),
                    0.42,
                    0.22,
                )
            } else {
                (
                    (0.08, 0.10, 0.16, 0.84),
                    (1.0, 1.0, 1.0, 0.30),
                    0.18,
                    0.32,
                )
            };

            let (cx, cy, r) =
                draw_glass_circle_badge(cr, f64::from(w), f64::from(h), fill, rim, gloss, shadow);

            // Crisp, perfectly centered anti-aliased `×` vector strokes.
            let arm = (r * 0.36).max(3.2);
            let stroke_w = if size >= 28 { 1.85 } else { 1.55 };
            if is_hov || !is_light {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.96);
            } else {
                cr.set_source_rgba(0.06, 0.09, 0.16, 0.88);
            }
            cr.set_line_width(stroke_w);
            cr.set_line_cap(gtk4::cairo::LineCap::Round);
            cr.move_to(cx - arm, cy - arm);
            cr.line_to(cx + arm, cy + arm);
            cr.move_to(cx + arm, cy - arm);
            cr.line_to(cx - arm, cy + arm);
            let _ = cr.stroke();
        });
    }

    let motion = EventControllerMotion::new();
    {
        let hov = Rc::clone(&hovered);
        let a = area.clone();
        motion.connect_enter(move |_, _, _| {
            hov.set(true);
            a.queue_draw();
        });
    }
    {
        let hov = Rc::clone(&hovered);
        let a = area.clone();
        motion.connect_leave(move |_| {
            hov.set(false);
            a.queue_draw();
        });
    }
    btn.add_controller(motion);
    btn.set_child(Some(&area));
    btn
}

fn build_vector_pin_button(
    size: i32,
    pinned: bool,
    is_light: bool,
    accent_rgb: (f64, f64, f64),
) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-ws-vec-circle-btn");
    btn.add_css_class("neon-ws-pin-btn");
    btn.set_size_request(size, size);

    let hovered = Rc::new(Cell::new(false));
    let area = DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_halign(Align::Center);
    area.set_valign(Align::Center);
    let accent_luma = 0.299 * accent_rgb.0 + 0.587 * accent_rgb.1 + 0.114 * accent_rgb.2;

    {
        let hov = Rc::clone(&hovered);
        area.set_draw_func(move |_, cr, w, h| {
            let is_hov = hov.get();
            let (fill, rim, gloss, shadow) = if pinned {
                let lift = if is_hov { 0.08 } else { 0.0 };
                (
                    (
                        (accent_rgb.0 + lift).min(1.0),
                        (accent_rgb.1 + lift).min(1.0),
                        (accent_rgb.2 + lift).min(1.0),
                        0.96,
                    ),
                    (1.0, 1.0, 1.0, 0.42),
                    0.24,
                    0.34,
                )
            } else if is_light {
                let a = if is_hov { 0.98 } else { 0.90 };
                (
                    (0.965, 0.976, 1.0, a),
                    (0.06, 0.09, 0.16, if is_hov { 0.26 } else { 0.16 }),
                    0.42,
                    0.22,
                )
            } else {
                let lift = if is_hov { 0.10 } else { 0.0 };
                (
                    (0.08 + lift, 0.10 + lift, 0.16 + lift, 0.86),
                    (1.0, 1.0, 1.0, if is_hov { 0.48 } else { 0.30 }),
                    0.18,
                    0.32,
                )
            };

            let (cx, cy, r) =
                draw_glass_circle_badge(cr, f64::from(w), f64::from(h), fill, rim, gloss, shadow);

            cr.save().ok();
            cr.translate(cx, cy);
            cr.rotate(0.72); // ~41° clean diagonal thumbtack tilt
            let s = r / 9.5;
            cr.scale(s, s);

            if pinned {
                if accent_luma > 0.52 {
                    cr.set_source_rgba(0.06, 0.08, 0.13, 0.95);
                } else {
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
                }
            } else if is_light {
                cr.set_source_rgba(0.06, 0.09, 0.16, 0.88);
            } else {
                cr.set_source_rgba(0.97, 0.98, 1.0, 0.94);
            }

            // Clean, balanced anti-aliased thumbtack silhouette (crisp even at 1x DPI).
            cr.set_line_join(gtk4::cairo::LineJoin::Round);
            cr.set_line_cap(gtk4::cairo::LineCap::Round);

            // Top cap + tapered stem + wide collar as one unified crisp polygon.
            draw_rounded_rect(cr, -2.7, -4.6, 5.4, 1.65, 0.8);
            let _ = cr.fill();

            cr.move_to(-1.75, -3.1);
            cr.line_to(1.75, -3.1);
            cr.line_to(2.35, 0.55);
            cr.line_to(-2.35, 0.55);
            cr.close_path();
            let _ = cr.fill();

            draw_rounded_rect(cr, -3.5, 0.35, 7.0, 1.5, 0.75);
            let _ = cr.fill();

            // Clean round-capped vector needle stroke (never sub-pixel thin!).
            cr.set_line_width(1.35);
            cr.move_to(0.0, 1.7);
            cr.line_to(0.0, 4.75);
            let _ = cr.stroke();

            cr.restore().ok();
        });
    }

    let motion = EventControllerMotion::new();
    {
        let hov = Rc::clone(&hovered);
        let a = area.clone();
        motion.connect_enter(move |_, _, _| {
            hov.set(true);
            a.queue_draw();
        });
    }
    {
        let hov = Rc::clone(&hovered);
        let a = area.clone();
        motion.connect_leave(move |_| {
            hov.set(false);
            a.queue_draw();
        });
    }
    btn.add_controller(motion);
    btn.set_child(Some(&area));
    btn
}

fn draw_rounded_rect(cr: &gtk4::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let rad = r.min(w * 0.5).min(h * 0.5).max(0.0);
    let pi = std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - rad, y + rad, rad, -pi * 0.5, 0.0);
    cr.arc(x + w - rad, y + h - rad, rad, 0.0, pi * 0.5);
    cr.arc(x + rad, y + h - rad, rad, pi * 0.5, pi);
    cr.arc(x + rad, y + rad, rad, pi, pi * 1.5);
    cr.close_path();
}

fn parse_hex_rgb(hex: &str) -> Option<(f64, f64, f64)> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
    ))
}

fn compute_snapshot_sig(snapshot: &DesktopSnapshot) -> String {
    let mut s = format!(
        "{}|{}|{}|{}|{}|{}|",
        snapshot.active_workspace_index,
        snapshot.appearance.theme,
        snapshot.appearance.accent_color,
        snapshot.appearance.dock_position,
        snapshot.appearance.dock_icon_size,
        snapshot.appearance.wallpaper.as_deref().unwrap_or("")
    );
    for ws in &snapshot.workspaces {
        s.push_str(&format!(
            "w:{}:{}:{}:{};",
            ws.id,
            ws.index,
            ws.active as u8,
            ws.pinned as u8
        ));
        for wid in &ws.window_ids {
            s.push_str(&format!("{wid},"));
        }
    }
    for win in &snapshot.windows {
        s.push_str(&format!(
            "n:{}:{}:{}:{}:{}:{}:{};",
            win.id,
            win.workspace_id,
            win.x,
            win.y,
            win.width,
            win.height,
            (win.focused as u8) | ((win.minimized as u8) << 1)
        ));
    }
    s
}

fn cleanup_stale_previews(dir: &PathBuf) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("png") {
                let _ = fs::remove_file(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_win(id: u64, w: i32, h: i32) -> WindowInfo {
        WindowInfo {
            id,
            workspace_id: 1,
            app_id: Some(format!("app-{id}")),
            title: Some(format!("Window {id}")),
            x: 0,
            y: 0,
            width: w,
            height: h,
            focused: id == 1,
            minimized: false,
            maximized: false,
            fullscreen: false,
            tiled: false,
            always_on_top: false,
        }
    }

    #[test]
    fn expose_layout_packs_mixed_aspect_windows_without_overlap() {
        // Mirrors the reference Mission Control layout: 8 windows including tall Calculator & wide Blender.
        let wins = vec![
            mk_win(1, 760, 540),  // Files
            mk_win(2, 920, 620),  // Chrome
            mk_win(3, 820, 560),  // Calendar
            mk_win(4, 340, 560),  // Calculator (tall narrow)
            mk_win(5, 780, 600),  // GIMP
            mk_win(6, 1080, 620), // Terminal (wide)
            mk_win(7, 860, 600),  // VS Code
            mk_win(8, 1140, 620), // Blender (wide)
        ];
        let slots = compute_expose_layout(&wins, 1200, 540);
        assert_eq!(slots.len(), 8);

        // Every slot must stay within stage bounds and never overlap another slot.
        for (i, a) in slots.iter().enumerate() {
            assert!(a.x >= 0 && a.y >= 0, "slot {a:?} out of bounds");
            assert!(a.x + a.w <= 1200 && a.y + a.h <= 540, "slot {a:?} overflows stage");
            assert!(a.w >= 64 && a.h >= 64);
            for b in &slots[i + 1..] {
                let overlaps =
                    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y;
                assert!(!overlaps, "slots {a:?} and {b:?} overlapped");
            }
        }

        // Tall Calculator (id=4) must stay noticeably narrower than wide Blender (id=8).
        let calc = slots.iter().find(|s| s.window_id == 4).unwrap();
        let blender = slots.iter().find(|s| s.window_id == 8).unwrap();
        assert!(calc.w < calc.h, "Calculator should preserve portrait aspect: {calc:?}");
        assert!(blender.w > blender.h, "Blender should preserve landscape aspect: {blender:?}");
        assert!(blender.w > calc.w * 2, "Wide window should be much wider than narrow window");
    }

    #[test]
    fn expose_layout_centers_single_window_cleanly() {
        let wins = vec![mk_win(1, 900, 600)];
        let slots = compute_expose_layout(&wins, 1200, 540);
        assert_eq!(slots.len(), 1);
        let s = &slots[0];
        assert!(s.x > 100 && s.x + s.w < 1100);
        assert!(s.y > 40 && s.y + s.h < 500);
    }
}
