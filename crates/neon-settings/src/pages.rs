//! macOS-inspired settings detail pages wired live to `neon-config` and `neon-ipc`.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gtk4::{
    Align, Box as GtkBox, Button, DropDown, Label, Orientation, PolicyType, Scale, ScrolledWindow,
    Separator, StringList, Switch, prelude::*,
};
use neon_config::{
    DockPosition, FocusMode, NeonConfig, PanelPosition, ThemeMode, UiDensity, export_config,
    save_atomic,
};
use neon_ipc::{IpcClient, IpcPayload, IpcRequest, IpcResult, OutputInfo};
use neon_system::{
    audio::{set_sink_mute, set_sink_volume},
    bluetooth::set_bluetooth_powered,
    network::set_wifi_enabled,
    notifications::{NESTED_BUS_NAME, NotificationUrgency, send_dbus_notification},
    power::set_power_profile,
    query_system_snapshot,
};
use tracing::warn;

use crate::{style::SettingsTheme, system_info::collect_hardware_summary};

/// Shared controller syncing changes atomically to disk and live to `neon-compositor`.
pub struct SettingsController {
    pub config_path: PathBuf,
    pub ipc_socket: PathBuf,
    pub config: RefCell<NeonConfig>,
    pub theme: SettingsTheme,
    pub status_label: Label,
}

impl SettingsController {
    pub fn persist_and_sync(&self, req: Option<IpcRequest>, note: &str) {
        let cfg = self.config.borrow().clone();
        self.theme
            .update(cfg.appearance.theme, &cfg.appearance.accent_color);

        let saved_ok = match save_atomic(&self.config_path, &cfg) {
            Ok(()) => true,
            Err(err) => {
                warn!(?err, "Failed to persist NEON DE config");
                false
            }
        };

        let mut ipc_ok = false;
        if let Some(request) = req
            && let Ok(mut client) = IpcClient::connect(&self.ipc_socket)
        {
            ipc_ok = matches!(client.request(request), Ok(IpcResult::Ok { .. }));
        }

        let status = match (saved_ok, ipc_ok) {
            (true, true) => format!("{note} — saved & applied live"),
            (true, false) => format!("{note} — saved to {}", self.config_path.display()),
            (false, _) => format!("{note} — warning: could not write config file"),
        };
        self.status_label.set_text(&status);
    }

    pub fn push_full_config_live(&self) {
        let cfg = self.config.borrow().clone();
        if let Ok(mut client) = IpcClient::connect(&self.ipc_socket) {
            let _ = client.request(build_effects_request(&cfg));
            let _ = client.request(build_appearance_request(&cfg));
            let _ = client.request(build_window_behavior_request(&cfg));
        }
    }

    pub fn fetch_outputs(&self) -> Vec<OutputInfo> {
        if let Ok(mut client) = IpcClient::connect(&self.ipc_socket)
            && let Ok(IpcResult::Ok {
                payload: IpcPayload::Outputs { outputs },
            }) = client.request(IpcRequest::GetOutputs)
        {
            return outputs;
        }
        Vec::new()
    }
}

pub fn build_effects_request(cfg: &NeonConfig) -> IpcRequest {
    IpcRequest::UpdateEffects {
        enabled: Some(cfg.effects.enabled),
        reduced_motion: Some(cfg.effects.reduced_motion),
        shadows_enabled: Some(cfg.effects.shadows_enabled),
        borders_enabled: Some(cfg.effects.borders_enabled),
        blur_enabled: Some(cfg.effects.blur_enabled),
        blur_strength: Some(cfg.effects.blur_strength),
        inactive_opacity: Some(cfg.effects.inactive_opacity),
        corner_radius: Some(cfg.effects.corner_radius),
        speed_scale: Some(cfg.effects.speed_scale),
    }
}

pub fn build_appearance_request(cfg: &NeonConfig) -> IpcRequest {
    let theme = match cfg.appearance.theme {
        ThemeMode::NeonDark => "neon_dark",
        ThemeMode::NeonLight => "neon_light",
        ThemeMode::Auto => "auto",
    };
    IpcRequest::UpdateAppearance {
        theme: Some(theme.into()),
        accent_color: Some(cfg.appearance.accent_color.clone()),
        dock_transparency: Some(cfg.dock.transparency),
        panel_transparency: Some(cfg.panel.transparency),
        panel_visible: Some(cfg.panel.visible),
        show_clock: Some(cfg.panel.show_clock),
        show_workspaces: Some(cfg.panel.show_workspaces),
    }
}

pub fn build_window_behavior_request(cfg: &NeonConfig) -> IpcRequest {
    IpcRequest::UpdateWindowBehavior {
        focus_follows_mouse: Some(cfg.windows.focus_mode == FocusMode::FocusFollowsMouse),
        snap_gap: Some(cfg.windows.snap_gap),
    }
}

