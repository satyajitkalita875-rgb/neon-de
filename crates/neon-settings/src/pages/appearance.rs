//! 5.1 Appearance settings: Light/Dark/Auto theme, accent swatches + custom hex,
//! wallpaper gallery + scaling/slideshow, Reduce Blur & intensity, GTK/Icon/Cursor themes,
//! pointer size, fonts, scaling, density, and login screen capability status.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gtk4::{
    Align, Box as GtkBox, Button, ContentFit, Entry, FlowBox, Label, Orientation, Picture,
    ScrolledWindow, SelectionMode, gdk, gdk_pixbuf, gio, prelude::*,
};
use neon_config::{ThemeMode, UiDensity, WallpaperScaleMode};

use crate::{
    metadata::CapabilityStatus,
    state::{
        SettingsController, apply_system_cursor, apply_toolkit_appearance,
        build_appearance_request, build_effects_request,
    },
    ui::{
        append_rows, boxed_card, capability_row, dropdown_row, page_shell, row_separator,
        section_heading, setting_row, slider_row, toggle_row,
    },
};

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell(
        "Appearance",
        "Customize visual style, accent color, wallpapers, icons, cursors, fonts, and glass blur.",
        Some("appearance"),
        &ctrl,
    );

    // ── Theme Mode & Accent Color ────────────────────────────────────────────
    page.append(&section_heading("Theme & Accent"));
    let style_card = boxed_card();

    let preview_row = GtkBox::new(Orientation::Horizontal, 16);
    preview_row.set_halign(Align::Center);
    preview_row.set_margin_top(14);
    preview_row.set_margin_bottom(14);

    let light_btn = build_style_preview_card("Light", "light");
    let dark_btn = build_style_preview_card("Dark", "dark");
    let auto_btn = build_style_preview_card("Automatic", "auto");

    let update_active_theme_cards = {
        let l = light_btn.clone();
        let d = dark_btn.clone();
        let a = auto_btn.clone();
        move |mode: ThemeMode| {
            l.remove_css_class("active");
            d.remove_css_class("active");
            a.remove_css_class("active");
            match mode {
                ThemeMode::NeonLight => l.add_css_class("active"),
                ThemeMode::NeonDark => d.add_css_class("active"),
                ThemeMode::Auto => a.add_css_class("active"),
            }
        }
    };
    update_active_theme_cards(ctrl.config.borrow().appearance.theme);

    for (btn, mode, label) in [
        (&light_btn, ThemeMode::NeonLight, "Switched to Neon Light"),
        (&dark_btn, ThemeMode::NeonDark, "Switched to Neon Dark"),
        (&auto_btn, ThemeMode::Auto, "Switched to Automatic theme"),
    ] {
        let ctrl_c = Rc::clone(&ctrl);
        let upd = update_active_theme_cards.clone();
        btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().appearance.theme = mode;
            upd(mode);
            let req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(req), label);
        });
    }

    preview_row.append(&light_btn);
    preview_row.append(&dark_btn);
    preview_row.append(&auto_btn);
    style_card.append(&preview_row);
    style_card.append(&row_separator());

    // Accent swatches + custom hex entry
    let accent_controls = GtkBox::new(Orientation::Horizontal, 10);
    accent_controls.set_valign(Align::Center);
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

    let hex_entry = Entry::new();
    hex_entry.set_width_chars(8);
    hex_entry.set_text(&current_accent);
    hex_entry.set_tooltip_text(Some("Custom hex color (#RRGGBB)"));

    for (name, hex, css_cls) in swatch_defs {
        let btn = Button::new();
        btn.add_css_class("accent-circle");
        btn.add_css_class(css_cls);
        btn.set_tooltip_text(Some(name));
        if current_accent == hex {
            btn.add_css_class("active");
        }
        accent_controls.append(&btn);
        swatch_buttons.borrow_mut().push((btn, hex));
    }

    for (btn, hex) in swatch_buttons.borrow().iter() {
        let ctrl_c = Rc::clone(&ctrl);
        let all = Rc::clone(&swatch_buttons);
        let entry_c = hex_entry.clone();
        let hex_str = *hex;
        btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().appearance.accent_color = hex_str.to_string();
            entry_c.set_text(hex_str);
            for (b, h) in all.borrow().iter() {
                if *h == hex_str {
                    b.add_css_class("active");
                } else {
                    b.remove_css_class("active");
                }
            }
            let req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(req), &format!("Updated accent color to {hex_str}"));
        });
    }

    let apply_hex_btn = Button::with_label("Apply");
    apply_hex_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        let entry_c = hex_entry.clone();
        let all = Rc::clone(&swatch_buttons);
        apply_hex_btn.connect_clicked(move |_| {
            let val = entry_c.text().trim().to_uppercase();
            if val.len() == 7 && val.starts_with('#') && val[1..].chars().all(|c| c.is_ascii_hexdigit()) {
                ctrl_c.config.borrow_mut().appearance.accent_color = val.clone();
                for (b, h) in all.borrow().iter() {
                    if *h == val {
                        b.add_css_class("active");
                    } else {
                        b.remove_css_class("active");
                    }
                }
                let req = build_appearance_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(Some(req), &format!("Applied custom accent {val}"));
            } else {
                ctrl_c.set_status_err("Enter a valid #RRGGBB hex color (e.g. #38BDF8)");
            }
        });
    }
    accent_controls.append(&hex_entry);
    accent_controls.append(&apply_hex_btn);

    style_card.append(&setting_row(
        "Accent Color",
        "Highlights buttons, active sidebar items, switches, and focus rings.",
        &accent_controls,
    ));
    page.append(&style_card);

    // ── Blur & Glass Effects (Performance) ───────────────────────────────────
    page.append(&section_heading("Glass, Blur & Corners"));
    let blur_card = boxed_card();

    let reduce_blur = ctrl.config.borrow().appearance.reduce_blur;
    let blur_strength = f64::from(ctrl.config.borrow().effects.blur_strength);
    let corner_radius = f64::from(ctrl.config.borrow().effects.corner_radius);
    let shadows_enabled = ctrl.config.borrow().effects.shadows_enabled;

    let reduce_blur_row = toggle_row(
        "Reduce Blur",
        "Disables heavy frosted-glass backdrop blur across panels, dock, and windows for snappier rendering.",
        reduce_blur,
        {
            let ctrl_c = Rc::clone(&ctrl);
            move |active| {
                {
                    let mut cfg = ctrl_c.config.borrow_mut();
                    cfg.appearance.reduce_blur = active;
                    cfg.effects.blur_enabled = !active;
                }
                let eff_req = build_effects_request(&ctrl_c.config.borrow());
                let _ = ctrl_c.send_ipc(eff_req);
                let app_req = build_appearance_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(
                    Some(app_req),
                    if active {
                        "Reduced desktop blur for smoother performance"
                    } else {
                        "Restored frosted-glass desktop blur"
                    },
                );
            }
        },
    );

    let blur_intensity_row = slider_row(
        "Blur Intensity",
        "Adjust backdrop blur radius from 0.0 (off) to 2.0 (deep frosted glass).",
        0.0,
        2.0,
        0.1,
        blur_strength,
        1,
        {
            let ctrl_c = Rc::clone(&ctrl);
            move |val| {
                {
                    let mut cfg = ctrl_c.config.borrow_mut();
                    cfg.effects.blur_strength = val as f32;
                    cfg.effects.blur_enabled = val > 0.05 && !cfg.appearance.reduce_blur;
                }
                let eff_req = build_effects_request(&ctrl_c.config.borrow());
                let _ = ctrl_c.send_ipc(eff_req);
                let app_req = build_appearance_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(Some(app_req), &format!("Blur intensity set to {val:.1}"));
            }
        },
    );

    let corner_row = slider_row(
        "Window Corner Radius",
        "Pixel rounding radius for compositor-managed windows.",
        0.0,
        24.0,
        1.0,
        corner_radius,
        0,
        {
            let ctrl_c = Rc::clone(&ctrl);
            move |val| {
                ctrl_c.config.borrow_mut().effects.corner_radius = val as f32;
                let req = build_effects_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(Some(req), &format!("Window corner radius: {:.0} px", val));
            }
        },
    );

    let shadow_row = toggle_row(
        "Window Drop Shadows",
        "Soft elevation shadows beneath floating surfaces.",
        shadows_enabled,
        {
            let ctrl_c = Rc::clone(&ctrl);
            move |active| {
                ctrl_c.config.borrow_mut().effects.shadows_enabled = active;
                let req = build_effects_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(Some(req), "Updated window shadows");
            }
        },
    );

    let open_effects_btn = Button::with_label("Effects & Animations →");
    open_effects_btn.add_css_class("settings-secondary-btn");
    {
        let ctrl_c = Rc::clone(&ctrl);
        open_effects_btn.connect_clicked(move |_| ctrl_c.navigate_to("effects", None));
    }

    append_rows(
        &blur_card,
        vec![
            reduce_blur_row,
            blur_intensity_row,
            corner_row,
            shadow_row,
            setting_row(
                "Animations & Quality Presets",
                "Fine-tune open/close/workspace transitions and Performance presets.",
                &open_effects_btn,
            ),
        ],
    );
    page.append(&blur_card);

    // ── Wallpaper Gallery & Scaling ──────────────────────────────────────────
    page.append(&section_heading("Wallpaper"));
    let wp_card = boxed_card();

    let wp_actions = GtkBox::new(Orientation::Horizontal, 8);
    let auto_wp_btn = Button::with_label("Match Theme");
    auto_wp_btn.add_css_class("settings-secondary-btn");
    let add_wp_btn = Button::with_label("Choose Image…");
    add_wp_btn.add_css_class("settings-action-btn");
    wp_actions.append(&auto_wp_btn);
    wp_actions.append(&add_wp_btn);

    wp_card.append(&setting_row(
        "Desktop Background",
        "Select a local wallpaper or pick any image file.",
        &wp_actions,
    ));
    wp_card.append(&row_separator());

    let wp_flow = FlowBox::new();
    wp_flow.set_selection_mode(SelectionMode::None);
    wp_flow.set_max_children_per_line(3);
    wp_flow.set_min_children_per_line(2);
    wp_flow.set_row_spacing(10);
    wp_flow.set_column_spacing(10);
    wp_flow.set_margin_top(12);
    wp_flow.set_margin_bottom(12);
    wp_flow.set_margin_start(14);
    wp_flow.set_margin_end(14);

    let wallpapers = discover_wallpapers();
    let current_wp = ctrl.config.borrow().appearance.wallpaper_path.clone();
    let wp_buttons: Rc<RefCell<Vec<(Button, String)>>> = Rc::new(RefCell::new(Vec::new()));

    for path in wallpapers.into_iter().take(12) {
        let path_str = path.to_string_lossy().to_string();
        let pic = if let Ok(pb) = gdk_pixbuf::Pixbuf::from_file_at_scale(&path, 320, 200, true) {
            Picture::for_paintable(&gdk::Texture::for_pixbuf(&pb))
        } else {
            Picture::for_file(&gio::File::for_path(&path))
        };
        pic.set_size_request(164, 102);
        pic.set_content_fit(ContentFit::Cover);
        pic.add_css_class("wp-thumb-pic");

        let btn = Button::new();
        btn.add_css_class("wp-thumb-btn");
        if current_wp.as_deref() == Some(path_str.as_str()) {
            btn.add_css_class("active");
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            btn.set_tooltip_text(Some(stem));
        }
        btn.set_child(Some(&pic));
        wp_flow.insert(&btn, -1);
        wp_buttons.borrow_mut().push((btn, path_str));
    }

    for (btn, path_str) in wp_buttons.borrow().iter() {
        let ctrl_c = Rc::clone(&ctrl);
        let all = Rc::clone(&wp_buttons);
        let p = path_str.clone();
        btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().appearance.wallpaper_path = Some(p.clone());
            for (b, bp) in all.borrow().iter() {
                if bp == &p {
                    b.add_css_class("active");
                } else {
                    b.remove_css_class("active");
                }
            }
            let req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(req), "Updated desktop wallpaper");
        });
    }

    {
        let ctrl_c = Rc::clone(&ctrl);
        let all = Rc::clone(&wp_buttons);
        auto_wp_btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().appearance.wallpaper_path = None;
            for (b, _) in all.borrow().iter() {
                b.remove_css_class("active");
            }
            let req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(req), "Reset wallpaper to automatic theme match");
        });
    }

    {
        let ctrl_c = Rc::clone(&ctrl);
        let all = Rc::clone(&wp_buttons);
        add_wp_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::builder()
                .title("Select Desktop Wallpaper")
                .modal(true)
                .build();
            let ctrl_inner = Rc::clone(&ctrl_c);
            let all_inner = Rc::clone(&all);
            dialog.open(Some(&ctrl_c.window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    let p = path.to_string_lossy().to_string();
                    ctrl_inner.config.borrow_mut().appearance.wallpaper_path = Some(p.clone());
                    for (b, _) in all_inner.borrow().iter() {
                        b.remove_css_class("active");
                    }
                    let req = build_appearance_request(&ctrl_inner.config.borrow());
                    ctrl_inner.persist_and_sync(Some(req), "Applied custom wallpaper");
                }
            });
        });
    }

    wp_card.append(&wp_flow);
    wp_card.append(&row_separator());

    let scale_idx = match ctrl.config.borrow().appearance.wallpaper_scale_mode {
        WallpaperScaleMode::Cover => 0,
        WallpaperScaleMode::Fit => 1,
        WallpaperScaleMode::Stretch => 2,
        WallpaperScaleMode::Center => 3,
    };
    let slideshow = ctrl.config.borrow().appearance.wallpaper_slideshow;
    let slideshow_mins = f64::from(ctrl.config.borrow().appearance.wallpaper_slideshow_minutes);

    append_rows(
        &wp_card,
        vec![
            dropdown_row(
                "Wallpaper Scaling",
                "How wallpapers fit displays of different aspect ratios.",
                &["Fill (Cover)", "Fit (Letterbox)", "Stretch", "Center"],
                scale_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().appearance.wallpaper_scale_mode = match idx {
                            1 => WallpaperScaleMode::Fit,
                            2 => WallpaperScaleMode::Stretch,
                            3 => WallpaperScaleMode::Center,
                            _ => WallpaperScaleMode::Cover,
                        };
                        ctrl_c.persist_and_sync(None, "Updated wallpaper scaling mode");
                    }
                },
            ),
            toggle_row(
                "Wallpaper Slideshow",
                "Rotate through discovered local wallpapers automatically.",
                slideshow,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().appearance.wallpaper_slideshow = active;
                        ctrl_c.persist_and_sync(None, "Updated wallpaper slideshow setting");
                    }
                },
            ),
            slider_row(
                "Slideshow Interval (minutes)",
                "Duration between automatic wallpaper transitions.",
                5.0,
                180.0,
                5.0,
                slideshow_mins,
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().appearance.wallpaper_slideshow_minutes =
                            val.round() as u32;
                        ctrl_c.persist_and_sync(None, &format!("Slideshow interval: {:.0} min", val));
                    }
                },
            ),
        ],
    );
    page.append(&wp_card);

    // ── GTK, Icon, Cursor & Typography ───────────────────────────────────────
    page.append(&section_heading("Toolkit Themes, Cursor & Typography"));
    let theme_card = boxed_card();

    let gtk_themes = ["adw-gtk3-dark", "adw-gtk3", "Adwaita-dark", "Adwaita", "Yaru-dark", "Yaru"];
    let gtk_idx = gtk_themes
        .iter()
        .position(|t| *t == ctrl.config.borrow().appearance.gtk_theme.as_str())
        .unwrap_or(0) as u32;

    let icon_themes = ["WhiteSur-dark", "WhiteSur-light", "Papirus-Dark", "Adwaita", "Yaru"];
    let icon_idx = icon_themes
        .iter()
        .position(|t| *t == ctrl.config.borrow().appearance.icon_theme.as_str())
        .unwrap_or(0) as u32;

    let cursor_names = ["macOS", "macOS-White", "Bibata-Modern-Ice", "Adwaita", "breeze_cursors"];
    let cursor_idx = cursor_names
        .iter()
        .position(|n| *n == ctrl.config.borrow().appearance.cursor_theme.as_str())
        .unwrap_or(0) as u32;

    let cursor_sizes: [u32; 5] = [16, 18, 20, 24, 32];
    let cur_sz_idx = cursor_sizes
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| (**s as i32 - ctrl.config.borrow().appearance.cursor_size as i32).abs())
        .map(|(i, _)| i as u32)
        .unwrap_or(2);

    let font_families = ["Inter", "Adwaita Sans", "Ubuntu", "Cantarell", "Noto Sans", "DejaVu Sans"];
    let font_idx = font_families
        .iter()
        .position(|f| *f == ctrl.config.borrow().appearance.font_family.as_str())
        .unwrap_or(0) as u32;

    let density_idx = match ctrl.config.borrow().appearance.ui_density {
        UiDensity::Compact => 0,
        UiDensity::Comfortable => 1,
        UiDensity::Spacious => 2,
    };

    let font_size = f64::from(ctrl.config.borrow().appearance.font_size);
    let font_scale = ctrl.config.borrow().appearance.font_scale;

    append_rows(
        &theme_card,
        vec![
            dropdown_row(
                "GTK Theme",
                "Applied to GTK3/GTK4 desktop applications via gsettings.",
                &gtk_themes,
                gtk_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let name = gtk_themes.get(idx as usize).copied().unwrap_or("adw-gtk3-dark");
                        ctrl_c.config.borrow_mut().appearance.gtk_theme = name.into();
                        apply_toolkit_appearance(Some(name), None, None, None);
                        ctrl_c.persist_and_sync(None, &format!("Set GTK theme to {name}"));
                    }
                },
            ),
            dropdown_row(
                "Icon Theme",
                "Desktop icon pack for panels, dock, and applications.",
                &icon_themes,
                icon_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let name = icon_themes.get(idx as usize).copied().unwrap_or("WhiteSur-dark");
                        ctrl_c.config.borrow_mut().appearance.icon_theme = name.into();
                        apply_toolkit_appearance(None, Some(name), None, None);
                        ctrl_c.persist_and_sync(None, &format!("Set icon theme to {name}"));
                    }
                },
            ),
            dropdown_row(
                "Cursor Theme",
                "Applied live to compositor and GTK/Wayland apps.",
                &cursor_names,
                cursor_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let name = cursor_names.get(idx as usize).copied().unwrap_or("macOS");
                        ctrl_c.config.borrow_mut().appearance.cursor_theme = name.into();
                        let sz = ctrl_c.config.borrow().appearance.cursor_size;
                        apply_system_cursor(name, sz);
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Set cursor theme to {name}"));
                    }
                },
            ),
            dropdown_row(
                "Pointer Size",
                "Mouse cursor size in logical pixels (20 px matches macOS).",
                &[
                    "16 px (Small)",
                    "18 px (Compact)",
                    "20 px (macOS)",
                    "24 px (Standard)",
                    "32 px (Large)",
                ],
                cur_sz_idx,
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
            dropdown_row(
                "System Font",
                "Default sans-serif typeface for interface labels.",
                &font_families,
                font_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        let fam = font_families.get(idx as usize).copied().unwrap_or("Inter");
                        let sz = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.appearance.font_family = fam.into();
                            cfg.appearance.font_size
                        };
                        let spec = format!("{fam} {sz}");
                        apply_toolkit_appearance(None, None, Some(&spec), None);
                        ctrl_c.persist_and_sync(None, &format!("Set system font to {spec}"));
                    }
                },
            ),
            slider_row(
                "Font Size (pt)",
                "Preferred UI text point size.",
                8.0,
                18.0,
                1.0,
                font_size,
                0,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        let sz = val.round() as u8;
                        let fam = {
                            let mut cfg = ctrl_c.config.borrow_mut();
                            cfg.appearance.font_size = sz;
                            cfg.appearance.font_family.clone()
                        };
                        let spec = format!("{fam} {sz}");
                        apply_toolkit_appearance(None, None, Some(&spec), None);
                        ctrl_c.persist_and_sync(None, &format!("Set font size to {sz} pt"));
                    }
                },
            ),
            slider_row(
                "Font Scaling Factor",
                "Scales interface typography proportionally across GTK apps.",
                0.8,
                1.6,
                0.05,
                font_scale,
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        ctrl_c.config.borrow_mut().appearance.font_scale = val;
                        apply_toolkit_appearance(None, None, None, Some(val));
                        ctrl_c.persist_and_sync(None, &format!("Font scaling set to {val:.2}x"));
                    }
                },
            ),
            dropdown_row(
                "Interface Density",
                "Controls spacing density across NEON DE panels and controls.",
                &["Compact", "Comfortable", "Spacious"],
                density_idx,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |idx| {
                        ctrl_c.config.borrow_mut().appearance.ui_density = match idx {
                            0 => UiDensity::Compact,
                            2 => UiDensity::Spacious,
                            _ => UiDensity::Comfortable,
                        };
                        ctrl_c.persist_and_sync(None, "Updated interface density");
                    }
                },
            ),
        ],
    );
    page.append(&theme_card);

    // ── Login Screen ─────────────────────────────────────────────────────────
    page.append(&section_heading("Login Screen"));
    let login_card = boxed_card();
    login_card.append(&capability_row(
        "Display Manager / Login Screen Theme",
        "Managed externally by the system display manager (GDM/SDDM) and requires root configuration.",
        CapabilityStatus::RequiresElevation,
    ));
    page.append(&login_card);

    scroller
}

fn build_style_preview_card(label: &str, kind: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("style-preview-btn");

    let vbox = GtkBox::new(Orientation::Vertical, 8);
    vbox.set_halign(Align::Center);

    let frame = GtkBox::new(Orientation::Vertical, 0);
    frame.add_css_class("style-preview-frame");
    frame.add_css_class(match kind {
        "light" => "style-preview-light-bg",
        "auto" => "style-preview-auto-bg",
        _ => "style-preview-dark-bg",
    });
    frame.set_halign(Align::Center);
    frame.set_valign(Align::Center);

    let mini = GtkBox::new(Orientation::Vertical, 0);
    mini.add_css_class(if kind == "light" {
        "style-mini-win-light"
    } else {
        "style-mini-win-dark"
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
