//! 5.2 Displays settings: live monitor detection, spatial arrangement preview,
//! fractional scaling, orientation, primary monitor, Night Light, HDR capability status,
//! and mandatory 15-second confirmation countdown with automatic revert on timeout!

use std::rc::Rc;

use gtk4::{
    Align, Button, DrawingArea, DropDown, Label, ScrolledWindow, StringList, Switch, glib,
    prelude::*,
};
use neon_ipc::{IpcRequest, OutputInfo};

use crate::{
    metadata::CapabilityStatus,
    state::SettingsController,
    ui::{
        append_rows, boxed_card, capability_row, display_confirm_countdown_dialog, info_row,
        page_shell, section_heading, setting_row, slider_row, status_badge, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Displays",
        "Inspect connected monitors, arrange scaling and orientation with automatic revert protection, and tune Night Light.",
        Some("displays"),
        &ctrl,
    );

    let outputs = ctrl.fetch_outputs();

    // ── Monitor Arrangement Preview ──────────────────────────────────────────
    page.append(&section_heading("Monitor Arrangement"));
    let layout_card = boxed_card();

    let refresh_btn = Button::with_label("Detect Displays");
    refresh_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        refresh_btn.connect_clicked(move |_| {
            ctrl_c.rebuild_page("displays");
            ctrl_c.set_status_info("Refreshed connected compositor outputs");
        });
    }

    if outputs.is_empty() {
        layout_card.append(&setting_row(
            "No Active Compositor Outputs Detected",
            "Run neon-settings inside a live NEON DE session to query and configure DRM/KMS outputs.",
            &refresh_btn,
        ));
    } else {
        let canvas = build_arrangement_canvas(outputs.clone());
        canvas.set_margin_top(12);
        canvas.set_margin_bottom(8);
        canvas.set_margin_start(16);
        canvas.set_margin_end(16);
        layout_card.append(&canvas);
        layout_card.append(&setting_row(
            &format!("{} Connected Monitor{}", outputs.len(), if outputs.len() == 1 { "" } else { "s" }),
            "Logical positions and resolutions reported live by neon-compositor.",
            &refresh_btn,
        ));
    }
    page.append(&layout_card);

    // ── Per-Monitor Configuration ────────────────────────────────────────────
    if !outputs.is_empty() {
        page.append(&section_heading("Per-Monitor Configuration"));
    }

    for out in &outputs {
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
                "Landscape (0°)",
                "Portrait (90°)",
                "Landscape Flipped (180°)",
                "Portrait Flipped (270°)",
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

        let apply_btn = Button::with_label("Apply & Verify");
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
                                ctrl_keep.set_status_ok(&format!(
                                    "Confirmed display configuration for {name_keep}"
                                ));
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
                                ctrl_rev.set_status_info(&format!(
                                    "Reverted {} to previous safe display settings",
                                    prev_for_revert.name
                                ));
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
                    "Active Mode & Refresh Rate",
                    "Native DRM/KMS mode reported by the compositor.",
                    &format!("{}×{} ({:.2} Hz)", out.width, out.height, refresh_hz),
                ),
                setting_row(
                    "Fractional Display Scaling",
                    "Adjust UI scale for HiDPI clarity.",
                    &scale_dd,
                ),
                setting_row(
                    "Screen Orientation",
                    "Rotate output transform.",
                    &rot_dd,
                ),
                setting_row(
                    "Primary Display",
                    "Shows the main top panel and floating dock.",
                    &primary_sw,
                ),
                setting_row(
                    "Apply Monitor Configuration",
                    "Applies live with a 15-second confirmation countdown and automatic safe revert.",
                    &apply_btn,
                ),
            ],
        );
        page.append(&card);
    }

    // ── Multi-Monitor & Color Controls ───────────────────────────────────────
    page.append(&section_heading("Multi-Monitor & Night Light"));
    let prefs_card = boxed_card();

    let night_light = ctrl.config.borrow().displays.night_light_enabled;
    let color_temp = f64::from(ctrl.config.borrow().displays.color_temperature_k);
    let mirror = ctrl.config.borrow().displays.mirror_displays;
    let per_mon_ws = ctrl.config.borrow().displays.per_monitor_workspaces;

    append_rows(
        &prefs_card,
        vec![
            toggle_row(
                "Mirror Displays",
                "Duplicate primary desktop across secondary displays when multiple monitors are attached.",
                mirror,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().displays.mirror_displays = active;
                        ctrl_c.persist_and_sync(None, "Updated display mirror preference");
                    }
                },
            ),
            toggle_row(
                "Independent Workspaces per Monitor",
                "Switch workspaces separately on each connected display.",
                per_mon_ws,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().displays.per_monitor_workspaces = active;
                        ctrl_c.config.borrow_mut().desktop.per_monitor_workspaces = active;
                        ctrl_c.persist_and_sync(None, "Updated per-monitor workspace mode");
                    }
                },
            ),
            toggle_row(
                "Night Light (Warm Color Shift)",
                "Reduces blue light emission at night. Saved in config; live DRM gamma LUT hooks are in progress.",
                night_light,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().displays.night_light_enabled = active;
                        ctrl_c.persist_and_sync(None, "Updated Night Light preference");
                    }
                },
            ),
            slider_row(
                "Color Temperature (K)",
                "Lower Kelvin produces warmer amber tones (1700K–6500K).",
                1700.0,
                6500.0,
                100.0,
                color_temp,
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().displays.color_temperature_k = val.round() as u32;
                        ctrl_c.persist_and_sync(
                            None,
                            &format!("Night Light temperature set to {:.0}K", val),
                        );
                    }
                },
            ),
            capability_row(
                "High Dynamic Range (HDR)",
                "Requires Wayland color-management protocol and 10-bit DRM HDR metadata support.",
                CapabilityStatus::Unsupported(
                    "Not yet supported by neon-renderer (standard SDR 8-bit pipeline active).",
                ),
            ),
        ],
    );
    page.append(&prefs_card);

    // Unused import guard for glib/Label
    let _ = (glib::Propagation::Proceed, Label::new(None));
    scroller
}

