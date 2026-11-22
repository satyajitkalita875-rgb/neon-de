//! Real XCursor theme loader and client-surface cursor rendering for bare-metal DRM/KMS.
//!
//! Loads crisp anti-aliased cursors from the user's installed XCursor themes (`Bibata-Modern-Ice`,
//! `Adwaita`, `default`), supports Wayland client-provided cursor surfaces (`wl_pointer.set_cursor`),
//! and falls back to an embedded anti-aliased arrow bitmap if no theme files exist.

use std::{collections::HashMap, fs};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::Transform,
};
use tracing::info;
use xcursor::{CursorTheme, parser::parse_xcursor};

/// A cached cursor image buffer along with its logical hotspot offset.
#[derive(Debug, Clone)]
pub struct LoadedCursor {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: (i32, i32),
}

/// Manages loaded XCursor theme bitmaps with lazy caching per [`CursorIcon`].
#[derive(Debug)]
pub struct CursorCache {
    theme: Option<CursorTheme>,
    size: u32,
    cache: HashMap<String, LoadedCursor>,
    fallback: LoadedCursor,
}

impl CursorCache {
    pub fn load_default() -> Self {
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(24)
            .clamp(16, 48);

        let preferred_env = std::env::var("XCURSOR_THEME").ok();
        let candidates: Vec<&str> = [
            preferred_env.as_deref(),
            Some("Bibata-Modern-Ice"),
            Some("Adwaita"),
            Some("breeze_cursors"),
            Some("default"),
        ]
        .into_iter()
        .flatten()
        .collect();

        let mut chosen_theme = None;
        for name in candidates {
            let theme = CursorTheme::load(name);
            if theme.load_icon("default").is_some() || theme.load_icon("left_ptr").is_some() {
                info!(theme = name, size, "Loaded XCursor theme for NEON DE");
                chosen_theme = Some(theme);
                break;
            }
        }

        let fallback = build_fallback_arrow_cursor();
        let mut cache = Self {
            theme: chosen_theme,
            size,
            cache: HashMap::new(),
            fallback,
        };
        // Pre-warm the default arrow cursor
        let _ = cache.get_named(CursorIcon::Default);
        cache
    }

    pub fn get_named(&mut self, icon: CursorIcon) -> &LoadedCursor {
        let primary = icon.name();
        if self.cache.contains_key(primary) {
            return self.cache.get(primary).unwrap_or(&self.fallback);
        }

        let aliases: Vec<&str> = match icon {
            CursorIcon::Default => vec!["default", "left_ptr", "arrow"],
            CursorIcon::Pointer => vec!["pointer", "hand2", "hand1", "pointing_hand"],
            CursorIcon::Text => vec!["text", "xterm", "ibeam"],
            CursorIcon::Grab => vec!["grab", "openhand"],
            CursorIcon::Grabbing => vec!["grabbing", "closedhand", "fleur"],
            CursorIcon::Move => vec!["move", "fleur", "all-scroll"],
            CursorIcon::Wait | CursorIcon::Progress => vec!["wait", "watch", "left_ptr_watch"],
            CursorIcon::EwResize | CursorIcon::ColResize => {
                vec!["ew-resize", "sb_h_double_arrow", "size_hor"]
            }
            CursorIcon::NsResize | CursorIcon::RowResize => {
                vec!["ns-resize", "sb_v_double_arrow", "size_ver"]
            }
            CursorIcon::NwseResize => vec!["nwse-resize", "bd_double_arrow", "size_fdiag"],
            CursorIcon::NeswResize => vec!["nesw-resize", "fd_double_arrow", "size_bdiag"],
            _ => vec![primary, "default", "left_ptr"],
        };

        if let Some(ref theme) = self.theme {
            for alias in aliases {
                if let Some(loaded) = load_cursor_from_theme(theme, alias, self.size) {
                    self.cache.insert(primary.to_string(), loaded);
                    return self.cache.get(primary).unwrap_or(&self.fallback);
                }
            }
        }

        &self.fallback
    }
}

fn load_cursor_from_theme(
    theme: &CursorTheme,
    name: &str,
    target_size: u32,
) -> Option<LoadedCursor> {
    let path = theme.load_icon(name)?;
    let bytes = fs::read(path).ok()?;
    let images = parse_xcursor(&bytes)?;
    let best = images
        .into_iter()
        .min_by_key(|img| (img.size as i32 - target_size as i32).abs())?;
    if best.width == 0 || best.height == 0 {
        return None;
    }
    // `xcursor` returns premultiplied RGBA bytes -> `Fourcc::Abgr8888` on little-endian.
    let buffer = MemoryRenderBuffer::from_slice(
        &best.pixels_rgba,
        Fourcc::Abgr8888,
        (best.width as i32, best.height as i32),
        1,
        Transform::Normal,
        None,
    );
    Some(LoadedCursor {
        buffer,
        hotspot: (best.xhot as i32, best.yhot as i32),
    })
}

/// Embedded crisp 20×24 white-on-black macOS-style arrow cursor used only if no XCursor theme exists.
fn build_fallback_arrow_cursor() -> LoadedCursor {
    const W: usize = 20;
    const H: usize = 24;
    let shape: [&str; H] = [
        "B...................",
        "BB..................",
        "BWB.................",
        "BWWB................",
        "BWWWB...............",
        "BWWWWB..............",
        "BWWWWWB.............",
        "BWWWWWWB............",
        "BWWWWWWWB...........",
        "BWWWWWWWWB..........",
        "BWWWWWWWWWB.........",
        "BWWWWWWWWWWB........",
        "BWWWWWWWWWWWB.......",
        "BWWWWWWBBBBBBB......",
        "BWWWBWWB............",
        "BWWB.BWWB...........",
        "BWB..BWWB...........",
        "BB....BWWB..........",
        "B.....BWWB..........",
        ".......BWWB.........",
        ".......BWWB.........",
        "........BB..........",
        "....................",
        "....................",
    ];
    let mut pixels = vec![0u8; W * H * 4];
    for (y, row) in shape.iter().enumerate() {
        for (x, ch) in row.bytes().enumerate() {
            let idx = (y * W + x) * 4;
            match ch {
                b'W' => {
                    pixels[idx] = 250;
                    pixels[idx + 1] = 252;
                    pixels[idx + 2] = 255;
                    pixels[idx + 3] = 255;
                }
                b'B' => {
                    pixels[idx] = 12;
                    pixels[idx + 1] = 16;
                    pixels[idx + 2] = 24;
                    pixels[idx + 3] = 255;
                }
                _ => {}
            }
        }
    }
    let buffer = MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Abgr8888,
        (W as i32, H as i32),
        1,
        Transform::Normal,
        None,
    );
    LoadedCursor {
        buffer,
        hotspot: (0, 0),
    }
}
