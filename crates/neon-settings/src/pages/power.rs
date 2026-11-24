//! 5.12 Power & Battery settings: UPower battery status/health/charging state,
//! PowerProfilesDaemon selector (Power Saver / Balanced / Performance), screen dimming/blanking,
//! suspend timeout, lid-close & power-button actions, and low-battery alerts.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ProgressBar, ScrolledWindow, Spinner, prelude::*};
use neon_system::power::{self, PowerStatus};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, dropdown_row, info_row, page_shell, run_async, section_heading,
        setting_row, status_badge, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Power & Battery",
        "Monitor battery charge and health via UPower, switch system power profiles, and configure sleep timeouts.",
        Some("power"),
        &ctrl,
    );

    let root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row(
        "Querying UPower & PowerProfiles…",
        "Reading battery state and thermal/power profiles over D-Bus.",
        &spin,
    ));
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

    // ── Battery Status ───────────────────────────────────────────────────────
    root.append(&section_heading("Battery & Energy"));
    let bat_card = boxed_card();

    let mut bat_rows = Vec::new();
    if status.has_battery {
        let pct = status.battery_percentage.clamp(0.0, 100.0);
        let bar = ProgressBar::new();
        bar.set_fraction(pct / 100.0);
        bar.set_width_request(180);
        bar.set_valign(Align::Center);
        bar.set_show_text(true);
        bar.set_text(Some(&format!("{pct:.0}%")));

        bat_rows.push(setting_row(
            "Battery Level",
            &format!("Status: {}", status.battery_state.label()),
            &bar,
        ));
        if let Some(health) = status.battery_health_percent {
            bat_rows.push(info_row(
                "Battery Health (Capacity)",
                "Current full-charge capacity compared to factory design capacity.",
                &format!("{health:.0}%"),
            ));
        }
    } else {
        bat_rows.push(setting_row(
            "Power Supply",
            "Running on AC mains power (no internal battery reported by UPower).",
            &status_badge("AC Power", "ok"),
        ));
    }

    // Power Profiles selector
    if !status.available_profiles.is_empty() {
        let seg = GtkBox::new(Orientation::Horizontal, 4);
        seg.add_css_class("settings-seg-track");
        seg.set_valign(Align::Center);

        for prof in &status.available_profiles {
            let label = match prof.as_str() {
                "power-saver" => "Power Saver",
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
                            ctrl_done.set_status_ok(&format!("Switched power mode to {lbl}"));
                            ctrl_done.rebuild_page("power");
                        }
                        Err(e) => ctrl_done.set_status_err(&format!("Power profile failed: {e}")),
                    },
                );
            });
            seg.append(&btn);
        }

        bat_rows.push(setting_row(
            "Power Mode",
            "Live org.freedesktop.UPower.PowerProfiles hardware profile.",
            &seg,
        ));
    } else {
        bat_rows.push(setting_row(
            "Power Profiles Daemon",
            "power-profiles-daemon is not active on this system.",
            &status_badge("Unavailable", "unavailable"),
        ));
    }

    append_rows(&bat_card, bat_rows);
    root.append(&bat_card);

    // ── Screen & Suspend Timeouts ────────────────────────────────────────────
    root.append(&section_heading("Screen Blank, Suspend & Hardware Buttons"));
    let pwr_cfg = ctrl.config.borrow().power.clone();
    let timeout_card = boxed_card();

    let dim_opts: [u32; 5] = [0, 60, 180, 300, 600];
    let dim_idx = dim_opts
        .iter()
        .position(|v| *v == pwr_cfg.screen_dim_seconds)
        .unwrap_or(2) as u32;

    let blank_opts: [u32; 6] = [0, 180, 300, 600, 900, 1800];
    let blank_idx = blank_opts
        .iter()
        .position(|v| *v == pwr_cfg.screen_blank_seconds)
        .unwrap_or(3) as u32;

    let susp_opts: [u32; 6] = [0, 600, 900, 1800, 3600, 7200];
    let susp_idx = susp_opts
        .iter()
        .position(|v| *v == pwr_cfg.suspend_timeout_seconds)
        .unwrap_or(3) as u32;

    let actions = ["suspend", "lock", "poweroff", "nothing"];
    let action_labels = ["Suspend", "Lock Screen", "Shut Down", "Do Nothing"];
    let lid_idx = actions
        .iter()
        .position(|a| *a == pwr_cfg.lid_close_action.as_str())
        .unwrap_or(0) as u32;
    let btn_idx = actions
        .iter()
        .position(|a| *a == pwr_cfg.power_button_action.as_str())
        .unwrap_or(0) as u32;

    append_rows(
        &timeout_card,
        vec![
            dropdown_row(
                "Dim Screen When Inactive",
                "Lower backlight brightness after inactivity.",
                &["Never", "1 minute", "3 minutes", "5 minutes", "10 minutes"],
                dim_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let secs = dim_opts.get(idx as usize).copied().unwrap_or(180);
                        ctrl_c.config.borrow_mut().power.screen_dim_seconds = secs;
                        ctrl_c.persist_and_sync(None, "Updated screen dimming timeout");
                    }
                },
            ),
            dropdown_row(
                "Automatic Screen Blanking",
                "Turn display off after inactivity.",
                &[
                    "Never",
                    "3 minutes",
                    "5 minutes",
                    "10 minutes",
                    "15 minutes",
                    "30 minutes",
                ],
                blank_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let secs = blank_opts.get(idx as usize).copied().unwrap_or(600);
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.power.screen_blank_seconds = secs;
                            cfg.privacy_accessibility.screen_timeout_seconds = secs;
                        }
                        ctrl_c.persist_and_sync(None, "Updated screen blanking timeout");
                    }
                },
            ),
            dropdown_row(
                "Automatic Suspend",
                "Put computer to sleep after extended inactivity.",
                &[
                    "Never",
                    "10 minutes",
                    "15 minutes",
                    "30 minutes",
                    "1 hour",
                    "2 hours",
                ],
                susp_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let secs = susp_opts.get(idx as usize).copied().unwrap_or(1800);
                        ctrl_c.config.borrow_mut().power.suspend_timeout_seconds = secs;
                        ctrl_c.persist_and_sync(None, "Updated automatic suspend timeout");
                    }
                },
            ),
            dropdown_row(
                "When Laptop Lid Is Closed",
                "System response when closing the lid.",
                &action_labels,
                lid_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let act = actions.get(idx as usize).copied().unwrap_or("suspend");
                        ctrl_c.config.borrow_mut().power.lid_close_action = act.into();
                        ctrl_c.persist_and_sync(None, "Updated lid-close behavior");
                    }
                },
            ),
            dropdown_row(
                "Power Button Behavior",
                "Action when pressing the physical power button.",
                &action_labels,
                btn_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let act = actions.get(idx as usize).copied().unwrap_or("suspend");
                        ctrl_c.config.borrow_mut().power.power_button_action = act.into();
                        ctrl_c.persist_and_sync(None, "Updated power button behavior");
                    }
                },
            ),
            toggle_row(
                "Low-Battery Notifications",
                "Warn before battery reaches critical charge levels.",
                pwr_cfg.low_battery_notifications,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().power.low_battery_notifications = v;
                        ctrl_c.persist_and_sync(None, "Updated low-battery notifications");
                    }
                },
            ),
        ],
    );
    root.append(&timeout_card);
}