fn page_shell(title: &str, subtitle: &str) -> (ScrolledWindow, GtkBox) {
    let vbox = GtkBox::new(Orientation::Vertical, 6);
    vbox.add_css_class("settings-content");

    let h = Label::new(Some(title));
    h.add_css_class("settings-page-title");
    h.set_halign(Align::Start);

    let sub = Label::new(Some(subtitle));
    sub.add_css_class("settings-page-subtitle");
    sub.set_halign(Align::Start);

    vbox.append(&h);
    vbox.append(&sub);

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&vbox)
        .build();
    (scroller, vbox)
}

fn card(heading: &str) -> GtkBox {
    let box_ = GtkBox::new(Orientation::Vertical, 10);
    box_.add_css_class("settings-card");
    let lbl = Label::new(Some(heading));
    lbl.add_css_class("settings-card-title");
    lbl.set_halign(Align::Start);
    box_.append(&lbl);
    box_
}

fn labeled_row(title: &str, desc: &str, control: &impl IsA<gtk4::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 16);
    let left = GtkBox::new(Orientation::Vertical, 2);
    left.set_hexpand(true);

    let t = Label::new(Some(title));
    t.add_css_class("settings-row-title");
    t.set_halign(Align::Start);

    let d = Label::new(Some(desc));
    d.add_css_class("settings-row-desc");
    d.set_halign(Align::Start);
    d.set_wrap(true);

    left.append(&t);
    left.append(&d);
    row.append(&left);
    row.append(control);
    row
}

pub fn build_appearance_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Appearance",
        "Customize NEON DE theme, accent color, typography, and visual density.",
    );

    let theme_card = card("THEME & ACCENT");
    let themes = StringList::new(&["Neon Dark", "Neon Light", "Automatic"]);
    let theme_dd = DropDown::new(Some(themes), gtk4::Expression::NONE);
    let initial_idx = match ctrl.config.borrow().appearance.theme {
        ThemeMode::NeonDark => 0,
        ThemeMode::NeonLight => 1,
        ThemeMode::Auto => 2,
    };
    theme_dd.set_selected(initial_idx);
    {
        let ctrl = Rc::clone(&ctrl);
        theme_dd.connect_selected_notify(move |dd| {
            let mode = match dd.selected() {
                1 => ThemeMode::NeonLight,
                2 => ThemeMode::Auto,
                _ => ThemeMode::NeonDark,
            };
            ctrl.config.borrow_mut().appearance.theme = mode;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated theme");
        });
    }
    theme_card.append(&labeled_row(
        "Color Theme",
        "Switches Neon Dark / Neon Light across shell panels, dock, overview, and settings.",
        &theme_dd,
    ));

    // Accent color swatches
    let swatch_box = GtkBox::new(Orientation::Horizontal, 8);
    const SWATCHES: &[(&str, &str)] = &[
        ("Cyan", "#38BDF8"),
        ("Violet", "#A855F7"),
        ("Emerald", "#10B981"),
        ("Amber", "#F59E0B"),
        ("Rose", "#F43F5E"),
    ];
    for &(label, hex) in SWATCHES {
        let btn = Button::with_label(label);
        btn.add_css_class("settings-secondary-btn");
        let ctrl = Rc::clone(&ctrl);
        let hex_owned = hex.to_string();
        btn.connect_clicked(move |_| {
            ctrl.config.borrow_mut().appearance.accent_color = hex_owned.clone();
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), &format!("Accent set to {label}"));
        });
        swatch_box.append(&btn);
    }
    theme_card.append(&Separator::new(Orientation::Horizontal));
    theme_card.append(&labeled_row(
        "Accent Colour",
        "Updates active window focus rings, workspace pills, and controls live.",
        &swatch_box,
    ));
    page.append(&theme_card);

    let typo_card = card("TYPOGRAPHY & DENSITY");
    let densities = StringList::new(&["Compact", "Comfortable", "Spacious"]);
    let density_dd = DropDown::new(Some(densities), gtk4::Expression::NONE);
    density_dd.set_selected(match ctrl.config.borrow().appearance.ui_density {
        UiDensity::Compact => 0,
        UiDensity::Comfortable => 1,
        UiDensity::Spacious => 2,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        density_dd.connect_selected_notify(move |dd| {
            ctrl.config.borrow_mut().appearance.ui_density = match dd.selected() {
                0 => UiDensity::Compact,
                2 => UiDensity::Spacious,
                _ => UiDensity::Comfortable,
            };
            ctrl.persist_and_sync(None, "Updated UI density");
        });
    }
    typo_card.append(&labeled_row(
        "Interface Density",
        "Controls spacing across native NEON DE applications.",
        &density_dd,
    ));

    let font_scale = Scale::with_range(Orientation::Horizontal, 9.0, 18.0, 1.0);
    font_scale.set_width_request(180);
    font_scale.set_draw_value(true);
    font_scale.set_value(f64::from(ctrl.config.borrow().appearance.font_size));
    {
        let ctrl = Rc::clone(&ctrl);
        font_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().appearance.font_size = s.value().round() as u8;
            ctrl.persist_and_sync(None, "Updated font size");
        });
    }
    typo_card.append(&Separator::new(Orientation::Horizontal));
    typo_card.append(&labeled_row(
        "Base Font Size",
        "Preferred UI font point size.",
        &font_scale,
    ));
    page.append(&typo_card);

    scroller
}

