//! Frosted-glass Liquid Glass CSS for `neon-screencast-picker`, matching NEON DE's
//! Quick Settings, Workspaces Sidebar, Dark/Light themes, accent color, and unified System Blur.

use gtk4::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION, gdk::Display};

pub fn apply_picker_css(
    light_theme: bool,
    accent_hex: &str,
    blur_active: bool,
    high_contrast: bool,
) {
    let accent = if accent_hex.starts_with('#') && accent_hex.len() == 7 {
        accent_hex
    } else {
        "#0A84FF"
    };
    let accent_soft = hex_to_rgba(accent, if light_theme { 0.16 } else { 0.22 });
    let accent_hover = hex_to_rgba(accent, if light_theme { 0.88 } else { 0.90 });
    let accent_glow = hex_to_rgba(accent, if light_theme { 0.26 } else { 0.36 });

    let (
        backdrop_scrim,
        shell_bg,
        shell_border,
        shell_shadow,
        header_bg,
        card_bg,
        card_hover,
        card_border,
        seg_bg,
        seg_active_bg,
        text_primary,
        text_muted,
        thumb_bg,
        pill_bg,
        cancel_bg,
        cancel_hover,
        sep_col,
        share_fg,
    ) = if light_theme {
        (
            if high_contrast {
                "rgba(15, 23, 42, 0.32)"
            } else {
                "rgba(15, 23, 42, 0.16)"
            },
            if high_contrast {
                "#FFFFFF"
            } else if blur_active {
                "rgba(244, 248, 255, 0.72)"
            } else {
                "rgba(245, 247, 252, 0.94)"
            },
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(255, 255, 255, 0.85)"
            },
            "0 20px 54px rgba(15, 23, 42, 0.22), inset 0 1px 0 rgba(255, 255, 255, 0.95)",
            if high_contrast {
                "#F1F5F9"
            } else {
                "rgba(255, 255, 255, 0.32)"
            },
            if high_contrast {
                "#FFFFFF"
            } else if blur_active {
                "rgba(255, 255, 255, 0.56)"
            } else {
                "rgba(255, 255, 255, 0.84)"
            },
            "rgba(255, 255, 255, 0.84)",
            if high_contrast {
                "#0F172A"
            } else {
                "rgba(15, 23, 42, 0.10)"
            },
            "rgba(15, 23, 42, 0.07)",
            "rgba(255, 255, 255, 0.92)",
            "#0B1220",
            "rgba(15, 23, 42, 0.62)",
            "rgba(15, 23, 42, 0.08)",
            "rgba(15, 23, 42, 0.09)",
            "rgba(15, 23, 42, 0.08)",
            "rgba(15, 23, 42, 0.14)",
            "rgba(15, 23, 42, 0.09)",
            "#FFFFFF",
        )
    } else {
        (
            if high_contrast {
                "rgba(2, 6, 16, 0.52)"
            } else {
                "rgba(4, 7, 16, 0.30)"
            },
            if high_contrast {
                "#0B1120"
            } else if blur_active {
                "rgba(15, 20, 32, 0.64)"
            } else {
                "rgba(20, 24, 36, 0.94)"
            },
            if high_contrast {
                "#F8FAFC"
            } else {
                "rgba(255, 255, 255, 0.20)"
            },
            "0 24px 64px rgba(0, 0, 0, 0.48), inset 0 1px 0 rgba(255, 255, 255, 0.24)",
            if high_contrast {
                "#111827"
            } else {
                "rgba(255, 255, 255, 0.04)"
            },
            if high_contrast {
                "#111827"
            } else if blur_active {
                "rgba(255, 255, 255, 0.075)"
            } else {
                "rgba(255, 255, 255, 0.08)"
            },
            "rgba(255, 255, 255, 0.13)",
            if high_contrast {
                "#E2E8F0"
            } else {
                "rgba(255, 255, 255, 0.12)"
            },
            "rgba(255, 255, 255, 0.07)",
            "rgba(255, 255, 255, 0.16)",
            "#F8FAFC",
            "rgba(226, 232, 240, 0.66)",
            "rgba(6, 9, 18, 0.56)",
            "rgba(255, 255, 255, 0.12)",
            "rgba(255, 255, 255, 0.09)",
            "rgba(255, 255, 255, 0.16)",
            "rgba(255, 255, 255, 0.10)",
            "#FFFFFF",
        )
    };

    let css = format!(
        r#"
* {{
    font-family: "Adwaita Sans", "Inter", "SF Pro Text", sans-serif;
}}

window.picker-backdrop-window {{
    background-color: transparent;
    background-image: none;
}}

.picker-backdrop-scrim {{
    background-color: {backdrop_scrim};
}}

window.picker-window {{
    background-color: transparent;
    background-image: none;
    color: {text_primary};
}}

.picker-shell {{
    background-color: {shell_bg};
    border: 1px solid {shell_border};
    border-radius: 22px;
    box-shadow: {shell_shadow};
    color: {text_primary};
}}

.picker-header {{
    background-color: {header_bg};
    border-top-left-radius: 21px;
    border-top-right-radius: 21px;
    border-bottom: 1px solid {sep_col};
    padding: 16px 22px 14px 22px;
}}

.picker-title {{
    font-size: 16.5px;
    font-weight: 700;
    letter-spacing: -0.2px;
    color: {text_primary};
}}

.picker-subtitle {{
    font-size: 12px;
    color: {text_muted};
}}

.picker-close-btn {{
    background-image: none;
    background-color: {cancel_bg};
    border: 1px solid {card_border};
    border-radius: 999px;
    min-width: 30px;
    min-height: 30px;
    padding: 0;
    color: {text_muted};
    transition: background-color 140ms ease, color 140ms ease;
}}

.picker-close-btn:hover {{
    background-image: none;
    background-color: {cancel_hover};
    color: {text_primary};
}}

.picker-tabs {{
    background-color: {seg_bg};
    border-radius: 999px;
    padding: 3px;
    border: 1px solid {card_border};
}}

button.picker-tab-btn {{
    background-image: none;
    background-color: transparent;
    border: none;
    border-radius: 999px;
    padding: 6px 16px;
    color: {text_muted};
    font-weight: 600;
    font-size: 12.5px;
    box-shadow: none;
    transition: background-color 140ms ease, color 140ms ease;
}}

button.picker-tab-btn:hover {{
    color: {text_primary};
}}

button.picker-tab-btn.active {{
    background-image: none;
    background-color: {seg_active_bg};
    color: {text_primary};
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.16), inset 0 1px 0 rgba(255, 255, 255, 0.18);
}}

