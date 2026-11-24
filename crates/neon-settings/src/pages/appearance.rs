//! Appearance settings: Theme (Light/Dark/Auto), Accent Color, ultra-fast async
//! cached Wallpaper Gallery, Glass Blur / Corners / Shadows / High Contrast, and
//! dynamically discovered Cursor, Icon, GTK, and Font settings.

use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, UNIX_EPOCH},
};

use gtk4::{
    Align, Box as GtkBox, Button, ContentFit, DropDown, FlowBox, Label, Orientation, Picture,
    ScrolledWindow, SelectionMode, StringList, gdk, gdk_pixbuf, gio, glib, prelude::*,
};
use neon_config::ThemeMode;

use crate::{
    state::{
        SettingsController, apply_system_cursor, apply_toolkit_appearance,
        build_appearance_request, build_effects_request,
    },
    ui::{
        append_rows, boxed_card, dropdown_row, page_reset_footer, page_shell, row_separator,
        run_async, section_heading, setting_row, slider_row, toggle_row,
    },
};

const ACCENTS: &[(&str, &str, &str)] = &[
    ("Blue", "#0A84FF", "accent-blue"),
    ("Cyan", "#38BDF8", "accent-cyan"),
    ("Violet", "#AF52DE", "accent-violet"),
    ("Green", "#30D158", "accent-emerald"),
    ("Orange", "#FF9F0A", "accent-amber"),
    ("Rose", "#FF375F", "accent-rose"),
    ("Pink", "#EC4899", "accent-pink"),
];

struct DiscoveredWallpaper {
    source_path: String,
    thumb_path: PathBuf,
    title: String,
}

struct InstalledThemes {
    gtk_themes: Vec<String>,
    icon_themes: Vec<String>,
    cursor_themes: Vec<String>,
}

