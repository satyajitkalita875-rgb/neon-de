//! GNOME 47-Inspired Interactive Screenshot & Screen Record Overlay (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Features:
//! - Drag-to-select, drag-to-move, and corner-resize selection box with translucent dimmed surround,
//!   crystal-clear cutout, circular corner & edge handles, and rounded `W × H` pill badge
//! - Interactive **Window** picker: click any visible window on screen to select it
//! - **Screenshot (Photo)** vs **Screen Record (Video)** capture mode switch right beside the
//!   signature GNOME round shutter/record button, plus optional PipeWire audio capture toggle
//! - Live screen recording powered by `wf-recorder` (`zwlr_screencopy_manager_v1`), saving clean
//!   H.264 MP4s to `~/Videos/Screencasts/`, paired with a pulsing red Top Bar stop indicator

use std::{
    cell::RefCell,
    collections::HashSet,
    f64::consts::TAU,
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, DrawingArea, EventControllerKey,
    GestureClick, GestureDrag, Image, Label, Orientation, Overlay, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest, WindowInfo};
use neon_system::notifications::{NotificationStore, NotificationUrgency};
use tracing::{info, warn};

use crate::ipc_bridge::IpcCommandSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Selection,
    Screen,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureKind {
    Screenshot,
    Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragAction {
    None,
    NewSelection,
    MoveBox,
    ResizeTL,
    ResizeTR,
    ResizeBL,
    ResizeBR,
}

#[derive(Debug, Clone, Copy)]
struct SelState {
    mode: CaptureMode,
    kind: CaptureKind,
    record_audio: bool,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    drag_action: DragAction,
    drag_start_x: f64,
    drag_start_y: f64,
    orig_x0: f64,
    orig_y0: f64,
    orig_x1: f64,
    orig_y1: f64,
    selected_window_id: Option<u64>,
}

impl SelState {
    fn rect(&self) -> (f64, f64, f64, f64) {
        let rx = self.x0.min(self.x1);
        let ry = self.y0.min(self.y1);
        let rw = (self.x1 - self.x0).abs().max(32.0);
        let rh = (self.y1 - self.y0).abs().max(32.0);
        (rx, ry, rw, rh)
    }

    fn normalize(&mut self) {
        let (rx, ry, rw, rh) = self.rect();
        self.x0 = rx;
        self.y0 = ry;
        self.x1 = rx + rw;
        self.y1 = ry + rh;
    }
}

struct ActiveRecording {
    child: Child,
    path: PathBuf,
    started_at: Instant,
}

/// Interactive GNOME 47-style Screenshot & Screen Record overlay.
#[derive(Clone)]
pub struct ScreenshotOverlay {
    window: ApplicationWindow,
    drawing_area: DrawingArea,
    state: Rc<RefCell<SelState>>,
    mode_sel_btn: Button,
    mode_scr_btn: Button,
    mode_win_btn: Button,
    kind_photo_btn: Button,
    kind_video_btn: Button,
    audio_toggle_btn: Button,
    shutter_btn: Button,
    shutter_icon: Image,
    stop_rec_btn: Button,
    hint_label: Label,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    recording: Rc<RefCell<Option<ActiveRecording>>>,
    notif_store: Arc<NotificationStore>,
    ipc: IpcCommandSender,
}

impl ScreenshotOverlay {
    pub fn build(
        app: &Application,
        ipc: IpcCommandSender,
        notif_store: Arc<NotificationStore>,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Screenshot & Screen Record")
            .decorated(false)
            .build();
        window.add_css_class("neon-screenshot-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-screenshot-ui"));
            window.set_layer(Layer::Overlay);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let state = Rc::new(RefCell::new(SelState {
            mode: CaptureMode::Selection,
            kind: CaptureKind::Screenshot,
            record_audio: false,
            x0: 360.0,
            y0: 200.0,
            x1: 1160.0,
            y1: 720.0,
            drag_action: DragAction::None,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            orig_x0: 360.0,
            orig_y0: 200.0,
            orig_x1: 1160.0,
            orig_y1: 720.0,
            selected_window_id: None,
        }));
        let last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>> = Rc::new(RefCell::new(None));
        let recording: Rc<RefCell<Option<ActiveRecording>>> = Rc::new(RefCell::new(None));

        let overlay = Overlay::new();

        // Fullscreen Cairo canvas drawing dimmed surround + crisp selection/window cutout
        let drawing_area = DrawingArea::new();
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        {
            let st = Rc::clone(&state);
            let snap_ref = Rc::clone(&last_snapshot);
            drawing_area.set_draw_func(move |_, cr, w, h| {
                let wf = f64::from(w);
                let hf = f64::from(h);
                let cur = *st.borrow();
                let is_record = cur.kind == CaptureKind::Record;

                match cur.mode {
                    CaptureMode::Screen => {
                        cr.set_source_rgba(0.0, 0.0, 0.0, 0.16);
                        cr.rectangle(0.0, 0.0, wf, hf);
                        let _ = cr.fill();

                        if is_record {
                            cr.set_source_rgba(0.95, 0.30, 0.30, 0.90);
                        } else {
                            cr.set_source_rgba(1.0, 1.0, 1.0, 0.88);
                        }
                        cr.set_line_width(4.0);
                        cr.rectangle(2.0, 2.0, wf - 4.0, hf - 4.0);
                        let _ = cr.stroke();

                        let badge = if is_record {
                            format!("REC  Full Screen  {w} x {h}")
                        } else {
                            format!("Full Screen  {w} x {h}")
                        };
                        draw_pill_badge(cr, (wf - 220.0) * 0.5, 46.0, &badge, is_record);
                    }
                    CaptureMode::Selection => {
                        let (rx, ry, rw, rh) = cur.rect();
                        let label = if is_record {
                            let ew = ((rw.round() as i32) & !1).max(2);
                            let eh = ((rh.round() as i32) & !1).max(2);
                            format!("REC  {ew} x {eh}")
                        } else {
                            format!("{:.0} x {:.0}", rw, rh)
                        };
                        draw_cutout_mask(cr, wf, hf, rx, ry, rw, rh, true, is_record, &label);
                    }
                    CaptureMode::Window => {
                        if let Some((wx, wy, ww, wh, title)) =
                            selected_window_rect(&snap_ref, cur.selected_window_id)
                        {
                            let short: String = title.chars().take(24).collect();
                            let label = if is_record {
                                format!("REC  {short}")
                            } else {
                                format!("{short}  {:.0}x{:.0}", ww, wh)
                            };
                            draw_cutout_mask(cr, wf, hf, wx, wy, ww, wh, false, is_record, &label);
                        } else {
                            cr.set_source_rgba(0.0, 0.0, 0.0, 0.38);
                            cr.rectangle(0.0, 0.0, wf, hf);
                            let _ = cr.fill();
                            draw_pill_badge(
                                cr,
                                (wf - 200.0) * 0.5,
                                46.0,
                                "No open window to capture",
                                false,
                            );
                        }
                    }
                }
            });
        }

        // Interactive drag gesture: resize corners, move box, or draw fresh selection
        let drag = GestureDrag::new();
        {
            let st = Rc::clone(&state);
            let da = drawing_area.clone();
            drag.connect_drag_begin(move |_, x, y| {
                let mut s = st.borrow_mut();
                if s.mode != CaptureMode::Selection {
                    return;
                }
                s.normalize();
                let (rx, ry, rw, rh) = s.rect();
                let handle_r = 20.0;
                let near = |hx: f64, hy: f64| ((x - hx).hypot(y - hy)) <= handle_r;

                s.drag_start_x = x;
                s.drag_start_y = y;
                s.orig_x0 = rx;
                s.orig_y0 = ry;
                s.orig_x1 = rx + rw;
                s.orig_y1 = ry + rh;

                if near(rx, ry) {
                    s.drag_action = DragAction::ResizeTL;
                } else if near(rx + rw, ry) {
                    s.drag_action = DragAction::ResizeTR;
                } else if near(rx, ry + rh) {
                    s.drag_action = DragAction::ResizeBL;
                } else if near(rx + rw, ry + rh) {
                    s.drag_action = DragAction::ResizeBR;
                } else if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
                    s.drag_action = DragAction::MoveBox;
                } else {
                    s.drag_action = DragAction::NewSelection;
                    s.x0 = x;
                    s.y0 = y;
                    s.x1 = x + 2.0;
                    s.y1 = y + 2.0;
                }
                da.queue_draw();
            });
        }
        {
            let st = Rc::clone(&state);
            let da = drawing_area.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let mut s = st.borrow_mut();
                if s.mode != CaptureMode::Selection {
                    return;
                }
                let max_w = f64::from(da.width().max(640));
                let max_h = f64::from(da.height().max(480));
                match s.drag_action {
                    DragAction::None => return,
                    DragAction::NewSelection => {
                        s.x1 = (s.drag_start_x + dx).clamp(0.0, max_w);
                        s.y1 = (s.drag_start_y + dy).clamp(0.0, max_h);
                    }
                    DragAction::MoveBox => {
                        let bw = s.orig_x1 - s.orig_x0;
                        let bh = s.orig_y1 - s.orig_y0;
                        let nx = (s.orig_x0 + dx).clamp(0.0, (max_w - bw).max(0.0));
                        let ny = (s.orig_y0 + dy).clamp(0.0, (max_h - bh).max(0.0));
                        s.x0 = nx;
                        s.y0 = ny;
                        s.x1 = nx + bw;
                        s.y1 = ny + bh;
                    }
                    DragAction::ResizeTL => {
                        s.x0 = (s.orig_x0 + dx).clamp(0.0, s.orig_x1 - 32.0);
                        s.y0 = (s.orig_y0 + dy).clamp(0.0, s.orig_y1 - 32.0);
                    }
                    DragAction::ResizeTR => {
                        s.x1 = (s.orig_x1 + dx).clamp(s.orig_x0 + 32.0, max_w);
                        s.y0 = (s.orig_y0 + dy).clamp(0.0, s.orig_y1 - 32.0);
                    }
                    DragAction::ResizeBL => {
                        s.x0 = (s.orig_x0 + dx).clamp(0.0, s.orig_x1 - 32.0);
                        s.y1 = (s.orig_y1 + dy).clamp(s.orig_y0 + 32.0, max_h);
                    }
                    DragAction::ResizeBR => {
                        s.x1 = (s.orig_x1 + dx).clamp(s.orig_x0 + 32.0, max_w);
                        s.y1 = (s.orig_y1 + dy).clamp(s.orig_y0 + 32.0, max_h);
                    }
                }
                da.queue_draw();
            });
        }
        {
            let st = Rc::clone(&state);
            drag.connect_drag_end(move |_, _, _| {
                let mut s = st.borrow_mut();
                s.normalize();
                s.drag_action = DragAction::None;
            });
        }
        drawing_area.add_controller(drag);

        // Click on any visible window in Window mode to select it
        let click_pick = GestureClick::new();
        {
            let st = Rc::clone(&state);
            let snap_ref = Rc::clone(&last_snapshot);
            let da = drawing_area.clone();
            click_pick.connect_pressed(move |_, _, x, y| {
                let mode = st.borrow().mode;
                if mode != CaptureMode::Window {
                    return;
                }
                if let Some(win_id) = pick_window_at(&snap_ref, x, y) {
                    st.borrow_mut().selected_window_id = Some(win_id);
                    da.queue_draw();
                }
            });
        }
        drawing_area.add_controller(click_pick);
        overlay.set_child(Some(&drawing_area));

        // =========================================================================
        // GNOME 47 Signature Floating Bottom Pill Toolbar
        // =========================================================================
        let toolbar = GtkBox::new(Orientation::Vertical, 12);
        toolbar.add_css_class("neon-gnome-shot-bar");
        toolbar.set_halign(Align::Center);
        toolbar.set_valign(Align::End);
        toolbar.set_margin_bottom(38);

        // Top row: Selection | Screen | Window  +  Audio toggle (Record mode)  +  Close
        let mode_row = GtkBox::new(Orientation::Horizontal, 6);
        mode_row.set_halign(Align::Center);
        mode_row.set_valign(Align::Center);

        let mode_sel_btn = make_mode_btn("select-rectangular-symbolic", "Selection");
        mode_sel_btn.add_css_class("active");
        let mode_scr_btn = make_mode_btn("video-display-symbolic", "Screen");
        let mode_win_btn = make_mode_btn("window-new-symbolic", "Window");

        let audio_toggle_btn = make_mode_btn("audio-input-microphone-symbolic", "Audio");
        audio_toggle_btn.add_css_class("neon-shot-toggle-btn");
        audio_toggle_btn.set_tooltip_text(Some("Include system / microphone audio in screen recording"));
        audio_toggle_btn.set_visible(false);

        let cancel_btn = Button::with_label("×");
        cancel_btn.add_css_class("neon-close-button");
        cancel_btn.set_tooltip_text(Some("Cancel (Esc)"));

        mode_row.append(&mode_sel_btn);
        mode_row.append(&mode_scr_btn);
        mode_row.append(&mode_win_btn);
        mode_row.append(&audio_toggle_btn);
        mode_row.append(&cancel_btn);

        // Middle row: Round Shutter/Record Button + Photo/Video Kind Switcher
        let action_row = GtkBox::new(Orientation::Horizontal, 16);
        action_row.set_halign(Align::Center);
        action_row.set_valign(Align::Center);

        let shutter_btn = Button::new();
        shutter_btn.add_css_class("neon-shot-shutter");
        shutter_btn.set_halign(Align::Center);
        shutter_btn.set_valign(Align::Center);
        shutter_btn.set_tooltip_text(Some("Take Screenshot (Enter)"));
        let shutter_icon = Image::from_icon_name("camera-photo-symbolic");
        shutter_icon.set_pixel_size(24);
        shutter_btn.set_child(Some(&shutter_icon));

        let kind_track = GtkBox::new(Orientation::Horizontal, 3);
        kind_track.add_css_class("neon-shot-kind-track");
        kind_track.set_valign(Align::Center);

        let kind_photo_btn = make_kind_btn("camera-photo-symbolic", "Screenshot");
        kind_photo_btn.add_css_class("active");
        let kind_video_btn = make_kind_btn("camera-video-symbolic", "Record");
        kind_track.append(&kind_photo_btn);
        kind_track.append(&kind_video_btn);

        let stop_rec_btn = Button::with_label("■  Stop Recording");
        stop_rec_btn.add_css_class("neon-shot-stop-btn");
        stop_rec_btn.set_visible(false);

        action_row.append(&shutter_btn);
        action_row.append(&kind_track);
        action_row.append(&stop_rec_btn);

        let hint_label = Label::new(Some(
            "Drag to select or resize · Enter to capture · Esc to cancel",
        ));
        hint_label.add_css_class("neon-qs-muted");
        hint_label.set_halign(Align::Center);

        toolbar.append(&mode_row);
        toolbar.append(&action_row);
        toolbar.append(&hint_label);
        overlay.add_overlay(&toolbar);

        window.set_child(Some(&overlay));
        window.set_visible(false);

        let shot = Self {
            window,
            drawing_area,
            state,
            mode_sel_btn,
            mode_scr_btn,
            mode_win_btn,
            kind_photo_btn,
            kind_video_btn,
            audio_toggle_btn,
            shutter_btn,
            shutter_icon,
            stop_rec_btn,
            hint_label,
            last_snapshot,
            recording,
            notif_store,
            ipc,
        };

        // Wire Selection / Screen / Window mode buttons
        {
            let this = shot.clone();
            shot.mode_sel_btn
                .connect_clicked(move |_| this.set_mode(CaptureMode::Selection));
        }
        {
            let this = shot.clone();
            shot.mode_scr_btn
                .connect_clicked(move |_| this.set_mode(CaptureMode::Screen));
        }
        {
            let this = shot.clone();
            shot.mode_win_btn
                .connect_clicked(move |_| this.set_mode(CaptureMode::Window));
        }

        // Wire Photo / Video capture kind switch
        {
            let this = shot.clone();
            shot.kind_photo_btn
                .connect_clicked(move |_| this.set_kind(CaptureKind::Screenshot));
        }
        {
            let this = shot.clone();
            shot.kind_video_btn
                .connect_clicked(move |_| this.set_kind(CaptureKind::Record));
        }

        // Wire Audio recording toggle
        {
            let this = shot.clone();
            shot.audio_toggle_btn.connect_clicked(move |_| {
                let next = {
                    let mut s = this.state.borrow_mut();
                    s.record_audio = !s.record_audio;
                    s.record_audio
                };
                if next {
                    this.audio_toggle_btn.add_css_class("active");
                } else {
                    this.audio_toggle_btn.remove_css_class("active");
                }
            });
        }

        {
            let this = shot.clone();
            cancel_btn.connect_clicked(move |_| this.close());
        }
        {
            let this = shot.clone();
            shot.shutter_btn.connect_clicked(move |_| this.execute_capture());
        }
        {
            let this = shot.clone();
            shot.stop_rec_btn.connect_clicked(move |_| {
                this.stop_recording();
                this.close();
            });
        }

        // Keyboard shortcuts: Esc cancels, Enter/Space captures or records
        let key_ctrl = EventControllerKey::new();
        {
            let this = shot.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    this.close();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Return || key == gdk::Key::space {
                    this.execute_capture();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        shot.window.add_controller(key_ctrl);

        shot
    }

    pub fn update_snapshot(&self, snap: &DesktopSnapshot) {
        *self.last_snapshot.borrow_mut() = Some(snap.clone());
    }

    pub fn open(&self) {
        if let Some(snap) = self.last_snapshot.borrow().as_ref() {
            if let Some(out) = snap.outputs.iter().find(|o| o.primary).or(snap.outputs.first()) {
                let w = out.width as f64;
                let h = out.height as f64;
                let mut s = self.state.borrow_mut();
                s.x0 = w * 0.20;
                s.y0 = h * 0.18;
                s.x1 = w * 0.80;
                s.y1 = h * 0.72;
            }
            let focused_id = snap
                .windows
                .iter()
                .find(|w| w.focused && !w.minimized)
                .or_else(|| snap.windows.iter().find(|w| !w.minimized))
                .map(|w| w.id);
            self.state.borrow_mut().selected_window_id = focused_id;
        }

        let is_rec = self.recording.borrow().is_some();
        self.stop_rec_btn.set_visible(is_rec);

        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        }
        self.window.set_visible(true);
        self.window.present();
        self.drawing_area.queue_draw();
    }

    pub fn close(&self) {
        if gtk4_layer_shell::is_supported() {
            self.window.set_keyboard_mode(KeyboardMode::None);
        }
        self.window.set_visible(false);
    }

    /// Switches programmatically to Screen Record mode (used for verification).
    pub fn set_record_mode_for_test(&self) {
        self.set_kind(CaptureKind::Record);
    }

    /// Polls live recording state; returns `Some(elapsed_secs)` while `wf-recorder` is running.
    pub fn poll_recording_elapsed(&self) -> Option<u64> {
        let mut guard = self.recording.borrow_mut();
        let rec = guard.as_mut()?;
        match rec.child.try_wait() {
            Ok(Some(status)) => {
                let path = rec.path.clone();
                *guard = None;
                drop(guard);
                if status.success() && path.exists() {
                    self.notify_recording_saved(&path);
                } else {
                    warn!(?status, path = %path.display(), "wf-recorder exited early");
                }
                None
            }
            Ok(None) => Some(rec.started_at.elapsed().as_secs()),
            Err(_) => {
                *guard = None;
                None
            }
        }
    }

    /// Gracefully stops the active `wf-recorder` screen recording (`SIGINT`) so the MP4 container
    /// finalizes cleanly, then posts a desktop notification with the saved file path.
    pub fn stop_recording(&self) {
        let Some(mut rec) = self.recording.borrow_mut().take() else {
            return;
        };
        let pid = rec.child.id();
        info!(pid, path = %rec.path.display(), "Stopping NEON DE screen recording (SIGINT)");
        let _ = Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status();

        let path = rec.path.clone();
        let store = Arc::clone(&self.notif_store);
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(4);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = rec.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(60));
            }
            let _ = rec.child.kill();
            let _ = rec.child.wait();
            if path.exists() {
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Screencast.mp4");
                store.push(
                    0,
                    "NEON Screen Recorder".into(),
                    "camera-video-symbolic".into(),
                    "Screen Recording Saved".into(),
                    format!("{file_name}\n{}", path.display()),
                    NotificationUrgency::Normal,
                    5000,
                );
            }
        });
    }

    fn notify_recording_saved(&self, path: &PathBuf) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Screencast.mp4");
        self.notif_store.push(
            0,
            "NEON Screen Recorder".into(),
            "camera-video-symbolic".into(),
            "Screen Recording Saved".into(),
            format!("{file_name}\n{}", path.display()),
            NotificationUrgency::Normal,
            5000,
        );
    }

    fn set_mode(&self, mode: CaptureMode) {
        self.state.borrow_mut().mode = mode;
        for (btn, target) in [
            (&self.mode_sel_btn, CaptureMode::Selection),
            (&self.mode_scr_btn, CaptureMode::Screen),
            (&self.mode_win_btn, CaptureMode::Window),
        ] {
            if mode == target {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }
        self.refresh_hint();
        self.drawing_area.queue_draw();
    }

    fn set_kind(&self, kind: CaptureKind) {
        self.state.borrow_mut().kind = kind;
        let is_record = kind == CaptureKind::Record;
        if is_record {
            self.kind_photo_btn.remove_css_class("active");
            self.kind_video_btn.add_css_class("active");
            self.shutter_btn.add_css_class("record");
            self.shutter_icon
                .set_icon_name(Some("media-record-symbolic"));
            self.shutter_btn
                .set_tooltip_text(Some("Start Screen Recording (Enter)"));
            self.audio_toggle_btn.set_visible(true);
        } else {
            self.kind_video_btn.remove_css_class("active");
            self.kind_photo_btn.add_css_class("active");
            self.shutter_btn.remove_css_class("record");
            self.shutter_icon
                .set_icon_name(Some("camera-photo-symbolic"));
            self.shutter_btn
                .set_tooltip_text(Some("Take Screenshot (Enter)"));
            self.audio_toggle_btn.set_visible(false);
        }
        self.refresh_hint();
        self.drawing_area.queue_draw();
    }

    fn refresh_hint(&self) {
        let cur = *self.state.borrow();
        let text = match (cur.kind, cur.mode) {
            (CaptureKind::Screenshot, CaptureMode::Selection) => {
                "Drag corners to resize or move selection · Enter to capture · Esc to cancel"
            }
            (CaptureKind::Screenshot, CaptureMode::Screen) => {
                "Captures entire screen · Enter to capture · Esc to cancel"
            }
            (CaptureKind::Screenshot, CaptureMode::Window) => {
                "Click any window to select it · Enter to capture · Esc to cancel"
            }
            (CaptureKind::Record, CaptureMode::Selection) => {
                "Select region to record MP4 · Enter to start · Stop from Top Bar"
            }
            (CaptureKind::Record, CaptureMode::Screen) => {
                "Records full screen to ~/Videos/Screencasts · Enter to start"
            }
            (CaptureKind::Record, CaptureMode::Window) => {
                "Click any window to record its area · Enter to start"
            }
        };
        self.hint_label.set_text(text);
    }

    fn current_region(&self) -> Option<[i32; 4]> {
        let cur = *self.state.borrow();
        match cur.mode {
            CaptureMode::Screen => None,
            CaptureMode::Selection => {
                let (x, y, w, h) = cur.rect();
                Some([
                    x.round() as i32,
                    y.round() as i32,
                    w.round() as i32,
                    h.round() as i32,
                ])
            }
            CaptureMode::Window => {
                selected_window_rect(&self.last_snapshot, cur.selected_window_id).map(
                    |(x, y, w, h, _)| {
                        [
                            x.round() as i32,
                            y.round() as i32,
                            w.round() as i32,
                            h.round() as i32,
                        ]
                    },
                )
            }
        }
    }

    fn execute_capture(&self) {
        let cur = *self.state.borrow();
        let region = self.current_region();

        // Hide the overlay before screenshot or starting video recording so UI never leaks in.
        self.close();

        match cur.kind {
            CaptureKind::Screenshot => {
                let ipc = self.ipc.clone();
                glib::timeout_add_local_once(Duration::from_millis(90), move || {
                    ipc.send(IpcRequest::TakeScreenshot { path: None, region });
                });
            }
            CaptureKind::Record => {
                // If already recording, stop first before starting a new clip
                if self.recording.borrow().is_some() {
                    self.stop_recording();
                }
                let this = self.clone();
                let record_audio = cur.record_audio;
                glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    this.start_recording(region, record_audio);
                });
            }
        }
    }

    fn start_recording(&self, region: Option<[i32; 4]>, record_audio: bool) {
        let (out_w, out_h) = self
            .last_snapshot
            .borrow()
            .as_ref()
            .and_then(|s| s.outputs.iter().find(|o| o.primary).or(s.outputs.first()))
            .map(|o| (o.width.max(320), o.height.max(240)))
            .unwrap_or((1920, 1080));

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let dir = home.join("Videos").join("Screencasts");
        let _ = fs::create_dir_all(&dir);

        let stamp = glib::DateTime::now_local()
            .and_then(|dt| dt.format("%Y-%m-%d-%H-%M-%S"))
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "recording".to_string());
        let path = dir.join(format!("Screencast-{stamp}.mp4"));

        let mut cmd = Command::new("wf-recorder");
        cmd.arg("-f")
            .arg(&path)
            .args(["-c", "libx264", "-p", "preset=veryfast", "-p", "crf=23", "-x", "yuv420p"]);

        if let Some([rx, ry, rw, rh]) = region {
            let cx = rx.clamp(0, out_w - 2);
            let cy = ry.clamp(0, out_h - 2);
            // Force even width & height >= 2 so yuv420p / H.264 never fails on odd dimensions.
            let cw = ((rw.min(out_w - cx)) & !1).max(2);
            let ch = ((rh.min(out_h - cy)) & !1).max(2);
            cmd.args(["-g", &format!("{cx},{cy} {cw}x{ch}")]);
        }

        if record_audio {
            cmd.arg("-a");
        }

        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match cmd.spawn() {
            Ok(child) => {
                info!(pid = child.id(), path = %path.display(), "Started NEON DE screen recording");
                *self.recording.borrow_mut() = Some(ActiveRecording {
                    child,
                    path,
                    started_at: Instant::now(),
                });
            }
            Err(err) => {
                warn!(%err, "Failed to launch wf-recorder");
                self.notif_store.push(
                    0,
                    "NEON Screen Recorder".into(),
                    "dialog-error-symbolic".into(),
                    "Screen Recording Failed".into(),
                    format!("Could not start wf-recorder: {err}"),
                    NotificationUrgency::Critical,
                    5000,
                );
            }
        }
    }
}

