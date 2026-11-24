//! 5.11 Notifications settings: enable/disable, Do Not Disturb, previews, duration,
//! banner position, sounds, lock-screen notifications, muted apps, live D-Bus service status,
//! and real test notification dispatch.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Entry, Orientation, ScrolledWindow, prelude::*};
use neon_system::{
    SystemAction, execute_system_action,
    notifications::query_notification_service_status,
};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, dropdown_row, page_shell, run_async, section_heading, setting_row,
        slider_row, status_badge, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Notifications",
        "Control Do Not Disturb, banner placement, preview privacy, sounds, and per-application permissions.",
        Some("notifications"),
        &ctrl,
    );

    let notif_cfg = ctrl.config.borrow().notifications.clone();
    let bus_status = query_notification_service_status();

    // ── Service & Test ───────────────────────────────────────────────────────
    page.append(&section_heading("Notification Service"));
    let svc_card = boxed_card();

    let active_bus = bus_status
        .active_bus_name
        .clone()
        .unwrap_or_else(|| "org.freedesktop.Notifications".into());
    let notif_active = bus_status.standard_bus_owned || bus_status.neon_bus_owned;

    let test_btn = Button::with_label("Send Test Notification");
    test_btn.add_css_class("settings-action-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        let bus_name = active_bus.clone();
        test_btn.connect_clicked(move |_| {
            let ctrl_done = Rc::clone(&ctrl_c);
            let bus = bus_name.clone();
            run_async(
                move || execute_system_action(&SystemAction::SendTestNotification { bus_name: bus }),
                move |res| match res {
                    Ok(()) => ctrl_done.set_status_ok("Sent test notification via D-Bus"),
                    Err(e) => ctrl_done.set_status_err(&format!("Notification failed: {e}")),
                },
            );
        });
    }

    append_rows(
        &svc_card,
        vec![
            setting_row(
                "Freedesktop Notifications v1.2",
                &format!("Active bus: {active_bus}"),
                &status_badge(
                    if notif_active { "Active" } else { "Standby" },
                    if notif_active { "ok" } else { "" },
                ),
            ),
            setting_row(
                "Test Banner Delivery",
                "Dispatches a real D-Bus notification to verify sound and popup layout.",
                &test_btn,
            ),
        ],
    );
    page.append(&svc_card);

    // ── Behavior & Appearance ────────────────────────────────────────────────
    page.append(&section_heading("Delivery & Appearance"));
    let pref_card = boxed_card();

    let positions = ["top_right", "top_center", "bottom_right", "bottom_left"];
    let pos_idx = positions
        .iter()
        .position(|p| *p == notif_cfg.position.as_str())
        .unwrap_or(0) as u32;

    append_rows(
        &pref_card,
        vec![
            toggle_row(
                "Enable Desktop Notifications",
                "Allow applications and system services to show toast banners.",
                notif_cfg.enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().notifications.enabled = v;
                        ctrl_c.persist_and_sync(None, "Updated desktop notifications");
                    }
                },
            ),
            toggle_row(
                "Do Not Disturb",
                "Silence all incoming banners while keeping notifications in history.",
                notif_cfg.do_not_disturb,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().notifications.do_not_disturb = v;
                        ctrl_c.persist_and_sync(
                            None,
                            if v {
                                "Do Not Disturb enabled"
                            } else {
                                "Do Not Disturb disabled"
                            },
                        );
                    }
                },
            ),
            toggle_row(
                "Show Message Previews",
                "Display full notification body text inside popup banners.",
                notif_cfg.show_previews,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().notifications.show_previews = v;
                        ctrl_c.persist_and_sync(None, "Updated notification previews");
                    }
                },
            ),
            toggle_row(
                "Notification Sounds",
                "Play an alert sound when new notifications arrive.",
                notif_cfg.sounds_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().notifications.sounds_enabled = v;
                        ctrl_c.persist_and_sync(None, "Updated notification sounds");
                    }
                },
            ),
            toggle_row(
                "Lock Screen Notifications",
                "Allow notification indicators while the session is locked.",
                notif_cfg.lock_screen_notifications,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().notifications.lock_screen_notifications = v;
                        ctrl_c.persist_and_sync(None, "Updated lock-screen notifications");
                    }
                },
            ),
            dropdown_row(
                "Notification Position",
                "Screen corner where notification banners appear.",
                &["Top Right", "Top Center", "Bottom Right", "Bottom Left"],
                pos_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let p = positions.get(idx as usize).copied().unwrap_or("top_right");
                        ctrl_c.config.borrow_mut().notifications.position = p.into();
                        ctrl_c.persist_and_sync(None, "Updated notification position");
                    }
                },
            ),
            slider_row(
                "Banner Duration (seconds)",
                "How long banners remain visible before auto-dismissing.",
                2.0,
                20.0,
                1.0,
                f64::from(notif_cfg.duration_secs),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().notifications.duration_secs = v.round() as u32;
                        ctrl_c.persist_and_sync(None, &format!("Notification duration: {:.0}s", v));
                    }
                },
            ),
        ],
    );
    page.append(&pref_card);

    // ── Per-Application Muting ───────────────────────────────────────────────
    page.append(&section_heading("Per-Application Notification Permissions"));
    let apps_card = boxed_card();

    let mute_entry = Entry::new();
    mute_entry.set_placeholder_text(Some("App name to mute (e.g. discord, slack)"));
    let add_mute_btn = Button::with_label("Mute App");
    add_mute_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        let e = mute_entry.clone();
        add_mute_btn.connect_clicked(move |_| {
            let app = e.text().trim().to_string();
            if app.is_empty() {
                return;
            }
            {
                let mut cfg = ctrl_c.config.borrow_mut();
                if !cfg.notifications.muted_apps.iter().any(|a| a.eq_ignore_ascii_case(&app)) {
                    cfg.notifications.muted_apps.push(app.clone());
                }
            }
            ctrl_c.persist_and_sync(None, &format!("Muted notifications from {app}"));
            ctrl_c.rebuild_page("notifications");
        });
    }
    let add_box = GtkBox::new(Orientation::Horizontal, 8);
    add_box.set_valign(Align::Center);
    add_box.append(&mute_entry);
    add_box.append(&add_mute_btn);

    let mut mute_rows = vec![setting_row(
        "Mute Specific Application",
        "Suppress banners from noisy applications while keeping others active.",
        &add_box,
    )];

    for (idx, app) in notif_cfg.muted_apps.into_iter().enumerate() {
        let unmute_btn = Button::with_label("Allow");
        unmute_btn.add_css_class("settings-secondary-btn");
        let ctrl_c = Rc::clone(&ctrl);
        unmute_btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().notifications.muted_apps.remove(idx);
            ctrl_c.persist_and_sync(None, "Unmuted application notifications");
            ctrl_c.rebuild_page("notifications");
        });
        mute_rows.push(setting_row(&app, "Notifications muted", &unmute_btn));
    }

    append_rows(&apps_card, mute_rows);
    page.append(&apps_card);

    scroller
}
