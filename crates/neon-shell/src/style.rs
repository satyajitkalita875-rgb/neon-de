//! NEON DE visual design language (GTK4 CSS stylesheet — Neon Dark default theme).

use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, gdk::Display};

pub const NEON_DARK_CSS: &str = r#"
window.neon-panel-window,
window.neon-dock-window,
window.neon-overview-window {
    background-color: transparent;
}

.neon-top-panel {
    background-color: rgba(12, 16, 26, 0.86);
    border-bottom: 1px solid rgba(148, 163, 184, 0.16);
    padding: 4px 12px;
    color: #E2E8F0;
    font-size: 13px;
}

.neon-pill-button {
    background-color: rgba(30, 41, 59, 0.55);
    color: #E2E8F0;
    border: 1px solid rgba(148, 163, 184, 0.18);
    border-radius: 999px;
    padding: 3px 12px;
    min-height: 24px;
    font-weight: 600;
    transition: background-color 140ms ease, border-color 140ms ease;
}

.neon-pill-button:hover {
    background-color: rgba(56, 189, 248, 0.22);
    border-color: rgba(56, 189, 248, 0.55);
}

.neon-pill-button.active {
    background-color: rgba(56, 189, 248, 0.28);
    border-color: #38BDF8;
    color: #F8FAFC;
}

.neon-ws-pill {
    background-color: rgba(30, 41, 59, 0.45);
    color: #94A3B8;
    border: 1px solid transparent;
    border-radius: 999px;
    padding: 2px 10px;
    min-height: 22px;
    min-width: 24px;
    font-size: 12px;
    font-weight: 600;
}

.neon-ws-pill:hover {
    background-color: rgba(51, 65, 85, 0.75);
    color: #F8FAFC;
}

.neon-ws-pill.active {
    background-color: #38BDF8;
    color: #09101D;
}

.neon-clock {
    font-weight: 700;
    color: #F8FAFC;
    letter-spacing: 0.3px;
}

.neon-focused-title {
    color: #94A3B8;
    font-size: 12px;
}

.neon-dock-container {
    background-color: rgba(15, 21, 34, 0.84);
    border: 1px solid rgba(148, 163, 184, 0.22);
    border-radius: 20px;
    padding: 6px 10px;
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.45);
}

.neon-dock-item {
    background-color: rgba(30, 41, 59, 0.42);
    border: 1px solid transparent;
    border-radius: 14px;
    padding: 6px 10px;
    min-width: 44px;
    min-height: 44px;
    color: #E2E8F0;
    transition: background-color 140ms ease, border-color 140ms ease;
}

.neon-dock-item:hover {
    background-color: rgba(56, 189, 248, 0.20);
    border-color: rgba(56, 189, 248, 0.45);
}

.neon-dock-item.running {
    border-bottom: 2px solid #38BDF8;
}

.neon-dock-item.focused {
    background-color: rgba(56, 189, 248, 0.25);
    border-color: #38BDF8;
}

.neon-dock-separator {
    background-color: rgba(148, 163, 184, 0.25);
    min-width: 1px;
    margin: 6px 4px;
}

.neon-overview-scrim {
    background-color: rgba(9, 13, 22, 0.86);
    padding: 28px 36px;
    color: #E2E8F0;
}

.neon-search-entry {
    background-color: rgba(24, 33, 52, 0.92);
    color: #F8FAFC;
    border: 1px solid rgba(56, 189, 248, 0.45);
    border-radius: 14px;
    padding: 8px 16px;
    font-size: 15px;
    min-width: 380px;
}

.neon-section-heading {
    color: #94A3B8;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.9px;
}

.neon-workspace-card {
    background-color: rgba(24, 33, 52, 0.78);
    border: 1px solid rgba(148, 163, 184, 0.22);
    border-radius: 14px;
    padding: 10px 14px;
    min-width: 150px;
    min-height: 76px;
    color: #E2E8F0;
}

.neon-workspace-card.active {
    border-color: #38BDF8;
    background-color: rgba(30, 58, 86, 0.65);
}

.neon-window-card {
    background-color: rgba(24, 33, 52, 0.82);
    border: 1px solid rgba(148, 163, 184, 0.20);
    border-radius: 14px;
    padding: 12px;
    min-width: 210px;
    color: #E2E8F0;
}

.neon-window-card.focused {
    border-color: #38BDF8;
}

.neon-app-tile {
    background-color: rgba(24, 33, 52, 0.65);
    border: 1px solid rgba(148, 163, 184, 0.14);
    border-radius: 14px;
    padding: 10px;
    min-width: 112px;
    min-height: 92px;
    color: #E2E8F0;
}

.neon-app-tile:hover {
    background-color: rgba(56, 189, 248, 0.22);
    border-color: #38BDF8;
}

.neon-close-button {
    background-color: rgba(239, 68, 68, 0.25);
    color: #FCA5A5;
    border-radius: 999px;
    min-width: 22px;
    min-height: 22px;
    padding: 0 6px;
}

.neon-close-button:hover {
    background-color: rgba(239, 68, 68, 0.7);
    color: #FFFFFF;
}
"#;

pub fn load_css() {
    let provider = CssProvider::new();
    provider.load_from_string(NEON_DARK_CSS);
    if let Some(display) = Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
