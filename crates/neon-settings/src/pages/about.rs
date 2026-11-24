//! About NEON DE: async hardware overview, live compositor telemetry, and
//! configuration Export / Import / Reset All Defaults.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, ScrolledWindow, Spinner, gio, prelude::*};
use neon_config::{CURRENT_SCHEMA_VERSION, NeonConfig, load_or_default, save_atomic};
use neon_ipc::{DiagnosticsInfo, OutputInfo};

use crate::{
    state::{SettingsController, fetch_diagnostics_sync, fetch_outputs_sync},
    system_info::{HardwareSummary, collect_hardware_summary},
    ui::{
        append_rows, boxed_card, confirm_action_dialog, info_row, page_shell, run_async,
        section_heading, setting_row, status_badge,
    },
};

struct AboutData {
    hw: HardwareSummary,
    outputs: Vec<OutputInfo>,
    diag: Option<DiagnosticsInfo>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    // Instant identity hero card
    let hero_card = boxed_card();
    let hero = GtkBox::new(Orientation::Horizontal, 14);
    hero.set_margin_top(14);
    hero.set_margin_bottom(14);
    hero.set_margin_start(16);
    hero.set_margin_end(16);
    hero.set_valign(Align::Center);

    let emblem = Label::new(Some("N"));
    emblem.add_css_class("neon-brand-emblem");
    emblem.set_size_request(44, 44);
    emblem.set_halign(Align::Center);
    emblem.set_valign(Align::Center);

    let hero_text = GtkBox::new(Orientation::Vertical, 2);
    hero_text.set_hexpand(true);
    let name_lbl = Label::new(Some(&format!(
        "NEON Desktop Environment {}",
        env!("CARGO_PKG_VERSION")
    )));
    name_lbl.add_css_class("settings-row-title");
    name_lbl.set_halign(Align::Start);

    let sub_lbl = Label::new(Some(&format!(
        "Wayland Compositor (Smithay) • GTK4 Shell • Schema v{CURRENT_SCHEMA_VERSION}"
    )));
    sub_lbl.add_css_class("settings-row-desc");
    sub_lbl.set_halign(Align::Start);
    sub_lbl.set_wrap(true);
    hero_text.append(&name_lbl);
    hero_text.append(&sub_lbl);

    hero.append(&emblem);
    hero.append(&hero_text);
    hero_card.append(&hero);
    page.append(&hero_card);

    let root = GtkBox::new(Orientation::Vertical, 2);
    page.append(&root);

    let load_card = boxed_card();
    let spin = Spinner::new();
    spin.start();
    load_card.append(&setting_row("Loading System Details…", "", &spin));
    root.append(&load_card);

    let socket = ctrl.ipc_socket.clone();
    let ctrl_done = Rc::clone(&ctrl);
    let root_done = root.clone();
    run_async(
        move || AboutData {
            hw: collect_hardware_summary(),
            outputs: fetch_outputs_sync(&socket),
            diag: fetch_diagnostics_sync(&socket),
        },
        move |data| render_about(&root_done, ctrl_done, data),
    );

    scroller
}