fn make_mode_btn(icon_name: &str, label: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-shot-mode-btn");
    let row = GtkBox::new(Orientation::Horizontal, 6);
    row.set_valign(Align::Center);
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(15);
    let lbl = Label::new(Some(label));
    row.append(&icon);
    row.append(&lbl);
    btn.set_child(Some(&row));
    btn
}

fn make_kind_btn(icon_name: &str, label: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("neon-shot-kind-btn");
    let row = GtkBox::new(Orientation::Horizontal, 6);
    row.set_valign(Align::Center);
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(15);
    let lbl = Label::new(Some(label));
    row.append(&icon);
    row.append(&lbl);
    btn.set_child(Some(&row));
    btn
}

fn active_workspace_windows(snap: &DesktopSnapshot) -> Vec<&WindowInfo> {
    let active_ws_ids: HashSet<u64> = snap
        .workspaces
        .iter()
        .find(|ws| ws.active)
        .map(|ws| ws.window_ids.iter().copied().collect())
        .unwrap_or_default();
    snap.windows
        .iter()
        .filter(|w| {
            !w.minimized
                && w.width > 0
                && w.height > 0
                && (active_ws_ids.is_empty() || active_ws_ids.contains(&w.id))
        })
        .collect()
}

fn pick_window_at(snap_ref: &Rc<RefCell<Option<DesktopSnapshot>>>, x: f64, y: f64) -> Option<u64> {
    let guard = snap_ref.borrow();
    let snap = guard.as_ref()?;
    let wins = active_workspace_windows(snap);
    // Prefer focused window if pointer is inside it, else first matching window
    wins.iter()
        .find(|w| {
            w.focused
                && x >= w.x as f64
                && x <= (w.x + w.width) as f64
                && y >= w.y as f64
                && y <= (w.y + w.height) as f64
        })
        .or_else(|| {
            wins.iter().rev().find(|w| {
                x >= w.x as f64
                    && x <= (w.x + w.width) as f64
                    && y >= w.y as f64
                    && y <= (w.y + w.height) as f64
            })
        })
        .map(|w| w.id)
}

