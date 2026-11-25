//! Battery & Power settings: live UPower battery level & health and PowerProfilesDaemon
//! performance profile selector.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ProgressBar, ScrolledWindow, Spinner, prelude::*};
use neon_system::power::{self, PowerStatus};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, info_row, page_shell, run_async, section_heading, setting_row,
        status_badge,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    let root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row("Checking Power & Battery…", "", &spin));
    root.append(&load_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(power::query_power_status, move |status| {
        render_power(&root_done, ctrl_done, status);
    });

    scroller
}

fn render_power(root: &GtkBox, ctrl: Rc<SettingsController>, status: PowerStatus) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Battery & Power Mode ─────────────────────────────────────────────────
    root.append(&section_heading("Battery & Performance"));
    let bat_card = boxed_card();

    let mut bat_rows = Vec::new();
    if status.has_battery {
        let pct = status.battery_percentage.clamp(0.0, 100.0);
        let bar = ProgressBar::new();
        bar.set_fraction(pct / 100.0);
        bar.set_width_request(165);
        bar.set_valign(Align::Center);
        bar.set_show_text(true);
        bar.set_text(Some(&format!("{pct:.0}%")));

        bat_rows.push(setting_row(
            "Battery Level",
            status.battery_state.label(),
            &bar,
        ));
        if let Some(health) = status.battery_health_percent {
            bat_rows.push(info_row(
                "Battery Health",
                "Maximum capacity relative to factory design.",
                &format!("{health:.0}%"),
            ));
        }
    } else {
        bat_rows.push(setting_row(
            "Power Source",
            "Running on AC power.",
            &status_badge("AC Power", "ok"),
        ));
    }

    if !status.available_profiles.is_empty() {
        let seg = GtkBox::new(Orientation::Horizontal, 3);
        seg.add_css_class("settings-seg-track");
        seg.set_valign(Align::Center);

        for prof in &status.available_profiles {
            let label = match prof.as_str() {
                "power-saver" => "Low Power",
                "balanced" => "Balanced",
                "performance" => "Performance",
                other => other,
            };
            let btn = Button::with_label(label);
            btn.add_css_class("settings-seg-btn");
            if status.active_profile.as_deref() == Some(prof.as_str()) {
                btn.add_css_class("active");
            }
            let ctrl_c = Rc::clone(&ctrl);
            let prof_owned = prof.clone();
            let label_owned = label.to_string();
            btn.connect_clicked(move |_| {
                let ctrl_done = Rc::clone(&ctrl_c);
                let p = prof_owned.clone();
                let lbl = label_owned.clone();
                run_async(
                    move || power::set_power_profile(&p),
                    move |res| match res {
                        Ok(()) => {
                            ctrl_done.set_status_ok(&format!("Power mode: {lbl}"));
                            ctrl_done.rebuild_page("power");
                        }
                        Err(e) => ctrl_done.set_status_err(&format!("Power mode failed: {e}")),
                    },
                );
            });
            seg.append(&btn);
        }

        bat_rows.push(setting_row(
            "Power Mode",
            "Active hardware performance profile.",
            &seg,
        ));
    }

    append_rows(&bat_card, bat_rows);
    root.append(&bat_card);
}
