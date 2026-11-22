//! Core settings controller, macOS × GNOME 47 boxed-list UI builders, and
//! Customization pages (Appearance, Displays, Dock & Panels, Windows & Effects).

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gtk4::{
    Align, Box as GtkBox, Button, ContentFit, DropDown, FlowBox, Label, Orientation, Picture,
    PolicyType, Scale, ScrolledWindow, SelectionMode, Separator, StringList, Switch, gio,
    prelude::*,
};
use neon_config::{
    DockPosition, FocusMode, NeonConfig, PanelPosition, ThemeMode, UiDensity, save_atomic,
};
use neon_ipc::{IpcClient, IpcPayload, IpcRequest, IpcResult, OutputInfo};
use tracing::warn;

use crate::style::SettingsTheme;

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
                self.status_label.set_text(&format!("Save failed: {err}"));
                false
            }
        };

        if let Some(request) = req {
            match self.send_ipc(request) {
                Ok(_) => {
                    if saved_ok {
                        self.status_label
                            .set_text(&format!("{note} — applied live & saved"));
                    }
                }
                Err(err) => {
                    if saved_ok {
                        self.status_label
                            .set_text(&format!("{note} — saved (compositor offline: {err})"));
                    }
                }
            }
        } else if saved_ok {
            self.status_label
                .set_text(&format!("{note} — saved to {}", self.config_path.display()));
        }
    }

    pub fn send_ipc(&self, req: IpcRequest) -> Result<IpcResult, String> {
        let mut client = IpcClient::connect(&self.ipc_socket).map_err(|e| e.to_string())?;
        client.request(req).map_err(|e| e.to_string())
    }

    pub fn fetch_outputs(&self) -> Vec<OutputInfo> {
        match self.send_ipc(IpcRequest::GetOutputs) {
            Ok(IpcResult::Ok {
                payload: IpcPayload::Outputs { outputs },
            }) => outputs,
            _ => Vec::new(),
        }
    }

    pub fn push_full_config_live(&self) {
        let cfg = self.config.borrow().clone();
        let _ = self.send_ipc(build_effects_request(&cfg));
        let _ = self.send_ipc(build_appearance_request(&cfg));
        let _ = self.send_ipc(build_window_behavior_request(&cfg));
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
        high_contrast: Some(cfg.privacy_accessibility.high_contrast),
        wallpaper: Some(cfg.appearance.wallpaper_path.clone().unwrap_or_default()),
    }
}

pub fn build_window_behavior_request(cfg: &NeonConfig) -> IpcRequest {
    IpcRequest::UpdateWindowBehavior {
        focus_follows_mouse: Some(cfg.windows.focus_mode == FocusMode::FocusFollowsMouse),
        snap_gap: Some(cfg.windows.snap_gap),
    }
}

/// Centered clamped scrollable page container matching GNOME 47 / macOS System Settings.
pub fn page_clamp() -> (ScrolledWindow, GtkBox) {
    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.set_halign(Align::Center);
    outer.set_hexpand(true);

    let column = GtkBox::new(Orientation::Vertical, 4);
    column.add_css_class("settings-content-clamp");
    column.set_width_request(600);
    outer.append(&column);

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&outer)
        .build();
    (scroller, column)
}

pub fn section_heading(title: &str) -> Label {
    let lbl = Label::new(Some(title));
    lbl.add_css_class("settings-section-title");
    lbl.set_halign(Align::Start);
    lbl
}

pub fn boxed_card() -> GtkBox {
    let box_card = GtkBox::new(Orientation::Vertical, 0);
    box_card.add_css_class("settings-card");
    box_card
}

pub fn row_separator() -> Separator {
    let sep = Separator::new(Orientation::Horizontal);
    sep.add_css_class("settings-row-sep");
    sep
}

