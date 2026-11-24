//! 5.3 Desktop & Workspaces settings: dynamic/static workspaces, workspace count,
//! custom workspace names, workspace grid, animations, Overview, hot corners, and desktop menu.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Entry, Orientation, ScrolledWindow, prelude::*};

use crate::{
    metadata::CapabilityStatus,
    state::{SettingsController, build_effects_request},
    ui::{
        append_rows, boxed_card, capability_row, dropdown_row, page_shell, section_heading,
        setting_row, slider_row, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Desktop & Workspaces",
        "Manage dynamic virtual desktops, workspace names, Mission Control overview, hot corners, and desktop interaction.",
        Some("desktop"),
        &ctrl,
    );

    // ── Workspaces ───────────────────────────────────────────────────────────
    page.append(&section_heading("Workspaces"));
    let ws_card = boxed_card();

    let dynamic = ctrl.config.borrow().windows.dynamic_workspaces;
    let static_count = ctrl.config.borrow().windows.static_workspace_count as f64;
    let grid_cols = f64::from(ctrl.config.borrow().desktop.workspace_columns);
    let ws_anim = ctrl.config.borrow().desktop.workspace_animations;
    let per_mon = ctrl.config.borrow().desktop.per_monitor_workspaces;

    let names_entry = Entry::new();
    names_entry.set_width_chars(28);
    names_entry.set_text(&ctrl.config.borrow().desktop.workspace_names.join(", "));
    let save_names_btn = Button::with_label("Save");
    save_names_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        let entry_c = names_entry.clone();
        save_names_btn.connect_clicked(move |_| {
            let parsed: Vec<String> = entry_c
                .text()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if parsed.is_empty() {
                ctrl_c.set_status_err("Provide at least one workspace name");
                return;
            }
            ctrl_c.config.borrow_mut().desktop.workspace_names = parsed;
            ctrl_c.persist_and_sync(None, "Saved workspace names");
        });
    }
    let names_box = GtkBox::new(Orientation::Horizontal, 8);
    names_box.set_valign(Align::Center);
    names_box.append(&names_entry);
    names_box.append(&save_names_btn);

    let ws_shortcuts_btn = Button::with_label("Edit Workspace Shortcuts →");
    ws_shortcuts_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        ws_shortcuts_btn.connect_clicked(move |_| ctrl_c.navigate_to("shortcuts", None));
    }

    append_rows(
        &ws_card,
        vec![
            toggle_row(
                "Dynamic Workspaces",
                "Automatically create new empty workspaces and clean up unused ones.",
                dynamic,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().windows.dynamic_workspaces = active;
                        ctrl_c.persist_and_sync(None, "Updated workspace allocation mode");
                    }
                },
            ),
            slider_row(
                "Fixed Workspace Count",
                "Number of workspaces when Dynamic Workspaces is disabled.",
                1.0,
                12.0,
                1.0,
                static_count,
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().windows.static_workspace_count =
                            val.round() as usize;
                        ctrl_c.persist_and_sync(None, &format!("Fixed workspace count: {:.0}", val));
                    }
                },
            ),
            setting_row(
                "Workspace Names",
                "Comma-separated labels shown in the workspace switcher and Overview.",
                &names_box,
            ),
            slider_row(
                "Workspace Grid Columns",
                "Horizontal column layout in Mission Control workspace strip.",
                1.0,
                8.0,
                1.0,
                grid_cols,
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().desktop.workspace_columns = val.round() as u8;
                        ctrl_c.persist_and_sync(None, &format!("Workspace grid columns: {:.0}", val));
                    }
                },
            ),
            toggle_row(
                "Workspace Transition Animations",
                "Smoothly slide between workspaces when switching.",
                ws_anim,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.desktop.workspace_animations = active;
                            cfg.effects.workspace_animation = active;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated workspace animations");
                    }
                },
            ),
            toggle_row(
                "Per-Monitor Workspaces",
                "Allow independent active workspaces on secondary monitors.",
                per_mon,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().desktop.per_monitor_workspaces = active;
                        ctrl_c.config.borrow_mut().displays.per_monitor_workspaces = active;
                        ctrl_c.persist_and_sync(None, "Updated per-monitor workspace behavior");
                    }
                },
            ),
            setting_row(
                "Workspace Switching Shortcuts",
                "Customize Ctrl+Alt+Left/Right and Move-to-Workspace bindings.",
                &ws_shortcuts_btn,
            ),
        ],
    );
    page.append(&ws_card);

    // ── Overview, Hot Corners & Desktop Interaction ──────────────────────────
    page.append(&section_heading("Overview, Hot Corners & Desktop"));
    let overview_card = boxed_card();

    let overview_all = ctrl.config.borrow().desktop.overview_show_all_workspaces;
    let right_click = ctrl.config.borrow().desktop.desktop_right_click_menu;
    let hot_actions = ["none", "overview", "lock", "screenshot"];
    let hot_labels = ["Disabled", "Open Overview", "Lock Screen", "Take Screenshot"];
    let tl_idx = hot_actions
        .iter()
        .position(|a| *a == ctrl.config.borrow().desktop.hot_corner_top_left.as_str())
        .unwrap_or(1) as u32;
    let tr_idx = hot_actions
        .iter()
        .position(|a| *a == ctrl.config.borrow().desktop.hot_corner_top_right.as_str())
        .unwrap_or(0) as u32;

    let gestures_btn = Button::with_label("Touchpad Gestures →");
    gestures_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        gestures_btn.connect_clicked(move |_| ctrl_c.navigate_to("input", None));
    }

    append_rows(
        &overview_card,
        vec![
            toggle_row(
                "Show All Workspaces in Overview",
                "Display thumbnails for every active workspace in Mission Control.",
                overview_all,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().desktop.overview_show_all_workspaces = active;
                        ctrl_c.persist_and_sync(None, "Updated Overview workspace visibility");
                    }
                },
            ),
            dropdown_row(
                "Top-Left Hot Corner",
                "Action triggered when pushing the pointer into the top-left corner.",
                &hot_labels,
                tl_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let act = hot_actions.get(idx as usize).copied().unwrap_or("none");
                        ctrl_c.config.borrow_mut().desktop.hot_corner_top_left = act.into();
                        ctrl_c.persist_and_sync(None, "Updated top-left hot corner");
                    }
                },
            ),
            dropdown_row(
                "Top-Right Hot Corner",
                "Action triggered when pushing the pointer into the top-right corner.",
                &hot_labels,
                tr_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let act = hot_actions.get(idx as usize).copied().unwrap_or("none");
                        ctrl_c.config.borrow_mut().desktop.hot_corner_top_right = act.into();
                        ctrl_c.persist_and_sync(None, "Updated top-right hot corner");
                    }
                },
            ),
            toggle_row(
                "Desktop Right-Click Context Menu",
                "Show native NEON DE actions (Terminal, Files, Wallpaper, Settings, Lock) on desktop right-click.",
                right_click,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().desktop.desktop_right_click_menu = active;
                        ctrl_c.persist_and_sync(None, "Updated desktop right-click menu preference");
                    }
                },
            ),
            capability_row(
                "Desktop File Icons",
                "Direct file icons on the wallpaper surface are not enabled in neon-shell; use the Files app or Dock.",
                CapabilityStatus::Unsupported(
                    "Desktop icon grid layer is not implemented in neon-shell yet.",
                ),
            ),
            setting_row(
                "Trackpad & Multi-Touch Gestures",
                "Configure 3-finger and 4-finger workspace/overview gestures.",
                &gestures_btn,
            ),
        ],
    );
    page.append(&overview_card);

    scroller
}
