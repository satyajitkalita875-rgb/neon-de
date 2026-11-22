//! GNOME 47-Inspired Interactive Screenshot Overlay (`zwlr_layer_shell_v1` `Layer::Overlay`).
//!
//! Features:
//! - Drag-to-select region with translucent dimmed surround, crystal-clear cutout, circular
//!   corner handles, and live `W × H` pixel badge
//! - Floating bottom-center GNOME dark pill toolbar: **Selection** | **Screen** | **Window**
//! - Signature round GNOME Capture shutter button (or `Enter` / `Space`) and `Esc` to cancel
//! - Captures cleanly via `neon-compositor`'s GPU offscreen screenshot pipeline

use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, DrawingArea, EventControllerKey,
    GestureDrag, Image, Label, Orientation, Overlay, gdk, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::{DesktopSnapshot, IpcRequest};

use crate::ipc_bridge::IpcCommandSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Selection,
    Screen,
    Window,
}

#[derive(Debug, Clone, Copy)]
struct SelState {
    mode: CaptureMode,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    dragging: bool,
    drag_start_x: f64,
    drag_start_y: f64,
}

impl SelState {
    fn rect(&self) -> (f64, f64, f64, f64) {
        let rx = self.x0.min(self.x1);
        let ry = self.y0.min(self.y1);
        let rw = (self.x1 - self.x0).abs().max(24.0);
        let rh = (self.y1 - self.y0).abs().max(24.0);
        (rx, ry, rw, rh)
    }
}

/// Interactive GNOME-style screenshot overlay.
#[derive(Clone)]
pub struct ScreenshotOverlay {
    window: ApplicationWindow,
    drawing_area: DrawingArea,
    state: Rc<RefCell<SelState>>,
    mode_sel_btn: Button,
    mode_scr_btn: Button,
    mode_win_btn: Button,
    last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>>,
    ipc: IpcCommandSender,
}

