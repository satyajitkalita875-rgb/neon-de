//! Storage & Printers settings: mounted disks & capacity bars, safe removable drive
//! ejection (`udisksctl`), and active CUPS printers (`lpstat`).

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ProgressBar, ScrolledWindow, Spinner, prelude::*};
use neon_system::storage::{self, StorageAndDevicesSnapshot};

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
    load_card.append(&setting_row("Scanning Storage & Printers…", "", &spin));
    root.append(&load_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(storage::query_storage_and_devices, move |snap| {
        render_storage(&root_done, ctrl_done, snap);
    });

    scroller
}

fn render_storage(root: &GtkBox, ctrl: Rc<SettingsController>, snap: StorageAndDevicesSnapshot) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Mounted Disks & Volumes ──────────────────────────────────────────────
    let hdr_row = GtkBox::new(Orientation::Horizontal, 8);
    let sec = section_heading(&format!("Disks & Volumes ({})", snap.devices.len()));
    sec.set_hexpand(true);
    let refresh_btn = Button::with_label("Refresh");
    refresh_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        refresh_btn.connect_clicked(move |_| ctrl_c.rebuild_page("storage"));
    }
    hdr_row.append(&sec);
    hdr_row.append(&refresh_btn);
    root.append(&hdr_row);

    let disk_card = boxed_card();
    if snap.devices.is_empty() {
        disk_card.append(&info_row("No Mounted Volumes", "", "—"));
    } else {
        let mut rows = Vec::new();
        for dev in snap.devices {
            let right = GtkBox::new(Orientation::Horizontal, 8);
            right.set_valign(Align::Center);

            if dev.mountpoint.is_some() {
                let bar = ProgressBar::new();
                bar.set_fraction(f64::from(dev.used_percent) / 100.0);
                bar.set_width_request(120);
                bar.set_show_text(true);
                bar.set_text(Some(&format!("{}%", dev.used_percent)));
                right.append(&bar);
            } else {
                right.append(&status_badge("Unmounted", ""));
            }

            if dev.removable && snap.udisks_available {
                let eject_btn = Button::with_label("Eject");
                eject_btn.add_css_class("settings-secondary-btn");
                let ctrl_c = Rc::clone(&ctrl);
                let dev_path = dev.device.clone();
                eject_btn.connect_clicked(move |_| {
                    let ctrl_done = Rc::clone(&ctrl_c);
                    let p = dev_path.clone();
                    ctrl_c.set_status_info(&format!("Ejecting {p}…"));
                    run_async(
                        move || storage::eject_removable_device(&p),
                        move |res| match res {
                            Ok(()) => {
                                ctrl_done.set_status_ok("Safely ejected drive");
                                ctrl_done.rebuild_page("storage");
                            }
                            Err(e) => ctrl_done.set_status_err(&e),
                        },
                    );
                });
                right.append(&eject_btn);
            }

            let title = match &dev.mountpoint {
                Some(mp) => format!("{}  ({})", mp, dev.device),
                None => dev.device.clone(),
            };
            let sub = if dev.mountpoint.is_some() {
                format!(
                    "{} • {} used / {} ({} free){}",
                    dev.fstype,
                    dev.used_human,
                    dev.size_human,
                    dev.avail_human,
                    if dev.removable { " • Removable" } else { "" }
                )
            } else {
                format!(
                    "{} • {}{}",
                    dev.fstype,
                    dev.size_human,
                    if dev.removable { " • Removable" } else { "" }
                )
            };
            rows.push(setting_row(&title, &sub, &right));
        }
        append_rows(&disk_card, rows);
    }
    root.append(&disk_card);

    // ── CUPS Printers ────────────────────────────────────────────────────────
    if snap.cups_available {
        root.append(&section_heading("Printers"));
        let pr_card = boxed_card();
        if snap.printers.is_empty() {
            pr_card.append(&setting_row(
                "Printers",
                "CUPS is running; no printers configured.",
                &status_badge("0 Printers", ""),
            ));
        } else {
            let mut rows = Vec::new();
            for pr in snap.printers {
                let badge = if pr.is_default {
                    status_badge("Default", "ok")
                } else {
                    status_badge("Ready", "")
                };
                let sub = match pr.uri {
                    Some(uri) => format!("{} • {}", pr.status, uri),
                    None => pr.status,
                };
                rows.push(setting_row(&pr.name, &sub, &badge));
            }
            append_rows(&pr_card, rows);
        }
        root.append(&pr_card);
    }
}
