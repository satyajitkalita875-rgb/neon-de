//! 5.19 Date, Time & Language: `org.freedesktop.timedate1` NTP sync toggle,
//! searchable IANA timezone selector, 12h/24h & date formatting, and system locale inspection.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, DropDown, Orientation, ScrolledWindow, SearchEntry, Spinner,
    StringList, Switch, glib, prelude::*,
};
use neon_system::datetime::{self, DateTimeLocaleStatus};

use crate::{
    metadata::CapabilityStatus,
    state::SettingsController,
    ui::{
        append_rows, boxed_card, capability_row, info_row, page_shell, run_async, section_heading,
        setting_row, status_badge, toggle_row,
    },
};

struct DateTimeData {
    status: DateTimeLocaleStatus,
    timezones: Vec<String>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Date, Time & Language",
        "Manage automatic network time (NTP), IANA timezone, clock formatting, and system locales.",
        None,
        &ctrl,
    );

    let root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row(
        "Querying systemd-timedated & Locale…",
        "Reading active timezone, NTP synchronization, and installed locales.",
        &spin,
    ));
    root.append(&load_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(
        || DateTimeData {
            status: datetime::query_datetime_locale(),
            timezones: datetime::list_timezones(),
        },
        move |data| render_datetime(&root_done, ctrl_done, data),
    );

    scroller
}