fn render_about(root: &GtkBox, ctrl: Rc<SettingsController>, data: AboutData) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }

    // ── Hardware & OS ────────────────────────────────────────────────────────
    root.append(&section_heading("Hardware"));
    let hw_card = boxed_card();

    let gpu_text = if data.hw.gpu_models.is_empty() {
        "DRM/KMS GPU".to_string()
    } else {
        data.hw.gpu_models.join(" + ")
    };
    let disp_summary = if data.outputs.is_empty() {
        "Compositor offline".to_string()
    } else {
        data.outputs
            .iter()
            .map(|o| {
                format!(
                    "{} ({}×{} @ {:.0} Hz)",
                    o.name,
                    o.width,
                    o.height,
                    o.refresh_mhz as f64 / 1000.0
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    append_rows(
        &hw_card,
        vec![
            info_row("OS", "", &data.hw.os_name),
            info_row("Hostname", "", &data.hw.hostname),
            info_row(
                "Kernel",
                "",
                &format!("{} ({})", data.hw.kernel, data.hw.architecture),
            ),
            info_row("Processor", "", &data.hw.cpu_model),
            info_row("Graphics", "", &gpu_text),
            info_row("Memory", "", &data.hw.memory_gib),
            info_row("Displays", "", &disp_summary),
            info_row("Uptime", "", &data.hw.uptime),
        ],
    );
    root.append(&hw_card);

    // ── Live Compositor Telemetry ────────────────────────────────────────────
    root.append(&section_heading("Compositor"));
    let diag_card = boxed_card();

    if let Some(d) = data.diag {
        append_rows(
            &diag_card,
            vec![
                setting_row(
                    "Backend",
                    &format!("Uptime: {}s", d.uptime_secs),
                    &status_badge(&d.backend, "ok"),
                ),
                info_row(
                    "Frames",
                    "",
                    &format!(
                        "{} rendered • last {} µs",
                        d.frames_rendered, d.last_frame_time_us
                    ),
                ),
                info_row(
                    "Session",
                    "",
                    &format!(
                        "{} windows • {} workspaces • {} outputs",
                        d.window_count, d.workspace_count, d.output_count
                    ),
                ),
            ],
        );
    } else {
        diag_card.append(&setting_row(
            "Compositor IPC",
            "",
            &status_badge("Offline", "unavailable"),
        ));
    }
    root.append(&diag_card);

    // ── Configuration Backup & Reset ─────────────────────────────────────────
    root.append(&section_heading("Configuration"));
    let cfg_card = boxed_card();

    let export_btn = Button::with_label("Export…");
    export_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        export_btn.connect_clicked(move |_| {
            let dlg = gtk4::FileDialog::builder()
                .title("Export NEON DE Configuration")
                .initial_name("neon-de-config-backup.toml")
                .modal(true)
                .build();
            let ctrl_inner = Rc::clone(&ctrl_c);
            dlg.save(Some(&ctrl_c.window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    let cfg = ctrl_inner.config.borrow().clone();
                    match save_atomic(&path, &cfg) {
                        Ok(()) => ctrl_inner.set_status_ok(&format!(
                            "Exported to {}",
                            path.display()
                        )),
                        Err(e) => ctrl_inner.set_status_err(&format!("Export failed: {e}")),
                    }
                }
            });
        });
    }

    let import_btn = Button::with_label("Import…");
    import_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        import_btn.connect_clicked(move |_| {
            let dlg = gtk4::FileDialog::builder()
                .title("Import NEON DE Configuration")
                .modal(true)
                .build();
            let ctrl_inner = Rc::clone(&ctrl_c);
            dlg.open(Some(&ctrl_c.window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    let imported = load_or_default(&path);
                    *ctrl_inner.config.borrow_mut() = imported;
                    ctrl_inner.persist_silent();
                    ctrl_inner.push_full_config_live();
                    ctrl_inner.set_status_ok("Imported configuration and applied live");
                }
            });
        });
    }

    let reset_all_btn = Button::with_label("Reset All…");
    reset_all_btn.add_css_class("settings-destructive-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        reset_all_btn.connect_clicked(move |_| {
            let ctrl_inner = Rc::clone(&ctrl_c);
            confirm_action_dialog(
                &ctrl_c.window,
                "Reset All Settings?",
                "This restores every NEON DE desktop preference back to factory defaults.",
                "Reset All",
                true,
                move || {
                    *ctrl_inner.config.borrow_mut() = NeonConfig::default();
                    ctrl_inner.persist_silent();
                    ctrl_inner.push_full_config_live();
                    ctrl_inner.set_status_ok("Restored factory defaults");
                },
            );
        });
    }

    let cfg_actions = GtkBox::new(Orientation::Horizontal, 6);
    cfg_actions.set_valign(Align::Center);
    cfg_actions.append(&export_btn);
    cfg_actions.append(&import_btn);
    cfg_actions.append(&reset_all_btn);

    append_rows(
        &cfg_card,
        vec![
            info_row("Config File", "", &ctrl.config_path.display().to_string()),
            setting_row("Backup & Reset", "", &cfg_actions),
        ],
    );
    root.append(&cfg_card);
}