pub fn setting_row(title: &str, subtitle: &str, control: &impl IsA<gtk4::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 16);
    row.add_css_class("settings-row");
    row.set_valign(Align::Center);

    let text = GtkBox::new(Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(Align::Center);

    let t = Label::new(Some(title));
    t.add_css_class("settings-row-title");
    t.set_halign(Align::Start);
    text.append(&t);

    if !subtitle.is_empty() {
        let s = Label::new(Some(subtitle));
        s.add_css_class("settings-row-desc");
        s.set_halign(Align::Start);
        s.set_wrap(true);
        text.append(&s);
    }

    row.append(&text);
    row.append(control);
    row
}

pub fn append_rows(card: &GtkBox, rows: Vec<GtkBox>) {
    for (idx, row) in rows.into_iter().enumerate() {
        if idx > 0 {
            card.append(&row_separator());
        }
        card.append(&row);
    }
}

// ============================================================================
// 1. APPEARANCE (Visual Style Preview + Accent Circles + Wallpaper Grid + KDE Options)
// ============================================================================

pub fn build_appearance_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();

    // --- Style Preview Cards (Light vs Dark) ---
    page.append(&section_heading("Style"));
    let style_card = boxed_card();
    let preview_row = GtkBox::new(Orientation::Horizontal, 24);
    preview_row.set_halign(Align::Center);
    preview_row.set_margin_top(14);
    preview_row.set_margin_bottom(14);

    let light_btn = build_style_preview_card("Default", false);
    let dark_btn = build_style_preview_card("Dark", true);

    if ctrl.config.borrow().appearance.theme == ThemeMode::NeonLight {
        light_btn.add_css_class("active");
    } else {
        dark_btn.add_css_class("active");
    }

    {
        let ctrl = Rc::clone(&ctrl);
        let l = light_btn.clone();
        let d = dark_btn.clone();
        light_btn.connect_clicked(move |_| {
            ctrl.config.borrow_mut().appearance.theme = ThemeMode::NeonLight;
            l.add_css_class("active");
            d.remove_css_class("active");
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Switched to Neon Light");
        });
    }
    {
        let ctrl = Rc::clone(&ctrl);
        let l = light_btn.clone();
        let d = dark_btn.clone();
        dark_btn.connect_clicked(move |_| {
            ctrl.config.borrow_mut().appearance.theme = ThemeMode::NeonDark;
            d.add_css_class("active");
            l.remove_css_class("active");
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Switched to Neon Dark");
        });
    }

    preview_row.append(&light_btn);
    preview_row.append(&dark_btn);
    style_card.append(&preview_row);
    style_card.append(&row_separator());

    // Circular Colored Accent Swatches
    let swatch_row = GtkBox::new(Orientation::Horizontal, 10);
    swatch_row.set_valign(Align::Center);
    let current_accent = ctrl.config.borrow().appearance.accent_color.to_uppercase();
    let swatch_defs = [
        ("Blue", "#0A84FF", "accent-blue"),
        ("Cyan", "#38BDF8", "accent-cyan"),
        ("Violet", "#8B5CF6", "accent-violet"),
        ("Emerald", "#10B981", "accent-emerald"),
        ("Amber", "#F59E0B", "accent-amber"),
        ("Rose", "#F43F5E", "accent-rose"),
        ("Pink", "#EC4899", "accent-pink"),
    ];
    let swatch_buttons: Rc<RefCell<Vec<(Button, &'static str)>>> =
        Rc::new(RefCell::new(Vec::new()));
    for (name, hex, css_cls) in swatch_defs {
        let btn = Button::new();
        btn.add_css_class("accent-circle");
        btn.add_css_class(css_cls);
        btn.set_tooltip_text(Some(name));
        if current_accent == hex {
            btn.add_css_class("active");
        }
        swatch_buttons.borrow_mut().push((btn.clone(), hex));
        swatch_row.append(&btn);
    }
    for (btn, hex) in swatch_buttons.borrow().iter() {
        let ctrl = Rc::clone(&ctrl);
        let all = Rc::clone(&swatch_buttons);
        let hex_str = (*hex).to_string();
        btn.connect_clicked(move |_| {
            ctrl.config.borrow_mut().appearance.accent_color = hex_str.clone();
            for (b, h) in all.borrow().iter() {
                if *h == hex_str {
                    b.add_css_class("active");
                } else {
                    b.remove_css_class("active");
                }
            }
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), &format!("Accent color set to {hex_str}"));
        });
    }
    style_card.append(&setting_row(
        "Accent Color",
        "Applies live to focus rings, Control Center, dock dots, and controls.",
        &swatch_row,
    ));
    page.append(&style_card);

    // --- Background Wallpaper Visual Thumbnail Grid ---
    let bg_header = GtkBox::new(Orientation::Horizontal, 8);
    let bg_title = section_heading("Background");
    bg_title.set_hexpand(true);
    let auto_wp_btn = Button::with_label("Match Theme");
    auto_wp_btn.add_css_class("settings-secondary-btn");
    auto_wp_btn.set_valign(Align::Center);
    bg_header.append(&bg_title);
    bg_header.append(&auto_wp_btn);
    page.append(&bg_header);

    let wp_card = boxed_card();
    let wp_flow = FlowBox::new();
    wp_flow.set_selection_mode(SelectionMode::None);
    wp_flow.set_column_spacing(10);
    wp_flow.set_row_spacing(10);
    wp_flow.set_max_children_per_line(3);
    wp_flow.set_min_children_per_line(3);
    wp_flow.set_homogeneous(true);
    wp_flow.set_margin_top(12);
    wp_flow.set_margin_bottom(12);
    wp_flow.set_margin_start(14);
    wp_flow.set_margin_end(14);

    let wp_buttons: Rc<RefCell<Vec<(Button, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let active_wp = ctrl.config.borrow().appearance.wallpaper_path.clone();

    for wp_path in discover_wallpapers().into_iter().take(12) {
        let path_str = wp_path.to_string_lossy().into_owned();
        let name = wp_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Wallpaper")
            .replace(['-', '_'], " ");

        let pic = Picture::for_file(&gio::File::for_path(&wp_path));
        pic.set_content_fit(ContentFit::Cover);
        pic.set_can_shrink(true);
        pic.set_size_request(164, 102);
        pic.add_css_class("wp-thumb-pic");

        let btn = Button::new();
        btn.add_css_class("wp-thumb-btn");
        btn.set_tooltip_text(Some(&name));
        btn.set_child(Some(&pic));
        if active_wp.as_deref() == Some(path_str.as_str()) {
            btn.add_css_class("active");
        }
        wp_buttons.borrow_mut().push((btn.clone(), path_str));
        wp_flow.insert(&btn, -1);
    }

    for (btn, path_str) in wp_buttons.borrow().iter() {
        let ctrl = Rc::clone(&ctrl);
        let all = Rc::clone(&wp_buttons);
        let chosen = path_str.clone();
        btn.connect_clicked(move |_| {
            ctrl.config.borrow_mut().appearance.wallpaper_path = Some(chosen.clone());
            for (b, p) in all.borrow().iter() {
                if *p == chosen {
                    b.add_css_class("active");
                } else {
                    b.remove_css_class("active");
                }
            }
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated desktop wallpaper");
        });
    }
    {
        let ctrl = Rc::clone(&ctrl);
        let all = Rc::clone(&wp_buttons);
        auto_wp_btn.connect_clicked(move |_| {
            ctrl.config.borrow_mut().appearance.wallpaper_path = None;
            for (b, _) in all.borrow().iter() {
                b.remove_css_class("active");
            }
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Reset wallpaper to automatic theme match");
        });
    }

    wp_card.append(&wp_flow);
    page.append(&wp_card);

    // --- KDE-Level Icons, Cursor & Typography ---
    page.append(&section_heading("Icons, Cursor & Typography"));
    let kde_card = boxed_card();

    let icon_themes =
        StringList::new(&["WhiteSur-dark", "WhiteSur-light", "Papirus-Dark", "Adwaita"]);
    let icon_dd = DropDown::new(Some(icon_themes), gtk4::Expression::NONE);
    icon_dd.set_valign(Align::Center);
    icon_dd.set_selected(match ctrl.config.borrow().appearance.icon_theme.as_str() {
        "WhiteSur-light" => 1,
        "Papirus-Dark" => 2,
        "Adwaita" => 3,
        _ => 0,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        icon_dd.connect_selected_notify(move |dd| {
            let name = match dd.selected() {
                1 => "WhiteSur-light",
                2 => "Papirus-Dark",
                3 => "Adwaita",
                _ => "WhiteSur-dark",
            };
            ctrl.config.borrow_mut().appearance.icon_theme = name.into();
            if let Some(s) = gtk4::Settings::default() {
                s.set_gtk_icon_theme_name(Some(name));
            }
            ctrl.persist_and_sync(None, &format!("Set icon theme to {name}"));
        });
    }

    let cursor_themes = StringList::new(&["Bibata-Modern-Ice", "Adwaita", "breeze_cursors"]);
    let cursor_dd = DropDown::new(Some(cursor_themes), gtk4::Expression::NONE);
    cursor_dd.set_valign(Align::Center);
    cursor_dd.set_selected(
        match ctrl.config.borrow().appearance.cursor_theme.as_str() {
            "Adwaita" => 1,
            "breeze_cursors" => 2,
            _ => 0,
        },
    );
    {
        let ctrl = Rc::clone(&ctrl);
        cursor_dd.connect_selected_notify(move |dd| {
            let name = match dd.selected() {
                1 => "Adwaita",
                2 => "breeze_cursors",
                _ => "Bibata-Modern-Ice",
            };
            ctrl.config.borrow_mut().appearance.cursor_theme = name.into();
            ctrl.persist_and_sync(None, &format!("Set cursor theme to {name}"));
        });
    }

    let density_opts = StringList::new(&["Compact", "Comfortable", "Spacious"]);
    let density_dd = DropDown::new(Some(density_opts), gtk4::Expression::NONE);
    density_dd.set_valign(Align::Center);
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

    let font_scale = Scale::with_range(Orientation::Horizontal, 9.0, 18.0, 1.0);
    font_scale.add_css_class("settings-scale");
    font_scale.set_width_request(180);
    font_scale.set_draw_value(true);
    font_scale.set_value(f64::from(ctrl.config.borrow().appearance.font_size));
    {
        let ctrl = Rc::clone(&ctrl);
        font_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().appearance.font_size = s.value().round() as u8;
            ctrl.persist_and_sync(None, "Updated base font size");
        });
    }

    append_rows(
        &kde_card,
        vec![
            setting_row(
                "Icon Pack",
                "Default macOS WhiteSur squircle icons.",
                &icon_dd,
            ),
            setting_row(
                "Cursor Theme",
                "Applied to bare-metal XCursor rendering.",
                &cursor_dd,
            ),
            setting_row(
                "Interface Density",
                "Controls widget padding across NEON DE.",
                &density_dd,
            ),
            setting_row("Font Size", "Preferred UI text point size.", &font_scale),
        ],
    );
    page.append(&kde_card);

    scroller
}

