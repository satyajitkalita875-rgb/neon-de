//! 5.7 Input Devices: Segmented Mouse | Touchpad | Keyboard panels backed by libinput & XKB config.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, Orientation, ScrolledWindow, Stack, StackTransitionType,
    prelude::*,
};
use neon_config::{AccelProfile, TouchpadClickMethod};

use crate::{
    state::{SettingsController, apply_system_cursor, build_appearance_request},
    ui::{
        append_rows, boxed_card, dropdown_row, page_shell, section_heading, setting_row,
        slider_row, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Input Devices",
        "Configure libinput Mouse, Touchpad gestures, and XKB Keyboard layouts and repeat rates.",
        Some("input"),
        &ctrl,
    );

    // Segmented switcher: Mouse | Touchpad | Keyboard
    let seg_bar = GtkBox::new(Orientation::Horizontal, 4);
    seg_bar.add_css_class("settings-seg-track");
    seg_bar.set_halign(Align::Center);
    seg_bar.set_margin_bottom(10);

    let mouse_tab = Button::with_label("Mouse");
    mouse_tab.add_css_class("settings-seg-btn");
    mouse_tab.add_css_class("active");
    let tp_tab = Button::with_label("Touchpad");
    tp_tab.add_css_class("settings-seg-btn");
    let kb_tab = Button::with_label("Keyboard");
    kb_tab.add_css_class("settings-seg-btn");

    seg_bar.append(&mouse_tab);
    seg_bar.append(&tp_tab);
    seg_bar.append(&kb_tab);
    page.append(&seg_bar);

    let sub_stack = Stack::new();
    sub_stack.set_transition_type(StackTransitionType::Crossfade);
    sub_stack.set_transition_duration(100);
    sub_stack.add_named(&build_mouse_section(Rc::clone(&ctrl)), Some("mouse"));
    sub_stack.add_named(&build_touchpad_section(Rc::clone(&ctrl)), Some("touchpad"));
    sub_stack.add_named(&build_keyboard_section(Rc::clone(&ctrl)), Some("keyboard"));
    page.append(&sub_stack);

    for (btn, name) in [(&mouse_tab, "mouse"), (&tp_tab, "touchpad"), (&kb_tab, "keyboard")] {
        let st = sub_stack.clone();
        let m = mouse_tab.clone();
        let t = tp_tab.clone();
        let k = kb_tab.clone();
        btn.connect_clicked(move |_| {
            st.set_visible_child_name(name);
            m.remove_css_class("active");
            t.remove_css_class("active");
            k.remove_css_class("active");
            match name {
                "mouse" => m.add_css_class("active"),
                "touchpad" => t.add_css_class("active"),
                _ => k.add_css_class("active"),
            }
        });
    }

    scroller
}

fn build_mouse_section(ctrl: Rc<SettingsController>) -> GtkBox {
    let box_col = GtkBox::new(Orientation::Vertical, 4);
    box_col.append(&section_heading("Mouse Pointer & Scrolling"));
    let card = boxed_card();

    let inp = ctrl.config.borrow().input.clone();
    let cur_size = ctrl.config.borrow().appearance.cursor_size;
    let accel_idx = match inp.mouse_accel_profile {
        AccelProfile::Adaptive => 0,
        AccelProfile::Flat => 1,
    };
    let cursor_sizes: [u32; 5] = [16, 18, 20, 24, 32];
    let cur_idx = cursor_sizes
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| (**s as i32 - cur_size as i32).abs())
        .map(|(i, _)| i as u32)
        .unwrap_or(2);

    append_rows(
        &card,
        vec![
            slider_row(
                "Pointer Speed",
                "libinput mouse sensitivity (-1.0 slow to +1.0 fast).",
                -1.0,
                1.0,
                0.05,
                inp.pointer_speed,
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.pointer_speed = v;
                        ctrl_c.persist_and_sync(None, &format!("Mouse pointer speed: {v:.2}"));
                    }
                },
            ),
            toggle_row(
                "Pointer Acceleration",
                "Increase pointer travel speed during rapid mouse movements.",
                inp.mouse_accel_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.mouse_accel_enabled = v;
                        ctrl_c.persist_and_sync(None, "Updated mouse acceleration");
                    }
                },
            ),
            dropdown_row(
                "Acceleration Profile",
                "Adaptive curves smoothly; Flat provides raw 1:1 linear motion.",
                &["Adaptive (Default)", "Flat (Raw / Gaming)"],
                accel_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().input.mouse_accel_profile = if idx == 1 {
                            AccelProfile::Flat
                        } else {
                            AccelProfile::Adaptive
                        };
                        ctrl_c.persist_and_sync(None, "Updated acceleration profile");
                    }
                },
            ),
            toggle_row(
                "Left-Handed Mode",
                "Swap primary (left) and secondary (right) mouse buttons.",
                inp.left_handed,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.left_handed = v;
                        ctrl_c.persist_and_sync(None, "Updated left-handed mouse mode");
                    }
                },
            ),
            toggle_row(
                "Natural Scrolling",
                "Content tracks scroll direction like a touchscreen or macOS trackpad.",
                inp.natural_scroll,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.natural_scroll = v;
                        ctrl_c.persist_and_sync(None, "Updated natural scrolling");
                    }
                },
            ),
            slider_row(
                "Double-Click Interval (ms)",
                "Maximum time window between two clicks to register a double-click.",
                150.0,
                900.0,
                25.0,
                f64::from(inp.double_click_ms),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.double_click_ms = v.round() as u32;
                        ctrl_c.persist_and_sync(None, &format!("Double-click speed: {:.0} ms", v));
                    }
                },
            ),
            dropdown_row(
                "Pointer Size",
                "Live cursor size across NEON DE compositor and GTK apps.",
                &["16 px", "18 px", "20 px (macOS)", "24 px", "32 px (Large)"],
                cur_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let sz = cursor_sizes.get(idx as usize).copied().unwrap_or(20);
                        ctrl_c.config.borrow_mut().appearance.cursor_size = sz;
                        let theme = ctrl_c.config.borrow().appearance.cursor_theme.clone();
                        apply_system_cursor(&theme, sz);
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Pointer size set to {sz}px"));
                    }
                },
            ),
        ],
    );
    box_col.append(&card);
    box_col
}

