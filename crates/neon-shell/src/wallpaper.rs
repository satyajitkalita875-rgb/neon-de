//! NEON DE Desktop Wallpaper Layer (`zwlr_layer_shell_v1` `Layer::Background`).
//!
//! Renders high-resolution desktop wallpapers scaled with `ContentFit::Cover` underneath all
//! desktop windows, top bar, and floating dock, and switches live when theme or wallpaper changes.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

use gtk4::{Application, ApplicationWindow, ContentFit, Picture, gio, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use neon_ipc::AppearanceInfo;
use tracing::info;

/// Live background wallpaper surface managed by `neon-shell`.
pub struct WallpaperLayer {
    _window: ApplicationWindow,
    picture: Picture,
    current_path: RefCell<Option<PathBuf>>,
}

impl WallpaperLayer {
    pub fn build(app: &Application) -> Self {
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

        window.set_child(Some(&picture));
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
        let file = gio::File::for_path(path);
        self.picture.set_file(Some(&file));
        *self.current_path.borrow_mut() = Some(path.to_path_buf());
        info!(wallpaper = %path.display(), "Applied NEON DE desktop wallpaper");
    }
}

fn resolve_wallpaper_path(explicit: Option<&str>, is_light: bool) -> Option<PathBuf> {
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
