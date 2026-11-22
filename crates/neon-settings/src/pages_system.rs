//! System, Connectivity, Hardware, Input (Mouse/Touchpad & Keyboard), Accessibility,
//! Privacy & Security, and About System pages matching the GNOME 47 / macOS reference design.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, CheckButton, DropDown, Image, Label, Orientation, ProgressBar,
    Scale, ScrolledWindow, Stack, StackTransitionType, StringList, Switch, prelude::*,
};
use neon_config::{NeonConfig, export_config};
use neon_ipc::{IpcClient, IpcPayload, IpcRequest, IpcResult};
use neon_system::{
    audio::{set_default_sink, set_sink_mute, set_sink_volume, toggle_source_mute},
    bluetooth::{set_bluetooth_powered, set_device_connected},
    network::set_wifi_enabled,
    notifications::{NESTED_BUS_NAME, NotificationUrgency, send_dbus_notification},
    power::set_power_profile,
    query_system_snapshot,
};

use crate::{
    pages::{
        SettingsController, append_rows, boxed_card, build_appearance_request,
        build_effects_request, page_clamp, section_heading, setting_row,
    },
    system_info::collect_hardware_summary,
};

// ============================================================================
// NETWORK
// ============================================================================

pub fn build_network_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    let sys = query_system_snapshot();

    page.append(&section_heading("Wi-Fi & Connectivity"));
    let card = boxed_card();

    let wifi_sw = Switch::new();
    wifi_sw.set_valign(Align::Center);
    wifi_sw.set_sensitive(sys.network.available);
    wifi_sw.set_active(sys.network.wifi_enabled);
    {
        let ctrl = Rc::clone(&ctrl);
        wifi_sw.connect_state_set(move |_, enabled| {
            match set_wifi_enabled(enabled) {
                Ok(()) => ctrl.status_label.set_text(if enabled {
                    "Wi-Fi enabled via NetworkManager"
                } else {
                    "Wi-Fi disabled via NetworkManager"
                }),
                Err(err) => ctrl.status_label.set_text(&format!("Network error: {err}")),
            }
            gtk4::glib::Propagation::Proceed
        });
    }

    let status_badge = Label::new(Some(&sys.network.panel_label()));
    status_badge.add_css_class("settings-badge");
    if sys.network.connectivity.is_connected() {
        status_badge.add_css_class("ok");
    }

    let conns = if sys.network.active_connections.is_empty() {
        "None".to_string()
    } else {
        sys.network.active_connections.join(", ")
    };
    let conns_lbl = Label::new(Some(&conns));
    conns_lbl.add_css_class("settings-row-desc");

    append_rows(
        &card,
        vec![
            setting_row(
                "Wi-Fi",
                "Enable or disable wireless radio (NetworkManager D-Bus).",
                &wifi_sw,
            ),
            setting_row(
                "Connection Status",
                "Live primary network link and signal.",
                &status_badge,
            ),
            setting_row(
                "Active Profiles",
                "Currently active NetworkManager connections.",
                &conns_lbl,
            ),
        ],
    );
    page.append(&card);

    scroller
}

// ============================================================================
// BLUETOOTH
// ============================================================================

