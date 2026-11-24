//! Authentic macOS Sequoia × GNOME 47 Boxed-List GTK4 CSS Theme for `neon-settings`.
//!
//! Supports Neon Light and Neon Dark, NEON DE sidebar branding, circular accent color swatches,
//! visual style preview cards, wallpaper thumbnail tiles, capability badges, search highlights,
//! and clean hairline boxed lists.

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
        _row_selected_bg,
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
            "#1C1C20",
            "#242429",
            "#2A2A30",
            "#F5F5F7",
            "#9A9AA3",
            "rgba(255, 255, 255, 0.08)",
            "rgba(255, 255, 255, 0.05)",
            "rgba(255, 255, 255, 0.12)",
            "rgba(255, 255, 255, 0.07)",
            "#3A3A44",
            "rgba(255, 255, 255, 0.14)",
            "0 1px 3px rgba(0, 0, 0, 0.28)",
        )
    };

    format!(
        r#"
* {{
    font-family: "Inter", "Adwaita Sans", "Cantarell", sans-serif;
}}

window.neon-settings-window {{
    background-color: {win_bg};
    color: {fg};
}}

/* ===================== SPLIT HEADERBAR (macOS x GNOME) ===================== */

.settings-split-header {{
    min-height: 48px;
    background-color: {win_bg};
}}

.settings-sidebar-header {{
    background-color: {sidebar_bg};
    border-right: 1px solid {border};
    padding: 0 10px;
    min-height: 48px;
}}

.settings-content-header {{
    background-color: {win_bg};
    border-bottom: 1px solid {border};
    padding: 0 14px;
    min-height: 48px;
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

button.settings-header-icon-btn:disabled {{
    opacity: 0.35;
}}

.macos-traffic-lights {{
    margin-left: 4px;
}}

button.macos-tl-btn {{
    background-image: none;
    border-radius: 999px;
    min-width: 13px;
    min-height: 13px;
    padding: 0;
    margin: 0;
    font-size: 9px;
    font-weight: 800;
    color: transparent;
    box-shadow: inset 0 0 0 0.75px rgba(0, 0, 0, 0.22);
    border: none;
    transition: background-color 120ms ease, color 120ms ease;
}}

button.macos-tl-close {{ background-color: #FF5F57; }}
button.macos-tl-min {{ background-color: #FEBC2E; }}
button.macos-tl-max {{ background-color: #28C840; }}

window:backdrop button.macos-tl-btn {{
    background-color: rgba(135, 137, 145, 0.45);
}}

.macos-traffic-lights:hover button.macos-tl-close {{
    background-color: #FF5F57;
    color: rgba(77, 0, 0, 0.85);
}}

.macos-traffic-lights:hover button.macos-tl-min {{
    background-color: #FEBC2E;
    color: rgba(110, 62, 0, 0.85);
}}

.macos-traffic-lights:hover button.macos-tl-max {{
    background-color: #28C840;
    color: rgba(0, 72, 12, 0.85);
}}

/* ===================== SIDEBAR BRANDING & NAVIGATION ===================== */

.settings-sidebar {{
    background-color: {sidebar_bg};
    border-right: 1px solid {border};
    padding: 8px 10px 10px 10px;
}}

.neon-brand-card {{
    padding: 6px 8px 10px 8px;
}}

.neon-brand-emblem {{
    background-image: linear-gradient(135deg, {accent} 0%, #6366F1 100%);
    color: #FFFFFF;
    border-radius: 9px;
    min-width: 30px;
    min-height: 30px;
    font-size: 13px;
    font-weight: 800;
    box-shadow: 0 2px 8px alpha({accent}, 0.35);
}}

.neon-brand-title {{
    font-size: 13px;
    font-weight: 700;
    color: {fg};
}}

.neon-brand-sub {{
    font-size: 11px;
    color: {muted};
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
    background-color: alpha({accent}, 0.18);
    color: {fg};
    font-weight: 600;
}}

.settings-sidebar-sep {{
    background-color: {border};
    min-height: 1px;
    margin: 6px 8px;
}}

button.settings-about-footer-btn {{
    background-image: none;
    background-color: transparent;
    border: 1px solid transparent;
    box-shadow: none;
    border-radius: 10px;
    padding: 8px 10px;
    margin-top: 6px;
    color: {fg};
    font-size: 13px;
    font-weight: 600;
    transition: background-color 120ms ease;
}}

button.settings-about-footer-btn:hover {{
    background-color: {row_hover};
}}

button.settings-about-footer-btn.active {{
    background-color: alpha({accent}, 0.18);
    border-color: alpha({accent}, 0.35);
}}

/* ===================== PAGE HEADER, CLAMP & BOXED CARDS ===================== */

.settings-content-clamp {{
    padding: 18px 28px 32px 28px;
}}

.settings-page-title {{
    font-size: 22px;
    font-weight: 700;
    color: {fg};
}}

.settings-page-desc {{
    font-size: 13px;
    color: {muted};
    margin-bottom: 10px;
}}

.settings-section-title {{
    font-size: 13px;
    font-weight: 700;
    color: {fg};
    margin-top: 12px;
    margin-bottom: 7px;
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

.settings-row-highlighted {{
    background-color: alpha({accent}, 0.14);
    border-radius: 10px;
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

/* ===================== CAPABILITY & STATUS BADGES ===================== */

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

.settings-badge.partial {{
    background-color: alpha(#F59E0B, 0.18);
    color: #F59E0B;
}}

.settings-badge.elevated {{
    background-color: alpha(#38BDF8, 0.18);
    color: #38BDF8;
}}

.settings-badge.unavailable {{
    background-color: alpha(#94A3B8, 0.18);
    color: {muted};
}}

.settings-badge.unsupported {{
    background-color: alpha(#EF4444, 0.14);
    color: #F87171;
}}

/* ===================== GLOBAL SEARCH RESULTS ===================== */

button.search-hit-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 10px;
    padding: 10px 14px;
    color: {fg};
}}

button.search-hit-btn:hover {{
    background-color: {row_hover};
}}

/* ===================== VISUAL STYLE CARDS ===================== */

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
    min-width: 152px;
    min-height: 98px;
    border: 3px solid transparent;
    padding: 10px;
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

.style-preview-auto-bg {{
    background-image: linear-gradient(135deg, #60A5FA 0%, #3B82F6 48%, #0F172A 52%, #1E293B 100%);
}}

.style-mini-win-light {{
    background-color: #FFFFFF;
    border-radius: 7px;
    min-width: 82px;
    min-height: 52px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.26);
}}

.style-mini-win-dark {{
    background-color: #1F2229;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 7px;
    min-width: 82px;
    min-height: 52px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.40);
}}

/* ===================== ACCENT SWATCHES & WALLPAPERS ===================== */

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

button.wp-thumb-btn {{
    background-image: none;
    background-color: transparent;
    border: 3px solid transparent;
    box-shadow: none;
    border-radius: 12px;
    padding: 2px;
    transition: border-color 140ms ease;
}}

button.wp-thumb-btn:hover {{ border-color: {border}; }}
button.wp-thumb-btn.active {{ border-color: {accent}; }}

.wp-thumb-pic {{ border-radius: 9px; }}

/* ===================== BUTTONS, SEGMENTED CONTROLS & SLIDERS ===================== */

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
    padding: 7px 15px;
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

button.settings-destructive-btn {{
    background-image: none;
    background-color: #DC2626;
    color: #FFFFFF;
    border: none;
    border-radius: 10px;
    padding: 7px 15px;
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

/* ===================== STATUS BAR & POPOVERS ===================== */

.settings-status-bar {{
    padding: 7px 16px;
    border-top: 1px solid {border};
    background-color: {sidebar_bg};
    font-size: 12px;
    color: {muted};
}}

.settings-status-bar.status-ok {{
    color: #10B981;
}}

.settings-status-bar.status-error {{
    color: #EF4444;
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

.monitor-canvas {{
    background-color: {seg_track};
    border: 1px solid {border};
    border-radius: 12px;
}}

.kbd-pill {{
    background-color: {seg_track};
    border: 1px solid {border};
    border-radius: 7px;
    padding: 3px 9px;
    font-family: monospace;
    font-size: 12px;
    font-weight: 700;
    color: {fg};
}}
"#
    )
}