impl ScreenshotOverlay {
    pub fn build(app: &Application, ipc: IpcCommandSender) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Screenshot")
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
            x0: 360.0,
            y0: 200.0,
            x1: 1160.0,
            y1: 720.0,
            dragging: false,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
        }));
        let last_snapshot: Rc<RefCell<Option<DesktopSnapshot>>> = Rc::new(RefCell::new(None));

        let overlay = Overlay::new();

        // Fullscreen Cairo canvas drawing dimmed surround + crisp selection cutout
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

                match cur.mode {
                    CaptureMode::Screen => {
                        // Subtle highlight frame around the entire screen
                        cr.set_source_rgba(0.0, 0.0, 0.0, 0.14);
                        cr.rectangle(0.0, 0.0, wf, hf);
                        let _ = cr.fill();

                        cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
                        cr.set_line_width(4.0);
                        cr.rectangle(2.0, 2.0, wf - 4.0, hf - 4.0);
                        let _ = cr.stroke();
                    }
                    CaptureMode::Selection => {
                        let (rx, ry, rw, rh) = cur.rect();
                        draw_cutout_mask(cr, wf, hf, rx, ry, rw, rh, true);
                    }
                    CaptureMode::Window => {
                        if let Some((wx, wy, ww, wh)) = active_window_rect(&snap_ref) {
                            draw_cutout_mask(cr, wf, hf, wx, wy, ww, wh, false);
                        } else {
                            cr.set_source_rgba(0.0, 0.0, 0.0, 0.32);
                            cr.rectangle(0.0, 0.0, wf, hf);
                            let _ = cr.fill();
                        }
                    }
                }
            });
        }

        // Drag gesture on the canvas to draw any custom screenshot region
        let drag = GestureDrag::new();
        {
            let st = Rc::clone(&state);
            let da = drawing_area.clone();
            drag.connect_drag_begin(move |_, x, y| {
                let mut s = st.borrow_mut();
                if s.mode == CaptureMode::Selection {
                    s.dragging = true;
                    s.drag_start_x = x;
                    s.drag_start_y = y;
                    s.x0 = x;
                    s.y0 = y;
                    s.x1 = x + 1.0;
                    s.y1 = y + 1.0;
                    da.queue_draw();
                }
            });
        }
        {
            let st = Rc::clone(&state);
            let da = drawing_area.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let mut s = st.borrow_mut();
                if s.dragging && s.mode == CaptureMode::Selection {
                    s.x1 = (s.drag_start_x + dx).max(0.0);
                    s.y1 = (s.drag_start_y + dy).max(0.0);
                    da.queue_draw();
                }
            });
        }
        {
            let st = Rc::clone(&state);
            drag.connect_drag_end(move |_, _, _| {
                st.borrow_mut().dragging = false;
            });
        }
        drawing_area.add_controller(drag);
        overlay.set_child(Some(&drawing_area));

        // --- GNOME 47 Signature Floating Bottom Pill Toolbar ---
        let toolbar = GtkBox::new(Orientation::Vertical, 12);
        toolbar.add_css_class("neon-gnome-shot-bar");
        toolbar.set_halign(Align::Center);
        toolbar.set_valign(Align::End);
        toolbar.set_margin_bottom(40);

        // Top row of toolbar: Mode switcher (Selection | Screen | Window) + Close
        let mode_row = GtkBox::new(Orientation::Horizontal, 8);
        mode_row.set_halign(Align::Center);

        let mode_sel_btn = make_mode_btn("select-rectangular-symbolic", "Selection");
        mode_sel_btn.add_css_class("active");
        let mode_scr_btn = make_mode_btn("video-display-symbolic", "Screen");
        let mode_win_btn = make_mode_btn("window-new-symbolic", "Window");

        let cancel_btn = Button::with_label("×");
        cancel_btn.add_css_class("neon-close-button");
        cancel_btn.set_tooltip_text(Some("Cancel (Esc)"));

        mode_row.append(&mode_sel_btn);
        mode_row.append(&mode_scr_btn);
        mode_row.append(&mode_win_btn);
        mode_row.append(&cancel_btn);

        // Bottom row: Big round GNOME Capture Shutter Button
        let shutter_btn = Button::new();
        shutter_btn.add_css_class("neon-shot-shutter");
        shutter_btn.set_halign(Align::Center);
        shutter_btn.set_tooltip_text(Some("Capture Screenshot (Enter)"));
        let shutter_icon = Image::from_icon_name("camera-photo-symbolic");
        shutter_icon.set_pixel_size(24);
        shutter_btn.set_child(Some(&shutter_icon));

        let hint = Label::new(Some(
            "Drag to select area · Enter to capture · Esc to cancel",
        ));
        hint.add_css_class("neon-qs-muted");

        toolbar.append(&mode_row);
        toolbar.append(&shutter_btn);
        toolbar.append(&hint);
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
            last_snapshot,
            ipc,
        };

        // Wire mode buttons
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
        {
            let this = shot.clone();
            cancel_btn.connect_clicked(move |_| this.close());
        }
        {
            let this = shot.clone();
            shutter_btn.connect_clicked(move |_| this.capture_now());
        }

        // Keyboard shortcuts inside overlay: Esc cancels, Enter/Space captures
        let key_ctrl = EventControllerKey::new();
        {
            let this = shot.clone();
            key_ctrl.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    this.close();
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Return || key == gdk::Key::space {
                    this.capture_now();
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
        // Reset default selection box centered cleanly on the primary output
        if let Some(snap) = self.last_snapshot.borrow().as_ref()
            && let Some(out) = snap.outputs.first()
        {
            let w = out.width as f64;
            let h = out.height as f64;
            let mut s = self.state.borrow_mut();
            s.x0 = w * 0.20;
            s.y0 = h * 0.18;
            s.x1 = w * 0.80;
            s.y1 = h * 0.72;
        }
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
        self.drawing_area.queue_draw();
    }

    fn capture_now(&self) {
        let cur = *self.state.borrow();
        let region: Option<[i32; 4]> = match cur.mode {
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
            CaptureMode::Window => active_window_rect(&self.last_snapshot).map(|(x, y, w, h)| {
                [
                    x.round() as i32,
                    y.round() as i32,
                    w.round() as i32,
                    h.round() as i32,
                ]
            }),
        };

        // Hide the screenshot overlay first so the captured PNG is 100% clean!
        self.close();

        let ipc = self.ipc.clone();
        glib::timeout_add_local_once(Duration::from_millis(90), move || {
            ipc.send(IpcRequest::TakeScreenshot { path: None, region });
        });
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

fn active_window_rect(
    snap_ref: &Rc<RefCell<Option<DesktopSnapshot>>>,
) -> Option<(f64, f64, f64, f64)> {
    let guard = snap_ref.borrow();
    let snap = guard.as_ref()?;
    let win = snap
        .windows
        .iter()
        .find(|w| w.focused && !w.minimized)
        .or_else(|| snap.windows.iter().find(|w| !w.minimized))?;
    if win.width <= 0 || win.height <= 0 {
        return None;
    }
    Some((
        win.x as f64,
        win.y as f64,
        win.width as f64,
        win.height as f64,
    ))
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
) {
    // Dim everything OUTSIDE the selection rectangle (4 clean strips)
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.52);
    cr.rectangle(0.0, 0.0, wf, ry.max(0.0));
    cr.rectangle(0.0, ry + rh, wf, (hf - (ry + rh)).max(0.0));
    cr.rectangle(0.0, ry, rx.max(0.0), rh);
    cr.rectangle(rx + rw, ry, (wf - (rx + rw)).max(0.0), rh);
    let _ = cr.fill();

    // Crisp GNOME 47 white border around selection cutout
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.94);
    cr.set_line_width(2.0);
    cr.rectangle(rx, ry, rw, rh);
    let _ = cr.stroke();

    // Circular corner drag handles
    if show_handles {
        for (hx, hy) in [(rx, ry), (rx + rw, ry), (rx, ry + rh), (rx + rw, ry + rh)] {
            cr.arc(hx, hy, 6.0, 0.0, std::f64::consts::TAU);
            cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            let _ = cr.fill();
        }
    }

    // Live dimension badge (e.g. "800 × 520")
    let dim_txt = format!("{:.0} × {:.0}", rw, rh);
    let bx = rx + 10.0;
    let by = if ry > 34.0 { ry - 30.0 } else { ry + 10.0 };
    cr.set_source_rgba(0.08, 0.10, 0.15, 0.86);
    cr.rectangle(bx, by, 92.0, 24.0);
    let _ = cr.fill();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.set_font_size(12.0);
    cr.move_to(bx + 10.0, by + 16.0);
    let _ = cr.show_text(&dim_txt);
}
