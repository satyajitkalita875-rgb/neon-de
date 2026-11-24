//! NEON DE macOS Tahoe ("Liquid Glass") Visual Design Language — Neon Dark & Neon Light themes,
//! GPU-blurred translucent glass backdrops, specular luminous rims, spring open/close animations,
//! authentic macOS menu bar & Control Center, floating glass dock, and blurred Launchpad.

use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, gdk::Display};
use neon_ipc::AppearanceInfo;

/// Holds the active `CssProvider` so theme updates apply instantaneously with zero restart.
pub struct ShellThemeController {
    provider: CssProvider,
    last_applied: Option<AppearanceInfo>,
}

impl ShellThemeController {
    pub fn init() -> Self {
        let provider = CssProvider::new();
        let default_appearance = AppearanceInfo {
            theme: "neon_dark".into(),
            accent_color: "#38BDF8".into(),
            dock_transparency: 0.78,
            panel_transparency: 0.76,
            panel_visible: true,
            show_clock: true,
            show_workspaces: true,
            high_contrast: false,
            wallpaper: None,
            reduce_blur: false,
            blur_strength: 1.0,
            cursor_theme: Some("macOS".into()),
            cursor_size: Some(20),
        };
        provider.load_from_string(&build_shell_css(&default_appearance));
        apply_gtk_settings(&default_appearance);
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
        }
    }

    pub fn apply_if_changed(&mut self, appearance: &AppearanceInfo) {
        if self.last_applied.as_ref() == Some(appearance) {
            return;
        }
        apply_gtk_settings(appearance);
        self.provider.load_from_string(&build_shell_css(appearance));
        self.last_applied = Some(appearance.clone());
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
    let reduce_blur = appearance.reduce_blur || appearance.blur_strength <= 0.05;
    let blur_s = appearance.blur_strength.clamp(0.0, 2.0);
    let accent = &appearance.accent_color;
    let panel_a = if high_contrast {
        1.0
    } else if reduce_blur {
        (appearance.panel_transparency * 0.94).clamp(0.76, 0.96)
    } else {
        (appearance.panel_transparency * 0.76).clamp(0.42, 0.94)
    };
    let dock_a = if high_contrast {
        1.0
    } else if reduce_blur {
        (appearance.dock_transparency * 0.92).clamp(0.74, 0.95)
    } else {
        (appearance.dock_transparency * 0.68).clamp(0.36, 0.92)
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

    let _ = (tahoe_glass_bg, tahoe_blur_filter, launchpad_blur_filter, slider_fill);

    format!(
        r#"
* {{
    font-family: "Adwaita Sans", "Inter", "SF Pro Text", sans-serif;
}}

window.neon-panel-window,
window.neon-dock-window,
window.neon-overview-window,
window.neon-quicksettings-window,
window.neon-toast-window,
window.neon-screenshot-window,
window.neon-contextmenu-window {{
    background-color: transparent;
    background-image: none;
    box-shadow: none;
    border: none;
    outline: none;
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
                color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
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
    padding: 16px 24px 16px 24px;
}}

.neon-dock-container {{
    background-color: {dock_bg};
    border: 1px solid {border_glass};
    border-radius: 22px;
    padding: 6px 12px 5px 12px;
    box-shadow: 0 3px 10px rgba(0, 0, 0, 0.28), {specular_inset};
    transition: background-color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                border-color 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                box-shadow 280ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-dock-container.drawer-open {{
    background-color: transparent;
    border-color: transparent;
    box-shadow: none;
}}

.neon-dock-container.drawer-open .neon-dock-separator {{
    background-color: rgba(255, 255, 255, 0.26);
}}

button.neon-dock-item {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 14px;
    padding: 4px 6px 2px 6px;
    min-width: 48px;
    min-height: 52px;
    color: {fg_primary};
    transition: transform 180ms cubic-bezier(0.22, 1.0, 0.36, 1.0), background-color 150ms ease;
}}

button.neon-dock-item:hover {{
    background-color: rgba(255, 255, 255, 0.12);
    transform: translateY(-5px) scale(1.13);
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
    background-color: transparent;
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
    min-width: 22px;
    min-height: 22px;
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

button.neon-cc-output-circle {{
    background-image: none;
    background-color: {cc_badge_idle};
    border: 1px solid {border_subtle};
    box-shadow: none;
    border-radius: 999px;
    min-width: 28px;
    min-height: 28px;
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

scale.neon-cc-slider trough {{
    background-color: {cc_slider_trough};
    border: none;
    border-radius: 999px;
    min-height: 6px;
    box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.16);
}}

scale.neon-cc-slider highlight {{
    background-color: {cc_slider_fill};
    border-radius: 999px;
}}

scale.neon-cc-slider slider {{
    background-image: none;
    background-color: #FFFFFF;
    border: 1px solid rgba(0, 0, 0, 0.08);
    border-radius: 999px;
    min-width: 18px;
    min-height: 18px;
    margin: -6px 0;
    box-shadow: 0 2px 7px rgba(0, 0, 0, 0.30);
    transition: transform 140ms ease;
}}

scale.neon-cc-slider:hover slider {{
    transform: scale(1.12);
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

/* ===================== WORKSPACES & DESKTOPS PANEL (TOP BAR NEON BUTTON) ===================== */

.neon-ws-panel-surface {{
    background-color: {cc_glass_bg};
    background-image: {cc_glass_grad};
    border: 1px solid {cc_glass_border};
    border-radius: 26px;
    padding: 16px 18px 18px 18px;
    box-shadow: {cc_glass_shadow};
    color: {fg_primary};
    min-width: 460px;
    transition: opacity 220ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                transform 260ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-ws-panel-surface.ws-closed {{
    opacity: 0;
    transform: translateY(-16px) scale(0.93);
}}

.neon-ws-panel-surface.ws-open {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
}}

.neon-ws-panel-title {{
    color: {fg_primary};
    font-size: 15px;
    font-weight: 700;
}}

button.neon-ws-add-btn {{
    background-image: none;
    background-color: {accent};
    color: #FFFFFF;
    border: none;
    box-shadow: 0 4px 14px alpha({accent}, 0.36);
    border-radius: 999px;
    padding: 6px 14px;
    font-size: 12px;
    font-weight: 700;
    transition: transform 140ms ease, opacity 140ms ease;
}}

button.neon-ws-add-btn:hover {{
    transform: scale(1.03);
}}

button.neon-ws-add-btn:active {{
    transform: scale(0.96);
}}

button.neon-ws-add-btn:disabled {{
    opacity: 0.42;
}}

.neon-ws-desktop-card {{
    background-color: {cc_badge_idle};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 10px 12px;
    transition: background-color 160ms ease, border-color 160ms ease, transform 150ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-ws-desktop-card:hover {{
    background-color: {cc_glass_hover};
    border-color: {cc_glass_border};
    transform: translateY(-2px);
}}

.neon-ws-desktop-card.active {{
    background-color: alpha({accent}, 0.24);
    border-color: {accent};
    box-shadow: 0 4px 14px alpha({accent}, 0.25);
}}

button.neon-ws-plus-card {{
    background-image: none;
    background-color: transparent;
    border: 1px dashed {cc_glass_border};
    box-shadow: none;
    border-radius: 16px;
    padding: 10px 12px;
    color: {fg_primary};
    transition: background-color 160ms ease, border-color 160ms ease, transform 150ms ease;
}}

button.neon-ws-plus-card:hover {{
    background-color: {cc_badge_idle};
    border-color: {accent};
    transform: translateY(-2px);
}}

button.neon-ws-remove-btn {{
    background-image: none;
    background-color: transparent;
    color: {fg_muted};
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 20px;
    min-height: 20px;
    padding: 0;
    font-size: 13px;
    font-weight: 700;
    transition: background-color 140ms ease, color 140ms ease;
}}

button.neon-ws-remove-btn:hover {{
    background-color: rgba(239, 68, 68, 0.85);
    color: #FFFFFF;
}}

.neon-ws-window-card {{
    background-color: {cc_badge_idle};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 10px 12px;
    color: {fg_primary};
}}

.neon-ws-window-card.focused {{
    border-color: {accent};
}}

.neon-ws-badge {{
    background-color: alpha({accent}, 0.22);
    color: {fg_primary};
    border-radius: 6px;
    padding: 1px 6px;
    font-size: 10px;
    font-weight: 700;
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

/* ===================== GNOME 47 INTERACTIVE SCREENSHOT UI ===================== */

.neon-gnome-shot-bar {{
    background-color: rgba(24, 26, 34, 0.88);
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

button.neon-shot-shutter {{
    background-image: none;
    background-color: #FFFFFF;
    color: #0F172A;
    border: 4px solid rgba(255, 255, 255, 0.35);
    border-radius: 999px;
    min-width: 54px;
    min-height: 54px;
    padding: 0;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.40);
    transition: background-color 120ms ease, transform 120ms ease;
}}

button.neon-shot-shutter:hover {{
    background-color: #E2E8F0;
    transform: scale(1.05);
}}

/* ===================== MACOS RIGHT-CLICK CONTEXT MENU ===================== */

.neon-ctx-menu {{
    background-color: {popover_bg};
    border: 1px solid {border_glass};
    border-radius: 14px;
    padding: 6px;
    box-shadow: 0 18px 42px rgba(0, 0, 0, 0.48), {specular_inset};
    color: {fg_primary};
}}

button.neon-ctx-item {{
    background-image: none;
    background-color: transparent;
    color: {fg_primary};
    border: none;
    box-shadow: none;
    border-radius: 8px;
    padding: 6px 10px;
    min-height: 24px;
    font-size: 13px;
    font-weight: 500;
    transition: background-color 90ms ease, color 90ms ease;
}}

button.neon-ctx-item:hover {{
    background-color: {accent};
    color: #FFFFFF;
}}

button.neon-ctx-item:hover .neon-ctx-shortcut {{
    color: rgba(255, 255, 255, 0.82);
}}

button.neon-ctx-item.destructive {{
    color: #F87171;
}}

button.neon-ctx-item.destructive:hover {{
    background-color: #EF4444;
    color: #FFFFFF;
}}

.neon-ctx-icon {{
    opacity: 0.90;
}}

.neon-ctx-shortcut {{
    color: {fg_muted};
    font-size: 11px;
    font-weight: 500;
    margin-left: 14px;
}}

.neon-ctx-sep {{
    background-color: {border_subtle};
    min-height: 1px;
    margin: 4px 8px;
}}
"#
    )
}
