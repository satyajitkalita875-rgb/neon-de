//! NEON DE Desktop Wallpaper Layer (`zwlr_layer_shell_v1` `Layer::Background`).
//!
//! Renders high-resolution desktop wallpapers scaled with `ContentFit::Cover` underneath all
//! desktop windows, top bar, and floating dock, and switches live when theme or wallpaper changes.

use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::UNIX_EPOCH,
};

use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, ContentFit, GestureClick, Grid,
    Label, Orientation, Overlay, Picture, gdk, gdk_pixbuf, gio, glib, prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::AppearanceInfo;
use tracing::info;

use crate::{
    context_menu::ContextMenuOverlay,
    desktop_apps::{load_app_icon, parse_desktop_file},
    file_ops::{desktop_dir, icon_for_path, open_path},
};

/// Decodes an image file scaled down to fit within `max_w × max_h` before GPU upload,
/// avoiding multi-hundred-megabyte uncompressed 6K textures and heavy blur bandwidth.
pub fn load_scaled_texture(path: &Path, max_w: i32, max_h: i32) -> Option<gdk::Texture> {
    let pixbuf = gdk_pixbuf::Pixbuf::from_file_at_scale(path, max_w, max_h, true).ok()?;
    Some(gdk::Texture::for_pixbuf(&pixbuf))
}

/// Pre-blurs a wallpaper once on the CPU using a fast 3-pass separable box blur (equivalent to
/// a smooth Gaussian blur) with saturation & brightness grading, uploading a static GPU texture.
///
/// Because GTK renders the result as a plain texture blit with **zero** live `filter: blur(...)`
/// shader passes per frame, App Drawer animations and page scrolling stay locked at 60 FPS.
pub fn load_blurred_texture(
    path: &Path,
    max_w: i32,
    max_h: i32,
    radius: usize,
    brightness: f32,
    saturation: f32,
) -> Option<gdk::Texture> {
    let raw_pb = gdk_pixbuf::Pixbuf::from_file_at_scale(path, max_w, max_h, true).ok()?;
    let pb = if raw_pb.has_alpha() {
        raw_pb
    } else {
        raw_pb.add_alpha(false, 0, 0, 0).ok()?
    };
    let w = pb.width() as usize;
    let h = pb.height() as usize;
    if w == 0 || h == 0 {
        return None;
    }
    let stride = pb.rowstride() as usize;
    let src_pixels = unsafe { pb.pixels() };
    let mut buf = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_row = &src_pixels[y * stride..y * stride + w * 4];
        let dst_row = &mut buf[y * w * 4..(y + 1) * w * 4];
        dst_row.copy_from_slice(src_row);
    }

    if radius > 0 {
        let mut tmp = vec![0u8; w * h * 4];
        for _ in 0..3 {
            box_blur_h(&buf, &mut tmp, w, h, radius);
            box_blur_v(&tmp, &mut buf, w, h, radius);
        }
    }

    // Apply macOS-like saturation + brightness grading in-place.
    for px in buf.chunks_exact_mut(4) {
        let r = f32::from(px[0]);
        let g = f32::from(px[1]);
        let b = f32::from(px[2]);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let nr = ((luma + (r - luma) * saturation) * brightness).clamp(0.0, 255.0);
        let ng = ((luma + (g - luma) * saturation) * brightness).clamp(0.0, 255.0);
        let nb = ((luma + (b - luma) * saturation) * brightness).clamp(0.0, 255.0);
        px[0] = nr as u8;
        px[1] = ng as u8;
        px[2] = nb as u8;
        px[3] = 255;
    }

    let bytes = glib::Bytes::from_owned(buf);
    let tex = gdk::MemoryTexture::new(
        w as i32,
        h as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        w * 4,
    );
    Some(tex.upcast())
}

fn box_blur_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let win = (2 * r + 1) as u32;
    let wm1 = (w - 1) as isize;
    for y in 0..h {
        let row = y * w * 4;
        let mut sr = 0u32;
        let mut sg = 0u32;
        let mut sb = 0u32;
        for dx in -(r as isize)..=(r as isize) {
            let x = dx.clamp(0, wm1) as usize;
            let idx = row + x * 4;
            sr += u32::from(src[idx]);
            sg += u32::from(src[idx + 1]);
            sb += u32::from(src[idx + 2]);
        }
        for x in 0..w {
            let out = row + x * 4;
            dst[out] = (sr / win) as u8;
            dst[out + 1] = (sg / win) as u8;
            dst[out + 2] = (sb / win) as u8;
            dst[out + 3] = 255;

            let rem_x = (x as isize - r as isize).clamp(0, wm1) as usize;
            let add_x = (x as isize + r as isize + 1).clamp(0, wm1) as usize;
            let rem_i = row + rem_x * 4;
            let add_i = row + add_x * 4;
            sr = sr + u32::from(src[add_i]) - u32::from(src[rem_i]);
            sg = sg + u32::from(src[add_i + 1]) - u32::from(src[rem_i + 1]);
            sb = sb + u32::from(src[add_i + 2]) - u32::from(src[rem_i + 2]);
        }
    }
}

