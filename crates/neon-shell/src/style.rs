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
        };
        provider.load_from_string(&build_shell_css(&default_appearance));
        apply_gtk_icon_theme(false);
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
        let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
        apply_gtk_icon_theme(is_light);
        self.provider.load_from_string(&build_shell_css(appearance));
        self.last_applied = Some(appearance.clone());
    }
}

fn apply_gtk_icon_theme(is_light: bool) {
    if let Some(settings) = gtk4::Settings::default() {
        let icon_theme = if is_light {
            "WhiteSur-light"
        } else {
            "WhiteSur-dark"
        };
        settings.set_gtk_icon_theme_name(Some(icon_theme));
        settings.set_gtk_application_prefer_dark_theme(!is_light);
    }
}

pub fn build_shell_css(appearance: &AppearanceInfo) -> String {
    let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
    let high_contrast = appearance.high_contrast;
    let accent = &appearance.accent_color;
    let panel_a = if high_contrast {
        1.0
    } else {
        (appearance.panel_transparency * 0.76).clamp(0.42, 0.94)
    };
    let dock_a = if high_contrast {
        1.0
    } else {
        (appearance.dock_transparency * 0.68).clamp(0.36, 0.92)
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
            } else {
                "rgba(244, 247, 253, 0.62)"
            },
            if high_contrast {
                "#F1F5F9"
            } else {
                "rgba(255, 255, 255, 0.56)"
            },
            "rgba(255, 255, 255, 0.76)",
            "blur(40px) saturate(1.55) brightness(1.14)",
            "blur(36px) saturate(1.30) brightness(0.95)",
            "rgba(236, 242, 252, 0.36)",
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
            "rgba(246, 248, 253, 0.86)",
        )
    } else {
        (
            format!("rgba(16, 18, 26, {panel_a:.2})"),
            format!("rgba(20, 23, 34, {dock_a:.2})"),
            if high_contrast {
                "#0B1120"
            } else {
                "rgba(24, 28, 42, 0.56)"
            },
            if high_contrast {
                "#111827"
            } else {
                "rgba(255, 255, 255, 0.11)"
            },
            "rgba(255, 255, 255, 0.17)",
            "blur(42px) saturate(1.65) brightness(0.76)",
            "blur(38px) saturate(1.35) brightness(0.62)",
            "rgba(10, 13, 22, 0.34)",
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
            "rgba(22, 26, 38, 0.84)",
        )
    };

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
window.neon-screenshot-window {{
    background-color: transparent;
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
    padding: 2px 8px;
    border-radius: 8px;
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

.neon-dock-container {{
    background-color: {dock_bg};
    border: 1px solid {border_glass};
    border-radius: 24px;
    padding: 6px 11px 4px 11px;
    box-shadow: 0 18px 46px rgba(0, 0, 0, 0.44), {specular_inset};
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

/* ===================== MACOS TAHOE ("LIQUID GLASS") CONTROL CENTER ===================== */

.neon-tahoe-wrapper {{
    padding: 8px 12px 24px 24px;
}}

.neon-tahoe-shell {{
    border-radius: 26px;
    border: 1px solid {border_glass};
    box-shadow: 0 24px 58px rgba(0, 0, 0, 0.48), {specular_inset};
    transition: opacity 200ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                transform 240ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-tahoe-shell.cc-closed {{
    opacity: 0;
    transform: translateY(-16px) scale(0.93);
}}

.neon-tahoe-shell.cc-open {{
    opacity: 1;
    transform: translateY(0px) scale(1.0);
}}

.neon-tahoe-blur-bg {{
    filter: {tahoe_blur_filter};
    transform: scale(1.18);
}}

.neon-tahoe-glass {{
    background-color: {tahoe_glass_bg};
    padding: 14px;
    color: {fg_primary};
    min-width: 344px;
}}

.neon-tahoe-card {{
    background-color: {tahoe_card_bg};
    border: 1px solid {border_subtle};
    border-radius: 20px;
    padding: 11px 13px;
    color: {fg_primary};
    box-shadow: {specular_inset};
}}

.neon-tahoe-sep {{
    background-color: {border_subtle};
    min-height: 1px;
    margin: 3px 4px;
}}

button.neon-tahoe-conn-row {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 14px;
    padding: 6px 6px;
    color: {fg_primary};
    transition: background-color 150ms ease, transform 140ms ease;
}}

button.neon-tahoe-conn-row:hover {{
    background-color: {tahoe_card_hover};
}}

button.neon-tahoe-conn-row:active {{
    transform: scale(0.97);
}}

button.neon-tahoe-focus-pill {{
    background-image: none;
    background-color: {tahoe_card_bg};
    border: 1px solid {border_subtle};
    box-shadow: {specular_inset};
    border-radius: 20px;
    padding: 10px 12px;
    color: {fg_primary};
    transition: background-color 160ms ease, border-color 160ms ease, transform 140ms ease;
}}

button.neon-tahoe-focus-pill:hover {{
    background-color: {tahoe_card_hover};
    border-color: {border_glass};
}}

button.neon-tahoe-focus-pill.active {{
    border-color: alpha({accent}, 0.60);
    background-color: alpha({accent}, 0.22);
}}

button.neon-tahoe-mini-tile {{
    background-image: none;
    background-color: {tahoe_card_bg};
    border: 1px solid {border_subtle};
    box-shadow: {specular_inset};
    border-radius: 20px;
    padding: 10px 6px;
    min-height: 58px;
    color: {fg_primary};
    transition: background-color 160ms ease, border-color 160ms ease, transform 140ms ease;
}}

button.neon-tahoe-mini-tile:hover {{
    background-color: {tahoe_card_hover};
    border-color: {border_glass};
    transform: translateY(-1px);
}}

button.neon-tahoe-mini-tile.active {{
    border-color: alpha({accent}, 0.55);
    background-color: alpha({accent}, 0.20);
}}

.neon-cc-badge {{
    background-color: {slider_trough};
    color: {fg_primary};
    border-radius: 999px;
    min-width: 34px;
    min-height: 34px;
    padding: 0;
    transition: background-color 180ms ease, color 180ms ease;
}}

button.neon-tahoe-conn-row.active .neon-cc-badge,
button.neon-tahoe-focus-pill.active .neon-cc-badge {{
    background-color: {accent};
    color: #FFFFFF;
    box-shadow: 0 4px 12px alpha({accent}, 0.45);
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

.neon-tahoe-slider-icon {{
    color: {fg_primary};
    margin: 0 6px 0 4px;
}}

/* macOS Tahoe Thick Liquid Glass Capsule Sliders (Display & Sound) */
scale.neon-tahoe-capsule trough {{
    background-color: {slider_trough};
    border: 1px solid {border_subtle};
    border-radius: 999px;
    min-height: 22px;
    box-shadow: inset 0 1px 3px rgba(0, 0, 0, 0.16);
}}

scale.neon-tahoe-capsule highlight {{
    background-color: {slider_fill};
    border-radius: 999px;
}}

scale.neon-tahoe-capsule slider {{
    background-color: #FFFFFF;
    border: 1px solid rgba(0, 0, 0, 0.12);
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    margin: -1px 0;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.32);
    transition: transform 140ms ease;
}}

scale.neon-tahoe-capsule:hover slider {{
    transform: scale(1.06);
}}

.neon-tahoe-notif-item {{
    background-color: {tahoe_card_bg};
    border-radius: 12px;
    padding: 7px 10px;
}}

button.neon-tahoe-footer-btn {{
    background-image: none;
    background-color: {tahoe_card_bg};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    box-shadow: {specular_inset};
    border-radius: 16px;
    padding: 8px 10px;
    min-height: 32px;
    font-size: 12px;
    font-weight: 600;
    transition: background-color 150ms ease, border-color 150ms ease, transform 140ms ease;
}}

button.neon-tahoe-footer-btn:hover {{
    background-color: {tahoe_card_hover};
    border-color: {border_glass};
    transform: translateY(-1px);
}}

button.neon-tahoe-footer-btn:active {{
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

/* ===================== MACOS LAUNCHPAD (GPU BLUR + ZOOM ANIMATION) ===================== */

.neon-launchpad-overlay {{
    transition: opacity 210ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-launchpad-overlay.lp-closed {{
    opacity: 0;
}}

.neon-launchpad-overlay.lp-open {{
    opacity: 1;
}}

.neon-launchpad-blur-bg {{
    filter: {launchpad_blur_filter};
    transform: scale(1.12);
}}

.neon-launchpad-scrim {{
    background-color: {scrim_bg};
    padding: 20px 44px 16px 44px;
    color: {fg_primary};
}}

.neon-launchpad-content {{
    transition: transform 240ms cubic-bezier(0.22, 1.0, 0.36, 1.0),
                opacity 200ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

.neon-launchpad-overlay.lp-closed .neon-launchpad-content {{
    opacity: 0;
    transform: scale(1.07);
}}

.neon-launchpad-overlay.lp-open .neon-launchpad-content {{
    opacity: 1;
    transform: scale(1.0);
}}

.neon-launchpad-search {{
    background-color: {tahoe_card_bg};
    color: {fg_primary};
    border: 1px solid {border_glass};
    border-radius: 14px;
    padding: 6px 16px;
    font-size: 14px;
    min-width: 290px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.25), {specular_inset};
}}

.neon-launchpad-search:focus-within {{
    border-color: {accent};
}}

button.neon-launchpad-tile {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    outline: none;
    border-radius: 22px;
    padding: 10px 10px;
    min-width: 118px;
    min-height: 108px;
    color: {fg_primary};
    transition: transform 180ms cubic-bezier(0.22, 1.0, 0.36, 1.0), background-color 150ms ease;
}}

button.neon-launchpad-tile:hover {{
    background-color: rgba(255, 255, 255, 0.13);
    transform: scale(1.08);
}}

button.neon-launchpad-tile:active {{
    transform: scale(0.96);
}}

.neon-launchpad-label {{
    color: #FFFFFF;
    font-size: 12px;
    font-weight: 500;
    text-shadow: 0 1px 5px rgba(0, 0, 0, 0.78);
}}

button.neon-page-dot {{
    background-image: none;
    background-color: rgba(255, 255, 255, 0.34);
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 8px;
    min-height: 8px;
    padding: 0;
    transition: background-color 180ms ease, min-width 180ms ease, transform 180ms ease;
}}

button.neon-page-dot:hover {{
    background-color: rgba(255, 255, 255, 0.72);
    transform: scale(1.15);
}}

button.neon-page-dot.active {{
    background-color: #FFFFFF;
    min-width: 22px;
    min-height: 8px;
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
"#
    )
}
