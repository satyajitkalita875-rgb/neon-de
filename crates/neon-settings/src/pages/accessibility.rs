//! 5.17 Accessibility settings: High Contrast, Reduced Motion, Text Scaling, Large Pointer,
//! Sticky/Slow/Bounce/Mouse keys, and honest host detection for Screen Reader / OSK / Magnifier.

use std::{process::Command, rc::Rc};

use gtk4::{ScrolledWindow, prelude::*};

use crate::{
    metadata::CapabilityStatus,
    state::{
        SettingsController, apply_system_cursor, apply_toolkit_appearance,
        build_appearance_request, build_effects_request,
    },
    ui::{
        append_rows, boxed_card, capability_row, page_shell, section_heading, slider_row,
        toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Accessibility",
        "Improve readability, pointer visibility, motion comfort, and keyboard ergonomics.",
        Some("accessibility"),
        &ctrl,
    );

    let a11y = ctrl.config.borrow().privacy_accessibility.clone();
    let reduced_motion = ctrl.config.borrow().effects.reduced_motion;

    // ── Seeing & Motion ──────────────────────────────────────────────────────
    page.append(&section_heading("Vision & Motion"));
    let vision_card = boxed_card();

    append_rows(
        &vision_card,
        vec![
            toggle_row(
                "High Contrast Mode",
                "Boosts contrast borders and readability across NEON DE panels and controls (live IPC).",
                a11y.high_contrast,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c
                            .config
                            .borrow_mut()
                            .privacy_accessibility
                            .high_contrast = active;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated High Contrast mode");
                    }
                },
            ),
            toggle_row(
                "Reduce Motion",
                "Disables animations and rapid zoom transitions across the compositor (live IPC).",
                reduced_motion,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().effects.reduced_motion = active;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated Reduce Motion");
                    }
                },
            ),
            slider_row(
                "Text Scale Factor",
                "Enlarges UI typography across GTK applications (0.85x to 1.75x).",
                0.85,
                1.75,
                0.05,
                a11y.text_scale,
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.privacy_accessibility.text_scale = val;
                            cfg.appearance.font_scale = val;
                        }
                        apply_toolkit_appearance(None, None, None, Some(val));
                        ctrl_c.persist_and_sync(None, &format!("Text scale set to {val:.2}x"));
                    }
                },
            ),
            toggle_row(
                "Large Pointer",
                "Switches mouse cursor to a high-visibility 32 px size live across Wayland & GTK.",
                a11y.large_pointer,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        let (theme, sz) = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.privacy_accessibility.large_pointer = active;
                            cfg.appearance.cursor_size = if active { 32 } else { 20 };
                            (
                                cfg.appearance.cursor_theme.clone(),
                                cfg.appearance.cursor_size,
                            )
                        };
                        apply_system_cursor(&theme, sz);
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(
                            Some(req),
                            if active {
                                "Enabled Large Pointer (32 px)"
                            } else {
                                "Restored standard pointer size (20 px)"
                            },
                        );
                    }
                },
            ),
        ],
    );
    page.append(&vision_card);

    // ── Typing & Pointer Assistance ──────────────────────────────────────────
    page.append(&section_heading("Typing & Pointer Assistance"));
    let typing_card = boxed_card();

    append_rows(
        &typing_card,
        vec![
            toggle_row(
                "Sticky Keys",
                "Press modifier keys (Shift, Ctrl, Alt, Super) sequentially instead of simultaneously.",
                a11y.sticky_keys,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().privacy_accessibility.sticky_keys = v;
                        ctrl_c.persist_and_sync(None, "Updated Sticky Keys");
                    }
                },
            ),
            toggle_row(
                "Slow Keys",
                "Require a short hold duration before registering keypresses.",
                a11y.slow_keys,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().privacy_accessibility.slow_keys = v;
                        ctrl_c.persist_and_sync(None, "Updated Slow Keys");
                    }
                },
            ),
            toggle_row(
                "Bounce Keys",
                "Ignore accidental rapid duplicate keypresses.",
                a11y.bounce_keys,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().privacy_accessibility.bounce_keys = v;
                        ctrl_c.persist_and_sync(None, "Updated Bounce Keys");
                    }
                },
            ),
            toggle_row(
                "Mouse Keys",
                "Control the mouse pointer using the numeric keypad.",
                a11y.mouse_keys,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().privacy_accessibility.mouse_keys = v;
                        ctrl_c.persist_and_sync(None, "Updated Mouse Keys");
                    }
                },
            ),
        ],
    );
    page.append(&typing_card);

    // ── Assistive Services (Honest Host Capability Detection) ────────────────
    page.append(&section_heading("Assistive Technologies"));
    let assist_card = boxed_card();

    let orca_installed = binary_available("orca");
    let osk_installed = binary_available("onboard") || binary_available("squeekboard");

    let mut assist_rows = Vec::new();
    if orca_installed {
        assist_rows.push(toggle_row(
            "Screen Reader (Orca)",
            "Spoken accessibility feedback using the installed Orca AT-SPI screen reader.",
            a11y.screen_reader,
            {
                let ctrl_c = Rc::clone(&ctrl);
                move |v| {
                    ctrl_c.config.borrow_mut().privacy_accessibility.screen_reader = v;
                    if v {
                        let _ = Command::new("orca").arg("--replace").spawn();
                    }
                    ctrl_c.persist_and_sync(None, "Updated Screen Reader preference");
                }
            },
        ));
    } else {
        assist_rows.push(capability_row(
            "Screen Reader (Orca)",
            "Install the `orca` package to enable spoken AT-SPI screen reading.",
            CapabilityStatus::TemporarilyUnavailable,
        ));
    }

    if osk_installed {
        assist_rows.push(toggle_row(
            "On-Screen Keyboard",
            "Show virtual touch keyboard when focusing text inputs.",
            a11y.on_screen_keyboard,
            {
                let ctrl_c = Rc::clone(&ctrl);
                move |v| {
                    ctrl_c
                        .config
                        .borrow_mut()
                        .privacy_accessibility
                        .on_screen_keyboard = v;
                    ctrl_c.persist_and_sync(None, "Updated On-Screen Keyboard preference");
                }
            },
        ));
    } else {
        assist_rows.push(capability_row(
            "On-Screen Keyboard",
            "Install `squeekboard` or `onboard` for virtual keyboard support.",
            CapabilityStatus::TemporarilyUnavailable,
        ));
    }

    assist_rows.push(capability_row(
        "Compositor Screen Magnifier",
        "Hardware zoom lens shader.",
        CapabilityStatus::Unsupported(
            "Viewport magnifier shader is not yet implemented in neon-renderer.",
        ),
    ));

    append_rows(&assist_card, assist_rows);
    page.append(&assist_card);

    scroller
}

fn binary_available(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}
