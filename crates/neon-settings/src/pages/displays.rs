//! Displays settings: async live monitor detection, spatial arrangement preview,
//! real Display Brightness via systemd-logind, fractional scaling, rotation, primary
//! display selection, and 15-second safe revert countdown.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, DrawingArea, DropDown, Orientation, Scale, ScrolledWindow,
    Spinner, StringList, Switch, prelude::*,
};
use neon_ipc::{IpcRequest, OutputInfo};
use neon_system::power;

use crate::{
    state::{SettingsController, fetch_outputs_sync},
    ui::{
        append_rows, boxed_card, display_confirm_countdown_dialog, info_row, page_shell, run_async,
        section_heading, setting_row, status_badge,
    },
};

struct DisplaysData {
    outputs: Vec<OutputInfo>,
    brightness: Option<u32>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    let root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row("Detecting Displays…", "", &spin));
    root.append(&load_card);

    let socket = ctrl.ipc_socket.clone();
    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(
        move || DisplaysData {
            outputs: fetch_outputs_sync(&socket),
            brightness: power::read_backlight_percent(),
        },
        move |data| render_displays(&root_done, ctrl_done, data),
    );

    scroller
}

fn render_displays(root: &GtkBox, ctrl: Rc<SettingsController>, data: DisplaysData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Arrangement Preview ──────────────────────────────────────────────────
    root.append(&section_heading("Arrangement"));
    let layout_card = boxed_card();

    let refresh_btn = Button::with_label("Detect Displays");
    refresh_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        refresh_btn.connect_clicked(move |_| {
            ctrl_c.rebuild_page("displays");
            ctrl_c.set_status_info("Refreshed displays");
        });
    }

    if data.outputs.is_empty() {
        layout_card.append(&setting_row(
            "No Active Displays Reported",
            "Connect to a live NEON DE compositor session to configure outputs.",
            &refresh_btn,
        ));
    } else {
        let canvas = build_arrangement_canvas(data.outputs.clone());
        canvas.set_margin_top(10);
        canvas.set_margin_bottom(6);
        canvas.set_margin_start(14);
        canvas.set_margin_end(14);
        layout_card.append(&canvas);
        layout_card.append(&setting_row(
            &format!(
                "{} Connected Display{}",
                data.outputs.len(),
                if data.outputs.len() == 1 { "" } else { "s" }
            ),
            "Live DRM/KMS outputs reported by neon-compositor.",
            &refresh_btn,
        ));
    }
    root.append(&layout_card);

    // ── Display Brightness (Real logind backlight control) ───────────────────
    if let Some(pct) = data.brightness {
        root.append(&section_heading("Brightness"));
        let br_card = boxed_card();
        let scale = Scale::with_range(Orientation::Horizontal, 5.0, 100.0, 1.0);
        scale.add_css_class("settings-scale");
        scale.set_width_request(210);
        scale.set_draw_value(true);
        scale.set_digits(0);
        scale.set_value(f64::from(pct.clamp(5, 100)));
        scale.connect_value_changed(|s| {
            let v = s.value().round() as u32;
            std::thread::spawn(move || {
                let _ = power::set_backlight_percent(v);
            });
        });
        br_card.append(&setting_row(
            "Display Brightness",
            "Built-in backlight controlled via systemd-logind.",
            &scale,
        ));
        root.append(&br_card);
    }

    // ── Per-Monitor Configuration ────────────────────────────────────────────
    if !data.outputs.is_empty() {
        root.append(&section_heading("Monitors"));
    }

    for out in &data.outputs {
        let card = boxed_card();
        let refresh_hz = out.refresh_mhz as f64 / 1000.0;
        let header_badge = if out.primary {
            status_badge("Primary", "ok")
        } else {
            status_badge("Secondary", "")
        };

        let scale_values = [1.0_f64, 1.25, 1.5, 1.75, 2.0];
        let scale_dd = DropDown::new(
            Some(StringList::new(&["100%", "125%", "150%", "175%", "200%"])),
            gtk4::Expression::NONE,
        );
        scale_dd.set_valign(Align::Center);
        let init_scale_idx = scale_values
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (*a - out.scale)
                    .abs()
                    .partial_cmp(&(*b - out.scale).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i as u32)
            .unwrap_or(0);
        scale_dd.set_selected(init_scale_idx);

        let transforms = ["normal", "90", "180", "270"];
        let rot_dd = DropDown::new(
            Some(StringList::new(&[
                "Standard (0°)",
                "90°",
                "180°",
                "270°",
            ])),
            gtk4::Expression::NONE,
        );
        rot_dd.set_valign(Align::Center);
        rot_dd.set_selected(match out.transform.as_str() {
            "90" => 1,
            "180" => 2,
            "270" => 3,
            _ => 0,
        });

        let primary_sw = Switch::new();
        primary_sw.set_active(out.primary);
        primary_sw.set_valign(Align::Center);

        let apply_btn = Button::with_label("Apply Changes");
        apply_btn.add_css_class("settings-action-btn");
        {
            let ctrl_c = Rc::clone(&ctrl);
            let prev = out.clone();
            let scale_dd_c = scale_dd.clone();
            let rot_dd_c = rot_dd.clone();
            let primary_sw_c = primary_sw.clone();

            apply_btn.connect_clicked(move |_| {
                let new_scale = scale_values
                    .get(scale_dd_c.selected() as usize)
                    .copied()
                    .unwrap_or(1.0);
                let new_transform = transforms
                    .get(rot_dd_c.selected() as usize)
                    .copied()
                    .unwrap_or("normal")
                    .to_string();
                let new_primary = primary_sw_c.is_active();

                let apply_req = IpcRequest::ConfigureOutput {
                    name: prev.name.clone(),
                    x: Some(prev.x),
                    y: Some(prev.y),
                    scale: Some(new_scale),
                    primary: Some(new_primary),
                    transform: Some(new_transform),
                };

                match ctrl_c.send_ipc(apply_req) {
                    Ok(_) => {
                        let ctrl_keep = Rc::clone(&ctrl_c);
                        let ctrl_rev = Rc::clone(&ctrl_c);
                        let prev_for_revert = prev.clone();
                        let name_keep = prev.name.clone();
                        display_confirm_countdown_dialog(
                            &ctrl_c.window,
                            15,
                            move || {
                                ctrl_keep.set_status_ok(&format!("Saved display {name_keep}"));
                                ctrl_keep.rebuild_page("displays");
                            },
                            move || {
                                let revert_req = IpcRequest::ConfigureOutput {
                                    name: prev_for_revert.name.clone(),
                                    x: Some(prev_for_revert.x),
                                    y: Some(prev_for_revert.y),
                                    scale: Some(prev_for_revert.scale),
                                    primary: Some(prev_for_revert.primary),
                                    transform: Some(prev_for_revert.transform.clone()),
                                };
                                let _ = ctrl_rev.send_ipc(revert_req);
                                ctrl_rev.set_status_info("Reverted display settings");
                                ctrl_rev.rebuild_page("displays");
                            },
                        );
                    }
                    Err(err) => {
                        ctrl_c.set_status_err(&format!("Display update failed: {err}"));
                    }
                }
            });
        }

        append_rows(
            &card,
            vec![
                setting_row(
                    &out.name,
                    &format!(
                        "{} × {} @ {:.2} Hz • Position ({}, {})",
                        out.width, out.height, refresh_hz, out.x, out.y
                    ),
                    &header_badge,
                ),
                info_row(
                    "Resolution & Refresh Rate",
                    "",
                    &format!("{}×{} ({:.2} Hz)", out.width, out.height, refresh_hz),
                ),
                setting_row("Display Scaling", "", &scale_dd),
                setting_row("Rotation", "", &rot_dd),
                setting_row(
                    "Primary Display",
                    "Shows the top bar and dock.",
                    &primary_sw,
                ),
                setting_row(
                    "Apply Configuration",
                    "Includes 15-second automatic revert protection.",
                    &apply_btn,
                ),
            ],
        );
        root.append(&card);
    }
}

