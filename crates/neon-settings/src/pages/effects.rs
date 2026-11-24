//! 5.6 Effects & Animations: 3 Performance Presets (Performance, Balanced, Visual Quality),
//! per-action animation controls, blur/shadow/corner/opacity tuning, speed scale, and
//! reduced motion — integrated live with `neon-renderer` via `UpdateEffects`.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Orientation, ScrolledWindow, prelude::*};
use neon_config::EffectsPreset;

use crate::{
    state::{SettingsController, build_appearance_request, build_effects_request},
    ui::{
        append_rows, boxed_card, page_shell, section_heading, setting_row, slider_row, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Effects & Animations",
        "Balance GPU responsiveness and visual polish with curated compositor presets and live shader toggles.",
        Some("effects"),
        &ctrl,
    );

    let eff = ctrl.config.borrow().effects.clone();

    // ── 3 Performance Presets ────────────────────────────────────────────────
    page.append(&section_heading("Performance Preset"));
    let preset_card = boxed_card();

    let seg = GtkBox::new(Orientation::Horizontal, 4);
    seg.add_css_class("settings-seg-track");
    seg.set_valign(Align::Center);

    let presets = [
        ("Performance", EffectsPreset::Performance),
        ("Balanced", EffectsPreset::Balanced),
        ("Visual Quality", EffectsPreset::VisualQuality),
    ];

    for (label, preset) in presets {
        let btn = Button::with_label(label);
        btn.add_css_class("settings-seg-btn");
        if eff.preset == preset {
            btn.add_css_class("active");
        }
        let ctrl_c = Rc::clone(&ctrl);
        btn.connect_clicked(move |_| {
            {
                let mut cfg = ctrl_c.config.borrow_mut();
                cfg.effects.apply_preset(preset);
                cfg.appearance.reduce_blur = !cfg.effects.blur_enabled;
            }
            let app_req = build_appearance_request(&ctrl_c.config.borrow());
            let _ = ctrl_c.send_ipc(app_req);
            let eff_req = build_effects_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(eff_req), &format!("Applied {label} effects preset"));
            ctrl_c.rebuild_page("effects");
        });
        seg.append(&btn);
    }

    preset_card.append(&setting_row(
        "Renderer Quality Profile",
        "Performance disables blur/shadows for lowest latency; Balanced keeps smooth motion; Visual Quality enables full frosted glass.",
        &seg,
    ));
    page.append(&preset_card);

    // ── Compositor Visual Effects ────────────────────────────────────────────
    page.append(&section_heading("Compositor Visual Effects"));
    let visual_card = boxed_card();

    append_rows(
        &visual_card,
        vec![
            toggle_row(
                "Enable Compositor Effects Pipeline",
                "Master switch for GPU transitions and shader effects.",
                eff.enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.enabled = active;
                            cfg.effects.preset = EffectsPreset::Custom;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Toggled compositor effects pipeline");
                    }
                },
            ),
            toggle_row(
                "Reduced Motion",
                "Shortens or eliminates motion transitions across windows and workspaces.",
                eff.reduced_motion,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.reduced_motion = active;
                            cfg.effects.preset = EffectsPreset::Custom;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated reduced motion");
                    }
                },
            ),
            toggle_row(
                "Background Blur",
                "Dual-Kawase glass blur behind translucent panels and surfaces.",
                eff.blur_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.blur_enabled = active;
                            cfg.appearance.reduce_blur = !active;
                            cfg.effects.preset = EffectsPreset::Custom;
                        }
                        let app_req = build_appearance_request(&ctrl_c.config.borrow());
                        let _ = ctrl_c.send_ipc(app_req);
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated backdrop blur");
                    }
                },
            ),
            toggle_row(
                "Drop Shadows",
                "GPU elevation shadows beneath floating windows.",
                eff.shadows_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.shadows_enabled = active;
                            cfg.effects.preset = EffectsPreset::Custom;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated drop shadows");
                    }
                },
            ),
            toggle_row(
                "Rounded Window Corners & Borders",
                "Shader-clipped corners and hairline window borders.",
                eff.borders_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.borders_enabled = active;
                            cfg.effects.preset = EffectsPreset::Custom;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated window borders");
                    }
                },
            ),
            slider_row(
                "Corner Radius (px)",
                "Window corner rounding applied by neon-renderer.",
                0.0,
                24.0,
                1.0,
                f64::from(eff.corner_radius),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.corner_radius = val as f32;
                            cfg.effects.preset = EffectsPreset::Custom;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated corner radius");
                    }
                },
            ),
            slider_row(
                "Animation Speed Multiplier",
                "Higher values make transitions snappier (0.5x slow to 2.0x fast).",
                0.5,
                2.0,
                0.1,
                eff.speed_scale,
                1,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.speed_scale = val;
                            cfg.effects.preset = EffectsPreset::Custom;
                        }
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Animation speed: {val:.1}x"));
                    }
                },
            ),
        ],
    );
    page.append(&visual_card);

    // ── Per-Action Animations ────────────────────────────────────────────────
    page.append(&section_heading("Window & Shell Animations"));
    let anim_card = boxed_card();

    append_rows(
        &anim_card,
        vec![
            toggle_row(
                "Window Open Animation",
                "Subtle scale & fade when opening new windows.",
                eff.open_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().effects.open_animation = v;
                        ctrl_c.persist_and_sync(None, "Updated open animation");
                    }
                },
            ),
            toggle_row(
                "Window Close Animation",
                "Smooth fade-out when closing windows.",
                eff.close_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().effects.close_animation = v;
                        ctrl_c.persist_and_sync(None, "Updated close animation");
                    }
                },
            ),
            toggle_row(
                "Minimize & Maximize Transitions",
                "Animate windows shrinking to the dock or expanding.",
                eff.minimize_animation && eff.maximize_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.minimize_animation = v;
                            cfg.effects.maximize_animation = v;
                        }
                        ctrl_c.persist_and_sync(None, "Updated minimize/maximize animations");
                    }
                },
            ),
            toggle_row(
                "Workspace & Overview Transitions",
                "Animate horizontal workspace switching and Mission Control.",
                eff.workspace_animation && eff.overview_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.workspace_animation = v;
                            cfg.effects.overview_animation = v;
                            cfg.desktop.workspace_animations = v;
                        }
                        ctrl_c.persist_and_sync(None, "Updated workspace/overview animations");
                    }
                },
            ),
            toggle_row(
                "Dock Animations",
                "Animate dock auto-hide and icon magnification.",
                eff.dock_animation,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.effects.dock_animation = v;
                            cfg.dock.animations = v;
                        }
                        ctrl_c.persist_and_sync(None, "Updated dock animations");
                    }
                },
            ),
        ],
    );
    page.append(&anim_card);

    scroller
}