pub fn build_page(ctrl: Rc<SettingsController>) -> ScrolledWindow {
    let (scroller, page) = page_shell();

    // ── Theme & Accent ───────────────────────────────────────────────────────
    page.append(&section_heading("Theme"));
    let style_card = boxed_card();

    let preview_row = GtkBox::new(Orientation::Horizontal, 12);
    preview_row.set_halign(Align::Center);
    preview_row.set_margin_top(12);
    preview_row.set_margin_bottom(10);

    let light_btn = build_style_preview_card("Light", "light");
    let dark_btn = build_style_preview_card("Dark", "dark");
    let auto_btn = build_style_preview_card("Auto", "auto");

    let cur_theme = ctrl.config.borrow().appearance.theme;
    match cur_theme {
        ThemeMode::NeonLight => light_btn.add_css_class("active"),
        ThemeMode::NeonDark => dark_btn.add_css_class("active"),
        ThemeMode::Auto => auto_btn.add_css_class("active"),
    }

    let style_btns = vec![
        (light_btn.clone(), ThemeMode::NeonLight, "Switched to Light"),
        (dark_btn.clone(), ThemeMode::NeonDark, "Switched to Dark"),
        (auto_btn.clone(), ThemeMode::Auto, "Switched to Auto"),
    ];
    for (btn, mode, msg) in &style_btns {
        let ctrl_c = Rc::clone(&ctrl);
        let mode = *mode;
        let msg = *msg;
        let all: Vec<Button> = style_btns.iter().map(|(b, _, _)| b.clone()).collect();
        let me = btn.clone();
        btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().appearance.theme = mode;
            for b in &all {
                b.remove_css_class("active");
            }
            me.add_css_class("active");
            let req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(req), msg);
        });
    }

    preview_row.append(&light_btn);
    preview_row.append(&dark_btn);
    preview_row.append(&auto_btn);
    style_card.append(&preview_row);
    style_card.append(&row_separator());

    // Accent swatches
    let accent_row_ctrl = GtkBox::new(Orientation::Horizontal, 8);
    accent_row_ctrl.set_valign(Align::Center);
    let cur_accent = ctrl.config.borrow().appearance.accent_color.clone();
    let mut swatch_btns: Vec<(Button, &'static str, &'static str)> = Vec::new();
    for &(name, hex, css) in ACCENTS {
        let b = Button::new();
        b.add_css_class("accent-circle");
        b.add_css_class(css);
        b.set_tooltip_text(Some(name));
        if cur_accent.eq_ignore_ascii_case(hex) {
            b.add_css_class("active");
        }
        accent_row_ctrl.append(&b);
        swatch_btns.push((b, name, hex));
    }
    for (btn, name, hex) in &swatch_btns {
        let ctrl_c = Rc::clone(&ctrl);
        let all: Vec<Button> = swatch_btns.iter().map(|(b, _, _)| b.clone()).collect();
        let me = btn.clone();
        let hex_s = (*hex).to_string();
        let name_s = *name;
        btn.connect_clicked(move |_| {
            ctrl_c.config.borrow_mut().appearance.accent_color = hex_s.clone();
            for b in &all {
                b.remove_css_class("active");
            }
            me.add_css_class("active");
            let req = build_appearance_request(&ctrl_c.config.borrow());
            ctrl_c.persist_and_sync(Some(req), &format!("Accent set to {name_s}"));
        });
    }
    style_card.append(&setting_row("Accent Color", "", &accent_row_ctrl));
    page.append(&style_card);

    // ── Wallpaper (100% Async + Persistent Disk Thumbnail Cache) ─────────────
    page.append(&section_heading("Wallpaper"));
    let wp_card = boxed_card();

    let wp_actions = GtkBox::new(Orientation::Horizontal, 8);
    wp_actions.set_valign(Align::Center);
    let auto_wp_btn = Button::with_label("Match Theme");
    auto_wp_btn.add_css_class("settings-secondary-btn");
    let add_wp_btn = Button::with_label("Choose Image…");
    add_wp_btn.add_css_class("settings-secondary-btn");
    wp_actions.append(&auto_wp_btn);
    wp_actions.append(&add_wp_btn);

    wp_card.append(&setting_row(
        "Desktop Wallpaper",
        "Pick a local wallpaper or choose any image.",
        &wp_actions,
    ));
    wp_card.append(&row_separator());

    let wp_flow = FlowBox::new();
    wp_flow.set_selection_mode(SelectionMode::None);
    wp_flow.set_max_children_per_line(4);
    wp_flow.set_min_children_per_line(3);
    wp_flow.set_homogeneous(true);
    wp_flow.set_row_spacing(8);
    wp_flow.set_column_spacing(8);
    wp_flow.set_margin_top(10);
    wp_flow.set_margin_bottom(10);
    wp_flow.set_margin_start(12);
    wp_flow.set_margin_end(12);

    let wp_buttons: Rc<RefCell<Vec<(Button, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let current_wp = ctrl.config.borrow().appearance.wallpaper_path.clone();

    // Stream downscaled 220x136 JPEG thumbnails asynchronously from background worker
    // (cached permanently under ~/.cache/neon-de/wallpaper-thumbs/ so repeat opens take <10ms!)
    let (tx, rx) = mpsc::channel::<DiscoveredWallpaper>();
    std::thread::spawn(move || {
        let cache_dir = wallpaper_thumb_cache_dir();
        let _ = std::fs::create_dir_all(&cache_dir);
        for path in discover_wallpapers().into_iter().take(16) {
            if let Some(thumb_path) = ensure_cached_thumbnail(&path, &cache_dir) {
                let title = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Wallpaper")
                    .to_string();
                let item = DiscoveredWallpaper {
                    source_path: path.to_string_lossy().to_string(),
                    thumb_path,
                    title,
                };
                if tx.send(item).is_err() {
                    break;
                }
            }
        }
    });

    {
        let flow_c = wp_flow.clone();
        let btns_c = Rc::clone(&wp_buttons);
        let ctrl_c = Rc::clone(&ctrl);
        let cur_wp_c = current_wp.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            // Drain up to 3 thumbnails per frame to keep UI at locked 60 FPS
            for _ in 0..3 {
                match rx.try_recv() {
                    Ok(item) => {
                        let pic = if let Ok(tex) = gdk::Texture::from_filename(&item.thumb_path) {
                            Picture::for_paintable(&tex)
                        } else {
                            continue;
                        };
                        pic.set_size_request(118, 72);
                        pic.set_content_fit(ContentFit::Cover);
                        pic.add_css_class("wp-thumb-pic");

                        let btn = Button::new();
                        btn.add_css_class("wp-thumb-btn");
                        btn.set_tooltip_text(Some(&item.title));
                        if cur_wp_c.as_deref() == Some(item.source_path.as_str()) {
                            btn.add_css_class("active");
                        }
                        btn.set_child(Some(&pic));

                        let ctrl_click = Rc::clone(&ctrl_c);
                        let all_click = Rc::clone(&btns_c);
                        let path_click = item.source_path.clone();
                        btn.connect_clicked(move |_| {
                            ctrl_click.config.borrow_mut().appearance.wallpaper_path =
                                Some(path_click.clone());
                            for (b, bp) in all_click.borrow().iter() {
                                if bp == &path_click {
                                    b.add_css_class("active");
                                } else {
                                    b.remove_css_class("active");
                                }
                            }
                            let req = build_appearance_request(&ctrl_click.config.borrow());
                            ctrl_click.persist_and_sync(Some(req), "Wallpaper updated");
                        });

                        flow_c.insert(&btn, -1);
                        btns_c.borrow_mut().push((btn, item.source_path));
                    }
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                }
            }
            glib::ControlFlow::Continue
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
            ctrl_c.persist_and_sync(Some(req), "Wallpaper reset to match theme");
        });
    }

    {
        let ctrl_c = Rc::clone(&ctrl);
        let all = Rc::clone(&wp_buttons);
        add_wp_btn.connect_clicked(move |_| {
            let dialog = gtk4::FileDialog::builder()
                .title("Select Wallpaper")
                .modal(true)
                .build();
            let ctrl_inner = Rc::clone(&ctrl_c);
            let all_inner = Rc::clone(&all);
            dialog.open(Some(&ctrl_c.window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    let p = path.to_string_lossy().to_string();
                    ctrl_inner.config.borrow_mut().appearance.wallpaper_path = Some(p);
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
    page.append(&wp_card);

    // ── Glass, Blur & Surfaces ───────────────────────────────────────────────
    page.append(&section_heading("Glass & Surface"));
    let blur_card = boxed_card();

    let (reduce_blur, blur_strength, corner_radius, shadows_on, high_contrast) = {
        let c = ctrl.config.borrow();
        (
            c.appearance.reduce_blur,
            f64::from(c.effects.blur_strength),
            f64::from(c.effects.corner_radius),
            c.effects.shadows_enabled,
            c.privacy_accessibility.high_contrast,
        )
    };

    append_rows(
        &blur_card,
        vec![
            toggle_row(
                "Reduce Blur",
                "Use cleaner translucent surfaces with lower GPU load.",
                reduce_blur,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().appearance.reduce_blur = active;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated blur mode");
                    }
                },
            ),
            slider_row(
                "Blur Intensity",
                "Controls glass blur strength across panels and App Drawer.",
                0.0,
                2.0,
                0.05,
                blur_strength,
                2,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |val| {
                        let v = val as f32;
                        {
                            let mut c = ctrl_c.config.borrow_mut();
                            c.effects.blur_strength = v;
                            c.effects.blur_enabled = v > 0.05;
                        }
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), &format!("Blur intensity: {v:.2}x"));
                    }
                },
            ),
            slider_row(
                "Window Corner Radius",
                "Rounded corner radius in pixels.",
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
                        ctrl_c.persist_and_sync(Some(req), &format!("Corner radius: {val:.0}px"));
                    }
                },
            ),
            toggle_row(
                "Window Shadows",
                "Soft elevation shadows beneath floating windows.",
                shadows_on,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().effects.shadows_enabled = active;
                        let req = build_effects_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated window shadows");
                    }
                },
            ),
            toggle_row(
                "High Contrast",
                "Opaque panel and dock surfaces with stronger borders.",
                high_contrast,
                {
                    let ctrl_c = Rc::clone(&ctrl);
                    move |active| {
                        ctrl_c.config.borrow_mut().privacy_accessibility.high_contrast = active;
                        let req = build_appearance_request(&ctrl_c.config.borrow());
                        ctrl_c.persist_and_sync(Some(req), "Updated high contrast mode");
                    }
                },
            ),
        ],
    );
    page.append(&blur_card);

    // ── Cursor, Icons, GTK Theme & Fonts (Themes Discovered Asynchronously) ──
    page.append(&section_heading("Cursor, Icons & Typography"));
    let theme_card = boxed_card();
    let theme_rows_holder = GtkBox::new(Orientation::Vertical, 0);
    theme_card.append(&theme_rows_holder);
    page.append(&theme_card);

    {
        let holder = theme_rows_holder.clone();
        let ctrl_done = Rc::clone(&ctrl);
        run_async(discover_installed_themes, move |themes| {
            populate_toolkit_card(&holder, ctrl_done, themes);
        });
    }

    page.append(&page_reset_footer("appearance", "Appearance", &ctrl));
    scroller
}