fn build_touchpad_section(ctrl: Rc<SettingsController>) -> GtkBox {
    let box_col = GtkBox::new(Orientation::Vertical, 4);
    box_col.append(&section_heading("Touchpad & Multi-Touch Gestures"));
    let card = boxed_card();

    let inp = ctrl.config.borrow().input.clone();
    let click_idx = match inp.click_method {
        TouchpadClickMethod::Clickfinger => 0,
        TouchpadClickMethod::ButtonAreas => 1,
    };

    append_rows(
        &card,
        vec![
            toggle_row(
                "Enable Touchpad",
                "Turn integrated laptop touchpad input on or off.",
                inp.touchpad_enabled,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.touchpad_enabled = v;
                        ctrl_c.persist_and_sync(None, "Updated touchpad enabled state");
                    }
                },
            ),
            slider_row(
                "Touchpad Pointer Speed",
                "libinput touchpad tracking sensitivity (-1.0 to +1.0).",
                -1.0,
                1.0,
                0.05,
                inp.touchpad_speed,
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.touchpad_speed = v;
                        ctrl_c.persist_and_sync(None, &format!("Touchpad speed: {v:.2}"));
                    }
                },
            ),
            toggle_row(
                "Tap to Click",
                "Tap with one finger for primary click, two fingers for right-click.",
                inp.tap_to_click,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.tap_to_click = v;
                        ctrl_c.persist_and_sync(None, "Updated tap-to-click");
                    }
                },
            ),
            toggle_row(
                "Two-Finger Scrolling",
                "Scroll vertically and horizontally with two fingers.",
                inp.two_finger_scroll,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.two_finger_scroll = v;
                        ctrl_c.persist_and_sync(None, "Updated two-finger scrolling");
                    }
                },
            ),
            dropdown_row(
                "Secondary Click Method",
                "Clickfinger (two-finger press like macOS) or bottom-right corner button areas.",
                &["Two-Finger Click (Clickfinger)", "Bottom Corner Areas"],
                click_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().input.click_method = if idx == 1 {
                            TouchpadClickMethod::ButtonAreas
                        } else {
                            TouchpadClickMethod::Clickfinger
                        };
                        ctrl_c.persist_and_sync(None, "Updated touchpad click method");
                    }
                },
            ),
            toggle_row(
                "Disable While Typing (Palm Rejection)",
                "Ignore accidental palm touches while typing on the keyboard.",
                inp.disable_while_typing,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.disable_while_typing = v;
                        ctrl_c.persist_and_sync(None, "Updated disable-while-typing");
                    }
                },
            ),
            toggle_row(
                "Three-Finger Workspace & Overview Gestures",
                "Swipe horizontally for workspaces, swipe up for Overview.",
                inp.three_finger_gestures,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.three_finger_gestures = v;
                        ctrl_c.persist_and_sync(None, "Updated three-finger gestures");
                    }
                },
            ),
            toggle_row(
                "Four-Finger Gestures",
                "Enable extended four-finger swipe gestures.",
                inp.four_finger_gestures,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.four_finger_gestures = v;
                        ctrl_c.persist_and_sync(None, "Updated four-finger gestures");
                    }
                },
            ),
        ],
    );
    box_col.append(&card);
    box_col
}

