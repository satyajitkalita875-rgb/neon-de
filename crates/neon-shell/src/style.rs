//! Dynamic NEON DE visual design language (Neon Dark & Neon Light themes with live accent color
//! and panel/dock transparency updates via `neon-ipc`).

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
            dock_transparency: 0.84,
            panel_transparency: 0.86,
            panel_visible: true,
            show_clock: true,
            show_workspaces: true,
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
    let accent = &appearance.accent_color;
    let panel_a = appearance.panel_transparency.clamp(0.25, 1.0);
    let dock_a = appearance.dock_transparency.clamp(0.25, 1.0);

    let (
        panel_bg,
        dock_bg,
        card_bg,
        pill_bg,
        scrim_bg,
        fg_primary,
        fg_muted,
        border_subtle,
        ws_active_fg,
    ) = if is_light {
        (
            format!("rgba(244, 247, 252, {panel_a:.2})"),
            format!("rgba(248, 250, 252, {dock_a:.2})"),
            "rgba(255, 255, 255, 0.88)",
            "rgba(226, 232, 240, 0.78)",
            "rgba(241, 245, 249, 0.90)",
            "#0F172A",
            "#475569",
            "rgba(15, 23, 42, 0.14)",
            "#FFFFFF",
        )
    } else {
        (
            format!("rgba(12, 16, 26, {panel_a:.2})"),
            format!("rgba(15, 21, 34, {dock_a:.2})"),
            "rgba(24, 33, 52, 0.82)",
            "rgba(30, 41, 59, 0.55)",
            "rgba(9, 13, 22, 0.86)",
            "#E2E8F0",
            "#94A3B8",
            "rgba(148, 163, 184, 0.18)",
            "#09101D",
        )
    };

    format!(
        r#"
window.neon-panel-window,
window.neon-dock-window,
window.neon-overview-window {{
    background-color: transparent;
}}

.neon-top-panel {{
    background-color: {panel_bg};
    border-bottom: 1px solid {border_subtle};
    padding: 4px 12px;
    color: {fg_primary};
    font-size: 13px;
}}

.neon-pill-button {{
    background-color: {pill_bg};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    border-radius: 999px;
    padding: 3px 12px;
    min-height: 24px;
    font-weight: 600;
    transition: background-color 140ms ease, border-color 140ms ease;
}}

.neon-pill-button:hover {{
    border-color: {accent};
}}

.neon-pill-button.active {{
    border-color: {accent};
    background-color: alpha({accent}, 0.24);
}}

.neon-ws-pill {{
    background-color: {pill_bg};
    color: {fg_muted};
    border: 1px solid transparent;
    border-radius: 999px;
    padding: 2px 10px;
    min-height: 22px;
    min-width: 24px;
    font-size: 12px;
    font-weight: 600;
}}

.neon-ws-pill:hover {{
    border-color: {accent};
    color: {fg_primary};
}}

.neon-ws-pill.active {{
    background-color: {accent};
    color: {ws_active_fg};
}}

.neon-clock {{
    font-weight: 700;
    color: {fg_primary};
    letter-spacing: 0.3px;
}}

.neon-focused-title {{
    color: {fg_muted};
    font-size: 12px;
}}

.neon-dock-container {{
    background-color: {dock_bg};
    border: 1px solid {border_subtle};
    border-radius: 20px;
    padding: 6px 10px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.35);
}}

.neon-dock-item {{
    background-color: {pill_bg};
    border: 1px solid transparent;
    border-radius: 14px;
    padding: 6px 10px;
    min-width: 44px;
    min-height: 44px;
    color: {fg_primary};
    transition: background-color 140ms ease, border-color 140ms ease;
}}

.neon-dock-item:hover {{
    border-color: {accent};
    background-color: alpha({accent}, 0.20);
}}

.neon-dock-item.running {{
    border-bottom: 2px solid {accent};
}}

.neon-dock-item.focused {{
    background-color: alpha({accent}, 0.26);
    border-color: {accent};
}}

.neon-dock-separator {{
    background-color: {border_subtle};
    min-width: 1px;
    margin: 6px 4px;
}}

.neon-overview-scrim {{
    background-color: {scrim_bg};
    padding: 28px 36px;
    color: {fg_primary};
}}

.neon-search-entry {{
    background-color: {card_bg};
    color: {fg_primary};
    border: 1px solid {accent};
    border-radius: 14px;
    padding: 8px 16px;
    font-size: 15px;
    min-width: 380px;
}}

.neon-section-heading {{
    color: {fg_muted};
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.9px;
}}

.neon-workspace-card {{
    background-color: {card_bg};
    border: 1px solid {border_subtle};
    border-radius: 14px;
    padding: 10px 14px;
    min-width: 150px;
    min-height: 76px;
    color: {fg_primary};
}}

.neon-workspace-card.active {{
    border-color: {accent};
    background-color: alpha({accent}, 0.18);
}}

.neon-window-card {{
    background-color: {card_bg};
    border: 1px solid {border_subtle};
    border-radius: 14px;
    padding: 12px;
    min-width: 210px;
    color: {fg_primary};
}}

.neon-window-card.focused {{
    border-color: {accent};
}}

.neon-app-tile {{
    background-color: {card_bg};
    border: 1px solid {border_subtle};
    border-radius: 14px;
    padding: 10px;
    min-width: 112px;
    min-height: 92px;
    color: {fg_primary};
}}

.neon-app-tile:hover {{
    background-color: alpha({accent}, 0.22);
    border-color: {accent};
}}

.neon-close-button {{
    background-color: rgba(239, 68, 68, 0.25);
    color: #FCA5A5;
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    padding: 0 6px;
}}

.neon-close-button:hover {{
    background-color: rgba(239, 68, 68, 0.75);
    color: #FFFFFF;
}}
"#
    )
}
