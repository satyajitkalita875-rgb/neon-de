//! NEON DE macOS Sequoia × GNOME 47 Visual Design Language (Neon Dark & Neon Light themes,
//! translucent glass surfaces, specular borders, icon-first menu bar, glass dock, and Control Center).

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
        self.provider.load_from_string(&build_shell_css(appearance));
        self.last_applied = Some(appearance.clone());
    }
}

pub fn build_shell_css(appearance: &AppearanceInfo) -> String {
    let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
    let high_contrast = appearance.high_contrast;
    let accent = &appearance.accent_color;
    let panel_a = if high_contrast {
        1.0
    } else {
        (appearance.panel_transparency * 0.88).clamp(0.45, 0.96)
    };
    let dock_a = if high_contrast {
        1.0
    } else {
        (appearance.dock_transparency * 0.82).clamp(0.40, 0.95)
    };

    let (
        panel_bg,
        dock_bg,
        popover_bg,
        card_bg,
        tile_bg,
        hover_bg,
        scrim_bg,
        fg_primary,
        fg_muted,
        border_glass,
        border_subtle,
        ws_dot_bg,
        slider_trough,
    ) = if is_light {
        (
            format!("rgba(246, 248, 252, {panel_a:.2})"),
            format!("rgba(248, 250, 255, {dock_a:.2})"),
            "rgba(246, 248, 253, 0.88)",
            if high_contrast {
                "#FFFFFF"
            } else {
                "rgba(255, 255, 255, 0.72)"
            },
            if high_contrast {
                "#E2E8F0"
            } else {
                "rgba(255, 255, 255, 0.82)"
            },
            "rgba(15, 23, 42, 0.08)",
            "rgba(236, 242, 250, 0.86)",
            "#0B1220",
            if high_contrast { "#0F172A" } else { "#526077" },
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(255, 255, 255, 0.75)"
            },
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(15, 23, 42, 0.12)"
            },
            "rgba(15, 23, 42, 0.28)",
            "rgba(15, 23, 42, 0.14)",
        )
    } else {
        (
            format!("rgba(18, 20, 28, {panel_a:.2})"),
            format!("rgba(22, 25, 36, {dock_a:.2})"),
            "rgba(22, 25, 35, 0.86)",
            if high_contrast {
                "#0B1120"
            } else {
                "rgba(255, 255, 255, 0.075)"
            },
            if high_contrast {
                "#111827"
            } else {
                "rgba(255, 255, 255, 0.09)"
            },
            "rgba(255, 255, 255, 0.11)",
            "rgba(10, 12, 20, 0.82)",
            if high_contrast { "#FFFFFF" } else { "#F1F5F9" },
            if high_contrast { "#E2E8F0" } else { "#94A3B8" },
            if high_contrast {
                "#F8FAFC"
            } else {
                "rgba(255, 255, 255, 0.18)"
            },
            if high_contrast {
                "#F8FAFC"
            } else {
                "rgba(255, 255, 255, 0.10)"
            },
            "rgba(255, 255, 255, 0.34)",
            "rgba(255, 255, 255, 0.14)",
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
window.neon-toast-window {{
    background-color: transparent;
}}

window.neon-wallpaper-window {{
    background-image: linear-gradient(140deg, #0b1020 0%, #15213d 50%, #1e1b3a 100%);
}}

/* ===================== TOP MENU BAR (macOS x GNOME 47) ===================== */

.neon-top-panel {{
    background-color: {panel_bg};
    border-bottom: 1px solid {border_subtle};
    padding: 0 12px;
    min-height: 30px;
    color: {fg_primary};
    font-size: 13px;
}}

.neon-brand-button {{
    background-color: transparent;
    color: {fg_primary};
    border: none;
    border-radius: 7px;
    padding: 2px 9px;
    min-height: 22px;
    font-weight: 700;
    letter-spacing: 0.4px;
    transition: background-color 140ms ease;
}}

.neon-brand-button:hover,
.neon-brand-button.active {{
    background-color: {hover_bg};
}}

.neon-app-title {{
    color: {fg_primary};
    font-weight: 600;
    font-size: 13px;
    margin-left: 4px;
}}

/* GNOME 47 style dynamic workspace dots / active pill */
.neon-ws-dot {{
    background-color: {ws_dot_bg};
    border: none;
    border-radius: 999px;
    min-width: 7px;
    min-height: 7px;
    padding: 0;
    margin: 0 2px;
    transition: all 160ms ease;
}}

.neon-ws-dot:hover {{
    background-color: {fg_primary};
    min-width: 12px;
}}

.neon-ws-dot.occupied {{
    background-color: {fg_muted};
}}

.neon-ws-dot.active {{
    background-color: {fg_primary};
    min-width: 22px;
}}

.neon-clock-btn {{
    background-color: transparent;
    border: none;
    border-radius: 7px;
    padding: 2px 10px;
    min-height: 22px;
    color: {fg_primary};
    font-weight: 600;
    font-size: 13px;
    letter-spacing: 0.2px;
    transition: background-color 140ms ease;
}}

.neon-clock-btn:hover {{
    background-color: {hover_bg};
}}

.neon-tray-btn {{
    background-color: transparent;
    border: none;
    border-radius: 7px;
    padding: 2px 7px;
    min-height: 22px;
    color: {fg_primary};
    font-size: 12px;
    font-weight: 500;
    transition: background-color 140ms ease;
}}

.neon-tray-btn:hover {{
    background-color: {hover_bg};
}}

.neon-tray-btn.active {{
    background-color: alpha({accent}, 0.26);
    color: {accent};
}}

.neon-notif-dot {{
    background-color: {accent};
    border-radius: 999px;
    min-width: 6px;
    min-height: 6px;
    margin-left: 2px;
}}

/* ===================== FLOATING GLASS DOCK (macOS) ===================== */

.neon-dock-container {{
    background-color: {dock_bg};
    border: 1px solid {border_glass};
    border-radius: 22px;
    padding: 6px 10px 4px 10px;
    box-shadow: 0 18px 44px rgba(0, 0, 0, 0.46), inset 0 1px 0 rgba(255, 255, 255, 0.14);
}}

.neon-dock-item {{
    background-color: transparent;
    border: none;
    border-radius: 14px;
    padding: 4px 6px 2px 6px;
    min-width: 48px;
    min-height: 50px;
    color: {fg_primary};
    transition: background-color 150ms ease, margin 150ms ease;
}}

.neon-dock-item:hover {{
    background-color: {hover_bg};
}}

.neon-dock-item.focused {{
    background-color: alpha({accent}, 0.18);
}}

.neon-dock-dot {{
    background-color: transparent;
    border-radius: 999px;
    min-width: 5px;
    min-height: 5px;
    margin-top: 3px;
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

/* ===================== CONTROL CENTER (macOS Sonoma) ===================== */

.neon-quicksettings-panel {{
    background-color: {popover_bg};
    border: 1px solid {border_glass};
    border-radius: 22px;
    padding: 16px;
    color: {fg_primary};
    box-shadow: 0 22px 54px rgba(0, 0, 0, 0.50);
    min-width: 344px;
}}

.neon-cc-tile {{
    background-color: {tile_bg};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 10px 12px;
    color: {fg_primary};
    min-height: 54px;
    transition: background-color 150ms ease, border-color 150ms ease;
}}

.neon-cc-tile:hover {{
    border-color: {border_glass};
    background-color: {hover_bg};
}}

.neon-cc-tile.active {{
    border-color: alpha({accent}, 0.55);
    background-color: alpha({accent}, 0.16);
}}

.neon-cc-badge {{
    background-color: {hover_bg};
    color: {fg_primary};
    border-radius: 999px;
    min-width: 34px;
    min-height: 34px;
    padding: 0;
}}

.neon-cc-tile.active .neon-cc-badge {{
    background-color: {accent};
    color: #FFFFFF;
}}

.neon-qs-card {{
    background-color: {card_bg};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 12px 14px;
    color: {fg_primary};
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

/* macOS Capsule Slider */
.neon-capsule-scale trough {{
    background-color: {slider_trough};
    border-radius: 999px;
    min-height: 10px;
}}

.neon-capsule-scale highlight {{
    background-color: {accent};
    border-radius: 999px;
}}

.neon-capsule-scale slider {{
    background-color: #FFFFFF;
    border-radius: 999px;
    min-width: 18px;
    min-height: 18px;
    margin: -4px 0;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35);
}}

/* Segmented Control (Power Profiles) */
.neon-segment-track {{
    background-color: {slider_trough};
    border-radius: 10px;
    padding: 3px;
}}

.neon-segment-btn {{
    background-color: transparent;
    border: none;
    border-radius: 8px;
    padding: 5px 10px;
    color: {fg_muted};
    font-size: 12px;
    font-weight: 600;
    transition: background-color 140ms ease, color 140ms ease;
}}

.neon-segment-btn:hover {{
    color: {fg_primary};
}}

.neon-segment-btn.active {{
    background-color: {accent};
    color: #FFFFFF;
}}

.neon-pill-button {{
    background-color: {tile_bg};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    border-radius: 10px;
    padding: 6px 12px;
    min-height: 28px;
    font-size: 12px;
    font-weight: 600;
    transition: background-color 140ms ease, border-color 140ms ease;
}}

.neon-pill-button:hover {{
    border-color: {accent};
    background-color: {hover_bg};
}}

.neon-pill-button.active {{
    border-color: {accent};
    background-color: alpha({accent}, 0.24);
}}

/* ===================== ACTIVITIES OVERVIEW (GNOME 47) ===================== */

.neon-overview-scrim {{
    background-color: {scrim_bg};
    padding: 32px 48px;
    color: {fg_primary};
}}

.neon-search-entry {{
    background-color: {tile_bg};
    color: {fg_primary};
    border: 1px solid {border_glass};
    border-radius: 999px;
    padding: 8px 18px;
    font-size: 15px;
    min-width: 440px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
}}

.neon-search-entry:focus-within {{
    border-color: {accent};
}}

.neon-section-heading {{
    color: {fg_muted};
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 1.0px;
}}

.neon-workspace-card {{
    background-color: {tile_bg};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 12px 16px;
    min-width: 176px;
    min-height: 92px;
    color: {fg_primary};
    transition: border-color 140ms ease, background-color 140ms ease;
}}

.neon-workspace-card:hover {{
    border-color: {border_glass};
}}

.neon-workspace-card.active {{
    border-color: {accent};
    background-color: alpha({accent}, 0.16);
}}

.neon-window-card {{
    background-color: {tile_bg};
    border: 1px solid {border_subtle};
    border-radius: 16px;
    padding: 12px 14px;
    min-width: 220px;
    color: {fg_primary};
}}

.neon-window-card.focused {{
    border-color: {accent};
}}

.neon-app-tile {{
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 18px;
    padding: 12px 10px;
    min-width: 112px;
    min-height: 102px;
    color: {fg_primary};
    transition: background-color 140ms ease, border-color 140ms ease;
}}

.neon-app-tile:hover {{
    background-color: {hover_bg};
    border-color: {border_subtle};
}}

.neon-close-button {{
    background-color: rgba(239, 68, 68, 0.22);
    color: #FCA5A5;
    border: none;
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    padding: 0 6px;
}}

.neon-close-button:hover {{
    background-color: rgba(239, 68, 68, 0.85);
    color: #FFFFFF;
}}

/* ===================== NOTIFICATION TOASTS ===================== */

.neon-toast-card {{
    background-color: {popover_bg};
    border: 1px solid {border_glass};
    border-radius: 18px;
    padding: 12px 16px;
    color: {fg_primary};
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.42);
    min-width: 320px;
}}

.neon-toast-card.critical {{
    border-color: #EF4444;
}}
"#
    )
}
