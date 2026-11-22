//! Authentic macOS Sequoia × GNOME 47 Boxed-List GTK4 CSS Theme for `neon-settings`.
//!
//! Supports both crisp **Neon Light** and neutral macOS **Neon Dark**, circular accent color
//! swatches, visual style preview cards, wallpaper thumbnail tiles, segmented controls, and
//! clean hairline boxed lists.

use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, gdk::Display};
use neon_config::ThemeMode;

pub struct SettingsTheme {
    provider: CssProvider,
}

impl SettingsTheme {
    pub fn init(mode: ThemeMode, accent: &str) -> Self {
        let provider = CssProvider::new();
        apply_settings_icon_theme(mode);
        provider.load_from_string(&build_css(mode, accent));
        if let Some(display) = Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        Self { provider }
    }

    pub fn update(&self, mode: ThemeMode, accent: &str) {
        apply_settings_icon_theme(mode);
        self.provider.load_from_string(&build_css(mode, accent));
    }
}

fn apply_settings_icon_theme(mode: ThemeMode) {
    if let Some(settings) = gtk4::Settings::default() {
        let light = mode == ThemeMode::NeonLight;
        settings.set_gtk_icon_theme_name(Some(if light {
            "WhiteSur-light"
        } else {
            "WhiteSur-dark"
        }));
        settings.set_gtk_application_prefer_dark_theme(!light);
    }
}

fn build_css(mode: ThemeMode, accent: &str) -> String {
    let light = mode == ThemeMode::NeonLight;
    let (
        win_bg,
        sidebar_bg,
        card_bg,
        fg,
        muted,
        border,
        row_hover,
        row_selected_bg,
        seg_track,
        seg_active,
        trough_bg,
        card_shadow,
    ) = if light {
        (
            "#FAFAFA",
            "#F0F0F2",
            "#FFFFFF",
            "#1D1D1F",
            "#6E6E73",
            "rgba(0, 0, 0, 0.08)",
            "rgba(0, 0, 0, 0.04)",
            "rgba(0, 0, 0, 0.09)",
            "rgba(0, 0, 0, 0.06)",
            "#FFFFFF",
            "rgba(0, 0, 0, 0.12)",
            "0 1px 2px rgba(0, 0, 0, 0.06)",
        )
    } else {
        (
            "#1E1E22",
            "#26262B",
            "#2B2B31",
            "#F5F5F7",
            "#9A9AA3",
            "rgba(255, 255, 255, 0.08)",
            "rgba(255, 255, 255, 0.05)",
            "rgba(255, 255, 255, 0.11)",
            "rgba(255, 255, 255, 0.07)",
            "#3A3A42",
            "rgba(255, 255, 255, 0.14)",
            "0 1px 3px rgba(0, 0, 0, 0.25)",
        )
    };

    format!(
        r#"
* {{
    font-family: "Adwaita Sans", "Inter", "SF Pro Text", sans-serif;
}}

window.neon-settings-window {{
    background-color: {win_bg};
    color: {fg};
}}

/* ===================== SPLIT HEADERBAR (macOS x GNOME 47) ===================== */

.settings-split-header {{
    min-height: 46px;
    background-color: {win_bg};
}}

.settings-sidebar-header {{
    background-color: {sidebar_bg};
    border-right: 1px solid {border};
    padding: 0 10px;
    min-height: 46px;
}}

.settings-content-header {{
    background-color: {win_bg};
    padding: 0 14px;
    min-height: 46px;
}}

.settings-header-title {{
    font-size: 14px;
    font-weight: 700;
    color: {fg};
}}

button.settings-header-icon-btn,
menubutton.settings-header-icon-btn > button {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 30px;
    min-height: 30px;
    padding: 0;
    color: {fg};
    transition: background-color 140ms ease;
}}

button.settings-header-icon-btn:hover,
menubutton.settings-header-icon-btn > button:hover {{
    background-color: {row_hover};
}}

button.settings-close-circle {{
    background-image: none;
    background-color: {seg_track};
    border: none;
    box-shadow: none;
    border-radius: 999px;
    min-width: 26px;
    min-height: 26px;
    padding: 0;
    color: {fg};
    font-weight: 700;
}}

button.settings-close-circle:hover {{
    background-color: rgba(239, 68, 68, 0.85);
    color: #FFFFFF;
}}

/* ===================== SIDEBAR NAVIGATION ===================== */

.settings-sidebar {{
    background-color: {sidebar_bg};
    border-right: 1px solid {border};
    padding: 6px 10px 12px 10px;
}}

.settings-search {{
    background-color: {card_bg};
    color: {fg};
    border: 1px solid {border};
    border-radius: 10px;
    padding: 3px 10px;
    margin-bottom: 8px;
}}

list.settings-nav-list,
list.settings-nav-list row {{
    background-color: transparent;
    background-image: none;
    box-shadow: none;
}}

list.settings-nav-list row.settings-nav-row {{
    border-radius: 10px;
    padding: 7px 10px;
    margin: 1px 0;
    color: {fg};
    font-size: 13px;
    font-weight: 500;
    transition: background-color 120ms ease;
}}

list.settings-nav-list row.settings-nav-row:hover {{
    background-color: {row_hover};
}}

list.settings-nav-list row.settings-nav-row:selected {{
    background-color: {row_selected_bg};
    color: {fg};
    font-weight: 600;
}}

.settings-sidebar-sep {{
    background-color: {border};
    min-height: 1px;
    margin: 7px 8px;
}}

/* ===================== CONTENT CLAMP & BOXED LIST CARDS ===================== */

.settings-content-clamp {{
    padding: 18px 28px 28px 28px;
}}

.settings-section-title {{
    font-size: 14px;
    font-weight: 700;
    color: {fg};
    margin-top: 10px;
    margin-bottom: 8px;
}}

.settings-card {{
    background-color: {card_bg};
    border: 1px solid {border};
    border-radius: 14px;
    padding: 4px 0;
    margin-bottom: 14px;
    box-shadow: {card_shadow};
}}

.settings-row {{
    padding: 11px 16px;
}}

.settings-row-sep {{
    background-color: {border};
    min-height: 1px;
    margin: 0 16px;
}}

.settings-row-title {{
    font-size: 14px;
    font-weight: 500;
    color: {fg};
}}

.settings-row-desc {{
    font-size: 12px;
    color: {muted};
}}

.settings-badge {{
    background-color: {seg_track};
    color: {fg};
    border-radius: 999px;
    padding: 3px 10px;
    font-size: 11px;
    font-weight: 600;
}}

.settings-badge.ok {{
    background-color: alpha(#10B981, 0.18);
    color: #10B981;
}}

/* ===================== VISUAL STYLE CARDS (GNOME 47 / macOS) ===================== */

button.style-preview-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    padding: 4px;
    border-radius: 14px;
}}