fn render_datetime(root: &GtkBox, ctrl: Rc<SettingsController>, data: DateTimeData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Date, Time & NTP ─────────────────────────────────────────────────────
    root.append(&section_heading("Date, Time & Synchronization"));
    let time_card = boxed_card();

    let ntp_sw = Switch::new();
    ntp_sw.set_active(data.status.ntp_enabled);
    ntp_sw.set_sensitive(data.status.timedated_available);
    ntp_sw.set_valign(Align::Center);
    {
        let ctrl_c = Rc::clone(&ctrl);
        ntp_sw.connect_state_set(move |_, active| {
            let ctrl_done = Rc::clone(&ctrl_c);
            run_async(
                move || datetime::set_ntp_enabled(active),
                move |res| match res {
                    Ok(()) => {
                        ctrl_done.set_status_ok(if active {
                            "Enabled automatic network time (NTP)"
                        } else {
                            "Disabled automatic network time (NTP)"
                        });
                        ctrl_done.rebuild_page("datetime");
                    }
                    Err(e) => ctrl_done.set_status_err(&e),
                },
            );
            glib::Propagation::Proceed
        });
    }

    let sync_badge = if data.status.ntp_synchronized {
        status_badge("Synchronized", "ok")
    } else if data.status.ntp_enabled {
        status_badge("Syncing", "partial")
    } else {
        status_badge("Manual", "")
    };

    let panel_cfg = ctrl.config.borrow().panel.clone();

    append_rows(
        &time_card,
        vec![
            info_row(
                "Current Local Time",
                &format!("Timezone: {}", data.status.timezone),
                &data.status.local_time_display,
            ),
            setting_row(
                "Automatic Date & Time (NTP)",
                "Synchronize system clock via systemd-timedated (polkit-gated).",
                &ntp_sw,
            ),
            setting_row(
                "NTP Synchronization Status",
                "org.freedesktop.timedate1.NTPSynchronized property.",
                &sync_badge,
            ),
            toggle_row(
                "24-Hour Clock",
                "Display 24-hour time format on the NEON DE top panel.",
                panel_cfg.clock_24h,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().panel.clock_24h = v;
                        ctrl_c.persist_and_sync(None, "Updated 24-hour clock format");
                    }
                },
            ),
            toggle_row(
                "Show Date in Panel Clock",
                "Include weekday and calendar date beside the time.",
                panel_cfg.show_date,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().panel.show_date = v;
                        ctrl_c.persist_and_sync(None, "Updated panel date visibility");
                    }
                },
            ),
        ],
    );
    root.append(&time_card);

    // ── Timezone Selector ────────────────────────────────────────────────────
    root.append(&section_heading("Timezone"));
    let tz_card = boxed_card();

    let tz_filter = SearchEntry::new();
    tz_filter.set_placeholder_text(Some("Filter timezones (e.g. Asia/Kolkata, Europe/London)…"));
    tz_filter.set_margin_top(10);
    tz_filter.set_margin_bottom(6);
    tz_filter.set_margin_start(16);
    tz_filter.set_margin_end(16);
    tz_card.append(&tz_filter);

    let tz_picker_holder = GtkBox::new(Orientation::Vertical, 0);
    tz_card.append(&tz_picker_holder);

    let all_tzs = Rc::new(data.timezones);
    let current_tz = data.status.timezone.clone();
    let timedated_ok = data.status.timedated_available;

    let rebuild_tz_row = {
        let ctrl_c = Rc::clone(&ctrl);
        let holder = tz_picker_holder.clone();
        let all_tzs = Rc::clone(&all_tzs);
        let cur_tz = current_tz.clone();
        Rc::new(move |filter: &str| {
            while let Some(child) = holder.first_child() {
                holder.remove(&child);
            }
            let q = filter.trim().to_ascii_lowercase();
            let matches: Vec<String> = all_tzs
                .iter()
                .filter(|z| q.is_empty() || z.to_ascii_lowercase().contains(&q))
                .take(40)
                .cloned()
                .collect();

            if matches.is_empty() {
                holder.append(&info_row("No Matching Timezones", "", "0"));
                return;
            }
            let refs: Vec<&str> = matches.iter().map(String::as_str).collect();
            let init_idx = matches.iter().position(|z| z == &cur_tz).unwrap_or(0) as u32;
            let dd = DropDown::new(Some(StringList::new(&refs)), gtk4::Expression::NONE);
            dd.set_valign(Align::Center);
            dd.set_selected(init_idx);

            let apply_btn = Button::with_label("Apply Timezone");
            apply_btn.add_css_class("settings-action-btn");
            apply_btn.set_sensitive(timedated_ok);
            {
                let ctrl_a = Rc::clone(&ctrl_c);
                let dd_c = dd.clone();
                let list_c = matches.clone();
                apply_btn.connect_clicked(move |_| {
                    if let Some(tz) = list_c.get(dd_c.selected() as usize).cloned() {
                        let ctrl_done = Rc::clone(&ctrl_a);
                        let tz_label = tz.clone();
                        run_async(
                            move || datetime::set_timezone(&tz),
                            move |res| match res {
                                Ok(()) => {
                                    ctrl_done.set_status_ok(&format!("Timezone set to {tz_label}"));
                                    ctrl_done.rebuild_page("datetime");
                                }
                                Err(e) => ctrl_done.set_status_err(&e),
                            },
                        );
                    }
                });
            }

            let right = GtkBox::new(Orientation::Horizontal, 8);
            right.set_valign(Align::Center);
            right.append(&dd);
            right.append(&apply_btn);
            holder.append(&setting_row(
                "System Timezone",
                &format!("Active: {cur_tz}"),
                &right,
            ));
        })
    };

    rebuild_tz_row("");
    {
        let r = Rc::clone(&rebuild_tz_row);
        tz_filter.connect_search_changed(move |e| r(&e.text()));
    }
    root.append(&tz_card);

    // ── Language & Regional Formats ──────────────────────────────────────────
    root.append(&section_heading("Language & Regional Formats"));
    let lang_card = boxed_card();

    let installed_preview = if data.status.available_locales.is_empty() {
        "System default".to_string()
    } else {
        let preview: Vec<&str> = data
            .status
            .available_locales
            .iter()
            .take(6)
            .map(String::as_str)
            .collect();
        format!(
            "{} ({} total)",
            preview.join(", "),
            data.status.available_locales.len()
        )
    };

    append_rows(
        &lang_card,
        vec![
            info_row(
                "Active System Locale",
                "Reported by systemd-localed / LANG environment.",
                &data.status.system_locale,
            ),
            info_row(
                "Installed Locales",
                "Available UTF-8 locales discovered via `locale -a`.",
                &installed_preview,
            ),
            capability_row(
                "System-Wide Locale Change",
                "Changing `/etc/locale.conf` via `localectl set-locale` requires administrator authorization and a fresh session login.",
                CapabilityStatus::RequiresElevation,
            ),
        ],
    );
    root.append(&lang_card);
}