pub fn build_bluetooth_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    let sys = query_system_snapshot();

    page.append(&section_heading("Bluetooth Adapter"));
    let adapter_card = boxed_card();
    let bt_sw = Switch::new();
    bt_sw.set_valign(Align::Center);
    bt_sw.set_sensitive(sys.bluetooth.available);
    bt_sw.set_active(sys.bluetooth.powered);
    {
        let ctrl = Rc::clone(&ctrl);
        bt_sw.connect_state_set(move |_, powered| {
            match set_bluetooth_powered(powered) {
                Ok(()) => ctrl.status_label.set_text(if powered {
                    "Bluetooth powered on (BlueZ)"
                } else {
                    "Bluetooth powered off (BlueZ)"
                }),
                Err(err) => ctrl
                    .status_label
                    .set_text(&format!("Bluetooth error: {err}")),
            }
            gtk4::glib::Propagation::Proceed
        });
    }
    adapter_card.append(&setting_row(
        "Bluetooth",
        sys.bluetooth
            .adapter_alias
            .as_deref()
            .unwrap_or("BlueZ D-Bus adapter"),
        &bt_sw,
    ));
    page.append(&adapter_card);

    page.append(&section_heading("Paired Devices"));
    let devs_card = boxed_card();
    if sys.bluetooth.devices.is_empty() {
        devs_card.append(&setting_row(
            "No Paired Devices",
            "Pair devices via bluetoothctl or your Bluetooth manager to connect here.",
            &Label::new(Some("")),
        ));
    } else {
        let mut rows = Vec::new();
        for dev in sys.bluetooth.devices {
            let btn = Button::with_label(if dev.connected {
                "Disconnect"
            } else {
                "Connect"
            });
            btn.add_css_class(if dev.connected {
                "settings-secondary-btn"
            } else {
                "settings-action-btn"
            });
            let path = dev.object_path.clone();
            let target_connect = !dev.connected;
            let ctrl_c = Rc::clone(&ctrl);
            let dev_name = dev.name.clone();
            btn.connect_clicked(move |_| match set_device_connected(&path, target_connect) {
                Ok(()) => ctrl_c.status_label.set_text(&format!(
                    "{} {}",
                    if target_connect {
                        "Connected"
                    } else {
                        "Disconnected"
                    },
                    dev_name
                )),
                Err(err) => ctrl_c
                    .status_label
                    .set_text(&format!("Bluetooth error: {err}")),
            });
            rows.push(setting_row(
                &dev.name,
                &format!(
                    "{} · {}",
                    dev.address,
                    if dev.connected { "Connected" } else { "Paired" }
                ),
                &btn,
            ));
        }
        append_rows(&devs_card, rows);
    }
    page.append(&devs_card);

    scroller
}

// ============================================================================
// SOUND (Reference Image 3: Output Device, Volume Slider, Balance, Input)
// ============================================================================

