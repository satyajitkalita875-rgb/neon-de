//! 5.8 Network settings: live NetworkManager D-Bus status, async Wi-Fi scanning,
//! secure password dialog (passwords NEVER stored in plain-text config), saved profiles,
//! Ethernet/VPN overview, IPv4/Gateway/DNS details, and Proxy/Hotspot status.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, Label, Orientation, PasswordEntry, ScrolledWindow, Spinner,
    Switch, Window, glib, prelude::*,
};
use neon_system::network::{
    self, NetworkDetails, NetworkStatus, SavedConnection, WifiAccessPoint,
};

use crate::{
    metadata::CapabilityStatus,
    state::SettingsController,
    ui::{
        append_rows, boxed_card, capability_row, info_row, page_shell, run_async, section_heading,
        setting_row, status_badge,
    },
};

struct NetworkSnapshotData {
    status: NetworkStatus,
    aps: Vec<WifiAccessPoint>,
    saved: Vec<SavedConnection>,
    details: Vec<NetworkDetails>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Network",
        "Manage Wi-Fi networks, Ethernet and VPN profiles, IP/DNS details, and NetworkManager settings.",
        None,
        &ctrl,
    );

    let dynamic_root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&dynamic_root);

    let load_state = {
        let ctrl_c = Rc::clone(&ctrl);
        let root = dynamic_root.clone();
        Rc::new(move |rescan: bool| {
            while let Some(child) = root.first_child() {
                root.remove(&child);
            }
            let loading_card = boxed_card();
            let spin = Spinner::new();
            spin.start();
            loading_card.append(&setting_row(
                if rescan {
                    "Scanning Wi-Fi Networks…"
                } else {
                    "Querying NetworkManager…"
                },
                "Fetching live interfaces, access points, and IPv4 details off the UI thread.",
                &spin,
            ));
            root.append(&loading_card);

            let ctrl_done = Rc::clone(&ctrl_c);
            let root_done = root.clone();
            run_async(
                move || NetworkSnapshotData {
                    status: network::query_network_status(),
                    aps: network::list_wifi_networks(rescan),
                    saved: network::list_saved_connections(),
                    details: network::query_network_details(),
                },
                move |data| {
                    render_network_data(&root_done, Rc::clone(&ctrl_done), data);
                },
            );
        })
    };

    load_state(false);
    scroller
}