fn build_arrangement_canvas(outputs: Vec<OutputInfo>) -> DrawingArea {
    let area = DrawingArea::new();
    area.add_css_class("monitor-canvas");
    area.set_content_height(136);
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
        let pad = 20.0;
        let scale = ((w - pad * 2.0) / span_x).min((h - pad * 2.0) / span_y);
        let offset_x = (w - span_x * scale) * 0.5;
        let offset_y = (h - span_y * scale) * 0.5;

        for out in &outputs {
            let rx = offset_x + (out.x as f64 - min_x) * scale;
            let ry = offset_y + (out.y as f64 - min_y) * scale;
            let rw = (out.width as f64 * scale).max(48.0);
            let rh = (out.height as f64 * scale).max(32.0);

            if out.primary {
                cr.set_source_rgba(0.22, 0.74, 0.97, 0.28);
            } else {
                cr.set_source_rgba(0.55, 0.58, 0.68, 0.25);
            }
            cr.rectangle(rx, ry, rw, rh);
            let _ = cr.fill();

            if out.primary {
                cr.set_source_rgba(0.22, 0.74, 0.97, 0.95);
            } else {
                cr.set_source_rgba(0.65, 0.68, 0.75, 0.75);
            }
            cr.set_line_width(2.0);
            cr.rectangle(rx, ry, rw, rh);
            let _ = cr.stroke();

            cr.set_source_rgba(0.92, 0.94, 0.98, 0.95);
            cr.set_font_size(11.5);
            cr.move_to(rx + 8.0, ry + 18.0);
            let _ = cr.show_text(&out.name);
            cr.set_font_size(10.0);
            cr.move_to(rx + 8.0, ry + 33.0);
            let _ = cr.show_text(&format!("{}×{}", out.width, out.height));
        }
    });

    area
}
