//! 5.15 Users & Accounts: AccountsService D-Bus integration (`org.freedesktop.Accounts`),
//! current user card, polkit-gated display name edit, local user list, and honest security boundaries.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Entry, Orientation, ScrolledWindow, Spinner, prelude::*};
use neon_system::users::{self, UsersSnapshot};

use crate::{
    metadata::CapabilityStatus,
    state::SettingsController,
    ui::{
        append_rows, boxed_card, capability_row, info_row, page_shell, run_async, section_heading,
        setting_row, status_badge,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Users & Accounts",
        "Inspect your Linux account, role, shell, and local desktop users via AccountsService.",
        None,
        &ctrl,
    );

    let root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row(
        "Querying AccountsService…",
        "Loading local desktop accounts from org.freedesktop.Accounts.",
        &spin,
    ));
    root.append(&load_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(users::query_users, move |snap| {
        render_users(&root_done, ctrl_done, snap);
    });

    scroller
}

fn render_users(root: &GtkBox, ctrl: Rc<SettingsController>, snap: UsersSnapshot) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    root.append(&section_heading("Current Account"));
    let cur_card = boxed_card();

    if let Some(cur) = snap.current_user {
        let name_entry = Entry::new();
        name_entry.set_width_chars(22);
        name_entry.set_text(&cur.real_name);

        let save_btn = Button::with_label("Update Name");
        save_btn.add_css_class("settings-action-btn");
        save_btn.set_sensitive(cur.object_path.is_some());
        if let Some(obj_path) = cur.object_path.clone() {
            let ctrl_c = Rc::clone(&ctrl);
            let entry_c = name_entry.clone();
            save_btn.connect_clicked(move |_| {
                let new_name = entry_c.text().trim().to_string();
                let ctrl_done = Rc::clone(&ctrl_c);
                let p = obj_path.clone();
                run_async(
                    move || users::set_user_real_name(&p, &new_name),
                    move |res| match res {
                        Ok(()) => {
                            ctrl_done.set_status_ok("Updated display name via AccountsService");
                            ctrl_done.rebuild_page("users");
                        }
                        Err(e) => ctrl_done.set_status_err(&e),
                    },
                );
            });
        }

        let name_box = GtkBox::new(Orientation::Horizontal, 8);
        name_box.set_valign(Align::Center);
        name_box.append(&name_entry);
        name_box.append(&save_btn);

        let role_badge = if cur.is_admin {
            status_badge("Administrator", "elevated")
        } else {
            status_badge("Standard User", "")
        };

        append_rows(
            &cur_card,
            vec![
                setting_row(
                    &format!("{} ({})", cur.real_name, cur.username),
                    &format!("UID {} • Home: {}", cur.uid, cur.home_dir),
                    &role_badge,
                ),
                setting_row(
                    "Display Name",
                    "Updates your account RealName through AccountsService (polkit-authorized).",
                    &name_box,
                ),
                info_row("Login Shell", "Default interactive login shell.", &cur.shell),
                capability_row(
                    "Change Password & Account Creation",
                    "Account creation, deletion, and password changes require PolicyKit administrator elevation (use `passwd` or system user tools).",
                    CapabilityStatus::RequiresElevation,
                ),
            ],
        );
    } else {
        cur_card.append(&info_row(
            "Current User",
            "Could not resolve active user entry from AccountsService or /etc/passwd.",
            "Unknown",
        ));
    }
    root.append(&cur_card);

    root.append(&section_heading("Local Desktop Accounts"));
    let list_card = boxed_card();
    if snap.all_users.is_empty() {
        list_card.append(&info_row("No Local Users Found", "", "0"));
    } else {
        let mut rows = Vec::new();
        for u in snap.all_users {
            let tag = if u.is_current {
                status_badge("Signed In", "ok")
            } else if u.is_admin {
                status_badge("Admin", "elevated")
            } else {
                status_badge("Standard", "")
            };
            rows.push(setting_row(
                &format!("{} (@{})", u.real_name, u.username),
                &format!("UID {} • {}", u.uid, u.home_dir),
                &tag,
            ));
        }
        append_rows(&list_card, rows);
    }
    root.append(&list_card);
}