pub fn build_displays_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Displays",
        "Configure connected monitors, fractional HiDPI scaling, rotation, and primary output.",
    );

    let outputs = ctrl.fetch_outputs();
    if outputs.is_empty() {
        let empty = card("CONNECTED DISPLAYS");
        empty.append(&Label::new(Some(
            "Connect neon-settings inside a running NEON DE session to configure live outputs.",
        )));
        page.append(&empty);
        return scroller;
    }

    for out in outputs {
        let out_card = card(&format!(
            "{}  ({}x{} @ {:.1} Hz)",
            out.name,
            out.width,
            out.height,
            out.refresh_mhz as f64 / 1000.0
        ));

        let scale_opts = StringList::new(&[
            "1.0x (100%)",
            "1.25x (125%)",
            "1.5x (150%)",
            "1.75x (175%)",
            "2.0x (200%)",
        ]);
        let scale_dd = DropDown::new(Some(scale_opts), gtk4::Expression::NONE);
        let init_scale_idx = if out.scale >= 1.9 {
            4
        } else if out.scale >= 1.65 {
            3
        } else if out.scale >= 1.4 {
            2
        } else if out.scale >= 1.15 {
            1
        } else {
            0
        };
        scale_dd.set_selected(init_scale_idx);

        let rot_opts =
            StringList::new(&["Normal (0°)", "Rotate 90°", "Rotate 180°", "Rotate 270°"]);
        let rot_dd = DropDown::new(Some(rot_opts), gtk4::Expression::NONE);
        rot_dd.set_selected(match out.transform.as_str() {
            "rotate90" => 1,
            "rotate180" => 2,
            "rotate270" => 3,
            _ => 0,
        });

        let apply_btn = Button::with_label("Apply Display Changes");
        apply_btn.add_css_class("settings-action-btn");
        {
            let ctrl = Rc::clone(&ctrl);
            let out_name = out.name.clone();
            let scale_dd = scale_dd.clone();
            let rot_dd = rot_dd.clone();
            apply_btn.connect_clicked(move |_| {
                let scale = match scale_dd.selected() {
                    1 => 1.25,
                    2 => 1.5,
                    3 => 1.75,
                    4 => 2.0,
                    _ => 1.0,
                };
                let transform = match rot_dd.selected() {
                    1 => "rotate90",
                    2 => "rotate180",
                    3 => "rotate270",
                    _ => "normal",
                };
                let req = IpcRequest::ConfigureOutput {
                    name: out_name.clone(),
                    x: None,
                    y: None,
                    scale: Some(scale),
                    primary: Some(true),
                    transform: Some(transform.into()),
                };
                ctrl.persist_and_sync(Some(req), &format!("Configured {out_name} ({scale}x)"));
            });
        }

        out_card.append(&labeled_row(
            "Fractional Scale",
            "Broadcasts wp_fractional_scale_v1 preferred scale live to all clients.",
            &scale_dd,
        ));
        out_card.append(&Separator::new(Orientation::Horizontal));
        out_card.append(&labeled_row(
            "Rotation",
            "Adjust display orientation.",
            &rot_dd,
        ));
        out_card.append(&Separator::new(Orientation::Horizontal));
        out_card.append(&labeled_row(
            "Apply Layout",
            "Updates the active compositor output immediately.",
            &apply_btn,
        ));
        page.append(&out_card);
    }

    scroller
}