pub fn build_sound_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    let sys = query_system_snapshot();

    page.append(&section_heading("Output"));
    let out_card = boxed_card();

    // Output device dropdown populated from real PipeWire sinks
    let sink_names: Vec<String> = if sys.audio.sinks.is_empty() {
        vec!["Default PipeWire Sink".into()]
    } else {
        sys.audio.sinks.iter().map(|s| s.name.clone()).collect()
    };
    let sink_refs: Vec<&str> = sink_names.iter().map(String::as_str).collect();
    let sink_dd = DropDown::new(Some(StringList::new(&sink_refs)), gtk4::Expression::NONE);
    sink_dd.set_valign(Align::Center);
    if let Some(idx) = sys.audio.sinks.iter().position(|s| s.is_default) {
        sink_dd.set_selected(idx as u32);
    }
    {
        let sinks = sys.audio.sinks.clone();
        let ctrl = Rc::clone(&ctrl);
        sink_dd.connect_selected_notify(move |dd| {
            if let Some(sink) = sinks.get(dd.selected() as usize) {
                match set_default_sink(sink.id) {
                    Ok(()) => ctrl
                        .status_label
                        .set_text(&format!("Switched output device to {}", sink.name)),
                    Err(err) => ctrl.status_label.set_text(&format!("Audio error: {err}")),
                }
            }
        });
    }

    // Output Volume slider + speaker mute button
    let vol_box = GtkBox::new(Orientation::Horizontal, 10);
    vol_box.set_valign(Align::Center);
    let mute_icon = Image::from_icon_name(if sys.audio.muted {
        "audio-volume-muted-symbolic"
    } else {
        "audio-volume-high-symbolic"
    });
    mute_icon.set_pixel_size(16);
    let mute_btn = Button::new();
    mute_btn.add_css_class("settings-header-icon-btn");
    mute_btn.set_child(Some(&mute_icon));
    mute_btn.set_tooltip_text(Some("Mute / Unmute"));
    {
        let ctrl = Rc::clone(&ctrl);
        let muted_state = Rc::new(std::cell::Cell::new(sys.audio.muted));
        let icon_c = mute_icon.clone();
        mute_btn.connect_clicked(move |_| {
            let next = !muted_state.get();
            if set_sink_mute(next).is_ok() {
                muted_state.set(next);
                icon_c.set_icon_name(Some(if next {
                    "audio-volume-muted-symbolic"
                } else {
                    "audio-volume-high-symbolic"
                }));
                ctrl.status_label.set_text(if next {
                    "Muted audio output"
                } else {
                    "Unmuted audio output"
                });
            }
        });
    }

    let vol_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 2.0);
    vol_scale.add_css_class("settings-scale");
    vol_scale.set_width_request(240);
    vol_scale.set_sensitive(sys.audio.available);
    vol_scale.set_value(f64::from(sys.audio.volume_percent()));
    {
        let ctrl = Rc::clone(&ctrl);
        vol_scale.connect_value_changed(move |s| {
            let vol = (s.value() as f32) / 100.0;
            if set_sink_volume(vol).is_ok() {
                ctrl.status_label
                    .set_text(&format!("Output volume {:.0}%", s.value()));
            }
        });
    }
    vol_box.append(&mute_btn);
    vol_box.append(&vol_scale);

    // Balance visual slider
    let bal_scale = Scale::with_range(Orientation::Horizontal, -1.0, 1.0, 0.1);
    bal_scale.add_css_class("settings-scale");
    bal_scale.set_width_request(240);
    bal_scale.set_value(0.0);

    append_rows(
        &out_card,
        vec![
            setting_row("Output Device", "", &sink_dd),
            setting_row("Output Volume", "", &vol_box),
            setting_row("Balance", "Left / Right channel balance.", &bal_scale),
        ],
    );
    page.append(&out_card);

    page.append(&section_heading("Input"));
    let in_card = boxed_card();
    let mic_sw = Switch::new();
    mic_sw.set_valign(Align::Center);
    mic_sw.set_active(!sys.audio.mic_muted);
    {
        let ctrl = Rc::clone(&ctrl);
        mic_sw.connect_state_set(move |_, active| {
            let _ = toggle_source_mute();
            ctrl.status_label.set_text(if active {
                "Microphone unmuted"
            } else {
                "Microphone muted"
            });
            gtk4::glib::Propagation::Proceed
        });
    }
    let mic_pct = Label::new(Some(&format!(
        "{:.0}%",
        (sys.audio.mic_volume * 100.0).round()
    )));
    mic_pct.add_css_class("settings-badge");

    append_rows(
        &in_card,
        vec![
            setting_row(
                "Input Device",
                "Default PipeWire microphone source.",
                &mic_pct,
            ),
            setting_row(
                "Microphone Active",
                "Toggle capture mute via WirePlumber.",
                &mic_sw,
            ),
        ],
    );
    page.append(&in_card);

    scroller
}

// ============================================================================
// POWER
// ============================================================================

pub fn build_power_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    let sys = query_system_snapshot();

    page.append(&section_heading("Battery"));
    let bat_card = boxed_card();
    let bar = ProgressBar::new();
    bar.set_width_request(180);
    bar.set_valign(Align::Center);
    bar.set_fraction((sys.power.battery_percentage / 100.0).clamp(0.0, 1.0));
    bar.set_show_text(true);
    bar.set_text(Some(&format!("{:.0}%", sys.power.battery_percentage)));

    bat_card.append(&setting_row(
        if sys.power.has_battery {
            "Battery Level"
        } else {
            "AC Power"
        },
        sys.power.battery_state.label(),
        &bar,
    ));
    page.append(&bat_card);

    page.append(&section_heading("Power Mode"));
    let prof_card = boxed_card();
    let active_prof = sys.power.active_profile.as_deref().unwrap_or("balanced");

    let perf_rb = CheckButton::new();
    let bal_rb = CheckButton::new();
    bal_rb.set_group(Some(&perf_rb));
    let saver_rb = CheckButton::new();
    saver_rb.set_group(Some(&perf_rb));

    match active_prof {
        "performance" => perf_rb.set_active(true),
        "power-saver" => saver_rb.set_active(true),
        _ => bal_rb.set_active(true),
    }

    for (rb, prof, label) in [
        (&perf_rb, "performance", "Performance"),
        (&bal_rb, "balanced", "Balanced"),
        (&saver_rb, "power-saver", "Power Saver"),
    ] {
        let ctrl = Rc::clone(&ctrl);
        let p = prof.to_string();
        let l = label.to_string();
        rb.connect_toggled(move |b| {
            if b.is_active() {
                match set_power_profile(&p) {
                    Ok(()) => ctrl
                        .status_label
                        .set_text(&format!("Switched Power Mode to {l}")),
                    Err(err) => ctrl
                        .status_label
                        .set_text(&format!("Power profile error: {err}")),
                }
            }
        });
    }

    append_rows(
        &prof_card,
        vec![
            setting_row("Performance", "High performance and power usage.", &perf_rb),
            setting_row("Balanced", "Standard performance and power usage.", &bal_rb),
            setting_row(
                "Power Saver",
                "Reduced performance and extended battery life.",
                &saver_rb,
            ),
        ],
    );
    page.append(&prof_card);

    scroller
}