fn build_style_preview_card(label: &str, dark: bool) -> Button {
    let btn = Button::new();
    btn.add_css_class("style-preview-btn");

    let vbox = GtkBox::new(Orientation::Vertical, 8);
    vbox.set_halign(Align::Center);

    let frame = GtkBox::new(Orientation::Vertical, 0);
    frame.add_css_class("style-preview-frame");
    frame.add_css_class(if dark {
        "style-preview-dark-bg"
    } else {
        "style-preview-light-bg"
    });
    frame.set_halign(Align::Center);
    frame.set_valign(Align::Center);

    let mini = GtkBox::new(Orientation::Vertical, 0);
    mini.add_css_class(if dark {
        "style-mini-win-dark"
    } else {
        "style-mini-win-light"
    });
    mini.set_halign(Align::Center);
    mini.set_valign(Align::Center);
    frame.append(&mini);

    let lbl = Label::new(Some(label));
    lbl.add_css_class("settings-row-title");
    vbox.append(&frame);
    vbox.append(&lbl);
    btn.set_child(Some(&vbox));
    btn
}

fn discover_wallpapers() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Pictures/Wallpapers"));
    }
    dirs.push(PathBuf::from("/usr/share/backgrounds"));

    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|s| s.to_str())
                && matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp"
                )
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

