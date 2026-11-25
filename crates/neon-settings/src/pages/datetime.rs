//! Date & Time settings: systemd-timedated NTP synchronization, searchable IANA
//! timezone selector, and active system locale info.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, DropDown, Label, Orientation, ScrolledWindow, SearchEntry,
    Spinner, StringList, Switch, glib, prelude::*,
};
use neon_system::datetime::{self, DateTimeLocaleStatus};

use crate::{
    state::{SettingsController, build_appearance_request},
    ui::{
        append_rows, boxed_card, info_row, page_shell, run_async, section_heading, setting_row,
        status_badge,
    },
};

struct DateTimeData {
    status: DateTimeLocaleStatus,
    timezones: Vec<String>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    let root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row("Loading Date & Time…", "", &spin));
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
    root.append(&section_heading("Clock & Sync"));
    let time_card = boxed_card();

    let use_12h_initial = !ctrl.config.borrow().panel.clock_24h;
    let time_display = format_current_time_preview(use_12h_initial, &data.status.local_time_display);
    let current_time_val = Label::new(Some(&time_display));
    current_time_val.add_css_class("settings-row-desc");
    current_time_val.set_valign(Align::Center);
    current_time_val.set_selectable(true);

    let clock_12h_sw = Switch::new();
    clock_12h_sw.set_active(use_12h_initial);
    clock_12h_sw.set_valign(Align::Center);
    {
        let ctrl_c = Rc::clone(&ctrl);
        let time_lbl = current_time_val.clone();
        let fallback_str = data.status.local_time_display.clone();
        clock_12h_sw.connect_state_set(move |_, use_12h| {
            ctrl_c.config.borrow_mut().panel.clock_24h = !use_12h;
            let req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(
                Some(req),
                if use_12h {
                    "Clock set to 12-hour (AM/PM)"
                } else {
                    "Clock set to 24-hour format"
                },
            );
            time_lbl.set_text(&format_current_time_preview(use_12h, &fallback_str));
            glib::Propagation::Proceed
        });
    }

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
                            "Automatic time enabled"
                        } else {
                            "Automatic time disabled"
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

    append_rows(
        &time_card,
        vec![
            setting_row("Current Time", &data.status.timezone, &current_time_val),
            setting_row(
                "12-Hour Clock (AM/PM)",
                "Display time with AM/PM in the top bar and menus.",
                &clock_12h_sw,
            ),
            setting_row(
                "Set Time Automatically (NTP)",
                "Synchronize clock via systemd-timedated.",
                &ntp_sw,
            ),
            setting_row("Sync Status", "", &sync_badge),
        ],
    );
    root.append(&time_card);

    // ── Timezone Selector ────────────────────────────────────────────────────
    root.append(&section_heading("Timezone"));
    let tz_card = boxed_card();

    let tz_filter = SearchEntry::new();
    tz_filter.add_css_class("settings-search");
    tz_filter.set_placeholder_text(Some("Search timezone (e.g. Asia/Kolkata, Europe/London)…"));
    tz_filter.set_margin_top(10);
    tz_filter.set_margin_bottom(4);
    tz_filter.set_margin_start(14);
    tz_filter.set_margin_end(14);
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

            let apply_btn = Button::with_label("Apply");
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
                                    ctrl_done.set_status_ok(&format!("Timezone: {tz_label}"));
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
            holder.append(&setting_row("Timezone", &format!("Current: {cur_tz}"), &right));
        })
    };

    rebuild_tz_row("");
    {
        let r = Rc::clone(&rebuild_tz_row);
        tz_filter.connect_search_changed(move |e| r(&e.text()));
    }
    root.append(&tz_card);

    // ── Region / Locale ──────────────────────────────────────────────────────
    root.append(&section_heading("Region"));
    let lang_card = boxed_card();
    lang_card.append(&info_row("System Locale", "", &data.status.system_locale));
    root.append(&lang_card);
}

fn format_current_time_preview(use_12h: bool, fallback: &str) -> String {
    let fmt = if use_12h {
        "%a, %b %-e, %Y  %-I:%M %p"
    } else {
        "%a, %b %-e, %Y  %H:%M"
    };
    glib::DateTime::now_local()
        .ok()
        .and_then(|dt| dt.format(fmt).ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| fallback.to_string())
}