fn build_keyboard_section(ctrl: Rc<SettingsController>) -> GtkBox {
    let box_col = GtkBox::new(Orientation::Vertical, 4);
    box_col.append(&section_heading("Keyboard Layouts & Typing"));
    let card = boxed_card();

    let inp = ctrl.config.borrow().input.clone();
    let layouts = ["us", "gb", "de", "fr", "es", "in", "jp", "ca", "pt", "it"];
    let layout_labels = [
        "English (US) [us]",
        "English (UK) [gb]",
        "German [de]",
        "French [fr]",
        "Spanish [es]",
        "English / Indic (India) [in]",
        "Japanese [jp]",
        "Canadian [ca]",
        "Portuguese [pt]",
        "Italian [it]",
    ];
    let cur_layout_idx = layouts
        .iter()
        .position(|l| *l == inp.keyboard_layout.as_str())
        .unwrap_or(0) as u32;

    let compose_vals = ["none", "ralt", "rctrl", "menu", "caps"];
    let compose_idx = compose_vals
        .iter()
        .position(|v| *v == inp.compose_key.as_str())
        .unwrap_or(0) as u32;

    let caps_vals = ["default", "ctrl", "escape", "disabled"];
    let caps_idx = caps_vals
        .iter()
        .position(|v| *v == inp.caps_lock_behavior.as_str())
        .unwrap_or(0) as u32;

    let open_shortcuts_btn = Button::with_label("Open Shortcut Manager →");
    open_shortcuts_btn.add_css_class("settings-action-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        open_shortcuts_btn.connect_clicked(move |_| ctrl_c.navigate_to("shortcuts", None));
    }

    append_rows(
        &card,
        vec![
            dropdown_row(
                "Primary XKB Keyboard Layout",
                "Active keyboard mapping for the Wayland compositor seat.",
                &layout_labels,
                cur_layout_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let code = layouts.get(idx as usize).copied().unwrap_or("us");
                        {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.input.keyboard_layout = code.into();
                            if !cfg.input.keyboard_layouts.iter().any(|l| l == code) {
                                cfg.input.keyboard_layouts.insert(0, code.into());
                            }
                        }
                        ctrl_c.persist_and_sync(None, &format!("Set primary layout to {code}"));
                    }
                },
            ),
            slider_row(
                "Key Repeat Rate (keys/sec)",
                "How fast held keys repeat.",
                5.0,
                80.0,
                1.0,
                f64::from(inp.keyboard_repeat_rate),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.keyboard_repeat_rate = v.round() as i32;
                        ctrl_c.persist_and_sync(None, &format!("Repeat rate: {:.0} keys/s", v));
                    }
                },
            ),
            slider_row(
                "Key Repeat Delay (ms)",
                "Wait duration before a held key begins repeating.",
                150.0,
                1200.0,
                25.0,
                f64::from(inp.keyboard_repeat_delay),
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().input.keyboard_repeat_delay = v.round() as i32;
                        ctrl_c.persist_and_sync(None, &format!("Repeat delay: {:.0} ms", v));
                    }
                },
            ),
            dropdown_row(
                "Compose Key",
                "Key used to type accented and special Unicode characters.",
                &["Disabled", "Right Alt", "Right Ctrl", "Menu Key", "Caps Lock"],
                compose_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let val = compose_vals.get(idx as usize).copied().unwrap_or("none");
                        ctrl_c.config.borrow_mut().input.compose_key = val.into();
                        ctrl_c.persist_and_sync(None, "Updated Compose key");
                    }
                },
            ),
            dropdown_row(
                "Caps Lock Behavior",
                "Keep standard Caps Lock or remap to Ctrl/Escape.",
                &["Default Caps Lock", "Act as Ctrl", "Act as Escape", "Disabled"],
                caps_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let val = caps_vals.get(idx as usize).copied().unwrap_or("default");
                        ctrl_c.config.borrow_mut().input.caps_lock_behavior = val.into();
                        ctrl_c.persist_and_sync(None, "Updated Caps Lock mapping");
                    }
                },
            ),
            setting_row(
                "Desktop & Custom Keyboard Shortcuts",
                "Search, customize, add custom bindings, and check for conflicts.",
                &open_shortcuts_btn,
            ),
        ],
    );
    box_col.append(&card);
    box_col
}