// ============================================================================
// 2. DISPLAYS
// ============================================================================

pub fn build_displays_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();
    page.append(&section_heading("Connected Displays"));

    let outputs = ctrl.fetch_outputs();
    if outputs.is_empty() {
        let card = boxed_card();
        card.append(&setting_row(
            "No Active Compositor Outputs",
            "Launch neon-settings inside a running NEON DE session to configure live scaling and rotation.",
            &Label::new(Some("Offline")),
        ));
        page.append(&card);
        return scroller;
    }

    for out in outputs {
        let card = boxed_card();
        let scale_opts = StringList::new(&["100%", "125%", "150%", "175%", "200%"]);
        let scale_dd = DropDown::new(Some(scale_opts), gtk4::Expression::NONE);
        scale_dd.set_valign(Align::Center);
        scale_dd.set_selected(if out.scale >= 1.9 {
            4
        } else if out.scale >= 1.65 {
            3
        } else if out.scale >= 1.4 {
            2
        } else if out.scale >= 1.15 {
            1
        } else {
            0
        });

        let rot_opts = StringList::new(&[
            "Landscape (0°)",
            "Portrait Right (90°)",
            "Flipped (180°)",
            "Portrait Left (270°)",
        ]);
        let rot_dd = DropDown::new(Some(rot_opts), gtk4::Expression::NONE);
        rot_dd.set_valign(Align::Center);
        rot_dd.set_selected(match out.transform.as_str() {
            "rotate90" => 1,
            "rotate180" => 2,
            "rotate270" => 3,
            _ => 0,
        });

        let apply_btn = Button::with_label("Apply");
        apply_btn.add_css_class("settings-action-btn");
        {
            let ctrl = Rc::clone(&ctrl);
            let out_name = out.name.clone();
            let scale_c = scale_dd.clone();
            let rot_c = rot_dd.clone();
            apply_btn.connect_clicked(move |_| {
                let scale = match scale_c.selected() {
                    1 => 1.25,
                    2 => 1.5,
                    3 => 1.75,
                    4 => 2.0,
                    _ => 1.0,
                };
                let transform = match rot_c.selected() {
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
                    primary: None,
                    transform: Some(transform.into()),
                };
                ctrl.persist_and_sync(Some(req), &format!("Updated display {out_name}"));
            });
        }

        let res_lbl = Label::new(Some(&format!(
            "{} × {} @ {:.0} Hz",
            out.width,
            out.height,
            out.refresh_mhz as f64 / 1000.0
        )));
        res_lbl.add_css_class("settings-badge");

        append_rows(
            &card,
            vec![
                setting_row(
                    &out.name,
                    if out.primary {
                        "Primary Display"
                    } else {
                        "Secondary Display"
                    },
                    &res_lbl,
                ),
                setting_row(
                    "Scale",
                    "Fractional HiDPI scaling (wp_fractional_scale_v1).",
                    &scale_dd,
                ),
                setting_row("Orientation", "Display transform rotation.", &rot_dd),
                setting_row(
                    "Apply Output Changes",
                    "Updates monitor geometry live without restart.",
                    &apply_btn,
                ),
            ],
        );
        page.append(&card);
    }

    scroller
}

