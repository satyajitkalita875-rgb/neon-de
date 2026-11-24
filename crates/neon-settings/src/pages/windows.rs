//! 5.5 Windows & Window Management: focus policy, raise on focus, placement, snapping/tiling,
//! titlebar double-click, macOS traffic-light vs standard button layout, SSD toggle,
//! and Per-Application Window Rules editor.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, CheckButton, Entry, Label, Orientation, ScrolledWindow,
    SpinButton, Window, prelude::*,
};
use neon_config::{
    FocusMode, TitlebarAction, WindowButtonLayout, WindowPlacement, WindowRuleConfig,
};

use crate::{
    state::{
        SettingsController, apply_window_button_layout, build_effects_request,
        build_window_behavior_request,
    },
    ui::{
        append_rows, boxed_card, dropdown_row, page_shell, section_heading, setting_row,
        slider_row, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Windows & Management",
        "Control focus behavior, edge snapping, window placement, titlebar buttons, and per-application rules.",
        Some("windows"),
        &ctrl,
    );

    let win_cfg = ctrl.config.borrow().windows.clone();
    let eff_cfg = ctrl.config.borrow().effects.clone();

    // ── Focus, Placement & Snapping ──────────────────────────────────────────
    page.append(&section_heading("Focus, Placement & Snapping"));
    let behavior_card = boxed_card();

    let focus_idx = match win_cfg.focus_mode {
        FocusMode::ClickToFocus => 0,
        FocusMode::FocusFollowsMouse => 1,
    };
    let place_idx = match win_cfg.placement {
        WindowPlacement::Smart => 0,
        WindowPlacement::Center => 1,
        WindowPlacement::Cascade => 2,
    };

    append_rows(
        &behavior_card,
        vec![
            dropdown_row(
                "Window Focus Policy",
                "Click-to-focus (default) or Focus-follows-mouse (applied live via compositor IPC).",
                &["Click to Focus", "Focus Follows Mouse"],
                focus_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().windows.focus_mode = if idx == 1 {
                            FocusMode::FocusFollowsMouse
                        } else {
                            FocusMode::ClickToFocus
                        };
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated window focus policy");
                    }
                },
            ),
            toggle_row(
                "Raise Window on Focus",
                "Bring focused windows to the top of the stacking order.",
                win_cfg.raise_on_focus,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().windows.raise_on_focus = active;
                        ctrl_c.persist_and_sync(None, "Updated raise-on-focus behavior");
                    }
                },
            ),
            dropdown_row(
                "New Window Placement",
                "How newly opened floating windows are positioned on screen.",
                &["Smart (Avoid Overlap)", "Centered", "Cascaded"],
                place_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().windows.placement = match idx {
                            1 => WindowPlacement::Center,
                            2 => WindowPlacement::Cascade,
                            _ => WindowPlacement::Smart,
                        };
                        ctrl_c.persist_and_sync(None, "Updated window placement policy");
                    }
                },
            ),
            toggle_row(
                "Edge Snapping & Half-Screen Tiling",
                "Snap windows when dragged to screen edges or via Super+Left/Right.",
                win_cfg.auto_snap,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().windows.auto_snap = active;
                        ctrl_c.persist_and_sync(None, "Updated window snapping");
                    }
                },
            ),
            slider_row(
                "Tiling & Snap Gap (px)",
                "Inner margin between tiled/snapped windows and screen edges (live IPC).",
                0.0,
                32.0,
                2.0,
                f64::from(win_cfg.snap_gap),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().windows.snap_gap = val.round() as i32;
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Snap gap set to {:.0} px", val));
                    }
                },
            ),
            toggle_row(
                "Center Modal Dialogs",
                "Open dialogs centered over their parent application window.",
                win_cfg.center_dialogs,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().windows.center_dialogs = active;
                        ctrl_c.persist_and_sync(None, "Updated dialog centering");
                    }
                },
            ),
        ],
    );
    page.append(&behavior_card);

    // ── Title Bar, Decorations & Visual Frame ────────────────────────────────
    page.append(&section_heading("Title Bar & Window Decorations"));
    let deco_card = boxed_card();

    let dbl_idx = match win_cfg.double_click_titlebar {
        TitlebarAction::Maximize => 0,
        TitlebarAction::Minimize => 1,
        TitlebarAction::None => 2,
    };
    let btn_layout_idx = match win_cfg.button_layout {
        WindowButtonLayout::LeftTrafficLights => 0,
        WindowButtonLayout::RightStandard => 1,
    };

    append_rows(
        &deco_card,
        vec![
            dropdown_row(
                "Window Control Buttons",
                "macOS-style Left Traffic Lights (Close/Min/Max) or Standard Right buttons across apps.",
                &["Left (macOS Traffic Lights)", "Right (Standard)"],
                btn_layout_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let layout = if idx == 1 {
                            WindowButtonLayout::RightStandard
                        } else {
                            WindowButtonLayout::LeftTrafficLights
                        };
                        ctrl_c.config.borrow_mut().windows.button_layout = layout;
                        apply_window_button_layout(layout);
                        ctrl_c.persist_and_sync(None, "Updated window button layout");
                    }
                },
            ),
            dropdown_row(
                "Double-Click Title Bar Action",
                "Action performed when double-clicking a window titlebar.",
                &["Maximize / Restore", "Minimize", "Do Nothing"],
                dbl_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().windows.double_click_titlebar = match idx {
                            1 => TitlebarAction::Minimize,
                            2 => TitlebarAction::None,
                            _ => TitlebarAction::Maximize,
                        };
                        ctrl_c.persist_and_sync(None, "Updated titlebar double-click action");
                    }
                },
            ),
            toggle_row(
                "Server-Side Decorations (SSD)",
                "Draw uniform NEON DE titlebars with traffic lights for XDG/XWayland windows that request SSD.",
                win_cfg.server_side_decorations,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().windows.server_side_decorations = active;
                        ctrl_c.persist_and_sync(None, "Updated server-side decorations preference");
                    }
                },
            ),
            toggle_row(
                "Window Shadows & Rounded Borders",
                "Live compositor drop shadows and rounded borders.",
                eff_cfg.shadows_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.shadows_enabled = active;
                            cfg.effects.borders_enabled = active;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated window shadows & borders");
                    }
                },
            ),
            slider_row(
                "Inactive Window Opacity",
                "Subtle dimming/transparency for unfocused windows (live IPC).",
                0.60,
                1.0,
                0.02,
                f64::from(eff_cfg.inactive_opacity),
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().effects.inactive_opacity = val as f32;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated inactive window opacity");
                    }
                },
            ),
        ],
    );
    page.append(&deco_card);

    // ── Per-Application Window Rules ─────────────────────────────────────────
    page.append(&section_heading("Per-Application Window Rules"));
    let rules_card = boxed_card();

    let add_rule_btn = Button::with_label("Add Window Rule…");
    add_rule_btn.add_css_class("settings-action-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        add_rule_btn.connect_clicked(move |_| open_add_rule_dialog(Rc::clone(&ctrl_c)));
    }

    let mut rule_rows = vec![setting_row(
        "Application Matching Rules",
        "Match by Wayland app_id or WM_CLASS to set always-on-top, maximized, workspace, monitor, or size limits.",
        &add_rule_btn,
    )];

    for (idx, rule) in win_cfg.rules.iter().enumerate() {
        let mut summary_parts = Vec::new();
        if rule.always_on_top {
            summary_parts.push("Always on Top".to_string());
        }
        if rule.start_maximized {
            summary_parts.push("Start Maximized".to_string());
        }
        if let Some(ws) = rule.workspace {
            summary_parts.push(format!("Workspace {ws}"));
        }
        if let Some(ref mon) = rule.preferred_monitor {
            summary_parts.push(format!("Monitor {mon}"));
        }
        if let (Some(w), Some(h)) = (rule.min_width, rule.min_height) {
            summary_parts.push(format!("Min {w}×{h}"));
        }
        if let (Some(w), Some(h)) = (rule.max_width, rule.max_height) {
            summary_parts.push(format!("Max {w}×{h}"));
        }
        let summary = if summary_parts.is_empty() {
            "No overrides configured".to_string()
        } else {
            summary_parts.join(" • ")
        };

        let del_btn = Button::with_label("Remove");
        del_btn.add_css_class("settings-secondary-btn");
        let ctrl_c = Rc::clone(&ctrl);
        del_btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().windows.rules.remove(idx);
            ctrl_c.persist_and_sync(None, "Removed window rule");
            ctrl_c.rebuild_page("windows");
        });
        rule_rows.push(setting_row(&rule.app_id, &summary, &del_btn));
    }

    append_rows(&rules_card, rule_rows);
    page.append(&rules_card);

    scroller
}

