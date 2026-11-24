//! NEON DE Desktop Wallpaper Layer (`zwlr_layer_shell_v1` `Layer::Background`).
//!
//! Renders high-resolution desktop wallpapers scaled with `ContentFit::Cover` underneath all
//! desktop windows, top bar, and floating dock, and switches live when theme or wallpaper changes.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

use gtk4::{
    Application, ApplicationWindow, ContentFit, GestureClick, Picture, gdk, gdk_pixbuf, gio, glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::AppearanceInfo;
use tracing::info;

use crate::context_menu::ContextMenuOverlay;

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

/// Live background wallpaper surface managed by `neon-shell`.
pub struct WallpaperLayer {
    _window: ApplicationWindow,
    picture: Picture,
    current_path: RefCell<Option<PathBuf>>,
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

        let picture = Picture::new();
        picture.set_content_fit(ContentFit::Cover);
        picture.set_can_shrink(true);
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.add_css_class("neon-wallpaper-picture");

        // Right-clicking anywhere on the desktop wallpaper opens the OS Desktop Context Menu.
        let right_click = GestureClick::new();
        right_click.set_button(3);
        right_click.connect_pressed(move |_, _, x, y| {
            context_menu.open_desktop_menu(x.round() as i32, y.round() as i32);
        });
        window.add_controller(right_click);

        window.set_child(Some(&picture));
        window.set_cursor_from_name(Some("default"));
        window.present();

        let layer = Self {
            _window: window,
            picture,
            current_path: RefCell::new(None),
        };
        if let Some(default_wp) = resolve_wallpaper_path(None, false) {
            layer.apply_path(&default_wp);
        }
        layer
    }

    /// Updates the rendered wallpaper if theme or configured wallpaper path changed.
    pub fn update(&self, appearance: &AppearanceInfo) {
        let is_light = appearance.theme == "neon_light" || appearance.theme == "light";
        let resolved = resolve_wallpaper_path(appearance.wallpaper.as_deref(), is_light);
        if let Some(path) = resolved
            && self.current_path.borrow().as_ref() != Some(&path)
        {
            self.apply_path(&path);
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
