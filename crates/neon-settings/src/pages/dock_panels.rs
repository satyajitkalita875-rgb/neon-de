//! Desktop & Dock settings: exposes ONLY genuine, live-applied Dock, Top Bar,
//! Window Behavior, and Compositor Animation settings wired to `neon-shell` and `neon-compositor`.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ScrolledWindow, prelude::*};
use neon_config::{EffectsPreset, FocusMode, WindowButtonLayout};

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

    let (dock_opacity, panel_visible, panel_opacity, show_clock, show_workspaces) = {
        let c = ctrl.config.borrow();
        (
            f64::from(c.dock.transparency),
            c.panel.visible,
            f64::from(c.panel.transparency),
            c.panel.show_clock,
            c.panel.show_workspaces,
        )
    };

    append_rows(
        &shell_card,
        vec![
            slider_row(
                "Dock Opacity",
                "Background opacity of the floating bottom dock.",
                0.20,
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
                "Display the top menu bar with NEON workspaces, clock, and Quick Settings.",
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
            slider_row(
                "Top Bar Opacity",
                "Background opacity of the top menu bar.",
                0.20,
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
                "Show the NEON workspace switcher in the top-left corner.",
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

    // ── Animations & Compositor Effects ──────────────────────────────────────
    page.append(&section_heading("Animations & Effects"));
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
                "Balance visual effects and GPU usage.",
                &seg,
            ),
            toggle_row(
                "Enable Animations & Effects",
                "Smooth spring open, close, minimize, maximize, and workspace transitions.",
                fx.enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().effects.enabled = v;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated compositor effects");
                    }
                },
            ),
            toggle_row(
                "Reduced Motion",
                "Minimize motion amplitude for snappy, low-motion transitions.",
                fx.reduced_motion,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().effects.reduced_motion = v;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated reduced motion");
                    }
                },
            ),
            slider_row(
                "Animation Speed",
                "Higher values speed up window and workspace animations.",
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

    // ── Windows ──────────────────────────────────────────────────────────────
    page.append(&section_heading("Windows"));
    let win_card = boxed_card();
    let win_cfg = ctrl.config.borrow().windows.clone();

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
            slider_row(
                "Snap Gap",
                "Padding in pixels between snapped windows and screen edges.",
                0.0,
                24.0,
                2.0,
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
                "Window Buttons",
                "Titlebar button placement for GTK applications.",
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
                        ctrl_c.persist_and_sync(None, "Updated window button layout");
                    }
                },
            ),
        ],
    );
    page.append(&win_card);

    page.append(&page_reset_footer("dock", "Desktop & Dock", &ctrl));
    scroller
}