fn box_blur_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let win = (2 * r + 1) as u32;
    let hm1 = (h - 1) as isize;
    for x in 0..w {
        let mut sr = 0u32;
        let mut sg = 0u32;
        let mut sb = 0u32;
        for dy in -(r as isize)..=(r as isize) {
            let y = dy.clamp(0, hm1) as usize;
            let idx = (y * w + x) * 4;
            sr += u32::from(src[idx]);
            sg += u32::from(src[idx + 1]);
            sb += u32::from(src[idx + 2]);
        }
        for y in 0..h {
            let out = (y * w + x) * 4;
            dst[out] = (sr / win) as u8;
            dst[out + 1] = (sg / win) as u8;
            dst[out + 2] = (sb / win) as u8;
            dst[out + 3] = 255;

            let rem_y = (y as isize - r as isize).clamp(0, hm1) as usize;
            let add_y = (y as isize + r as isize + 1).clamp(0, hm1) as usize;
            let rem_i = (rem_y * w + x) * 4;
            let add_i = (add_y * w + x) * 4;
            sr = sr + u32::from(src[add_i]) - u32::from(src[rem_i]);
            sg = sg + u32::from(src[add_i + 1]) - u32::from(src[rem_i + 1]);
            sb = sb + u32::from(src[add_i + 2]) - u32::from(src[rem_i + 2]);
        }
    }
}

/// Live background wallpaper + interactive Desktop Icons (`~/Desktop`) surface managed by `neon-shell`.
#[derive(Clone)]
pub struct WallpaperLayer {
    window: ApplicationWindow,
    picture: Picture,
    icons_grid: Grid,
    current_path: Rc<RefCell<Option<PathBuf>>>,
    show_icons: Rc<Cell<bool>>,
    sort_by: Rc<RefCell<String>>,
    last_dir_sig: Rc<Cell<u64>>,
    selected_tile: Rc<RefCell<Option<Button>>>,
    context_menu: ContextMenuOverlay,
}

impl WallpaperLayer {
    pub fn build(app: &Application, context_menu: ContextMenuOverlay) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("NEON DE Wallpaper")
            .decorated(false)
            .build();
        window.add_css_class("neon-wallpaper-window");