// ============================================================================
// 3. DOCK & PANELS
// ============================================================================

pub fn build_dock_panels_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();

    page.append(&section_heading("Menu Bar (Top Panel)"));
    let panel_card = boxed_card();

    let panel_vis = Switch::new();
    panel_vis.set_valign(Align::Center);
    panel_vis.set_active(ctrl.config.borrow().panel.visible);
    {
        let ctrl = Rc::clone(&ctrl);
        panel_vis.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().panel.visible = active;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled Menu Bar visibility");
            gtk4::glib::Propagation::Proceed
        });
    }

    let clock_sw = Switch::new();
    clock_sw.set_valign(Align::Center);
    clock_sw.set_active(ctrl.config.borrow().panel.show_clock);
    {
        let ctrl = Rc::clone(&ctrl);
        clock_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().panel.show_clock = active;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled Menu Bar clock");
            gtk4::glib::Propagation::Proceed
        });
    }

    let ws_sw = Switch::new();
    ws_sw.set_valign(Align::Center);
    ws_sw.set_active(ctrl.config.borrow().panel.show_workspaces);
    {
        let ctrl = Rc::clone(&ctrl);
        ws_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().panel.show_workspaces = active;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled workspace indicator dots");
            gtk4::glib::Propagation::Proceed
        });
    }

    let panel_op = Scale::with_range(Orientation::Horizontal, 0.45, 1.0, 0.05);
    panel_op.add_css_class("settings-scale");
    panel_op.set_width_request(180);
    panel_op.set_value(f64::from(ctrl.config.borrow().panel.transparency));
    {
        let ctrl = Rc::clone(&ctrl);
        panel_op.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().panel.transparency = s.value() as f32;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated Menu Bar opacity");
        });
    }

    let panel_pos = DropDown::new(
        Some(StringList::new(&["Top", "Bottom"])),
        gtk4::Expression::NONE,
    );
    panel_pos.set_valign(Align::Center);
    panel_pos.set_selected(match ctrl.config.borrow().panel.position {
        PanelPosition::Top => 0,
        PanelPosition::Bottom => 1,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        panel_pos.connect_selected_notify(move |dd| {
            ctrl.config.borrow_mut().panel.position = if dd.selected() == 1 {
                PanelPosition::Bottom
            } else {
                PanelPosition::Top
            };
            ctrl.persist_and_sync(None, "Saved Menu Bar position");
        });
    }

    append_rows(
        &panel_card,
        vec![
            setting_row(
                "Show Menu Bar",
                "Full-width translucent bar across the top edge.",
                &panel_vis,
            ),
            setting_row(
                "Show Date & Time",
                "Displays live clock on the right side of the Menu Bar.",
                &clock_sw,
            ),
            setting_row(
                "Show Workspace Dots",
                "Dynamic GNOME-style workspace indicators.",
                &ws_sw,
            ),
            setting_row(
                "Menu Bar Opacity",
                "Controls frosted glass translucency live.",
                &panel_op,
            ),
            setting_row(
                "Panel Edge",
                "Preferred screen edge for the Menu Bar.",
                &panel_pos,
            ),
        ],
    );
    page.append(&panel_card);

    page.append(&section_heading("Floating Dock"));
    let dock_card = boxed_card();

    let dock_op = Scale::with_range(Orientation::Horizontal, 0.40, 1.0, 0.05);
    dock_op.add_css_class("settings-scale");
    dock_op.set_width_request(180);
    dock_op.set_value(f64::from(ctrl.config.borrow().dock.transparency));
    {
        let ctrl = Rc::clone(&ctrl);
        dock_op.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().dock.transparency = s.value() as f32;
            let req = build_appearance_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated Dock opacity");
        });
    }

    let dock_pos = DropDown::new(
        Some(StringList::new(&["Bottom", "Left", "Right"])),
        gtk4::Expression::NONE,
    );
    dock_pos.set_valign(Align::Center);
    dock_pos.set_selected(match ctrl.config.borrow().dock.position {
        DockPosition::Bottom => 0,
        DockPosition::Left => 1,
        DockPosition::Right => 2,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        dock_pos.connect_selected_notify(move |dd| {
            ctrl.config.borrow_mut().dock.position = match dd.selected() {
                1 => DockPosition::Left,
                2 => DockPosition::Right,
                _ => DockPosition::Bottom,
            };
            ctrl.persist_and_sync(None, "Saved Dock position");
        });
    }

    let autohide_sw = Switch::new();
    autohide_sw.set_valign(Align::Center);
    autohide_sw.set_active(ctrl.config.borrow().dock.auto_hide);
    {
        let ctrl = Rc::clone(&ctrl);
        autohide_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().dock.auto_hide = active;
            ctrl.persist_and_sync(None, "Saved Dock auto-hide");
            gtk4::glib::Propagation::Proceed
        });
    }

    append_rows(
        &dock_card,
        vec![
            setting_row(
                "Dock Opacity",
                "Controls frosted glass dock capsule opacity live.",
                &dock_op,
            ),
            setting_row(
                "Position on Screen",
                "Bottom, Left, or Right edge placement.",
                &dock_pos,
            ),
            setting_row(
                "Automatically Hide Dock",
                "Intelligently hides the dock when windows maximize.",
                &autohide_sw,
            ),
        ],
    );
    page.append(&dock_card);

    scroller
}

