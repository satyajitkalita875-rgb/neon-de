//! Authentic macOS System Settings GTK4 CSS theme for `neon-settings`.
//! Features vibrant macOS traffic lights, colorful 20x20 rounded sidebar icon badges,
//! calm neutral graphite action buttons, compact grouped cards, and zero-redundancy CSS caching.

use std::cell::RefCell;

use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, gdk};
use neon_config::ThemeMode;

/// Holds the active GTK4 CSS provider for live Light/Dark/Accent updates without redundant reloads.
pub struct SettingsTheme {
    provider: CssProvider,
    last_state: RefCell<Option<(ThemeMode, String)>>,
}

impl SettingsTheme {
    pub fn install(theme: ThemeMode, accent: &str) -> Self {
        let provider = CssProvider::new();
        let instance = Self {
            provider,
            last_state: RefCell::new(None),
        };
        instance.update(theme, accent);
        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &instance.provider,
                STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        instance
    }

    pub fn update(&self, theme: ThemeMode, accent: &str) {
        let key = (theme, accent.to_string());
        if self.last_state.borrow().as_ref() == Some(&key) {
            return;
        }
        *self.last_state.borrow_mut() = Some(key);

        let light = theme == ThemeMode::NeonLight;
        if let Some(settings) = gtk4::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(!light);
        }
        let css = build_css(light, accent);
        self.provider.load_from_string(&css);
    }
}

