//! NEON DE COSMIC-Inspired Left-Edge Workspaces Sidebar (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Slides in smoothly from the left edge of the screen with real-time GPU backdrop blur
//! (`neon-renderer` `"neon-workspaces-panel"` namespace) and a fullscreen transparent click-catcher
//! (`"neon-workspaces-backdrop"`). Displays a vertical strip of live workspace preview cards
//! (real GPU compositor thumbnails with instant wallpaper + window-miniature fallback), active
//! accent outline, COSMIC-style workspace pin toggles, and one-click `+ New Workspace` (up to 10).

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    fs,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CenterBox, ContentFit, DrawingArea,
    EventControllerKey, GestureClick, Image, Label, Orientation, Overlay, PolicyType, Picture,
    ScrolledWindow, Separator, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest, ScreencastTarget, WindowInfo, WorkspaceInfo};

use crate::{
    ipc_bridge::IpcCommandSender,
    wallpaper::{load_scaled_texture, resolve_wallpaper_path},
};

const MAX_DESKTOPS: usize = 10;
const WS_SIDEBAR_WIDTH: i32 = 228;
const THUMB_WIDTH: i32 = 194;
const THUMB_HEIGHT: i32 = 110;

type StateChangeCallback = Rc<RefCell<Option<Box<dyn Fn(bool)>>>>;

struct PendingThumbSlot {
    ws_id: u64,
    path: PathBuf,
    picture: Picture,
    fallback_overlay: DrawingArea,
    loaded: bool,
}

/// COSMIC-style vertical left-edge Workspaces Sidebar toggled by the Top Bar `NEON` button.
#[derive(Clone)]
pub struct WorkspacesPanel {
    window: ApplicationWindow,
    backdrop_window: ApplicationWindow,
    is_open: Rc<Cell<bool>>,
    anim_gen: Rc<Cell<u64>>,
    preview_gen: Rc<Cell<u64>>,
    on_state_change: StateChangeCallback,
    cards_box: GtkBox,
    add_btn: Button,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    last_structure_sig: Rc<RefCell<Option<String>>>,
    wallpaper_path: Rc<RefCell<Option<PathBuf>>>,
    wallpaper_tex: Rc<RefCell<Option<gdk::Texture>>>,
    preview_cache: Rc<RefCell<HashMap<u64, gdk::Texture>>>,
    pending_slots: Rc<RefCell<Vec<PendingThumbSlot>>>,
    preview_dir: PathBuf,
    ipc: IpcCommandSender,
}