        if gtk4_layer_shell::is_supported() {
            window.init_layer_shell();
            window.set_namespace(Some("neon-wallpaper"));
            window.set_layer(Layer::Background);
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::None);
        }

        let overlay = Overlay::new();

        let picture = Picture::new();
        picture.set_content_fit(ContentFit::Cover);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.add_css_class("neon-wallpaper-picture");
        overlay.set_child(Some(&picture));

        let icons_grid = Grid::new();
        icons_grid.add_css_class("neon-desktop-icons-layer");
        icons_grid.set_row_spacing(12);
        icons_grid.set_column_spacing(14);
        icons_grid.set_halign(Align::Start);
        icons_grid.set_valign(Align::Start);
        icons_grid.set_visible(true);
        overlay.add_overlay(&icons_grid);

        let selected_tile: Rc<RefCell<Option<Button>>> = Rc::new(RefCell::new(None));

        // Left-clicking empty wallpaper clears Desktop icon selection.
        {
            let clear_click = GestureClick::new();
            clear_click.set_button(1);
            let sel = Rc::clone(&selected_tile);
            clear_click.connect_pressed(move |_, _, _, _| {
                if let Some(prev) = sel.borrow_mut().take() {
                    prev.remove_css_class("selected");
                }
            });
            picture.add_controller(clear_click);
        }

        // Right-clicking empty desktop canvas opens the macOS Desktop Context Menu.
        // Right-clicking an icon tile claims the gesture first and opens the File/Folder menu.
        let right_click = GestureClick::new();
        right_click.set_button(3);
        {
            let cm = context_menu.clone();
            right_click.connect_pressed(move |_, _, x, y| {
                cm.open_desktop_menu(x.round() as i32, y.round() as i32);
            });
        }
        window.add_controller(right_click);

        window.set_child(Some(&overlay));
        window.set_cursor_from_name(Some("default"));
        window.present();

        let layer = Self {
            window,
            picture,
            icons_grid,
            current_path: Rc::new(RefCell::new(None)),
            show_icons: Rc::new(Cell::new(true)),
            sort_by: Rc::new(RefCell::new("name".to_string())),
            last_dir_sig: Rc::new(Cell::new(0)),
            selected_tile,
            context_menu,
        };
        if let Some(default_wp) = resolve_wallpaper_path(None, false) {
            layer.apply_path(&default_wp);
        }
        layer.refresh_icons();
        layer
    }

    /// Updates wallpaper, desktop icon visibility, and sort mode when appearance changes.
    pub fn update(&self, appearance: &AppearanceInfo) {
        let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
        let resolved = resolve_wallpaper_path(appearance.wallpaper.as_deref(), is_light);
        if let Some(path) = resolved
            && self.current_path.borrow().as_ref() != Some(&path)
        {
            self.apply_path(&path);
        }

        let prev_show = self.show_icons.get();
        let sort_changed = *self.sort_by.borrow() != appearance.desktop_sort_by;
        if sort_changed {
            *self.sort_by.borrow_mut() = appearance.desktop_sort_by.clone();
        }
        if prev_show != appearance.show_desktop_icons {
            self.show_icons.set(appearance.show_desktop_icons);
            self.icons_grid.set_visible(appearance.show_desktop_icons);
            if appearance.show_desktop_icons {
                self.refresh_icons();
                return;
            }
        }
        if appearance.show_desktop_icons {
            let sig = desktop_dir_signature(&desktop_dir());
            if sort_changed || sig != self.last_dir_sig.get() {
                self.refresh_icons();
            }
        }
    }

    /// Enables desktop icon visibility and rescans `~/Desktop` immediately (e.g. after New Folder).
    pub fn force_show_and_refresh(&self) {
        self.show_icons.set(true);
        self.icons_grid.set_visible(true);
        self.refresh_icons();
    }

    /// Lightweight periodic watcher so files/folders created externally appear automatically.
    pub fn poll_dir_if_visible(&self) {
        if !self.show_icons.get() {
            return;
        }
        let sig = desktop_dir_signature(&desktop_dir());
        if sig != self.last_dir_sig.get() {
            self.refresh_icons();
        }
    }

    /// Rescans `~/Desktop` and rebuilds the interactive Desktop icons grid immediately.
    pub fn refresh_icons(&self) {
        let dir = desktop_dir();
        self.last_dir_sig.set(desktop_dir_signature(&dir));
        self.selected_tile.borrow_mut().take();

        while let Some(child) = self.icons_grid.first_child() {
            self.icons_grid.remove(&child);
        }

        if !self.show_icons.get() {
            self.icons_grid.set_visible(false);
            return;
        }
        self.icons_grid.set_visible(true);

        let Ok(rd) = fs::read_dir(&dir) else {
            return;
        };

        struct DesktopEntry {
            path: PathBuf,
            display_name: String,
            icon_spec: String,
            is_dir: bool,
            size: u64,
            modified: u64,
            ext: String,
        }

        let mut entries: Vec<DesktopEntry> = rd
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let fname = e.file_name().to_string_lossy().into_owned();
                if fname.starts_with('.') {
                    return None;
                }
                let meta = fs::metadata(&path).ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();

                let (display_name, icon_spec) = if !is_dir && ext == "desktop" {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                        && let Some(app) = parse_desktop_file(&path, stem)
                    {
                        (
                            app.name,
                            app.icon.unwrap_or_else(|| "application-x-executable".into()),
                        )
                    } else {
                        (fname.clone(), icon_for_path(&path))
                    }
                } else {
                    (fname.clone(), icon_for_path(&path))
                };

                Some(DesktopEntry {
                    path,
                    display_name,
                    icon_spec,
                    is_dir,
                    size,
                    modified,
                    ext,
                })
            })
            .collect();

        let sort_mode = self.sort_by.borrow().clone();
        entries.sort_by(|a, b| {
            // Folders first unless sorting purely by date modified
            if sort_mode != "modified" && a.is_dir != b.is_dir {
                return b.is_dir.cmp(&a.is_dir);
            }
            match sort_mode.as_str() {
                "modified" => b
                    .modified
                    .cmp(&a.modified)
                    .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase())),
                "size" => b
                    .size
                    .cmp(&a.size)
                    .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase())),
                "type" => a
                    .ext
                    .cmp(&b.ext)
                    .then_with(|| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase())),
                _ => a
                    .display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase()),
            }
        });

        const MAX_ROWS_PER_COL: usize = 6;
        for (idx, item) in entries.into_iter().take(42).enumerate() {
            let col = (idx / MAX_ROWS_PER_COL) as i32;
            let row = (idx % MAX_ROWS_PER_COL) as i32;

            let tile = Button::new();
            tile.add_css_class("neon-desktop-icon-tile");
            tile.set_tooltip_text(Some(&item.display_name));

            let vbox = GtkBox::new(Orientation::Vertical, 6);
            vbox.set_halign(Align::Center);
            vbox.set_valign(Align::Center);

            let icon = load_app_icon(Some(&item.icon_spec), &item.display_name, 48);
            icon.set_halign(Align::Center);
            vbox.append(&icon);

            let lbl = Label::new(Some(&item.display_name));
            lbl.add_css_class("neon-desktop-icon-label");
            lbl.set_halign(Align::Center);
            lbl.set_max_width_chars(12);
            lbl.set_lines(2);
            lbl.set_wrap(true);
            lbl.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
            lbl.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            lbl.set_justify(gtk4::Justification::Center);
            vbox.append(&lbl);

            tile.set_child(Some(&vbox));

            // Single-click selects; double-click launches/opens.
            {
                let click = GestureClick::new();
                click.set_button(1);
                let sel = Rc::clone(&self.selected_tile);
                let tile_c = tile.clone();
                let p = item.path.clone();
                click.connect_pressed(move |gesture, n_press, _, _| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    if let Some(prev) = sel.borrow_mut().replace(tile_c.clone()) {
                        prev.remove_css_class("selected");
                    }
                    tile_c.add_css_class("selected");
                    if n_press >= 2 {
                        open_path(&p);
                    }
                });
                tile.add_controller(click);
            }

            // Right-click selects tile & opens the Files & Folders macOS context menu!
            {
                let rc = GestureClick::new();
                rc.set_button(3);
                let cm = self.context_menu.clone();
                let sel = Rc::clone(&self.selected_tile);
                let win_c = self.window.clone();
                let tile_c = tile.clone();
                let p = item.path;
                rc.connect_pressed(move |gesture, _, lx, ly| {
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    if let Some(prev) = sel.borrow_mut().replace(tile_c.clone()) {
                        prev.remove_css_class("selected");
                    }
                    tile_c.add_css_class("selected");
                    let (sx, sy) = tile_c
                        .compute_bounds(&win_c)
                        .map(|b| ((b.x() as f64 + lx).round() as i32, (b.y() as f64 + ly).round() as i32))
                        .unwrap_or((lx.round() as i32 + 40, ly.round() as i32 + 60));
                    cm.open_file_menu(sx, sy, p.clone());
                });
                tile.add_controller(rc);
            }

            self.icons_grid.attach(&tile, col, row, 1, 1);
        }
    }

    fn apply_path(&self, path: &Path) {
        if let Some(tex) = load_scaled_texture(path, 1920, 1080) {
            self.picture.set_paintable(Some(&tex));
        } else {
            let file = gio::File::for_path(path);
            self.picture.set_file(Some(&file));
        }
        *self.current_path.borrow_mut() = Some(path.to_path_buf());
        info!(wallpaper = %path.display(), "Applied NEON DE desktop wallpaper");
    }
}