.picker-tab-count {{
    background-color: {pill_bg};
    border-radius: 999px;
    padding: 1px 7px;
    font-size: 10.5px;
    font-weight: 700;
    color: {text_muted};
}}

button.picker-tab-btn.active .picker-tab-count {{
    background-color: {accent};
    color: {share_fg};
}}

entry.picker-search {{
    background-image: none;
    background-color: {card_bg};
    color: {text_primary};
    border: 1px solid {card_border};
    border-radius: 12px;
    padding: 6px 12px;
    min-height: 36px;
    font-size: 13px;
    box-shadow: none;
}}

entry.picker-search:focus-within {{
    border-color: {accent};
    box-shadow: 0 0 0 2px {accent_soft};
}}

button.picker-card {{
    background-image: none;
    background-color: {card_bg};
    border: 2px solid {card_border};
    border-radius: 16px;
    padding: 10px;
    box-shadow: none;
    transition: border-color 130ms ease, background-color 130ms ease, box-shadow 130ms ease, transform 140ms cubic-bezier(0.22, 1.0, 0.36, 1.0);
}}

button.picker-card:hover {{
    background-image: none;
    background-color: {card_hover};
    border-color: rgba(148, 163, 184, 0.42);
    transform: translateY(-1px);
}}

button.picker-card.selected {{
    background-image: none;
    background-color: {accent_soft};
    border-color: {accent};
    box-shadow: 0 0 0 1px {accent_glow};
}}

.picker-thumb-frame {{
    background-color: {thumb_bg};
    border-radius: 10px;
    border: 1px solid {card_border};
}}

.picker-thumb-pic {{
    border-radius: 9px;
}}

.picker-thumb-placeholder {{
    color: {text_muted};
    opacity: 0.55;
}}

.picker-badge {{
    background-color: rgba(12, 16, 28, 0.76);
    color: #F8FAFC;
    border: 1px solid rgba(255, 255, 255, 0.16);
    border-radius: 999px;
    padding: 2px 9px;
    font-size: 10px;
    font-weight: 700;
    margin: 7px;
}}

.picker-badge-accent {{
    background-color: {accent};
    border-color: transparent;
    color: {share_fg};
}}

.picker-card-title {{
    font-size: 13px;
    font-weight: 650;
    color: {text_primary};
}}

.picker-card-sub {{
    font-size: 11.5px;
    color: {text_muted};
}}

.picker-empty-title {{
    font-size: 14px;
    font-weight: 600;
    color: {text_primary};
}}

.picker-empty-sub {{
    font-size: 12px;
    color: {text_muted};
}}

.picker-footer {{
    background-color: {header_bg};
    border-bottom-left-radius: 21px;
    border-bottom-right-radius: 21px;
    border-top: 1px solid {sep_col};
    padding: 13px 22px;
}}

.picker-selection-note {{
    font-size: 12px;
    font-weight: 500;
    color: {text_muted};
}}

button.picker-cancel-btn {{
    background-image: none;
    background-color: {cancel_bg};
    color: {text_primary};
    border: 1px solid {card_border};
    border-radius: 999px;
    padding: 7px 20px;
    font-size: 13px;
    font-weight: 600;
    box-shadow: none;
    transition: background-color 140ms ease;
}}

button.picker-cancel-btn:hover {{
    background-image: none;
    background-color: {cancel_hover};
}}

button.picker-share-btn {{
    background-image: none;
    background-color: {accent};
    color: {share_fg};
    border: none;
    border-radius: 999px;
    padding: 7px 24px;
    font-size: 13px;
    font-weight: 700;
    box-shadow: 0 4px 14px {accent_glow};
    transition: background-color 140ms ease, transform 120ms ease;
}}

button.picker-share-btn:hover {{
    background-image: none;
    background-color: {accent_hover};
}}

button.picker-share-btn:disabled {{
    opacity: 0.42;
    box-shadow: none;
}}

scrollbar slider {{
    background-color: {pill_bg};
    border-radius: 999px;
    min-width: 5px;
}}
"#
    );

    if let Some(display) = Display::default() {
        let provider = CssProvider::new();
        provider.load_from_string(&css);
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn hex_to_rgba(hex: &str, alpha: f32) -> String {
    let stripped = hex.trim_start_matches('#');
    if stripped.len() == 6
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&stripped[0..2], 16),
            u8::from_str_radix(&stripped[2..4], 16),
            u8::from_str_radix(&stripped[4..6], 16),
        )
    {
        return format!("rgba({r}, {g}, {b}, {alpha:.2})");
    }
    format!("rgba(10, 132, 255, {alpha:.2})")
}