impl WorkspacesPanel {
    pub fn build(app: &Application, ipc: IpcCommandSender) -> Self {
        // 1. Fullscreen transparent click-catcher backdrop (clicking outside closes sidebar).
        let backdrop_window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Workspaces Backdrop")
            .decorated(false)
            .build();
        backdrop_window.add_css_class("neon-workspaces-window");
        if gtk4_layer_shell::is_supported() {
            backdrop_window.init_layer_shell();
            backdrop_window.set_namespace(Some("neon-workspaces-backdrop"));
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

        // 2. Full-height left-edge vertical sidebar anchored Left + Top + Bottom so
        //    neon-renderer blurs its exact 16px rounded frame and slides it smoothly from left.
        //    Do NOT set `.resizable(false)` — GTK4 requires resizable(true) to honor the
        //    compositor's Top+Bottom stretched layer-shell height instead of collapsing to minimum!
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Workspaces")
            .decorated(false)
            .default_width(WS_SIDEBAR_WIDTH)
            .build();
        window.add_css_class("neon-workspaces-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-workspaces-panel"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_margin(Edge::Left, 8);
            window.set_margin(Edge::Top, 38);
            window.set_margin(Edge::Bottom, 10);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let sidebar = GtkBox::new(Orientation::Vertical, 6);
        sidebar.add_css_class("neon-ws-sidebar");
        sidebar.set_size_request(WS_SIDEBAR_WIDTH, -1);
        sidebar.set_vexpand(true);
        sidebar.set_valign(Align::Fill);

        // Scrollable vertical stack of COSMIC-style workspace preview cards.
        let scroller = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .overlay_scrolling(true)
            .vexpand(true)
            .build();
        scroller.add_css_class("neon-ws-scroller");

        let cards_box = GtkBox::new(Orientation::Vertical, 4);
        cards_box.add_css_class("neon-ws-cards-list");
        cards_box.set_valign(Align::Start);
        scroller.set_child(Some(&cards_box));
        sidebar.append(&scroller);

        // Compact COSMIC-style "+ New Workspace" pill at bottom of sidebar (up to 10 workspaces).
        let add_btn = Button::new();
        add_btn.add_css_class("neon-ws-cosmic-add-btn");
        let add_row = GtkBox::new(Orientation::Horizontal, 6);
        add_row.set_halign(Align::Center);
        add_row.set_valign(Align::Center);
        let plus_icon = Image::from_icon_name("list-add-symbolic");
        plus_icon.set_pixel_size(14);
        let add_lbl = Label::new(Some("New Workspace"));
        add_row.append(&plus_icon);
        add_row.append(&add_lbl);
        add_btn.set_child(Some(&add_row));
        {
            let ipc_add = ipc.clone();
            add_btn.connect_clicked(move |_| {
                ipc_add.send(IpcRequest::AddWorkspace);
            });
        }
        sidebar.append(&add_btn);

        window.set_child(Some(&sidebar));
        window.set_visible(false);

        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let preview_dir = runtime_dir.join("neon-de").join("ws-previews");
        let _ = fs::create_dir_all(&preview_dir);

        let initial_wp_path = resolve_wallpaper_path(None, false);
        let initial_wp_tex = initial_wp_path
            .as_deref()
            .and_then(|p| load_scaled_texture(p, THUMB_WIDTH * 2, THUMB_HEIGHT * 2));

        let panel = Self {
            window,
            backdrop_window,
            is_open: Rc::new(Cell::new(false)),
            anim_gen: Rc::new(Cell::new(0u64)),
            preview_gen: Rc::new(Cell::new(0u64)),
            on_state_change: Rc::new(RefCell::new(None)),
            cards_box,
            add_btn,
            last_snapshot: Rc::new(RefCell::new(None)),
            last_structure_sig: Rc::new(RefCell::new(None)),
            wallpaper_path: Rc::new(RefCell::new(initial_wp_path)),
            wallpaper_tex: Rc::new(RefCell::new(initial_wp_tex)),
            preview_cache: Rc::new(RefCell::new(HashMap::new())),
            pending_slots: Rc::new(RefCell::new(Vec::new())),
            preview_dir,
            ipc,
        };

        // Click anywhere outside sidebar on backdrop closes smoothly.
        {
            let outside_click = GestureClick::new();
            outside_click.set_button(0);
            outside_click.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let p = panel.clone();
            outside_click.connect_pressed(move |_, _, _, _| {
                if p.is_open.get() {
                    p.close();
                }
            });
            panel.backdrop_window.add_controller(outside_click);
        }

        // Keyboard controls on sidebar & backdrop: Escape closes, Up/Down switches workspace.
        for w in [&panel.window, &panel.backdrop_window] {
            let key_ctrl = EventControllerKey::new();
            let p = panel.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if !p.is_open.get() {
                    return glib::Propagation::Proceed;
                }
                match key {
                    gdk::Key::Escape => {
                        p.close();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Up | gdk::Key::k => {
                        if let Some(ref snap) = *p.last_snapshot.borrow()
                            && snap.active_workspace_index > 0
                        {
                            p.ipc.send(IpcRequest::SwitchWorkspace {
                                index: snap.active_workspace_index - 1,
                            });
                        }
                        glib::Propagation::Stop
                    }
                    gdk::Key::Down | gdk::Key::j => {
                        if let Some(ref snap) = *p.last_snapshot.borrow()
                            && snap.active_workspace_index + 1 < snap.workspaces.len()
                        {
                            p.ipc.send(IpcRequest::SwitchWorkspace {
                                index: snap.active_workspace_index + 1,
                            });
                        }
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            w.add_controller(key_ctrl);
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
            *self.last_structure_sig.borrow_mut() = Some(compute_snapshot_sig(snap));
            // Render immediately using cached textures + wallpaper (0ms GPU readback cost)
            // so the 240ms left-edge slide-in stays locked at 60 FPS with zero stutter.
            self.rebuild(snap, false);
        }

        // Trigger compositor-driven 60 FPS GPU left-edge slide-in animation (starts cleanly
        // on the first frame GTK commits a real painted buffer via `PendingOpen`).
        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-workspaces-panel".into(),
            opening: true,
        });

        // Map click-catcher backdrop first, then left sidebar on top.
        self.backdrop_window.set_visible(true);
        self.backdrop_window.present();
        self.backdrop_window.queue_draw();
        self.window.set_visible(true);
        self.window.present();

        // Refresh live compositor thumbnail previews only after the 240ms slide-in settles.
        {
            let this = self.clone();
            let gen_cell = Rc::clone(&self.anim_gen);
            glib::timeout_add_local_once(Duration::from_millis(290), move || {
                if gen_cell.get() == epoch && this.is_open.get() {
                    let snap_opt = this.last_snapshot.borrow().clone();
                    if let Some(snap) = snap_opt {
                        this.rebuild(&snap, true);
                    }
                }
            });
        }
    }

    pub fn close(&self) {
        if !self.is_open.get() && !self.window.is_visible() {
            return;
        }
        let epoch = self.anim_gen.get().wrapping_add(1);
        self.anim_gen.set(epoch);
        self.preview_gen.set(self.preview_gen.get().wrapping_add(1));
        self.is_open.set(false);
        self.notify_state_change(false);

        // Immediately release the fullscreen click-catcher backdrop while the sidebar glides out.
        self.backdrop_window.set_visible(false);

        // Trigger smooth 60 FPS GPU slide-out back to left edge (175ms).
        self.ipc.send(IpcRequest::AnimateOverlay {
            namespace: "neon-workspaces-panel".into(),
            opening: false,
        });

        let win = self.window.clone();
        let backdrop = self.backdrop_window.clone();
        let gen_cell = Rc::clone(&self.anim_gen);
        glib::timeout_add_local_once(Duration::from_millis(195), move || {
            if gen_cell.get() == epoch {
                if gtk4_layer_shell::is_supported() {
                    win.set_keyboard_mode(KeyboardMode::None);
                }
                win.set_visible(false);
                backdrop.set_visible(false);
            }
        });
    }

    pub fn update(&self, snapshot: &DesktopSnapshot) {
        let wallpaper_changed = self.refresh_wallpaper_cache(snapshot);
        let sig = compute_snapshot_sig(snapshot);
        let changed = self.last_structure_sig.borrow().as_deref() != Some(sig.as_str());
        *self.last_snapshot.borrow_mut() = Some(snapshot.clone());
        if self.is_open.get() && (changed || wallpaper_changed) {
            *self.last_structure_sig.borrow_mut() = Some(sig);
            if wallpaper_changed {
                // Rebuild immediately with the new wallpaper fallback, then capture live GPU
                // thumbnails after the wallpaper layer commits its updated Wayland buffer.
                self.rebuild(snapshot, false);
                let this = self.clone();
                let snap_c = snapshot.clone();
                glib::timeout_add_local_once(Duration::from_millis(160), move || {
                    if this.is_open.get() {
                        this.rebuild(&snap_c, true);
                    }
                });
            } else {
                self.rebuild(snapshot, true);
            }
        }
    }

    fn refresh_wallpaper_cache(&self, snapshot: &DesktopSnapshot) -> bool {
        let is_light = snapshot.appearance.theme == "neon_light"
            || snapshot.appearance.theme == "light";
        let resolved = resolve_wallpaper_path(snapshot.appearance.wallpaper.as_deref(), is_light);
        if resolved != *self.wallpaper_path.borrow() {
            let tex = resolved
                .as_deref()
                .and_then(|p| load_scaled_texture(p, THUMB_WIDTH * 2, THUMB_HEIGHT * 2));
            *self.wallpaper_path.borrow_mut() = resolved;
            *self.wallpaper_tex.borrow_mut() = tex;
            self.preview_cache.borrow_mut().clear();
            return true;
        }
        false
    }

    fn notify_state_change(&self, open: bool) {
        if let Some(cb) = self.on_state_change.borrow().as_ref() {
            cb(open);
        }
    }

    fn rebuild(&self, snapshot: &DesktopSnapshot, request_live_previews: bool) {
        while let Some(child) = self.cards_box.first_child() {
            self.cards_box.remove(&child);
        }
        self.pending_slots.borrow_mut().clear();

        let total = snapshot.workspaces.len();
        let can_add = total < MAX_DESKTOPS;
        self.add_btn.set_visible(can_add);
        self.add_btn.set_sensitive(can_add);
        self.add_btn.set_tooltip_text(Some(&format!(
            "Add Workspace ({total} / {MAX_DESKTOPS})"
        )));

        let (out_w, out_h) = snapshot
            .outputs
            .iter()
            .find(|o| o.primary)
            .or_else(|| snapshot.outputs.first())
            .map(|o| (o.width.max(320), o.height.max(200)))
            .unwrap_or((1280, 800));

        let is_light = snapshot.appearance.theme == "neon_light"
            || snapshot.appearance.theme == "light";
        let accent_rgb = parse_hex_rgb(&snapshot.appearance.accent_color)
            .unwrap_or((0.04, 0.52, 1.0));

        // Prune preview cache entries for removed workspaces.
        {
            let live_ids: std::collections::HashSet<u64> =
                snapshot.workspaces.iter().map(|w| w.id).collect();
            self.preview_cache
                .borrow_mut()
                .retain(|id, _| live_ids.contains(id));
        }

        let gen_id = self.preview_gen.get().wrapping_add(1);
        self.preview_gen.set(gen_id);
        if request_live_previews {
            cleanup_stale_previews(&self.preview_dir);
        }

        let wp_tex = self.wallpaper_tex.borrow().clone();
        let mut capture_requests: Vec<(usize, PathBuf)> = Vec::new();

        for (pos, ws) in snapshot.workspaces.iter().enumerate() {
            // COSMIC-style crisp hairline divider between consecutive non-active workspace cards.
            if pos > 0 {
                let prev_active = snapshot.workspaces[pos - 1].active;
                if !prev_active && !ws.active {
                    let sep = Separator::new(Orientation::Horizontal);
                    sep.add_css_class("neon-ws-card-sep");
                    self.cards_box.append(&sep);
                }
            }

            let ws_windows: Vec<WindowInfo> = snapshot
                .windows
                .iter()
                .filter(|w| w.workspace_id == ws.id && !w.minimized)
                .cloned()
                .collect();

            let (card, slot_opt) = self.build_workspace_card(
                ws,
                total,
                &ws_windows,
                out_w,
                out_h,
                is_light,
                accent_rgb,
                wp_tex.as_ref(),
                gen_id,
                request_live_previews,
            );

            if let Some((slot, idx, path)) = slot_opt {
                self.pending_slots.borrow_mut().push(slot);
                capture_requests.push((idx, path));
            }

            self.cards_box.append(&card);
        }

        if request_live_previews && !capture_requests.is_empty() {
            for (ws_index, path) in capture_requests {
                self.ipc.send(IpcRequest::CaptureScreencastPreview {
                    target: ScreencastTarget::Workspace { index: ws_index },
                    path: path.to_string_lossy().into_owned(),
                    max_width: Some((THUMB_WIDTH * 2) as u32),
                    max_height: Some((THUMB_HEIGHT * 2) as u32),
                });
            }
            self.start_preview_poller(gen_id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_workspace_card(
        &self,
        ws: &WorkspaceInfo,
        total_workspaces: usize,
        ws_windows: &[WindowInfo],
        out_w: i32,
        out_h: i32,
        is_light: bool,
        accent_rgb: (f64, f64, f64),
        wp_tex: Option<&gdk::Texture>,
        gen_id: u64,
        request_live_preview: bool,
    ) -> (GtkBox, Option<(PendingThumbSlot, usize, PathBuf)>) {
        let card = GtkBox::new(Orientation::Vertical, 6);
        card.add_css_class("neon-ws-cosmic-card");
        if ws.active {
            card.add_css_class("active");
        }

        // Rich tooltip showing shortcut hint + open apps on this workspace.
        let shortcut_num = if ws.index < 9 {
            format!("{}", ws.index + 1)
        } else {
            "0".to_string()
        };
        let tooltip = if ws_windows.is_empty() {
            format!("Workspace {} · Empty · Super+{shortcut_num}", ws.index + 1)
        } else {
            let names: Vec<&str> = ws_windows
                .iter()
                .take(3)
                .filter_map(|w| w.title.as_deref().or(w.app_id.as_deref()))
                .collect();
            let noun = if ws_windows.len() == 1 {
                "window"
            } else {
                "windows"
            };
            format!(
                "Workspace {} · {} {noun} ({}) · Super+{shortcut_num}",
                ws.index + 1,
                ws_windows.len(),
                names.join(", ")
            )
        };
        card.set_tooltip_text(Some(&tooltip));

        // ── Preview Thumbnail (16:10 rounded frame with live GPU capture + instant fallback) ──
        let thumb_frame = GtkBox::new(Orientation::Vertical, 0);
        thumb_frame.add_css_class("neon-ws-thumb-frame");
        thumb_frame.set_size_request(THUMB_WIDTH, THUMB_HEIGHT);
        thumb_frame.set_overflow(gtk4::Overflow::Hidden);

        let overlay = Overlay::new();
        overlay.set_size_request(THUMB_WIDTH, THUMB_HEIGHT);

        let cached_tex = self.preview_cache.borrow().get(&ws.id).cloned();
        let has_cached = cached_tex.is_some();

        let pic = Picture::new();
        pic.set_content_fit(ContentFit::Cover);
        pic.set_can_shrink(true);
        pic.set_size_request(THUMB_WIDTH, THUMB_HEIGHT);
        pic.set_hexpand(true);
        pic.set_vexpand(true);
        if let Some(ref tex) = cached_tex {
            pic.set_paintable(Some(tex));
        } else if let Some(wp) = wp_tex {
            pic.set_paintable(Some(wp));
        }
        overlay.set_child(Some(&pic));

        // Instant proportional window miniature overlay shown until the live GPU screenshot loads.
        let mini_windows = DrawingArea::new();
        mini_windows.set_content_width(THUMB_WIDTH);
        mini_windows.set_content_height(THUMB_HEIGHT);
        mini_windows.set_can_target(false);
        mini_windows.set_visible(!has_cached);
        {
            let wins_c = ws_windows.to_vec();
            let ow = f64::from(out_w.max(1));
            let oh = f64::from(out_h.max(1));
            mini_windows.set_draw_func(move |_, cr, w, h| {
                let wf = f64::from(w);
                let hf = f64::from(h);

                // Subtle miniature top bar strip
                cr.set_source_rgba(0.06, 0.08, 0.13, 0.62);
                cr.rectangle(0.0, 0.0, wf, 5.0);
                let _ = cr.fill();

                // Proportional miniature window rectangles
                for win in &wins_c {
                    let rx = (f64::from(win.x) / ow * wf).clamp(2.0, wf - 14.0);
                    let ry = (f64::from(win.y) / oh * hf).clamp(6.0, hf - 12.0);
                    let rw = (f64::from(win.width) / ow * wf).clamp(18.0, wf - rx - 2.0);
                    let rh = (f64::from(win.height) / oh * hf).clamp(12.0, hf - ry - 4.0);

                    draw_rounded_rect(cr, rx, ry, rw, rh, 3.0);
                    if is_light {
                        cr.set_source_rgba(0.95, 0.97, 1.0, 0.90);
                    } else {
                        cr.set_source_rgba(0.14, 0.17, 0.24, 0.92);
                    }
                    let _ = cr.fill_preserve();

                    if win.focused {
                        cr.set_source_rgba(accent_rgb.0, accent_rgb.1, accent_rgb.2, 0.92);
                        cr.set_line_width(1.2);
                    } else {
                        cr.set_source_rgba(1.0, 1.0, 1.0, 0.26);
                        cr.set_line_width(0.8);
                    }
                    let _ = cr.stroke();

                    // Tiny titlebar accent bar inside miniature window
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.16);
                    cr.rectangle(rx + 1.5, ry + 1.5, (rw - 3.0).max(0.0), 3.0);
                    let _ = cr.fill();
                }

                // Subtle miniature dock pill at bottom center
                let dock_w = (wf * 0.34).max(34.0);
                let dock_h = 5.0;
                draw_rounded_rect(cr, (wf - dock_w) * 0.5, hf - dock_h - 3.0, dock_w, dock_h, 2.5);
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.24);
                let _ = cr.fill();
            });
        }
        overlay.add_overlay(&mini_windows);

        // Discreet top-right remove button (only when >1 workspace exists) so users can still
        // remove extra desktops cleanly while preserving the authentic COSMIC footer.
        if total_workspaces > 1 {
            let rm_btn = Button::with_label("×");
            rm_btn.add_css_class("neon-ws-thumb-close");
            rm_btn.set_halign(Align::End);
            rm_btn.set_valign(Align::Start);
            rm_btn.set_margin_top(4);
            rm_btn.set_margin_end(4);
            rm_btn.set_tooltip_text(Some("Remove Workspace"));
            let idx = ws.index;
            let ipc_rm = self.ipc.clone();
            rm_btn.connect_clicked(move |_| {
                ipc_rm.send(IpcRequest::RemoveWorkspace { index: idx });
            });
            overlay.add_overlay(&rm_btn);
        }

        thumb_frame.append(&overlay);
        card.append(&thumb_frame);

        // ── COSMIC Footer Row: centered "Workspace N" label + right circular Pin toggle ──
        let footer = CenterBox::new();
        footer.add_css_class("neon-ws-cosmic-footer");
        footer.set_hexpand(true);

        // Left balancing spacer so "Workspace N" stays optically centered like COSMIC Desktop.
        let left_spacer = GtkBox::new(Orientation::Horizontal, 0);
        left_spacer.set_size_request(22, 22);
        footer.set_start_widget(Some(&left_spacer));

        let name_lbl = Label::new(Some(&format!("Workspace {}", ws.index + 1)));
        name_lbl.add_css_class("neon-ws-cosmic-label");
        if ws.active {
            name_lbl.add_css_class("active");
        }
        footer.set_center_widget(Some(&name_lbl));

        let pin_btn = Button::new();
        pin_btn.add_css_class("neon-ws-pin-btn");
        if ws.pinned {
            pin_btn.add_css_class("pinned");
            pin_btn.set_tooltip_text(Some("Pinned workspace (click to unpin)"));
        } else {
            pin_btn.set_tooltip_text(Some("Pin workspace (keep even when empty)"));
        }
        let pin_icon = build_pin_icon_drawing_area(ws.pinned, is_light, accent_rgb);
        pin_btn.set_child(Some(&pin_icon));
        {
            let idx = ws.index;
            let ipc_pin = self.ipc.clone();
            pin_btn.connect_clicked(move |_| {
                ipc_pin.send(IpcRequest::TogglePinWorkspace { index: idx });
            });
        }
        footer.set_end_widget(Some(&pin_btn));
        card.append(&footer);

        // Clicking anywhere on the card body (except Pin / Remove buttons) switches workspace.
        let click = GestureClick::new();
        let idx = ws.index;
        let ipc_sw = self.ipc.clone();
        let p_close = self.clone();
        click.connect_released(move |gesture, _, x, y| {
            let on_button = gesture
                .widget()
                .and_then(|w| w.pick(x, y, gtk4::PickFlags::DEFAULT))
                .and_then(|picked| picked.ancestor(Button::static_type()))
                .is_some();
            if !on_button {
                ipc_sw.send(IpcRequest::SwitchWorkspace { index: idx });
                p_close.close();
            }
        });
        card.add_controller(click);

        let slot_opt = if request_live_preview {
            let path = self
                .preview_dir
                .join(format!("ws-{}-{gen_id}.png", ws.id));
            Some((
                PendingThumbSlot {
                    ws_id: ws.id,
                    path: path.clone(),
                    picture: pic,
                    fallback_overlay: mini_windows,
                    loaded: false,
                },
                ws.index,
                path,
            ))
        } else {
            None
        };

        (card, slot_opt)
    }

    fn start_preview_poller(&self, gen_id: u64) {
        let slots_c = Rc::clone(&self.pending_slots);
        let cache_c = Rc::clone(&self.preview_cache);
        let gen_c = Rc::clone(&self.preview_gen);
        let open_c = Rc::clone(&self.is_open);
        let started = Instant::now();

        glib::timeout_add_local(Duration::from_millis(35), move || {
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
                    slot.fallback_overlay.set_visible(false);
                    cache_c.borrow_mut().insert(slot.ws_id, tex);
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

/// Draws a crisp COSMIC-style diagonal thumbtack pin icon using Cairo so it never depends on
/// third-party icon themes. Automatically chooses dark ink inside bright accent circles (matching
/// COSMIC's high-contrast pinned badge) and light ink on dark accents.
fn build_pin_icon_drawing_area(
    pinned: bool,
    is_light: bool,
    accent_rgb: (f64, f64, f64),
) -> DrawingArea {
    let area = DrawingArea::new();
    area.set_content_width(14);
    area.set_content_height(14);
    area.set_halign(Align::Center);
    area.set_valign(Align::Center);
    let accent_luma = 0.299 * accent_rgb.0 + 0.587 * accent_rgb.1 + 0.114 * accent_rgb.2;
    area.set_draw_func(move |_, cr, w, h| {
        let wf = f64::from(w);
        let hf = f64::from(h);
        cr.save().ok();
        cr.translate(wf * 0.5, hf * 0.5);
        cr.rotate(std::f64::consts::FRAC_PI_4);

        if pinned {
            if accent_luma > 0.42 {
                // Dark crisp pin inside bright/pastel accent circle (exact COSMIC style)
                cr.set_source_rgba(0.07, 0.08, 0.12, 0.96);
            } else {
                // White crisp pin inside deep/dark accent circle
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
            }
        } else if is_light {
            cr.set_source_rgba(0.12, 0.16, 0.24, 0.80);
        } else {
            cr.set_source_rgba(0.95, 0.97, 1.0, 0.88);
        }

        // Pin head cap
        draw_rounded_rect(cr, -2.6, -4.8, 5.2, 1.6, 0.7);
        let _ = cr.fill();

        // Tapered pin body
        cr.move_to(-1.7, -3.2);
        cr.line_to(1.7, -3.2);
        cr.line_to(2.4, 0.6);
        cr.line_to(-2.4, 0.6);
        cr.close_path();
        let _ = cr.fill();

        // Base flange
        draw_rounded_rect(cr, -3.4, 0.4, 6.8, 1.3, 0.6);
        let _ = cr.fill();

        // Sharp needle
        cr.move_to(-0.65, 1.6);
        cr.line_to(0.65, 1.6);
        cr.line_to(0.0, 5.1);
        cr.close_path();
        let _ = cr.fill();

        cr.restore().ok();
    });
    area
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
        "{}|{}|{}|{}|",
        snapshot.active_workspace_index,
        snapshot.appearance.theme,
        snapshot.appearance.accent_color,
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