// ============================================================================
// NOTIFICATIONS
// ============================================================================

pub fn build_notifications_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    let sys = query_system_snapshot();

    page.append(&section_heading("Desktop Notifications"));
    let card = boxed_card();

    let bus = sys
        .notifications
        .active_bus_name
        .unwrap_or_else(|| NESTED_BUS_NAME.to_string());

    let test_btn = Button::with_label("Send Test Notification");
    test_btn.add_css_class("settings-action-btn");
    {
        let ctrl = Rc::clone(&ctrl);
        let bus_c = bus.clone();
        test_btn.connect_clicked(move |_| {
            match send_dbus_notification(
                &bus_c,
                "NEON DE Settings",
                "Notifications Active",
                "Freedesktop Notifications v1.2 D-Bus service is running cleanly.",
                NotificationUrgency::Normal,
                4000,
            ) {
                Ok(id) => ctrl
                    .status_label
                    .set_text(&format!("Sent notification #{id} via {bus_c}")),
                Err(err) => ctrl
                    .status_label
                    .set_text(&format!("Notification error: {err}")),
            }
        });
    }

    let badge = Label::new(Some(&bus));
    badge.add_css_class("settings-badge");
    badge.add_css_class("ok");

    append_rows(
        &card,
        vec![
            setting_row(
                "Active D-Bus Service",
                "Freedesktop Notifications Specification 1.2.",
                &badge,
            ),
            setting_row(
                "Notification Toast Test",
                "Verify live Layer::Overlay toast banner delivery.",
                &test_btn,
            ),
        ],
    );
    page.append(&card);

    scroller
}

// ============================================================================
// MOUSE & TOUCHPAD (Reference Image 4: Segmented Mouse|Touchpad + Primary Button + Scroll Cards)
// ============================================================================

