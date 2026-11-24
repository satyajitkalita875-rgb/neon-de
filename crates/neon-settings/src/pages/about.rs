//! 5.22 About NEON DE: identity header, version & build info, real hardware specs
//! (OS, hostname, kernel, CPU, GPUs, memory, displays, uptime), live compositor diagnostics
//! (`GetDiagnostics`), and Configuration Export / Import / Reset All Defaults.

use std::rc::Rc;

use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, ScrolledWindow, gio, prelude::*};
use neon_config::{CURRENT_SCHEMA_VERSION, NeonConfig, load_or_default, save_atomic};

use crate::{
    state::SettingsController,
    system_info::collect_hardware_summary,
    ui::{
        append_rows, boxed_card, confirm_action_dialog, info_row, page_shell, section_heading,
        setting_row, status_badge,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "About NEON DE",
        "Desktop environment identity, real hardware overview, live Wayland compositor diagnostics, and configuration backup.",
        None,
        &ctrl,
    );

    let hw = collect_hardware_summary();
    let outputs = ctrl.fetch_outputs();
    let diag = ctrl.fetch_diagnostics();

    // ── NEON DE Identity Banner ──────────────────────────────────────────────
    let hero_card = boxed_card();
    let hero = GtkBox::new(Orientation::Horizontal, 16);
    hero.set_margin_top(16);
    hero.set_margin_bottom(16);
    hero.set_margin_start(18);
    hero.set_margin_end(18);
    hero.set_valign(Align::Center);

    let emblem = Label::new(Some("N"));
    emblem.add_css_class("neon-brand-emblem");
    emblem.set_size_request(52, 52);
    emblem.set_halign(Align::Center);
    emblem.set_valign(Align::Center);

    let hero_text = GtkBox::new(Orientation::Vertical, 3);
    hero_text.set_hexpand(true);
    let name_lbl = Label::new(Some(&format!(
        "NEON Desktop Environment {}",
        env!("CARGO_PKG_VERSION")
    )));
    name_lbl.add_css_class("settings-section-title");
    name_lbl.set_halign(Align::Start);

    let sub_lbl = Label::new(Some(&format!(
        "Wayland Compositor (Smithay 0.7) • GTK4 Shell & Settings • Config Schema v{CURRENT_SCHEMA_VERSION} • MIT OR Apache-2.0"
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

    // ── Hardware & Operating System ──────────────────────────────────────────
    page.append(&section_heading("Hardware & Operating System"));
    let hw_card = boxed_card();

    let gpu_text = if hw.gpu_models.is_empty() {
        "Detected by DRM/KMS".to_string()
    } else {
        hw.gpu_models.join(" + ")
    };
    let disp_summary = if outputs.is_empty() {
        "Compositor offline".to_string()
    } else {
        outputs
            .iter()
            .map(|o| format!("{} ({}×{} @ {:.0}Hz)", o.name, o.width, o.height, o.refresh_mhz as f64 / 1000.0))
            .collect::<Vec<_>>()
            .join(", ")
    };

    append_rows(
        &hw_card,
        vec![
            info_row("Operating System", "Host Linux distribution.", &hw.os_name),
            info_row("Device Hostname", "Local machine hostname.", &hw.hostname),
            info_row("Linux Kernel", "Active kernel release and architecture.", &format!("{} ({})", hw.kernel, hw.architecture)),
            info_row("Processor (CPU)", "Detected from /proc/cpuinfo.", &hw.cpu_model),
            info_row("Graphics (GPU)", "Detected PCI display controllers.", &gpu_text),
            info_row("Memory (RAM)", "Used / total system memory from /proc/meminfo.", &hw.memory_gib),
            info_row("Connected Displays", "Active outputs reported by neon-compositor.", &disp_summary),
            info_row("System Uptime", "Time elapsed since boot.", &hw.uptime),
        ],
    );
    page.append(&hw_card);

    // ── Live Compositor Diagnostics ──────────────────────────────────────────
    page.append(&section_heading("Live Compositor Telemetry"));
    let diag_card = boxed_card();

    if let Some(d) = diag {
        append_rows(
            &diag_card,
            vec![
                setting_row(
                    "Compositor Backend",
                    &format!("Uptime: {}s • Safe Mode: {}", d.uptime_secs, if d.safe_mode { "Yes" } else { "No" }),
                    &status_badge(&d.backend, "ok"),
                ),
                info_row(
                    "Frame Timing & Idle Skips",
                    "VBlank-synchronized DRM/EGL rendering telemetry.",
                    &format!(
                        "{} rendered • {} idle skipped • last frame {} µs",
                        d.frames_rendered, d.idle_frames_skipped, d.last_frame_time_us
                    ),
                ),
                info_row(
                    "Active Scene Counts",
                    "Live toplevel windows, workspaces, and outputs.",
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
            &format!("Socket: {}", ctrl.ipc_socket.display()),
            &status_badge("Offline", "unavailable"),
        ));
    }
    page.append(&diag_card);

    // ── Configuration Backup, Import & Full Reset ────────────────────────────
    page.append(&section_heading("Configuration Backup & Recovery"));
    let cfg_card = boxed_card();

    let export_btn = Button::with_label("Export Config…");
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
                            "Exported configuration to {}",
                            path.display()
                        )),
                        Err(e) => ctrl_inner.set_status_err(&format!("Export failed: {e}")),
                    }
                }
            });
        });
    }

    let import_btn = Button::with_label("Import Config…");
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
                    let cfg = ctrl_inner.config.borrow().clone();
                    if let Err(e) = save_atomic(&ctrl_inner.config_path, &cfg) {
                        ctrl_inner.set_status_err(&format!("Import save failed: {e}"));
                        return;
                    }
                    ctrl_inner.push_full_config_live();
                    ctrl_inner.set_status_ok(&format!(
                        "Imported configuration from {} and applied live",
                        path.display()
                    ));
                }
            });
        });
    }

    let reset_all_btn = Button::with_label("Reset All Defaults");
    reset_all_btn.add_css_class("settings-destructive-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        reset_all_btn.connect_clicked(move |_| {
            let ctrl_inner = Rc::clone(&ctrl_c);
            confirm_action_dialog(
                &ctrl_c.window,
                "Reset All NEON DE Settings?",
                "This restores every NEON DE desktop preference (Appearance, Dock, Windows, Effects, Shortcuts, and Input) back to factory defaults.",
                "Reset Everything",
                true,
                move || {
                    *ctrl_inner.config.borrow_mut() = NeonConfig::default();
                    let cfg = ctrl_inner.config.borrow().clone();
                    if let Err(e) = save_atomic(&ctrl_inner.config_path, &cfg) {
                        ctrl_inner.set_status_err(&format!("Reset failed: {e}"));
                        return;
                    }
                    ctrl_inner.push_full_config_live();
                    ctrl_inner.set_status_ok("Restored all NEON DE settings to factory defaults");
                },
            );
        });
    }

    let cfg_actions = GtkBox::new(Orientation::Horizontal, 8);
    cfg_actions.set_valign(Align::Center);
    cfg_actions.append(&export_btn);
    cfg_actions.append(&import_btn);
    cfg_actions.append(&reset_all_btn);

    append_rows(
        &cfg_card,
        vec![
            info_row(
                "Configuration File",
                "Atomic TOML storage with automatic corrupt-file backup.",
                &ctrl.config_path.display().to_string(),
            ),
            setting_row(
                "Backup, Restore & Factory Reset",
                "Export your setup as TOML, import a backup, or restore defaults.",
                &cfg_actions,
            ),
        ],
    );
    page.append(&cfg_card);

    scroller
}