// ============================================================================
// 4. WINDOWS & EFFECTS (KDE-Grade Compositor & Window Manager Controls)
// ============================================================================

pub fn build_windows_effects_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_clamp();

    page.append(&section_heading("Window Management & Workspaces"));
    let wm_card = boxed_card();

    let focus_dd = DropDown::new(
        Some(StringList::new(&["Click to Focus", "Focus Follows Mouse"])),
        gtk4::Expression::NONE,
    );
    focus_dd.set_valign(Align::Center);
    focus_dd.set_selected(match ctrl.config.borrow().windows.focus_mode {
        FocusMode::ClickToFocus => 0,
        FocusMode::FocusFollowsMouse => 1,
    });
    {
        let ctrl = Rc::clone(&ctrl);
        focus_dd.connect_selected_notify(move |dd| {
            ctrl.config.borrow_mut().windows.focus_mode = if dd.selected() == 1 {
                FocusMode::FocusFollowsMouse
            } else {
                FocusMode::ClickToFocus
            };
            let req = build_window_behavior_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated window focus policy");
        });
    }

    let gap_scale = Scale::with_range(Orientation::Horizontal, 0.0, 32.0, 2.0);
    gap_scale.add_css_class("settings-scale");
    gap_scale.set_width_request(180);
    gap_scale.set_draw_value(true);
    gap_scale.set_value(f64::from(ctrl.config.borrow().windows.snap_gap));
    {
        let ctrl = Rc::clone(&ctrl);
        gap_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().windows.snap_gap = s.value().round() as i32;
            let req = build_window_behavior_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated window snap gap");
        });
    }

    let snap_sw = Switch::new();
    snap_sw.set_valign(Align::Center);
    snap_sw.set_active(ctrl.config.borrow().windows.auto_snap);
    {
        let ctrl = Rc::clone(&ctrl);
        snap_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().windows.auto_snap = active;
            ctrl.persist_and_sync(None, "Saved window snapping preference");
            gtk4::glib::Propagation::Proceed
        });
    }

    let center_sw = Switch::new();
    center_sw.set_valign(Align::Center);
    center_sw.set_active(ctrl.config.borrow().windows.center_dialogs);
    {
        let ctrl = Rc::clone(&ctrl);
        center_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().windows.center_dialogs = active;
            ctrl.persist_and_sync(None, "Saved dialog centering preference");
            gtk4::glib::Propagation::Proceed
        });
    }

    append_rows(
        &wm_card,
        vec![
            setting_row(
                "Focus Policy",
                "Choose macOS click-to-focus or KDE focus-follows-mouse.",
                &focus_dd,
            ),
            setting_row(
                "Tiling / Snap Gap (px)",
                "Margin between snapped half/quarter windows.",
                &gap_scale,
            ),
            setting_row(
                "Edge & Shortcut Snapping",
                "Enable Super+Left/Right/Up smart window snapping.",
                &snap_sw,
            ),
            setting_row(
                "Center Transient Dialogs",
                "Automatically center modal dialogs over parent windows.",
                &center_sw,
            ),
        ],
    );
    page.append(&wm_card);

    page.append(&section_heading("GPU Visual Effects"));
    let fx_card = boxed_card();

    let shadows_sw = Switch::new();
    shadows_sw.set_valign(Align::Center);
    shadows_sw.set_active(ctrl.config.borrow().effects.shadows_enabled);
    {
        let ctrl = Rc::clone(&ctrl);
        shadows_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().effects.shadows_enabled = active;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled GPU drop shadows");
            gtk4::glib::Propagation::Proceed
        });
    }

    let borders_sw = Switch::new();
    borders_sw.set_valign(Align::Center);
    borders_sw.set_active(ctrl.config.borrow().effects.borders_enabled);
    {
        let ctrl = Rc::clone(&ctrl);
        borders_sw.connect_state_set(move |_, active| {
            ctrl.config.borrow_mut().effects.borders_enabled = active;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Toggled accent focus rings");
            gtk4::glib::Propagation::Proceed
        });
    }

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

    let radius_scale = Scale::with_range(Orientation::Horizontal, 0.0, 24.0, 1.0);
    radius_scale.add_css_class("settings-scale");
    radius_scale.set_width_request(180);
    radius_scale.set_draw_value(true);
    radius_scale.set_value(f64::from(ctrl.config.borrow().effects.corner_radius));
    {
        let ctrl = Rc::clone(&ctrl);
        radius_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().effects.corner_radius = s.value() as f32;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated window corner radius");
        });
    }

    let inactive_scale = Scale::with_range(Orientation::Horizontal, 0.70, 1.0, 0.02);
    inactive_scale.add_css_class("settings-scale");
    inactive_scale.set_width_request(180);
    inactive_scale.set_value(f64::from(ctrl.config.borrow().effects.inactive_opacity));
    {
        let ctrl = Rc::clone(&ctrl);
        inactive_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().effects.inactive_opacity = s.value() as f32;
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated inactive window opacity");
        });
    }

    let speed_scale = Scale::with_range(Orientation::Horizontal, 0.25, 2.5, 0.25);
    speed_scale.add_css_class("settings-scale");
    speed_scale.set_width_request(180);
    speed_scale.set_draw_value(true);
    speed_scale.set_value(ctrl.config.borrow().effects.speed_scale);
    {
        let ctrl = Rc::clone(&ctrl);
        speed_scale.connect_value_changed(move |s| {
            ctrl.config.borrow_mut().effects.speed_scale = s.value();
            let req = build_effects_request(&ctrl.config.borrow());
            ctrl.persist_and_sync(Some(req), "Updated animation speed scale");
        });
    }

    append_rows(
        &fx_card,
        vec![
            setting_row(
                "Drop Shadows",
                "Soft analytical SDF shadows rendered on the GPU.",
                &shadows_sw,
            ),
            setting_row(
                "Accent Focus Rings",
                "Subtle glowing rim around the active window.",
                &borders_sw,
            ),
            setting_row(
                "Frosted-Glass Blur",
                "9-tap GPU diffusion backdrop behind windows.",
                &blur_sw,
            ),
            setting_row(
                "Corner Radius (px)",
                "Curvature for shadows, focus rings, and frosted glass.",
                &radius_scale,
            ),
            setting_row(
                "Inactive Window Opacity",
                "Dim unfocused windows slightly for focus clarity.",
                &inactive_scale,
            ),
            setting_row(
                "Animation Speed",
                "Scale compositor transition speed (0.25× – 2.5×).",
                &speed_scale,
            ),
        ],
    );
    page.append(&fx_card);

    scroller
}
