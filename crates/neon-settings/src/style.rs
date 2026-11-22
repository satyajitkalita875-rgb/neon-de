//! Live GTK4 CSS theme for `neon-settings` supporting both Neon Dark and Neon Light modes.

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
    let (win_bg, sidebar_bg, card_bg, fg, muted, border, active_fg) = if light {
        (
            "#F1F5F9",
            "#E8EEF6",
            "#FFFFFF",
            "#0F172A",
            "#475569",
            "rgba(15, 23, 42, 0.12)",
            "#FFFFFF",
        )
    } else {
        (
            "#0B101B",
            "#101726",
            "#161F33",
            "#E2E8F0",
            "#94A3B8",
            "rgba(148, 163, 184, 0.16)",
            "#07101C",
        )
    };

    format!(
        r#"
window.neon-settings-window {{
    background-color: {win_bg};
    color: {fg};
}}

.settings-sidebar {{
    background-color: {sidebar_bg};
    border-right: 1px solid {border};
    padding: 14px 10px;
}}

.settings-search {{
    background-color: {card_bg};
    color: {fg};
    border: 1px solid {border};
    border-radius: 10px;
    padding: 4px 10px;
    margin-bottom: 10px;
}}

.settings-nav-row {{
    border-radius: 10px;
    padding: 8px 12px;
    margin: 2px 0;
    color: {fg};
}}

.settings-nav-row:selected {{
    background-color: {accent};
    color: {active_fg};
    font-weight: 700;
}}

.settings-content {{
    padding: 24px 32px;
}}

.settings-page-title {{
    font-size: 22px;
    font-weight: 700;
    color: {fg};
    margin-bottom: 4px;
}}

.settings-page-subtitle {{
    font-size: 13px;
    color: {muted};
    margin-bottom: 16px;
}}

.settings-card {{
    background-color: {card_bg};
    border: 1px solid {border};
    border-radius: 14px;
    padding: 14px 18px;
    margin-bottom: 14px;
}}

.settings-card-title {{
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.7px;
    color: {muted};
    margin-bottom: 8px;
}}

.settings-row-title {{
    font-size: 14px;
    font-weight: 600;
    color: {fg};
}}

.settings-row-desc {{
    font-size: 12px;
    color: {muted};
}}

.settings-action-btn {{
    background-color: {accent};
    color: {active_fg};
    border-radius: 10px;
    padding: 6px 14px;
    font-weight: 700;
}}

.settings-secondary-btn {{
    background-color: transparent;
    color: {fg};
    border: 1px solid {border};
    border-radius: 10px;
    padding: 6px 14px;
    font-weight: 600;
}}

.settings-swatch {{
    min-width: 28px;
    min-height: 28px;
    border-radius: 999px;
    border: 2px solid {border};
}}

.settings-status-bar {{
    background-color: {sidebar_bg};
    border-top: 1px solid {border};
    padding: 6px 16px;
    font-size: 12px;
    color: {muted};
}}
"#
    )
}
