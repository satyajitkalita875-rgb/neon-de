//! Desktop & Dock settings: exposes ONLY genuine, live-applied Dock, Top Bar,
//! Window Behavior, and Compositor Animation settings wired to `neon-shell` and `neon-compositor`.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ScrolledWindow, prelude::*};
use neon_config::{
    DockPosition, EffectsPreset, FocusMode, PanelMode, TilingLayout, WindowButtonLayout,
};

use crate::{
    state::{
        SettingsController, apply_window_button_layout, build_appearance_request,
        build_effects_request, build_window_behavior_request,
    },
    ui::{
        append_rows, boxed_card, dropdown_row, page_reset_footer, page_shell, section_heading,
        setting_row, slider_row, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    // ── Dock & Top Bar ───────────────────────────────────────────────────────
    page.append(&section_heading("Dock & Top Bar"));
    let shell_card = boxed_card();

    let (
        dock_position,
        dock_icon_size,
        dock_height,
        dock_magnification,
        dock_auto_hide,
        dock_smart_hide,
        dock_opacity,
        show_desktop_icons,
        panel_mode,
        dynamic_island_enabled,
        panel_visible,
        panel_smart_hide,
        panel_opacity,
        show_clock,
        show_workspaces,
    ) = {
        let c = ctrl.config.borrow();
        (
            c.dock.position,
            f64::from(c.dock.icon_size),
            f64::from(c.dock.height),
            c.dock.magnification,
            c.dock.auto_hide,
            c.dock.intelligent_hide,
            f64::from(c.dock.transparency),
            c.desktop.show_desktop_icons,
            c.panel.mode,
            c.panel.dynamic_island_enabled,
            c.panel.visible,
            c.panel.smart_hide,
            f64::from(c.panel.transparency),
            c.panel.show_clock,
            c.panel.show_workspaces,
        )
    };

    let dock_pos_idx = match dock_position {
        DockPosition::Bottom => 0,
        DockPosition::Left => 1,
        DockPosition::Right => 2,
    };

    let dock_vis_idx = if dock_auto_hide {
        1
    } else if dock_smart_hide {
        2
    } else {
        0
    };

    let panel_mode_idx = match panel_mode {
        PanelMode::TopAndDock => 0,
        PanelMode::CombinedBottom => 1,
    };

    append_rows(
        &shell_card,
        vec![
            dropdown_row(
                "Panel Mode",
                "Switch between separate Top Bar + Dock or a Combined Bottom Taskbar with Dynamic Island.",
                &["Top Bar + Dock", "Combined Bottom Taskbar"],
                panel_mode_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let next_mode = if idx == 1 {
                            PanelMode::CombinedBottom
                        } else {
                            PanelMode::TopAndDock
                        };
                        if ctrl_c.config.borrow().panel.mode != next_mode {
                            ctrl_c.config.borrow_mut().panel.mode = next_mode;
                            let req = build_appearance_request(&ctrl_c.config.borrow());
                            ctrl_c.persist_and_sync(Some(req), "Updated panel mode");
                        }
                    }
                },
            ),
            toggle_row(
                "Dynamic Island",
                "Show the interactive pill at top center in Combined Bottom Taskbar mode for media, recording, and live notifications.",
                dynamic_island_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().panel.dynamic_island_enabled = v;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated Dynamic Island");
                    }
                },
            ),
            dropdown_row(
                "Dock Position",
                "Place the floating dock on the bottom, left, or right screen edge.",
                &["Bottom", "Left", "Right"],
                dock_pos_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().dock.position = match idx {
                            1 => DockPosition::Left,
                            2 => DockPosition::Right,
                            _ => DockPosition::Bottom,
                        };
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated dock position");
                    }
                },
            ),
            slider_row(
                "Dock Icon Size",
                "Adjust the icon size inside the dock or taskbar (22–64 px).",
                22.0,
                64.0,
                2.0,
                dock_icon_size,
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().dock.icon_size = val.round() as u32;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            &format!("Dock icon size: {:.0} px", val),
                        );
                    }
                },
            ),
            slider_row(
                "Dock Height",
                "Adjust the height of the floating dock or combined bottom taskbar (36–88 px).",
                36.0,
                88.0,
                2.0,
                dock_height,
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().dock.height = val.round() as u32;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            &format!("Dock height: {:.0} px", val),
                        );
                    }
                },
            ),
            toggle_row(
                "Dock Magnification",
                "Smoothly magnify dock icons on pointer hover.",
                dock_magnification,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().dock.magnification = v;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated dock magnification");
                    }
                },
            ),
            toggle_row(
                "Show Desktop Icons",
                "Display interactive files, folders, and shortcuts from ~/Desktop on the wallpaper.",
                show_desktop_icons,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().desktop.show_desktop_icons = v;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated desktop icons visibility");
                    }
                },
            ),
            dropdown_row(
                "Dock Visibility",
                "Always show the dock, auto-hide until bottom edge hover, or smart-hide when windows overlap it.",
                &["Always Visible", "Auto-Hide", "Smart Hide (Intelligent)"],
                dock_vis_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.dock.auto_hide = idx == 1;
                            cfg.dock.intelligent_hide = idx == 2;
                        }
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        let note = match idx {
                            1 => "Dock set to Auto-Hide",
                            2 => "Dock set to Smart Hide",
                            _ => "Dock set to Always Visible",
                        };
                        ctrl_c.persist_and_sync(Some(req), note);
                    }
                },
            ),
            slider_row(
                "Dock Opacity",
                "Background tint opacity of the floating dock (0% = clear glass, 100% = solid).",
                0.00,
                1.00,
                0.05,
                dock_opacity,
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().dock.transparency = val as f32;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Dock opacity: {:.0}%", val * 100.0));
                    }
                },
            ),
            toggle_row(
                "Show Top Bar",
                "Display the top menu bar with workspaces, clock, and Quick Settings.",
                panel_visible,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().panel.visible = v;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated top bar visibility");
                    }
                },
            ),
            toggle_row(
                "Smart Hide Top Bar",
                "Automatically hide the top bar when a window overlaps it; move pointer to the top edge to reveal.",
                panel_smart_hide,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().panel.smart_hide = v;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            if v {
                                "Enabled Smart Hide Top Bar"
                            } else {
                                "Disabled Smart Hide Top Bar"
                            },
                        );
                    }
                },
            ),
            slider_row(
                "Top Bar Opacity",
                "Background tint opacity of the top menu bar (0% = clear glass, 100% = solid).",
                0.00,
                1.00,
                0.05,
                panel_opacity,
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().panel.transparency = val as f32;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            &format!("Top bar opacity: {:.0}%", val * 100.0),
                        );
                    }
                },
            ),
            toggle_row(
                "Show Clock",
                "Display date and time in the top bar.",
                show_clock,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().panel.show_clock = v;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated clock visibility");
                    }
                },
            ),
            toggle_row(
                "Show Workspaces Button",
                "Show the Task View workspaces button and dots on the panel.",
                show_workspaces,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().panel.show_workspaces = v;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated workspaces button");
                    }
                },
            ),
        ],
    );
    page.append(&shell_card);

    // ── Material 3 Expressive Motion & Compositor Effects ────────────────────
    page.append(&section_heading("Material 3 Expressive Motion & Effects"));
    let fx_card = boxed_card();

    let fx = ctrl.config.borrow().effects.clone();
    let seg = GtkBox::new(Orientation::Horizontal, 3);
    seg.add_css_class("settings-seg-track");
    seg.set_valign(Align::Center);

    let presets = [
        ("Performance", EffectsPreset::Performance),
        ("Balanced", EffectsPreset::Balanced),
        ("Visual", EffectsPreset::VisualQuality),
    ];
    for (label, preset) in presets {
        let btn = Button::with_label(label);
        btn.add_css_class("settings-seg-btn");
        if fx.preset == preset {
            btn.add_css_class("active");
        }
        let ctrl_c = Rc::clone(&ctrl);
        btn.connect_clicked(move |_| {
            {
                let mut cfg = ctrl_c.config.borrow_mut();
                cfg.effects.apply_preset(preset);
                cfg.appearance.reduce_blur = matches!(preset, EffectsPreset::Performance);
            }
            let fx_req = build_effects_request(&ctrl_c.config.borrow());
            let app_req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(fx_req), &format!("Applied {label} preset"));
            let _ = ctrl_c.send_ipc(app_req);
            ctrl_c.rebuild_page("dock");
        });
        seg.append(&btn);
    }

    append_rows(
        &fx_card,
        vec![
            setting_row(
                "Quality Preset",
                "Balance Material 3 Expressive motion, Liquid Glass blur, and GPU usage.",
                &seg,
            ),
            toggle_row(
                "Material 3 Expressive Animations",
                "System-wide spring physics for apps, Dock, Quick Settings, and workspaces.",
                fx.enabled && !fx.reduced_motion,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        let needs_rebuild = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.enabled = v;
                            if v {
                                let was_reduced = cfg.effects.reduced_motion;
                                cfg.effects.reduced_motion = false;
                                let all_off = !cfg.effects.open_animation
                                    && !cfg.effects.close_animation
                                    && !cfg.effects.minimize_animation
                                    && !cfg.effects.maximize_animation
                                    && !cfg.effects.workspace_animation
                                    && !cfg.effects.overview_animation
                                    && !cfg.effects.dock_animation;
                                if all_off {
                                    cfg.effects.open_animation = true;
                                    cfg.effects.close_animation = true;
                                    cfg.effects.minimize_animation = true;
                                    cfg.effects.maximize_animation = true;
                                    cfg.effects.workspace_animation = true;
                                    cfg.effects.overview_animation = true;
                                    cfg.effects.dock_animation = true;
                                }
                                was_reduced || all_off
                            } else {
                                false
                            }
                        };
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated Material 3 Expressive animations");
                        if needs_rebuild {
                            ctrl_c.rebuild_page("dock");
                        }
                    }
                },
            ),
            toggle_row(
                "Reduced Motion",
                "Disable spring motion system-wide for instant, accessibility-friendly transitions.",
                fx.reduced_motion,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.reduced_motion = v;
                            if !v && cfg.effects.enabled {
                                let all_off = !cfg.effects.open_animation
                                    && !cfg.effects.close_animation
                                    && !cfg.effects.minimize_animation
                                    && !cfg.effects.maximize_animation
                                    && !cfg.effects.workspace_animation
                                    && !cfg.effects.overview_animation
                                    && !cfg.effects.dock_animation;
                                if all_off {
                                    cfg.effects.open_animation = true;
                                    cfg.effects.close_animation = true;
                                    cfg.effects.minimize_animation = true;
                                    cfg.effects.maximize_animation = true;
                                    cfg.effects.workspace_animation = true;
                                    cfg.effects.overview_animation = true;
                                    cfg.effects.dock_animation = true;
                                }
                            }
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated reduced motion");
                        ctrl_c.rebuild_page("dock");
                    }
                },
            ),
            slider_row(
                "Animation Speed",
                "Scale all compositor and shell spring animation speeds live.",
                0.5,
                2.5,
                0.1,
                fx.speed_scale,
                1,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().effects.speed_scale = val;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Animation speed: {val:.1}x"));
                    }
                },
            ),
            toggle_row(
                "Android App Open & Close",
                "Launch apps out of clicked Dock/Drawer icons and shrink back on close.",
                fx.open_animation && fx.close_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        let needs_rebuild = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.open_animation = v;
                            cfg.effects.close_animation = v;
                            if v && (!cfg.effects.enabled || cfg.effects.reduced_motion) {
                                cfg.effects.enabled = true;
                                cfg.effects.reduced_motion = false;
                                true
                            } else {
                                false
                            }
                        };
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated app open & close animations");
                        if needs_rebuild {
                            ctrl_c.rebuild_page("dock");
                        }
                    }
                },
            ),
            toggle_row(
                "Window Minimize & Maximize Morph",
                "Dock swoop minimize/restore and smooth spring maximize & snap transitions.",
                fx.minimize_animation && fx.maximize_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        let needs_rebuild = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.minimize_animation = v;
                            cfg.effects.maximize_animation = v;
                            if v && (!cfg.effects.enabled || cfg.effects.reduced_motion) {
                                cfg.effects.enabled = true;
                                cfg.effects.reduced_motion = false;
                                true
                            } else {
                                false
                            }
                        };
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated minimize & maximize animations");
                        if needs_rebuild {
                            ctrl_c.rebuild_page("dock");
                        }
                    }
                },
            ),
            toggle_row(
                "Workspaces & Mission Control",
                "Expressive springs for Mission Control, App Drawer, and Desk switches.",
                fx.workspace_animation && fx.overview_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        let needs_rebuild = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.workspace_animation = v;
                            cfg.effects.overview_animation = v;
                            if v && (!cfg.effects.enabled || cfg.effects.reduced_motion) {
                                cfg.effects.enabled = true;
                                cfg.effects.reduced_motion = false;
                                true
                            } else {
                                false
                            }
                        };
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated workspace & overview animations");
                        if needs_rebuild {
                            ctrl_c.rebuild_page("dock");
                        }
                    }
                },
            ),
            toggle_row(
                "Dock Spring & Launch Bounce",
                "Expressive Dock hover magnification, icon pop-in, and app launch hop.",
                fx.dock_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        let needs_rebuild = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.dock_animation = v;
                            if v && (!cfg.effects.enabled || cfg.effects.reduced_motion) {
                                cfg.effects.enabled = true;
                                cfg.effects.reduced_motion = false;
                                true
                            } else {
                                false
                            }
                        };
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated Dock spring animations");
                        if needs_rebuild {
                            ctrl_c.rebuild_page("dock");
                        }
                    }
                },
            ),
            toggle_row(
                "Window Borders",
                "Draw subtle rounded accent borders around focused windows.",
                fx.borders_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().effects.borders_enabled = v;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated window borders");
                    }
                },
            ),
            slider_row(
                "Inactive Window Opacity",
                "Slight dim/translucency on unfocused windows (1.00 = fully opaque).",
                0.70,
                1.00,
                0.02,
                f64::from(fx.inactive_opacity),
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().effects.inactive_opacity = val as f32;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            &format!("Inactive opacity: {:.0}%", val * 100.0),
                        );
                    }
                },
            ),
        ],
    );
    page.append(&fx_card);

    // ── Window Snapping & Tiling ─────────────────────────────────────────────
    page.append(&section_heading("Window Snapping & Tiling"));
    let win_card = boxed_card();
    let win_cfg = ctrl.config.borrow().windows.clone();

    let tiling_layout_idx = match win_cfg.tiling_layout {
        TilingLayout::Dwindle => 0,
        TilingLayout::MasterStack => 1,
    };
    let focus_idx = match win_cfg.focus_mode {
        FocusMode::ClickToFocus => 0,
        FocusMode::FocusFollowsMouse => 1,
    };
    let btn_layout_idx = match win_cfg.button_layout {
        WindowButtonLayout::LeftTrafficLights => 0,
        WindowButtonLayout::RightStandard => 1,
    };

    append_rows(
        &win_card,
        vec![
            toggle_row(
                "Hyprland Auto-Tiling",
                "Automatically tile open windows across all workspaces (Super+T toggle, Super+Arrows focus, Super+Shift+Arrows swap, Super+Ctrl+Arrows resize, Super+Shift+F float, Super+J flip split).",
                win_cfg.auto_tiling,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().windows.auto_tiling = v;
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            if v {
                                "Enabled Hyprland Auto-Tiling"
                            } else {
                                "Disabled Hyprland Auto-Tiling"
                            },
                        );
                    }
                },
            ),
            dropdown_row(
                "Tiling Layout",
                "Layout algorithm used when Hyprland Auto-Tiling is active.",
                &["Dwindle (BSP Spiral)", "Master–Stack"],
                tiling_layout_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let layout = if idx == 1 {
                            TilingLayout::MasterStack
                        } else {
                            TilingLayout::Dwindle
                        };
                        ctrl_c.config.borrow_mut().windows.tiling_layout = layout;
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            if idx == 1 {
                                "Tiling layout: Master–Stack"
                            } else {
                                "Tiling layout: Dwindle"
                            },
                        );
                    }
                },
            ),
            slider_row(
                "Tiling Gap",
                "Spacing in pixels between tiled windows and screen edges when Auto-Tiling is active.",
                0.0,
                24.0,
                1.0,
                f64::from(win_cfg.tiling_gap),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().windows.tiling_gap = val.round() as i32;
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Tiling gap: {val:.0}px"));
                    }
                },
            ),
            toggle_row(
                "Drag-to-Edge Window Snapping",
                "Show translucent glass Snap Preview when dragging windows to screen edges or corners (active when Auto-Tiling is off).",
                win_cfg.auto_snap,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().windows.auto_snap = v;
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated drag-to-edge window snapping");
                    }
                },
            ),
            toggle_row(
                "Windows 11 Snap Layouts Drop Bar",
                "Reveal interactive 5-layout Snap Bar at the top-center edge while dragging a window.",
                win_cfg.snap_layouts_bar,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().windows.snap_layouts_bar = v;
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated Snap Layouts drop bar");
                    }
                },
            ),
            slider_row(
                "Snap Gap",
                "Padding in pixels between drag-snapped windows and screen edges.",
                0.0,
                24.0,
                1.0,
                f64::from(win_cfg.snap_gap),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().windows.snap_gap = val.round() as i32;
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Snap gap: {val:.0}px"));
                    }
                },
            ),
            dropdown_row(
                "Window Focus",
                "How windows gain keyboard focus.",
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
            dropdown_row(
                "Window Buttons",
                "Place close, minimize, and maximize buttons on the left or right across all apps.",
                &["Left (macOS)", "Right (Standard)"],
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
                        ctrl_c.notify_button_layout_changed(layout);
                        let req = build_window_behavior_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated window button layout");
                    }
                },
            ),
        ],
    );
    page.append(&win_card);

    page.append(&page_reset_footer("dock", "Desktop & Dock", &ctrl));
    scroller
}
