//! NEON DE macOS Tahoe ("Liquid Glass") Visual Design Language — Neon Dark & Neon Light themes,
//! GPU-blurred translucent glass backdrops, specular luminous rims, spring open/close animations,
//! authentic macOS menu bar & Control Center, floating glass dock, and blurred Launchpad.

use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, gdk::Display, prelude::*};
use neon_ipc::{AppearanceInfo, EffectsInfo};

/// Holds the active `CssProvider` so theme and motion updates apply instantaneously with zero restart.
pub struct ShellThemeController {
    provider: CssProvider,
    last_applied: Option<AppearanceInfo>,
    last_effects: Option<EffectsInfo>,
    last_synced_light: Option<bool>,
    last_synced_buttons_right: Option<bool>,
}

impl ShellThemeController {
    pub fn init() -> Self {
        let provider = CssProvider::new();
        let default_appearance = AppearanceInfo::default();
        let default_effects = EffectsInfo::default();
        provider.load_from_string(&build_shell_css(&default_appearance, &default_effects));
        apply_gtk_settings(&default_appearance);
        crate::desktop_apps::register_extra_icon_search_paths();
        if let Some(display) = Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        Self {
            provider,
            last_applied: Some(default_appearance),
            last_effects: Some(default_effects),
            last_synced_light: None,
            last_synced_buttons_right: None,
        }
    }

    pub fn apply_if_changed(&mut self, appearance: &AppearanceInfo, effects: &EffectsInfo) {
        if self.last_applied.as_ref() == Some(appearance)
            && self.last_effects.as_ref() == Some(effects)
        {
            return;
        }
        apply_gtk_settings(appearance);
        self.provider
            .load_from_string(&build_shell_css(appearance, effects));
        self.last_applied = Some(appearance.clone());
        self.last_effects = Some(effects.clone());

        // Propagate Light/Dark theme live to XDG Desktop Portal (`org.freedesktop.appearance`),
        // gsettings, and GTK3/GTK4 settings.ini so Chrome, Electron/Antigravity, Firefox, GTK,
        // and Qt apps follow NEON DE's theme immediately.
        let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
        if self.last_synced_light != Some(is_light) {
            self.last_synced_light = Some(is_light);
            std::thread::spawn(move || {
                neon_system::portals::sync_system_color_scheme(is_light);
            });
        }
    }

    pub fn sync_button_layout_if_changed(&mut self, button_layout: &str) {
        let buttons_right = matches!(
            button_layout.to_lowercase().as_str(),
            "right" | "right_standard"
        );
        if self.last_synced_buttons_right == Some(buttons_right) {
            return;
        }
        self.last_synced_buttons_right = Some(buttons_right);
        if let Some(settings) = gtk4::Settings::default() {
            let val = if buttons_right {
                ":minimize,maximize,close"
            } else {
                "close,minimize,maximize:"
            };
            settings.set_gtk_decoration_layout(Some(val));
        }
        std::thread::spawn(move || {
            neon_system::portals::sync_window_button_layout(buttons_right);
        });
    }
}

fn is_hidpi_display() -> bool {
    Display::default()
        .and_then(|d| d.monitors().item(0))
        .and_then(|obj| obj.downcast::<gtk4::gdk::Monitor>().ok())
        .is_some_and(|m| m.scale_factor() >= 2)
}

fn apply_gtk_settings(appearance: &AppearanceInfo) {
    let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
    let hint_metrics = !is_hidpi_display();
    if let Some(settings) = gtk4::Settings::default() {
        let icon_theme = if is_light {
            "WhiteSur-light"
        } else {
            "WhiteSur-dark"
        };
        settings.set_gtk_icon_theme_name(Some(icon_theme));
        settings.set_gtk_application_prefer_dark_theme(!is_light);
        // Smooth anti-aliased text across GTK 4.16+ (Vulkan/GL/Cairo):
        // - Keep `hintslight` + full AA so curves stay smooth without horizontal distortion.
        // - Enable `gtk-hint-font-metrics = true` on 1x (96 DPI) displays so vertical stems snap
        //   cleanly onto whole pixels instead of smearing unevenly across grayscale subpixels,
        //   and disable it only on >=2x HiDPI panels.
        settings.set_gtk_xft_antialias(1);
        settings.set_gtk_xft_hinting(1);
        settings.set_gtk_xft_hintstyle(Some("hintslight"));
        settings.set_gtk_xft_rgba(Some("rgb"));
        settings.set_gtk_hint_font_metrics(hint_metrics);
        if let Some(pspec) = settings.find_property("gtk-font-rendering")
            && let Some(enum_cls) = gtk4::glib::EnumClass::with_type(pspec.value_type())
            && let Some(val) = enum_cls.to_value_by_nick("manual")
        {
            settings.set_property_from_value("gtk-font-rendering", &val);
        }
        if let Some(ref ct) = appearance.cursor_theme
            && !ct.trim().is_empty()
        {
            settings.set_gtk_cursor_theme_name(Some(ct));
        }
        if let Some(cs) = appearance.cursor_size {
            settings.set_gtk_cursor_theme_size(cs.clamp(16, 48) as i32);
        }
    }
}

