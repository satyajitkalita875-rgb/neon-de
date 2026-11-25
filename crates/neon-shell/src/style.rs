//! NEON DE macOS Tahoe ("Liquid Glass") Visual Design Language — Neon Dark & Neon Light themes,
//! GPU-blurred translucent glass backdrops, specular luminous rims, spring open/close animations,
//! authentic macOS menu bar & Control Center, floating glass dock, and blurred Launchpad.

use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, gdk::Display};
use neon_ipc::AppearanceInfo;

/// Holds the active `CssProvider` so theme updates apply instantaneously with zero restart.
pub struct ShellThemeController {
    provider: CssProvider,
    last_applied: Option<AppearanceInfo>,
    last_synced_light: Option<bool>,
}

impl ShellThemeController {
    pub fn init() -> Self {
        let provider = CssProvider::new();
        let default_appearance = AppearanceInfo::default();
        provider.load_from_string(&build_shell_css(&default_appearance));
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
            last_synced_light: None,
        }
    }

    pub fn apply_if_changed(&mut self, appearance: &AppearanceInfo) {
        if self.last_applied.as_ref() == Some(appearance) {
            return;
        }
        apply_gtk_settings(appearance);
        self.provider.load_from_string(&build_shell_css(appearance));
        self.last_applied = Some(appearance.clone());

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
}