pub fn build_windows_desktop_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Desktop & Windows",
        "Manage floating window focus policies, smart edge snapping, and dynamic workspaces.",
    );

    let win_card = card("WINDOW BEHAVIOUR");
    let ffm_switch = Switch::new();
    ffm_switch.set_valign(Align::Center);
    ffm_switch.set_active(ctrl.config.borrow().windows.focus_mode == FocusMode::FocusFollowsMouse);
    {
        let ctrl = Rc::clone(&ctrl);
        ffm_switch.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().windows.focus_mode = if active {
                FocusMode::FocusFollowsMouse
            } else {
                FocusMode::ClickToFocus
            };
            let req = build_window_behavior_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated window focus policy");
            gtk4::glib::Propagation::Proceed
        });
    }
    win_card.append(&labeled_row(
        "Focus Follows Mouse",
        "Hovering over a window transfers keyboard focus automatically without clicking.",
        &ffm_switch,
    ));

    let gap_scale = Scale::with_range(Orientation::Horizontal, 0.0, 24.0, 2.0);
    gap_scale.set_width_request(180);
    gap_scale.set_draw_value(true);
    gap_scale.set_value(f64::from(ctrl.config.borrow().windows.snap_gap));
    {
        let ctrl = Rc::clone(&ctrl);
        gap_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().windows.snap_gap = s.value().round() as i32;
            let req = build_window_behavior_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated snap gap");
        });
    }
    win_card.append(&Separator::new(Orientation::Horizontal));
    win_card.append(&labeled_row(
        "Smart Snap Gap (px)",
        "Spacing around half-screen and quarter-screen snapped windows.",
        &gap_scale,
    ));
    page.append(&win_card);

    let ws_card = card("WORKSPACES & OVERVIEW");
    let dyn_ws_switch = Switch::new();
    dyn_ws_switch.set_valign(Align::Center);
    dyn_ws_switch.set_active(ctrl.config.borrow().windows.dynamic_workspaces);
    {
        let ctrl = Rc::clone(&ctrl);
        dyn_ws_switch.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().windows.dynamic_workspaces = active;
            ctrl.persist_and_sync(None, "Updated dynamic workspaces preference");
            gtk4::glib::Propagation::Proceed
        });
    }
    ws_card.append(&labeled_row(
        "Dynamic Workspaces",
        "Automatically create a trailing workspace and prune unused empty workspaces.",
        &dyn_ws_switch,
    ));

    let overview_btn = Button::with_label("Open Activities Overview");
    overview_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl = Rc::clone(&ctrl);
        overview_btn.connect_clicked(move |_| {
            ctrl.persist_and_sync(
                Some(IpcRequest::ToggleOverview),
                "Toggled Activities Overview",
            );
        });
    }
    ws_card.append(&Separator::new(Orientation::Horizontal));
    ws_card.append(&labeled_row(
        "Activities Overview",
        "Preview workspaces, search open windows, and launch XDG applications (Super+Space).",
        &overview_btn,
    ));
    page.append(&ws_card);

    scroller
}

pub fn build_dock_panels_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Dock & Panels",
        "Customize the NEON DE Top Panel and Floating Dock appearance and visibility.",
    );

    let panel_card = card("TOP PANEL");
    let panel_vis = Switch::new();
    panel_vis.set_valign(Align::Center);
    panel_vis.set_active(ctrl.config.borrow().panel.visible);
    {
        let ctrl = Rc::clone(&ctrl);
        panel_vis.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().panel.visible = active;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated top panel visibility");
            gtk4::glib::Propagation::Proceed
        });
    }
    panel_card.append(&labeled_row(
        "Show Top Panel",
        "Toggles the top shell bar live via IPC.",
        &panel_vis,
    ));

    let clock_vis = Switch::new();
    clock_vis.set_valign(Align::Center);
    clock_vis.set_active(ctrl.config.borrow().panel.show_clock);
    {
        let ctrl = Rc::clone(&ctrl);
        clock_vis.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().panel.show_clock = active;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated panel clock visibility");
            gtk4::glib::Propagation::Proceed
        });
    }
    panel_card.append(&Separator::new(Orientation::Horizontal));
    panel_card.append(&labeled_row(
        "Show Clock",
        "Display the centered clock & date in the top panel.",
        &clock_vis,
    ));

    let ws_vis = Switch::new();
    ws_vis.set_valign(Align::Center);
    ws_vis.set_active(ctrl.config.borrow().panel.show_workspaces);
    {
        let ctrl = Rc::clone(&ctrl);
        ws_vis.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().panel.show_workspaces = active;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated workspace indicator pills");
            gtk4::glib::Propagation::Proceed
        });
    }
    panel_card.append(&Separator::new(Orientation::Horizontal));
    panel_card.append(&labeled_row(
        "Show Workspace Indicators",
        "Display clickable dynamic workspace pills next to Activities.",
        &ws_vis,
    ));

    let panel_alpha = Scale::with_range(Orientation::Horizontal, 0.35, 1.0, 0.05);
    panel_alpha.set_width_request(180);
    panel_alpha.set_draw_value(true);
    panel_alpha.set_value(f64::from(ctrl.config.borrow().panel.transparency));
    {
        let ctrl = Rc::clone(&ctrl);
        panel_alpha.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().panel.transparency = s.value() as f32;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated top panel opacity");
        });
    }
    panel_card.append(&Separator::new(Orientation::Horizontal));
    panel_card.append(&labeled_row(
        "Top Panel Opacity",
        "Adjusts top panel surface translucency live.",
        &panel_alpha,
    ));
    page.append(&panel_card);

    let dock_card = card("FLOATING DOCK");
    let pos_opts = StringList::new(&["Bottom", "Left", "Right"]);
    let pos_dd = DropDown::new(Some(pos_opts), gtk4::Expression::NONE);
    pos_dd.set_selected(match ctrl.config.borrow().dock.position {
        DockPosition::Bottom => 0,
        DockPosition::Left => 1,
        DockPosition::Right => 2,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        pos_dd.connect_selected_notify(move |dd| {
            ctrl.config.borrow_mut().dock.position = match dd.selected() {
                1 => DockPosition::Left,
                2 => DockPosition::Right,
                _ => DockPosition::Bottom,
            };
            ctrl.persist_and_sync(None, "Saved dock edge preference");
        });
    }
    dock_card.append(&labeled_row(
        "Dock Position",
        "Preferred screen edge for the NEON DE dock.",
        &pos_dd,
    ));

    let dock_alpha = Scale::with_range(Orientation::Horizontal, 0.35, 1.0, 0.05);
    dock_alpha.set_width_request(180);
    dock_alpha.set_draw_value(true);
    dock_alpha.set_value(f64::from(ctrl.config.borrow().dock.transparency));
    {
        let ctrl = Rc::clone(&ctrl);
        dock_alpha.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().dock.transparency = s.value() as f32;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated dock opacity");
        });
    }
    dock_card.append(&Separator::new(Orientation::Horizontal));
    dock_card.append(&labeled_row(
        "Dock Opacity",
        "Adjusts floating dock glass translucency live.",
        &dock_alpha,
    ));

    let panel_pos_opts = StringList::new(&["Top", "Bottom"]);
    let panel_pos_dd = DropDown::new(Some(panel_pos_opts), gtk4::Expression::NONE);
    panel_pos_dd.set_selected(match ctrl.config.borrow().panel.position {
        PanelPosition::Top => 0,
        PanelPosition::Bottom => 1,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        panel_pos_dd.connect_selected_notify(move |dd| {
            ctrl.config.borrow_mut().panel.position = if dd.selected() == 1 {
                PanelPosition::Bottom
            } else {
                PanelPosition::Top
            };
            ctrl.persist_and_sync(None, "Saved panel position preference");
        });
    }
    dock_card.append(&Separator::new(Orientation::Horizontal));
    dock_card.append(&labeled_row(
        "Panel Edge",
        "Preferred anchor edge for the shell panel.",
        &panel_pos_dd,
    ));
    page.append(&dock_card);

    scroller
}

