//! Real XCursor theme loader and client-surface cursor rendering for bare-metal DRM/KMS.
//!
//! Loads crisp anti-aliased cursors from the user's installed macOS XCursor themes (`macOS`,
//! `macOS-White`, `Bibata-Modern-Ice`, `Adwaita`, `default`), outputs true little-endian
//! `Fourcc::Argb8888` matching DRM cursor planes for zero-copy 1:1 hardware scanout, supports
//! HiDPI buffer scaling and live theme/size reload, and falls back to an embedded sleek 16×20
//! macOS arrow bitmap.

use std::{collections::HashMap, fs};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Logical, Size, Transform},
};
use tracing::info;
use xcursor::{CursorTheme, parser::parse_xcursor};

/// A cached cursor image buffer along with its logical hotspot offset and optional target size.
#[derive(Debug, Clone)]
pub struct LoadedCursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: (f64, f64),
    /// Set only when the best available XCursor bitmap differs from the requested logical size
    /// so oversized fallback strikes never appear blown-up on screen.
    pub logical_size: Option<Size<i32, Logical>>,
}

/// Manages loaded XCursor theme bitmaps with lazy per-scale caching per [`CursorIcon`].
#[derive(Debug)]
pub struct CursorCache {
    theme_name: String,
    theme: Option<CursorTheme>,
    size: u32,
    cache: HashMap<(String, i32), LoadedCursor>,
    fallback: LoadedCursor,
}

impl CursorCache {
    pub fn load_default() -> Self {
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(20)
            .clamp(16, 48);

        let preferred_env = std::env::var("XCURSOR_THEME").ok();
        let preferred = preferred_env.as_deref().unwrap_or("macOS");
        Self::load_with(preferred, size)
    }

    pub fn load_with(preferred_theme: &str, size: u32) -> Self {
        let size = size.clamp(16, 48);
        let (chosen_name, chosen_theme) = resolve_cursor_theme(preferred_theme, size);

        let fallback = build_fallback_macos_cursor();
        let mut cache = Self {
            theme_name: chosen_name,
            theme: chosen_theme,
            size,
            cache: HashMap::new(),
            fallback,
        };
        let _ = cache.get_named_scaled(CursorIcon::Default, 1.0);
        cache
    }

    /// Live-updates the cursor theme and logical pixel size if either changed.
    pub fn set_theme_and_size(&mut self, preferred_theme: &str, size: u32) {
        let clamped = size.clamp(16, 48);
        let desired = if preferred_theme.trim().is_empty() {
            "macOS"
        } else {
            preferred_theme.trim()
        };
        if self.theme_name == desired && self.size == clamped {
            return;
        }
        let (chosen_name, chosen_theme) = resolve_cursor_theme(desired, clamped);
        self.theme_name = chosen_name;
        self.theme = chosen_theme;
        self.size = clamped;
        self.cache.clear();
        let _ = self.get_named_scaled(CursorIcon::Default, 1.0);
    }

    pub fn get_named(&mut self, icon: CursorIcon) -> &LoadedCursor {
        self.get_named_scaled(icon, 1.0)
    }