.style-preview-frame {{
    border-radius: 12px;
    min-width: 176px;
    min-height: 112px;
    border: 3px solid transparent;
    padding: 12px;
    transition: border-color 140ms ease;
}}

button.style-preview-btn.active .style-preview-frame {{
    border-color: {accent};
}}

.style-preview-light-bg {{
    background-image: linear-gradient(135deg, #3B82F6 0%, #60A5FA 45%, #1D4ED8 100%);
}}

.style-preview-dark-bg {{
    background-image: linear-gradient(135deg, #0F172A 0%, #1E3A8A 50%, #172554 100%);
}}

.style-mini-win-light {{
    background-color: #FFFFFF;
    border-radius: 7px;
    min-width: 96px;
    min-height: 60px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.28);
}}

.style-mini-win-dark {{
    background-color: #1F2229;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 7px;
    min-width: 96px;
    min-height: 60px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.40);
}}

/* ===================== CIRCULAR ACCENT COLOR SWATCHES ===================== */

button.accent-circle {{
    background-image: none;
    border-radius: 999px;
    min-width: 28px;
    min-height: 28px;
    padding: 0;
    border: 2px solid transparent;
    box-shadow: inset 0 1px 2px rgba(255, 255, 255, 0.25);
}}

button.accent-circle.active {{
    border: 3px solid {fg};
}}

button.accent-blue {{ background-color: #0A84FF; }}
button.accent-cyan {{ background-color: #38BDF8; }}
button.accent-violet {{ background-color: #8B5CF6; }}
button.accent-emerald {{ background-color: #10B981; }}
button.accent-amber {{ background-color: #F59E0B; }}
button.accent-rose {{ background-color: #F43F5E; }}
button.accent-pink {{ background-color: #EC4899; }}

/* ===================== WALLPAPER THUMBNAIL GRID ===================== */

button.wp-thumb-btn {{
    background-image: none;
    background-color: transparent;
    border: 3px solid transparent;
    box-shadow: none;
    border-radius: 12px;
    padding: 2px;
    transition: border-color 140ms ease;
}}

button.wp-thumb-btn:hover {{
    border-color: {border};
}}

button.wp-thumb-btn.active {{
    border-color: {accent};
}}

.wp-thumb-pic {{
    border-radius: 9px;
}}

/* ===================== SEGMENTED CONTROLS & SLIDERS ===================== */

.settings-seg-track {{
    background-color: {seg_track};
    border-radius: 10px;
    padding: 3px;
}}

button.settings-seg-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 8px;
    padding: 6px 14px;
    font-size: 13px;
    font-weight: 600;
    color: {muted};
}}

button.settings-seg-btn.active {{
    background-color: {seg_active};
    color: {fg};
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
}}

button.settings-action-btn {{
    background-image: none;
    background-color: {accent};
    color: #FFFFFF;
    border: none;
    border-radius: 10px;
    padding: 7px 16px;
    font-size: 13px;
    font-weight: 600;
}}

button.settings-secondary-btn {{
    background-image: none;
    background-color: {seg_track};
    color: {fg};
    border: 1px solid {border};
    border-radius: 10px;
    padding: 7px 14px;
    font-size: 13px;
    font-weight: 600;
}}

scale.settings-scale trough {{
    background-color: {trough_bg};
    border-radius: 999px;
    min-height: 6px;
}}

scale.settings-scale highlight {{
    background-color: {accent};
    border-radius: 999px;
}}

scale.settings-scale slider {{
    background-color: #FFFFFF;
    border: 1px solid rgba(0, 0, 0, 0.16);
    border-radius: 999px;
    min-width: 18px;
    min-height: 18px;
    margin: -6px 0;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.25);
}}

.settings-status-bar {{
    padding: 6px 16px;
    border-top: 1px solid {border};
    background-color: {sidebar_bg};
    font-size: 12px;
    color: {muted};
}}

popover > contents {{
    background-color: {card_bg};
    color: {fg};
    border: 1px solid {border};
    border-radius: 12px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
}}

button.settings-menu-item {{
    background-image: none;
    background-color: transparent;
    color: {fg};
    border: none;
    box-shadow: none;
    border-radius: 8px;
    padding: 7px 12px;
    font-size: 13px;
    font-weight: 500;
}}

button.settings-menu-item:hover {{
    background-color: {row_hover};
}}
"#
    )
}