pub fn build_effects_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Visual Effects",
        "Live GPU compositor controls for shadows, frosted-glass blur, rounded borders, and animations.",
    );

    let gpu_card = card("GPU COMPOSITION EFFECTS");

    let shadows_sw = Switch::new();
    shadows_sw.set_valign(Align::Center);
    shadows_sw.set_active(ctrl.config.borrow().effects.shadows_enabled);
    {
        let ctrl = Rc::clone(&ctrl);
        shadows_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().effects.shadows_enabled = active;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled window shadows");
            gtk4::glib::Propagation::Proceed
        });
    }
    gpu_card.append(&labeled_row(
        "Window Drop Shadows",
        "Analytical SDF soft drop shadows rendered on the GPU.",
        &shadows_sw,
    ));

    let borders_sw = Switch::new();
    borders_sw.set_valign(Align::Center);
    borders_sw.set_active(ctrl.config.borrow().effects.borders_enabled);
    {
        let ctrl = Rc::clone(&ctrl);
        borders_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().effects.borders_enabled = active;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled window focus rings");
            gtk4::glib::Propagation::Proceed
        });
    }
    gpu_card.append(&Separator::new(Orientation::Horizontal));
    gpu_card.append(&labeled_row(
        "Accent Focus Rings",
        "Subtle rim borders that glow with your accent color on the focused window.",
        &borders_sw,
    ));

    let blur_sw = Switch::new();
    blur_sw.set_valign(Align::Center);
    blur_sw.set_active(ctrl.config.borrow().effects.blur_enabled);
    {
        let ctrl = Rc::clone(&ctrl);
        blur_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().effects.blur_enabled = active;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled frosted-glass blur");
            gtk4::glib::Propagation::Proceed
        });
    }
    gpu_card.append(&Separator::new(Orientation::Horizontal));
    gpu_card.append(&labeled_row(
        "Frosted-Glass Blur",
        "9-tap GPU diffusion backdrop behind translucent windows.",
        &blur_sw,
    ));

    let radius_scale = Scale::with_range(Orientation::Horizontal, 0.0, 24.0, 1.0);
    radius_scale.set_width_request(180);
    radius_scale.set_draw_value(true);
    radius_scale.set_value(f64::from(ctrl.config.borrow().effects.corner_radius));
    {
        let ctrl = Rc::clone(&ctrl);
        radius_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().effects.corner_radius = s.value() as f32;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated corner radius");
        });
    }
    gpu_card.append(&Separator::new(Orientation::Horizontal));
    gpu_card.append(&labeled_row(
        "Corner Radius (px)",
        "Adjusts shadow, border, and frosted-glass corner curvature live.",
        &radius_scale,
    ));

    let opacity_scale = Scale::with_range(Orientation::Horizontal, 0.70, 1.0, 0.02);
    opacity_scale.set_width_request(180);
    opacity_scale.set_draw_value(true);
    opacity_scale.set_value(f64::from(ctrl.config.borrow().effects.inactive_opacity));
    {
        let ctrl = Rc::clone(&ctrl);
        opacity_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().effects.inactive_opacity = s.value() as f32;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated inactive window opacity");
        });
    }
    gpu_card.append(&Separator::new(Orientation::Horizontal));
    gpu_card.append(&labeled_row(
        "Inactive Window Opacity",
        "Subtle translucency on unfocused windows to emphasize active work.",
        &opacity_scale,
    ));
    page.append(&gpu_card);

    let anim_card = card("MOTION & ANIMATIONS");
    let reduced_sw = Switch::new();
    reduced_sw.set_valign(Align::Center);
    reduced_sw.set_active(ctrl.config.borrow().effects.reduced_motion);
    {
        let ctrl = Rc::clone(&ctrl);
        reduced_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().effects.reduced_motion = active;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated reduced motion mode");
            gtk4::glib::Propagation::Proceed
        });
    }
    anim_card.append(&labeled_row(
        "Reduce Motion",
        "Disables sliding and fade animations immediately for accessibility or low-power mode.",
        &reduced_sw,
    ));

    let speed_scale = Scale::with_range(Orientation::Horizontal, 0.25, 2.5, 0.25);
    speed_scale.set_width_request(180);
    speed_scale.set_draw_value(true);
    speed_scale.set_value(ctrl.config.borrow().effects.speed_scale);
    {
        let ctrl = Rc::clone(&ctrl);
        speed_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().effects.speed_scale = s.value();
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated animation speed");
        });
    }
    anim_card.append(&Separator::new(Orientation::Horizontal));
    anim_card.append(&labeled_row(
        "Animation Speed Multiplier",
        "Scales window open, focus, and workspace transition speed.",
        &speed_scale,
    ));
    page.append(&anim_card);

    scroller
}