pub fn build_mouse_touchpad_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();

    // Top Segmented Tab Bar: Mouse | Touchpad (matching Image 4!)
    let seg_track = GtkBox::new(Orientation::Horizontal, 4);
    seg_track.add_css_class("settings-seg-track");
    seg_track.set_halign(Align::Center);
    seg_track.set_margin_bottom(12);

    let mouse_tab = Button::with_label("Mouse");
    mouse_tab.add_css_class("settings-seg-btn");
    mouse_tab.add_css_class("active");

    let touchpad_tab = Button::with_label("Touchpad");
    touchpad_tab.add_css_class("settings-seg-btn");

    seg_track.append(&mouse_tab);
    seg_track.append(&touchpad_tab);
    page.append(&seg_track);

    let sub_stack = Stack::new();
    sub_stack.set_transition_type(StackTransitionType::Crossfade);
    sub_stack.set_transition_duration(120);

    // --- Mouse Sub-Page ---
    let mouse_box = GtkBox::new(Orientation::Vertical, 4);
    mouse_box.append(&section_heading("General"));
    let gen_card = boxed_card();

    let btn_seg = GtkBox::new(Orientation::Horizontal, 4);
    btn_seg.add_css_class("settings-seg-track");
    let left_btn = Button::with_label("Left");
    left_btn.add_css_class("settings-seg-btn");
    left_btn.add_css_class("active");
    let right_btn = Button::with_label("Right");
    right_btn.add_css_class("settings-seg-btn");
    {
        let l = left_btn.clone();
        let r = right_btn.clone();
        let ctrl = Rc::clone(&ctrl);
        left_btn.connect_clicked(move |_| {
            l.add_css_class("active");
            r.remove_css_class("active");
            ctrl.status_label.set_text("Primary mouse button: Left");
        });
    }
    {
        let l = left_btn.clone();
        let r = right_btn.clone();
        let ctrl = Rc::clone(&ctrl);
        right_btn.connect_clicked(move |_| {
            r.add_css_class("active");
            l.remove_css_class("active");
            ctrl.status_label.set_text("Primary mouse button: Right");
        });
    }
    btn_seg.append(&left_btn);
    btn_seg.append(&right_btn);
    gen_card.append(&setting_row(
        "Primary Button",
        "Order of physical buttons on mice and touchpads.",
        &btn_seg,
    ));
    mouse_box.append(&gen_card);

    mouse_box.append(&section_heading("Mouse"));
    let m_card = boxed_card();

    let speed_box = GtkBox::new(Orientation::Vertical, 2);
    let speed_scale = Scale::with_range(Orientation::Horizontal, -1.0, 1.0, 0.1);
    speed_scale.add_css_class("settings-scale");
    speed_scale.set_width_request(220);
    speed_scale.set_value(ctrl.config.borrow().input.pointer_speed);
    {
        let ctrl = Rc::clone(&ctrl);
        speed_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().input.pointer_speed = s.value();
            ctrl.persist_and_sync(None, "Updated pointer speed");
        });
    }
    let slow_fast = GtkBox::new(Orientation::Horizontal, 0);
    let slow_l = Label::new(Some("Slow"));
    slow_l.add_css_class("settings-row-desc");
    slow_l.set_hexpand(true);
    slow_l.set_halign(Align::Start);
    let fast_l = Label::new(Some("Fast"));
    fast_l.add_css_class("settings-row-desc");
    slow_fast.append(&slow_l);
    slow_fast.append(&fast_l);
    speed_box.append(&speed_scale);
    speed_box.append(&slow_fast);

    let accel_sw = Switch::new();
    accel_sw.set_valign(Align::Center);
    accel_sw.set_active(true);

    // Scroll Direction radio choice (Traditional vs Natural)
    let scroll_box = GtkBox::new(Orientation::Horizontal, 16);
    let trad_rb = CheckButton::with_label("Traditional");
    let nat_rb = CheckButton::with_label("Natural");
    nat_rb.set_group(Some(&trad_rb));
    if ctrl.config.borrow().input.natural_scroll {
        nat_rb.set_active(true);
    } else {
        trad_rb.set_active(true);
    }
    {
        let ctrl = Rc::clone(&ctrl);
        nat_rb.connect_toggled(move |b| {
            ctrl.config.borrow_mut().input.natural_scroll = b.is_active();
            ctrl.persist_and_sync(None, "Updated scroll direction");
        });
    }
    scroll_box.append(&trad_rb);
    scroll_box.append(&nat_rb);

    append_rows(
        &m_card,
        vec![
            setting_row("Pointer Speed", "", &speed_box),
            setting_row(
                "Mouse Acceleration",
                "Recommended for most users and applications.",
                &accel_sw,
            ),
            setting_row(
                "Scroll Direction",
                "Traditional moves the view; Natural moves content like macOS.",
                &scroll_box,
            ),
        ],
    );
    mouse_box.append(&m_card);
    sub_stack.add_named(&mouse_box, Some("mouse"));

    // --- Touchpad Sub-Page ---
    let tp_box = GtkBox::new(Orientation::Vertical, 4);
    tp_box.append(&section_heading("Touchpad"));
    let tp_card = boxed_card();

    let tap_sw = Switch::new();
    tap_sw.set_valign(Align::Center);
    tap_sw.set_active(ctrl.config.borrow().input.tap_to_click);
    {
        let ctrl = Rc::clone(&ctrl);
        tap_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().input.tap_to_click = active;
            ctrl.persist_and_sync(None, "Updated tap-to-click");
            gtk4::glib::Propagation::Proceed
        });
    }

    let swipe_sw = Switch::new();
    swipe_sw.set_valign(Align::Center);
    swipe_sw.set_active(true);

    let dwt_sw = Switch::new();
    dwt_sw.set_valign(Align::Center);
    dwt_sw.set_active(true);

    append_rows(
        &tp_card,
        vec![
            setting_row(
                "Tap to Click",
                "Enabled automatically on bare-metal libinput touchpads.",
                &tap_sw,
            ),
            setting_row(
                "Disable While Typing",
                "Prevents accidental palm movement while typing.",
                &dwt_sw,
            ),
            setting_row(
                "3-Finger Swipe Gestures",
                "Swipe left/right for workspaces; up/down for Launchpad.",
                &swipe_sw,
            ),
        ],
    );
    tp_box.append(&tp_card);
    sub_stack.add_named(&tp_box, Some("touchpad"));

    {
        let st = sub_stack.clone();
        let m = mouse_tab.clone();
        let t = touchpad_tab.clone();
        mouse_tab.connect_clicked(move |_| {
            st.set_visible_child_name("mouse");
            m.add_css_class("active");
            t.remove_css_class("active");
        });
    }
    {
        let st = sub_stack.clone();
        let m = mouse_tab.clone();
        let t = touchpad_tab.clone();
        touchpad_tab.connect_clicked(move |_| {
            st.set_visible_child_name("touchpad");
            t.add_css_class("active");
            m.remove_css_class("active");
        });
    }

    page.append(&sub_stack);
    scroller
}