fn desktop_dir_signature(dir: &Path) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let Ok(rd) = fs::read_dir(dir) else {
        return hash;
    };
    let mut items: Vec<(String, u64, u64)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let m = fs::metadata(e.path()).ok();
            let sz = m.as_ref().map(|x| x.len()).unwrap_or(0);
            let mt = m
                .as_ref()
                .and_then(|x| x.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Some((name, sz, mt))
        })
        .collect();
    items.sort();
    for (name, sz, mt) in items {
        for b in name.as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for b in sz.to_le_bytes().into_iter().chain(mt.to_le_bytes()) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

pub fn resolve_wallpaper_path(explicit: Option<&str>, is_light: bool) -> Option<PathBuf> {
    if let Some(raw) = explicit
        && !raw.trim().is_empty()
    {
        let p = PathBuf::from(raw);
        if p.exists() {
            return Some(p);
        }
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(h) = home {
        let wp_dir = h.join("Pictures/Wallpapers");
        if is_light {
            for name in [
                "macOS-Sequoia-Light.jpg",
                "macOS-Sonoma-Light.jpg",
                "macOS-Ventura-Light.jpg",
                "macOS-Tahoe-Light.png",
                "macOS-Big-Sur-Day.jpg",
            ] {
                candidates.push(wp_dir.join(name));
            }
        } else {
            for name in [
                "macOS-Sequoia-Dark.jpg",
                "macOS-Sonoma-Dark.jpg",
                "macOS-Monterey-Dark.jpg",
                "macOS-Ventura-Dark.jpg",
                "macOS-Tahoe-Dark.png",
                "macOS-Big-Sur-Night.jpg",
            ] {
                candidates.push(wp_dir.join(name));
            }
        }
        candidates.push(h.join("Pictures/wallpaper.png"));
    }

    if is_light {
        candidates.push(PathBuf::from(
            "/usr/share/backgrounds/Resolute_Raccoon_Wallpaper_Light_3840x2160.png",
        ));
    } else {
        candidates.push(PathBuf::from(
            "/usr/share/backgrounds/Resolute_Raccoon_Wallpaper_Dimmed_3840x2160.png",
        ));
    }

    candidates.into_iter().find(|p| p.exists())
}
