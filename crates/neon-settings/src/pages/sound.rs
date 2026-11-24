//! 5.10 Sound settings: PipeWire & WirePlumber output/input devices, master & microphone
//! volume/mute, Speaker Test, per-application stream volumes, and system sound preferences.

use std::rc::Rc;

use gtk4::{
    Align, Box as GtkBox, Button, DropDown, Orientation, Scale, ScrolledWindow, Spinner,
    StringList, Switch, glib, prelude::*,
};
use neon_system::audio::{self, AudioSink, AudioStatus};

use crate::{
    metadata::CapabilityStatus,
    state::SettingsController,
    ui::{
        append_rows, boxed_card, capability_row, info_row, page_shell, run_async, section_heading,
        setting_row, status_badge, toggle_row,
    },
};

struct SoundData {
    status: AudioStatus,
    sources: Vec<AudioSink>,
    streams: Vec<AudioSink>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Sound",
        "Manage PipeWire / WirePlumber output devices, microphones, per-app volume streams, and system alerts.",
        None,
        &ctrl,
    );

    let root = GtkBox::new(Orientation::Vertical, 4);
    page.append(&root);

    let loading_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    loading_card.append(&setting_row(
        "Connecting to PipeWire / WirePlumber…",
        "Querying audio sinks, microphone sources, and active app streams.",
        &spin,
    ));
    root.append(&loading_card);

    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(
        || SoundData {
            status: audio::query_audio_status(),
            sources: audio::query_audio_sources(),
            streams: audio::query_audio_streams(),
        },
        move |data| render_sound(&root_done, ctrl_done, data),
    );

    scroller
}

