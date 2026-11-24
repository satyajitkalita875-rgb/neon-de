//! Bluetooth settings: BlueZ adapter power, nearby discovery, and paired/nearby device management.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ScrolledWindow, Spinner, Switch, glib, prelude::*};
use neon_system::bluetooth::{self, BluetoothDevice, BluetoothStatus};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, confirm_action_dialog, info_row, page_shell, run_async,
        section_heading, setting_row, status_badge,
    },
};

struct BtSnapshot {
    status: BluetoothStatus,
    all_devices: Vec<BluetoothDevice>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    let root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&root);

    let loading_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    loading_card.append(&setting_row("Loading Bluetooth…", "", &spin));
    root.append(&loading_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(
        || BtSnapshot {
            status: bluetooth::query_bluetooth_status(),
            all_devices: bluetooth::list_all_bluetooth_devices(),
        },
        move |snap| render_bluetooth(&root_done, ctrl_done, snap),
    );

    scroller
}

fn render_bluetooth(root: &GtkBox, ctrl: Rc<SettingsController>, snap: BtSnapshot) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    root.append(&section_heading("Adapter"));
    let adapter_card = boxed_card();

    let power_sw = Switch::new();
    power_sw.set_active(snap.status.powered);
    power_sw.set_sensitive(snap.status.available);
    power_sw.set_valign(Align::Center);
    {
        let ctrl_c = Rc::clone(&ctrl);
        power_sw.connect_state_set(move |_, active| {
            let ctrl_done = Rc::clone(&ctrl_c);
            run_async(
                move || bluetooth::set_bluetooth_powered(active),
                move |res| match res {
                    Ok(()) => {
                        ctrl_done.set_status_ok(if active {
                            "Bluetooth on"
                        } else {
                            "Bluetooth off"
                        });
                        ctrl_done.rebuild_page("bluetooth");
                    }
                    Err(e) => ctrl_done.set_status_err(&format!("Bluetooth failed: {e}")),
                },
            );
            glib::Propagation::Proceed
        });
    }

    let scan_btn = Button::with_label(if snap.status.discovering {
        "Stop Scanning"
    } else {
        "Scan"
    });
    scan_btn.add_css_class("settings-secondary-btn");
    scan_btn.set_sensitive(snap.status.available && snap.status.powered);
    {
        let ctrl_c = Rc::clone(&ctrl);
        let enable_disc = !snap.status.discovering;
        scan_btn.connect_clicked(move |_| {
            let ctrl_done = Rc::clone(&ctrl_c);
            run_async(
                move || bluetooth::set_bluetooth_discovery(enable_disc),
                move |res| match res {
                    Ok(()) => {
                        ctrl_done.set_status_ok(if enable_disc {
                            "Scanning for devices…"
                        } else {
                            "Stopped scanning"
                        });
                        ctrl_done.rebuild_page("bluetooth");
                    }
                    Err(e) => ctrl_done.set_status_err(&format!("Scan failed: {e}")),
                },
            );
        });
    }

    let refresh_btn = Button::with_label("Refresh");
    refresh_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        refresh_btn.connect_clicked(move |_| ctrl_c.rebuild_page("bluetooth"));
    }

    let scan_controls = GtkBox::new(Orientation::Horizontal, 6);
    scan_controls.set_valign(Align::Center);
    scan_controls.append(&scan_btn);
    scan_controls.append(&refresh_btn);

    let adapter_name = snap
        .status
        .adapter_alias
        .clone()
        .unwrap_or_else(|| "No Bluetooth adapter found".into());

    append_rows(
        &adapter_card,
        vec![
            setting_row("Bluetooth", &adapter_name, &power_sw),
            setting_row(
                "Nearby Devices",
                if snap.status.discovering {
                    "Discovering nearby peripherals…"
                } else {
                    "Search for headphones, keyboards, mice, and speakers."
                },
                &scan_controls,
            ),
        ],
    );
    root.append(&adapter_card);

    root.append(&section_heading("Devices"));
    let dev_card = boxed_card();

    if !snap.status.available {
        dev_card.append(&info_row("Bluetooth Unavailable", "BlueZ service is offline.", "Offline"));
    } else if snap.all_devices.is_empty() {
        dev_card.append(&info_row(
            "No Devices",
            "Turn on Bluetooth and click Scan to discover devices.",
            "—",
        ));
    } else {
        let mut rows = Vec::new();
        for dev in snap.all_devices.into_iter().take(20) {
            let right = GtkBox::new(Orientation::Horizontal, 6);
            right.set_valign(Align::Center);

            if dev.connected {
                right.append(&status_badge("Connected", "ok"));
                let btn = Button::with_label("Disconnect");
                btn.add_css_class("settings-secondary-btn");
                let ctrl_c = Rc::clone(&ctrl);
                let path = dev.object_path.clone();
                btn.connect_clicked(move |_| {
                    let ctrl_done = Rc::clone(&ctrl_c);
                    let p = path.clone();
                    run_async(
                        move || bluetooth::set_device_connected(&p, false),
                        move |res| match res {
                            Ok(()) => {
                                ctrl_done.set_status_ok("Disconnected");
                                ctrl_done.rebuild_page("bluetooth");
                            }
                            Err(e) => ctrl_done.set_status_err(&format!("Disconnect failed: {e}")),
                        },
                    );
                });
                right.append(&btn);
            } else if dev.paired {
                let conn_btn = Button::with_label("Connect");
                conn_btn.add_css_class("settings-action-btn");
                let ctrl_c = Rc::clone(&ctrl);
                let path = dev.object_path.clone();
                conn_btn.connect_clicked(move |_| {
                    let ctrl_done = Rc::clone(&ctrl_c);
                    let p = path.clone();
                    ctrl_c.set_status_info("Connecting…");
                    run_async(
                        move || bluetooth::set_device_connected(&p, true),
                        move |res| match res {
                            Ok(()) => {
                                ctrl_done.set_status_ok("Connected");
                                ctrl_done.rebuild_page("bluetooth");
                            }
                            Err(e) => ctrl_done.set_status_err(&format!("Connect failed: {e}")),
                        },
                    );
                });

                let rem_btn = Button::with_label("Forget");
                rem_btn.add_css_class("settings-secondary-btn");
                let ctrl_r = Rc::clone(&ctrl);
                let path_r = dev.object_path.clone();
                let name_r = dev.name.clone();
                rem_btn.connect_clicked(move |_| {
                    let ctrl_inner = Rc::clone(&ctrl_r);
                    let p = path_r.clone();
                    confirm_action_dialog(
                        &ctrl_r.window,
                        &format!("Forget {name_r}?"),
                        "This removes the paired Bluetooth device.",
                        "Forget",
                        true,
                        move || {
                            let ctrl_done = Rc::clone(&ctrl_inner);
                            run_async(
                                move || bluetooth::remove_device(&p),
                                move |res| match res {
                                    Ok(()) => {
                                        ctrl_done.set_status_ok("Removed device");
                                        ctrl_done.rebuild_page("bluetooth");
                                    }
                                    Err(e) => ctrl_done.set_status_err(&format!("Remove failed: {e}")),
                                },
                            );
                        },
                    );
                });
                right.append(&conn_btn);
                right.append(&rem_btn);
            } else {
                let pair_btn = Button::with_label("Pair");
                pair_btn.add_css_class("settings-action-btn");
                let ctrl_c = Rc::clone(&ctrl);
                let path = dev.object_path.clone();
                pair_btn.connect_clicked(move |_| {
                    let ctrl_done = Rc::clone(&ctrl_c);
                    let p = path.clone();
                    ctrl_c.set_status_info("Pairing…");
                    run_async(
                        move || bluetooth::pair_device(&p),
                        move |res| match res {
                            Ok(()) => {
                                ctrl_done.set_status_ok("Paired");
                                ctrl_done.rebuild_page("bluetooth");
                            }
                            Err(e) => ctrl_done.set_status_err(&format!("Pairing failed: {e}")),
                        },
                    );
                });
                right.append(&pair_btn);
            }

            let state_txt = if dev.connected {
                "Connected"
            } else if dev.paired {
                "Not Connected"
            } else {
                "Nearby"
            };
            rows.push(setting_row(
                &dev.name,
                &format!("{} • {}", dev.address, state_txt),
                &right,
            ));
        }
        append_rows(&dev_card, rows);
    }
    root.append(&dev_card);
}