fn selected_window_rect(
    snap_ref: &Rc<RefCell<Option<DesktopSnapshot>>>,
    selected_id: Option<u64>,
) -> Option<(f64, f64, f64, f64, String)> {
    let guard = snap_ref.borrow();
    let snap = guard.as_ref()?;
    let wins = active_workspace_windows(snap);
    let win = selected_id
        .and_then(|id| wins.iter().copied().find(|w| w.id == id))
        .or_else(|| wins.iter().copied().find(|w| w.focused))
        .or_else(|| wins.first().copied())?;
    let title = win
        .title
        .as_deref()
        .or(win.app_id.as_deref())
        .unwrap_or("Window")
        .to_string();
    Some((
        win.x as f64,
        win.y as f64,
        win.width as f64,
        win.height as f64,
        title,
    ))
}

fn rounded_rect_path(cr: &gtk4::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w * 0.5).min(h * 0.5);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    cr.close_path();
}

fn draw_pill_badge(cr: &gtk4::cairo::Context, x: f64, y: f64, text: &str, is_record: bool) {
    cr.set_font_size(12.5);
    let text_w = cr
        .text_extents(text)
        .map(|e| e.width())
        .unwrap_or(text.len() as f64 * 7.2);
    let dot_pad = if is_record { 16.0 } else { 0.0 };
    let pw = (text_w + 24.0 + dot_pad).max(88.0);
    let ph = 26.0;

    rounded_rect_path(cr, x, y, pw, ph, 13.0);
    if is_record {
        cr.set_source_rgba(0.14, 0.06, 0.08, 0.92);
    } else {
        cr.set_source_rgba(0.08, 0.10, 0.15, 0.88);
    }
    let _ = cr.fill_preserve();

    if is_record {
        cr.set_source_rgba(0.95, 0.32, 0.32, 0.78);
    } else {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.24);
    }
    cr.set_line_width(1.0);
    let _ = cr.stroke();

    if is_record {
        cr.arc(x + 14.0, y + 13.0, 4.5, 0.0, TAU);
        cr.set_source_rgba(0.98, 0.28, 0.28, 1.0);
        let _ = cr.fill();
    }

    cr.set_source_rgba(1.0, 1.0, 1.0, 0.96);
    cr.move_to(x + 12.0 + dot_pad, y + 17.5);
    let _ = cr.show_text(text);
}

