//! Applications settings: Default Apps (XDG MIME) and searchable installed applications
//! browser with real uninstallation (APT / Flatpak / Snap / user launchers).

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, DropDown, Orientation, ScrolledWindow, SearchEntry, Spinner,
    StringList, prelude::*,
};
use neon_system::apps::{self, InstalledApp};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, confirm_action_dialog, info_row, page_shell, run_async,
        section_heading, setting_row, status_badge,
    },
};

struct AppsData {
    installed: Vec<InstalledApp>,
    default_browser: Option<String>,
    default_mail: Option<String>,
    default_files: Option<String>,
    default_media: Option<String>,
    default_terminal: Option<String>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    let root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row("Loading Applications…", "", &spin));
    root.append(&load_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(
        || AppsData {
            installed: apps::discover_installed_apps(),
            default_browser: apps::query_default_mime_app("x-scheme-handler/https"),
            default_mail: apps::query_default_mime_app("x-scheme-handler/mailto"),
            default_files: apps::query_default_mime_app("inode/directory"),
            default_media: apps::query_default_mime_app("video/mp4"),
            default_terminal: apps::query_default_mime_app("x-scheme-handler/terminal"),
        },
        move |data| render_apps(&root_done, ctrl_done, data),
    );

    scroller
}

fn render_apps(root: &GtkBox, ctrl: Rc<SettingsController>, data: AppsData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Default Applications ─────────────────────────────────────────────────
    root.append(&section_heading("Default Applications"));
    let def_card = boxed_card();

    let mut def_rows = Vec::new();
    let mime_roles: &[(&str, &[&str], &Option<String>, &[&str])] = &[
        (
            "Web Browser",
            &["x-scheme-handler/http", "x-scheme-handler/https", "text/html"],
            &data.default_browser,
            &["WebBrowser", "Network"],
        ),
        (
            "Email",
            &["x-scheme-handler/mailto"],
            &data.default_mail,
            &["Email", "Office"],
        ),
        (
            "File Manager",
            &["inode/directory"],
            &data.default_files,
            &["FileManager", "System"],
        ),
        (
            "Terminal",
            &["x-scheme-handler/terminal"],
            &data.default_terminal,
            &["TerminalEmulator"],
        ),
        (
            "Media Player",
            &["video/mp4", "video/x-matroska", "audio/mpeg"],
            &data.default_media,
            &["AudioVideo", "Player", "Video"],
        ),
    ];

    for (title, mimes, current_id, cat_hints) in mime_roles {
        let mut candidates: Vec<&InstalledApp> = data
            .installed
            .iter()
            .filter(|app| {
                current_id.as_deref() == Some(app.desktop_id.as_str())
                    || app.mime_types.iter().any(|m| mimes.contains(&m.as_str()))
                    || app
                        .categories
                        .iter()
                        .any(|c| cat_hints.iter().any(|h| c.eq_ignore_ascii_case(h)))
            })
            .collect();
        if candidates.is_empty() {
            candidates = data.installed.iter().take(15).collect();
        }
        if candidates.is_empty() {
            def_rows.push(setting_row(
                title,
                "",
                &status_badge("None installed", "unavailable"),
            ));
            continue;
        }

        let labels: Vec<&str> = candidates.iter().map(|a| a.name.as_str()).collect();
        let ids: Vec<String> = candidates.iter().map(|a| a.desktop_id.clone()).collect();
        let selected_idx = current_id
            .as_deref()
            .and_then(|cur| ids.iter().position(|id| id == cur))
            .unwrap_or(0) as u32;

        let dd = DropDown::new(Some(StringList::new(&labels)), gtk4::Expression::NONE);
        dd.set_valign(Align::Center);
        dd.set_selected(selected_idx);

        let ctrl_c = Rc::clone(&ctrl);
        let mimes_owned: Vec<&'static str> = mimes.to_vec();
        let role_name = (*title).to_string();
        dd.connect_selected_notify(move |d| {
            if let Some(desktop_id) = ids.get(d.selected() as usize).cloned() {
                let ctrl_done = Rc::clone(&ctrl_c);
                let role = role_name.clone();
                let m = mimes_owned.clone();
                run_async(
                    move || apps::set_default_mime_app(&desktop_id, &m),
                    move |res| match res {
                        Ok(()) => ctrl_done.set_status_ok(&format!("Updated default {role}")),
                        Err(e) => ctrl_done.set_status_err(&format!("Failed: {e}")),
                    },
                );
            }
        });
        def_rows.push(setting_row(title, "", &dd));
    }

    append_rows(&def_card, def_rows);
    root.append(&def_card);

    // ── Installed Applications ───────────────────────────────────────────────
    root.append(&section_heading(&format!(
        "Installed Applications ({})",
        data.installed.len()
    )));

    let search = SearchEntry::new();
    search.add_css_class("settings-search");
    search.set_placeholder_text(Some("Filter applications…"));
    root.append(&search);

    let apps_box = GtkBox::new(Orientation::Vertical, 2);
    root.append(&apps_box);

    let installed_rc = Rc::new(data.installed);
    let render_list = {
        let ctrl_c = Rc::clone(&ctrl);
        let box_c = apps_box.clone();
        let all_apps = Rc::clone(&installed_rc);
        Rc::new(move |query: &str| {
            while let Some(child) = box_c.first_child() {
                box_c.remove(&child);
            }
            let q = query.trim().to_ascii_lowercase();
            let card = boxed_card();
            let filtered: Vec<&InstalledApp> = all_apps
                .iter()
                .filter(|a| {
                    q.is_empty()
                        || a.name.to_ascii_lowercase().contains(&q)
                        || a.desktop_id.to_ascii_lowercase().contains(&q)
                        || a.comment
                            .as_deref()
                            .is_some_and(|c| c.to_ascii_lowercase().contains(&q))
                })
                .take(100)
                .collect();

            if filtered.is_empty() {
                card.append(&info_row("No Matching Applications", "", "0"));
            } else {
                let mut rows = Vec::new();
                for app in filtered {
                    let source = apps::detect_install_source(app);
                    let source_label = source.label();
                    let uninstall_btn = Button::with_label("Uninstall…");
                    uninstall_btn.add_css_class("settings-destructive-btn");
                    let ctrl_a = Rc::clone(&ctrl_c);
                    let app_clone = app.clone();
                    uninstall_btn.connect_clicked(move |btn| {
                        let ctrl_inner = Rc::clone(&ctrl_a);
                        let app_run = app_clone.clone();
                        let btn_c = btn.clone();
                        let title = format!("Uninstall {}?", app_clone.name);
                        let body = format!(
                            "Remove {} ({source_label}) from this computer? Administrator authentication may be requested for system packages.",
                            app_clone.name
                        );
                        confirm_action_dialog(
                            &ctrl_a.window,
                            &title,
                            &body,
                            "Uninstall",
                            true,
                            move || {
                                btn_c.set_sensitive(false);
                                btn_c.set_label("Uninstalling…");
                                ctrl_inner.set_status_info(&format!(
                                    "Uninstalling {}…",
                                    app_run.name
                                ));
                                let ctrl_done = Rc::clone(&ctrl_inner);
                                let btn_done = btn_c.clone();
                                run_async(
                                    move || apps::uninstall_app(&app_run),
                                    move |res| match res {
                                        Ok(msg) => {
                                            ctrl_done.set_status_ok(&msg);
                                            ctrl_done.rebuild_page("applications");
                                        }
                                        Err(e) => {
                                            btn_done.set_sensitive(true);
                                            btn_done.set_label("Uninstall…");
                                            ctrl_done.set_status_err(&format!(
                                                "Uninstall failed: {e}"
                                            ));
                                        }
                                    },
                                );
                            },
                        );
                    });
                    let sub = match app.comment.as_deref().filter(|s| !s.trim().is_empty()) {
                        Some(comment) => format!("{source_label} • {comment}"),
                        None => format!("{source_label} • {}", app.desktop_id),
                    };
                    rows.push(setting_row(&app.name, &sub, &uninstall_btn));
                }
                append_rows(&card, rows);
            }
            box_c.append(&card);
        })
    };

    render_list("");
    {
        let r = Rc::clone(&render_list);
        search.connect_search_changed(move |e| r(&e.text()));
    }
}
