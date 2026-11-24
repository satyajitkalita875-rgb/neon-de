//! 5.16 Dedicated Keyboard Shortcuts Manager: grouped categories, search filtering,
//! interactive shortcut editing with real conflict detection, custom shortcut creation,
//! and one-click Reset to Defaults.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, DropDown, Entry, Label, Orientation, ScrolledWindow, SearchEntry,
    StringList, Window, prelude::*,
};
use neon_config::{ShortcutBindingConfig, find_shortcut_conflict};

use crate::{
    state::SettingsController,
    ui::{
        append_rows, boxed_card, page_shell, section_heading, setting_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Keyboard Shortcuts",
        "View, search, customize, and add global keyboard bindings with live conflict detection.",
        Some("shortcuts"),
        &ctrl,
    );

    // Search + Add Custom bar
    let top_bar = GtkBox::new(Orientation::Horizontal, 10);
    top_bar.set_margin_bottom(8);
    let filter_entry = SearchEntry::new();
    filter_entry.set_hexpand(true);
    filter_entry.set_placeholder_text(Some("Filter shortcuts by action, category, or keys…"));

    let add_btn = Button::with_label("+ Custom Shortcut");
    add_btn.add_css_class("settings-action-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        add_btn.connect_clicked(move |_| open_custom_shortcut_dialog(Rc::clone(&ctrl_c)));
    }
    top_bar.append(&filter_entry);
    top_bar.append(&add_btn);
    page.append(&top_bar);

    let list_container = GtkBox::new(Orientation::Vertical, 4);
    page.append(&list_container);

    let populate = {
        let ctrl_c = Rc::clone(&ctrl);
        let container = list_container.clone();
        Rc::new(move |query: &str| {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
            let q = query.trim().to_ascii_lowercase();
            let shortcuts = ctrl_c.config.borrow().input.shortcuts.clone();

            let categories = ["System", "Windows", "Workspaces", "Applications", "Media", "Custom"];
            for cat in categories {
                let matching: Vec<&ShortcutBindingConfig> = shortcuts
                    .iter()
                    .filter(|s| {
                        let same_cat = s.category.eq_ignore_ascii_case(cat)
                            || (cat == "Custom" && s.custom);
                        if !same_cat {
                            return false;
                        }
                        if q.is_empty() {
                            return true;
                        }
                        s.label.to_ascii_lowercase().contains(&q)
                            || s.accelerator.to_ascii_lowercase().contains(&q)
                            || s.category.to_ascii_lowercase().contains(&q)
                    })
                    .collect();

                if matching.is_empty() {
                    continue;
                }

                container.append(&section_heading(cat));
                let card = boxed_card();
                let mut rows = Vec::new();
                for sc in matching {
                    let right = GtkBox::new(Orientation::Horizontal, 8);
                    right.set_valign(Align::Center);

                    let pill = Label::new(Some(&sc.accelerator));
                    pill.add_css_class("kbd-pill");

                    let edit_btn = Button::with_label("Edit");
                    edit_btn.add_css_class("settings-secondary-btn");
                    {
                        let ctrl_e = Rc::clone(&ctrl_c);
                        let sc_owned = sc.clone();
                        edit_btn.connect_clicked(move |_| {
                            open_edit_shortcut_dialog(Rc::clone(&ctrl_e), sc_owned.clone());
                        });
                    }

                    right.append(&pill);
                    right.append(&edit_btn);

                    if sc.custom {
                        let del_btn = Button::with_label("Delete");
                        del_btn.add_css_class("settings-secondary-btn");
                        let ctrl_d = Rc::clone(&ctrl_c);
                        let sc_id = sc.id.clone();
                        del_btn.connect_clicked(move |_| {
                            ctrl_d
                                .config
                                .borrow_mut()
                                .input
                                .shortcuts
                                .retain(|s| s.id != sc_id);
                            ctrl_d.persist_and_sync(None, "Deleted custom shortcut");
                            ctrl_d.rebuild_page("shortcuts");
                        });
                        right.append(&del_btn);
                    }

                    let sub = sc
                        .command
                        .as_deref()
                        .map(|cmd| format!("Command: {cmd}"))
                        .unwrap_or_else(|| format!("Action ID: {}", sc.id));
                    rows.push(setting_row(&sc.label, &sub, &right));
                }
                append_rows(&card, rows);
                container.append(&card);
            }
        })
    };

    populate("");
    {
        let pop = Rc::clone(&populate);
        filter_entry.connect_search_changed(move |e| pop(&e.text()));
    }

    scroller
}