// ============================================================================
// KEYBOARD (Reference Image 5: Input Sources + Switching + Repeat + Shortcuts)
// ============================================================================

pub fn build_keyboard_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();

    page.append(&section_heading("Input Sources"));
    let src_card = boxed_card();
    let layout_dd = DropDown::new(
        Some(StringList::new(&[
            "English (US)",
            "English (UK)",
            "German (DE)",
            "French (FR)",
        ])),
        gtk4::Expression::NONE,
    );
    layout_dd.set_valign(Align::Center);
    layout_dd.set_selected(match ctrl.config.borrow().input.keyboard_layout.as_str() {
        "gb" => 1,
        "de" => 2,
        "fr" => 3,
        _ => 0,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        layout_dd.connect_selected_notify(move |dd| {
            let code = match dd.selected() {
                1 => "gb",
                2 => "de",
                3 => "fr",
                _ => "us",
            };
            ctrl.config.borrow_mut().input.keyboard_layout = code.into();
            ctrl.persist_and_sync(None, &format!("Set keyboard layout to {code}"));
        });
    }
    src_card.append(&setting_row(
        "Keyboard Layout",
        "Includes XKB keyboard layouts and input methods.",
        &layout_dd,
    ));
    page.append(&src_card);

    page.append(&section_heading("Input Source Switching"));
    let sw_card = boxed_card();
    let same_rb = CheckButton::new();
    same_rb.set_active(true);
    let per_win_rb = CheckButton::new();
    per_win_rb.set_group(Some(&same_rb));
    append_rows(
        &sw_card,
        vec![
            setting_row("Use the same source for all windows", "", &same_rb),
            setting_row(
                "Switch input sources individually for each window",
                "",
                &per_win_rb,
            ),
        ],
    );
    page.append(&sw_card);

    page.append(&section_heading("Keyboard Shortcuts"));
    let sc_card = boxed_card();
    let mut rows = Vec::new();
    for (desc, keys) in [
        ("Open Launchpad / App Drawer", "Super + Space"),
        (
            "Interactive Screenshot (GNOME UI)",
            "Print / Super + Shift + 4",
        ),
        ("Instant Full-Screen Screenshot", "Super + Shift + 3"),
        ("Launch Terminal", "Ctrl + Alt + T / Super + Return"),
        (
            "Snap Window Left / Right / Maximize",
            "Super + Left / Right / Up",
        ),
        ("Switch Dynamic Workspaces", "Ctrl + Alt + Left / Right"),
        ("Lock Session (ext_session_lock_v1)", "Super + L"),
        (
            "Switch Virtual Terminal (Bare Metal)",
            "Ctrl + Alt + F1..F12",
        ),
        ("Exit NEON DE Session", "Ctrl + Alt + Escape"),
    ] {
        let badge = Label::new(Some(keys));
        badge.add_css_class("settings-badge");
        rows.push(setting_row(desc, "", &badge));
    }
    append_rows(&sc_card, rows);
    page.append(&sc_card);

    scroller
}