fn build_arrangement_canvas(outputs: Vec<OutputInfo>) -> DrawingArea {
    let area = DrawingArea::new();
    area.add_css_class("monitor-canvas");
    area.set_content_height(116);
    area.set_hexpand(true);

    area.set_draw_func(move |_, cr, width, height| {
        let w = f64::from(width);
        let h = f64::from(height);

        let min_x = outputs.iter().map(|o| o.x).min().unwrap_or(0) as f64;
        let min_y = outputs.iter().map(|o| o.y).min().unwrap_or(0) as f64;
        let max_x = outputs
            .iter()
            .map(|o| o.x + o.width.max(320))
            .max()
            .unwrap_or(1920) as f64;
        let max_y = outputs
            .iter()
            .map(|o| o.y + o.height.max(200))
            .max()
            .unwrap_or(1080) as f64;

        let span_x = (max_x - min_x).max(1.0);
        let span_y = (max_y - min_y).max(1.0);
        let pad = 16.0;
        let scale = ((w - pad * 2.0) / span_x).min((h - pad * 2.0) / span_y);
        let offset_x = (w - span_x * scale) * 0.5;
        let offset_y = (h - span_y * scale) * 0.5;

        for out in &outputs {
            let rx = offset_x + (out.x as f64 - min_x) * scale;
            let ry = offset_y + (out.y as f64 - min_y) * scale;
            let rw = (out.width as f64 * scale).max(48.0);
            let rh = (out.height as f64 * scale).max(32.0);

            if out.primary {
                cr.set_source_rgba(0.04, 0.52, 1.0, 0.24);
            } else {
                cr.set_source_rgba(0.55, 0.58, 0.68, 0.22);
            }
            cr.rectangle(rx, ry, rw, rh);
            let _ = cr.fill();

            if out.primary {
                cr.set_source_rgba(0.04, 0.52, 1.0, 0.90);
            } else {
                cr.set_source_rgba(0.65, 0.68, 0.75, 0.70);
            }
            cr.set_line_width(1.5);
            cr.rectangle(rx, ry, rw, rh);
            let _ = cr.stroke();

            cr.set_source_rgba(0.95, 0.96, 0.98, 0.95);
            cr.set_font_size(11.0);
            cr.move_to(rx + 8.0, ry + 17.0);
            let _ = cr.show_text(&out.name);
            cr.set_font_size(9.5);
            cr.move_to(rx + 8.0, ry + 31.0);
            let _ = cr.show_text(&format!("{}×{}", out.width, out.height));
        }
    });

    area
}
