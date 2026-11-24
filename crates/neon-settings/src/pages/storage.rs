//! 5.20 Storage & Devices: UDisks2/`lsblk`/`df` disks & mounted volumes with capacity bars,
//! safe removable media Eject (`udisksctl`), auto-mount preferences, CUPS printers (`lpstat`),
//! and SANE scanner capability detection.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ProgressBar, ScrolledWindow, Spinner, prelude::*};
use neon_system::storage::{self, StorageAndDevicesSnapshot};

use crate::{
    metadata::CapabilityStatus,
    state::SettingsController,
    ui::{
        append_rows, boxed_card, capability_row, info_row, page_shell, run_async, section_heading,
        setting_row, status_badge, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Storage & Devices",
        "Inspect disks, partitions, removable drives, CUPS printers, and connected scanners.",
        Some("storage"),
        &ctrl,
    );

    let root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row(
        "Scanning Storage & Peripheral Devices…",
        "Querying mounted filesystems, UDisks2, CUPS printers, and SANE scanners.",
        &spin,
    ));
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

    // ── Disks & Partitions ───────────────────────────────────────────────────
    let hdr_row = GtkBox::new(Orientation::Horizontal, 8);
    let sec = section_heading(&format!("Disks & Mounted Volumes ({})", snap.devices.len()));
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
        disk_card.append(&info_row(
            "No Block Partitions Detected",
            "lsblk did not report mounted or formatted partitions.",
            "—",
        ));
    } else {
        let mut rows = Vec::new();
        for dev in snap.devices {
            let right = GtkBox::new(Orientation::Horizontal, 10);
            right.set_valign(Align::Center);

            if dev.mountpoint.is_some() {
                let bar = ProgressBar::new();
                bar.set_fraction(f64::from(dev.used_percent) / 100.0);
                bar.set_width_request(130);
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
                    ctrl_c.set_status_info(&format!("Safely ejecting {p}…"));
                    run_async(
                        move || storage::eject_removable_device(&p),
                        move |res| match res {
                            Ok(()) => {
                                ctrl_done.set_status_ok("Safely ejected removable drive");
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
                    "{} • {} used / {} total ({} free){}",
                    dev.fstype,
                    dev.used_human,
                    dev.size_human,
                    dev.avail_human,
                    if dev.removable { " • Removable" } else { "" }
                )
            } else {
                format!(
                    "{} • {} total{}",
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

    // ── Removable Media Preferences ──────────────────────────────────────────
    root.append(&section_heading("Removable Media"));
    let rm_card = boxed_card();
    let st_cfg = ctrl.config.borrow().storage.clone();
    append_rows(
        &rm_card,
        vec![
            toggle_row(
                "Automatically Mount Removable Drives",
                "Mount USB flash drives and SD cards via UDisks2 when plugged in.",
                st_cfg.automount_removable,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().storage.automount_removable = v;
                        ctrl_c.persist_and_sync(None, "Updated removable media auto-mount");
                    }
                },
            ),
            toggle_row(
                "Open File Manager on Mount",
                "Launch default file manager when a new removable volume is mounted.",
                st_cfg.automount_open_files,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().storage.automount_open_files = v;
                        ctrl_c.persist_and_sync(None, "Updated auto-open on mount preference");
                    }
                },
            ),
        ],
    );
    root.append(&rm_card);

    // ── Printers (CUPS) & Scanners (SANE) ────────────────────────────────────
    root.append(&section_heading("Printers & Scanners"));
    let periph_card = boxed_card();
    let mut p_rows = Vec::new();

    if !snap.cups_available {
        p_rows.push(capability_row(
            "CUPS Printing Service",
            "CUPS (`lpstat`) is not installed or unavailable on this system.",
            CapabilityStatus::TemporarilyUnavailable,
        ));
    } else if snap.printers.is_empty() {
        p_rows.push(setting_row(
            "CUPS Printers",
            "CUPS daemon is running, but no printers are currently configured.",
            &status_badge("0 Printers", "ok"),
        ));
    } else {
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
            p_rows.push(setting_row(&pr.name, &sub, &badge));
        }
    }

    if !snap.sane_available {
        p_rows.push(capability_row(
            "Document Scanners (SANE)",
            "Install `sane-utils` (`scanimage`) to detect and manage local/network scanners.",
            CapabilityStatus::Unsupported("`scanimage` utility is not installed on this system."),
        ));
    } else if snap.scanners.is_empty() {
        p_rows.push(setting_row(
            "Document Scanners (SANE)",
            "SANE is available; no connected scanners were detected.",
            &status_badge("0 Scanners", ""),
        ));
    } else {
        for sc in snap.scanners {
            p_rows.push(info_row("Detected Scanner", "Reported by SANE (`scanimage -L`).", &sc));
        }
    }

    append_rows(&periph_card, p_rows);
    root.append(&periph_card);
}
