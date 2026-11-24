//! 5.13 Privacy & Security: Lock Screen (ext_session_lock_v1), auto-lock timeout,
//! IPC SO_PEERCRED / PolicyKit1 / XDG Portals status, permission indicators,
//! and Recent File History management (`~/.local/share/recently-used.xbel`).

use std::{path::PathBuf, rc::Rc};

use gtk4::{Button, ScrolledWindow, prelude::*};
use neon_ipc::IpcRequest;
use neon_system::{polkit::query_polkit_status, portals::query_portal_status};

use crate::{
    metadata::CapabilityStatus,
    state::SettingsController,
    ui::{
        append_rows, boxed_card, capability_row, confirm_action_dialog, dropdown_row, page_shell,
        section_heading, setting_row, status_badge, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Privacy & Security",
        "Control screen lock, PolicyKit privilege boundaries, XDG Desktop Portals, and recent file history.",
        Some("privacy"),
        &ctrl,
    );

    let priv_cfg = ctrl.config.borrow().privacy_accessibility.clone();
    let polkit = query_polkit_status();
    let portals = query_portal_status();

    // ── Screen Lock & Authentication ─────────────────────────────────────────
    page.append(&section_heading("Screen Lock & Session Authentication"));
    let lock_card = boxed_card();

    let lock_now_btn = Button::with_label("Lock Screen Now");
    lock_now_btn.add_css_class("settings-action-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        lock_now_btn.connect_clicked(move |_| match ctrl_c.send_ipc(IpcRequest::LockSession) {
            Ok(_) => ctrl_c.set_status_ok("Activated compositor screen lock (ext_session_lock_v1)"),
            Err(e) => ctrl_c.set_status_err(&format!("Lock failed (compositor offline): {e}")),
        });
    }

    let lock_opts: [u32; 6] = [0, 60, 180, 300, 600, 1800];
    let lock_idx = lock_opts
        .iter()
        .position(|v| *v == priv_cfg.auto_lock_seconds)
        .unwrap_or(3) as u32;

    append_rows(
        &lock_card,
        vec![
            setting_row(
                "Lock Session Immediately",
                "Triggers native Wayland ext_session_lock_v1 backed by PAM authentication.",
                &lock_now_btn,
            ),
            dropdown_row(
                "Automatic Screen Lock Timeout",
                "Require password unlock after screen idle duration.",
                &[
                    "Disabled",
                    "1 minute",
                    "3 minutes",
                    "5 minutes",
                    "10 minutes",
                    "30 minutes",
                ],
                lock_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let secs = lock_opts.get(idx as usize).copied().unwrap_or(300);
                        ctrl_c
                            .config
                            .borrow_mut()
                            .privacy_accessibility
                            .auto_lock_seconds = secs;
                        ctrl_c.persist_and_sync(None, "Updated automatic screen lock timeout");
                    }
                },
            ),
            setting_row(
                "Password Security Model",
                "Passwords are verified exclusively through PAM/polkit — never stored or read by Settings.",
                &status_badge("PAM / Polkit", "ok"),
            ),
        ],
    );
    page.append(&lock_card);

    // ── Security Architecture & Portals ──────────────────────────────────────
    page.append(&section_heading("System Isolation & App Permissions"));
    let sec_card = boxed_card();

    append_rows(
        &sec_card,
        vec![
            setting_row(
                "Compositor IPC Authentication",
                "Unix socket directory 0700, socket 0600, strict same-UID SO_PEERCRED verification.",
                &status_badge("Verified", "ok"),
            ),
            setting_row(
                "PolicyKit1 Authorization Daemon",
                "Handles privileged elevation prompts without running Settings as root.",
                &status_badge(
                    if polkit.daemon_available {
                        "Active"
                    } else {
                        "Offline"
                    },
                    if polkit.daemon_available {
                        "ok"
                    } else {
                        "unavailable"
                    },
                ),
            ),
            setting_row(
                "XDG Desktop Portals (Screen Sharing, File Picker, Camera, Location)",
                &format!(
                    "Desktop Portal: {} • Documents Portal: {} • Backends: {}",
                    if portals.desktop_portal_running {
                        "Running"
                    } else {
                        "Standby"
                    },
                    if portals.documents_portal_running {
                        "Running"
                    } else {
                        "Standby"
                    },
                    if portals.installed_backends.is_empty() {
                        "Default".to_string()
                    } else {
                        portals.installed_backends.join(", ")
                    },
                ),
                &status_badge(
                    if portals.desktop_portal_running {
                        "Active"
                    } else {
                        "Standby"
                    },
                    if portals.desktop_portal_running {
                        "ok"
                    } else {
                        ""
                    },
                ),
            ),
            capability_row(
                "Per-App Camera, Microphone & Location Grants",
                "Sandboxed Flatpak/Snap permissions are mediated by xdg-desktop-portal PermissionStore.",
                CapabilityStatus::PartiallyImplemented,
            ),
            capability_row(
                "Remote Desktop & Screen Sharing Server",
                "Requires RDP/VNC portal daemon (e.g. gnome-remote-desktop or wayvnc).",
                CapabilityStatus::Unsupported(
                    "No remote-access server is bundled with NEON DE by default.",
                ),
            ),
        ],
    );
    page.append(&sec_card);

    // ── History & Data Controls ──────────────────────────────────────────────
    page.append(&section_heading("Recent Files & Usage History"));
    let hist_card = boxed_card();

    let clear_recent_btn = Button::with_label("Clear Recent Files");
    clear_recent_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        clear_recent_btn.connect_clicked(move |_| {
            let ctrl_inner = Rc::clone(&ctrl_c);
            confirm_action_dialog(
                &ctrl_c.window,
                "Clear Recent File History?",
                "This clears ~/.local/share/recently-used.xbel so recently opened files no longer appear in file pickers.",
                "Clear History",
                true,
                move || {
                    if let Some(home) = std::env::var_os("HOME") {
                        let path = PathBuf::from(home).join(".local/share/recently-used.xbel");
                        let empty_xbel = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\"/>\n";
                        match std::fs::write(&path, empty_xbel) {
                            Ok(()) => ctrl_inner.set_status_ok("Cleared recent files history"),
                            Err(e) => ctrl_inner.set_status_err(&format!("Clear failed: {e}")),
                        }
                    }
                },
            );
        });
    }

    append_rows(
        &hist_card,
        vec![
            toggle_row(
                "Remember Recent Files",
                "Track recently opened files for quick access in GTK file dialogs.",
                priv_cfg.recent_files_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c
                            .config
                            .borrow_mut()
                            .privacy_accessibility
                            .recent_files_enabled = v;
                        ctrl_c.persist_and_sync(None, "Updated recent files tracking");
                    }
                },
            ),
            toggle_row(
                "Remember Application Usage History",
                "Track frequently launched applications for launcher ordering.",
                priv_cfg.usage_history_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c
                            .config
                            .borrow_mut()
                            .privacy_accessibility
                            .usage_history_enabled = v;
                        ctrl_c.persist_and_sync(None, "Updated usage history preference");
                    }
                },
            ),
            setting_row(
                "Clear Recent File History",
                "Empties ~/.local/share/recently-used.xbel immediately.",
                &clear_recent_btn,
            ),
        ],
    );
    page.append(&hist_card);

    scroller
}