fn populate_toolkit_card(
    holder: &GtkBox,
    ctrl: Rc<SettingsController>,
    themes: InstalledThemes,
) {
    let (cur_cursor, cur_size, cur_icon, cur_gtk, cur_font, cur_font_sz, cur_font_scale) = {
        let a = &ctrl.config.borrow().appearance;
        (
            a.cursor_theme.clone(),
            a.cursor_size,
            a.icon_theme.clone(),
            a.gtk_theme.clone(),
            a.font_family.clone(),
            f64::from(a.font_size),
            a.font_scale,
        )
    };

    let mut rows = Vec::new();

    // Cursor Theme
    let mut cursor_list = themes.cursor_themes;
    if !cursor_list.iter().any(|c| c == &cur_cursor) {
        cursor_list.insert(0, cur_cursor.clone());
    }
    let cursor_refs: Vec<&str> = cursor_list.iter().map(String::as_str).collect();
    let cursor_idx = cursor_list
        .iter()
        .position(|c| c == &cur_cursor)
        .unwrap_or(0) as u32;
    let cursor_dd = DropDown::new(Some(StringList::new(&cursor_refs)), gtk4::Expression::NONE);
    cursor_dd.set_valign(Align::Center);
    cursor_dd.set_selected(cursor_idx);
    {
        let ctrl_c = Rc::clone(&ctrl);
        let list = cursor_list.clone();
        cursor_dd.connect_selected_notify(move |d| {
            if let Some(name) = list.get(d.selected() as usize) {
                ctrl_c.config.borrow_mut().appearance.cursor_theme = name.clone();
                let sz = ctrl_c.config.borrow().appearance.cursor_size;
                apply_system_cursor(name, sz);
                let req = build_appearance_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(Some(req), &format!("Cursor theme: {name}"));
            }
        });
    }
    rows.push(setting_row("Cursor Theme", "", &cursor_dd));

    // Pointer Size
    let cursor_sizes: [u32; 5] = [16, 18, 20, 24, 32];
    let cur_sz_idx = cursor_sizes
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| (**s as i32 - cur_size as i32).abs())
        .map(|(i, _)| i as u32)
        .unwrap_or(2);
    rows.push(dropdown_row(
        "Pointer Size",
        "",
        &["16 px", "18 px", "20 px (macOS)", "24 px", "32 px"],
        cur_sz_idx,
        {
            let ctrl_c = Rc::clone(&ctrl);
            move |idx| {
                let sz = cursor_sizes.get(idx as usize).copied().unwrap_or(20);
                ctrl_c.config.borrow_mut().appearance.cursor_size = sz;
                let theme = ctrl_c.config.borrow().appearance.cursor_theme.clone();
                apply_system_cursor(&theme, sz);
                let req = build_appearance_request(&ctrl_c.config.borrow());
                ctrl_c.persist_and_sync(Some(req), &format!("Pointer size: {sz}px"));
            }
        },
    ));

    // Icon Theme
    let mut icon_list = themes.icon_themes;
    if !icon_list.iter().any(|i| i == &cur_icon) {
        icon_list.insert(0, cur_icon.clone());
    }
    let icon_refs: Vec<&str> = icon_list.iter().map(String::as_str).collect();
    let icon_idx = icon_list.iter().position(|i| i == &cur_icon).unwrap_or(0) as u32;
    let icon_dd = DropDown::new(Some(StringList::new(&icon_refs)), gtk4::Expression::NONE);
    icon_dd.set_valign(Align::Center);
    icon_dd.set_selected(icon_idx);
    {
        let ctrl_c = Rc::clone(&ctrl);
        let list = icon_list.clone();
        icon_dd.connect_selected_notify(move |d| {
            if let Some(name) = list.get(d.selected() as usize) {
                ctrl_c.config.borrow_mut().appearance.icon_theme = name.clone();
                apply_toolkit_appearance(None, Some(name), None, None);
                ctrl_c.persist_and_sync(None, &format!("Icon theme: {name}"));
            }
        });
    }
    rows.push(setting_row("Icon Theme", "", &icon_dd));

    // GTK Theme
    if !themes.gtk_themes.is_empty() {
        let mut gtk_list = themes.gtk_themes;
        if !gtk_list.iter().any(|g| g == &cur_gtk) {
            gtk_list.insert(0, cur_gtk.clone());
        }
        let gtk_refs: Vec<&str> = gtk_list.iter().map(String::as_str).collect();
        let gtk_idx = gtk_list.iter().position(|g| g == &cur_gtk).unwrap_or(0) as u32;
        let gtk_dd = DropDown::new(Some(StringList::new(&gtk_refs)), gtk4::Expression::NONE);
        gtk_dd.set_valign(Align::Center);
        gtk_dd.set_selected(gtk_idx);
        {
            let ctrl_c = Rc::clone(&ctrl);
            let list = gtk_list.clone();
            gtk_dd.connect_selected_notify(move |d| {
                if let Some(name) = list.get(d.selected() as usize) {
                    ctrl_c.config.borrow_mut().appearance.gtk_theme = name.clone();
                    apply_toolkit_appearance(Some(name), None, None, None);
                    ctrl_c.persist_and_sync(None, &format!("GTK theme: {name}"));
                }
            });
        }
        rows.push(setting_row("GTK Theme", "", &gtk_dd));
    }

    // System Font, Size & Scale
    let font_families = [
        "Inter",
        "Adwaita Sans",
        "Ubuntu",
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
    ];
    let font_idx = font_families
        .iter()
        .position(|f| *f == cur_font.as_str())
        .unwrap_or(0) as u32;

    rows.push(dropdown_row(
        "System Font",
        "",
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
                ctrl_c.persist_and_sync(None, &format!("Font: {spec}"));
            }
        },
    ));

    rows.push(slider_row(
        "Font Size",
        "Point size for GTK applications.",
        8.0,
        18.0,
        1.0,
        cur_font_sz,
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
                ctrl_c.persist_and_sync(None, &format!("Font size: {sz} pt"));
            }
        },
    ));

    rows.push(slider_row(
        "Text Scaling",
        "Scales text across GTK applications.",
        0.8,
        1.6,
        0.05,
        cur_font_scale,
        2,
        {
            let ctrl_c = Rc::clone(&ctrl);
            move |val| {
                ctrl_c.config.borrow_mut().appearance.font_scale = val;
                apply_toolkit_appearance(None, None, None, Some(val));
                ctrl_c.persist_and_sync(None, &format!("Text scaling: {val:.2}x"));
            }
        },
    ));

    append_rows(holder, rows);
}