fn build_css(light: bool, accent_hex: &str) -> String {
    let accent = if accent_hex.starts_with('#') && accent_hex.len() == 7 {
        accent_hex
    } else {
        "#0A84FF"
    };

    let (
        win_bg,
        sidebar_bg,
        card_bg,
        card_border,
        row_sep,
        sidebar_border,
        fg,
        muted,
        row_hover,
        search_bg,
        btn_bg,
        btn_hover,
        btn_active,
        btn_border,
        btn_top_hl,
        seg_track,
        seg_active,
        trough_bg,
        switch_off,
        card_shadow,
    ) = if light {
        (
            "#F5F5F7",
            "#EAEAEF",
            "#FFFFFF",
            "rgba(0, 0, 0, 0.08)",
            "rgba(0, 0, 0, 0.06)",
            "rgba(0, 0, 0, 0.10)",
            "#1D1D1F",
            "#6E6E73",
            "rgba(0, 0, 0, 0.05)",
            "rgba(0, 0, 0, 0.06)",
            "#FFFFFF",
            "#F0F0F3",
            "#E5E5EA",
            "rgba(0, 0, 0, 0.14)",
            "rgba(255, 255, 255, 0.90)",
            "rgba(0, 0, 0, 0.06)",
            "#FFFFFF",
            "rgba(0, 0, 0, 0.14)",
            "rgba(0, 0, 0, 0.16)",
            "0 1px 2px rgba(0, 0, 0, 0.04)",
        )
    } else {
        (
            "#222225",
            "#2B2B30",
            "#2B2B2F",
            "rgba(255, 255, 255, 0.075)",
            "rgba(255, 255, 255, 0.06)",
            "rgba(255, 255, 255, 0.09)",
            "#F5F5F7",
            "#98989F",
            "rgba(255, 255, 255, 0.06)",
            "rgba(255, 255, 255, 0.08)",
            "#44444A",
            "#505057",
            "#3A3A40",
            "rgba(255, 255, 255, 0.11)",
            "rgba(255, 255, 255, 0.16)",
            "rgba(255, 255, 255, 0.07)",
            "#48484F",
            "rgba(255, 255, 255, 0.16)",
            "rgba(255, 255, 255, 0.18)",
            "0 1px 3px rgba(0, 0, 0, 0.18)",
        )
    };

    format!(
        r#"
window.neon-settings-window {{
    background-color: {win_bg};
    color: {fg};
    font-family: "Inter", "SF Pro Text", "Adwaita Sans", "Ubuntu", sans-serif;
    font-size: 13px;
}}

/* ===================== SIDEBAR (macOS System Settings) ===================== */

.settings-sidebar {{
    background-color: {sidebar_bg};
    border-right: 1px solid {sidebar_border};
    padding: 0 10px 10px 10px;
}}

.settings-sidebar-top {{
    padding: 12px 6px 10px 6px;
}}

/* Crisp macOS traffic lights */
button.mac-dot {{
    background-image: none;
    border-radius: 999px;
    min-width: 12px;
    min-height: 12px;
    padding: 0;
    margin: 0;
    border: 1px solid rgba(0, 0, 0, 0.22);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.25);
}}

button.mac-dot-close {{ background-color: #FF5F57; }}
button.mac-dot-close:hover {{ background-color: #FF453A; }}
button.mac-dot-min {{ background-color: #FEBC2E; }}
button.mac-dot-min:hover {{ background-color: #FFB000; }}
button.mac-dot-max {{ background-color: #28C840; }}
button.mac-dot-max:hover {{ background-color: #20B838; }}

window.neon-settings-window:backdrop button.mac-dot {{
    background-color: alpha({muted}, 0.35);
    border-color: transparent;
    box-shadow: none;
}}

/* Compact macOS Sidebar Search */
entry.settings-search,
searchbar,
searchentry.settings-search {{
    background-color: {search_bg};
    color: {fg};
    border: 1px solid {card_border};
    border-radius: 7px;
    padding: 2px 8px;
    min-height: 28px;
    margin: 2px 2px 10px 2px;
    font-size: 12.5px;
    box-shadow: none;
}}

searchentry.settings-search:focus-within {{
    border-color: alpha({accent}, 0.85);
    box-shadow: 0 0 0 2px alpha({accent}, 0.24);
}}

.settings-nav-list {{
    background-color: transparent;
}}

.settings-nav-row {{
    border-radius: 6px;
    padding: 5px 8px;
    margin: 1px 0;
    color: {fg};
    font-size: 13px;
    font-weight: 500;
    min-height: 26px;
}}

.settings-nav-row:hover {{
    background-color: {row_hover};
}}

.settings-nav-row:selected {{
    background-color: #0A68D8;
    color: #FFFFFF;
    font-weight: 600;
}}

.settings-nav-row:selected label {{
    color: #FFFFFF;
}}

/* Colorful macOS 20x20 rounded-square sidebar icon badges */
.nav-badge {{
    border-radius: 5px;
    min-width: 20px;
    min-height: 20px;
    color: #FFFFFF;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.24), 0 1px 2px rgba(0, 0, 0, 0.18);
}}

.nav-badge image {{
    color: #FFFFFF;
    -gtk-icon-size: 12px;
}}

.nav-badge-blue   {{ background-color: #0A84FF; }}
.nav-badge-indigo {{ background-color: #5E5CE6; }}
.nav-badge-violet {{ background-color: #AF52DE; }}
.nav-badge-sky    {{ background-color: #32ADE6; }}
.nav-badge-teal   {{ background-color: #30B0C7; }}
.nav-badge-pink   {{ background-color: #FF2D55; }}
.nav-badge-green  {{ background-color: #30D158; }}
.nav-badge-orange {{ background-color: #FF9F0A; }}
.nav-badge-red    {{ background-color: #FF453A; }}
.nav-badge-gray   {{ background-color: #636366; }}
.nav-badge-slate  {{ background-color: #555760; }}

.settings-sidebar-sep {{
    background-color: {sidebar_border};
    min-height: 1px;
    margin: 6px 6px;
}}

button.settings-about-footer-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 6px;
    padding: 5px 8px;
    margin-top: 2px;
    color: {fg};
    font-size: 13px;
    font-weight: 500;
}}

button.settings-about-footer-btn:hover {{
    background-color: {row_hover};
}}

button.settings-about-footer-btn.active {{
    background-color: #0A68D8;
    color: #FFFFFF;
    font-weight: 600;
}}

button.settings-about-footer-btn.active label {{
    color: #FFFFFF;
}}

/* ===================== CONTENT HEADER (< > Title) ===================== */

.settings-content-header {{
    background-color: {win_bg};
    padding: 10px 18px 8px 14px;
    min-height: 36px;
}}

button.nav-chevron-btn {{
    background-image: none;
    background-color: transparent;
    color: {fg};
    border: none;
    box-shadow: none;
    border-radius: 6px;
    min-width: 26px;
    min-height: 26px;
    padding: 0;
}}

button.nav-chevron-btn:hover:not(:disabled) {{
    background-color: {row_hover};
}}

button.nav-chevron-btn:disabled {{
    opacity: 0.30;
}}

.settings-header-title {{
    font-size: 14.5px;
    font-weight: 700;
    color: {fg};
    margin-left: 6px;
}}

/* ===================== CONTENT CLAMP, SECTIONS & BOXED CARDS ===================== */

.settings-content-clamp {{
    padding: 6px 22px 24px 22px;
}}

.settings-section-title {{
    font-size: 12.5px;
    font-weight: 700;
    color: {fg};
    margin-top: 12px;
    margin-bottom: 6px;
    margin-left: 4px;
}}

.settings-card {{
    background-color: {card_bg};
    border: 1px solid {card_border};
    border-radius: 10px;
    padding: 2px 0;
    margin-bottom: 10px;
    box-shadow: {card_shadow};
}}

.settings-row {{
    padding: 9px 14px;
    min-height: 26px;
}}

.settings-row-sep {{
    background-color: {row_sep};
    min-height: 1px;
    margin: 0 14px;
}}

.settings-row-title {{
    font-size: 13px;
    font-weight: 500;
    color: {fg};
}}

.settings-row-desc {{
    font-size: 11.5px;
    color: {muted};
}}

/* ===================== STATUS BADGES ===================== */

.settings-badge {{
    background-color: {seg_track};
    color: {muted};
    border-radius: 999px;
    padding: 2px 9px;
    font-size: 11px;
    font-weight: 600;
}}

.settings-badge.ok {{
    background-color: alpha(#30D158, 0.16);
    color: #30D158;
}}

.settings-badge.partial {{
    background-color: alpha(#FF9F0A, 0.16);
    color: #FF9F0A;
}}

.settings-badge.unavailable {{
    background-color: alpha(#98989F, 0.16);
    color: {muted};
}}

/* ===================== NEUTRAL macOS BUTTONS (NO LOUD BLUE!) ===================== */

button.settings-action-btn,
button.settings-secondary-btn {{
    background-image: none;
    background-color: {btn_bg};
    color: {fg};
    border: 1px solid {btn_border};
    border-top-color: {btn_top_hl};
    border-radius: 6px;
    padding: 4px 12px;
    min-height: 24px;
    font-size: 12.5px;
    font-weight: 500;
    box-shadow: 0 1px 1px rgba(0, 0, 0, 0.14);
}}

button.settings-action-btn:hover,
button.settings-secondary-btn:hover {{
    background-color: {btn_hover};
}}

button.settings-action-btn:active,
button.settings-secondary-btn:active {{
    background-color: {btn_active};
}}

button.settings-destructive-btn {{
    background-image: none;
    background-color: alpha(#FF453A, 0.16);
    color: #FF6961;
    border: 1px solid alpha(#FF453A, 0.30);
    border-radius: 6px;
    padding: 4px 12px;
    min-height: 24px;
    font-size: 12.5px;
    font-weight: 600;
    box-shadow: none;
}}

button.settings-destructive-btn:hover {{
    background-color: #D93025;
    color: #FFFFFF;
    border-color: #D93025;
}}

/* ===================== SWITCHES, DROPDOWNS, SEGMENTED & SLIDERS ===================== */

switch {{
    background-image: none;
    background-color: {switch_off};
    border: none;
    border-radius: 12px;
    min-width: 40px;
    min-height: 22px;
    padding: 2px;
    box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.14);
}}

switch:checked {{
    background-image: none;
    background-color: {accent};
    border: none;
}}

switch > image {{
    color: transparent;
    opacity: 0;
    min-width: 0;
    min-height: 0;
}}

switch > slider,
switch:hover > slider,
switch:checked > slider,
switch:checked:hover > slider {{
    background-image: none;
    background-color: #FFFFFF;
    border: none;
    outline: none;
    margin: 0;
    border-radius: 9px;
    min-width: 18px;
    min-height: 18px;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.28);
}}

dropdown > button {{
    background-image: none;
    background-color: {btn_bg};
    color: {fg};
    border: 1px solid {btn_border};
    border-radius: 6px;
    padding: 3px 10px;
    min-height: 24px;
    font-size: 12.5px;
    box-shadow: 0 1px 1px rgba(0, 0, 0, 0.12);
}}

dropdown > button:hover {{
    background-color: {btn_hover};
}}

.settings-seg-track {{
    background-color: {seg_track};
    border: 1px solid {card_border};
    border-radius: 7px;
    padding: 2px;
}}

button.settings-seg-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 5px;
    padding: 4px 11px;
    font-size: 12px;
    font-weight: 500;
    color: {muted};
}}

button.settings-seg-btn.active {{
    background-color: {seg_active};
    color: {fg};
    font-weight: 600;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.18);
}}

scale,
scale.settings-scale {{
    padding: 8px 0;
    margin: 0;
}}

scale > trough,
scale.settings-scale > trough {{
    background-image: none;
    background-color: {trough_bg};
    border: none;
    border-radius: 3px;
    min-height: 4px;
    box-shadow: none;
}}

scale > trough > highlight,
scale.settings-scale > trough > highlight {{
    background-image: none;
    background-color: {accent};
    border: none;
    margin: 0;
    border-radius: 3px;
    box-shadow: none;
}}

scale > trough > slider,
scale > trough > slider:hover,
scale > trough > slider:active,
scale.settings-scale > trough > slider,
scale.settings-scale > trough > slider:hover,
scale.settings-scale > trough > slider:active {{
    background-image: none;
    background-color: #FFFFFF;
    border: none;
    outline: none;
    border-radius: 8px;
    min-width: 16px;
    min-height: 16px;
    margin: -6px 0;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.34);
}}

scale > value,
scale.settings-scale > value {{
    font-size: 11.5px;
    color: {muted};
}}

/* ===================== APPEARANCE PREVIEWS, ACCENTS & WALLPAPERS ===================== */

button.style-preview-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    padding: 4px;
    border-radius: 10px;
}}

.style-preview-frame {{
    border-radius: 9px;
    min-width: 120px;
    min-height: 74px;
    border: 2.5px solid transparent;
    padding: 8px;
}}

button.style-preview-btn.active .style-preview-frame {{
    border-color: {accent};
}}

.style-preview-light-bg {{
    background-image: linear-gradient(135deg, #60A5FA 0%, #3B82F6 50%, #2563EB 100%);
}}

.style-preview-dark-bg {{
    background-image: linear-gradient(135deg, #1E1B4B 0%, #1E3A8A 55%, #0F172A 100%);
}}

.style-preview-auto-bg {{
    background-image: linear-gradient(135deg, #60A5FA 0%, #3B82F6 49%, #1E293B 51%, #0F172A 100%);
}}

.style-mini-win-light {{
    background-color: #FFFFFF;
    border-radius: 5px;
    min-width: 64px;
    min-height: 40px;
    box-shadow: 0 3px 8px rgba(0, 0, 0, 0.24);
}}

.style-mini-win-dark {{
    background-color: #242428;
    border: 1px solid rgba(255, 255, 255, 0.14);
    border-radius: 5px;
    min-width: 64px;
    min-height: 40px;
    box-shadow: 0 3px 8px rgba(0, 0, 0, 0.36);
}}

button.accent-circle {{
    background-image: none;
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    padding: 0;
    border: 2px solid transparent;
    box-shadow: inset 0 1px 1px rgba(255, 255, 255, 0.25);
}}

button.accent-circle.active {{
    border: 2.5px solid {fg};
}}

button.accent-blue    {{ background-color: #0A84FF; }}
button.accent-cyan    {{ background-color: #38BDF8; }}
button.accent-violet  {{ background-color: #AF52DE; }}
button.accent-emerald {{ background-color: #30D158; }}
button.accent-amber   {{ background-color: #FF9F0A; }}
button.accent-rose    {{ background-color: #FF375F; }}
button.accent-pink    {{ background-color: #EC4899; }}

button.wp-thumb-btn {{
    background-image: none;
    background-color: {seg_track};
    border: 2.5px solid transparent;
    box-shadow: none;
    border-radius: 9px;
    padding: 2px;
}}

button.wp-thumb-btn:hover {{
    border-color: {btn_border};
}}

button.wp-thumb-btn.active {{
    border-color: {accent};
}}

.wp-thumb-pic {{
    border-radius: 6px;
}}

.wp-placeholder {{
    background-color: {seg_track};
    border-radius: 6px;
}}

/* ===================== SEARCH HITS, KEYCAPS, MONITOR CANVAS & TOAST ===================== */

button.search-hit-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    box-shadow: none;
    border-radius: 8px;
    padding: 8px 12px;
    color: {fg};
}}

button.search-hit-btn:hover {{
    background-color: {row_hover};
}}

.settings-status-bar {{
    padding: 5px 14px;
    border-top: 1px solid {card_border};
    background-color: {sidebar_bg};
    font-size: 11.5px;
    color: {muted};
}}

.settings-status-bar.status-ok {{
    color: #30D158;
}}

.settings-status-bar.status-error {{
    color: #FF6961;
}}

popover > contents {{
    background-color: {card_bg};
    color: {fg};
    border: 1px solid {card_border};
    border-radius: 9px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.28);
}}

.monitor-canvas {{
    background-color: {seg_track};
    border: 1px solid {card_border};
    border-radius: 8px;
}}

.kbd-pill {{
    background-color: {btn_bg};
    border: 1px solid {btn_border};
    border-radius: 5px;
    padding: 2px 8px;
    font-family: monospace;
    font-size: 11.5px;
    font-weight: 600;
    color: {fg};
}}

.neon-brand-emblem {{
    background-image: linear-gradient(135deg, #0A84FF 0%, #5E5CE6 100%);
    color: #FFFFFF;
    border-radius: 11px;
    font-size: 20px;
    font-weight: 800;
}}
"#
    )
}