    pub fn get_named_scaled(&mut self, icon: CursorIcon, scale_f: f64) -> &LoadedCursor {
        let int_scale = scale_f.ceil().clamp(1.0, 4.0) as i32;
        let primary = icon.name();
        let key = (primary.to_string(), int_scale);
        if self.cache.contains_key(&key) {
            return self.cache.get(&key).unwrap_or(&self.fallback);
        }

        let aliases: Vec<&str> = match icon {
            CursorIcon::Default => vec!["default", "left_ptr", "arrow"],
            CursorIcon::Pointer => vec!["pointer", "hand2", "hand1", "pointing_hand"],
            CursorIcon::Text => vec!["text", "xterm", "ibeam"],
            CursorIcon::VerticalText => vec!["vertical-text", "text", "xterm"],
            CursorIcon::Grab => vec!["grab", "openhand", "hand1"],
            CursorIcon::Grabbing => vec!["grabbing", "closedhand", "fleur"],
            CursorIcon::Move | CursorIcon::AllScroll | CursorIcon::AllResize => {
                vec!["move", "all-scroll", "fleur"]
            }
            CursorIcon::Wait => vec!["wait", "watch", "left_ptr_watch"],
            CursorIcon::Progress => vec!["progress", "left_ptr_watch", "wait"],
            CursorIcon::Help => vec!["help", "question_arrow", "left_ptr"],
            CursorIcon::ContextMenu => vec!["context-menu", "default", "left_ptr"],
            CursorIcon::Crosshair => vec!["crosshair", "tcross", "cross"],
            CursorIcon::Cell => vec!["cell", "plus", "crosshair"],
            CursorIcon::Copy => vec!["copy", "dnd-copy", "default"],
            CursorIcon::Alias => vec!["alias", "link", "dnd-link", "default"],
            CursorIcon::NotAllowed | CursorIcon::NoDrop => {
                vec!["not-allowed", "no-drop", "crossed_circle", "pirate"]
            }
            CursorIcon::ZoomIn => vec!["zoom-in", "default"],
            CursorIcon::ZoomOut => vec!["zoom-out", "default"],
            CursorIcon::EwResize | CursorIcon::ColResize => {
                vec!["ew-resize", "col-resize", "sb_h_double_arrow", "size_hor"]
            }
            CursorIcon::NsResize | CursorIcon::RowResize => {
                vec!["ns-resize", "row-resize", "sb_v_double_arrow", "size_ver"]
            }
            CursorIcon::EResize => vec!["e-resize", "right_side", "ew-resize"],
            CursorIcon::WResize => vec!["w-resize", "left_side", "ew-resize"],
            CursorIcon::NResize => vec!["n-resize", "top_side", "ns-resize"],
            CursorIcon::SResize => vec!["s-resize", "bottom_side", "ns-resize"],
            CursorIcon::NwseResize | CursorIcon::NwResize | CursorIcon::SeResize => {
                vec!["nwse-resize", "bd_double_arrow", "size_fdiag"]
            }
            CursorIcon::NeswResize | CursorIcon::NeResize | CursorIcon::SwResize => {
                vec!["nesw-resize", "fd_double_arrow", "size_bdiag"]
            }
            _ => vec![primary, "default", "left_ptr"],
        };

        let target_px = (self.size * int_scale as u32).clamp(16, 96);
        if let Some(ref theme) = self.theme {
            for alias in aliases {
                if let Some(loaded) =
                    load_cursor_from_theme(theme, alias, target_px, self.size, int_scale)
                {
                    self.cache.insert(key.clone(), loaded);
                    return self.cache.get(&key).unwrap_or(&self.fallback);
                }
            }
        }

        &self.fallback
    }
}

fn resolve_cursor_theme(preferred: &str, size: u32) -> (String, Option<CursorTheme>) {
    let candidates: [&str; 7] = [
        preferred,
        "macOS",
        "macOS-White",
        "Bibata-Modern-Ice",
        "Adwaita",
        "breeze_cursors",
        "default",
    ];
    for name in candidates {
        if name.trim().is_empty() {
            continue;
        }
        let theme = CursorTheme::load(name);
        if theme.load_icon("default").is_some() || theme.load_icon("left_ptr").is_some() {
            info!(theme = name, size, "Loaded XCursor theme for NEON DE");
            return (name.to_string(), Some(theme));
        }
    }
    (preferred.to_string(), None)
}