fn build_style_preview_card(label: &str, kind: &str) -> Button {
    let btn = Button::new();
    btn.add_css_class("style-preview-btn");

    let vbox = GtkBox::new(Orientation::Vertical, 6);
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

fn wallpaper_thumb_cache_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("neon-de/wallpaper-thumbs");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/neon-de/wallpaper-thumbs");
    }
    PathBuf::from("/tmp/neon-de-wallpaper-thumbs")
}

fn ensure_cached_thumbnail(src: &Path, cache_dir: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(src).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut hasher = DefaultHasher::new();
    src.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    mtime.hash(&mut hasher);
    let key = format!("{:016x}.jpg", hasher.finish());
    let dest = cache_dir.join(key);

    if dest.exists() {
        return Some(dest);
    }

    let pb = gdk_pixbuf::Pixbuf::from_file_at_scale(src, 220, 136, true).ok()?;
    let tmp = dest.with_extension("jpg.tmp");
    pb.savev(&tmp, "jpeg", &[("quality", "80")]).ok()?;
    let _ = std::fs::rename(&tmp, &dest);
    Some(dest)
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
            if path.is_file()
                && let Some(ext) = path.extension().and_then(|s| s.to_str())
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

fn discover_installed_themes() -> InstalledThemes {
    let mut icon_dirs = vec![PathBuf::from("/usr/share/icons")];
    let mut gtk_dirs = vec![PathBuf::from("/usr/share/themes")];
    if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        icon_dirs.push(h.join(".local/share/icons"));
        icon_dirs.push(h.join(".icons"));
        gtk_dirs.push(h.join(".local/share/themes"));
        gtk_dirs.push(h.join(".themes"));
    }

    let mut cursors = Vec::new();
    let mut icons = Vec::new();
    for dir in icon_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if matches!(name, "default" | "hicolor" | "locolor") {
                continue;
            }
            if p.join("cursors").is_dir() && !cursors.iter().any(|x| x == name) {
                cursors.push(name.to_string());
            }
            if p.join("index.theme").is_file()
                && !p.join("cursors").is_dir()
                && !icons.iter().any(|x| x == name)
            {
                icons.push(name.to_string());
            }
        }
    }
    cursors.sort();
    icons.sort();

    let mut gtk_themes = Vec::new();
    for dir in gtk_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if matches!(name, "Default" | "Emacs") {
                continue;
            }
            if (p.join("gtk-3.0").is_dir() || p.join("gtk-4.0").is_dir() || p.join("index.theme").is_file())
                && !gtk_themes.iter().any(|x| x == name)
            {
                gtk_themes.push(name.to_string());
            }
        }
    }
    gtk_themes.sort();

    InstalledThemes {
        gtk_themes,
        icon_themes: icons,
        cursor_themes: cursors,
    }
}