fn open_edit_shortcut_dialog(ctrl: Rc<SettingsController>, binding: ShortcutBindingConfig) {
    let dialog = Window::builder()
        .transient_for(&ctrl.window)
        .modal(true)
        .resizable(false)
        .title(&format!("Edit Shortcut: {}", binding.label))
        .default_width(400)
        .build();
    dialog.add_css_class("neon-settings-window");

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_top(18);
    vbox.set_margin_bottom(18);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let info = Label::new(Some(
        "Enter key combination (e.g. Super+Shift+T, Ctrl+Alt+L, Alt+F4):",
    ));
    info.add_css_class("settings-row-desc");
    info.set_halign(Align::Start);
    info.set_wrap(true);

    let accel_entry = Entry::new();
    accel_entry.set_text(&binding.accelerator);

    let conflict_lbl = Label::new(None);
    conflict_lbl.add_css_class("settings-row-desc");
    conflict_lbl.set_halign(Align::Start);
    conflict_lbl.set_wrap(true);

    {
        let ctrl_c = Rc::clone(&ctrl);
        let id_c = binding.id.clone();
        let cl = conflict_lbl.clone();
        accel_entry.connect_changed(move |e| {
            let candidate = e.text();
            let cfg = ctrl_c.config.borrow();
            if let Some(other) = find_shortcut_conflict(&cfg.input.shortcuts, &id_c, &candidate) {
                cl.set_text(&format!(
                    "Conflict detected: already assigned to \"{}\".",
                    other.label
                ));
            } else {
                cl.set_text("No shortcut conflicts detected.");
            }
        });
    }

    let actions = GtkBox::new(Orientation::Horizontal, 10);
    actions.set_halign(Align::End);
    let cancel_btn = Button::with_label("Cancel");
    cancel_btn.add_css_class("settings-secondary-btn");
    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }

    let save_btn = Button::with_label("Save Binding");
    save_btn.add_css_class("settings-action-btn");
    {
        let dlg = dialog.clone();
        let ctrl_c = Rc::clone(&ctrl);
        let entry_c = accel_entry.clone();
        let id_c = binding.id.clone();
        let label_c = binding.label.clone();
        save_btn.connect_clicked(move |_| {
            let new_accel = entry_c.text().trim().to_string();
            if new_accel.is_empty() {
                ctrl_c.set_status_err("Shortcut key combination cannot be empty");
                return;
            }
            {
                let cfg = ctrl_c.config.borrow();
                if let Some(conflict) =
                    find_shortcut_conflict(&cfg.input.shortcuts, &id_c, &new_accel)
                {
                    ctrl_c.set_status_err(&format!(
                        "Conflict with \"{}\" ({new_accel}) — choose a unique binding",
                        conflict.label
                    ));
                    return;
                }
            }
            if let Some(item) = ctrl_c
                .config
                .borrow_mut()
                .input
                .shortcuts
                .iter_mut()
                .find(|s| s.id == id_c)
            {
                item.accelerator = new_accel.clone();
            }
            ctrl_c.persist_and_sync(None, &format!("Updated \"{label_c}\" → {new_accel}"));
            ctrl_c.rebuild_page("shortcuts");
            dlg.close();
        });
    }

    actions.append(&cancel_btn);
    actions.append(&save_btn);
    vbox.append(&info);
    vbox.append(&accel_entry);
    vbox.append(&conflict_lbl);
    vbox.append(&actions);
    dialog.set_child(Some(&vbox));
    dialog.present();
}

