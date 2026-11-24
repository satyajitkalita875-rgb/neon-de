//! Wi-Fi & Network settings: live NetworkManager status, async Wi-Fi scanning,
//! secure password dialog, active IPv4/DNS details, and saved connections.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, PasswordEntry, ScrolledWindow, Spinner,
    Switch, Window, glib, prelude::*,
};
use neon_system::network::{
    self, NetworkDetails, NetworkStatus, SavedConnection, WifiAccessPoint,
};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, info_row, page_shell, run_async, section_heading, setting_row,
        status_badge,
    },
};

struct NetworkSnapshotData {
    status: NetworkStatus,
    aps: Vec<WifiAccessPoint>,
    saved: Vec<SavedConnection>,
    details: Vec<NetworkDetails>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    let dynamic_root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&dynamic_root);

    let loading_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    loading_card.append(&setting_row("Loading Network…", "", &spin));
    dynamic_root.append(&loading_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = dynamic_root.clone();
    run_async(
        || NetworkSnapshotData {
            status: network::query_network_status(),
            aps: network::list_wifi_networks(false),
            saved: network::list_saved_connections(),
            details: network::query_network_details(),
        },
        move |data| render_network_data(&root_done, ctrl_done, data),
    );

    scroller
}

fn render_network_data(root: &GtkBox, ctrl: Rc<SettingsController>, data: NetworkSnapshotData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Wi-Fi & Status ───────────────────────────────────────────────────────
    root.append(&section_heading("Wi-Fi"));
    let radio_card = boxed_card();

    let wifi_sw = Switch::new();
    wifi_sw.set_active(data.status.wifi_enabled);
    wifi_sw.set_sensitive(data.status.available);
    wifi_sw.set_valign(Align::Center);
    {
        let ctrl_c = Rc::clone(&ctrl);
        wifi_sw.connect_state_set(move |_, active| {
            let ctrl_res = Rc::clone(&ctrl_c);
            run_async(
                move || network::set_wifi_enabled(active),
                move |res| match res {
                    Ok(()) => {
                        ctrl_res.set_status_ok(if active {
                            "Wi-Fi enabled"
                        } else {
                            "Wi-Fi disabled"
                        });
                        ctrl_res.rebuild_page("network");
                    }
                    Err(e) => ctrl_res.set_status_err(&format!("Wi-Fi toggle failed: {e}")),
                },
            );
            glib::Propagation::Proceed
        });
    }

    let scan_btn = Button::with_label("Scan");
    scan_btn.add_css_class("settings-secondary-btn");
    scan_btn.set_sensitive(data.status.available && data.status.wifi_enabled);
    {
        let ctrl_c = Rc::clone(&ctrl);
        let root_c = root.clone();
        scan_btn.connect_clicked(move |_| {
            ctrl_c.set_status_info("Scanning nearby Wi-Fi networks…");
            let ctrl_done = Rc::clone(&ctrl_c);
            let root_done = root_c.clone();
            run_async(
                || NetworkSnapshotData {
                    status: network::query_network_status(),
                    aps: network::list_wifi_networks(true),
                    saved: network::list_saved_connections(),
                    details: network::query_network_details(),
                },
                move |fresh| {
                    ctrl_done.set_status_ok("Wi-Fi scan complete");
                    render_network_data(&root_done, ctrl_done, fresh);
                },
            );
        });
    }

    let status_badge_w = if !data.status.available {
        status_badge("Offline", "unavailable")
    } else if data.status.connectivity.is_connected() {
        status_badge(&data.status.panel_label(), "ok")
    } else {
        status_badge("Disconnected", "")
    };

    append_rows(
        &radio_card,
        vec![
            setting_row("Status", "", &status_badge_w),
            setting_row("Wi-Fi", "", &wifi_sw),
            setting_row("Nearby Networks", "Refresh wireless access points.", &scan_btn),
        ],
    );
    root.append(&radio_card);

    // ── Available Wi-Fi Networks ─────────────────────────────────────────────
    if data.status.wifi_enabled {
        root.append(&section_heading("Available Networks"));
        let wifi_card = boxed_card();
        if data.aps.is_empty() {
            wifi_card.append(&info_row(
                "No Networks Found",
                "Click Scan above to search for nearby Wi-Fi.",
                "—",
            ));
        } else {
            let mut rows = Vec::new();
            for ap in data.aps.into_iter().take(12) {
                let right = GtkBox::new(Orientation::Horizontal, 8);
                right.set_valign(Align::Center);
                let sig = status_badge(
                    &format!("{}%", ap.strength),
                    if ap.in_use { "ok" } else { "" },
                );
                right.append(&sig);

                if ap.in_use {
                    let disc_btn = Button::with_label("Disconnect");
                    disc_btn.add_css_class("settings-secondary-btn");
                    let ctrl_c = Rc::clone(&ctrl);
                    let ssid = ap.ssid.clone();
                    disc_btn.connect_clicked(move |_| {
                        let ctrl_done = Rc::clone(&ctrl_c);
                        let s = ssid.clone();
                        run_async(
                            move || network::set_connection_active(&s, false),
                            move |res| match res {
                                Ok(()) => {
                                    ctrl_done.set_status_ok("Disconnected");
                                    ctrl_done.rebuild_page("network");
                                }
                                Err(e) => ctrl_done.set_status_err(&format!("Disconnect failed: {e}")),
                            },
                        );
                    });
                    right.append(&disc_btn);
                } else {
                    let conn_btn = Button::with_label("Connect");
                    conn_btn.add_css_class("settings-action-btn");
                    let ctrl_c = Rc::clone(&ctrl);
                    let ap_clone = ap.clone();
                    conn_btn.connect_clicked(move |_| {
                        if ap_clone.security.eq_ignore_ascii_case("Open") {
                            let ctrl_done = Rc::clone(&ctrl_c);
                            let ssid = ap_clone.ssid.clone();
                            ctrl_c.set_status_info(&format!("Connecting to {ssid}…"));
                            run_async(
                                move || network::connect_wifi(&ssid, None),
                                move |res| match res {
                                    Ok(()) => {
                                        ctrl_done.set_status_ok("Connected to Wi-Fi");
                                        ctrl_done.rebuild_page("network");
                                    }
                                    Err(e) => ctrl_done.set_status_err(&format!("Connect failed: {e}")),
                                },
                            );
                        } else {
                            open_wifi_password_dialog(Rc::clone(&ctrl_c), ap_clone.ssid.clone());
                        }
                    });
                    right.append(&conn_btn);
                }

                rows.push(setting_row(
                    &ap.ssid,
                    &format!(
                        "{} • {}",
                        if ap.in_use { "Connected" } else { "Available" },
                        ap.security
                    ),
                    &right,
                ));
            }
            append_rows(&wifi_card, rows);
        }
        root.append(&wifi_card);
    }

    // ── Active IPv4 / DNS Details ────────────────────────────────────────────
    if !data.details.is_empty() {
        root.append(&section_heading("Active Interfaces"));
        let details_card = boxed_card();
        let mut rows = Vec::new();
        for d in data.details {
            let dns_str = if d.dns_servers.is_empty() {
                "Auto".to_string()
            } else {
                d.dns_servers.join(", ")
            };
            rows.push(info_row(
                &format!("{}  ({})", d.device, d.ip4_address),
                &format!(
                    "Gateway: {} • DNS: {}",
                    if d.gateway.is_empty() { "—" } else { &d.gateway },
                    dns_str
                ),
                "Active",
            ));
        }
        append_rows(&details_card, rows);
        root.append(&details_card);
    }

    // ── Saved Connections (hide loopback / internal profiles) ────────────────
    let saved: Vec<SavedConnection> = data
        .saved
        .into_iter()
        .filter(|c| c.conn_type != "loopback" && c.name != "lo")
        .collect();
    if !saved.is_empty() {
        root.append(&section_heading("Saved Connections"));
        let saved_card = boxed_card();
        let mut rows = Vec::new();
        for conn in saved.into_iter().take(12) {
            let btn = Button::with_label(if conn.active { "Disconnect" } else { "Connect" });
            btn.add_css_class("settings-secondary-btn");
            let ctrl_c = Rc::clone(&ctrl);
            let name = conn.name.clone();
            let target_up = !conn.active;
            btn.connect_clicked(move |_| {
                let ctrl_done = Rc::clone(&ctrl_c);
                let n = name.clone();
                run_async(
                    move || network::set_connection_active(&n, target_up),
                    move |res| match res {
                        Ok(()) => {
                            ctrl_done.set_status_ok(if target_up {
                                "Connected"
                            } else {
                                "Disconnected"
                            });
                            ctrl_done.rebuild_page("network");
                        }
                        Err(e) => ctrl_done.set_status_err(&format!("Action failed: {e}")),
                    },
                );
            });
            rows.push(setting_row(
                &conn.name,
                &format!(
                    "{} • {}",
                    conn.conn_type,
                    if conn.active { &conn.device } else { "Inactive" }
                ),
                &btn,
            ));
        }
        append_rows(&saved_card, rows);
        root.append(&saved_card);
    }
}