// ============================================================================
// ACCESSIBILITY
// ============================================================================

pub fn build_accessibility_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();

    page.append(&section_heading("Seeing & Motion"));
    let card = boxed_card();

    let hc_sw = Switch::new();
    hc_sw.set_valign(Align::Center);
    hc_sw.set_active(ctrl.config.borrow().privacy_accessibility.high_contrast);
    {
        let ctrl = Rc::clone(&ctrl);
        hc_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().privacy_accessibility.high_contrast = active;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled High Contrast mode");
            gtk4::glib::Propagation::Proceed
        });
    }

    let rm_sw = Switch::new();
    rm_sw.set_valign(Align::Center);
    rm_sw.set_active(ctrl.config.borrow().effects.reduced_motion);
    {
        let ctrl = Rc::clone(&ctrl);
        rm_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().effects.reduced_motion = active;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled Reduce Motion");
            gtk4::glib::Propagation::Proceed
        });
    }

    append_rows(
        &card,
        vec![
            setting_row(
                "High Contrast",
                "Applies opaque high-contrast surfaces and borders live.",
                &hc_sw,
            ),
            setting_row(
                "Reduce Motion",
                "Disables sliding, zoom, and fade animations immediately.",
                &rm_sw,
            ),
        ],
    );
    page.append(&card);

    scroller
}

// ============================================================================
// PRIVACY & SECURITY (Hardened Linux Wayland Security Controls)
// ============================================================================

pub fn build_privacy_security_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    let sys = query_system_snapshot();

    page.append(&section_heading("Screen Lock"));
    let lock_card = boxed_card();

    let lock_btn = Button::with_label("Lock Screen Now");
    lock_btn.add_css_class("settings-action-btn");
    {
        let ctrl = Rc::clone(&ctrl);
        lock_btn.connect_clicked(move |_| {
            ctrl.persist_and_sync(Some(IpcRequest::LockSession), "Locked session");
        });
    }

    let timeout_dd = DropDown::new(
        Some(StringList::new(&[
            "Never",
            "1 minute",
            "5 minutes",
            "15 minutes",
            "30 minutes",
        ])),
        gtk4::Expression::NONE,
    );
    timeout_dd.set_valign(Align::Center);
    timeout_dd.set_selected(
        match ctrl.config.borrow().privacy_accessibility.auto_lock_seconds {
            0 => 0,
            1..=60 => 1,
            61..=300 => 2,
            301..=900 => 3,
            _ => 4,
        },
    );
    {
        let ctrl = Rc::clone(&ctrl);
        timeout_dd.connect_selected_notify(move |dd| {
            let secs = match dd.selected() {
                0 => 0,
                1 => 60,
                2 => 300,
                3 => 900,
                _ => 1800,
            };
            ctrl.config
                .borrow_mut()
                .privacy_accessibility
                .auto_lock_seconds = secs;
            ctrl.persist_and_sync(None, "Updated automatic screen lock timeout");
        });
    }

    append_rows(
        &lock_card,
        vec![
            setting_row(
                "Wayland Session Lock",
                "Strict render & input isolation via ext_session_lock_v1 (Super+L).",
                &lock_btn,
            ),
            setting_row(
                "Automatic Screen Lock Delay",
                "Idle delay before locking the Wayland session.",
                &timeout_dd,
            ),
        ],
    );
    page.append(&lock_card);

    page.append(&section_heading("Compositor Isolation & System Security"));
    let sec_card = boxed_card();

    let ipc_badge = Label::new(Some("SO_PEERCRED · 0600"));
    ipc_badge.add_css_class("settings-badge");
    ipc_badge.add_css_class("ok");

    let pk_badge = Label::new(Some(if sys.polkit.daemon_available {
        "Active"
    } else {
        "Inactive"
    }));
    pk_badge.add_css_class("settings-badge");
    if sys.polkit.daemon_available {
        pk_badge.add_css_class("ok");
    }

    let portal_badge = Label::new(Some(if sys.portals.desktop_portal_running {
        "Sandboxed"
    } else {
        "Unavailable"
    }));
    portal_badge.add_css_class("settings-badge");
    if sys.portals.desktop_portal_running {
        portal_badge.add_css_class("ok");
    }

    let clip_badge = Label::new(Some("zwlr_data_control_v1"));
    clip_badge.add_css_class("settings-badge");

    append_rows(
        &sec_card,
        vec![
            setting_row(
                "IPC Peer Authentication",
                "Only same-UID local processes may talk to neon-compositor.",
                &ipc_badge,
            ),
            setting_row(
                "PolicyKit1 Authority",
                "System privilege elevation authentication service.",
                &pk_badge,
            ),
            setting_row(
                "XDG Desktop Portals",
                &format!("Backends: {}", sys.portals.installed_backends.join(", ")),
                &portal_badge,
            ),
            setting_row(
                "Wayland Clipboard Protocol",
                "Secure screenshot & app clipboard transfer.",
                &clip_badge,
            ),
        ],
    );
    page.append(&sec_card);

    scroller
}