fn render_network_data(root: &GtkBox, ctrl: Rc<SettingsController>, data: NetworkSnapshotData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Radio & Status ───────────────────────────────────────────────────────
    root.append(&section_heading("Wi-Fi & Connectivity"));
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
                            "Enabled Wi-Fi radio"
                        } else {
                            "Disabled Wi-Fi radio"
                        });
                        ctrl_res.rebuild_page("network");
                    }
                    Err(e) => ctrl_res.set_status_err(&format!("Wi-Fi toggle failed: {e}")),
                },
            );
            glib::Propagation::Proceed
        });
    }

    let scan_btn = Button::with_label("Scan Wi-Fi");
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
        status_badge("NetworkManager Offline", "unavailable")
    } else if data.status.connectivity.is_connected() {
        status_badge(&data.status.panel_label(), "ok")
    } else {
        status_badge("Disconnected", "")
    };

    append_rows(
        &radio_card,
        vec![
            setting_row(
                "NetworkManager Service",
                "org.freedesktop.NetworkManager system D-Bus daemon.",
                &status_badge_w,
            ),
            setting_row(
                "Wi-Fi Radio",
                "Enable or disable wireless networking hardware.",
                &wifi_sw,
            ),
            setting_row(
                "Available Access Points",
                "Trigger an active wireless scan for nearby SSIDs.",
                &scan_btn,
            ),
        ],
    );
    root.append(&radio_card);

    // ── Available Wi-Fi Networks ─────────────────────────────────────────────
    if data.status.wifi_enabled {
        root.append(&section_heading("Available Wi-Fi Networks"));
        let wifi_card = boxed_card();
        if data.aps.is_empty() {
            wifi_card.append(&info_row(
                "No Wi-Fi Networks Visible",
                "Click Scan Wi-Fi above to refresh nearby access points.",
                "0 found",
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
                                    ctrl_done.set_status_ok("Disconnected Wi-Fi network");
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
                        "{} • Security: {}",
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

    // ── Active IPv4 / DNS / MAC Details ──────────────────────────────────────
    root.append(&section_heading("Active Interfaces, IP & DNS"));
    let details_card = boxed_card();
    if data.details.is_empty() {
        details_card.append(&info_row(
            "No Active IPv4 Leases",
            "Connect to Ethernet or Wi-Fi to inspect IP address, gateway, and DNS.",
            "Offline",
        ));
    } else {
        let mut rows = Vec::new();
        for d in data.details {
            let dns_str = if d.dns_servers.is_empty() {
                "Auto / DHCP".to_string()
            } else {
                d.dns_servers.join(", ")
            };
            rows.push(info_row(
                &format!("{}  ({})", d.device, d.ip4_address),
                &format!(
                    "Gateway: {} • DNS: {} • MAC: {}",
                    if d.gateway.is_empty() { "—" } else { &d.gateway },
                    dns_str,
                    if d.mac_address.is_empty() { "—" } else { &d.mac_address }
                ),
                "Active",
            ));
        }
        append_rows(&details_card, rows);
    }
    root.append(&details_card);

    // ── Saved Connections (Wi-Fi, Ethernet, VPN) ─────────────────────────────
    root.append(&section_heading("Saved Connections (Ethernet, Wi-Fi & VPN)"));
    let saved_card = boxed_card();
    if data.saved.is_empty() {
        saved_card.append(&info_row(
            "No Saved Network Profiles",
            "NetworkManager connection profiles will appear here.",
            "—",
        ));
    } else {
        let mut rows = Vec::new();
        for conn in data.saved.into_iter().take(12) {
            let btn = Button::with_label(if conn.active { "Disconnect" } else { "Connect" });
            btn.add_css_class(if conn.active {
                "settings-secondary-btn"
            } else {
                "settings-action-btn"
            });
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
                                "Activated connection"
                            } else {
                                "Deactivated connection"
                            });
                            ctrl_done.rebuild_page("network");
                        }
                        Err(e) => ctrl_done.set_status_err(&format!("Connection action failed: {e}")),
                    },
                );
            });
            rows.push(setting_row(
                &conn.name,
                &format!(
                    "Type: {} • Device: {}",
                    conn.conn_type,
                    if conn.active { &conn.device } else { "Inactive" }
                ),
                &btn,
            ));
        }
        append_rows(&saved_card, rows);
    }
    root.append(&saved_card);

    // ── Proxy & Hotspot Capabilities ─────────────────────────────────────────
    root.append(&section_heading("Advanced Networking"));
    let adv_card = boxed_card();
    append_rows(
        &adv_card,
        vec![
            capability_row(
                "System HTTP/SOCKS Proxy",
                "Uses standard environment/gsettings proxy variables; manual GUI editor planned.",
                CapabilityStatus::PartiallyImplemented,
            ),
            capability_row(
                "Wi-Fi Hotspot Sharing",
                "Requires AP-capable wireless hardware and NetworkManager shared mode.",
                CapabilityStatus::PartiallyImplemented,
            ),
        ],
    );
    root.append(&adv_card);
}

fn open_wifi_password_dialog(ctrl: Rc<SettingsController>, ssid: String) {
    let dialog = Window::builder()
        .transient_for(&ctrl.window)
        .modal(true)
        .resizable(false)
        .title(&format!("Connect to {ssid}"))
        .default_width(390)
        .build();
    dialog.add_css_class("neon-settings-window");

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(18);
    vbox.set_margin_bottom(18);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let msg = Label::new(Some(&format!(
        "Enter the Wi-Fi password for \"{ssid}\". Passwords are passed directly to NetworkManager and never saved in NEON DE config files."
    )));
    msg.add_css_class("settings-row-desc");
    msg.set_halign(Align::Start);
    msg.set_wrap(true);

    let pw_entry = PasswordEntry::new();
    pw_entry.set_show_peek_icon(true);

    let actions = GtkBox::new(Orientation::Horizontal, 10);
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
                move || network::connect_wifi(&ssid_run, if pw.is_empty() { None } else { Some(&pw) }),
                move |res| match res {
                    Ok(()) => {
                        ctrl_done.set_status_ok("Connected to Wi-Fi network");
                        ctrl_done.rebuild_page("network");
                    }
                    Err(e) => ctrl_done.set_status_err(&format!("Wi-Fi connection failed: {e}")),
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