#[allow(clippy::too_many_arguments)]
fn draw_cutout_mask(
    cr: &gtk4::cairo::Context,
    wf: f64,
    hf: f64,
    rx: f64,
    ry: f64,
    rw: f64,
    rh: f64,
    show_handles: bool,
    is_record: bool,
    badge_text: &str,
) {
    // Dim everything OUTSIDE the selection rectangle
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.54);
    cr.rectangle(0.0, 0.0, wf, ry.max(0.0));
    cr.rectangle(0.0, ry + rh, wf, (hf - (ry + rh)).max(0.0));
    cr.rectangle(0.0, ry, rx.max(0.0), rh);
    cr.rectangle(rx + rw, ry, (wf - (rx + rw)).max(0.0), rh);
    let _ = cr.fill();

    // Crisp GNOME 47 border (white for screenshot, warm red for screen record)
    if is_record {
        cr.set_source_rgba(0.97, 0.34, 0.34, 0.96);
    } else {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    }
    cr.set_line_width(2.0);
    cr.rectangle(rx, ry, rw, rh);
    let _ = cr.stroke();

    // Circular corner & midpoint handles
    if show_handles {
        let handles = [
            (rx, ry, 6.5),
            (rx + rw, ry, 6.5),
            (rx, ry + rh, 6.5),
            (rx + rw, ry + rh, 6.5),
            (rx + rw * 0.5, ry, 4.5),
            (rx + rw * 0.5, ry + rh, 4.5),
            (rx, ry + rh * 0.5, 4.5),
            (rx + rw, ry + rh * 0.5, 4.5),
        ];
        for (hx, hy, rad) in handles {
            cr.arc(hx, hy, rad, 0.0, TAU);
            if is_record {
                cr.set_source_rgba(0.98, 0.35, 0.35, 1.0);
            } else {
                cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            }
            let _ = cr.fill_preserve();
            cr.set_source_rgba(0.06, 0.08, 0.12, 0.65);
            cr.set_line_width(1.2);
            let _ = cr.stroke();
        }
    }

    let bx = (rx + 10.0).clamp(10.0, (wf - 180.0).max(10.0));
    let by = if ry > 38.0 { ry - 34.0 } else { ry + 12.0 };
    draw_pill_badge(cr, bx, by, badge_text, is_record);
}