fn render_sound(root: &GtkBox, ctrl: Rc<SettingsController>, data: SoundData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Output ───────────────────────────────────────────────────────────────
    root.append(&section_heading("Output"));
    let out_card = boxed_card();

    let mut out_rows = Vec::new();
    if !data.status.available {
        out_rows.push(setting_row(
            "PipeWire / WirePlumber Status",
            "wpctl did not report an active default sink.",
            &status_badge("Unavailable", "unavailable"),
        ));
    } else {
        if !data.status.sinks.is_empty() {
            let sink_names: Vec<&str> = data.status.sinks.iter().map(|s| s.name.as_str()).collect();
            let sink_ids: Vec<u32> = data.status.sinks.iter().map(|s| s.id).collect();
            let def_idx = data
                .status
                .sinks
                .iter()
                .position(|s| s.is_default)
                .unwrap_or(0) as u32;
            let dd = DropDown::new(Some(StringList::new(&sink_names)), gtk4::Expression::NONE);
            dd.set_valign(Align::Center);
            dd.set_selected(def_idx);
            let ctrl_c = Rc::clone(&ctrl);
            dd.connect_selected_notify(move |d| {
                if let Some(&id) = sink_ids.get(d.selected() as usize) {
                    let ctrl_done = Rc::clone(&ctrl_c);
                    run_async(
                        move || audio::set_default_sink(id),
                        move |res| match res {
                            Ok(()) => ctrl_done.set_status_ok("Switched default audio output"),
                            Err(e) => ctrl_done.set_status_err(&format!("Switch sink failed: {e}")),
                        },
                    );
                }
            });
            out_rows.push(setting_row(
                "Output Device",
                "Active PipeWire playback sink.",
                &dd,
            ));
        }

        let vol_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 1.0);
        vol_scale.add_css_class("settings-scale");
        vol_scale.set_width_request(210);
        vol_scale.set_draw_value(true);
        vol_scale.set_digits(0);
        vol_scale.set_value(f64::from(data.status.volume_percent()));
        {
            let ctrl_c = Rc::clone(&ctrl);
            vol_scale.connect_value_changed(move |s| {
                let pct = s.value() as f32 / 100.0;
                std::thread::spawn(move || {
                    let _ = audio::set_sink_volume(pct);
                });
                ctrl_c.set_status_info(&format!("Output volume: {:.0}%", s.value()));
            });
        }
        out_rows.push(setting_row(
            "Master Output Volume (%)",
            "Supports up to 150% software amplification via PipeWire.",
            &vol_scale,
        ));

        let mute_sw = Switch::new();
        mute_sw.set_active(data.status.muted);
        mute_sw.set_valign(Align::Center);
        {
            let ctrl_c = Rc::clone(&ctrl);
            mute_sw.connect_state_set(move |_, active| {
                let ctrl_done = Rc::clone(&ctrl_c);
                run_async(
                    move || audio::set_sink_mute(active),
                    move |res| match res {
                        Ok(()) => ctrl_done.set_status_ok(if active {
                            "Muted audio output"
                        } else {
                            "Unmuted audio output"
                        }),
                        Err(e) => ctrl_done.set_status_err(&format!("Mute failed: {e}")),
                    },
                );
                glib::Propagation::Proceed
            });
        }
        out_rows.push(setting_row(
            "Mute Output",
            "Silence the default audio output device.",
            &mute_sw,
        ));

        let test_btn = Button::with_label("Test Speakers");
        test_btn.add_css_class("settings-secondary-btn");
        {
            let ctrl_c = Rc::clone(&ctrl);
            test_btn.connect_clicked(move |_| {
                let ctrl_done = Rc::clone(&ctrl_c);
                run_async(audio::test_speakers, move |res| match res {
                    Ok(()) => ctrl_done.set_status_ok("Played stereo speaker test chime"),
                    Err(e) => ctrl_done.set_status_err(&format!("Speaker test failed: {e}")),
                });
            });
        }
        out_rows.push(setting_row(
            "Speaker Test",
            "Play a freedesktop stereo test tone through the active PipeWire sink.",
            &test_btn,
        ));
    }
    append_rows(&out_card, out_rows);
    root.append(&out_card);

    // ── Input / Microphone ───────────────────────────────────────────────────
    root.append(&section_heading("Input (Microphone)"));
    let in_card = boxed_card();
    let mut in_rows = Vec::new();

    if !data.sources.is_empty() {
        let src_names: Vec<&str> = data.sources.iter().map(|s| s.name.as_str()).collect();
        let src_ids: Vec<u32> = data.sources.iter().map(|s| s.id).collect();
        let def_idx = data
            .sources
            .iter()
            .position(|s| s.is_default)
            .unwrap_or(0) as u32;
        let dd = DropDown::new(Some(StringList::new(&src_names)), gtk4::Expression::NONE);
        dd.set_valign(Align::Center);
        dd.set_selected(def_idx);
        let ctrl_c = Rc::clone(&ctrl);
        dd.connect_selected_notify(move |d| {
            if let Some(&id) = src_ids.get(d.selected() as usize) {
                let ctrl_done = Rc::clone(&ctrl_c);
                run_async(
                    move || audio::set_default_source(id),
                    move |res| match res {
                        Ok(()) => ctrl_done.set_status_ok("Switched default microphone"),
                        Err(e) => ctrl_done.set_status_err(&format!("Switch mic failed: {e}")),
                    },
                );
            }
        });
        in_rows.push(setting_row(
            "Input Device",
            "Active PipeWire microphone / capture source.",
            &dd,
        ));
    }

    let mic_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 1.0);
    mic_scale.add_css_class("settings-scale");
    mic_scale.set_width_request(210);
    mic_scale.set_draw_value(true);
    mic_scale.set_digits(0);
    mic_scale.set_value((data.status.mic_volume * 100.0).round() as f64);
    mic_scale.set_sensitive(data.status.available);
    {
        let ctrl_c = Rc::clone(&ctrl);
        mic_scale.connect_value_changed(move |s| {
            let pct = s.value() as f32 / 100.0;
            std::thread::spawn(move || {
                let _ = audio::set_source_volume(pct);
            });
            ctrl_c.set_status_info(&format!("Microphone gain: {:.0}%", s.value()));
        });
    }
    in_rows.push(setting_row(
        "Input Volume & Sensitivity (%)",
        "Adjust default PipeWire microphone capture level.",
        &mic_scale,
    ));

    let mic_mute = Switch::new();
    mic_mute.set_active(data.status.mic_muted);
    mic_mute.set_sensitive(data.status.available);
    mic_mute.set_valign(Align::Center);
    {
        let ctrl_c = Rc::clone(&ctrl);
        mic_mute.connect_state_set(move |_, active| {
            let ctrl_done = Rc::clone(&ctrl_c);
            run_async(
                move || audio::set_source_mute(active),
                move |res| match res {
                    Ok(()) => ctrl_done.set_status_ok(if active {
                        "Muted microphone"
                    } else {
                        "Unmuted microphone"
                    }),
                    Err(e) => ctrl_done.set_status_err(&format!("Mic mute failed: {e}")),
                },
            );
            glib::Propagation::Proceed
        });
    }
    in_rows.push(setting_row(
        "Mute Microphone",
        "Hardware/PipeWire mute for the default capture source.",
        &mic_mute,
    ));
    append_rows(&in_card, in_rows);
    root.append(&in_card);

    // ── Per-Application Streams ──────────────────────────────────────────────
    root.append(&section_heading("Per-Application Volume"));
    let apps_card = boxed_card();
    let refresh_btn = Button::with_label("Refresh Streams");
    refresh_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        refresh_btn.connect_clicked(move |_| ctrl_c.rebuild_page("sound"));
    }

    if data.streams.is_empty() {
        apps_card.append(&setting_row(
            "No Active Application Audio Streams",
            "Play audio in a browser or media player to adjust individual app volumes here.",
            &refresh_btn,
        ));
    } else {
        let mut stream_rows = vec![setting_row(
            "Active Audio Streams",
            "Live per-application PipeWire playback levels.",
            &refresh_btn,
        )];
        for stream in data.streams {
            let s_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 1.0);
            s_scale.add_css_class("settings-scale");
            s_scale.set_width_request(180);
            s_scale.set_draw_value(true);
            s_scale.set_digits(0);
            s_scale.set_value((stream.volume.unwrap_or(1.0) * 100.0).round() as f64);
            let node_id = stream.id;
            let ctrl_c = Rc::clone(&ctrl);
            let name = stream.name.clone();
            s_scale.connect_value_changed(move |s| {
                let pct = s.value() as f32 / 100.0;
                std::thread::spawn(move || {
                    let _ = audio::set_node_volume(node_id, pct);
                });
                ctrl_c.set_status_info(&format!("{name}: {:.0}%", s.value()));
            });
            stream_rows.push(setting_row(
                &stream.name,
                &format!("PipeWire Node #{}", stream.id),
                &s_scale,
            ));
        }
        append_rows(&apps_card, stream_rows);
    }
    root.append(&apps_card);

    // ── System Sounds & Profiles ─────────────────────────────────────────────
    root.append(&section_heading("System Sounds & Advanced Audio"));
    let sys_card = boxed_card();
    let sounds_on = ctrl.config.borrow().notifications.sounds_enabled;
    append_rows(
        &sys_card,
        vec![
            toggle_row(
                "Notification & System Sounds",
                "Play gentle chimes for desktop alerts and system feedback.",
                sounds_on,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |v| {
                        ctrl_c.config.borrow_mut().notifications.sounds_enabled = v;
                        ctrl_c.persist_and_sync(None, "Updated notification sound preference");
                    }
                },
            ),
            capability_row(
                "Channel Balance & Hardware Card Profiles",
                "Stereo L/R balance and multi-port ALSA/BlueZ profile switching.",
                CapabilityStatus::PartiallyImplemented,
            ),
            info_row(
                "Audio Server",
                "Native PipeWire + WirePlumber control backend.",
                "PipeWire (wpctl)",
            ),
        ],
    );
    root.append(&sys_card);
}