pub fn build_shell_css(appearance: &AppearanceInfo, effects: &EffectsInfo) -> String {
    let motion_on = effects.enabled && !effects.reduced_motion;
    let dock_motion_on = motion_on && effects.dock_animation;
    let overview_motion_on = motion_on && effects.overview_animation;
    let speed = effects.speed_scale.clamp(0.5, 2.5);

    let scale_ms = |enabled: bool, base: u32| -> u32 {
        if !enabled {
            0
        } else {
            ((base as f64) / speed).round().clamp(35.0, 1200.0) as u32
        }
    };
    let scale_delay = |enabled: bool, base: u32| -> u32 {
        if !enabled {
            0
        } else {
            ((base as f64) / speed).round().clamp(0.0, 400.0) as u32
        }
    };

    // Official Material 3 Expressive motion curves (m3.material.io/styles/motion/overview/how-it-works)
    let m3_fast_sp = "cubic-bezier(0.42, 1.67, 0.21, 0.90)";
    let m3_def_sp = "cubic-bezier(0.38, 1.21, 0.22, 1.00)";
    let m3_exit_sp = "cubic-bezier(0.38, 1.18, 0.22, 1.00)";
    let m3_fast_fx = "cubic-bezier(0.31, 0.94, 0.34, 1.00)";
    let m3_def_fx = "cubic-bezier(0.34, 0.80, 0.34, 1.00)";
    let m3_slow_fx = "cubic-bezier(0.34, 0.88, 0.34, 1.00)";

    let d_fast_sp = scale_ms(motion_on, 350);
    let d_def_sp = scale_ms(motion_on, 460);
    let d_fast_fx = scale_ms(motion_on, 150);
    let d_def_fx = scale_ms(motion_on, 200);
    let d_slow_fx = scale_ms(motion_on, 240);

    // Dock-specific Material 3 Expressive motion tokens
    let d_dock_sp = scale_ms(dock_motion_on, 350);
    let d_dock_wave = scale_ms(dock_motion_on, 125);
    let d_dock_reveal = scale_ms(dock_motion_on, 440);
    let d_dock_hide = scale_ms(dock_motion_on, 220);
    let d_dock_fx = scale_ms(dock_motion_on, 160);
    let dock_hop_tf = if dock_motion_on {
        "translateY(-14px) scale(1.16)"
    } else {
        "none"
    };
    let dock_hop_v_tf = if dock_motion_on {
        "scale(1.16)"
    } else {
        "none"
    };
    let dock_enter_tf = if dock_motion_on {
        "translateY(18px) scale(0.45)"
    } else {
        "none"
    };
    let dock_enter_op = if dock_motion_on { "0" } else { "1" };

    // Overview / Launchpad / Mission Control tokens
    let d_ov_in = scale_ms(overview_motion_on, 500);
    let d_ov_out = scale_ms(overview_motion_on, 370);
    let d_ov_fast = scale_ms(overview_motion_on, 350);
    let d_ov_fx_in = scale_ms(overview_motion_on, 180);
    let d_ov_fx_def = scale_ms(overview_motion_on, 200);
    let d_ov_fx_out = scale_ms(overview_motion_on, 220);
    let d_ov_bg_out = scale_ms(overview_motion_on, 280);
    let d_ov_bg_del = scale_delay(overview_motion_on, 100);
    let d_ov_fade_del = scale_delay(overview_motion_on, 70);
    let d_ov_lp_out = scale_ms(overview_motion_on, 240);
    let d_desk_in = scale_ms(overview_motion_on, 360);
    let d_desk_rm = scale_ms(overview_motion_on, 240);
    let d_desk_rm_fx = scale_ms(overview_motion_on, 165);

    let d_st_in_1 = scale_delay(overview_motion_on, 24);
    let d_st_in_2 = scale_delay(overview_motion_on, 48);
    let d_st_in_3 = scale_delay(overview_motion_on, 72);
    let d_st_in_4 = scale_delay(overview_motion_on, 96);
    let d_st_in_5 = scale_delay(overview_motion_on, 115);

    let d_st_out_0_f = scale_delay(overview_motion_on, 65);
    let d_st_out_1_t = scale_delay(overview_motion_on, 18);
    let d_st_out_1_f = scale_delay(overview_motion_on, 80);
    let d_st_out_2_t = scale_delay(overview_motion_on, 36);
    let d_st_out_2_f = scale_delay(overview_motion_on, 95);
    let d_st_out_3_t = scale_delay(overview_motion_on, 54);
    let d_st_out_3_f = scale_delay(overview_motion_on, 110);
    let d_st_out_4_t = scale_delay(overview_motion_on, 72);
    let d_st_out_4_f = scale_delay(overview_motion_on, 125);
    let d_st_out_5_t = scale_delay(overview_motion_on, 86);
    let d_st_out_5_f = scale_delay(overview_motion_on, 138);

    // Power Menu staggered wave delays
    let d_pm_1 = scale_delay(motion_on, 28);
    let d_pm_2 = scale_delay(motion_on, 56);
    let d_pm_3 = scale_delay(motion_on, 84);
    let d_pm_4 = scale_delay(motion_on, 112);

    let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
    let high_contrast = appearance.high_contrast;
    let reduce_blur = appearance.reduce_blur || appearance.blur_strength <= 0.01;
    let blur_s = appearance.blur_strength.clamp(0.0, 2.0);
    let accent = &appearance.accent_color;
    let dock_h = appearance.dock_height.clamp(36, 88);
    let panel_a = if high_contrast {
        1.0
    } else if reduce_blur {
        appearance.panel_transparency.clamp(0.0, 1.0)
    } else {
        (appearance.panel_transparency * 0.62).clamp(0.0, 0.92)
    };
    let dock_a = if high_contrast {
        1.0
    } else if reduce_blur {
        appearance.dock_transparency.clamp(0.0, 1.0)
    } else {
        (appearance.dock_transparency * 0.56).clamp(0.0, 0.90)
    };

    let cc_blur_px = (14.0 * blur_s).clamp(2.0, 24.0);
    let lp_blur_px = (24.0 * blur_s).clamp(10.0, 34.0);
    let tahoe_blur_light = if high_contrast || reduce_blur {
        "none".to_string()
    } else {
        format!("blur({cc_blur_px:.0}px) saturate(1.35) brightness(1.10)")
    };
    let launchpad_blur_light = if high_contrast {
        "none".to_string()
    } else if reduce_blur {
        "blur(8px) saturate(1.18) brightness(0.94)".to_string()
    } else {
        format!("blur({lp_blur_px:.0}px) saturate(1.32) brightness(0.92)")
    };
    let tahoe_blur_dark = if high_contrast || reduce_blur {
        "none".to_string()
    } else {
        format!("blur({cc_blur_px:.0}px) saturate(1.40) brightness(0.78)")
    };
    let launchpad_blur_dark = if high_contrast {
        "none".to_string()
    } else if reduce_blur {
        "blur(8px) saturate(1.22) brightness(0.80)".to_string()
    } else {
        format!("blur({lp_blur_px:.0}px) saturate(1.42) brightness(0.78)")
    };

    let (
        panel_bg,
        dock_bg,
        tahoe_glass_bg,
        tahoe_card_bg,
        tahoe_card_hover,
        tahoe_blur_filter,
        launchpad_blur_filter,
        scrim_bg,
        fg_primary,
        fg_muted,
        border_glass,
        border_subtle,
        specular_inset,
        ws_dot_bg,
        slider_trough,
        slider_fill,
        popover_bg,
    ) = if is_light {
        (
        format!("rgba(246, 248, 253, {panel_a:.2})"),
        format!("rgba(246, 249, 255, {dock_a:.2})"),
            if high_contrast {
                "#FFFFFF"
            } else if reduce_blur {
                "rgba(244, 247, 253, 0.94)"
            } else {
                "rgba(244, 247, 253, 0.68)"
            },
            if high_contrast {
                "#F1F5F9"
            } else if reduce_blur {
                "rgba(255, 255, 255, 0.86)"
            } else {
                "rgba(255, 255, 255, 0.56)"
            },
            "rgba(255, 255, 255, 0.76)",
            tahoe_blur_light,
            launchpad_blur_light,
            if high_contrast {
                "rgba(232, 238, 248, 0.94)"
            } else {
                "rgba(14, 18, 30, 0.14)"
            },
            "#0B1220",
            if high_contrast { "#0F172A" } else { "#475569" },
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(255, 255, 255, 0.82)"
            },
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(15, 23, 42, 0.12)"
            },
            "inset 0 1px 0 rgba(255, 255, 255, 0.90), inset 0 -1px 0 rgba(15, 23, 42, 0.06)",
            "rgba(15, 23, 42, 0.28)",
            "rgba(15, 23, 42, 0.14)",
            "#FFFFFF",
            "rgba(246, 248, 253, 0.92)",
        )
    } else {
        (
        format!("rgba(16, 18, 26, {panel_a:.2})"),
        format!("rgba(20, 23, 34, {dock_a:.2})"),
            if high_contrast {
                "#0B1120"
            } else if reduce_blur {
                "rgba(22, 25, 36, 0.94)"
            } else {
                "rgba(24, 28, 42, 0.62)"
            },
            if high_contrast {
                "#111827"
            } else if reduce_blur {
                "rgba(255, 255, 255, 0.09)"
            } else {
                "rgba(255, 255, 255, 0.11)"
            },
            "rgba(255, 255, 255, 0.17)",
            tahoe_blur_dark,
            launchpad_blur_dark,
            if high_contrast {
                "rgba(12, 15, 24, 0.94)"
            } else {
                "rgba(10, 14, 24, 0.22)"
            },
            if high_contrast { "#FFFFFF" } else { "#F8FAFC" },
            if high_contrast { "#E2E8F0" } else { "#CBD5E1" },
            if high_contrast {
                "#F8FAFC"
            } else {
                "rgba(255, 255, 255, 0.30)"
            },
            if high_contrast {
                "#F8FAFC"
            } else {
                "rgba(255, 255, 255, 0.14)"
            },
            "inset 0 1px 0 rgba(255, 255, 255, 0.32), inset 0 -1px 0 rgba(255, 255, 255, 0.06)",
            "rgba(255, 255, 255, 0.36)",
            "rgba(255, 255, 255, 0.18)",
            "rgba(255, 255, 255, 0.94)",
            "rgba(22, 26, 38, 0.90)",
        )
    };

    let (
        cc_glass_bg,
        cc_glass_grad,
        cc_glass_hover,
        cc_glass_border,
        cc_glass_shadow,
        cc_badge_idle,
        cc_slider_trough,
        cc_slider_fill,
        cc_active_circle_bg,
        cc_active_circle_fg,
    ) = if is_light {
        (
            if high_contrast {
                "#FFFFFF"
            } else if reduce_blur {
                "rgba(242, 246, 255, 0.92)"
            } else {
                "rgba(242, 247, 255, 0.68)"
            },
            if high_contrast {
                "none"
            } else {
                "linear-gradient(180deg, rgba(255, 255, 255, 0.56) 0%, rgba(255, 255, 255, 0.18) 100%)"
            },
            "rgba(255, 255, 255, 0.84)",
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(255, 255, 255, 0.88)"
            },
            "0 8px 22px rgba(15, 23, 42, 0.14), inset 0 1px 0 rgba(255, 255, 255, 0.96), inset 0 -1px 0 rgba(15, 23, 42, 0.06)",
            "rgba(15, 23, 42, 0.11)",
            "rgba(15, 23, 42, 0.16)",
            "#0A84FF",
            "rgba(255, 255, 255, 0.97)",
            "#0A84FF",
        )
    } else {
        (
            if high_contrast {
                "#0F172A"
            } else if reduce_blur {
                "rgba(22, 28, 42, 0.86)"
            } else {
                "rgba(22, 32, 50, 0.54)"
            },
            if high_contrast {
                "none"
            } else {
                "linear-gradient(180deg, rgba(255, 255, 255, 0.16) 0%, rgba(255, 255, 255, 0.04) 55%, rgba(255, 255, 255, 0.10) 100%)"
            },
            "rgba(255, 255, 255, 0.19)",
            if high_contrast {
                "#F8FAFC"
            } else {
                "rgba(255, 255, 255, 0.34)"
            },
            "0 8px 22px rgba(0, 0, 0, 0.28), inset 0 1px 0 rgba(255, 255, 255, 0.40), inset 0 -1px 0 rgba(255, 255, 255, 0.10)",
            "rgba(255, 255, 255, 0.18)",
            "rgba(255, 255, 255, 0.24)",
            "#FFFFFF",
            "rgba(255, 255, 255, 0.94)",
            "#0A84FF",
        )
    };

    let (
        qs_panel_bg,
        qs_inner_bg,
        qs_pill_bg,
        qs_pill_hover,
        qs_pill_active_bg,
        qs_pill_active_fg,
        pm_scrim_bg,
    ) = if is_light {
        (
            if high_contrast { "#FFFFFF" } else { "rgba(244, 248, 255, 0.72)" },
            if high_contrast { "#F1F5F9" } else { "rgba(255, 255, 255, 0.58)" },
            if high_contrast { "#E2E8F0" } else { "rgba(255, 255, 255, 0.62)" },
            "rgba(255, 255, 255, 0.84)",
            accent.as_str(),
            "#FFFFFF",
            if high_contrast { "rgba(232, 238, 248, 0.90)" } else { "rgba(218, 226, 240, 0.42)" },
        )
    } else {
        (
            if high_contrast { "#0B1120" } else { "rgba(15, 20, 32, 0.62)" },
            if high_contrast { "#111827" } else { "rgba(255, 255, 255, 0.08)" },
            if high_contrast { "#1E293B" } else { "rgba(255, 255, 255, 0.11)" },
            "rgba(255, 255, 255, 0.18)",
            accent.as_str(),
            "#FFFFFF",
            if high_contrast { "rgba(6, 9, 16, 0.90)" } else { "rgba(7, 10, 18, 0.44)" },
        )
    };

    let (ctx_bg, ctx_border, ctx_shadow, ctx_sub_open_bg, ctx_dialog_bg, ctx_input_bg) = if is_light
    {
        (
            if high_contrast {
                "#FFFFFF"
            } else if reduce_blur {
                "rgba(248, 250, 254, 0.96)"
            } else {
                "rgba(246, 248, 253, 0.74)"
            },
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(15, 23, 42, 0.14)"
            },
            "0 8px 20px rgba(15, 23, 42, 0.18), 0 2px 5px rgba(15, 23, 42, 0.10), inset 0 1px 0 rgba(255, 255, 255, 0.92)",
            "rgba(15, 23, 42, 0.08)",
            if high_contrast { "#FFFFFF" } else { "rgba(248, 250, 254, 0.95)" },
            "rgba(15, 23, 42, 0.06)",
        )
    } else {
        (
            if high_contrast {
                "#0B1120"
            } else if reduce_blur {
                "rgba(24, 28, 40, 0.95)"
            } else {
                "rgba(26, 30, 44, 0.70)"
            },
            if high_contrast {
                "#F8FAFC"
            } else {
                "rgba(255, 255, 255, 0.18)"
            },
            "0 8px 20px rgba(0, 0, 0, 0.44), 0 2px 5px rgba(0, 0, 0, 0.26), inset 0 1px 0 rgba(255, 255, 255, 0.16)",
            "rgba(255, 255, 255, 0.11)",
            if high_contrast { "#0B1120" } else { "rgba(24, 28, 40, 0.94)" },
            "rgba(255, 255, 255, 0.08)",
        )
    };

    let (
        ws_scrim_bg,
        ws_tray_bg,
        ws_tray_grad,
        ws_tray_shadow,
        ws_empty_bg,
        ws_thumb_bg,
        ws_thumb_border,
        ws_win_frame_bg,
        ws_win_border,
        ws_win_shadow,
        ws_hover_shadow,
        ws_add_bg,
        ws_add_hover,
    ) = if is_light {
        (
            if high_contrast {
                "rgba(232, 238, 248, 0.94)"
            } else {
                "rgba(236, 242, 255, 0.12)"
            },
            if high_contrast {
                "#FFFFFF"
            } else if reduce_blur {
                "rgba(244, 248, 255, 0.92)"
            } else {
                "rgba(244, 248, 255, 0.42)"
            },
            if high_contrast {
                "none"
            } else {
                "linear-gradient(180deg, rgba(255, 255, 255, 0.30) 0%, rgba(255, 255, 255, 0.06) 100%)"
            },
            "0 10px 28px rgba(15, 23, 42, 0.16), inset 0 1px 0 rgba(255, 255, 255, 0.90), inset 0 -1px 0 rgba(15, 23, 42, 0.06)",
            if high_contrast {
                "#FFFFFF"
            } else if reduce_blur {
                "rgba(244, 248, 255, 0.94)"
            } else {
                "rgba(244, 248, 255, 0.54)"
            },
            "rgba(236, 241, 250, 0.86)",
            "rgba(15, 23, 42, 0.14)",
            "rgba(246, 249, 255, 0.82)",
            "rgba(255, 255, 255, 0.80)",
            "0 16px 38px rgba(15, 23, 42, 0.22), 0 3px 10px rgba(15, 23, 42, 0.12)",
            "0 22px 50px rgba(15, 23, 42, 0.28)",
            if high_contrast { "#E2E8F0" } else { "rgba(255, 255, 255, 0.38)" },
            "rgba(255, 255, 255, 0.64)",
        )
    } else {
        (
            if high_contrast {
                "rgba(10, 14, 24, 0.94)"
            } else {
                "rgba(8, 12, 22, 0.24)"
            },
            if high_contrast {
                "#0B1120"
            } else if reduce_blur {
                "rgba(18, 22, 34, 0.92)"
            } else {
                "rgba(18, 22, 34, 0.48)"
            },
            if high_contrast {
                "none"
            } else {
                "linear-gradient(180deg, rgba(255, 255, 255, 0.13) 0%, rgba(255, 255, 255, 0.03) 55%, rgba(255, 255, 255, 0.08) 100%)"
            },
            "0 12px 32px rgba(0, 0, 0, 0.38), inset 0 1px 0 rgba(255, 255, 255, 0.34), inset 0 -1px 0 rgba(255, 255, 255, 0.08)",
            if high_contrast {
                "#0B1120"
            } else if reduce_blur {
                "rgba(18, 22, 34, 0.92)"
            } else {
                "rgba(16, 20, 32, 0.56)"
            },
            "rgba(14, 18, 30, 0.72)",
            "rgba(255, 255, 255, 0.20)",
            "rgba(16, 20, 32, 0.82)",
            "rgba(255, 255, 255, 0.22)",
            "0 18px 44px rgba(0, 0, 0, 0.44), 0 4px 12px rgba(0, 0, 0, 0.26)",
            "0 24px 54px rgba(0, 0, 0, 0.54)",
            if high_contrast { "#1E293B" } else { "rgba(255, 255, 255, 0.10)" },
            "rgba(255, 255, 255, 0.19)",
        )
    };

    let _ = (tahoe_glass_bg, tahoe_blur_filter, launchpad_blur_filter, slider_fill);

    format!(
        r#"
* {{
    font-family: "Inter", "SF Pro Text", "Adwaita Sans", "Noto Sans", "Ubuntu Sans", "Roboto", sans-serif;
}}

window.neon-panel-window,
window.neon-dock-window,
window.neon-overview-window,
window.neon-quicksettings-window,
window.neon-workspaces-window,
window.neon-powermenu-window,
window.neon-edge-sensor-window,
window.neon-toast-window,
window.neon-screenshot-window,
window.neon-contextmenu-window,
window.neon-ctx-backdrop-window,
window.neon-dialog-window,
window.neon-polkit-window,
window.neon-notification-center-window,
window.neon-dynamic-island-window,
window.neon-dock-tooltip-window {{
    background-color: transparent;
    background-image: none;
    box-shadow: none;
    border: none;
    outline: none;
}}

/* Almost-zero alpha guarantees GTK commits an interactive input buffer while staying 100% invisible */
.neon-edge-sensor-strip {{
    background-color: rgba(0, 0, 0, 0.004);
    min-height: 3px;
}}

window.neon-wallpaper-window {{
    background-image: linear-gradient(140deg, #0b1020 0%, #15213d 50%, #1e1b3a 100%);
}}

/* ===================== TOP MENU BAR (macOS Tahoe Liquid Glass) ===================== */

.neon-top-panel {{
    background-color: {panel_bg};
    border-bottom: 1px solid {border_subtle};
    box-shadow: inset 0 -1px 0 rgba(255, 255, 255, 0.05);
    padding: 0 12px;
    min-height: 30px;
    color: {fg_primary};
    font-size: 13px;
    transition: background-color {d_def_fx}ms {m3_def_fx},
                border-color {d_def_fx}ms {m3_def_fx},
                color {d_def_fx}ms {m3_def_fx},
                transform {d_def_sp}ms {m3_def_sp},
                opacity {d_def_fx}ms {m3_def_fx};
}}

.neon-top-panel.panel-hidden {{
    transform: translateY(-34px);
    opacity: 0;
    transition: transform {d_slow_fx}ms {m3_slow_fx}, opacity {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-rec-pill {{
    background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.16) 0%, rgba(255, 255, 255, 0.04) 100%);
    background-color: rgba(239, 68, 68, 0.80);
    color: #FFFFFF;
    border: 1px solid rgba(254, 202, 202, 0.48);
    box-shadow: 0 3px 12px rgba(220, 38, 38, 0.34);
    border-radius: 12px;
    padding: 2px 11px;
    min-height: 22px;
    margin-right: 6px;
    font-size: 11px;
    font-weight: 700;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-rec-pill:hover {{
    background-color: rgba(239, 68, 68, 0.92);
    border-color: rgba(255, 255, 255, 0.62);
    transform: scale(1.04);
}}

button.neon-rec-pill:active {{
    transform: scale(0.93);
}}

.neon-rec-dot {{
    background-color: #FFFFFF;
    box-shadow: 0 0 6px rgba(255, 255, 255, 0.85);
    border-radius: 999px;
    min-width: 7px;
    min-height: 7px;
}}

.neon-rec-label {{
    color: #FFFFFF;
    font-size: 11px;
    font-weight: 700;
}}

.neon-top-panel.drawer-open {{
    background-color: transparent;
    border-bottom-color: transparent;
    box-shadow: none;
    color: #FFFFFF;
}}

.neon-top-panel.drawer-open button.neon-brand-button,
.neon-top-panel.drawer-open button.neon-ws-taskview-btn,
.neon-top-panel.drawer-open button.neon-clock-btn,
.neon-top-panel.drawer-open button.neon-tray-btn,
.neon-top-panel.drawer-open .neon-app-title {{
    color: #FFFFFF;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.45);
}}

button.neon-ws-taskview-btn {{
    background-image: none;
    background-color: transparent;
    color: {fg_primary};
    border: 1px solid transparent;
    box-shadow: none;
    border-radius: 8px;
    padding: 3px 8px;
    min-width: 30px;
    min-height: 24px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-ws-taskview-btn:hover {{
    background-color: {tahoe_card_hover};
    border-color: {border_subtle};
}}

button.neon-ws-taskview-btn.active {{
    background-color: alpha({accent}, 0.28);
    border-color: alpha({accent}, 0.52);
}}

button.neon-ws-taskview-btn:active {{
    transform: scale(0.92);
}}

button.neon-brand-button {{
    background-image: none;
    background-color: transparent;
    color: {fg_primary};
    border: none;
    box-shadow: none;
    border-radius: 7px;
    padding: 2px 9px;
    min-height: 22px;
    font-weight: 700;
    letter-spacing: 0.4px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-brand-button:hover,
button.neon-brand-button.active {{
    background-color: {tahoe_card_hover};
}}

button.neon-brand-button:active {{
    transform: scale(0.94);
}}

.neon-app-title {{
    color: {fg_primary};
    font-weight: 600;
    font-size: 13px;
    margin-left: 4px;
}}

button.neon-ws-dot {{
    background-image: none;
    background-color: {ws_dot_bg};
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 7px;
    min-height: 7px;
    padding: 0;
    margin: 0 2px;
    transition: min-width {d_fast_sp}ms {m3_fast_sp},
                transform {d_fast_sp}ms {m3_fast_sp},
                background-color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-ws-dot:hover {{
    background-color: {fg_primary};
    min-width: 12px;
}}

button.neon-ws-dot:active {{
    transform: scale(0.88);
}}

button.neon-ws-dot.occupied {{
    background-color: {fg_muted};
}}

button.neon-ws-dot.active {{
    background-color: {fg_primary};
    min-width: 22px;
}}

button.neon-clock-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 7px;
    padding: 2px 10px;
    min-height: 22px;
    color: {fg_primary};
    font-weight: 600;
    font-size: 13px;
    letter-spacing: 0.2px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-clock-btn:hover {{
    background-color: {tahoe_card_hover};
}}

button.neon-clock-btn:active {{
    transform: scale(0.95);
}}

button.neon-tray-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 7px;
    padding: 2px 7px;
    min-height: 22px;
    color: {fg_primary};
    font-size: 12px;
    font-weight: 500;
    transition: background-color {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-tray-btn:hover {{
    background-color: {tahoe_card_hover};
}}

button.neon-tray-btn:active {{
    transform: scale(0.93);
}}

button.neon-tray-btn.active {{
    background-color: alpha({accent}, 0.28);
    color: {fg_primary};
}}

.neon-tray-apps {{
    margin-right: 2px;
}}

button.neon-tray-app-btn {{
    padding: 2px 5px;
    min-width: 24px;
    min-height: 22px;
    border-radius: 6px;
}}

button.neon-status-icon-btn {{
    padding: 2px 6px;
    min-width: 24px;
    min-height: 22px;
    border-radius: 7px;
}}

.neon-tray-sep {{
    background-color: {border_subtle};
    min-width: 1px;
    min-height: 14px;
    margin: 0 4px;
}}

button.neon-cc-toggle-btn {{
    padding: 2px 10px;
    border-radius: 999px;
}}

button.neon-cc-toggle-btn.active {{
    background-color: {tahoe_card_hover};
    box-shadow: inset 0 0 0 1px {border_glass};
}}

.neon-notif-dot {{
    background-color: {accent};
    border-radius: 999px;
    min-width: 6px;
    min-height: 6px;
    margin-left: 2px;
}}

/* ===================== FLOATING LIQUID GLASS DOCK (macOS Tahoe) ===================== */

.neon-dock-wrapper {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 16px 24px 5px 24px;
    transition: transform {d_dock_reveal}ms {m3_def_sp},
                opacity {d_dock_fx}ms {m3_def_fx};
}}

.neon-dock-wrapper.dock-hidden {{
    transform: translateY(96px) scale(0.96);
    opacity: 0;
    transition: transform {d_dock_hide}ms {m3_slow_fx},
                opacity {d_dock_fx}ms {m3_fast_fx};
}}

.neon-dock-wrapper.combined-taskbar {{
    padding: 0;
}}

.neon-taskbar-combined {{
    background-color: {panel_bg};
    border-top: 1px solid {border_glass};
    box-shadow: 0 -4px 18px rgba(0, 0, 0, 0.22), inset 0 1px 0 rgba(255, 255, 255, 0.24);
    padding: 0 16px;
    min-height: {dock_h}px;
    color: {fg_primary};
    transition: background-color {d_def_fx}ms {m3_def_fx},
                border-color {d_def_fx}ms {m3_def_fx};
}}

.neon-taskbar-combined.drawer-open {{
    background-color: rgba(255, 255, 255, 0.10);
    border-top-color: rgba(255, 255, 255, 0.20);
    color: #FFFFFF;
}}

.neon-taskbar-combined.drawer-open button.neon-tray-btn,
.neon-taskbar-combined.drawer-open button.neon-clock-btn,
.neon-taskbar-combined.drawer-open .neon-taskbar-clock-time {{
    color: #FFFFFF;
}}

.neon-taskbar-combined.drawer-open .neon-taskbar-clock-date {{
    color: rgba(248, 250, 252, 0.78);
}}

.neon-taskbar-combined button.neon-ws-taskview-btn {{
    min-width: 34px;
    min-height: 30px;
    padding: 4px 9px;
    border-radius: 9px;
}}

.neon-taskbar-combined button.neon-clock-btn {{
    padding: 2px 10px;
    min-height: 28px;
}}

.neon-taskbar-clock-stack {{
    padding: 0;
    margin: 0;
}}

.neon-taskbar-clock-time {{
    font-size: 12px;
    font-weight: 700;
    color: {fg_primary};
    font-feature-settings: "tnum";
}}

.neon-taskbar-clock-date {{
    font-size: 11px;
    font-weight: 500;
    color: {fg_muted};
    font-feature-settings: "tnum";
}}

.neon-taskbar-left,
.neon-taskbar-right {{
    color: {fg_primary};
}}

.neon-dock-container {{
    background-color: {dock_bg};
    border: 1px solid {border_glass};
    border-radius: 20px;
    padding: 6px 12px;
    min-height: {dock_h}px;
    box-shadow: 0 3px 10px rgba(0, 0, 0, 0.28), {specular_inset};
    transition: background-color {d_def_fx}ms {m3_def_fx},
                border-color {d_def_fx}ms {m3_def_fx},
                transform {d_dock_reveal}ms {m3_def_sp},
                opacity {d_dock_fx}ms {m3_def_fx};
}}

.neon-dock-container.dock-hidden {{
    transform: translateY(96px) scale(0.92);
    opacity: 0;
    transition: transform {d_dock_hide}ms {m3_slow_fx},
                opacity {d_dock_fx}ms {m3_fast_fx};
}}

.neon-dock-container.drawer-open {{
    background-color: rgba(255, 255, 255, 0.11);
    border-color: rgba(255, 255, 255, 0.20);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.18), inset 0 1px 0 rgba(255, 255, 255, 0.22);
}}

.neon-dock-container.drawer-open .neon-dock-separator {{
    background-color: rgba(255, 255, 255, 0.26);
}}

/* Declared AFTER .drawer-open so Combined Bottom Taskbar never draws an inner pill during App Drawer */
.neon-dock-container.in-taskbar,
.neon-dock-container.in-taskbar.drawer-open {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 0 6px;
    border-radius: 0;
    min-height: 0;
}}

.neon-dock-wrapper.dock-vertical {{
    padding: 16px 7px;
}}

.neon-dock-container.dock-vertical {{
    padding: 8px 6px;
    min-width: {dock_h}px;
    min-height: 0;
}}

.neon-dock-container.dock-vertical .neon-dock-separator {{
    min-width: 24px;
    min-height: 1px;
    margin: 5px 0;
}}

button.neon-dock-item {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 12px;
    padding: 0;
    margin: 0;
    min-width: 24px;
    min-height: 24px;
    color: {fg_primary};
    transition: transform {d_dock_sp}ms {m3_fast_sp},
                margin-left {d_dock_wave}ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                margin-right {d_dock_wave}ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                margin-top {d_dock_wave}ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                margin-bottom {d_dock_wave}ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                opacity {d_dock_fx}ms {m3_fast_fx},
                background-color {d_dock_fx}ms {m3_fast_fx};
}}

.neon-dock-icon-slot {{
    padding: 6px 5px;
    transition: transform {d_dock_wave}ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-dock-container.in-taskbar .neon-dock-icon-slot {{
    padding: 5px 5px;
}}

.neon-dock-container.dock-vertical .neon-dock-icon-slot {{
    padding: 5px 6px;
}}

button.neon-dock-item.dock-icon-enter {{
    opacity: {dock_enter_op};
    transform: {dock_enter_tf};
    transition: none;
}}

button.neon-dock-item.dock-launch-hop {{
    transform: {dock_hop_tf};
}}

.neon-dock-container.dock-vertical button.neon-dock-item.dock-launch-hop {{
    transform: {dock_hop_v_tf};
}}

.neon-dock-container.dock-compact {{
    border-radius: 15px;
    padding: 4px 8px;
}}

.neon-dock-container.dock-compact button.neon-dock-item {{
    border-radius: 9px;
    padding: 0;
    min-width: 22px;
    min-height: 22px;
}}

.neon-dock-container.dock-compact .neon-dock-icon-slot {{
    padding: 4px 3px;
}}

.neon-dock-container.dock-compact .neon-dock-dot {{
    min-width: 4px;
    min-height: 3px;
    margin-bottom: 1px;
}}

.neon-dock-container.dock-compact .neon-dock-dot.focused {{
    min-width: 10px;
}}

.neon-dock-container.dock-compact .neon-dock-separator {{
    min-height: 18px;
    margin: 0 4px;
}}

button.neon-dock-item:hover {{
    background-color: rgba(255, 255, 255, 0.11);
}}

/* macOS Gaussian Parabolic Wave Magnification — Floating Horizontal Dock */
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-1 {{ margin-left: 1px; margin-right: 1px; }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-2 {{ margin-left: 2px; margin-right: 2px; }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-3 {{ margin-left: 2px; margin-right: 2px; }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-4 {{ margin-left: 3px; margin-right: 3px; }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-5 {{ margin-left: 4px; margin-right: 4px; }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-6 {{ margin-left: 5px; margin-right: 5px; }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-7 {{ margin-left: 5px; margin-right: 5px; }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-8 {{ margin-left: 6px; margin-right: 6px; }}

.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-1 .neon-dock-icon-slot {{ transform: translateY(-1px) scale(1.05); }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-2 .neon-dock-icon-slot {{ transform: translateY(-2px) scale(1.10); }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-3 .neon-dock-icon-slot {{ transform: translateY(-3px) scale(1.15); }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-4 .neon-dock-icon-slot {{ transform: translateY(-4px) scale(1.20); }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-5 .neon-dock-icon-slot {{ transform: translateY(-5px) scale(1.25); }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-6 .neon-dock-icon-slot {{ transform: translateY(-6px) scale(1.29); }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-7 .neon-dock-icon-slot {{ transform: translateY(-7px) scale(1.33); }}
.neon-dock-container.dock-magnify button.neon-dock-item.dock-mag-8 .neon-dock-icon-slot {{ transform: translateY(-8px) scale(1.36); }}

/* macOS Gaussian Parabolic Wave Magnification — Combined Bottom Taskbar (zero edge clipping) */
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-1 {{ margin-left: 1px; margin-right: 1px; }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-2 {{ margin-left: 2px; margin-right: 2px; }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-3 {{ margin-left: 2px; margin-right: 2px; }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-4 {{ margin-left: 3px; margin-right: 3px; }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-5 {{ margin-left: 4px; margin-right: 4px; }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-6 {{ margin-left: 5px; margin-right: 5px; }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-7 {{ margin-left: 5px; margin-right: 5px; }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-8 {{ margin-left: 6px; margin-right: 6px; }}

.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-1 .neon-dock-icon-slot {{ transform: scale(1.03); }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-2 .neon-dock-icon-slot {{ transform: scale(1.06); }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-3 .neon-dock-icon-slot {{ transform: scale(1.09); }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-4 .neon-dock-icon-slot {{ transform: translateY(-0.5px) scale(1.12); }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-5 .neon-dock-icon-slot {{ transform: translateY(-0.5px) scale(1.15); }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-6 .neon-dock-icon-slot {{ transform: translateY(-1px) scale(1.18); }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-7 .neon-dock-icon-slot {{ transform: translateY(-1px) scale(1.20); }}
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item.dock-mag-8 .neon-dock-icon-slot {{ transform: translateY(-1px) scale(1.22); }}

/* macOS Gaussian Parabolic Wave Magnification — Vertical Left/Right Dock */
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-1 {{ margin: 1px 0; }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-2 {{ margin: 2px 0; }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-3 {{ margin: 2px 0; }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-4 {{ margin: 3px 0; }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-5 {{ margin: 4px 0; }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-6 {{ margin: 5px 0; }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-7 {{ margin: 5px 0; }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-8 {{ margin: 6px 0; }}

.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-1 .neon-dock-icon-slot {{ transform: scale(1.05); }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-2 .neon-dock-icon-slot {{ transform: scale(1.10); }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-3 .neon-dock-icon-slot {{ transform: scale(1.14); }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-4 .neon-dock-icon-slot {{ transform: scale(1.19); }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-5 .neon-dock-icon-slot {{ transform: scale(1.23); }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-6 .neon-dock-icon-slot {{ transform: scale(1.27); }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-7 .neon-dock-icon-slot {{ transform: scale(1.31); }}
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item.dock-mag-8 .neon-dock-icon-slot {{ transform: scale(1.34); }}

button.neon-dock-item:active .neon-dock-icon-slot,
.neon-dock-container.dock-magnify button.neon-dock-item:active .neon-dock-icon-slot,
.neon-dock-container.in-taskbar.dock-magnify button.neon-dock-item:active .neon-dock-icon-slot,
.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item:active .neon-dock-icon-slot {{
    transform: scale(0.88);
}}

button.neon-dock-item.focused {{
    background-color: transparent;
}}

.neon-dock-dot {{
    background-color: transparent;
    border-radius: 999px;
    min-width: 5px;
    min-height: 4px;
    margin-bottom: 1px;
    transition: min-width {d_dock_sp}ms {m3_fast_sp},
                background-color {d_dock_fx}ms {m3_fast_fx};
}}

.neon-dock-dot.running {{
    background-color: {fg_muted};
}}

.neon-dock-dot.focused {{
    background-color: {accent};
    min-width: 14px;
}}

.neon-dock-separator {{
    background-color: {border_glass};
    min-width: 1px;
    min-height: 24px;
    margin: 0 5px;
}}

/* Floating Liquid Glass Above-Icon App Title Pill (macOS Dock Tooltip) */
.neon-dock-tooltip-wrap {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 8px 14px 10px 14px;
}}

.neon-dock-tooltip-pill {{
    background-color: {popover_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    border-radius: 14px;
    padding: 5px 14px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.32);
}}

.neon-dock-tooltip-label {{
    color: {fg_primary};
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.15px;
}}

/* ===================== LIQUID GLASS FLOATING CONTROL CENTER ===================== */

.neon-cc-backdrop {{
    /* Non-zero alpha (1/255, visually imperceptible) forces GTK4/GSK Vulkan & GL to
       always paint and attach a full-screen wl_buffer so click-outside dismissal works. */
    background-color: rgba(0, 0, 0, 0.005);
    background-image: none;
    border: none;
    box-shadow: none;
}}

.neon-cc-surface {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 8px 10px 22px 22px;
    transition: opacity 220ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                transform 260ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-cc-surface.cc-closed {{
    opacity: 0;
    transform: translateY(-18px) scale(0.92);
}}

.neon-cc-surface.cc-open {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
}}

/* Shared floating Liquid Glass lens surface */
.neon-cc-glass {{
    background-color: {cc_glass_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    box-shadow: {cc_glass_shadow};
    color: {fg_primary};
}}

/* 2×1 Stadium Pills (Wi-Fi & Focus: exact 152 × 70 px border-box = 126+24+2 × 68+2) */
button.neon-cc-stadium-pill {{
    background-color: {cc_glass_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    box-shadow: {cc_glass_shadow};
    border-radius: 999px;
    min-width: 126px;
    min-height: 68px;
    padding: 0 12px 0 12px;
    color: {fg_primary};
    transition: background-color 160ms ease, border-color 160ms ease, transform 150ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

button.neon-cc-stadium-pill:hover {{
    background-color: {cc_glass_hover};
    transform: scale(1.02);
}}

button.neon-cc-stadium-pill:active {{
    transform: scale(0.97);
}}

.neon-cc-pill-badge {{
    background-color: {cc_badge_idle};
    color: {fg_primary};
    border-radius: 999px;
    min-width: 42px;
    min-height: 42px;
    padding: 0;
    transition: background-color 180ms ease, color 180ms ease;
}}

button.neon-cc-stadium-pill.wifi-style.active .neon-cc-pill-badge {{
    background-color: {cc_active_circle_bg};
    color: {cc_active_circle_fg};
    box-shadow: 0 3px 10px rgba(0, 0, 0, 0.16);
}}

button.neon-cc-stadium-pill.active:not(.wifi-style) {{
    border-color: alpha({accent}, 0.65);
    background-color: alpha({accent}, 0.28);
}}

button.neon-cc-stadium-pill.active:not(.wifi-style) .neon-cc-pill-badge {{
    background-color: {cc_active_circle_bg};
    color: #5E5CE6;
    box-shadow: 0 3px 10px rgba(0, 0, 0, 0.16);
}}

/* 1×1 Floating Circular Buttons (68 × 68 px) */
button.neon-cc-circle-btn {{
    background-color: {cc_glass_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    box-shadow: {cc_glass_shadow};
    border-radius: 999px;
    min-width: 68px;
    min-height: 68px;
    padding: 0;
    color: {fg_primary};
    transition: background-color 180ms ease, color 180ms ease, border-color 180ms ease, transform 150ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

button.neon-cc-circle-btn:hover {{
    background-color: {cc_glass_hover};
    transform: scale(1.04);
}}

button.neon-cc-circle-btn:active {{
    transform: scale(0.95);
}}

button.neon-cc-circle-btn.active {{
    background-image: none;
    background-color: {cc_active_circle_bg};
    color: {cc_active_circle_fg};
    border-color: rgba(255, 255, 255, 0.96);
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.22);
}}

button.neon-cc-circle-btn.active:hover {{
    background-color: #FFFFFF;
}}

/* 2×2 Now Playing Liquid Glass Card (exact 152 × 152 px border-box = 124+26+2 × 128+22+2) */
.neon-cc-media-card {{
    border-radius: 26px;
    padding: 12px 13px 10px 13px;
    min-width: 124px;
    min-height: 128px;
}}

.neon-cc-media-art {{
    border-radius: 12px;
    border: 1px solid rgba(255, 255, 255, 0.24);
    box-shadow: 0 3px 10px rgba(0, 0, 0, 0.24);
}}

.neon-cc-media-art-placeholder {{
    background-image: linear-gradient(135deg, alpha({accent}, 0.55) 0%, rgba(99, 102, 241, 0.65) 100%);
    color: #FFFFFF;
}}

button.neon-cc-media-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    padding: 0;
    min-width: 30px;
    min-height: 30px;
    color: {fg_primary};
    transition: background-color 140ms ease, transform 130ms ease;
}}

button.neon-cc-media-btn:hover {{
    background-color: {cc_badge_idle};
    transform: scale(1.08);
}}

button.neon-cc-media-btn:active {{
    transform: scale(0.92);
}}

button.neon-cc-media-btn:disabled {{
    opacity: 0.42;
}}

/* Full-width Display & Sound Liquid Glass Cards (exact 316 px border-box = 282+32+2) */
.neon-cc-slider-card {{
    border-radius: 26px;
    padding: 11px 16px 12px 16px;
    min-width: 282px;
}}

.neon-cc-title {{
    color: {fg_primary};
    font-weight: 700;
    font-size: 13px;
    letter-spacing: 0.1px;
}}

.neon-cc-sub {{
    color: {fg_muted};
    font-weight: 500;
    font-size: 11px;
}}

.neon-qs-title {{
    color: {fg_primary};
    font-weight: 600;
    font-size: 13px;
}}

.neon-qs-muted {{
    color: {fg_muted};
    font-size: 11px;
}}

.neon-cc-slider-icon {{
    color: {fg_primary};
}}

.neon-cc-slider-icon-muted {{
    color: {fg_primary};
    opacity: 0.78;
}}

button.neon-cc-inline-icon-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 24px;
    min-height: 24px;
    padding: 0;
    margin: 0;
    color: {fg_primary};
    transition: background-color 140ms ease, opacity 140ms ease;
}}

button.neon-cc-inline-icon-btn:hover {{
    background-color: {cc_badge_idle};
}}

button.neon-cc-inline-icon-btn.muted {{
    color: #F87171;
}}

.neon-cc-slider-divider {{
    background-color: {border_subtle};
    min-height: 1px;
    margin: 1px 0;
}}

button.neon-cc-output-circle {{
    background-image: none;
    background-color: {cc_badge_idle};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    padding: 0;
    margin: 0 0 0 2px;
    color: {fg_primary};
    transition: background-color 150ms ease, transform 140ms ease;
}}

button.neon-cc-output-circle:hover {{
    background-color: {cc_glass_hover};
    transform: scale(1.06);
}}

/* Sleek Liquid Glass Sliders (Thin translucent track + crisp white fill + round white knob) */
scale.neon-cc-slider {{
    padding: 7px 0;
    margin: 0;
}}

scale.neon-cc-slider > trough {{
    background-image: none;
    background-color: {cc_slider_trough};
    border: none;
    border-radius: 999px;
    min-height: 6px;
    box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.16);
}}

scale.neon-cc-slider > trough > highlight {{
    background-image: none;
    background-color: {cc_slider_fill};
    border: none;
    margin: 0;
    border-radius: 999px;
}}

scale.neon-cc-slider > trough > slider,
scale.neon-cc-slider > trough > slider:hover,
scale.neon-cc-slider > trough > slider:active {{
    background-image: none;
    background-color: #FFFFFF;
    border: none;
    outline: none;
    border-radius: 9px;
    min-width: 18px;
    min-height: 18px;
    margin: -6px 0;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.30);
    transition: transform {d_fast_sp}ms {m3_fast_sp};
}}

scale.neon-cc-slider:hover > trough > slider {{
    transform: scale(1.14);
}}

scale.neon-cc-slider:active > trough > slider {{
    transform: scale(1.22);
}}

/* Compact System Power Glass Pill (exact 316 px border-box = 302+12+2) */
.neon-cc-power-pill {{
    border-radius: 26px;
    padding: 5px 6px;
    min-width: 302px;
}}

button.neon-cc-power-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 20px;
    padding: 6px 4px;
    min-height: 40px;
    color: {fg_primary};
    transition: background-color 150ms ease, color 150ms ease, transform 130ms ease;
}}

button.neon-cc-power-btn:hover {{
    background-color: {cc_badge_idle};
    transform: translateY(-1px);
}}

button.neon-cc-power-btn.danger:hover {{
    background-color: alpha(#EF4444, 0.26);
    color: #FCA5A5;
}}

button.neon-cc-power-btn:active {{
    transform: scale(0.95);
}}

.neon-cc-power-lbl {{
    font-size: 10px;
    font-weight: 600;
    color: {fg_muted};
}}

.neon-cc-confirm-card {{
    border-radius: 20px;
    border-color: alpha({accent}, 0.60);
    padding: 9px 13px;
}}

button.neon-cc-small-btn {{
    background-image: none;
    background-color: {cc_badge_idle};
    color: {fg_primary};
    border: none;
    border-radius: 999px;
    padding: 4px 11px;
    font-size: 11px;
    font-weight: 600;
    transition: background-color 140ms ease;
}}

button.neon-cc-small-btn:hover {{
    background-color: {cc_glass_hover};
}}

button.neon-power-confirm-btn {{
    background-image: none;
    background-color: {accent};
    color: #FFFFFF;
    border: none;
    border-radius: 999px;
    padding: 4px 12px;
    font-size: 11px;
    font-weight: 700;
}}

button.neon-power-confirm-btn.danger {{
    background-color: #DC2626;
}}

.neon-cc-notif-item {{
    background-color: {cc_badge_idle};
    border-radius: 14px;
    padding: 7px 10px;
}}

/* Centered bottom Liquid Glass capsule pill ("Open Settings") */
button.neon-cc-bottom-pill {{
    background-color: {cc_glass_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    box-shadow: {cc_glass_shadow};
    border-radius: 999px;
    padding: 6px 20px;
    min-height: 28px;
    color: {fg_primary};
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.1px;
    transition: background-color 150ms ease, transform 140ms ease;
}}

button.neon-cc-bottom-pill:hover {{
    background-color: {cc_glass_hover};
    transform: scale(1.03);
}}

button.neon-cc-bottom-pill:active {{
    transform: scale(0.96);
}}

/* Segmented Control (Power Profiles & Launchpad/Mission Control) */
.neon-segment-track {{
    background-color: {slider_trough};
    border: 1px solid {border_subtle};
    border-radius: 999px;
    padding: 3px;
}}

button.neon-segment-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    padding: 5px 12px;
    color: {fg_muted};
    font-size: 12px;
    font-weight: 600;
    transition: background-color 160ms ease, color 160ms ease;
}}

button.neon-segment-btn:hover {{
    color: {fg_primary};
}}

button.neon-segment-btn.active {{
    background-color: {accent};
    color: #FFFFFF;
    box-shadow: 0 2px 8px alpha({accent}, 0.35);
}}

button.neon-pill-button {{
    background-image: none;
    background-color: {tahoe_card_bg};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    border-radius: 12px;
    padding: 6px 12px;
    min-height: 28px;
    font-size: 12px;
    font-weight: 600;
    transition: background-color 140ms ease, border-color 140ms ease;
}}

button.neon-pill-button:hover {{
    border-color: {accent};
    background-color: {tahoe_card_hover};
}}

/* ===================== MACOS LAUNCHPAD (MATERIAL 3 EXPRESSIVE MOTION) ===================== */

.neon-launchpad-overlay {{
    /* M3 Expressive Default Effects (zero bounce on backdrop opacity) */
    transition: opacity {d_ov_fx_def}ms {m3_def_fx};
}}

.neon-launchpad-overlay.lp-closed {{
    opacity: 0;
}}

.neon-launchpad-overlay.lp-open {{
    opacity: 1;
}}

.neon-launchpad-blur-bg {{
    filter: none;
    transform: none;
}}

.neon-launchpad-scrim {{
    background-color: {scrim_bg};
    padding: 26px 64px 98px 64px;
    color: #FFFFFF;
}}

.neon-launchpad-content {{
    /* Exit: M3 Expressive Slow Effects (clean zero-bounce retreat) */
    transition: transform {d_ov_lp_out}ms {m3_slow_fx},
                opacity {d_ov_fx_in}ms {m3_def_fx};
}}

.neon-launchpad-overlay.lp-closed .neon-launchpad-content {{
    opacity: 0;
    transform: scale(0.92) translateY(18px);
}}

.neon-launchpad-overlay.lp-open .neon-launchpad-content {{
    opacity: 1;
    transform: scale(1.0) translateY(0px);
    /* Enter: M3 Expressive Default Spatial (organic overshoot) + Fast Effects opacity */
    transition: transform {d_ov_in}ms {m3_def_sp},
                opacity {d_ov_fx_in}ms {m3_fast_fx};
}}

.neon-launchpad-search {{
    background-image: none;
    background-color: rgba(255, 255, 255, 0.12);
    color: #FFFFFF;
    caret-color: #FFFFFF;
    border: 1px solid rgba(255, 255, 255, 0.30);
    border-radius: 10px;
    padding: 4px 14px;
    font-size: 13px;
    min-width: 256px;
    min-height: 28px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.14), inset 0 1px 0 rgba(255, 255, 255, 0.22);
    transition: background-color 180ms ease, border-color 180ms ease;
}}

.neon-launchpad-search:focus-within {{
    background-color: rgba(255, 255, 255, 0.18);
    border-color: rgba(255, 255, 255, 0.58);
}}

.neon-launchpad-search text placeholder {{
    color: rgba(255, 255, 255, 0.68);
}}

button.neon-launchpad-ghost-btn {{
    background-image: none;
    background-color: rgba(255, 255, 255, 0.10);
    color: rgba(255, 255, 255, 0.78);
    border: 1px solid rgba(255, 255, 255, 0.18);
    box-shadow: none;
    border-radius: 999px;
    padding: 4px 13px;
    min-height: 24px;
    font-size: 11px;
    font-weight: 600;
    transition: background-color 160ms ease, color 160ms ease, border-color 160ms ease;
}}

button.neon-launchpad-ghost-btn:hover {{
    background-color: rgba(255, 255, 255, 0.20);
    color: #FFFFFF;
    border-color: rgba(255, 255, 255, 0.38);
}}

button.neon-launchpad-tile {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    outline: none;
    border-radius: 22px;
    padding: 8px 10px;
    min-width: 112px;
    min-height: 108px;
    color: #FFFFFF;
    transition: transform {d_fast_sp}ms {m3_fast_sp}, background-color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-launchpad-tile:hover {{
    background-color: rgba(255, 255, 255, 0.13);
    transform: translateY(-2px) scale(1.10);
}}

button.neon-launchpad-tile:active {{
    transform: scale(0.90);
}}

.neon-launchpad-label {{
    color: #FFFFFF;
    font-size: 12px;
    font-weight: 500;
    text-shadow: 0 1px 5px rgba(0, 0, 0, 0.72);
}}

button.neon-page-dot {{
    background-image: none;
    background-color: rgba(255, 255, 255, 0.36);
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 8px;
    min-height: 8px;
    padding: 0;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                min-width {d_fast_sp}ms {m3_fast_sp},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-page-dot:hover {{
    background-color: rgba(255, 255, 255, 0.72);
    transform: scale(1.22);
}}

button.neon-page-dot.active {{
    background-color: #FFFFFF;
    min-width: 18px;
    min-height: 8px;
    transform: scale(1.06);
}}

button.neon-workspace-card {{
    background-image: none;
    background-color: {tahoe_card_bg};
    border: 1px solid {border_subtle};
    box-shadow: {specular_inset};
    border-radius: 18px;
    padding: 12px 16px;
    min-width: 176px;
    min-height: 92px;
    color: {fg_primary};
    transition: border-color 160ms ease, background-color 160ms ease, transform 160ms ease;
}}

button.neon-workspace-card:hover {{
    border-color: {border_glass};
    transform: translateY(-2px);
}}

button.neon-workspace-card.active {{
    border-color: {accent};
    background-color: alpha({accent}, 0.20);
}}

.neon-window-card {{
    background-color: {tahoe_card_bg};
    border: 1px solid {border_subtle};
    box-shadow: {specular_inset};
    border-radius: 18px;
    padding: 12px 14px;
    min-width: 220px;
    color: {fg_primary};
}}

.neon-window-card.focused {{
    border-color: {accent};
}}

button.neon-close-button {{
    background-image: none;
    background-color: rgba(239, 68, 68, 0.25);
    color: #FCA5A5;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    padding: 0 6px;
}}

button.neon-close-button:hover {{
    background-color: rgba(239, 68, 68, 0.88);
    color: #FFFFFF;
}}

/* ===================== FULLSCREEN MISSION CONTROL / WORKSPACES EXPOSÉ (LIQUID GLASS × MATERIAL 3 EXPRESSIVE) ===================== */
/* Official M3 Expressive motion physics tokens (m3.material.io/styles/motion/overview/how-it-works):
 * - Fast Spatial:    350ms cubic-bezier(0.42, 1.67, 0.21, 0.90)  (lively overshoot for hover/press/switch)
 * - Default Spatial: 500ms cubic-bezier(0.38, 1.21, 0.22, 1.00)  (organic bounce for IN entrances)
 * - Fast Effects:    150ms cubic-bezier(0.31, 0.94, 0.34, 1.00)  (zero-bounce opacity & color)
 * - Default Effects: 200ms cubic-bezier(0.34, 0.80, 0.34, 1.00)  (zero-bounce backdrop & scrim fade)
 * - Slow Effects:    240ms cubic-bezier(0.34, 0.88, 0.34, 1.00)  (smooth zero-bounce OUT retreat)
 */

.neon-ws-mc-overlay {{
    transition: opacity {d_ov_fx_def}ms {m3_def_fx};
}}

.neon-ws-mc-overlay.ws-closed {{
    opacity: 0;
    transition: none;
}}

.neon-ws-mc-overlay.ws-open {{
    opacity: 1;
    transition: opacity {d_ov_fx_def}ms {m3_def_fx};
}}

.neon-ws-mc-overlay.ws-closing {{
    opacity: 0;
    /* Keep blurred backdrop vivid during the expressive glide-out, then dissolve smoothly */
    transition: opacity {d_ov_bg_out}ms {m3_slow_fx} {d_ov_bg_del}ms;
}}

.neon-ws-mc-blur-bg {{
    filter: none;
    transform: none;
}}

.neon-ws-mc-content {{
    background-color: {ws_scrim_bg};
    color: {fg_primary};
}}

/* Floating Liquid Glass Top Desk Capsule (visual twin of the floating Dock & Quick Settings) */
.neon-ws-desk-tray {{
    background-color: {ws_tray_bg};
    background-image: {ws_tray_grad};
    border: 1px solid {border_glass};
    border-radius: 22px;
    padding: 8px 8px 7px 8px;
    box-shadow: {ws_tray_shadow};
    color: {fg_primary};
    transition: transform {d_ov_in}ms {m3_def_sp},
                opacity {d_ov_fx_in}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-ws-mc-overlay.ws-closed .neon-ws-desk-tray {{
    opacity: 0;
    transform: translateY(-42px) scale(0.90);
    transition: none;
}}

.neon-ws-mc-overlay.ws-open .neon-ws-desk-tray {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
}}

.neon-ws-mc-overlay.ws-closing .neon-ws-desk-tray {{
    opacity: 0;
    transform: translateY(-46px) scale(0.90);
    /* Mirror the IN Material 3 Expressive spatial glide cleanly upward */
    transition: transform {d_ov_out}ms {m3_exit_sp},
                opacity {d_ov_fx_out}ms {m3_def_fx} {d_ov_fade_del}ms;
}}

.neon-ws-desk-tray.drag-active {{
    border-color: alpha({accent}, 0.72);
    box-shadow: 0 0 0 1px alpha({accent}, 0.32), {ws_tray_shadow};
}}

.neon-ws-desk-scroll,
.neon-ws-desk-scroll > viewport {{
    background-color: transparent;
    background-image: none;
    border: none;
}}

.neon-ws-desk-scroll scrollbar {{
    background-color: transparent;
    border: none;
}}

.neon-ws-desk-scroll scrollbar slider {{
    background-color: {border_glass};
    border-radius: 999px;
    min-height: 4px;
}}

.neon-ws-desk-row {{
    padding: 2px 2px 1px 2px;
}}

.neon-ws-tray-sep {{
    background-color: {border_subtle};
    min-width: 1px;
    margin: 12px 2px 30px 2px;
}}

.neon-ws-desk-col {{
    padding: 1px;
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_desk_in}ms {m3_fast_sp},
                opacity {d_ov_fx_in}ms {m3_fast_fx};
}}

.neon-ws-desk-col.ws-desk-enter {{
    opacity: 0;
    transform: translateY(-12px) scale(0.76);
    transition: none;
}}

.neon-ws-desk-col.ws-desk-removing {{
    opacity: 0;
    transform: translateY(-10px) scale(0.74);
    transition: transform {d_desk_rm}ms {m3_exit_sp},
                opacity {d_desk_rm_fx}ms {m3_fast_fx};
}}

.neon-ws-desk-col:hover {{
    transform: translateY(-2px);
}}

.neon-ws-desk-col.ws-desk-removing:hover {{
    transform: translateY(-10px) scale(0.74);
}}

.neon-ws-desk-col.drop-target {{
    transform: scale(1.06);
}}

.neon-ws-desk-ring {{
    padding: 3px;
    border-radius: 13px;
    border: 2px solid transparent;
    background-color: transparent;
    transition: border-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                background-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00);
}}

.neon-ws-desk-col:hover .neon-ws-desk-ring {{
    border-color: {cc_glass_border};
    background-color: {qs_inner_bg};
}}

.neon-ws-desk-ring.active {{
    border-color: {accent};
    background-color: alpha({accent}, 0.20);
    box-shadow: 0 0 0 1px alpha({accent}, 0.36), 0 6px 16px alpha({accent}, 0.24);
}}

.neon-ws-desk-col:hover .neon-ws-desk-ring.active {{
    border-color: {accent};
    background-color: alpha({accent}, 0.26);
}}

.neon-ws-desk-col.drop-target .neon-ws-desk-ring {{
    border-color: {accent};
    background-color: alpha({accent}, 0.32);
    box-shadow: 0 0 0 2px alpha({accent}, 0.55), 0 10px 24px alpha({accent}, 0.36);
}}

.neon-ws-desk-thumb {{
    background-color: {ws_thumb_bg};
    border: 1px solid {ws_thumb_border};
    border-radius: 10px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.24);
}}

.neon-ws-desk-mini-win {{
    background-color: {ws_win_frame_bg};
    border: 1px solid rgba(255, 255, 255, 0.28);
    border-radius: 3px;
    box-shadow: 0 2px 5px rgba(0, 0, 0, 0.40);
}}

.neon-ws-desk-label {{
    color: {fg_muted};
    font-size: 11.5px;
    font-weight: 500;
    letter-spacing: 0.1px;
    transition: color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00);
}}

.neon-ws-desk-col:hover .neon-ws-desk-label {{
    color: {fg_primary};
}}

.neon-ws-desk-label.active {{
    color: {fg_primary};
    font-weight: 700;
}}

.neon-ws-add-label {{
    opacity: 0.78;
}}

.neon-ws-add-col:hover .neon-ws-add-label {{
    color: {fg_primary};
    opacity: 1.0;
}}

/* Dock-mirrored active accent indicator pill beneath each Desk label */
.neon-ws-desk-indicator {{
    background-color: transparent;
    border-radius: 999px;
    min-width: 5px;
    min-height: 3px;
    margin-top: 1px;
    transition: min-width 350ms cubic-bezier(0.42, 1.67, 0.21, 0.90),
                background-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                opacity 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00);
}}

.neon-ws-desk-indicator.occupied {{
    background-color: {fg_muted};
    opacity: 0.65;
}}

.neon-ws-desk-indicator.active {{
    background-color: {accent};
    min-width: 18px;
    box-shadow: 0 0 8px alpha({accent}, 0.60);
}}

.neon-ws-add-col {{
    padding: 1px;
    transition: transform 350ms cubic-bezier(0.42, 1.67, 0.21, 0.90);
}}

.neon-ws-add-col:hover {{
    transform: translateY(-2px);
}}

.neon-ws-add-col.drop-target {{
    transform: scale(1.06);
}}

.neon-ws-add-ring {{
    padding: 3px;
    border-radius: 13px;
    border: 2px solid transparent;
    background-color: transparent;
    transition: border-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                background-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00);
}}

.neon-ws-add-col:hover .neon-ws-add-ring {{
    border-color: {cc_glass_border};
    background-color: {qs_inner_bg};
}}

.neon-ws-add-col.drop-target .neon-ws-add-ring {{
    border-color: {accent};
    background-color: alpha({accent}, 0.28);
    box-shadow: 0 0 0 2px alpha({accent}, 0.50), 0 10px 24px alpha({accent}, 0.34);
}}

button.neon-ws-desk-add-btn {{
    background-image: {ws_tray_grad};
    background-color: {ws_add_bg};
    color: {fg_primary};
    border: 1px solid {ws_thumb_border};
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.16), {specular_inset};
    border-radius: 10px;
    padding: 0;
    transition: background-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                border-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                transform 350ms cubic-bezier(0.42, 1.67, 0.21, 0.90);
}}

button.neon-ws-desk-add-btn:hover {{
    background-color: {ws_add_hover};
    border-color: {border_glass};
}}

button.neon-ws-desk-add-btn:active,
button.neon-ws-desk-add-btn.add-pulse {{
    transform: scale(0.92);
}}

.neon-ws-add-col.drop-target button.neon-ws-desk-add-btn {{
    background-image: none;
    background-color: alpha({accent}, 0.34);
    color: #FFFFFF;
    border-color: alpha({accent}, 0.80);
}}

/* Pure borderless container for anti-aliased Cairo vector circle buttons (zero GSK 4-dot border artifacts) */
button.neon-ws-vec-circle-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    outline: none;
    border-radius: 0;
    min-width: 0;
    min-height: 0;
    padding: 0;
    margin: 0;
    opacity: 0;
    transform: scale(0.80);
    transition: opacity 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                transform 350ms cubic-bezier(0.42, 1.67, 0.21, 0.90);
}}

.neon-ws-desk-col:hover button.neon-ws-pin-btn,
.neon-ws-desk-col:hover button.neon-ws-thumb-close,
.neon-ws-expose-card:hover button.neon-ws-expose-close {{
    opacity: 1.0;
    transform: scale(1.0);
}}

.neon-ws-desk-col:hover button.neon-ws-vec-circle-btn:hover,
.neon-ws-expose-card:hover button.neon-ws-vec-circle-btn:hover {{
    opacity: 1.0;
    transform: scale(1.12);
}}

.neon-ws-desk-col:hover button.neon-ws-vec-circle-btn:active,
.neon-ws-expose-card:hover button.neon-ws-vec-circle-btn:active {{
    transform: scale(0.92);
}}

/* ── Center Exposé Window Spread Stage & Cards (M3 Expressive Choreography) ── */

.neon-ws-expose-stage {{
    background-color: transparent;
    transition: transform {d_ov_fast}ms {m3_fast_sp},
                opacity {d_ov_fx_in}ms {m3_fast_fx};
}}

.neon-ws-expose-stage.ws-stage-enter {{
    opacity: 0;
    transform: translateY(14px) scale(0.94);
    transition: none;
}}

.neon-ws-mc-overlay.ws-closed .neon-ws-expose-stage {{
    opacity: 0;
    transform: translateY(22px) scale(0.92);
    transition: none;
}}

.neon-ws-mc-overlay.ws-open .neon-ws-expose-stage {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_ov_in}ms {m3_def_sp},
                opacity {d_ov_fx_in}ms {m3_fast_fx};
}}

.neon-ws-mc-overlay.ws-closing .neon-ws-expose-stage {{
    opacity: 1;
    transform: translateY(16px) scale(0.93);
    transition: transform {d_ov_out}ms {m3_exit_sp};
}}

.neon-ws-expose-card {{
    border-radius: 12px;
    transition: transform {d_ov_fast}ms {m3_fast_sp},
                opacity {d_fast_fx}ms {m3_fast_fx};
}}

.neon-ws-mc-overlay.ws-closed .neon-ws-expose-card {{
    opacity: 0;
    transform: translateY(18px) scale(0.88);
    transition: none;
}}

.neon-ws-mc-overlay.ws-open .neon-ws-expose-card {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_ov_in}ms {m3_def_sp},
                opacity {d_ov_fx_in}ms {m3_fast_fx};
}}

.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.neon-ws-stagger-0 {{ transition-delay: 0ms; }}
.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.neon-ws-stagger-1 {{ transition-delay: {d_st_in_1}ms; }}
.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.neon-ws-stagger-2 {{ transition-delay: {d_st_in_2}ms; }}
.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.neon-ws-stagger-3 {{ transition-delay: {d_st_in_3}ms; }}
.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.neon-ws-stagger-4 {{ transition-delay: {d_st_in_4}ms; }}
.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.neon-ws-stagger-5 {{ transition-delay: {d_st_in_5}ms; }}

.neon-ws-mc-overlay.ws-closing .neon-ws-expose-card {{
    opacity: 0;
    transform: translateY(22px) scale(0.86);
    /* Mirror the expressive IN card wave cleanly without premature double-fade */
    transition: transform {d_ov_out}ms {m3_exit_sp},
                opacity {d_ov_fx_out}ms {m3_def_fx};
}}

.neon-ws-mc-overlay.ws-closing .neon-ws-expose-card.neon-ws-stagger-0 {{ transition-delay: 0ms, {d_st_out_0_f}ms; }}
.neon-ws-mc-overlay.ws-closing .neon-ws-expose-card.neon-ws-stagger-1 {{ transition-delay: {d_st_out_1_t}ms, {d_st_out_1_f}ms; }}
.neon-ws-mc-overlay.ws-closing .neon-ws-expose-card.neon-ws-stagger-2 {{ transition-delay: {d_st_out_2_t}ms, {d_st_out_2_f}ms; }}
.neon-ws-mc-overlay.ws-closing .neon-ws-expose-card.neon-ws-stagger-3 {{ transition-delay: {d_st_out_3_t}ms, {d_st_out_3_f}ms; }}
.neon-ws-mc-overlay.ws-closing .neon-ws-expose-card.neon-ws-stagger-4 {{ transition-delay: {d_st_out_4_t}ms, {d_st_out_4_f}ms; }}
.neon-ws-mc-overlay.ws-closing .neon-ws-expose-card.neon-ws-stagger-5 {{ transition-delay: {d_st_out_5_t}ms, {d_st_out_5_f}ms; }}

.neon-ws-mc-overlay.ws-open .neon-ws-expose-card:hover {{
    transform: translateY(-3px) scale(1.024);
    transition: transform {d_ov_fast}ms {m3_fast_sp},
                opacity {d_fast_fx}ms {m3_fast_fx};
    transition-delay: 0ms;
}}

.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.minimized {{
    opacity: 0.84;
}}

.neon-ws-mc-overlay.ws-open .neon-ws-expose-card.dragging-source {{
    opacity: 0.20;
    transform: scale(0.95);
    transition-delay: 0ms;
}}

.neon-ws-expose-frame {{
    background-color: {ws_win_frame_bg};
    border: 1px solid {ws_win_border};
    border-radius: 12px;
    box-shadow: {ws_win_shadow};
    transition: border-color 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00);
}}

.neon-ws-expose-card.focused .neon-ws-expose-frame {{
    border-color: alpha({accent}, 0.72);
    box-shadow: 0 0 0 1px alpha({accent}, 0.34), {ws_win_shadow};
}}

.neon-ws-expose-card:hover .neon-ws-expose-frame {{
    border-color: {accent};
    box-shadow: 0 0 0 2px alpha({accent}, 0.48), {ws_hover_shadow};
}}

.neon-ws-expose-placeholder {{
    background-color: {qs_panel_bg};
    background-image: {cc_glass_grad};
    padding: 16px;
}}

.neon-ws-expose-placeholder-title {{
    color: {fg_primary};
    font-size: 13px;
    font-weight: 600;
}}

/* Frosted Liquid Glass hover title pill (Light & Dark native) */
.neon-ws-expose-title-pill {{
    background-color: {cc_glass_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    border-radius: 999px;
    padding: 5px 12px 5px 9px;
    box-shadow: {cc_glass_shadow};
    opacity: 0;
    transform: translateY(4px) scale(0.94);
    transition: opacity 150ms cubic-bezier(0.31, 0.94, 0.34, 1.00),
                transform 350ms cubic-bezier(0.42, 1.67, 0.21, 0.90);
}}

.neon-ws-expose-card:hover .neon-ws-expose-title-pill,
.neon-ws-expose-title-pill.always-visible {{
    opacity: 1.0;
    transform: translateY(0px) scale(1.0);
}}

.neon-ws-expose-title-text {{
    color: {fg_primary};
    font-size: 12px;
    font-weight: 600;
}}

.neon-ws-expose-min-tag {{
    background-color: alpha({accent}, 0.24);
    border: 1px solid alpha({accent}, 0.42);
    color: {fg_primary};
    border-radius: 999px;
    padding: 1px 7px;
    font-size: 10.5px;
    font-weight: 600;
}}

/* ── Floating 60 FPS Dragged Window Ghost ── */

.neon-ws-drag-ghost {{
    background-color: {ws_win_frame_bg};
    border: 2px solid {accent};
    border-radius: 12px;
    box-shadow: 0 24px 58px rgba(0, 0, 0, 0.54), 0 0 0 2px alpha({accent}, 0.38);
    opacity: 0.95;
}}

/* ── Liquid Glass Empty Desk State Card ── */

.neon-ws-empty-state {{
    background-color: {ws_empty_bg};
    background-image: {ws_tray_grad};
    border: 1px solid {border_glass};
    border-radius: 24px;
    padding: 22px 28px;
    box-shadow: {ws_tray_shadow};
    color: {fg_primary};
    transition: transform {d_ov_in}ms {m3_def_sp},
                opacity {d_ov_fx_in}ms {m3_fast_fx};
}}

.neon-ws-mc-overlay.ws-closed .neon-ws-empty-state {{
    opacity: 0;
    transform: translateY(24px) scale(0.86);
    transition: none;
}}

.neon-ws-mc-overlay.ws-open .neon-ws-empty-state {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
}}

.neon-ws-mc-overlay.ws-closing .neon-ws-empty-state {{
    opacity: 0;
    transform: translateY(24px) scale(0.86);
    transition: transform {d_ov_out}ms {m3_exit_sp},
                opacity {d_ov_fx_out}ms {m3_def_fx} {d_ov_fade_del}ms;
}}

.neon-ws-empty-badge {{
    background-color: alpha({accent}, 0.22);
    border: 1px solid alpha({accent}, 0.44);
    border-radius: 999px;
    min-width: 44px;
    min-height: 44px;
    color: {accent};
}}

.neon-ws-empty-title {{
    color: {fg_primary};
    font-size: 16px;
    font-weight: 700;
    letter-spacing: 0.1px;
}}

.neon-ws-empty-sub {{
    color: {fg_muted};
    font-size: 12px;
    font-weight: 500;
}}

/* ===================== NOTIFICATION TOASTS (MATERIAL 3 EXPRESSIVE) ===================== */

.neon-toast-card {{
    background-color: {popover_bg};
    border: 1px solid {border_glass};
    border-radius: 20px;
    padding: 12px 16px;
    color: {fg_primary};
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.42), {specular_inset};
    min-width: 320px;
    opacity: 1;
    transform: translateX(0px) scale(1.0);
    transition: transform {d_def_sp}ms {m3_def_sp},
                opacity {d_def_fx}ms {m3_fast_fx};
}}

.neon-toast-card.toast-enter {{
    opacity: 0;
    transform: translateX(52px) scale(0.90);
    transition: none;
}}

.neon-toast-card.toast-exit {{
    opacity: 0;
    transform: translateX(46px) scale(0.92);
    transition: transform {d_slow_fx}ms {m3_slow_fx},
                opacity {d_fast_fx}ms {m3_fast_fx};
}}

.neon-toast-card.critical {{
    border-color: #EF4444;
}}

button.neon-toast-body-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 12px;
    padding: 4px 6px;
    margin: -2px -6px;
    color: {fg_primary};
    transition: background-color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-toast-body-btn:hover {{
    background-color: {qs_inner_bg};
}}

.neon-toast-actions-row {{
    margin-top: 2px;
}}

button.neon-notif-action-btn {{
    background-image: none;
    background-color: {qs_pill_bg};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 999px;
    padding: 3px 11px;
    min-height: 24px;
    font-size: 11.5px;
    font-weight: 600;
    color: {fg_primary};
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-notif-action-btn:hover {{
    background-color: alpha({accent}, 0.24);
    border-color: alpha({accent}, 0.50);
    transform: scale(1.03);
}}

button.neon-notif-action-btn:active {{
    transform: scale(0.95);
}}

/* ===================== FROSTED GLASS QUICK SETTINGS (MATERIAL 3 EXPRESSIVE) ===================== */

.neon-qs-panel-card {{
    background-color: {qs_panel_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    border-radius: 24px;
    padding: 14px;
    box-shadow: {specular_inset};
    color: {fg_primary};
}}

.neon-qs-status-pill {{
    background-color: {qs_inner_bg};
    border: 1px solid {border_subtle};
    border-radius: 999px;
    padding: 4px 11px;
    min-height: 24px;
}}

button.neon-qs-header-power-btn {{
    background-image: none;
    background-color: {qs_pill_bg};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 999px;
    padding: 4px 12px;
    min-height: 28px;
    color: {fg_primary};
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-qs-header-power-btn:hover {{
    background-color: {qs_pill_hover};
    border-color: {cc_glass_border};
    transform: scale(1.05);
}}

button.neon-qs-header-power-btn:active {{
    transform: scale(0.92);
}}

.neon-qs-header-btn-lbl {{
    font-size: 12px;
    font-weight: 600;
    color: {fg_primary};
}}

/* Equal-Length Split Connectivity Pills (Material 3 Expressive Shape Morphing) */
.neon-qs-split-pill {{
    background-color: {qs_pill_bg};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    min-height: 54px;
    color: {fg_primary};
    transition: background-color {d_def_fx}ms {m3_def_fx},
                border-color {d_def_fx}ms {m3_def_fx},
                border-radius {d_def_fx}ms {m3_def_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

.neon-qs-split-pill.active {{
    background-color: alpha({qs_pill_active_bg}, 0.88);
    border-color: rgba(255, 255, 255, 0.28);
    border-radius: 26px;
    color: {qs_pill_active_fg};
}}

.neon-qs-split-pill.active .neon-qs-pill-title,
.neon-qs-split-pill.active .neon-qs-pill-sub,
.neon-qs-split-pill.active .neon-qs-split-icon,
.neon-qs-split-pill.active .neon-qs-tiling-layout-lbl,
.neon-qs-split-pill.active button.neon-qs-split-main,
.neon-qs-split-pill.active button.neon-qs-split-arrow,
.neon-qs-split-pill.active button.neon-qs-split-layout-btn {{
    color: {qs_pill_active_fg};
}}

button.neon-qs-split-main {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 24px 0 0 24px;
    padding: 8px 8px 8px 12px;
    color: {fg_primary};
    transition: background-color {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-qs-split-main:hover {{
    background-color: rgba(255, 255, 255, 0.11);
}}

button.neon-qs-split-main:active {{
    transform: scale(0.95);
}}

.neon-qs-split-sep {{
    background-color: rgba(255, 255, 255, 0.16);
    min-width: 1px;
    margin: 10px 0;
}}

button.neon-qs-split-arrow {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 0 24px 24px 0;
    min-width: 30px;
    padding: 0 6px;
    color: {fg_primary};
    transition: background-color {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-qs-split-arrow:hover {{
    background-color: rgba(255, 255, 255, 0.15);
}}

button.neon-qs-split-arrow:active {{
    transform: scale(0.88);
}}

button.neon-qs-split-layout-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 0 24px 24px 0;
    min-width: 84px;
    padding: 0 12px;
    color: {fg_primary};
    transition: background-color {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-qs-split-layout-btn:hover {{
    background-color: rgba(255, 255, 255, 0.15);
}}

button.neon-qs-split-layout-btn:active {{
    transform: scale(0.93);
}}

.neon-qs-tiling-layout-lbl {{
    font-size: 11.5px;
    font-weight: 700;
    color: {fg_primary};
}}

/* Material 3 Expressive Elastic Chevron Flip */
.neon-qs-chevron {{
    transform: rotate(0deg);
    transition: transform {d_fast_sp}ms {m3_fast_sp};
}}

.neon-qs-chevron.expanded {{
    transform: rotate(180deg);
}}

.neon-qs-pill-title {{
    font-size: 13px;
    font-weight: 700;
    color: {fg_primary};
}}

.neon-qs-pill-sub {{
    font-size: 11px;
    font-weight: 500;
    color: {fg_muted};
    opacity: 0.90;
}}

/* Inline Expandable Dropdown Card (Material 3 Expressive Spring Reveal) */
.neon-qs-dropdown-card {{
    background-color: {qs_inner_bg};
    border: 1px solid {border_subtle};
    border-radius: 18px;
    padding: 10px 12px;
    transition: opacity {d_def_fx}ms {m3_def_fx},
                transform {d_def_sp}ms {m3_def_sp},
                border-radius {d_def_fx}ms {m3_def_fx};
}}

.neon-qs-dropdown-card.dd-closed {{
    opacity: 0;
    transform: translateY(-10px) scale(0.95);
    border-radius: 12px;
}}

.neon-qs-dropdown-card.dd-swap {{
    opacity: 0.45;
    transform: translateY(-4px) scale(0.98);
    transition: opacity {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_fx}ms {m3_fast_fx};
}}

.neon-qs-dropdown-card.dd-open {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    border-radius: 18px;
}}

button.neon-qs-dd-item {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 12px;
    padding: 7px 9px;
    color: {fg_primary};
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp},
                border-radius {d_def_fx}ms {m3_def_fx};
}}

button.neon-qs-dd-item:hover {{
    background-color: {qs_pill_hover};
    transform: translateX(3px);
}}

button.neon-qs-dd-item:active {{
    transform: scale(0.97);
    border-radius: 16px;
}}

button.neon-qs-dd-item.connected {{
    background-color: alpha({accent}, 0.22);
    border-radius: 14px;
}}

.neon-qs-dd-name {{
    font-size: 12.5px;
    font-weight: 600;
    color: {fg_primary};
}}

.neon-qs-dd-badge {{
    background-color: alpha({accent}, 0.28);
    color: {fg_primary};
    border-radius: 999px;
    padding: 2px 8px;
    font-size: 10.5px;
    font-weight: 700;
}}

.neon-qs-pw-entry {{
    background-image: none;
    background-color: {qs_pill_bg};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    border-radius: 12px;
    padding: 4px 10px;
    min-height: 28px;
    font-size: 12px;
    transition: border-color 180ms ease;
}}

.neon-qs-pw-entry:focus-within {{
    border-color: {accent};
}}

/* Inner Glass Cards (Display & Sound stack, Power Mode) */
.neon-qs-inner-card {{
    background-color: {qs_inner_bg};
    border: 1px solid {border_subtle};
    border-radius: 18px;
    padding: 10px 13px 11px 13px;
}}

.neon-qs-seg-track {{
    background-color: {slider_trough};
    border: 1px solid {border_subtle};
    border-radius: 14px;
    padding: 3px;
}}

button.neon-qs-seg-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 10px;
    padding: 6px 4px;
    min-height: 26px;
    color: {fg_muted};
    font-size: 11.5px;
    font-weight: 600;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx},
                border-radius {d_def_fx}ms {m3_def_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-qs-seg-btn:hover {{
    color: {fg_primary};
    background-color: rgba(255, 255, 255, 0.08);
}}

button.neon-qs-seg-btn:active {{
    transform: scale(0.94);
}}

button.neon-qs-seg-btn.active {{
    background-color: {accent};
    color: #FFFFFF;
    border-radius: 12px;
    box-shadow: 0 2px 10px alpha({accent}, 0.38);
}}

/* Bottom Equal-Length Pills (Material 3 Expressive Shape Morphing: Dark Mode + Screenshot) */
button.neon-qs-action-pill {{
    background-image: none;
    background-color: {qs_pill_bg};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 15px;
    min-height: 44px;
    padding: 0 12px;
    color: {fg_primary};
    transition: background-color {d_def_fx}ms {m3_def_fx},
                border-color {d_def_fx}ms {m3_def_fx},
                border-radius {d_def_fx}ms {m3_def_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-qs-action-pill:hover {{
    background-color: {qs_pill_hover};
    border-color: {cc_glass_border};
    transform: scale(1.03);
}}

button.neon-qs-action-pill:active {{
    transform: scale(0.93);
    border-radius: 11px;
}}

button.neon-qs-action-pill.active {{
    background-color: alpha({qs_pill_active_bg}, 0.88);
    border-color: rgba(255, 255, 255, 0.28);
    border-radius: 24px;
    color: {qs_pill_active_fg};
}}

button.neon-qs-action-pill.active .neon-qs-pill-title {{
    color: {qs_pill_active_fg};
}}

/* ===================== FULLSCREEN POWER MENU (MATERIAL 3 EXPRESSIVE CHOREOGRAPHY) ===================== */

.neon-power-menu-backdrop {{
    background-color: {pm_scrim_bg};
}}

.neon-power-menu-card {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 16px;
    color: {fg_primary};
}}

button.neon-pm-action-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 18px;
    padding: 14px 12px;
    color: #FFFFFF;
    text-shadow: 0 2px 10px rgba(0, 0, 0, 0.45);
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp},
                opacity {d_def_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-power-menu-card.pm-closed button.neon-pm-action-btn {{
    opacity: 0;
    transform: translateY(28px) scale(0.84);
    transition: none;
}}

.neon-power-menu-card.pm-open button.neon-pm-action-btn {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_def_sp}ms {m3_def_sp},
                opacity {d_def_fx}ms {m3_fast_fx},
                background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-power-menu-card.pm-open button.neon-pm-action-btn.pm-btn-0 {{ transition-delay: 0ms; }}
.neon-power-menu-card.pm-open button.neon-pm-action-btn.pm-btn-1 {{ transition-delay: {d_pm_1}ms; }}
.neon-power-menu-card.pm-open button.neon-pm-action-btn.pm-btn-2 {{ transition-delay: {d_pm_2}ms; }}
.neon-power-menu-card.pm-open button.neon-pm-action-btn.pm-btn-3 {{ transition-delay: {d_pm_3}ms; }}
.neon-power-menu-card.pm-open button.neon-pm-action-btn.pm-btn-4 {{ transition-delay: {d_pm_4}ms; }}

.neon-power-menu-card.pm-closing button.neon-pm-action-btn {{
    opacity: 0;
    transform: translateY(20px) scale(0.88);
    transition: transform {d_slow_fx}ms {m3_slow_fx},
                opacity {d_fast_fx}ms {m3_fast_fx};
}}

.neon-power-menu-card.pm-open button.neon-pm-action-btn:hover {{
    background-color: rgba(255, 255, 255, 0.16);
    transform: translateY(-4px) scale(1.12);
    transition: transform {d_fast_sp}ms {m3_fast_sp},
                background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx};
    transition-delay: 0ms;
}}

.neon-power-menu-card.pm-open button.neon-pm-action-btn.danger:hover {{
    background-color: alpha(#EF4444, 0.30);
    color: #FCA5A5;
}}

.neon-power-menu-card.pm-open button.neon-pm-action-btn:active {{
    transform: scale(0.92);
    transition-delay: 0ms;
}}

.neon-pm-action-lbl {{
    font-size: 13px;
    font-weight: 600;
    color: #FFFFFF;
    text-shadow: 0 1px 6px rgba(0, 0, 0, 0.55);
}}

.neon-power-menu-card .neon-cc-sub {{
    color: rgba(255, 255, 255, 0.72);
    text-shadow: 0 1px 6px rgba(0, 0, 0, 0.50);
}}

.neon-power-menu-confirm {{
    background-color: rgba(15, 20, 32, 0.55);
    border: 1px solid rgba(255, 255, 255, 0.22);
    border-radius: 999px;
    padding: 8px 16px;
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_fast_sp}ms {m3_fast_sp}, opacity {d_fast_fx}ms {m3_fast_fx};
}}

.neon-power-menu-confirm.pm-confirm-enter {{
    opacity: 0;
    transform: translateY(10px) scale(0.90);
    transition: none;
}}

/* ===================== LIQUID GLASS SCREENSHOT & SCREEN RECORD UI (MATCHES CONTROL CENTER) ===================== */

.neon-shot-bar-surface {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 18px;
}}

.neon-gnome-shot-bar {{
    background-color: {cc_glass_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    border-radius: 26px;
    padding: 14px 20px;
    box-shadow: {cc_glass_shadow};
    color: {fg_primary};
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_def_sp}ms {m3_def_sp}, opacity {d_def_fx}ms {m3_fast_fx};
}}

.neon-gnome-shot-bar.shot-closed {{
    opacity: 0;
    transform: translateY(38px) scale(0.90);
    transition: none;
}}

.neon-gnome-shot-bar.shot-open {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
}}

.neon-gnome-shot-bar.shot-closing {{
    opacity: 0;
    transform: translateY(28px) scale(0.92);
    transition: transform {d_slow_fx}ms {m3_slow_fx}, opacity {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-shot-mode-btn {{
    background-image: none;
    background-color: {cc_badge_idle};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 18px;
    padding: 7px 14px;
    font-size: 13px;
    font-weight: 600;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-shot-mode-btn:hover {{
    background-color: {cc_glass_hover};
    border-color: {cc_glass_border};
    color: {fg_primary};
    transform: scale(1.04);
}}

button.neon-shot-mode-btn:active {{
    transform: scale(0.94);
}}

button.neon-shot-mode-btn.active {{
    background-color: {accent};
    border-color: alpha({accent}, 0.85);
    color: #FFFFFF;
    box-shadow: 0 4px 12px alpha({accent}, 0.34);
}}

button.neon-shot-toggle-btn.active {{
    background-color: rgba(239, 68, 68, 0.86);
    border-color: rgba(254, 202, 202, 0.50);
    color: #FFFFFF;
}}

.neon-shot-kind-track {{
    background-color: {cc_badge_idle};
    border: 1px solid {border_subtle};
    border-radius: 18px;
    padding: 3px;
}}

button.neon-shot-kind-btn {{
    background-image: none;
    background-color: transparent;
    color: {fg_muted};
    border: none;
    box-shadow: none;
    border-radius: 15px;
    padding: 6px 13px;
    font-size: 12.5px;
    font-weight: 600;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-shot-kind-btn:hover {{
    color: {fg_primary};
}}

button.neon-shot-kind-btn:active {{
    transform: scale(0.94);
}}

button.neon-shot-kind-btn.active {{
    background-color: {cc_glass_hover};
    color: {fg_primary};
    box-shadow: none;
}}

button.neon-shot-shutter {{
    background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.98) 0%, rgba(236, 242, 252, 0.92) 100%);
    background-color: #FFFFFF;
    color: #0F172A;
    border: 3px solid {cc_glass_border};
    border-radius: 28px;
    min-width: 56px;
    min-height: 56px;
    padding: 0;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.30);
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-shot-shutter:hover {{
    background-color: #F1F5F9;
    transform: scale(1.08);
}}

button.neon-shot-shutter:active {{
    transform: scale(0.90);
}}

button.neon-shot-shutter.record {{
    background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.20) 0%, rgba(255, 255, 255, 0.02) 100%);
    background-color: #EF4444;
    color: #FFFFFF;
    border: 3px solid rgba(254, 202, 202, 0.58);
    box-shadow: 0 6px 20px rgba(239, 68, 68, 0.42);
}}

button.neon-shot-shutter.record:hover {{
    background-color: #DC2626;
}}

button.neon-shot-stop-btn {{
    background-image: linear-gradient(180deg, rgba(255, 255, 255, 0.18) 0%, rgba(255, 255, 255, 0.02) 100%);
    background-color: rgba(220, 38, 38, 0.86);
    color: #FFFFFF;
    border: 1px solid rgba(254, 202, 202, 0.48);
    box-shadow: 0 4px 14px rgba(220, 38, 38, 0.36);
    border-radius: 18px;
    padding: 8px 16px;
    font-size: 12.5px;
    font-weight: 700;
}}

button.neon-shot-stop-btn:hover {{
    background-color: rgba(239, 68, 68, 0.94);
}}

/* ===================== MACOS RIGHT-CLICK CONTEXT MENU ===================== */

.neon-ctx-backdrop-scrim {{
    background-color: rgba(0, 0, 0, 0.005);
    background-image: none;
    border: none;
    box-shadow: none;
}}

.neon-ctx-wrapper {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 22px;
}}

.neon-ctx-menu {{
    background-color: {ctx_bg};
    border: 1px solid {ctx_border};
    border-radius: 13px;
    padding: 5px;
    box-shadow: {ctx_shadow};
    color: {fg_primary};
}}

button.neon-ctx-item {{
    background-image: none;
    background-color: transparent;
    color: {fg_primary};
    border: none;
    box-shadow: none;
    border-radius: 7px;
    padding: 5px 9px;
    min-height: 26px;
    font-size: 13px;
    font-weight: 500;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-ctx-item:active {{
    transform: scale(0.97);
}}

button.neon-ctx-item:hover,
button.neon-ctx-item.focused {{
    background-color: {accent};
    color: #FFFFFF;
}}

button.neon-ctx-item.submenu-open {{
    background-color: {ctx_sub_open_bg};
}}

button.neon-ctx-item.submenu-open:hover,
button.neon-ctx-item.submenu-open.focused {{
    background-color: {accent};
    color: #FFFFFF;
}}

button.neon-ctx-item:hover .neon-ctx-shortcut,
button.neon-ctx-item.focused .neon-ctx-shortcut,
button.neon-ctx-item:hover .neon-ctx-chevron,
button.neon-ctx-item.focused .neon-ctx-chevron {{
    color: rgba(255, 255, 255, 0.88);
}}

button.neon-ctx-item.disabled {{
    opacity: 0.42;
}}

button.neon-ctx-item.destructive {{
    color: #F87171;
}}

button.neon-ctx-item.destructive:hover,
button.neon-ctx-item.destructive.focused {{
    background-color: #EF4444;
    color: #FFFFFF;
}}

.neon-ctx-icon {{
    opacity: 0.90;
}}

.neon-ctx-check {{
    opacity: 0.96;
}}

.neon-ctx-chevron {{
    opacity: 0.72;
    margin-left: 8px;
}}

.neon-ctx-shortcut {{
    color: {fg_muted};
    font-size: 11px;
    font-weight: 500;
    margin-left: 16px;
}}

.neon-ctx-header {{
    color: {fg_muted};
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.3px;
    padding: 4px 10px 3px 10px;
}}

.neon-ctx-sep {{
    background-color: {border_subtle};
    min-height: 1px;
    margin: 4px 8px;
}}

/* ===================== MACOS MODAL DIALOGS (RENAME / NEW FOLDER / INFO) ===================== */

.neon-dialog-card {{
    background-color: {ctx_dialog_bg};
    border: 1px solid {ctx_border};
    border-radius: 16px;
    padding: 18px 20px;
    margin: 12px;
    box-shadow: {ctx_shadow};
    color: {fg_primary};
}}

.neon-dialog-title {{
    font-size: 15px;
    font-weight: 700;
    color: {fg_primary};
}}

.neon-dialog-sub {{
    font-size: 12px;
    color: {fg_muted};
}}

.neon-dialog-key {{
    font-size: 12px;
    font-weight: 600;
    color: {fg_muted};
}}

.neon-dialog-val {{
    font-size: 12.5px;
    color: {fg_primary};
}}

.neon-dialog-error {{
    font-size: 12px;
    font-weight: 600;
    color: #F87171;
}}

entry.neon-dialog-entry {{
    background-color: {ctx_input_bg};
    color: {fg_primary};
    border: 1px solid {ctx_border};
    border-radius: 9px;
    padding: 7px 11px;
    font-size: 13px;
    min-height: 32px;
}}

entry.neon-dialog-entry:focus-within {{
    border-color: {accent};
    box-shadow: 0 0 0 2px alpha({accent}, 0.28);
}}

button.neon-dialog-btn {{
    background-image: none;
    background-color: {ctx_input_bg};
    color: {fg_primary};
    border: 1px solid {ctx_border};
    border-radius: 9px;
    padding: 6px 15px;
    min-height: 30px;
    font-size: 13px;
    font-weight: 600;
    transition: background-color {d_fast_fx}ms {m3_fast_fx}, transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-dialog-btn:hover {{
    background-color: {ctx_sub_open_bg};
    transform: scale(1.03);
}}

button.neon-dialog-btn:active {{
    transform: scale(0.94);
}}

button.neon-dialog-btn.primary {{
    background-color: {accent};
    color: #FFFFFF;
    border-color: transparent;
}}

button.neon-dialog-btn.destructive {{
    background-color: #EF4444;
    color: #FFFFFF;
    border-color: transparent;
}}

/* ===================== INTERACTIVE DESKTOP ICONS LAYER (M3 EXPRESSIVE) ===================== */

.neon-desktop-icons-layer {{
    background-color: transparent;
    padding: 46px 22px 92px 22px;
}}

button.neon-desktop-icon-tile {{
    background-image: none;
    background-color: transparent;
    border: 1px solid transparent;
    box-shadow: none;
    border-radius: 12px;
    padding: 8px 6px;
    min-width: 88px;
    min-height: 92px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-desktop-icon-tile:hover {{
    background-color: rgba(255, 255, 255, 0.14);
    border-color: rgba(255, 255, 255, 0.24);
    transform: translateY(-2px) scale(1.04);
}}

button.neon-desktop-icon-tile:active {{
    transform: scale(0.93);
}}

button.neon-desktop-icon-tile.selected {{
    background-color: alpha({accent}, 0.30);
    border-color: alpha({accent}, 0.72);
}}

.neon-desktop-icon-label {{
    color: #FFFFFF;
    font-size: 12px;
    font-weight: 600;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.82), 0 0 2px rgba(0, 0, 0, 0.65);
}}

/* ===================== POLICYKIT AUTHENTICATION DIALOG (LIQUID GLASS + M3 EXPRESSIVE) ===================== */

.neon-polkit-backdrop {{
    background-color: rgba(6, 10, 20, 0.54);
    transition: background-color {d_def_fx}ms {m3_fast_fx};
}}

.neon-polkit-backdrop.pk-closing {{
    background-color: rgba(6, 10, 20, 0.0);
    transition: background-color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-polkit-card {{
    background-color: {ctx_dialog_bg};
    border: 1px solid {ctx_border};
    border-radius: 24px;
    padding: 26px 26px 22px 26px;
    margin: 18px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.56), inset 0 1px 0 rgba(255, 255, 255, 0.20);
    color: {fg_primary};
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_def_sp}ms {m3_def_sp}, opacity {d_def_fx}ms {m3_fast_fx};
}}

.neon-polkit-card.pk-closed {{
    opacity: 0;
    transform: translateY(18px) scale(0.90);
    transition: none;
}}

.neon-polkit-card.pk-open {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
    transition: transform {d_def_sp}ms {m3_def_sp}, opacity {d_def_fx}ms {m3_fast_fx};
}}

.neon-polkit-card.pk-closing {{
    opacity: 0;
    transform: translateY(10px) scale(0.94);
    transition: transform {d_slow_fx}ms {m3_slow_fx}, opacity {d_fast_fx}ms {m3_fast_fx};
}}

.neon-polkit-emblem {{
    background-color: alpha({accent}, 0.20);
    border: 1px solid alpha({accent}, 0.48);
    border-radius: 28px;
    color: {accent};
    box-shadow: 0 6px 20px alpha({accent}, 0.24);
    margin-bottom: 4px;
}}

.neon-polkit-title {{
    font-size: 17px;
    font-weight: 700;
    color: {fg_primary};
}}

.neon-polkit-message {{
    font-size: 13px;
    color: {fg_muted};
}}

.neon-polkit-user-pill {{
    background-color: {ctx_input_bg};
    border: 1px solid {ctx_border};
    border-radius: 999px;
    padding: 5px 14px;
    color: {fg_primary};
}}

.neon-polkit-user-label {{
    font-size: 12.5px;
    font-weight: 600;
    color: {fg_primary};
}}

.neon-polkit-dropdown > button {{
    background-image: none;
    background-color: {ctx_input_bg};
    color: {fg_primary};
    border: 1px solid {ctx_border};
    border-radius: 12px;
    padding: 7px 12px;
}}

entry.neon-polkit-entry {{
    background-color: {ctx_input_bg};
    color: {fg_primary};
    border: 1px solid {ctx_border};
    border-radius: 12px;
    padding: 6px 12px;
    min-height: 38px;
    font-size: 14px;
    transition: border-color {d_fast_fx}ms {m3_fast_fx};
}}

entry.neon-polkit-entry:focus-within {{
    border-color: {accent};
    box-shadow: 0 0 0 2px alpha({accent}, 0.32);
}}

.neon-polkit-status {{
    padding: 2px 4px;
}}

.neon-polkit-status-label {{
    font-size: 12.5px;
    font-weight: 600;
    color: {fg_muted};
}}

.neon-polkit-error {{
    background-color: rgba(239, 68, 68, 0.16);
    border: 1px solid rgba(248, 113, 113, 0.45);
    border-radius: 11px;
    padding: 8px 12px;
    color: #FCA5A5;
}}

.neon-polkit-error-label {{
    font-size: 12.5px;
    font-weight: 600;
    color: #FCA5A5;
}}

.neon-polkit-expander {{
    font-size: 12px;
    color: {fg_muted};
}}

.neon-polkit-details-box {{
    background-color: {ctx_input_bg};
    border: 1px solid {ctx_border};
    border-radius: 10px;
    padding: 8px 10px;
    margin-top: 6px;
}}

.neon-polkit-detail-key {{
    font-size: 11.5px;
    font-weight: 700;
    color: {fg_muted};
}}

.neon-polkit-detail-val {{
    font-size: 11.5px;
    color: {fg_primary};
}}

button.neon-polkit-btn {{
    background-image: none;
    background-color: {ctx_input_bg};
    color: {fg_primary};
    border: 1px solid {ctx_border};
    border-radius: 12px;
    padding: 9px 16px;
    min-height: 36px;
    font-size: 13.5px;
    font-weight: 600;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp},
                opacity {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-polkit-btn:hover {{
    background-color: {ctx_sub_open_bg};
    transform: scale(1.02);
}}

button.neon-polkit-btn:active {{
    transform: scale(0.95);
}}

button.neon-polkit-btn.primary {{
    background-color: {accent};
    color: #FFFFFF;
    border-color: transparent;
    box-shadow: 0 6px 18px alpha({accent}, 0.34);
}}

button.neon-polkit-btn.primary:hover {{
    background-color: alpha({accent}, 0.88);
}}

button.neon-polkit-btn.primary:disabled {{
    opacity: 0.45;
    box-shadow: none;
}}

/* ===================== DEDICATED NOTIFICATION CENTER (LIQUID GLASS + CALENDAR) ===================== */

.neon-nc-panel-card {{
    background-color: {qs_panel_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    border-radius: 24px;
    padding: 16px;
    box-shadow: {specular_inset};
    color: {fg_primary};
}}

.neon-nc-date-title {{
    font-size: 15px;
    font-weight: 700;
    color: {fg_primary};
}}

button.neon-nc-dnd-btn {{
    background-image: none;
    background-color: {qs_pill_bg};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 999px;
    padding: 4px 10px;
    min-height: 26px;
    color: {fg_muted};
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-nc-dnd-btn:hover {{
    background-color: {qs_pill_hover};
    color: {fg_primary};
    transform: scale(1.03);
}}

button.neon-nc-dnd-btn.active {{
    background-color: alpha({accent}, 0.25);
    border-color: alpha({accent}, 0.62);
    color: {accent};
}}

.neon-nc-count-pill {{
    background-color: alpha({accent}, 0.22);
    border: 1px solid alpha({accent}, 0.45);
    border-radius: 999px;
    padding: 1px 8px;
    font-size: 11px;
    font-weight: 700;
    color: {accent};
}}

.neon-nc-calendar-card {{
    background-color: {qs_inner_bg};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 6px 8px;
}}

calendar.neon-nc-calendar {{
    background-color: transparent;
    border: none;
    color: {fg_primary};
    font-size: 12px;
}}

calendar.neon-nc-calendar > header {{
    background-color: transparent;
    border: none;
    color: {fg_primary};
    font-weight: 700;
    padding: 2px 4px;
}}

calendar.neon-nc-calendar > header > button {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 8px;
    color: {fg_primary};
    min-width: 24px;
    min-height: 24px;
}}

calendar.neon-nc-calendar > header > button:hover {{
    background-color: {qs_pill_hover};
}}

calendar.neon-nc-calendar > grid > label.day-name {{
    color: {fg_muted};
    font-size: 11px;
    font-weight: 600;
}}

calendar.neon-nc-calendar > grid > label.day-number {{
    border-radius: 999px;
    padding: 3px;
    color: {fg_primary};
}}

calendar.neon-nc-calendar > grid > label.day-number.other-month {{
    color: {fg_muted};
    opacity: 0.45;
}}

calendar.neon-nc-calendar > grid > label.day-number.today {{
    border: 1px solid alpha({accent}, 0.72);
    font-weight: 700;
}}

calendar.neon-nc-calendar > grid > label.day-number:selected {{
    background-color: {accent};
    color: #FFFFFF;
    font-weight: 700;
}}

.neon-nc-scroller {{
    background-color: transparent;
}}

.neon-nc-item {{
    background-color: {qs_inner_bg};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 10px 12px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-nc-item:hover {{
    background-color: {qs_pill_hover};
    border-color: {border_glass};
}}

.neon-nc-time {{
    font-size: 11px;
    color: {fg_muted};
}}

button.neon-nc-item-body-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 10px;
    padding: 3px 4px;
    margin: 0 -4px;
    color: {fg_primary};
}}

button.neon-nc-item-body-btn:hover {{
    background-color: rgba(255, 255, 255, 0.06);
}}

.neon-nc-empty {{
    padding: 20px 10px;
}}

.neon-nc-empty-icon {{
    color: {fg_muted};
    opacity: 0.75;
    margin-bottom: 2px;
}}

/* ===================== DYNAMIC ISLAND (AUTHENTIC APPLE iOS OLED + M3 EXPRESSIVE) ===================== */

.neon-island-canvas {{
    background-color: transparent;
    padding: 0;
    margin: 0;
}}

.neon-island-shell {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    border-radius: 18px;
    padding: 0;
    margin: 0;
    color: #FFFFFF;
}}

.neon-island-shell.expanded {{
    border-radius: 36px;
}}

.neon-island-shell.notch-flush {{
    border-radius: 0 0 18px 18px;
}}

.neon-island-shell.notch-flush.expanded {{
    border-radius: 0 0 28px 28px;
}}

.neon-island-clip,
.neon-island-clip > viewport,
.neon-island-stack {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
}}

button.neon-island-pill-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    padding: 2px 14px;
    min-height: 32px;
    color: #FFFFFF;
}}

.neon-ios-pill-title {{
    font-size: 15px;
    font-weight: 600;
    color: #FFFFFF;
}}

.neon-ios-charge-pct {{
    font-size: 15px;
    font-weight: 600;
    color: #30D158;
    font-feature-settings: "tnum";
}}

.neon-ios-silent-lbl {{
    font-size: 15px;
    font-weight: 500;
    color: #FF453A;
}}

.neon-ios-silent-lbl.muted-off {{
    color: #AEAEB2;
}}

.neon-island-idle-dot {{
    background-color: #8E8E93;
    border-radius: 4px;
    min-width: 7px;
    min-height: 7px;
}}

.neon-island-idle-dot.unread {{
    background-color: {accent};
    box-shadow: 0 0 8px alpha({accent}, 0.85);
}}

.neon-island-idle-clock {{
    font-size: 13px;
    font-weight: 600;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

.neon-island-unread-badge {{
    background-color: alpha({accent}, 0.28);
    border: 1px solid alpha({accent}, 0.60);
    border-radius: 10px;
    padding: 0 6px;
    font-size: 11px;
    font-weight: 700;
    color: #FFFFFF;
}}

.neon-island-rec-dot {{
    background-color: #FF3B30;
    border-radius: 4px;
    min-width: 8px;
    min-height: 8px;
    box-shadow: 0 0 8px rgba(255, 59, 48, 0.85);
}}

.neon-island-rec-time {{
    font-size: 13px;
    font-weight: 600;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

.neon-island-rec-tag {{
    background-color: rgba(255, 59, 48, 0.24);
    border: 1px solid rgba(255, 69, 58, 0.55);
    border-radius: 10px;
    padding: 0 6px;
    font-size: 10px;
    font-weight: 700;
    color: #FF6961;
}}

.neon-island-mini-art {{
    border-radius: 6px;
    background-color: #1C1C1E;
    border: none;
}}

.neon-island-media-pill-title {{
    font-size: 13px;
    font-weight: 600;
    color: #FFFFFF;
}}

.neon-island-page {{
    padding: 14px 16px;
}}

/* Authentic Apple iOS Expanded Now Playing Card */
.neon-island-media-card {{
    padding: 20px 22px 16px 22px;
}}

.neon-island-clip-card {{
    padding: 14px 14px 12px 16px;
}}

.neon-island-title-lbl {{
    font-size: 14px;
    font-weight: 600;
    color: #FFFFFF;
}}

.neon-island-media-title {{
    font-size: 16px;
    font-weight: 600;
    color: #FFFFFF;
}}

.neon-island-media-artist {{
    font-size: 14px;
    font-weight: 400;
    color: #AEAEB2;
}}

.neon-island-media-time {{
    font-size: 12px;
    font-weight: 500;
    color: #AEAEB2;
    font-feature-settings: "tnum";
}}

.neon-island-sub-lbl {{
    font-size: 12px;
    color: #AEAEB2;
}}

.neon-island-time-tag {{
    font-size: 11px;
    color: #8E8E93;
}}

.neon-island-hero-time {{
    font-size: 21px;
    font-weight: 700;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

.neon-island-status-chip {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 12px;
    padding: 4px 10px;
    font-size: 12px;
    font-weight: 600;
    color: #FFFFFF;
}}

button.neon-ios-frameless-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    padding: 0;
    margin: 0;
    color: #FFFFFF;
    transition: opacity {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-ios-frameless-btn:hover {{
    background-color: transparent;
    opacity: 0.86;
}}

button.neon-ios-transport-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 14px;
    padding: 0;
    color: #FFFFFF;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp},
                opacity {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-ios-transport-btn:hover {{
    background-color: rgba(255, 255, 255, 0.10);
    transform: scale(1.06);
}}

button.neon-ios-transport-btn:active {{
    background-color: rgba(255, 255, 255, 0.16);
    transform: scale(0.88);
}}

/* Authentic Apple iOS Notification / Call Pill */
.neon-ios-notif-page {{
    padding: 15px 16px;
}}

.neon-ios-notif-avatar {{
    border-radius: 22px;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
}}

.neon-ios-notif-eyebrow {{
    font-size: 12px;
    font-weight: 400;
    color: #AEAEB2;
}}

.neon-ios-notif-title {{
    font-size: 15px;
    font-weight: 600;
    color: #FFFFFF;
}}

.neon-ios-notif-body {{
    font-size: 12px;
    font-weight: 400;
    color: #AEAEB2;
}}

button.neon-ios-circle-red {{
    background-image: none;
    background-color: #FF3B30;
    border: none;
    box-shadow: none;
    border-radius: 19px;
    min-width: 38px;
    min-height: 38px;
    padding: 0;
    color: #FFFFFF;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-ios-circle-red:hover {{
    background-color: #FF453A;
    transform: scale(1.06);
}}

button.neon-ios-circle-red:active {{
    transform: scale(0.92);
}}

button.neon-ios-circle-green {{
    background-image: none;
    background-color: #34C759;
    border: none;
    box-shadow: none;
    border-radius: 19px;
    min-width: 38px;
    min-height: 38px;
    padding: 0;
    color: #FFFFFF;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-ios-circle-green:hover {{
    background-color: #30D158;
    transform: scale(1.06);
}}

button.neon-ios-circle-green:active {{
    transform: scale(0.92);
}}

button.neon-island-close-btn {{
    background-image: none;
    background-color: #1C1C1E;
    border: none;
    box-shadow: none;
    border-radius: 12px;
    min-width: 24px;
    min-height: 24px;
    padding: 0;
    color: #AEAEB2;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-island-close-btn:hover {{
    background-color: #2C2C2E;
    color: #FFFFFF;
    transform: scale(1.06);
}}

button.neon-island-icon-pill {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: none;
    border-radius: 15px;
    min-width: 28px;
    min-height: 28px;
    padding: 0;
    color: #E5E5EA;
    transition: background-color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-island-icon-pill:hover {{
    background-color: #2C2C2E;
    color: #FFFFFF;
}}

button.neon-island-icon-pill.active {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.16);
    color: #FFFFFF;
}}

button.neon-island-notif-body-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 12px;
    padding: 4px 6px;
    margin: 0 -6px;
    color: #FFFFFF;
}}

button.neon-island-notif-body-btn:hover {{
    background-color: #1C1C1E;
}}

.neon-island-rec-emblem {{
    background-color: rgba(255, 59, 48, 0.20);
    border: 1px solid rgba(255, 69, 58, 0.45);
    border-radius: 18px;
}}

button.neon-island-stop-btn {{
    background-image: none;
    background-color: #FF3B30;
    border: none;
    box-shadow: none;
    border-radius: 16px;
    padding: 6px 15px;
    color: #FFFFFF;
    font-size: 13px;
    font-weight: 600;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-island-stop-btn:hover {{
    background-color: #FF453A;
    transform: scale(1.04);
}}

.neon-island-art {{
    background-color: #1C1C1E;
    border: none;
    border-radius: 14px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.45);
}}

.neon-island-art-fallback {{
    color: #FFFFFF;
}}

button.neon-island-action-pill {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: none;
    border-radius: 14px;
    padding: 5px 11px;
    min-height: 30px;
    color: #FFFFFF;
    font-size: 12px;
    font-weight: 600;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-island-action-pill:hover {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.16);
}}

button.neon-island-action-pill.active {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.22);
    color: #FFFFFF;
}}

.neon-island-mini-strip,
button.neon-island-mini-strip-btn {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    box-shadow: none;
    border-radius: 12px;
    padding: 6px 10px;
    color: #FFFFFF;
}}

button.neon-island-mini-strip-btn:hover {{
    background-color: #2C2C2E;
}}

/* ---- Scrollable Clipboard Inside Dynamic Island ---- */
.neon-island-clip-emblem {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 10px;
    color: #FFFFFF;
}}

.neon-island-count-chip {{
    background-color: #2C2C2E;
    border-radius: 9px;
    padding: 1px 7px;
    font-size: 11px;
    font-weight: 600;
    color: #E5E5EA;
    font-feature-settings: "tnum";
}}

.neon-island-kbd-chip {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 7px;
    padding: 2px 6px;
    font-size: 10px;
    font-weight: 600;
    color: #8E8E93;
}}

button.neon-island-clear-btn {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: none;
    border-radius: 10px;
    padding: 2px 9px;
    min-height: 24px;
    font-size: 11px;
    font-weight: 600;
    color: #AEAEB2;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-island-clear-btn:hover {{
    background-color: rgba(255, 59, 48, 0.24);
    border-color: rgba(255, 69, 58, 0.45);
    color: #FF6961;
}}

.neon-island-clip-scroller,
.neon-island-clip-scroller > viewport {{
    background-color: transparent;
    background-image: none;
    border: none;
}}

.neon-island-clip-scroller scrollbar {{
    background-color: transparent;
    border: none;
    padding: 2px;
}}

.neon-island-clip-scroller scrollbar slider {{
    background-color: #3A3A3C;
    border-radius: 5px;
    min-width: 5px;
    min-height: 28px;
}}

.neon-island-clip-scroller scrollbar slider:hover {{
    background-color: #636366;
}}

.neon-island-clip-list {{
    padding-right: 4px;
    padding-bottom: 4px;
}}

.neon-island-clip-row {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 14px;
    padding: 7px 8px 7px 10px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-island-clip-row:hover {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.12);
}}

.neon-island-clip-row.selected {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.22);
}}

.neon-island-clip-row.pinned {{
    border-color: alpha({accent}, 0.42);
}}

.neon-island-clip-row.copied {{
    background-color: rgba(48, 209, 88, 0.18);
    border-color: rgba(48, 209, 88, 0.65);
}}

button.neon-island-clip-body-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    padding: 0;
    color: #FFFFFF;
}}

.neon-island-clip-preview {{
    font-size: 13px;
    font-weight: 500;
    color: #FFFFFF;
}}

.neon-island-clip-meta {{
    font-size: 11px;
    font-weight: 500;
    color: #AEAEB2;
}}

.neon-island-clip-pin-tag {{
    background-color: alpha({accent}, 0.22);
    border-radius: 6px;
    padding: 0 5px;
    font-size: 10px;
    font-weight: 700;
    color: {accent};
}}

.neon-island-clip-copied-tag {{
    background-color: rgba(48, 209, 88, 0.24);
    border-radius: 6px;
    padding: 0 6px;
    font-size: 10px;
    font-weight: 700;
    color: #30D158;
}}

button.neon-island-clip-pin-btn,
button.neon-island-clip-del-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 10px;
    min-width: 24px;
    min-height: 24px;
    padding: 0;
    color: #8E8E93;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-island-clip-pin-btn:hover {{
    background-color: #2C2C2E;
    color: #FFFFFF;
    transform: scale(1.06);
}}

button.neon-island-clip-pin-btn.pinned {{
    background-color: alpha({accent}, 0.22);
    color: {accent};
}}

button.neon-island-clip-del-btn:hover {{
    background-color: rgba(255, 59, 48, 0.24);
    color: #FF6961;
    transform: scale(1.06);
}}

.neon-island-clip-empty {{
    padding: 34px 16px;
}}

.neon-island-empty-icon {{
    color: #636366;
    margin-bottom: 4px;
}}

/* ---- Pure OLED Black Floating Bottom Switcher Pill & 12-Module Suite ---- */

.neon-notch-dock-pill {{
    background-color: transparent;
    background-image: none;
    border: none;
    box-shadow: none;
    padding: 0;
}}

.neon-notch-dock-sep {{
    background-color: rgba(255, 255, 255, 0.16);
    min-width: 1px;
    min-height: 16px;
    margin: 0 2px;
}}

button.neon-notch-dock-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 14px;
    min-width: 34px;
    min-height: 28px;
    padding: 0;
    color: #98989F;
    transition: background-color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-notch-dock-btn:hover {{
    background-color: #1C1C1E;
    color: #FFFFFF;
}}

button.neon-notch-dock-btn.active {{
    background-color: #2C2C2E;
    color: #FFFFFF;
}}

.neon-island-page-pad {{
    padding: 16px 20px 16px 20px;
}}

.neon-island-vsep {{
    background-color: rgba(255, 255, 255, 0.10);
    min-width: 1px;
    margin: 4px 0;
}}

.neon-island-hero-clock {{
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.5px;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

.neon-island-eyebrow {{
    font-size: 11px;
    font-weight: 600;
    color: #AEAEB2;
}}

.neon-island-muted-xs {{
    font-size: 11px;
    font-weight: 500;
    color: #AEAEB2;
}}

.neon-island-hud-name {{
    font-size: 12px;
    font-weight: 600;
    color: #FFFFFF;
}}

.neon-island-hud-pct {{
    font-size: 12px;
    font-weight: 600;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

.neon-island-timer-pill {{
    font-size: 14px;
    font-weight: 700;
    color: #FF9F0A;
    font-feature-settings: "tnum";
}}

.neon-island-timer-hero {{
    font-size: 32px;
    font-weight: 700;
    color: #FF9F0A;
    letter-spacing: -0.5px;
    font-feature-settings: "tnum";
}}

.neon-island-timer-hero.break {{
    color: #30D158;
}}

button.neon-island-mode-chip {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    box-shadow: none;
    border-radius: 12px;
    padding: 3px 11px;
    min-height: 24px;
    font-size: 12px;
    font-weight: 600;
    color: #AEAEB2;
}}

button.neon-island-mode-chip.focus-active {{
    background-color: rgba(255, 159, 10, 0.22);
    border-color: rgba(255, 159, 10, 0.55);
    color: #FF9F0A;
}}

button.neon-island-mode-chip.break-active {{
    background-color: rgba(48, 209, 88, 0.20);
    border-color: rgba(48, 209, 88, 0.55);
    color: #30D158;
}}

button.neon-island-timer-start {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow: none;
    border-radius: 14px;
    padding: 5px 15px;
    min-height: 30px;
    font-size: 13px;
    font-weight: 600;
    color: #FFFFFF;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-island-timer-start:hover {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.22);
}}

entry.neon-island-search,
searchbar.neon-island-search,
searchentry.neon-island-search {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 10px;
    padding: 3px 9px;
    min-height: 26px;
    font-size: 12px;
    color: #FFFFFF;
    box-shadow: none;
}}

entry.neon-island-search:focus,
searchentry.neon-island-search:focus {{
    border-color: alpha({accent}, 0.72);
    background-color: #2C2C2E;
}}

button.neon-island-filter-pill {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 9px;
    padding: 2px 9px;
    min-height: 24px;
    font-size: 11px;
    font-weight: 600;
    color: #8E8E93;
}}

button.neon-island-filter-pill:hover {{
    background-color: #1C1C1E;
    color: #FFFFFF;
}}

button.neon-island-filter-pill.active {{
    background-color: #2C2C2E;
    color: #FFFFFF;
}}

.neon-island-hscroller,
.neon-island-hscroller > viewport {{
    background-color: transparent;
    border: none;
}}

.neon-island-clip-cards,
.neon-island-shelf-strip {{
    padding-bottom: 8px;
}}

.neon-island-hscroller scrollbar {{
    background-color: transparent;
    border: none;
    box-shadow: none;
    margin-top: 2px;
}}

.neon-island-hscroller scrollbar trough {{
    background-color: transparent;
    border: none;
    min-height: 4px;
}}

.neon-island-hscroller scrollbar slider {{
    background-color: rgba(255, 255, 255, 0.24);
    border: none;
    border-radius: 4px;
    min-height: 4px;
    min-width: 28px;
}}

.neon-island-hscroller scrollbar slider:hover {{
    background-color: rgba(255, 255, 255, 0.42);
}}

.neon-notch-clip-card {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 14px;
    padding: 10px 10px 7px 10px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-notch-clip-card:hover {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.14);
}}

.neon-notch-clip-card.pinned {{
    border-color: alpha({accent}, 0.52);
}}

.neon-notch-clip-card.copied {{
    background-color: rgba(48, 209, 88, 0.18);
    border-color: rgba(48, 209, 88, 0.65);
}}

.neon-notch-clip-code {{
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 11px;
    color: #64D2FF;
}}

.neon-notch-type-tag {{
    background-color: #2C2C2E;
    border-radius: 6px;
    padding: 1px 5px;
    font-size: 10px;
    font-weight: 600;
    color: #AEAEB2;
}}

button.neon-notch-file-tile {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    box-shadow: none;
    border-radius: 13px;
    padding: 6px;
    color: #FFFFFF;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-notch-file-tile:hover {{
    background-color: #2C2C2E;
}}

button.neon-notch-file-tile.selected {{
    background-color: alpha({accent}, 0.22);
    border-color: alpha({accent}, 0.72);
}}

.neon-notch-file-thumb {{
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.12);
}}

.neon-notch-file-icon {{
    color: #64D2FF;
}}

.neon-notch-file-name {{
    font-size: 11px;
    font-weight: 600;
    color: #FFFFFF;
}}

button.neon-island-mini-btn {{
    background-image: none;
    background-color: #2C2C2E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: none;
    border-radius: 8px;
    padding: 2px 8px;
    min-height: 22px;
    font-size: 11px;
    font-weight: 600;
    color: #FFFFFF;
}}

button.neon-island-mini-btn:hover {{
    background-color: #3A3A3C;
}}

button.neon-notch-note-item {{
    background-image: none;
    background-color: transparent;
    border: 1px solid transparent;
    box-shadow: none;
    border-radius: 10px;
    padding: 6px 8px;
    color: #FFFFFF;
}}

button.neon-notch-note-item:hover {{
    background-color: #1C1C1E;
}}

button.neon-notch-note-item.active {{
    background-color: rgba(255, 214, 10, 0.16);
    border-color: rgba(255, 214, 10, 0.42);
}}

textview.neon-island-note-editor,
textview.neon-island-note-editor > text {{
    background-color: transparent;
    color: #FFFFFF;
    font-size: 13px;
    caret-color: #FFD60A;
}}

button.neon-notch-cal-day {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    box-shadow: none;
    border-radius: 12px;
    padding: 5px 4px;
    color: #FFFFFF;
}}

button.neon-notch-cal-day:hover {{
    background-color: #2C2C2E;
}}

button.neon-notch-cal-day.today {{
    background-color: rgba(255, 59, 48, 0.24);
    border-color: rgba(255, 69, 58, 0.65);
}}

button.neon-notch-cal-day.selected {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.35);
}}

.neon-notch-cal-num {{
    font-size: 14px;
    font-weight: 700;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

.neon-notch-cal-dot {{
    background-color: #FF453A;
    border-radius: 3px;
    min-width: 5px;
    min-height: 5px;
}}

.neon-notch-cal-event {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 11px;
    padding: 6px 10px;
}}

.neon-notch-cal-bar {{
    background-color: #FF453A;
    border-radius: 2px;
    min-width: 3px;
    min-height: 22px;
}}

button.neon-notch-tool-tile {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.06);
    box-shadow: none;
    border-radius: 14px;
    padding: 12px 8px;
    color: #FFFFFF;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                transform {d_fast_sp}ms {m3_fast_sp};
}}

button.neon-notch-tool-tile:hover {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.16);
}}

.neon-island-calc-input-box {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 14px;
    padding: 8px 14px;
}}

.neon-island-calc-input-box:focus-within {{
    border-color: alpha({accent}, 0.58);
}}

entry.neon-island-calc-entry {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    outline: none;
    padding: 0;
    min-height: 28px;
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 16px;
    font-weight: 500;
    color: #FFFFFF;
    caret-color: {accent};
}}

entry.neon-island-calc-entry text placeholder {{
    color: rgba(255, 255, 255, 0.22);
}}

button.neon-island-calc-ans-box {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: none;
    border-radius: 14px;
    padding: 8px 14px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-island-calc-ans-box:hover {{
    background-color: #2C2C2E;
    border-color: rgba(255, 255, 255, 0.16);
}}

.neon-island-calc-eq {{
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 20px;
    font-weight: 700;
    color: #8E8E93;
}}

.neon-island-calc-hero {{
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 24px;
    font-weight: 700;
    color: #30D158;
    font-feature-settings: "tnum";
}}

.neon-island-calc-hero.empty {{
    color: #48484A;
}}

.neon-island-stat-val {{
    font-size: 20px;
    font-weight: 700;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

.neon-island-stat-track {{
    background-color: #3A3A3C;
    border-radius: 4px;
    min-height: 6px;
}}

.neon-island-stat-fill {{
    background-color: #0A84FF;
    border-radius: 4px;
    min-height: 6px;
}}

.neon-notch-bt-dot {{
    background-color: #30D158;
    border-radius: 4px;
    min-width: 7px;
    min-height: 7px;
}}

.neon-island-awake-hero {{
    font-size: 36px;
    font-weight: 700;
    letter-spacing: -1px;
    color: #FFFFFF;
    font-feature-settings: "tnum";
}}

button.neon-island-awake-btn {{
    background-image: none;
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.12);
    box-shadow: none;
    border-radius: 16px;
    padding: 8px 20px;
    font-size: 13px;
    font-weight: 600;
    color: #FFFFFF;
}}

button.neon-island-awake-btn.active {{
    background-color: #FF9F0A;
    border-color: transparent;
    color: #000000;
}}

scale.neon-island-slider {{
    padding: 8px 0;
    margin: 0;
}}

scale.neon-island-slider > trough {{
    background-image: none;
    background-color: #3A3A3C;
    border: none;
    box-shadow: none;
    outline: none;
    border-radius: 999px;
    min-height: 6px;
}}

scale.neon-island-slider > trough > highlight {{
    background-image: none;
    background-color: #FFFFFF;
    border: none;
    box-shadow: none;
    margin: 0;
    border-radius: 999px;
}}

scale.neon-island-slider > trough > slider,
scale.neon-island-slider > trough > slider:hover,
scale.neon-island-slider > trough > slider:active {{
    background-image: none;
    background-color: #FFFFFF;
    border: none;
    outline: none;
    border-radius: 999px;
    min-width: 16px;
    min-height: 16px;
    margin: -5px 0;
    padding: 0;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.52);
}}

.neon-island-store-pad {{
    padding: 14px 16px;
}}

flowbox.neon-notch-store-grid,
flowbox.neon-notch-store-grid > flowboxchild {{
    padding: 0;
    margin: 0;
    background: transparent;
    border: none;
    outline: none;
}}

.neon-notch-style-seg {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 11px;
    padding: 2px;
}}

.neon-notch-store-card {{
    background-color: #1C1C1E;
    border: 1px solid rgba(255, 255, 255, 0.07);
    border-radius: 14px;
    padding: 7px 9px;
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                border-color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-notch-store-card:hover {{
    background-color: #262629;
    border-color: rgba(255, 255, 255, 0.16);
}}

.neon-notch-store-title {{
    font-size: 13px;
    font-weight: 600;
    color: #FFFFFF;
}}

.neon-notch-store-sub {{
    font-size: 11px;
    font-weight: 500;
    color: #AEAEB2;
}}

/* Apple iOS-style vivid colored module badges */
.neon-notch-mod-badge {{
    border-radius: 8px;
    color: #FFFFFF;
}}
.neon-notch-mod-badge.badge-blue {{ background-color: #0A84FF; }}
.neon-notch-mod-badge.badge-pink {{ background-color: #FF2D55; }}
.neon-notch-mod-badge.badge-amber {{ background-color: #FF9F0A; }}
.neon-notch-mod-badge.badge-violet {{ background-color: #5E5CE6; }}
.neon-notch-mod-badge.badge-sky {{ background-color: #64D2FF; }}
.neon-notch-mod-badge.badge-yellow {{ background-color: #FFD60A; }}
.neon-notch-mod-badge.badge-coral {{ background-color: #FF3B30; }}
.neon-notch-mod-badge.badge-taupe {{ background-color: #8E8E93; }}
.neon-notch-mod-badge.badge-teal {{ background-color: #30B0C7; }}
.neon-notch-mod-badge.badge-green {{ background-color: #30D158; }}
.neon-notch-mod-badge.badge-orange {{ background-color: #FF9500; }}
.neon-notch-mod-badge.badge-cyan {{ background-color: #32ADE6; }}

/* =========================================================================
 * DOCK HOVER LIVE WINDOW PREVIEW POPUP (Unified Single-Box Frosted Glass)
 * ========================================================================= */
window.neon-dock-preview-window {{
    background-color: transparent;
}}

.neon-dock-preview-card {{
    background-color: {tahoe_glass_bg};
    border: 1px solid {border_subtle};
    border-radius: 14px;
    padding: 8px;
    color: {fg_primary};
}}

.neon-dock-preview-card.multi {{
    padding: 6px;
}}

.neon-dock-preview-item {{
    background-color: transparent;
    border: none;
    border-radius: 9px;
    padding: 0;
    transition: background-color {d_fast_fx}ms {m3_fast_fx};
}}

.neon-dock-preview-card.multi .neon-dock-preview-item {{
    padding: 4px;
}}

.neon-dock-preview-card.multi .neon-dock-preview-item:hover {{
    background-color: {tahoe_card_bg};
}}

.neon-dock-preview-header {{
    padding: 0 2px;
}}

.neon-dock-preview-sep {{
    background-color: {border_subtle};
    min-width: 1px;
    margin: 4px 0;
}}

.neon-dock-preview-title {{
    font-size: 12px;
    font-weight: 600;
    color: {fg_primary};
}}

.neon-dock-preview-badge {{
    background-color: {tahoe_card_bg};
    border: none;
    border-radius: 999px;
    padding: 1px 6px;
    font-size: 10px;
    font-weight: 600;
    color: {fg_muted};
}}

button.neon-dock-preview-close {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 20px;
    min-height: 20px;
    padding: 0;
    color: {fg_muted};
    transition: background-color {d_fast_fx}ms {m3_fast_fx},
                color {d_fast_fx}ms {m3_fast_fx};
}}

button.neon-dock-preview-close:hover {{
    background-color: rgba(239, 68, 68, 0.86);
    color: #FFFFFF;
}}

.neon-dock-preview-thumb-frame {{
    background-color: transparent;
    border: none;
    border-radius: 8px;
}}

.neon-dock-preview-placeholder-lbl {{
    font-size: 11px;
    font-weight: 500;
    color: {fg_muted};
}}
"#
    )
}