fn open_custom_shortcut_dialog(ctrl: Rc<SettingsController>) {
    let dialog = Window::builder()
        .transient_for(&ctrl.window)
        .modal(true)
        .resizable(false)
        .title("Add Custom Shortcut")
        .default_width(420)
        .build();
    dialog.add_css_class("neon-settings-window");

    let vbox = GtkBox::new(Orientation::Vertical, 10);
    vbox.set_margin_top(18);
    vbox.set_margin_bottom(18);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let name_entry = Entry::new();
    name_entry.set_placeholder_text(Some("Action name (e.g. Launch Browser)"));
    let cmd_entry = Entry::new();
    cmd_entry.set_placeholder_text(Some("Executable path or desktop app (e.g. firefox)"));
    let accel_entry = Entry::new();
    accel_entry.set_placeholder_text(Some("Key combination (e.g. Super+B)"));
    let cat_dd = DropDown::new(
        Some(StringList::new(&["Custom", "Applications", "System"])),
        gtk4::Expression::NONE,
    );

    let actions = GtkBox::new(Orientation::Horizontal, 10);
    actions.set_halign(Align::End);
    let cancel_btn = Button::with_label("Cancel");
    cancel_btn.add_css_class("settings-secondary-btn");
    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }

    let add_btn = Button::with_label("Add Shortcut");
    add_btn.add_css_class("settings-action-btn");
    {
        let dlg = dialog.clone();
        let ctrl_c = Rc::clone(&ctrl);
        let name_c = name_entry.clone();
        let cmd_c = cmd_entry.clone();
        let accel_c = accel_entry.clone();
        let cat_c = cat_dd.clone();
        add_btn.connect_clicked(move |_| {
            let name = name_c.text().trim().to_string();
            let cmd = cmd_c.text().trim().to_string();
            let accel = accel_c.text().trim().to_string();
            if name.is_empty() || accel.is_empty() || cmd.is_empty() {
                ctrl_c.set_status_err("Fill in name, executable, and key combination");
                return;
            }
            if let Some(conflict) =
                find_shortcut_conflict(&ctrl_c.config.borrow().input.shortcuts, "", &accel)
            {
                ctrl_c.set_status_err(&format!(
                    "Shortcut conflict with \"{}\"",
                    conflict.label
                ));
                return;
            }
            let cat = match cat_c.selected() {
                1 => "Applications",
                2 => "System",
                _ => "Custom",
            };
            let id = format!(
                "custom_{}",
                name.to_ascii_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect::<String>()
            );
            ctrl_c.config.borrow_mut().input.shortcuts.push(
                ShortcutBindingConfig {
                    id,
                    category: cat.into(),
                    label: name.clone(),
                    accelerator: accel.clone(),
                    command: Some(cmd),
                    custom: true,
                },
            );
            ctrl_c.persist_and_sync(None, &format!("Added custom shortcut \"{name}\" ({accel})"));
            ctrl_c.rebuild_page("shortcuts");
            dlg.close();
        });
    }

    for (title, widget) in [
        ("Shortcut Name", name_entry.upcast_ref::<gtk4::Widget>()),
        ("Application / Binary", cmd_entry.upcast_ref::<gtk4::Widget>()),
        ("Key Combination", accel_entry.upcast_ref::<gtk4::Widget>()),
        ("Category", cat_dd.upcast_ref::<gtk4::Widget>()),
    ] {
        let lbl = Label::new(Some(title));
        lbl.set_halign(Align::Start);
        vbox.append(&lbl);
        vbox.append(widget);
    }
    actions.append(&cancel_btn);
    actions.append(&add_btn);
    vbox.append(&actions);
    dialog.set_child(Some(&vbox));
    dialog.present();
}