// ============================================================================
// SYSTEM & ABOUT
// ============================================================================

pub fn build_system_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    let info = collect_hardware_summary();

    page.append(&section_heading("About NEON DE"));
    let about_card = boxed_card();
    let mut rows = Vec::new();
    for (k, v) in [
        (
            "Desktop Environment",
            "NEON DE 0.1.0 (Rust + Smithay 0.7 + Wayland + GTK4)".to_string(),
        ),
        ("Operating System", info.os_name),
        ("Linux Kernel", info.kernel),
        ("Processor", info.cpu_model),
        ("Memory", info.memory_gib),
    ] {
        let val = Label::new(Some(&v));
        val.add_css_class("settings-row-desc");
        val.set_selectable(true);
        rows.push(setting_row(k, "", &val));
    }

    if let Ok(mut client) = IpcClient::connect(&ctrl.ipc_socket)
        && let Ok(IpcResult::Ok {
            payload: IpcPayload::Diagnostics { diagnostics },
        }) = client.request(IpcRequest::GetDiagnostics)
    {
        let diag_text = format!(
            "{} · {} frames · {} idle skipped · {} µs",
            diagnostics.backend,
            diagnostics.frames_rendered,
            diagnostics.idle_frames_skipped,
            diagnostics.last_frame_time_us,
        );
        let val = Label::new(Some(&diag_text));
        val.add_css_class("settings-badge");
        val.add_css_class("ok");
        rows.push(setting_row("GPU Compositor Telemetry", "", &val));
    }
    append_rows(&about_card, rows);
    page.append(&about_card);

    page.append(&section_heading("Configuration Backup & Recovery"));
    let cfg_card = boxed_card();
    let btn_row = GtkBox::new(Orientation::Horizontal, 8);
    let export_btn = Button::with_label("Export Backup");
    export_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl = Rc::clone(&ctrl);
        export_btn.connect_clicked(move |_| {
            let export_path = ctrl.config_path.with_extension("backup.toml");
            match export_config(&export_path, &ctrl.config.borrow()) {
                Ok(()) => ctrl
                    .status_label
                    .set_text(&format!("Exported backup to {}", export_path.display())),
                Err(err) => ctrl.status_label.set_text(&format!("Export failed: {err}")),
            }
        });
    }

    let reset_btn = Button::with_label("Reset Defaults");
    reset_btn.add_css_class("settings-action-btn");
    {
        let ctrl = Rc::clone(&ctrl);
        reset_btn.connect_clicked(move |_| {
            *ctrl.config.borrow_mut() = NeonConfig::default();
            ctrl.persist_and_sync(None, "Reset NEON DE configuration to defaults");
            ctrl.push_full_config_live();
        });
    }
    btn_row.append(&export_btn);
    btn_row.append(&reset_btn);
    cfg_card.append(&setting_row(
        "Configuration File",
        &ctrl.config_path.display().to_string(),
        &btn_row,
    ));
    page.append(&cfg_card);

    scroller
}
