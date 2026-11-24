//! 5.21 Updates & Software: real OS/distro/kernel/package-manager discovery,
//! async `apt list --upgradable` viewer, configured software repositories,
//! update notification preference, and clean handoff to the host distribution updater
//! (never replacing the Linux package manager).

use std::{process::Command, rc::Rc};

use gtk4::{Align, Box as GtkBox, Button, Orientation, ScrolledWindow, Spinner, prelude::*};
use neon_system::updates::{self, UpdatesSnapshot};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, info_row, page_shell, run_async, section_heading, setting_row,
        status_badge, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Updates & Software",
        "Inspect system version, available distribution package updates, and configured software repositories.",
        None,
        &ctrl,
    );

    let root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row(
        "Checking System Packages & Updates…",
        "Querying host package manager metadata off the UI thread.",
        &spin,
    ));
    root.append(&load_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(updates::query_updates_snapshot, move |snap| {
        render_updates(&root_done, ctrl_done, snap);
    });

    scroller
}

fn render_updates(root: &GtkBox, ctrl: Rc<SettingsController>, snap: UpdatesSnapshot) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── System & Update Status ───────────────────────────────────────────────
    root.append(&section_heading("System Update Status"));
    let summary_card = boxed_card();

    let action_box = GtkBox::new(Orientation::Horizontal, 8);
    action_box.set_valign(Align::Center);

    let recheck_btn = Button::with_label("Check Again");
    recheck_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        recheck_btn.connect_clicked(move |_| {
            ctrl_c.set_status_info("Rechecking host package updates…");
            ctrl_c.rebuild_page("updates");
        });
    }

    let open_updater_btn = Button::with_label("Open System Updater");
    open_updater_btn.add_css_class("settings-action-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        open_updater_btn.connect_clicked(move |_| {
            let launched = [
                "update-manager",
                "gnome-software",
                "plasma-discover",
                "software-properties-gtk",
            ]
            .iter()
            .any(|bin| Command::new(bin).spawn().is_ok());
            if launched {
                ctrl_c.set_status_ok("Launched native host software updater");
            } else {
                ctrl_c.set_status_info(
                    "No GUI updater found — run `sudo apt update && sudo apt upgrade` in a terminal",
                );
            }
        });
    }
    action_box.append(&recheck_btn);
    action_box.append(&open_updater_btn);

    let upd_badge = if snap.pending_updates.is_empty() {
        status_badge("Up to Date", "ok")
    } else {
        status_badge(
            &format!("{} Available", snap.pending_updates.len()),
            "partial",
        )
    };

    let notif_on = ctrl.config.borrow().session.update_notifications;

    append_rows(
        &summary_card,
        vec![
            setting_row(
                &snap.os_pretty_name,
                &format!("Kernel {} • Package Manager: {}", snap.kernel, snap.package_manager),
                &upd_badge,
            ),
            info_row(
                "Installed Packages",
                "Total packages tracked by the host distribution package database.",
                &snap.installed_package_count.to_string(),
            ),
            setting_row(
                "Host Package Manager Integration",
                "NEON DE delegates package installation cleanly to your distribution's native updater.",
                &action_box,
            ),
            toggle_row(
                "Update Notifications",
                "Notify when security or desktop package updates are available.",
                notif_on,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().session.update_notifications = v;
                        ctrl_c.persist_and_sync(None, "Updated update-notification preference");
                    }
                },
            ),
        ],
    );
    root.append(&summary_card);

    // ── Available Updates List ───────────────────────────────────────────────
    if !snap.pending_updates.is_empty() {
        root.append(&section_heading(&format!(
            "Pending Updates ({})",
            snap.pending_updates.len()
        )));
        let list_card = boxed_card();
        let mut rows = Vec::new();
        for u in snap.pending_updates.into_iter().take(25) {
            rows.push(info_row(&u.package, &u.detail, &u.new_version));
        }
        append_rows(&list_card, rows);
        root.append(&list_card);
    }

    // ── Software Repositories ────────────────────────────────────────────────
    root.append(&section_heading("Configured Software Sources"));
    let src_card = boxed_card();
    if snap.software_sources.is_empty() {
        src_card.append(&info_row(
            "Software Sources",
            "Managed by host distribution package manager.",
            "—",
        ));
    } else {
        let mut rows = Vec::new();
        for src in snap.software_sources.into_iter().take(16) {
            rows.push(info_row(&src, "Active repository source file", "Enabled"));
        }
        append_rows(&src_card, rows);
    }
    root.append(&src_card);
}