pub fn build_input_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Input & Shortcuts",
        "Configure keyboard repeat timing, touchpad preferences, and inspect NEON DE shortcuts.",
    );

    let kbd_card = card("KEYBOARD & TOUCHPAD");
    let nat_scroll = Switch::new();
    nat_scroll.set_valign(Align::Center);
    nat_scroll.set_active(ctrl.config.borrow().input.natural_scroll);
    {
        let ctrl = Rc::clone(&ctrl);
        nat_scroll.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().input.natural_scroll = active;
            ctrl.persist_and_sync(None, "Saved natural scrolling preference");
            gtk4::glib::Propagation::Proceed
        });
    }
    kbd_card.append(&labeled_row(
        "Natural Scrolling",
        "Content tracks finger movement direction on touchpads.",
        &nat_scroll,
    ));

    let tap_click = Switch::new();
    tap_click.set_valign(Align::Center);
    tap_click.set_active(ctrl.config.borrow().input.tap_to_click);
    {
        let ctrl = Rc::clone(&ctrl);
        tap_click.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().input.tap_to_click = active;
            ctrl.persist_and_sync(None, "Saved tap-to-click preference");
            gtk4::glib::Propagation::Proceed
        });
    }
    kbd_card.append(&Separator::new(Orientation::Horizontal));
    kbd_card.append(&labeled_row(
        "Tap to Click",
        "Single-finger tap triggers primary click on libinput touchpads.",
        &tap_click,
    ));
    page.append(&kbd_card);

    let sc_card = card("ACTIVE COMPOSITOR SHORTCUTS");
    for (keys, desc) in [
        (
            "Super + Space / Super + A",
            "Toggle Activities Overview & Application Grid",
        ),
        ("Super + Return", "Launch Terminal"),
        (
            "Super + Left / Right",
            "Smart Snap Window Left / Right Half",
        ),
        ("Super + Up / Down", "Toggle Maximize / Restore or Minimize"),
        (
            "Super + H / Super + Shift + H",
            "Minimize Focused / Restore Last Minimized",
        ),
        (
            "Ctrl + Alt + Left / Right",
            "Switch Dynamic Workspaces (or 3-finger swipe)",
        ),
        ("Super + L", "Lock Session (ext_session_lock_v1)"),
        (
            "Alt + Left / Right Drag",
            "Interactive Move / Resize Window",
        ),
    ] {
        let lbl = Label::new(Some(keys));
        lbl.add_css_class("settings-row-title");
        sc_card.append(&labeled_row(desc, "Active in neon-compositor", &lbl));
    }
    page.append(&sc_card);

    scroller
}

pub fn build_privacy_accessibility_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Privacy, Security & Accessibility",
        "Session lock controls, accessibility options, and verified compositor security boundaries.",
    );

    let sec_card = card("SESSION LOCK & SECURITY");
    let lock_btn = Button::with_label("Lock Screen Now");
    lock_btn.add_css_class("settings-action-btn");
    {
        let ctrl = Rc::clone(&ctrl);
        lock_btn.connect_clicked(move |_| {
            ctrl.persist_and_sync(
                Some(IpcRequest::LockSession),
                "Requested ext_session_lock_v1 lock",
            );
        });
    }
    sec_card.append(&labeled_row(
        "Screen Lock (ext_session_lock_v1)",
        "Immediately isolates rendering & input behind neon-lockscreen (Super+L).",
        &lock_btn,
    ));

    let ipc_badge = Label::new(Some("Active (SO_PEERCRED + 0600)"));
    ipc_badge.add_css_class("settings-row-title");
    sec_card.append(&Separator::new(Orientation::Horizontal));
    sec_card.append(&labeled_row(
        "IPC Peer Authentication",
        "Only same-UID processes can communicate with neon-compositor.",
        &ipc_badge,
    ));
    page.append(&sec_card);

    let a11y_card = card("ACCESSIBILITY");
    let hc_sw = Switch::new();
    hc_sw.set_valign(Align::Center);
    hc_sw.set_active(ctrl.config.borrow().privacy_accessibility.high_contrast);
    {
        let ctrl = Rc::clone(&ctrl);
        hc_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().privacy_accessibility.high_contrast = active;
            ctrl.persist_and_sync(None, "Saved high contrast preference");
            gtk4::glib::Propagation::Proceed
        });
    }
    a11y_card.append(&labeled_row(
        "High Contrast",
        "Persists high-contrast preference in neon-config.",
        &hc_sw,
    ));
    page.append(&a11y_card);

    scroller
}

