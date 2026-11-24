//! 5.18 Startup & Session: real XDG Autostart manager (`~/.config/autostart` +
//! `/etc/xdg/autostart`), session restore preferences, and confirmed `systemd-logind`
//! Suspend / Hibernate / Restart / Shutdown controls.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ScrolledWindow, Spinner, Switch, glib, prelude::*};
use neon_system::{
    apps::{self, AutostartEntry},
    power::{self, PowerStatus},
};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, confirm_action_dialog, info_row, page_shell, run_async,
        section_heading, setting_row, status_badge, toggle_row,
    },
};

struct StartupData {
    autostart: Vec<AutostartEntry>,
    power: PowerStatus,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Startup & Session",
        "Manage XDG login autostart applications, session restoration, and systemd-logind power actions.",
        Some("startup"),
        &ctrl,
    );

    let root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row(
        "Loading Startup & Session State…",
        "Reading XDG autostart entries and systemd-logind capabilities.",
        &spin,
    ));
    root.append(&load_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(
        || StartupData {
            autostart: apps::list_autostart_entries(),
            power: power::query_power_status(),
        },
        move |data| render_startup(&root_done, ctrl_done, data),
    );

    scroller
}

fn render_startup(root: &GtkBox, ctrl: Rc<SettingsController>, data: StartupData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Session Behavior ─────────────────────────────────────────────────────
    root.append(&section_heading("Session Preferences"));
    let sess_card = boxed_card();
    let sess_cfg = ctrl.config.borrow().session.clone();

    append_rows(
        &sess_card,
        vec![
            toggle_row(
                "Restore Previous Session on Login",
                "Remember open desktop workspace layout across logins.",
                sess_cfg.restore_session,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().session.restore_session = v;
                        ctrl_c.persist_and_sync(None, "Updated session restore preference");
                    }
                },
            ),
            info_row(
                "Active Desktop Session",
                "Current Wayland session registered with systemd-logind.",
                &std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| sess_cfg.default_session),
            ),
        ],
    );
    root.append(&sess_card);

    // ── System Power & Session Controls (systemd-logind) ─────────────────────
    root.append(&section_heading("Session & Power Controls (systemd-logind)"));
    let pwr_card = boxed_card();

    let btn_bar = GtkBox::new(Orientation::Horizontal, 8);
    btn_bar.set_valign(Align::Center);

    let suspend_btn = Button::with_label("Suspend");
    suspend_btn.add_css_class("settings-secondary-btn");
    suspend_btn.set_sensitive(data.power.can_suspend);
    {
        let ctrl_c = Rc::clone(&ctrl);
        suspend_btn.connect_clicked(move |_| {
            let ctrl_inner = Rc::clone(&ctrl_c);
            confirm_action_dialog(
                &ctrl_c.window,
                "Suspend Computer?",
                "The system will enter low-power sleep via systemd-logind.",
                "Suspend",
                false,
                move || {
                    let ctrl_done = Rc::clone(&ctrl_inner);
                    run_async(
                        || power::invoke_logind_action("Suspend"),
                        move |res| match res {
                            Ok(()) => ctrl_done.set_status_ok("Suspend requested"),
                            Err(e) => ctrl_done.set_status_err(&e),
                        },
                    );
                },
            );
        });
    }

    let hibernate_btn = Button::with_label("Hibernate");
    hibernate_btn.add_css_class("settings-secondary-btn");
    hibernate_btn.set_sensitive(data.power.can_hibernate);
    if !data.power.can_hibernate {
        hibernate_btn.set_tooltip_text(Some("Hibernation is not enabled/supported on this host"));
    }
    {
        let ctrl_c = Rc::clone(&ctrl);
        hibernate_btn.connect_clicked(move |_| {
            let ctrl_inner = Rc::clone(&ctrl_c);
            confirm_action_dialog(
                &ctrl_c.window,
                "Hibernate Computer?",
                "Saves RAM to swap and powers off via systemd-logind.",
                "Hibernate",
                false,
                move || {
                    let ctrl_done = Rc::clone(&ctrl_inner);
                    run_async(
                        || power::invoke_logind_action("Hibernate"),
                        move |res| match res {
                            Ok(()) => ctrl_done.set_status_ok("Hibernate requested"),
                            Err(e) => ctrl_done.set_status_err(&e),
                        },
                    );
                },
            );
        });
    }

    let reboot_btn = Button::with_label("Restart");
    reboot_btn.add_css_class("settings-secondary-btn");
    reboot_btn.set_sensitive(data.power.can_reboot);
    {
        let ctrl_c = Rc::clone(&ctrl);
        reboot_btn.connect_clicked(move |_| {
            let ctrl_inner = Rc::clone(&ctrl_c);
            confirm_action_dialog(
                &ctrl_c.window,
                "Restart Computer?",
                "All open applications will close and the system will reboot via systemd-logind.",
                "Restart",
                true,
                move || {
                    let ctrl_done = Rc::clone(&ctrl_inner);
                    run_async(
                        || power::invoke_logind_action("Reboot"),
                        move |res| match res {
                            Ok(()) => ctrl_done.set_status_ok("Reboot requested"),
                            Err(e) => ctrl_done.set_status_err(&e),
                        },
                    );
                },
            );
        });
    }

    let shutdown_btn = Button::with_label("Shut Down");
    shutdown_btn.add_css_class("settings-destructive-btn");
    shutdown_btn.set_sensitive(data.power.can_power_off);
    {
        let ctrl_c = Rc::clone(&ctrl);
        shutdown_btn.connect_clicked(move |_| {
            let ctrl_inner = Rc::clone(&ctrl_c);
            confirm_action_dialog(
                &ctrl_c.window,
                "Shut Down Computer?",
                "All open applications will close and the system will power off via systemd-logind.",
                "Shut Down",
                true,
                move || {
                    let ctrl_done = Rc::clone(&ctrl_inner);
                    run_async(
                        || power::invoke_logind_action("PowerOff"),
                        move |res| match res {
                            Ok(()) => ctrl_done.set_status_ok("Shutdown requested"),
                            Err(e) => ctrl_done.set_status_err(&e),
                        },
                    );
                },
            );
        });
    }

    btn_bar.append(&suspend_btn);
    btn_bar.append(&hibernate_btn);
    btn_bar.append(&reboot_btn);
    btn_bar.append(&shutdown_btn);

    pwr_card.append(&setting_row(
        "Power Actions",
        "Confirmed system transitions executed via org.freedesktop.login1.Manager.",
        &btn_bar,
    ));
    root.append(&pwr_card);

    // ── XDG Autostart Applications ───────────────────────────────────────────
    root.append(&section_heading(&format!(
        "Autostart Applications ({})",
        data.autostart.len()
    )));
    let auto_card = boxed_card();

    if data.autostart.is_empty() {
        auto_card.append(&setting_row(
            "No Autostart Entries Found",
            "Use Applications & Default Apps to add any installed app to ~/.config/autostart.",
            &status_badge("0 apps", ""),
        ));
    } else {
        let mut rows = Vec::new();
        for entry in data.autostart.into_iter().take(25) {
            let sw = Switch::new();
            sw.set_active(entry.enabled);
            sw.set_valign(Align::Center);
            let ctrl_c = Rc::clone(&ctrl);
            let fname = entry.file_name.clone();
            let app_name = entry.name.clone();
            sw.connect_state_set(move |_, active| {
                match apps::set_autostart_enabled(&fname, active) {
                    Ok(()) => ctrl_c.set_status_ok(&format!(
                        "{} autostart for {}",
                        if active { "Enabled" } else { "Disabled" },
                        app_name
                    )),
                    Err(e) => ctrl_c.set_status_err(&format!("Autostart update failed: {e}")),
                }
                glib::Propagation::Proceed
            });

            let scope = if entry.user_owned {
                "User (~/.config/autostart)"
            } else {
                "System (/etc/xdg/autostart)"
            };
            rows.push(setting_row(
                &entry.name,
                &format!("{scope} • {}", entry.exec),
                &sw,
            ));
        }
        append_rows(&auto_card, rows);
    }
    root.append(&auto_card);
}