fn apply_gtk_settings(appearance: &AppearanceInfo) {
    let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
    if let Some(settings) = gtk4::Settings::default() {
        let icon_theme = if is_light {
            "WhiteSur-light"
        } else {
            "WhiteSur-dark"
        };
        settings.set_gtk_icon_theme_name(Some(icon_theme));
        settings.set_gtk_application_prefer_dark_theme(!is_light);
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

pub fn build_shell_css(appearance: &AppearanceInfo) -> String {
    let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
    let high_contrast = appearance.high_contrast;
    let reduce_blur = appearance.reduce_blur || appearance.blur_strength <= 0.01;
    let blur_s = appearance.blur_strength.clamp(0.0, 2.0);
    let accent = &appearance.accent_color;
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

    let _ = (tahoe_glass_bg, tahoe_blur_filter, launchpad_blur_filter, slider_fill, popover_bg);

    format!(
        r#"
* {{
    font-family: "Adwaita Sans", "Inter", "SF Pro Text", sans-serif;
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
window.neon-dialog-window {{
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
    transition: background-color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                border-color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                box-shadow 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                transform 200ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                opacity 180ms ease;
}}

.neon-top-panel.panel-hidden {{
    transform: translateY(-34px);
    opacity: 0;
}}

button.neon-rec-pill {{
    background-image: none;
    background-color: rgba(220, 38, 38, 0.90);
    color: #FFFFFF;
    border: 1px solid rgba(255, 255, 255, 0.28);
    box-shadow: 0 2px 8px rgba(220, 38, 38, 0.42);
    border-radius: 999px;
    padding: 1px 10px;
    min-height: 22px;
    margin-right: 6px;
    font-size: 11px;
    font-weight: 700;
    transition: background-color 140ms ease, transform 120ms ease;
}}

button.neon-rec-pill:hover {{
    background-color: #EF4444;
    transform: scale(1.03);
}}

.neon-rec-dot {{
    background-color: #FFFFFF;
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
.neon-top-panel.drawer-open button.neon-clock-btn,
.neon-top-panel.drawer-open button.neon-tray-btn,
.neon-top-panel.drawer-open .neon-app-title {{
    color: #FFFFFF;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.45);
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
    transition: background-color 140ms ease;
}}

button.neon-brand-button:hover,
button.neon-brand-button.active {{
    background-color: {tahoe_card_hover};
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
    transition: all 180ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

button.neon-ws-dot:hover {{
    background-color: {fg_primary};
    min-width: 12px;
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
    transition: background-color 140ms ease;
}}

button.neon-clock-btn:hover {{
    background-color: {tahoe_card_hover};
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
    transition: background-color 150ms ease, transform 140ms ease;
}}

button.neon-tray-btn:hover {{
    background-color: {tahoe_card_hover};
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
    padding: 7px 20px 5px 20px;
}}

.neon-dock-container {{
    background-color: {dock_bg};
    border: 1px solid {border_glass};
    border-radius: 20px;
    padding: 5px 11px 4px 11px;
    box-shadow: 0 3px 10px rgba(0, 0, 0, 0.28), {specular_inset};
    transition: background-color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                border-color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                box-shadow 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                transform 210ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                opacity 190ms ease;
}}

.neon-dock-container.dock-hidden {{
    transform: translateY(96px);
    opacity: 0;
}}

.neon-dock-container.drawer-open {{
    background-color: transparent;
    border-color: transparent;
    box-shadow: none;
}}

.neon-dock-container.drawer-open .neon-dock-separator {{
    background-color: rgba(255, 255, 255, 0.26);
}}

.neon-dock-wrapper.dock-vertical {{
    padding: 16px 7px;
}}

.neon-dock-container.dock-vertical {{
    padding: 8px 6px;
}}

.neon-dock-container.dock-vertical .neon-dock-separator {{
    min-width: 24px;
    min-height: 1px;
    margin: 5px 8px;
}}

button.neon-dock-item {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 12px;
    padding: 3px 5px 2px 5px;
    min-width: 24px;
    min-height: 26px;
    color: {fg_primary};
    transition: transform 180ms cubic-bezier(0.22, 1.0, 0.36, 1.0), background-color 150ms ease;
}}

.neon-dock-container.dock-compact {{
    border-radius: 15px;
    padding: 3px 7px 2px 7px;
}}

.neon-dock-container.dock-compact button.neon-dock-item {{
    border-radius: 9px;
    padding: 2px 3px 1px 3px;
    min-width: 22px;
    min-height: 24px;
}}

.neon-dock-container.dock-compact .neon-dock-dot {{
    min-width: 4px;
    min-height: 4px;
    margin-top: 2px;
}}

.neon-dock-container.dock-compact .neon-dock-dot.focused {{
    min-width: 10px;
}}

.neon-dock-container.dock-compact .neon-dock-separator {{
    margin: 5px 3px 6px 3px;
}}

button.neon-dock-item:hover {{
    background-color: rgba(255, 255, 255, 0.12);
}}

.neon-dock-container.dock-magnify button.neon-dock-item:hover {{
    transform: translateY(-6px) scale(1.18);
}}

.neon-dock-container.dock-vertical.dock-magnify button.neon-dock-item:hover {{
    transform: scale(1.18);
}}

button.neon-dock-item:active {{
    transform: translateY(-1px) scale(0.97);
}}

button.neon-dock-item.focused {{
    background-color: transparent;
}}

.neon-dock-dot {{
    background-color: transparent;
    border-radius: 999px;
    min-width: 5px;
    min-height: 5px;
    margin-top: 3px;
    transition: all 180ms ease;
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
    margin: 8px 5px 10px 5px;
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
    transition: background-color 180ms ease, color 180ms ease, box-shadow 180ms ease;
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
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.22), inset 0 1px 0 #FFFFFF;
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
    transition: transform 140ms ease;
}}

scale.neon-cc-slider:hover > trough > slider {{
    transform: scale(1.10);
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

/* ===================== MACOS LAUNCHPAD (FULLSCREEN BLUR + ZOOM ANIMATION) ===================== */

.neon-launchpad-overlay {{
    transition: opacity 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
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
    padding: 36px 64px 88px 64px;
    color: #FFFFFF;
}}

.neon-launchpad-content {{
    transition: transform 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                opacity 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-launchpad-overlay.lp-closed .neon-launchpad-content {{
    opacity: 0;
    transform: scale(1.09);
}}

.neon-launchpad-overlay.lp-open .neon-launchpad-content {{
    opacity: 1;
    transform: scale(1.0);
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
    transition: transform 180ms cubic-bezier(0.22, 1.0, 0.36, 1.0), background-color 150ms ease;
}}

button.neon-launchpad-tile:hover {{
    background-color: rgba(255, 255, 255, 0.12);
    transform: scale(1.09);
}}

button.neon-launchpad-tile:active {{
    transform: scale(0.95);
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
    transition: background-color 180ms ease, transform 180ms ease;
}}

button.neon-page-dot:hover {{
    background-color: rgba(255, 255, 255, 0.72);
    transform: scale(1.20);
}}

button.neon-page-dot.active {{
    background-color: #FFFFFF;
    min-width: 8px;
    min-height: 8px;
    transform: scale(1.12);
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

/* ===================== COSMIC-STYLE LEFT WORKSPACES SIDEBAR ===================== */

.neon-ws-sidebar {{
    background-color: {qs_panel_bg};
    background-image: none;
    border: 1px solid {border_glass};
    border-radius: 16px;
    padding: 9px 8px 9px 8px;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.34), {specular_inset};
    color: {fg_primary};
}}

.neon-ws-scroller,
.neon-ws-scroller > viewport {{
    background-color: transparent;
    background-image: none;
    border: none;
}}

.neon-ws-scroller scrollbar {{
    background-color: transparent;
    border: none;
}}

.neon-ws-scroller scrollbar slider {{
    background-color: rgba(255, 255, 255, 0.22);
    border-radius: 999px;
    min-width: 4px;
    min-height: 24px;
}}

.neon-ws-cards-list {{
    padding: 1px 1px;
}}

.neon-ws-card-sep {{
    background-color: {border_subtle};
    min-height: 1px;
    margin: 2px 6px;
}}

.neon-ws-cosmic-card {{
    background-color: transparent;
    border: 2.5px solid transparent;
    border-radius: 11px;
    padding: 5px 5px 7px 5px;
    transition: background-color 150ms ease,
                border-color 160ms ease,
                transform 160ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-ws-cosmic-card:hover {{
    background-color: {qs_pill_bg};
    border-color: {border_subtle};
}}

.neon-ws-cosmic-card.active {{
    background-color: alpha({accent}, 0.11);
    border-color: {accent};
    box-shadow: none;
}}

.neon-ws-thumb-frame {{
    background-color: rgba(10, 14, 24, 0.72);
    border: 1px solid {border_subtle};
    border-radius: 7px;
}}

.neon-ws-cosmic-card.active .neon-ws-thumb-frame {{
    border-color: rgba(255, 255, 255, 0.18);
}}

.neon-ws-cosmic-footer {{
    margin-top: 3px;
    padding: 0 2px;
}}

.neon-ws-cosmic-label {{
    color: {fg_muted};
    font-size: 12px;
    font-weight: 500;
    letter-spacing: 0.1px;
}}

.neon-ws-cosmic-label.active {{
    color: {fg_primary};
    font-weight: 600;
}}

button.neon-ws-pin-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    padding: 0;
    margin: 0;
    transition: background-color 150ms ease, transform 160ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
}}

button.neon-ws-pin-btn:hover {{
    background-color: {qs_pill_hover};
    transform: scale(1.08);
}}

button.neon-ws-pin-btn:active {{
    transform: scale(0.92);
}}

button.neon-ws-pin-btn.pinned {{
    background-color: {accent};
    box-shadow: 0 1px 5px alpha({accent}, 0.45);
}}

button.neon-ws-pin-btn.pinned:hover {{
    background-color: {accent};
    opacity: 0.92;
    transform: scale(1.08);
}}

button.neon-ws-thumb-close {{
    background-image: none;
    background-color: rgba(10, 14, 24, 0.64);
    color: rgba(255, 255, 255, 0.90);
    border: 1px solid rgba(255, 255, 255, 0.18);
    box-shadow: none;
    border-radius: 999px;
    min-width: 19px;
    min-height: 19px;
    padding: 0;
    font-size: 12px;
    font-weight: 700;
    opacity: 0;
    transition: opacity 150ms ease, background-color 140ms ease, transform 140ms ease;
}}

.neon-ws-cosmic-card:hover button.neon-ws-thumb-close {{
    opacity: 0.92;
}}

button.neon-ws-thumb-close:hover {{
    opacity: 1.0;
    background-color: rgba(239, 68, 68, 0.92);
    border-color: rgba(255, 255, 255, 0.28);
    color: #FFFFFF;
    transform: scale(1.08);
}}

button.neon-ws-cosmic-add-btn {{
    background-image: none;
    background-color: {qs_pill_bg};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 10px;
    padding: 7px 10px;
    min-height: 30px;
    font-size: 12px;
    font-weight: 600;
    transition: background-color 150ms ease, border-color 150ms ease, transform 140ms ease;
}}

button.neon-ws-cosmic-add-btn:hover {{
    background-color: {qs_pill_hover};
    border-color: {accent};
}}

button.neon-ws-cosmic-add-btn:active {{
    transform: scale(0.97);
}}

button.neon-ws-cosmic-add-btn:disabled {{
    opacity: 0.42;
}}

/* ===================== NOTIFICATION TOASTS ===================== */

.neon-toast-card {{
    background-color: {popover_bg};
    border: 1px solid {border_glass};
    border-radius: 20px;
    padding: 12px 16px;
    color: {fg_primary};
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.42), {specular_inset};
    min-width: 320px;
}}

.neon-toast-card.critical {{
    border-color: #EF4444;
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
    transition: background-color 180ms ease,
                border-color 180ms ease,
                transform 280ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
}}

button.neon-qs-header-power-btn:hover {{
    background-color: {qs_pill_hover};
    border-color: {cc_glass_border};
    transform: scale(1.04);
}}

button.neon-qs-header-power-btn:active {{
    transform: scale(0.94);
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
    transition: background-color 220ms ease,
                border-color 220ms ease,
                border-radius 320ms cubic-bezier(0.34, 1.48, 0.46, 1.0),
                transform 280ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
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
.neon-qs-split-pill.active button.neon-qs-split-main,
.neon-qs-split-pill.active button.neon-qs-split-arrow {{
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
    transition: background-color 150ms ease, transform 240ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
}}

button.neon-qs-split-main:hover {{
    background-color: rgba(255, 255, 255, 0.11);
}}

button.neon-qs-split-main:active {{
    transform: scale(0.96);
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
    transition: background-color 150ms ease, transform 240ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
}}

button.neon-qs-split-arrow:hover {{
    background-color: rgba(255, 255, 255, 0.15);
}}

button.neon-qs-split-arrow:active {{
    transform: scale(0.90);
}}

/* Material 3 Expressive Elastic Chevron Flip */
.neon-qs-chevron {{
    transform: rotate(0deg);
    transition: transform 360ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
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
    transition: opacity 240ms cubic-bezier(0.20, 0.0, 0.0, 1.0),
                transform 340ms cubic-bezier(0.28, 1.32, 0.36, 1.0),
                border-radius 300ms cubic-bezier(0.34, 1.48, 0.46, 1.0);
}}

.neon-qs-dropdown-card.dd-closed {{
    opacity: 0;
    transform: translateY(-10px) scale(0.96);
    border-radius: 12px;
}}

.neon-qs-dropdown-card.dd-swap {{
    opacity: 0.45;
    transform: translateY(-4px) scale(0.98);
    transition: opacity 90ms ease, transform 90ms ease;
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
    transition: background-color 150ms ease,
                transform 260ms cubic-bezier(0.34, 1.56, 0.64, 1.0),
                border-radius 240ms cubic-bezier(0.34, 1.48, 0.46, 1.0);
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
    transition: background-color 200ms ease,
                color 180ms ease,
                border-radius 280ms cubic-bezier(0.34, 1.48, 0.46, 1.0),
                transform 260ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
}}

button.neon-qs-seg-btn:hover {{
    color: {fg_primary};
    background-color: rgba(255, 255, 255, 0.08);
}}

button.neon-qs-seg-btn:active {{
    transform: scale(0.95);
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
    transition: background-color 200ms ease,
                border-color 200ms ease,
                border-radius 320ms cubic-bezier(0.34, 1.48, 0.46, 1.0),
                transform 280ms cubic-bezier(0.34, 1.56, 0.64, 1.0);
}}

button.neon-qs-action-pill:hover {{
    background-color: {qs_pill_hover};
    border-color: {cc_glass_border};
    transform: scale(1.02);
}}

button.neon-qs-action-pill:active {{
    transform: scale(0.95);
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

/* ===================== FULLSCREEN SIMPLE CENTERED POWER MENU ===================== */

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
    transition: background-color 140ms ease, transform 140ms cubic-bezier(0.22, 1.0, 0.36, 1.0), color 140ms ease;
}}

button.neon-pm-action-btn:hover {{
    background-color: rgba(255, 255, 255, 0.14);
    transform: scale(1.08);
}}

button.neon-pm-action-btn.danger:hover {{
    background-color: alpha(#EF4444, 0.28);
    color: #FCA5A5;
}}

button.neon-pm-action-btn:active {{
    transform: scale(0.96);
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
}}

/* ===================== GNOME 47 INTERACTIVE SCREENSHOT & SCREEN RECORD UI ===================== */

.neon-gnome-shot-bar {{
    background-color: rgba(20, 23, 32, 0.90);
    border: 1px solid rgba(255, 255, 255, 0.22);
    border-radius: 26px;
    padding: 14px 20px;
    box-shadow: 0 20px 48px rgba(0, 0, 0, 0.55), inset 0 1px 0 rgba(255, 255, 255, 0.22);
    color: #F8FAFC;
}}

button.neon-shot-mode-btn {{
    background-image: none;
    background-color: rgba(255, 255, 255, 0.10);
    color: #CBD5E1;
    border: 1px solid transparent;
    box-shadow: none;
    border-radius: 999px;
    padding: 7px 14px;
    font-size: 13px;
    font-weight: 600;
    transition: background-color 140ms ease, color 140ms ease;
}}

button.neon-shot-mode-btn:hover {{
    background-color: rgba(255, 255, 255, 0.18);
    color: #FFFFFF;
}}

button.neon-shot-mode-btn.active {{
    background-color: {accent};
    color: #FFFFFF;
}}

button.neon-shot-toggle-btn.active {{
    background-color: rgba(239, 68, 68, 0.88);
    color: #FFFFFF;
}}

.neon-shot-kind-track {{
    background-color: rgba(255, 255, 255, 0.10);
    border: 1px solid rgba(255, 255, 255, 0.16);
    border-radius: 999px;
    padding: 3px;
}}

button.neon-shot-kind-btn {{
    background-image: none;
    background-color: transparent;
    color: #CBD5E1;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    padding: 6px 13px;
    font-size: 12.5px;
    font-weight: 600;
    transition: background-color 140ms ease, color 140ms ease;
}}

button.neon-shot-kind-btn:hover {{
    color: #FFFFFF;
}}

button.neon-shot-kind-btn.active {{
    background-color: rgba(255, 255, 255, 0.22);
    color: #FFFFFF;
}}

button.neon-shot-shutter {{
    background-image: none;
    background-color: #FFFFFF;
    color: #0F172A;
    border: 4px solid rgba(255, 255, 255, 0.35);
    border-radius: 999px;
    min-width: 56px;
    min-height: 56px;
    padding: 0;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.40);
    transition: background-color 140ms ease, border-color 140ms ease, transform 120ms ease;
}}

button.neon-shot-shutter:hover {{
    background-color: #E2E8F0;
    transform: scale(1.05);
}}

button.neon-shot-shutter.record {{
    background-color: #EF4444;
    color: #FFFFFF;
    border: 4px solid rgba(254, 202, 202, 0.45);
}}

button.neon-shot-shutter.record:hover {{
    background-color: #DC2626;
}}

button.neon-shot-stop-btn {{
    background-image: none;
    background-color: #DC2626;
    color: #FFFFFF;
    border: 1px solid rgba(255, 255, 255, 0.25);
    border-radius: 999px;
    padding: 8px 16px;
    font-size: 12.5px;
    font-weight: 700;
}}

button.neon-shot-stop-btn:hover {{
    background-color: #EF4444;
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
    transition: background-color 80ms ease, color 80ms ease;
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
    transition: background-color 110ms ease;
}}

button.neon-dialog-btn:hover {{
    background-color: {ctx_sub_open_bg};
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

/* ===================== INTERACTIVE DESKTOP ICONS LAYER ===================== */

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
    transition: background-color 120ms ease, border-color 120ms ease, transform 120ms ease;
}}

button.neon-desktop-icon-tile:hover {{
    background-color: rgba(255, 255, 255, 0.14);
    border-color: rgba(255, 255, 255, 0.22);
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
"#
    )
}
