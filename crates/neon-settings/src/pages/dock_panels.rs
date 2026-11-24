//! 5.4 Dock & Panels settings: dock placement, icon size, auto-hide, intelligent hide,
//! magnification, glass/solid/minimal style, running indicators, recent/trash/grouping,
//! top panel position/height/clock/date/tray/notifications/quick settings, and multi-panel config.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ScrolledWindow, prelude::*};
use neon_config::{DockBackgroundStyle, DockPosition, ExtraPanelConfig, PanelPosition};

use crate::{
    state::{SettingsController, build_appearance_request},
    ui::{
        append_rows, boxed_card, dropdown_row, page_shell, section_heading, setting_row,
        slider_row, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Dock & Panels",
        "Configure the floating application dock, top menu bar, clock & tray modules, and additional panels.",
        Some("dock"),
        &ctrl,
    );

    // ── Floating Dock ────────────────────────────────────────────────────────
    page.append(&section_heading("Floating Dock"));
    let dock_card = boxed_card();

    let dock_cfg = ctrl.config.borrow().dock.clone();
    let pos_idx = match dock_cfg.position {
        DockPosition::Bottom => 0,
        DockPosition::Left => 1,
        DockPosition::Right => 2,
    };
    let bg_idx = match dock_cfg.background_style {
        DockBackgroundStyle::Glass => 0,
        DockBackgroundStyle::Solid => 1,
        DockBackgroundStyle::Minimal => 2,
    };

    append_rows(
        &dock_card,
        vec![
            dropdown_row(
                "Dock Position",
                "Screen edge where the floating dock anchors.",
                &["Bottom", "Left", "Right"],
                pos_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().dock.position = match idx {
                            1 => DockPosition::Left,
                            2 => DockPosition::Right,
                            _ => DockPosition::Bottom,
                        };
                        ctrl_c.persist_and_sync(None, "Updated dock position");
                    }
                },
            ),
            slider_row(
                "Icon Size (px)",
                "Size of pinned and running app icons on the dock.",
                28.0,
                72.0,
                2.0,
                f64::from(dock_cfg.icon_size),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().dock.icon_size = val.round() as u32;
                        ctrl_c.persist_and_sync(None, &format!("Dock icon size: {:.0} px", val));
                    }
                },
            ),
            dropdown_row(
                "Background Style",
                "Surface rendering style for the dock pill.",
                &["Frosted Glass", "Solid Surface", "Minimal Floating"],
                bg_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().dock.background_style = match idx {
                            1 => DockBackgroundStyle::Solid,
                            2 => DockBackgroundStyle::Minimal,
                            _ => DockBackgroundStyle::Glass,
                        };
                        ctrl_c.persist_and_sync(None, "Updated dock background style");
                    }
                },
            ),
            slider_row(
                "Dock Opacity",
                "Controls dock surface transparency live.",
                0.30,
                1.0,
                0.05,
                f64::from(dock_cfg.transparency),
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().dock.transparency = val as f32;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated dock opacity");
                    }
                },
            ),
            toggle_row(
                "Auto-Hide Dock",
                "Automatically slide the dock off-screen when unused.",
                dock_cfg.auto_hide,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().dock.auto_hide = active;
                        ctrl_c.persist_and_sync(None, "Updated dock auto-hide");
                    }
                },
            ),
            toggle_row(
                "Intelligent Auto-Hide",
                "Hide only when an active window overlaps the dock area.",
                dock_cfg.intelligent_hide,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().dock.intelligent_hide = active;
                        ctrl_c.persist_and_sync(None, "Updated intelligent dock hiding");
                    }
                },
            ),
            toggle_row(
                "Hover Magnification & Animations",
                "Smoothly enlarge dock icons on pointer hover.",
                dock_cfg.magnification,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.dock.magnification = active;
                            cfg.dock.animations = active;
                        }
                        ctrl_c.persist_and_sync(None, "Updated dock magnification");
                    }
                },
            ),
            toggle_row(
                "Running Application Indicators",
                "Show dot indicators beneath open applications.",
                dock_cfg.show_running_indicators,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().dock.show_running_indicators = active;
                        ctrl_c.persist_and_sync(None, "Updated running indicators");
                    }
                },
            ),
            toggle_row(
                "Group Application Windows",
                "Combine multiple windows of the same app under one dock icon.",
                dock_cfg.group_apps,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().dock.group_apps = active;
                        ctrl_c.persist_and_sync(None, "Updated dock window grouping");
                    }
                },
            ),
            toggle_row(
                "Show Recent Applications",
                "Display recently launched unpinned apps on the dock.",
                dock_cfg.show_recent_apps,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().dock.show_recent_apps = active;
                        ctrl_c.persist_and_sync(None, "Updated recent apps on dock");
                    }
                },
            ),
            toggle_row(
                "Show Trash Icon",
                "Keep a quick Trash shortcut at the end of the dock.",
                dock_cfg.show_trash,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().dock.show_trash = active;
                        ctrl_c.persist_and_sync(None, "Updated dock Trash shortcut");
                    }
                },
            ),
        ],
    );
    page.append(&dock_card);

    // ── Top Panel ────────────────────────────────────────────────────────────
    page.append(&section_heading("Primary Panel (Menu Bar)"));
    let panel_card = boxed_card();
    let panel_cfg = ctrl.config.borrow().panel.clone();

    let p_pos_idx = match panel_cfg.position {
        PanelPosition::Top => 0,
        PanelPosition::Bottom => 1,
    };

    append_rows(
        &panel_card,
        vec![
            toggle_row(
                "Show Top Panel",
                "Toggle visibility of the main NEON DE menu bar live.",
                panel_cfg.visible,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().panel.visible = active;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated panel visibility");
                    }
                },
            ),
            dropdown_row(
                "Panel Position",
                "Edge placement for the primary system panel.",
                &["Top", "Bottom"],
                p_pos_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().panel.position = if idx == 1 {
                            PanelPosition::Bottom
                        } else {
                            PanelPosition::Top
                        };
                        ctrl_c.persist_and_sync(None, "Updated primary panel position");
                    }
                },
            ),
            slider_row(
                "Panel Height (px)",
                "Vertical thickness of the system panel.",
                26.0,
                52.0,
                2.0,
                f64::from(panel_cfg.height),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().panel.height = val.round() as u32;
                        ctrl_c.persist_and_sync(None, &format!("Panel height: {:.0} px", val));
                    }
                },
            ),
            slider_row(
                "Panel Opacity",
                "Live transparency level of the top panel.",
                0.30,
                1.0,
                0.05,
                f64::from(panel_cfg.transparency),
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().panel.transparency = val as f32;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated panel opacity");
                    }
                },
            ),
            toggle_row(
                "Show Clock",
                "Display live time in the center of the top bar.",
                panel_cfg.show_clock,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().panel.show_clock = active;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated panel clock");
                    }
                },
            ),
            toggle_row(
                "24-Hour Clock Format",
                "Use 24-hour time instead of 12-hour AM/PM.",
                panel_cfg.clock_24h,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().panel.clock_24h = active;
                        ctrl_c.persist_and_sync(None, "Updated 24-hour clock format");
                    }
                },
            ),
            toggle_row(
                "Show Date & Seconds",
                "Include calendar date alongside the top-bar clock.",
                panel_cfg.show_date,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().panel.show_date = active;
                        ctrl_c.persist_and_sync(None, "Updated panel date display");
                    }
                },
            ),
            toggle_row(
                "Show Workspace Pills",
                "Display interactive workspace switcher buttons on the top panel.",
                panel_cfg.show_workspaces,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().panel.show_workspaces = active;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated panel workspace pills");
                    }
                },
            ),
            toggle_row(
                "System Tray & Notification Area",
                "Show StatusNotifier tray indicators and notification bell.",
                panel_cfg.show_system_tray,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.panel.show_system_tray = active;
                            cfg.panel.show_notifications = active;
                        }
                        ctrl_c.persist_and_sync(None, "Updated system tray & notification area");
                    }
                },
            ),
            toggle_row(
                "Quick Settings Control Center",
                "Show Wi-Fi, Bluetooth, Volume, and Power quick toggles in the panel.",
                panel_cfg.show_quick_settings,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().panel.show_quick_settings = active;
                        ctrl_c.persist_and_sync(None, "Updated Quick Settings indicator");
                    }
                },
            ),
        ],
    );
    page.append(&panel_card);

    // ── Multi-Panel Architecture (Secondary Panels) ──────────────────────────
    page.append(&section_heading("Additional Panels (Multi-Panel Schema)"));
    let extra_card = boxed_card();

    let add_panel_btn = Button::with_label("Add Secondary Panel");
    add_panel_btn.add_css_class("settings-action-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        add_panel_btn.connect_clicked(move |_| {
            {
                let mut cfg = ctrl_c.config.borrow_mut();
                let next_num = cfg.panel.extra_panels.len() + 2;
                cfg.panel.extra_panels.push(ExtraPanelConfig {
                    id: format!("panel-{next_num}"),
                    position: PanelPosition::Bottom,
                    height: 32,
                    visible: true,
                    transparency: 0.86,
                    modules: vec!["workspaces".into(), "clock".into(), "tray".into()],
                });
            }
            ctrl_c.persist_and_sync(None, "Added secondary panel configuration");
            ctrl_c.rebuild_page("dock");
        });
    }

    let mut extra_rows = vec![setting_row(
        "User-Created Panels",
        "Persisted in config.toml so neon-shell can spawn additional multi-monitor or edge panels.",
        &add_panel_btn,
    )];

    let extra_panels = ctrl.config.borrow().panel.extra_panels.clone();
    for (idx, ep) in extra_panels.into_iter().enumerate() {
        let rm_btn = Button::with_label("Remove");
        rm_btn.add_css_class("settings-secondary-btn");
        let ctrl_c = Rc::clone(&ctrl);
        rm_btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().panel.extra_panels.remove(idx);
            ctrl_c.persist_and_sync(None, "Removed secondary panel");
            ctrl_c.rebuild_page("dock");
        });
        let right = GtkBox::new(Orientation::Horizontal, 8);
        right.set_valign(Align::Center);
        right.append(&rm_btn);
        extra_rows.push(setting_row(
            &ep.id,
            &format!(
                "Edge: {:?} • {} px • Modules: {}",
                ep.position,
                ep.height,
                ep.modules.join(", ")
            ),
            &right,
        ));
    }

    append_rows(&extra_card, extra_rows);
    page.append(&extra_card);

    scroller
}