fn open_add_rule_dialog(ctrl: Rc<SettingsController>) {
    let dialog = Window::builder()
        .transient_for(&ctrl.window)
        .modal(true)
        .resizable(false)
        .title("Add Per-Application Window Rule")
        .default_width(430)
        .build();
    dialog.add_css_class("neon-settings-window");

    let vbox = GtkBox::new(Orientation::Vertical, 10);
    vbox.set_margin_top(18);
    vbox.set_margin_bottom(18);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    let app_entry = Entry::new();
    app_entry.set_placeholder_text(Some("e.g. firefox, code, com.blackmagicdesign.resolve"));

    let aot_chk = CheckButton::with_label("Always on top");
    let max_chk = CheckButton::with_label("Start maximized");

    let ws_spin = SpinButton::with_range(0.0, 16.0, 1.0);
    ws_spin.set_value(0.0);
    let mon_entry = Entry::new();
    mon_entry.set_placeholder_text(Some("Optional output name (e.g. eDP-1, HDMI-A-1)"));

    let min_w = SpinButton::with_range(0.0, 7680.0, 50.0);
    let min_h = SpinButton::with_range(0.0, 4320.0, 50.0);
    let size_row = GtkBox::new(Orientation::Horizontal, 8);
    size_row.append(&Label::new(Some("Min W:")));
    size_row.append(&min_w);
    size_row.append(&Label::new(Some("Min H:")));
    size_row.append(&min_h);

    let actions = GtkBox::new(Orientation::Horizontal, 10);
    actions.set_halign(Align::End);
    actions.set_margin_top(8);

    let cancel_btn = Button::with_label("Cancel");
    cancel_btn.add_css_class("settings-secondary-btn");
    {
        let dlg = dialog.clone();
        cancel_btn.connect_clicked(move |_| dlg.close());
    }

    let save_btn = Button::with_label("Save Rule");
    save_btn.add_css_class("settings-action-btn");
    {
        let dlg = dialog.clone();
        let ctrl_c = Rc::clone(&ctrl);
        let app_c = app_entry.clone();
        let aot_c = aot_chk.clone();
        let max_c = max_chk.clone();
        let ws_c = ws_spin.clone();
        let mon_c = mon_entry.clone();
        let min_w_c = min_w.clone();
        let min_h_c = min_h.clone();
        save_btn.connect_clicked(move |_| {
            let app_id = app_c.text().trim().to_string();
            if app_id.is_empty() {
                ctrl_c.set_status_err("App ID cannot be empty");
                return;
            }
            let ws_val = ws_c.value_as_int();
            let mon_txt = mon_c.text().trim().to_string();
            let mw = min_w_c.value_as_int();
            let mh = min_h_c.value_as_int();
            let rule = WindowRuleConfig {
                app_id: app_id.clone(),
                always_on_top: aot_c.is_active(),
                start_maximized: max_c.is_active(),
                workspace: if ws_val > 0 { Some(ws_val as usize) } else { None },
                preferred_monitor: if mon_txt.is_empty() { None } else { Some(mon_txt) },
                min_width: if mw > 0 { Some(mw) } else { None },
                min_height: if mh > 0 { Some(mh) } else { None },
                max_width: None,
                max_height: None,
            };
            ctrl_c.config.borrow_mut().windows.rules.push(rule);
            ctrl_c.persist_and_sync(None, &format!("Added window rule for {app_id}"));
            ctrl_c.rebuild_page("windows");
            dlg.close();
        });
    }

    let lbl_app = Label::new(Some("Application ID (app_id / WM_CLASS)"));
    lbl_app.set_halign(Align::Start);
    let lbl_ws = Label::new(Some("Assign to Workspace (0 = no override)"));
    lbl_ws.set_halign(Align::Start);
    let lbl_mon = Label::new(Some("Preferred Monitor (optional)"));
    lbl_mon.set_halign(Align::Start);

    actions.append(&cancel_btn);
    actions.append(&save_btn);
    vbox.append(&lbl_app);
    vbox.append(&app_entry);
    vbox.append(&aot_chk);
    vbox.append(&max_chk);
    vbox.append(&lbl_ws);
    vbox.append(&ws_spin);
    vbox.append(&lbl_mon);
    vbox.append(&mon_entry);
    vbox.append(&size_row);
    vbox.append(&actions);

    dialog.set_child(Some(&vbox));
    dialog.present();
}