pub fn build_system_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "System & About NEON DE",
        "Live host hardware details, Linux system service status, and configuration backup/reset.",
    );

    let info = collect_hardware_summary();
    let about_card = card("ABOUT NEON DE");
    for (k, v) in [
        (
            "Desktop Environment",
            "NEON DE 0.1.0 (Rust + Smithay 0.7 + Wayland + GTK4)".to_string(),
        ),
        ("Operating System", info.os_name),
        ("Linux Kernel", info.kernel),
        ("Processor", info.cpu_model),
        ("System Memory", info.memory_gib),
    ] {
        let val = Label::new(Some(&v));
        val.add_css_class("settings-row-desc");
        val.set_selectable(true);
        about_card.append(&labeled_row(k, "", &val));
    }
    page.append(&about_card);

    let sys = query_system_snapshot();

    // Live Audio (PipeWire / WirePlumber)
    let audio_card = card("AUDIO (PIPEWIRE / WIREPLUMBER)");
    let vol_scale = Scale::with_range(Orientation::Horizontal, 0.0, 150.0, 2.0);
    vol_scale.set_width_request(180);
    vol_scale.set_draw_value(true);
    vol_scale.set_sensitive(sys.audio.available);
    vol_scale.set_value(f64::from(sys.audio.volume_percent()));
    {
        let ctrl = Rc::clone(&ctrl);
        vol_scale.connect_value_changed(move |s| {
            let vol = (s.value() as f32) / 100.0;
            match set_sink_volume(vol) {
                Ok(()) => ctrl
                    .status_label
                    .set_text(&format!("Set PipeWire volume to {:.0}%", s.value())),
                Err(err) => ctrl.status_label.set_text(&format!("Audio error: {err}")),
            }
        });
    }
    let sink_desc = sys
        .audio
        .default_sink_name
        .as_deref()
        .unwrap_or("PipeWire default output sink");
    audio_card.append(&labeled_row("Output Volume", sink_desc, &vol_scale));

    let mute_sw = Switch::new();
    mute_sw.set_valign(Align::Center);
    mute_sw.set_sensitive(sys.audio.available);
    mute_sw.set_active(sys.audio.muted);
    {
        let ctrl = Rc::clone(&ctrl);
        mute_sw.connect_state_set(move |_, muted| {
            match set_sink_mute(muted) {
                Ok(()) => ctrl.status_label.set_text(if muted {
                    "Muted PipeWire audio output"
                } else {
                    "Unmuted PipeWire audio output"
                }),
                Err(err) => ctrl.status_label.set_text(&format!("Mute error: {err}")),
            }
            gtk4::glib::Propagation::Proceed
        });
    }
    audio_card.append(&Separator::new(Orientation::Horizontal));
    audio_card.append(&labeled_row(
        "Mute Audio Output",
        "Controls @DEFAULT_AUDIO_SINK@ via WirePlumber.",
        &mute_sw,
    ));
    page.append(&audio_card);

    // Live Connectivity & Power (NetworkManager, BlueZ, UPower)
    let dev_card = card("NETWORK, BLUETOOTH & POWER (D-BUS)");

    let wifi_sw = Switch::new();
    wifi_sw.set_valign(Align::Center);
    wifi_sw.set_sensitive(sys.network.available);
    wifi_sw.set_active(sys.network.wifi_enabled);
    {
        let ctrl = Rc::clone(&ctrl);
        wifi_sw.connect_state_set(move |_, enabled| {
            match set_wifi_enabled(enabled) {
                Ok(()) => ctrl.status_label.set_text(if enabled {
                    "Enabled Wi-Fi via NetworkManager"
                } else {
                    "Disabled Wi-Fi via NetworkManager"
                }),
                Err(err) => ctrl.status_label.set_text(&format!("Network error: {err}")),
            }
            gtk4::glib::Propagation::Proceed
        });
    }
    dev_card.append(&labeled_row(
        "Wi-Fi (NetworkManager)",
        &format!("Status: {}", sys.network.panel_label()),
        &wifi_sw,
    ));

    let bt_sw = Switch::new();
    bt_sw.set_valign(Align::Center);
    bt_sw.set_sensitive(sys.bluetooth.available);
    bt_sw.set_active(sys.bluetooth.powered);
    {
        let ctrl = Rc::clone(&ctrl);
        bt_sw.connect_state_set(move |_, powered| {
            match set_bluetooth_powered(powered) {
                Ok(()) => ctrl.status_label.set_text(if powered {
                    "Powered on BlueZ Bluetooth adapter"
                } else {
                    "Powered off BlueZ Bluetooth adapter"
                }),
                Err(err) => ctrl
                    .status_label
                    .set_text(&format!("Bluetooth error: {err}")),
            }
            gtk4::glib::Propagation::Proceed
        });
    }
    dev_card.append(&Separator::new(Orientation::Horizontal));
    dev_card.append(&labeled_row(
        "Bluetooth (BlueZ)",
        &format!(
            "{} paired · {} connected",
            sys.bluetooth.devices.len(),
            sys.bluetooth.connected_count()
        ),
        &bt_sw,
    ));

    let profile_model = StringList::new(&["power-saver", "balanced", "performance"]);
    let profile_dd = DropDown::new(Some(profile_model), gtk4::Expression::NONE);
    profile_dd.set_valign(Align::Center);
    profile_dd.set_sensitive(sys.power.active_profile.is_some());
    profile_dd.set_selected(match sys.power.active_profile.as_deref() {
        Some("power-saver") => 0,
        Some("performance") => 2,
        _ => 1,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        profile_dd.connect_selected_notify(move |dd| {
            let prof = match dd.selected() {
                0 => "power-saver",
                2 => "performance",
                _ => "balanced",
            };
            match set_power_profile(prof) {
                Ok(()) => ctrl
                    .status_label
                    .set_text(&format!("Switched power profile to {prof}")),
                Err(err) => ctrl
                    .status_label
                    .set_text(&format!("Power profile error: {err}")),
            }
        });
    }
    let power_desc = if sys.power.has_battery {
        format!(
            "Battery: {:.0}% ({})",
            sys.power.battery_percentage,
            sys.power.battery_state.label()
        )
    } else {
        "AC Power (UPower)".to_string()
    };
    dev_card.append(&Separator::new(Orientation::Horizontal));
    dev_card.append(&labeled_row(
        "Power Profile (UPower)",
        &power_desc,
        &profile_dd,
    ));
    page.append(&dev_card);

    // Desktop Services (Notifications, PolicyKit1, XDG Desktop Portals)
    let svc_card = card("DESKTOP SERVICES (D-BUS, POLKIT & PORTALS)");
    let notif_bus = sys
        .notifications
        .active_bus_name
        .clone()
        .unwrap_or_else(|| NESTED_BUS_NAME.to_string());
    let test_notif_btn = Button::with_label("Test Toast");
    test_notif_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl = Rc::clone(&ctrl);
        let bus = notif_bus.clone();
        test_notif_btn.connect_clicked(move |_| {
            match send_dbus_notification(
                &bus,
                "NEON DE Settings",
                "Notifications Active",
                "Freedesktop Notifications v1.2 D-Bus integration verified.",
                NotificationUrgency::Normal,
                4000,
            ) {
                Ok(id) => ctrl
                    .status_label
                    .set_text(&format!("Sent D-Bus notification #{id} via {bus}")),
                Err(err) => ctrl
                    .status_label
                    .set_text(&format!("Notification error: {err}")),
            }
        });
    }
    svc_card.append(&labeled_row(
        "Desktop Notifications (v1.2)",
        &format!("Active bus: {notif_bus}"),
        &test_notif_btn,
    ));

    let polkit_badge = Label::new(Some(if sys.polkit.daemon_available {
        "Available"
    } else {
        "Unavailable"
    }));
    polkit_badge.add_css_class("settings-row-title");
    svc_card.append(&Separator::new(Orientation::Horizontal));
    svc_card.append(&labeled_row(
        "PolicyKit1 Authority",
        &format!("Agent path: {}", sys.polkit.agent_object_path),
        &polkit_badge,
    ));

    let portal_badge = Label::new(Some(if sys.portals.desktop_portal_running {
        "Active"
    } else {
        "Inactive"
    }));
    portal_badge.add_css_class("settings-row-title");
    let backends = if sys.portals.installed_backends.is_empty() {
        "none detected".to_string()
    } else {
        sys.portals.installed_backends.join(", ")
    };
    svc_card.append(&Separator::new(Orientation::Horizontal));
    svc_card.append(&labeled_row(
        "XDG Desktop Portals",
        &format!(
            "Desktop: {} · Backends: {backends}",
            sys.portals.current_desktop
        ),
        &portal_badge,
    ));
    page.append(&svc_card);

    let cfg_card = card("CONFIGURATION MANAGEMENT");
    let btn_row = GtkBox::new(Orientation::Horizontal, 10);

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

    let reset_btn = Button::with_label("Reset to Defaults");
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
    cfg_card.append(&labeled_row(
        "Configuration File",
        &ctrl.config_path.display().to_string(),
        &btn_row,
    ));
    page.append(&cfg_card);

    scroller
}