fn open_wifi_password_dialog(ctrl: Rc<SettingsController>, ssid: String) {
    let dialog = Window::builder()
        .transient_for(&ctrl.window)
        .modal(true)
        .resizable(false)
        .title(&format!("Connect to {ssid}"))
        .default_width(370)
        .build();
    dialog.add_css_class("neon-settings-window");

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(18);
    vbox.set_margin_bottom(18);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let msg = Label::new(Some(&format!(
        "Enter the Wi-Fi password for \"{ssid}\"."
    )));
    msg.add_css_class("settings-row-desc");
    msg.set_halign(Align::Start);
    msg.set_wrap(true);

    let pw_entry = PasswordEntry::new();
    pw_entry.set_show_peek_icon(true);

    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_halign(Align::End);
    let cancel_btn = Button::with_label("Cancel");
    cancel_btn.add_css_class("settings-secondary-btn");
    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }

    let connect_btn = Button::with_label("Connect");
    connect_btn.add_css_class("settings-action-btn");
    {
        let dlg = dialog.clone();
        let ctrl_c = Rc::clone(&ctrl);
        let pw_c = pw_entry.clone();
        connect_btn.connect_clicked(move |_| {
            let pw = pw_c.text().to_string();
            dlg.close();
            ctrl_c.set_status_info(&format!("Connecting to {ssid}…"));
            let ctrl_done = Rc::clone(&ctrl_c);
            let ssid_run = ssid.clone();
            run_async(
                move || {
                    network::connect_wifi(&ssid_run, if pw.is_empty() { None } else { Some(&pw) })
                },
                move |res| match res {
                    Ok(()) => {
                        ctrl_done.set_status_ok("Connected to Wi-Fi");
                        ctrl_done.rebuild_page("network");
                    }
                    Err(e) => ctrl_done.set_status_err(&format!("Connection failed: {e}")),
                },
            );
        });
    }

    actions.append(&cancel_btn);
    actions.append(&connect_btn);
    vbox.append(&msg);
    vbox.append(&pw_entry);
    vbox.append(&actions);
    dialog.set_child(Some(&vbox));
    dialog.present();
}