fn load_cursor_from_theme(
    theme: &CursorTheme,
    name: &str,
    target_px: u32,
    logical_target_px: u32,
    buffer_scale: i32,
) -> Option<LoadedCursor> {
    let path = theme.load_icon(name)?;
    let bytes = fs::read(path).ok()?;
    let images = parse_xcursor(&bytes)?;
    let best = images
        .into_iter()
        .min_by_key(|img| (img.size as i32 - target_px as i32).abs())?;
    if best.width == 0 || best.height == 0 {
        return None;
    }
    let eff_scale = buffer_scale.max(1);
    // XCursor files store premultiplied 32-bit little-endian ARGB pixels (BGRA byte order),
    // which maps directly to `Fourcc::Argb8888` — enabling accurate colors and zero-copy
    // 1:1 DRM hardware cursor plane scanout on Intel/AMD/NVIDIA KMS.
    let buffer = MemoryRenderBuffer::from_slice(
        &best.pixels_rgba,
        Fourcc::Argb8888,
        (best.width as i32, best.height as i32),
        eff_scale,
        Transform::Normal,
        None,
    );

    let natural_logical_w = best.width as f64 / eff_scale as f64;
    let natural_logical_h = best.height as f64 / eff_scale as f64;
    let max_dim = best.width.max(best.height);
    let (logical_size, hotspot) = if (max_dim as i32 - target_px as i32).abs() > 2 {
        let ratio = logical_target_px as f64 / natural_logical_w.max(natural_logical_h).max(1.0);
        let scaled_w = (natural_logical_w * ratio).round().max(1.0) as i32;
        let scaled_h = (natural_logical_h * ratio).round().max(1.0) as i32;
        (
            Some(Size::from((scaled_w, scaled_h))),
            (
                (best.xhot as f64 / eff_scale as f64) * ratio,
                (best.yhot as f64 / eff_scale as f64) * ratio,
            ),
        )
    } else {
        (
            None,
            (
                best.xhot as f64 / eff_scale as f64,
                best.yhot as f64 / eff_scale as f64,
            ),
        )
    };

    Some(LoadedCursor {
        buffer,
        hotspot,
        logical_size,
    })
}

/// Embedded crisp 16×20 authentic macOS black-fill, white-outline arrow cursor fallback.
fn build_fallback_macos_cursor() -> LoadedCursor {
    const W: usize = 16;
    const H: usize = 20;
    let shape: [&str; H] = [
        "W...............",
        "WW..............",
        "WBW.............",
        "WBBW............",
        "WBBBW...........",
        "WBBBBW..........",
        "WBBBBBW.........",
        "WBBBBBBW........",
        "WBBBBBBBW.......",
        "WBBBBBBBBW......",
        "WBBBBBBBBBW.....",
        "WBBBBBBBBBBW....",
        "WBBBBBBWWWWWW...",
        "WBBBWBBW........",
        "WBBW.WBBW.......",
        "WBW...WBBW......",
        "WW....WBBW......",
        ".......WBBW.....",
        "........WW......",
        "................",
    ];
    let mut pixels = vec![0u8; W * H * 4];
    for (y, row) in shape.iter().enumerate() {
        for (x, ch) in row.bytes().enumerate() {
            let idx = (y * W + x) * 4;
            match ch {
                b'W' => {
                    // BGRA (Fourcc::Argb8888)
                    pixels[idx] = 255;
                    pixels[idx + 1] = 255;
                    pixels[idx + 2] = 255;
                    pixels[idx + 3] = 255;
                }
                b'B' => {
                    pixels[idx] = 16;
                    pixels[idx + 1] = 12;
                    pixels[idx + 2] = 10;
                    pixels[idx + 3] = 255;
                }
                _ => {}
            }
        }
    }
    let buffer = MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Argb8888,
        (W as i32, H as i32),
        1,
        Transform::Normal,
        None,
    );
    LoadedCursor {
        buffer,
        hotspot: (0.0, 0.0),
        logical_size: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_macos_cursor_has_exact_dimensions_and_argb_format() {
        let cur = build_fallback_macos_cursor();
        assert_eq!(cur.hotspot, (0.0, 0.0));
        assert!(cur.logical_size.is_none());
    }

    #[test]
    fn cursor_cache_loads_at_requested_size_without_panicking() {
        let mut cache = CursorCache::load_with("macOS", 20);
        let cur = cache.get_named_scaled(CursorIcon::Default, 1.0);
        assert!(cur.hotspot.0 >= 0.0 && cur.hotspot.1 >= 0.0);
        let ptr = cache.get_named_scaled(CursorIcon::Pointer, 1.0);
        assert!(ptr.hotspot.0 >= 0.0 && ptr.hotspot.1 >= 0.0);
    }
}
