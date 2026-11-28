//! Apple iOS Dynamic Island + Little Notch anti-aliased Cairo visual engine for NEON DE.
//!
//! Renders:
//! - Authentic floating pure `#000000` Apple iOS Dynamic Island pill/squircle (default `OledBlack`),
//!   plus optional top-flush concave-ear MacBook notch (`NotchBlack`) and frosted `LiquidGlass`
//! - Refined animated 60fps revolving neon gradient rim & subtle outer bloom derived live from
//!   active album art colors while keeping the interior pure high-contrast OLED black
//! - Razor-sharp custom Cairo SF-style vector glyphs (`Antialias::Best`):
//!   Apple transport icons (Rewind, Pause, Play, Forward, AirPlay/Audio Waves), Apple green-gradient
//!   charging battery icon, Apple Silent red bell-slash pill, Apple Music gradient fallback cover,
//!   live audio waveform, clean Apple progress scrubber, and module gauges/charts.

use std::f64::consts::{PI, TAU};

use gtk4::{cairo, gdk, prelude::*};
use serde::{Deserialize, Serialize};

use crate::island_data::NotchModule;

/// Linear RGB triplet in `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }

    pub fn lerp(self, target: Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            r: self.r + (target.r - self.r) * t,
            g: self.g + (target.g - self.g) * t,
            b: self.b + (target.b - self.b) * t,
        }
    }

    pub fn distance_sq(self, other: Self) -> f64 {
        let dr = self.r - other.r;
        let dg = self.g - other.g;
        let db = self.b - other.b;
        dr * dr + dg * dg + db * db
    }

    /// Parses `#RRGGBB` hex string (fallback to Apple Blue `#0A84FF`).
    pub fn from_hex(hex: &str) -> Self {
        let s = hex.trim().trim_start_matches('#');
        if s.len() == 6
            && let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&s[0..2], 16),
                u8::from_str_radix(&s[2..4], 16),
                u8::from_str_radix(&s[4..6], 16),
            )
        {
            return Self::new(
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
            );
        }
        Self::new(0.04, 0.52, 1.0)
    }
}

/// Three-stop vibrant neon palette derived from active album art.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NeonPalette {
    pub primary: Rgb,
    pub secondary: Rgb,
    pub tertiary: Rgb,
}

impl Default for NeonPalette {
    fn default() -> Self {
        Self {
            primary: Rgb::new(0.98, 0.24, 0.52),   // Apple Music Crimson-Pink
            secondary: Rgb::new(0.55, 0.34, 0.99), // Vivid Indigo-Violet
            tertiary: Rgb::new(0.18, 0.80, 0.99),  // Electric Cyan
        }
    }
}

impl NeonPalette {
    pub fn lerp(self, target: Self, t: f64) -> Self {
        Self {
            primary: self.primary.lerp(target.primary, t),
            secondary: self.secondary.lerp(target.secondary, t),
            tertiary: self.tertiary.lerp(target.tertiary, t),
        }
    }

    pub fn is_close_to(self, other: Self) -> bool {
        self.primary.distance_sq(other.primary) < 1e-5
            && self.secondary.distance_sq(other.secondary) < 1e-5
            && self.tertiary.distance_sq(other.tertiary) < 1e-5
    }
}

/// Visual silhouette & surface style for the Dynamic Island.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotchAppearance {
    /// Floating pure `#000000` Apple iOS Dynamic Island pill/squircle (default).
    #[default]
    Black,
    /// Top-flush `#000000` MacBook notch with concave inverted-fillet ears.
    BlackGlass,
    /// Floating translucent frosted Liquid Glass capsule.
    LiquidGlass,
}

impl NotchAppearance {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Black => "iOS Black",
            Self::BlackGlass => "Mac Notch",
            Self::LiquidGlass => "Liquid Glass",
        }
    }

    pub const fn is_top_flush_notch(self) -> bool {
        matches!(self, Self::BlackGlass)
    }
}

/// Extracts a 3-stop vibrant neon gradient palette directly from a GTK4 `gdk::Texture`.
pub fn extract_neon_palette_from_texture(texture: &gdk::Texture) -> NeonPalette {
    let w = texture.width().max(0) as usize;
    let h = texture.height().max(0) as usize;
    if w == 0 || h == 0 {
        return NeonPalette::default();
    }
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    extract_neon_palette_from_rgba(bytes.as_ref(), w, h, stride)
}

/// Pure vibrant neon palette extractor from RGBA8888 album-art pixels.
pub fn extract_neon_palette_from_rgba(
    rgba: &[u8],
    width: usize,
    height: usize,
    stride: usize,
) -> NeonPalette {
    if width == 0 || height == 0 || stride < width * 4 {
        return NeonPalette::default();
    }

    const BINS: usize = 12;
    let mut bin_weight = [0.0_f64; BINS];
    let mut bin_r = [0.0_f64; BINS];
    let mut bin_g = [0.0_f64; BINS];
    let mut bin_b = [0.0_f64; BINS];

    let step_x = (width / 18).max(1);
    let step_y = (height / 18).max(1);

    let mut fallback_w = 0.0_f64;
    let mut fallback_r = 0.0_f64;
    let mut fallback_g = 0.0_f64;
    let mut fallback_b = 0.0_f64;

    let mut y = step_y / 2;
    while y < height {
        let row_off = y * stride;
        let mut x = step_x / 2;
        while x < width {
            let idx = row_off + x * 4;
            if idx + 3 < rgba.len() {
                let a = f64::from(rgba[idx + 3]) / 255.0;
                if a > 0.25 {
                    let r = f64::from(rgba[idx]) / 255.0;
                    let g = f64::from(rgba[idx + 1]) / 255.0;
                    let b = f64::from(rgba[idx + 2]) / 255.0;
                    let (hue, sat, val) = rgb_to_hsv(r, g, b);

                    if val > 0.10 {
                        fallback_w += 1.0;
                        fallback_r += r;
                        fallback_g += g;
                        fallback_b += b;
                    }

                    if sat >= 0.16 && val >= 0.16 && !(sat < 0.22 && val > 0.94) {
                        let bin = ((hue / 30.0).floor() as usize) % BINS;
                        let bright_factor = 0.35 + 0.65 * (1.0 - (val - 0.72).abs().min(0.72));
                        let w = sat.powf(1.55) * bright_factor * a;
                        bin_weight[bin] += w;
                        bin_r[bin] += r * w;
                        bin_g[bin] += g * w;
                        bin_b[bin] += b * w;
                    }
                }
            }
            x += step_x;
        }
        y += step_y;
    }

    let mut ranked: Vec<(usize, f64)> = bin_weight
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, w)| *w > 1e-4)
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

    let Some(&(best_bin, best_w)) = ranked.first() else {
        if fallback_w > 0.0 {
            let avg = Rgb::new(
                fallback_r / fallback_w,
                fallback_g / fallback_w,
                fallback_b / fallback_w,
            );
            let lum = (0.299 * avg.r + 0.587 * avg.g + 0.114 * avg.b).clamp(0.45, 0.85);
            return NeonPalette {
                primary: Rgb::new((lum * 0.88).min(1.0), (lum * 0.96).min(1.0), 1.0),
                secondary: Rgb::new(0.54, 0.74, 0.98),
                tertiary: Rgb::new(0.84, 0.90, 0.99),
            };
        }
        return NeonPalette::default();
    };

    let primary_raw = Rgb::new(
        bin_r[best_bin] / best_w,
        bin_g[best_bin] / best_w,
        bin_b[best_bin] / best_w,
    );
    let primary = boost_to_neon(primary_raw, 0.74, 0.94);
    let (h1, _, _) = rgb_to_hsv(primary.r, primary.g, primary.b);

    let secondary_bin = ranked.iter().skip(1).find_map(|&(bin, w)| {
        let dist = circular_bin_distance(best_bin, bin, BINS);
        (dist >= 2 && w >= best_w * 0.12).then_some((bin, w))
    });
    let secondary = match secondary_bin {
        Some((bin, w)) => boost_to_neon(
            Rgb::new(bin_r[bin] / w, bin_g[bin] / w, bin_b[bin] / w),
            0.72,
            0.96,
        ),
        None => hsv_to_rgb((h1 + 42.0) % 360.0, 0.84, 0.98),
    };

    let tertiary_bin = ranked.iter().skip(1).find_map(|&(bin, w)| {
        let d1 = circular_bin_distance(best_bin, bin, BINS);
        let d2 = secondary_bin
            .map(|(sb, _)| circular_bin_distance(sb, bin, BINS))
            .unwrap_or(2);
        (d1 >= 2 && d2 >= 2 && w >= best_w * 0.10).then_some((bin, w))
    });
    let tertiary = match tertiary_bin {
        Some((bin, w)) => boost_to_neon(
            Rgb::new(bin_r[bin] / w, bin_g[bin] / w, bin_b[bin] / w),
            0.68,
            1.0,
        ),
        None => hsv_to_rgb((h1 + 322.0) % 360.0, 0.78, 1.0),
    };

    NeonPalette {
        primary,
        secondary,
        tertiary,
    }
}

fn circular_bin_distance(a: usize, b: usize, bins: usize) -> usize {
    let diff = a.abs_diff(b);
    diff.min(bins - diff)
}

fn boost_to_neon(rgb: Rgb, min_sat: f64, min_val: f64) -> Rgb {
    let (h, s, v) = rgb_to_hsv(rgb.r, rgb.g, rgb.b);
    let boosted_s = (s * 1.25).clamp(min_sat, 0.96);
    let boosted_v = (v * 1.22).clamp(min_val, 1.0);
    hsv_to_rgb(h, boosted_s, boosted_v)
}

fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta <= 1e-6 {
        0.0
    } else if (max - r).abs() <= 1e-6 {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() <= 1e-6 {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let s = if max <= 1e-6 { 0.0 } else { delta / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> Rgb {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Rgb::new(r1 + m, g1 + m, b1 + m)
}

/// Velocity-preserving Material 3 Expressive 1D damped harmonic spring.
#[derive(Debug, Clone, Copy)]
pub struct Spring1D {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
}

impl Spring1D {
    pub const fn new(initial: f64) -> Self {
        Self {
            value: initial,
            velocity: 0.0,
            target: initial,
        }
    }

    pub fn snap(&mut self, value: f64) {
        self.value = value;
        self.velocity = 0.0;
        self.target = value;
    }

    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    pub fn step(&mut self, dt: f64, zeta: f64, omega0: f64, epsilon: f64) -> bool {
        let clamped_dt = dt.clamp(0.0, 0.05);
        if clamped_dt <= 0.0 {
            return !self.is_settled(epsilon);
        }

        let sub_steps = ((clamped_dt * 240.0).ceil() as usize).clamp(1, 12);
        let h = clamped_dt / (sub_steps as f64);
        let k = omega0 * omega0;
        let c = 2.0 * zeta * omega0;

        for _ in 0..sub_steps {
            let disp = self.value - self.target;
            let accel = -k * disp - c * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
        }

        if self.is_settled(epsilon) {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }

    pub fn is_settled(&self, epsilon: f64) -> bool {
        (self.value - self.target).abs() <= epsilon && self.velocity.abs() <= epsilon * 6.0
    }
}

/// Visual accent mode for the Dynamic Island backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IslandAuraKind {
    Idle,
    Media,
    Timer,
    Clipboard,
    FileShelf,
    Recording,
    Notification,
    Hud,
    Charging,
    Silent,
}

/// Draws the Dynamic Island body, shadow, and refined animated album-art neon rim.
///
/// - `NotchAppearance::Black` (default): authentic Apple iOS floating pure `#000000` pill / squircle
///   positioned at `y = top_offset`, with razor-sharp contrast and subtle elevation shadow.
/// - `NotchAppearance::BlackGlass`: top-flush `#000000` MacBook notch with concave ears (`y = 0`).
/// - `NotchAppearance::LiquidGlass`: floating frosted Liquid Glass pill/squircle (`y = top_offset`).
#[allow(clippy::too_many_arguments)]
pub fn draw_notch_backdrop(
    cr: &cairo::Context,
    canvas_w: f64,
    top_offset: f64,
    w: f64,
    h: f64,
    radius: f64,
    ear_radius: f64,
    appearance: NotchAppearance,
    aura_kind: IslandAuraKind,
    expanded_alpha: f64,
    palette: NeonPalette,
    neon_intensity: f64,
    neon_phase: f64,
    _accent: Rgb,
    drag_hover: bool,
) {
    cr.set_antialias(cairo::Antialias::Best);

    // Snap width to an even integer so `(canvas_w - w) * 0.5` is ALWAYS an exact integer pixel!
    let w = (((w * 0.5).round()) * 2.0).max(112.0);
    let h = h.round().max(30.0);
    let x = ((canvas_w - w) * 0.5).round();
    let is_notch = appearance.is_top_flush_notch();
    let y = if is_notch { 0.0 } else { top_offset.round() };
    let r = radius.clamp(14.0, (h * 0.5).min(w * 0.5));
    let er = ear_radius.clamp(6.0, (h * 0.42).min(18.0));

    let trace_closed = |cr: &cairo::Context, ox: f64, oy: f64, ow: f64, oh: f64, or: f64| {
        if is_notch {
            notch_ear_path(cr, ox, 0.0, ow, oh, er, or);
        } else {
            rounded_rect_path(cr, ox, oy, ow, oh, or);
        }
    };
    let trace_rim = |cr: &cairo::Context, ox: f64, oy: f64, ow: f64, oh: f64, or: f64| {
        if is_notch {
            notch_ear_open_rim_path(cr, ox, 0.0, ow, oh, er, or);
        } else {
            rounded_rect_path(cr, ox, oy, ow, oh, or);
        }
    };

    // 1. Continuous 22-step Gaussian ambient drop shadow (zero Mach banding or stepped rings!).
    let exp_t = expanded_alpha.clamp(0.0, 1.0);
    let shadow_scale = 0.42 + 0.58 * exp_t;
    const SHADOW_STEPS: usize = 22;
    for i in (1..=SHADOW_STEPS).rev() {
        let t = (i as f64) / (SHADOW_STEPS as f64);
        let spread = (0.8 + 22.0 * t * t) * shadow_scale;
        let dy = (0.8 + 8.5 * t) * shadow_scale;
        let gauss = (-2.65 * t * t).exp();
        let alpha = 0.0155 * gauss * shadow_scale;
        if alpha > 0.001 {
            trace_closed(
                cr,
                x - spread * 0.5,
                y + dy - spread * 0.28,
                w + spread,
                h + spread * 0.68,
                r + spread * 0.46,
            );
            cr.set_source_rgba(0.0, 0.0, 0.0, alpha);
            let _ = cr.fill();
        }
    }

    // 2. Smooth multi-step Gaussian outer aura ONLY for File Dock drag-hover or active Recording
    //    (media playback uses an interior dark-AMOLED animated gradient instead of an outer border glow).
    let neon = neon_intensity.clamp(0.0, 1.0);
    if drag_hover {
        for i in (1..=12).rev() {
            let t = f64::from(i) / 12.0;
            let expand = 1.0 + 12.0 * t * t;
            let alpha = 0.048 * (-2.2 * t * t).exp();
            trace_rim(
                cr,
                x - expand * 0.5,
                y - expand * 0.5,
                w + expand,
                h + expand,
                r + expand * 0.5,
            );
            cr.set_source_rgba(0.04, 0.52, 1.0, alpha);
            cr.set_line_width(2.2);
            let _ = cr.stroke();
        }
    } else if aura_kind == IslandAuraKind::Recording {
        let pulse = 0.55 + 0.45 * (neon_phase * 2.4).sin().abs();
        for i in (1..=8).rev() {
            let t = f64::from(i) / 8.0;
            let expand = 0.8 + 7.0 * t * t;
            trace_rim(
                cr,
                x - expand * 0.5,
                y - expand * 0.5,
                w + expand,
                h + expand,
                r + expand * 0.5,
            );
            cr.set_source_rgba(1.0, 0.23, 0.19, 0.045 * (-2.0 * t * t).exp() * pulse);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
    }

    // 3. Pure `#000000` Apple OLED fill + subtle dark-AMOLED left-to-right song-color gradient inside.
    let _ = cr.save();
    trace_closed(cr, x, y, w, h, r);
    cr.clip();

    match appearance {
        NotchAppearance::Black | NotchAppearance::BlackGlass => {
            cr.set_source_rgba(0.0, 0.0, 0.0, 1.0);
            cr.rectangle(x - er - 2.0, y - 2.0, w + er * 2.0 + 4.0, h + 4.0);
            let _ = cr.fill();
        }
        NotchAppearance::LiquidGlass => {
            let g = cairo::LinearGradient::new(x, y, x, y + h);
            g.add_color_stop_rgba(0.0, 0.16, 0.21, 0.30, 0.44);
            g.add_color_stop_rgba(0.55, 0.10, 0.13, 0.20, 0.38);
            g.add_color_stop_rgba(1.0, 0.06, 0.08, 0.14, 0.46);
            let _ = cr.set_source(&g);
            cr.rectangle(x - 2.0, y - 2.0, w + 4.0, h + 4.0);
            let _ = cr.fill();
        }
    }

    if neon > 0.01 {
        draw_amoled_media_sweep(cr, x, y, w, h, er, neon_phase, palette, neon);
    }
    let _ = cr.restore();

    // 4. Crisp 1px pixel-centered neutral hairline border (`x + 0.5`, `y + 0.5` — no colored media border).
    trace_rim(
        cr,
        x + 0.5,
        y + 0.5,
        w - 1.0,
        h - 1.0,
        (r - 0.5).max(10.0),
    );
    if drag_hover {
        cr.set_source_rgba(0.04, 0.52, 1.0, 0.95);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    } else if aura_kind == IslandAuraKind::Recording {
        cr.set_source_rgba(1.0, 0.27, 0.23, 0.72);
        cr.set_line_width(1.2);
        let _ = cr.stroke();
    } else if aura_kind == IslandAuraKind::Charging {
        cr.set_source_rgba(0.19, 0.82, 0.35, 0.30);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    } else if aura_kind == IslandAuraKind::Silent {
        cr.set_source_rgba(1.0, 0.23, 0.19, 0.28);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    } else if matches!(appearance, NotchAppearance::LiquidGlass) {
        let rim = cairo::LinearGradient::new(x, y, x + w * 0.35, y + h);
        rim.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.72);
        rim.add_color_stop_rgba(0.32, 1.0, 1.0, 1.0, 0.22);
        rim.add_color_stop_rgba(0.75, 1.0, 1.0, 1.0, 0.12);
        rim.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.38);
        let _ = cr.set_source(&rim);
        cr.set_line_width(1.2);
        let _ = cr.stroke();
    } else {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.11);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }
}

/// Parameters controlling Apple Liquid Glass rendering across both GPU-composited mode
/// (`gpu_backed = true`) and immediate pre-restart wallpaper-refraction fallback.
pub struct LiquidGlassPaint<'a> {
    pub wallpaper_crop: Option<&'a cairo::ImageSurface>,
    pub wallpaper_avg_lum: f64,
    pub gpu_backed: bool,
    pub aura_kind: IslandAuraKind,
    pub expanded_alpha: f64,
    pub palette: NeonPalette,
    pub neon_intensity: f64,
    pub neon_phase: f64,
    pub drag_hover: bool,
}

fn draw_liquid_glass_capsule(
    cr: &cairo::Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    r: f64,
    wp_offset_x: f64,
    wp_offset_y: f64,
    paint: &LiquidGlassPaint<'_>,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let r = r.clamp(12.0, (h * 0.5).min(w * 0.5));

    if paint.gpu_backed {
        // Real-time GPU `LIQUID_GLASS_FRAGMENT_SHADER` paints blur + Snell refraction + 3D rim.
        // Only draw active state outlines (drag target / live recording) in Cairo to avoid double borders.
        if paint.drag_hover {
            rounded_rect_path(cr, x + 0.75, y + 0.75, w - 1.5, h - 1.5, (r - 0.75).max(8.0));
            cr.set_source_rgba(0.04, 0.52, 1.0, 0.92);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
        } else if paint.aura_kind == IslandAuraKind::Recording {
            rounded_rect_path(cr, x + 0.75, y + 0.75, w - 1.5, h - 1.5, (r - 0.75).max(8.0));
            cr.set_source_rgba(1.0, 0.27, 0.23, 0.74);
            cr.set_line_width(1.25);
            let _ = cr.stroke();
        }
        return;
    }

    // 1. Soft ambient Mac floating elevation shadow.
    let exp_t = paint.expanded_alpha.clamp(0.0, 1.0);
    let shadow_scale = 0.45 + 0.55 * exp_t;
    for i in (1..=14).rev() {
        let t = f64::from(i) / 14.0;
        let spread = (0.8 + 16.0 * t * t) * shadow_scale;
        let dy = (1.0 + 6.0 * t) * shadow_scale;
        let a = 0.014 * (-2.6 * t * t).exp() * shadow_scale;
        if a > 0.001 {
            rounded_rect_path(
                cr,
                x - spread * 0.5,
                y + dy - spread * 0.25,
                w + spread,
                h + spread * 0.70,
                (r + spread * 0.40).max(8.0),
            );
            cr.set_source_rgba(0.0, 0.0, 0.0, a);
            let _ = cr.fill();
        }
    }

    // 2. Optical Glass Body + Radial Rim Refraction clipped inside capsule.
    let _ = cr.save();
    rounded_rect_path(cr, x, y, w, h, r);
    cr.clip();

    if let Some(wp) = paint.wallpaper_crop {
        let pat = cairo::SurfacePattern::create(wp);
        pat.set_extend(cairo::Extend::Pad);
        pat.set_filter(cairo::Filter::Bilinear);
        let mut m = cairo::Matrix::identity();
        m.translate(wp_offset_x, wp_offset_y);
        pat.set_matrix(m);
        let _ = cr.set_source(&pat);
        let _ = cr.paint();

        // Outer curved meniscus refraction ring (`EvenOdd` band radially magnified around center).
        let bevel = (w.min(h) * 0.26).clamp(7.0, 17.0);
        if w > bevel * 2.5 && h > bevel * 2.5 {
            let _ = cr.save();
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            rounded_rect_path(cr, x, y, w, h, r);
            rounded_rect_path(
                cr,
                x + bevel,
                y + bevel,
                w - bevel * 2.0,
                h - bevel * 2.0,
                (r - bevel).max(4.0),
            );
            cr.clip();

            let cx = x + w * 0.5 + wp_offset_x;
            let cy = y + h * 0.5 + wp_offset_y;
            let refract_pat = cairo::SurfacePattern::create(wp);
            refract_pat.set_extend(cairo::Extend::Pad);
            refract_pat.set_filter(cairo::Filter::Bilinear);
            let mut rm = cairo::Matrix::identity();
            rm.translate(cx, cy);
            rm.scale(0.90, 0.88);
            rm.translate(-cx + wp_offset_x, -cy + wp_offset_y);
            refract_pat.set_matrix(rm);
            let _ = cr.set_source(&refract_pat);
            let _ = cr.paint();

            // Subtle prismatic spectral shimmer inside the curved edge bevel.
            let prism = cairo::LinearGradient::new(x, y, x + w, y + h);
            prism.add_color_stop_rgba(0.0, 0.55, 0.85, 1.0, 0.14);
            prism.add_color_stop_rgba(0.5, 1.0, 1.0, 1.0, 0.04);
            prism.add_color_stop_rgba(1.0, 1.0, 0.68, 0.88, 0.11);
            rounded_rect_path(cr, x + 0.8, y + 0.8, w - 1.6, h - 1.6, (r - 0.8).max(8.0));
            let _ = cr.set_source(&prism);
            cr.set_line_width(2.6);
            let _ = cr.stroke();
            let _ = cr.restore();
        }
    } else {
        let base = cairo::LinearGradient::new(x, y, x, y + h);
        base.add_color_stop_rgba(0.0, 0.16, 0.21, 0.30, 0.44);
        base.add_color_stop_rgba(1.0, 0.07, 0.10, 0.16, 0.46);
        let _ = cr.set_source(&base);
        let _ = cr.paint();
    }

    // 3. Adaptive luminance-aware polarizing frost + media jewel tint + upper convex sheen.
    let lum = paint.wallpaper_avg_lum.clamp(0.0, 1.0);
    let scrim = 0.20 + 0.28 * lum;
    let tint_r = 0.14 - 0.08 * lum;
    let tint_g = 0.18 - 0.10 * lum;
    let tint_b = 0.26 - 0.13 * lum;
    let frost = cairo::LinearGradient::new(x, y, x, y + h);
    frost.add_color_stop_rgba(0.0, tint_r, tint_g, tint_b, scrim);
    frost.add_color_stop_rgba(1.0, tint_r * 0.55, tint_g * 0.55, tint_b * 0.65, scrim + 0.06);
    let _ = cr.set_source(&frost);
    let _ = cr.paint();

    if paint.neon_intensity > 0.01 {
        let _ = cr.save();
        cr.push_group();
        draw_amoled_media_sweep(
            cr,
            x,
            y,
            w,
            h,
            0.0,
            paint.neon_phase,
            paint.palette,
            paint.neon_intensity,
        );
        let _ = cr.pop_group_to_source();
        let _ = cr.paint_with_alpha(0.45);
        let _ = cr.restore();
    }

    let dome = cairo::LinearGradient::new(x, y, x, y + h * 0.48);
    dome.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.16);
    dome.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
    let _ = cr.set_source(&dome);
    let _ = cr.paint();

    rounded_rect_path(cr, x + 1.5, y + 1.5, w - 3.0, h - 3.0, (r - 1.5).max(8.0));
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.15);
    cr.set_line_width(2.2);
    let _ = cr.stroke();
    let _ = cr.restore();

    // 4. Sculpted 3D overhead studio specular bevel + bottom bounce caustic + outer hairline contrast.
    rounded_rect_path(cr, x, y, w, h, r);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.28);
    cr.set_line_width(0.8);
    let _ = cr.stroke();

    rounded_rect_path(cr, x + 0.65, y + 0.65, w - 1.3, h - 1.3, (r - 0.65).max(8.0));
    if paint.drag_hover {
        cr.set_source_rgba(0.04, 0.52, 1.0, 0.95);
        cr.set_line_width(1.6);
    } else if paint.aura_kind == IslandAuraKind::Recording {
        cr.set_source_rgba(1.0, 0.27, 0.23, 0.76);
        cr.set_line_width(1.3);
    } else if paint.aura_kind == IslandAuraKind::Charging {
        cr.set_source_rgba(0.19, 0.82, 0.35, 0.42);
        cr.set_line_width(1.1);
    } else if paint.aura_kind == IslandAuraKind::Silent {
        cr.set_source_rgba(1.0, 0.23, 0.19, 0.38);
        cr.set_line_width(1.1);
    } else {
        let rim = cairo::LinearGradient::new(x, y, x + w * 0.34, y + h);
        rim.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.78);
        rim.add_color_stop_rgba(0.30, 1.0, 1.0, 1.0, 0.24);
        rim.add_color_stop_rgba(0.75, 1.0, 1.0, 1.0, 0.14);
        rim.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.44);
        let _ = cr.set_source(&rim);
        cr.set_line_width(1.25);
    }
    let _ = cr.stroke();
}

/// Draws Apple Liquid Glass backdrop for the floating Mac Dynamic Island capsule.
pub fn draw_notch_liquid_glass(
    cr: &cairo::Context,
    canvas_w: f64,
    top_offset: f64,
    w: f64,
    h: f64,
    radius: f64,
    paint: &LiquidGlassPaint<'_>,
) {
    let w = (((w * 0.5).round()) * 2.0).max(112.0);
    let h = h.round().max(30.0);
    let x = ((canvas_w - w) * 0.5).round();
    let y = top_offset.round();
    let r = radius.clamp(14.0, (h * 0.5).min(w * 0.5));
    draw_liquid_glass_capsule(cr, x, y, w, h, r, 0.0, 0.0, paint);
}

/// Draws Apple Liquid Glass backdrop for the floating Mac Dock Switcher capsule.
pub fn draw_dock_pill_liquid_glass(
    cr: &cairo::Context,
    canvas_w: f64,
    canvas_h: f64,
    wp_offset_x: f64,
    wp_offset_y: f64,
    paint: &LiquidGlassPaint<'_>,
) {
    let pad_x = 12.0;
    let pad_top = 4.0;
    let pad_bot = 14.0;
    let w = (canvas_w - pad_x * 2.0).max(40.0).round();
    let h = (canvas_h - pad_top - pad_bot).max(24.0).round();
    let x = pad_x;
    let y = pad_top;
    let r = (h * 0.5).clamp(12.0, 20.0);
    draw_liquid_glass_capsule(cr, x, y, w, h, r, wp_offset_x, wp_offset_y, paint);
}

// =============================================================================
// Custom Razor-Sharp Apple SF-Style Vector Glyphs (Antialias::Best)
// =============================================================================

/// Draws Apple iOS Now Playing Rewind (`⏪`) or Forward (`⏩`) double-triangle SF glyph.
pub fn draw_sf_skip_icon(cr: &cairo::Context, w: f64, h: f64, forward: bool) {
    cr.set_antialias(cairo::Antialias::Best);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.96);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_width(2.0);

    let cx = w * 0.5;
    let cy = h * 0.5;
    let tw = 11.5;
    let th = 15.0;

    if forward {
        // Two right-pointing rounded triangles side by side
        draw_rounded_triangle(cr, cx - tw + 0.8, cy - th * 0.5, cx + 0.4, cy, cx - tw + 0.8, cy + th * 0.5);
        draw_rounded_triangle(cr, cx + 0.8, cy - th * 0.5, cx + tw + 0.4, cy, cx + 0.8, cy + th * 0.5);
    } else {
        // Two left-pointing rounded triangles side by side
        draw_rounded_triangle(cr, cx - 0.8, cy - th * 0.5, cx - tw - 0.4, cy, cx - 0.8, cy + th * 0.5);
        draw_rounded_triangle(cr, cx + tw - 0.8, cy - th * 0.5, cx - 0.4, cy, cx + tw - 0.8, cy + th * 0.5);
    }
}

fn draw_rounded_triangle(
    cr: &cairo::Context,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    x3: f64,
    y3: f64,
) {
    cr.new_sub_path();
    cr.move_to(x1, y1);
    cr.line_to(x2, y2);
    cr.line_to(x3, y3);
    cr.close_path();
    let _ = cr.fill_preserve();
    let _ = cr.stroke();
}

/// Draws Apple iOS Now Playing Pause (`⏸`) or Play (`▶`) pure white glyph (NO background circle!).
pub fn draw_sf_play_pause_icon(cr: &cairo::Context, w: f64, h: f64, playing: bool) {
    cr.set_antialias(cairo::Antialias::Best);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
    let cx = w * 0.5;
    let cy = h * 0.5;

    if playing {
        // Crisp bold twin rounded vertical pause bars (exact iOS Dynamic Island proportions!)
        let bar_w = 6.6;
        let bar_h = 23.0;
        let gap = 5.4;
        let r = 2.0;
        rounded_rect_path(cr, cx - gap * 0.5 - bar_w, cy - bar_h * 0.5, bar_w, bar_h, r);
        let _ = cr.fill();
        rounded_rect_path(cr, cx + gap * 0.5, cy - bar_h * 0.5, bar_w, bar_h, r);
        let _ = cr.fill();
    } else {
        // Crisp rounded Apple play triangle
        cr.set_line_join(cairo::LineJoin::Round);
        cr.set_line_width(2.6);
        let left = cx - 8.5;
        let right = cx + 10.5;
        let top = cy - 11.2;
        let bottom = cy + 11.2;
        draw_rounded_triangle(cr, left, top, right, cy, left, bottom);
    }
}

/// Draws Apple AirPlay / Audio Output concentric wave arcs + bottom triangle glyph.
pub fn draw_sf_airplay_icon(cr: &cairo::Context, w: f64, h: f64) {
    cr.set_antialias(cairo::Antialias::Best);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.92);
    cr.set_line_cap(cairo::LineCap::Round);

    let cx = w * 0.5;
    let cy = h * 0.46;

    // Three concentric audio broadcast arcs
    for (r, lw) in [(3.6, 1.7), (6.8, 1.7), (10.0, 1.7)] {
        cr.set_line_width(lw);
        cr.new_sub_path();
        cr.arc(cx, cy, r, PI * 0.78, PI * 2.22);
        let _ = cr.stroke();
    }

    // Center dot
    cr.arc(cx, cy, 1.5, 0.0, TAU);
    let _ = cr.fill();

    // Small crisp upward triangle centered at bottom
    cr.new_sub_path();
    cr.move_to(cx, cy + 4.4);
    cr.line_to(cx - 4.8, cy + 10.6);
    cr.line_to(cx + 4.8, cy + 10.6);
    cr.close_path();
    let _ = cr.fill();
}

/// Draws Apple Music-style vibrant gradient fallback album cover with crisp white music glyph.
pub fn draw_apple_fallback_art(cr: &cairo::Context, w: f64, h: f64, palette: NeonPalette) {
    cr.set_antialias(cairo::Antialias::Best);
    let grad = cairo::LinearGradient::new(0.0, 0.0, w, h);
    grad.add_color_stop_rgba(0.0, palette.primary.r, palette.primary.g, palette.primary.b, 1.0);
    grad.add_color_stop_rgba(
        0.55,
        palette.secondary.r,
        palette.secondary.g,
        palette.secondary.b,
        1.0,
    );
    grad.add_color_stop_rgba(
        1.0,
        palette.tertiary.r * 0.85,
        palette.tertiary.g * 0.85,
        palette.tertiary.b * 0.95,
        1.0,
    );
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.set_source(&grad);
    let _ = cr.fill();

    // Crisp Apple double eighth-note glyph centered in white
    let s = w.min(h) / 58.0;
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.94);
    let cx = w * 0.5;
    let cy = h * 0.5;

    // Top angled beam
    cr.new_sub_path();
    cr.move_to(cx - 5.5 * s, cy - 10.5 * s);
    cr.line_to(cx + 10.5 * s, cy - 13.5 * s);
    cr.line_to(cx + 10.5 * s, cy - 8.2 * s);
    cr.line_to(cx - 5.5 * s, cy - 5.2 * s);
    cr.close_path();
    let _ = cr.fill();

    // Two stems
    cr.set_line_width(2.7 * s);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.move_to(cx - 4.3 * s, cy - 8.0 * s);
    cr.line_to(cx - 4.3 * s, cy + 9.0 * s);
    cr.move_to(cx + 9.2 * s, cy - 11.0 * s);
    cr.line_to(cx + 9.2 * s, cy + 6.2 * s);
    let _ = cr.stroke();

    // Two crisp note heads
    cr.arc(cx - 8.2 * s, cy + 9.4 * s, 4.3 * s, 0.0, TAU);
    let _ = cr.fill();
    cr.arc(cx + 5.3 * s, cy + 6.6 * s, 4.3 * s, 0.0, TAU);
    let _ = cr.fill();
}

/// Draws Apple iOS green-gradient Charging battery icon (exact match to Image 2 Card 1!).
pub fn draw_apple_battery_glyph(cr: &cairo::Context, w: f64, h: f64, pct: f64, charging: bool) {
    cr.set_antialias(cairo::Antialias::Best);
    let bw = (w - 5.0).max(18.0);
    let bh = (h - 2.0).max(10.0);
    let x = 0.5;
    let y = (h - bh) * 0.5;
    let r = 4.2;

    // Dark translucent battery body
    rounded_rect_path(cr, x, y, bw, bh, r);
    cr.set_source_rgba(0.24, 0.36, 0.28, 0.55);
    let _ = cr.fill();

    // Bright green-teal gradient charge fill
    let frac = (pct / 100.0).clamp(0.08, 1.0);
    let fill_w = (bw * frac).max(r * 1.6);
    let _ = cr.save();
    rounded_rect_path(cr, x, y, bw, bh, r);
    cr.clip();
    let grad = cairo::LinearGradient::new(x, y, x + bw, y);
    if charging || pct >= 30.0 {
        grad.add_color_stop_rgba(0.0, 0.14, 0.86, 0.72, 1.0); // Apple teal-mint
        grad.add_color_stop_rgba(1.0, 0.22, 0.88, 0.42, 1.0); // Apple green
    } else if pct >= 15.0 {
        grad.add_color_stop_rgba(0.0, 1.0, 0.74, 0.14, 1.0);
        grad.add_color_stop_rgba(1.0, 1.0, 0.62, 0.04, 1.0);
    } else {
        grad.add_color_stop_rgba(0.0, 1.0, 0.30, 0.26, 1.0);
        grad.add_color_stop_rgba(1.0, 1.0, 0.23, 0.19, 1.0);
    }
    rounded_rect_path(cr, x, y, fill_w, bh, r);
    let _ = cr.set_source(&grad);
    let _ = cr.fill();
    let _ = cr.restore();

    // Terminal nub on right
    let nub_h = bh * 0.38;
    let nub_y = (h - nub_h) * 0.5;
    rounded_rect_path(cr, x + bw + 1.6, nub_y, 2.4, nub_h, 1.2);
    cr.set_source_rgba(0.22, 0.64, 0.38, 0.75);
    let _ = cr.fill();
}

/// Draws Apple iOS Silent pill badge (vibrant coral-red pill `#FF3B30` + crisp white bell-slash!).
pub fn draw_apple_silent_pill(cr: &cairo::Context, w: f64, h: f64, silent: bool) {
    cr.set_antialias(cairo::Antialias::Best);
    let r = h * 0.5;
    rounded_rect_path(cr, 0.0, 0.0, w, h, r);
    if silent {
        cr.set_source_rgba(1.0, 0.231, 0.188, 1.0); // Apple #FF3B30
    } else {
        cr.set_source_rgba(0.22, 0.22, 0.24, 1.0);
    }
    let _ = cr.fill();

    // White bell silhouette centered inside pill
    let cx = w * 0.5;
    let cy = h * 0.5;
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
    // Bell dome
    cr.new_sub_path();
    cr.arc(cx, cy - 1.5, 5.4, PI, 0.0);
    cr.line_to(cx + 6.4, cy + 4.8);
    cr.line_to(cx - 6.4, cy + 4.8);
    cr.close_path();
    let _ = cr.fill();
    // Bell clapper
    cr.arc(cx, cy + 6.5, 2.0, 0.0, PI);
    let _ = cr.fill();

    if silent {
        // Bold diagonal slash with crisp red cutout behind it
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_width(3.6);
        cr.set_source_rgba(1.0, 0.231, 0.188, 1.0);
        cr.move_to(cx - 7.4, cy - 6.8);
        cr.line_to(cx + 7.4, cy + 7.4);
        let _ = cr.stroke();

        cr.set_line_width(2.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
        cr.move_to(cx - 7.4, cy - 6.8);
        cr.line_to(cx + 7.4, cy + 7.4);
        let _ = cr.stroke();
    }
}

/// Closed top-flush MacBook notch path with G1-continuous concave top ears + rounded bottom.
pub fn notch_ear_path(
    cr: &cairo::Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    ear_r: f64,
    bottom_r: f64,
) {
    let er = ear_r.clamp(0.0, (h * 0.48).min(w * 0.25));
    let br = bottom_r.clamp(0.0, (h - er).max(0.0).min(w * 0.5));

    cr.new_sub_path();
    cr.move_to(x - er, y);
    cr.line_to(x + w + er, y);
    if er > 0.5 {
        cr.arc_negative(x + w + er, y + er, er, -PI * 0.5, -PI);
    }
    cr.line_to(x + w, y + h - br);
    if br > 0.5 {
        cr.arc(x + w - br, y + h - br, br, 0.0, PI * 0.5);
    }
    cr.line_to(x + br, y + h);
    if br > 0.5 {
        cr.arc(x + br, y + h - br, br, PI * 0.5, PI);
    }
    cr.line_to(x, y + er);
    if er > 0.5 {
        cr.arc_negative(x - er, y + er, er, 0.0, -PI * 0.5);
    }
    cr.close_path();
}

/// Open stroke path tracing ONLY the left ear, left wall, bottom curve, right wall, and right ear.
pub fn notch_ear_open_rim_path(
    cr: &cairo::Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    ear_r: f64,
    bottom_r: f64,
) {
    let er = ear_r.clamp(0.0, (h * 0.48).min(w * 0.25));
    let br = bottom_r.clamp(0.0, (h - er).max(0.0).min(w * 0.5));

    cr.new_sub_path();
    cr.move_to(x + w + er, y + 0.5);
    if er > 0.5 {
        cr.arc_negative(x + w + er, y + er, er, -PI * 0.5, -PI);
    }
    cr.line_to(x + w, y + h - br);
    if br > 0.5 {
        cr.arc(x + w - br, y + h - br, br, 0.0, PI * 0.5);
    }
    cr.line_to(x + br, y + h);
    if br > 0.5 {
        cr.arc(x + br, y + h - br, br, PI * 0.5, PI);
    }
    cr.line_to(x, y + er);
    if er > 0.5 {
        cr.arc_negative(x - er, y + er, er, 0.0, -PI * 0.5);
    }
}

/// Draws Apple Dynamic Island live audio waveform visualizer bars colored by album-art palette.
pub fn draw_audio_waveform(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    bar_count: usize,
    playing: bool,
    phase: f64,
    palette: NeonPalette,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let n = bar_count.max(3);
    let gap = 2.6;
    let total_gap = gap * (n.saturating_sub(1) as f64);
    let bar_w = ((w - total_gap) / (n as f64)).clamp(2.2, 3.4);
    let total_w = bar_w * (n as f64) + total_gap;
    let start_x = (w - total_w) * 0.5;

    let grad = cairo::LinearGradient::new(start_x, 0.0, start_x + total_w, h);
    grad.add_color_stop_rgba(0.0, palette.primary.r, palette.primary.g, palette.primary.b, 0.98);
    grad.add_color_stop_rgba(
        0.52,
        palette.secondary.r,
        palette.secondary.g,
        palette.secondary.b,
        0.98,
    );
    grad.add_color_stop_rgba(
        1.0,
        palette.tertiary.r,
        palette.tertiary.g,
        palette.tertiary.b,
        0.98,
    );

    let freqs: [f64; 6] = [2.9, 4.1, 3.4, 4.7, 3.1, 3.8];
    let offsets: [f64; 6] = [0.0, 1.3, 2.5, 0.7, 3.6, 2.0];

    for i in 0..n {
        let bx = start_x + (i as f64) * (bar_w + gap);
        let frac = if playing {
            let f1 = freqs[i % freqs.len()];
            let off = offsets[i % offsets.len()];
            let wave = 0.5
                + 0.34 * (phase * f1 + off).sin()
                + 0.16 * (phase * (f1 * 1.7) - off * 0.6).cos();
            wave.clamp(0.22, 0.96)
        } else {
            let center = (n as f64 - 1.0) * 0.5;
            let dist = ((i as f64) - center).abs() / center.max(1.0);
            (0.24 - 0.06 * dist).clamp(0.16, 0.26)
        };
        let bh = (h * frac).clamp(bar_w, h);
        let by = (h - bh) * 0.5;
        rounded_rect_path(cr, bx, by, bar_w, bh, bar_w * 0.5);
        if playing {
            let _ = cr.set_source(&grad);
        } else {
            cr.set_source_rgba(0.56, 0.56, 0.58, 0.72);
        }
        let _ = cr.fill();
    }
}

/// Draws Apple iOS Now Playing progress scrubber (crisp `#3A3A3C` track + pure white / subtle tinted fill).
pub fn draw_media_progress_bar(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    fraction: f64,
    hovered: bool,
    palette: NeonPalette,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let frac = fraction.clamp(0.0, 1.0);
    let track_h = if hovered { 6.5 } else { 5.0 };
    let pad_x = 4.0;
    let track_w = (w - pad_x * 2.0).max(8.0);
    let ty = (h - track_h) * 0.5;
    let r = track_h * 0.5;

    // Clean Apple iOS #3A3A3C background rail
    rounded_rect_path(cr, pad_x, ty, track_w, track_h, r);
    cr.set_source_rgba(1.0, 1.0, 1.0, if hovered { 0.28 } else { 0.22 });
    let _ = cr.fill();

    // Crisp pure white fill (with subtle bright album-art tint on hover!)
    let fill_w = (track_w * frac).max(if frac > 0.0 { track_h } else { 0.0 });
    if fill_w > 0.5 {
        rounded_rect_path(cr, pad_x, ty, fill_w, track_h, r);
        if hovered {
            let grad = cairo::LinearGradient::new(pad_x, ty, pad_x + track_w, ty);
            grad.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.98);
            grad.add_color_stop_rgba(
                1.0,
                palette.primary.r * 0.45 + 0.55,
                palette.primary.g * 0.45 + 0.55,
                palette.primary.b * 0.45 + 0.55,
                1.0,
            );
            let _ = cr.set_source(&grad);
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.96);
        }
        let _ = cr.fill();

        if hovered {
            let knob_x = pad_x + fill_w.clamp(r, track_w);
            let knob_y = h * 0.5;
            cr.arc(knob_x, knob_y, 5.0, 0.0, TAU);
            cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            let _ = cr.fill();
        }
    }
}

/// Draws compact circular countdown ring (amber Focus / emerald Break).
pub fn draw_timer_ring(cr: &cairo::Context, w: f64, h: f64, remaining_frac: f64, focus_mode: bool) {
    cr.set_antialias(cairo::Antialias::Best);
    let cx = w * 0.5;
    let cy = h * 0.5;
    let r = (w.min(h) * 0.5 - 2.4).max(4.0);
    let (cr_r, cr_g, cr_b) = if focus_mode {
        (1.0, 0.62, 0.04) // Apple Orange #FF9F0A
    } else {
        (0.19, 0.82, 0.35) // Apple Green #30D158
    };

    cr.set_line_width(2.6);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.arc(cx, cy, r, 0.0, TAU);
    cr.set_source_rgba(cr_r, cr_g, cr_b, 0.22);
    let _ = cr.stroke();

    let sweep = remaining_frac.clamp(0.02, 1.0) * TAU;
    cr.arc(cx, cy, r, -PI * 0.5, -PI * 0.5 + sweep);
    cr.set_source_rgba(cr_r, cr_g, cr_b, 0.98);
    let _ = cr.stroke();
}

/// Draws interactive scrubbable horizontal tick-mark ruler (`5..60` min).
pub fn draw_timer_ruler(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    selected_mins: u32,
    focus_mode: bool,
    hovered: bool,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let mut fo = cairo::FontOptions::new().unwrap();
    fo.set_antialias(cairo::Antialias::Subpixel);
    fo.set_subpixel_order(cairo::SubpixelOrder::Rgb);
    fo.set_hint_style(cairo::HintStyle::Slight);
    fo.set_hint_metrics(cairo::HintMetrics::On);
    cr.set_font_options(&fo);
    cr.select_font_face("Noto Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(11.0);

    let (ar, ag, ab) = if focus_mode {
        (1.0, 0.62, 0.04)
    } else {
        (0.19, 0.82, 0.35)
    };

    let min_m = 5.0_f64;
    let max_m = 60.0_f64;
    let pad_x = 14.0;
    let span = (w - pad_x * 2.0).max(20.0);
    let clamped = f64::from(selected_mins.clamp(1, 90));
    let marker_frac = ((clamped - min_m) / (max_m - min_m)).clamp(0.0, 1.0);
    let marker_x = pad_x + span * marker_frac;

    let label_y = 11.0_f64;
    let tick_top = 18.0;
    let tick_bottom = h - 4.0;
    let tick_h = (tick_bottom - tick_top).max(10.0);

    for m in 5..=60 {
        let frac = (f64::from(m) - min_m) / (max_m - min_m);
        let tx = pad_x + span * frac;
        let is_major = m % 5 == 0;
        let active = f64::from(m) <= clamped;

        if is_major && m >= 10 {
            let txt = m.to_string();
            if let Ok(ext) = cr.text_extents(&txt) {
                cr.move_to((tx - ext.width() * 0.5).round(), label_y.round());
                if active {
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.92);
                } else {
                    cr.set_source_rgba(0.68, 0.68, 0.70, 0.68);
                }
                let _ = cr.show_text(&txt);
            }
        }

        let len = if is_major { tick_h * 0.82 } else { tick_h * 0.44 };
        let y0 = tick_bottom - len;
        cr.set_line_width(if is_major { 1.8 } else { 1.1 });
        cr.set_line_cap(cairo::LineCap::Round);
        if active {
            cr.set_source_rgba(ar, ag, ab, if is_major { 0.96 } else { 0.65 });
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, if is_major { 0.28 } else { 0.14 });
        }
        cr.move_to(tx, y0);
        cr.line_to(tx, tick_bottom);
        let _ = cr.stroke();
    }

    let needle_w = if hovered { 3.2 } else { 2.6 };
    rounded_rect_path(
        cr,
        marker_x - needle_w * 0.5,
        tick_top - 2.0,
        needle_w,
        tick_h + 4.0,
        needle_w * 0.5,
    );
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
    let _ = cr.fill();
}

/// Draws crisp vertical bar CPU history graph.
pub fn draw_cpu_bars(cr: &cairo::Context, w: f64, h: f64, samples: &[f32]) {
    cr.set_antialias(cairo::Antialias::Best);
    let n = 22usize;
    let gap = 2.0;
    let bar_w = ((w - gap * (n as f64 - 1.0)) / n as f64).clamp(1.6, 5.0);
    let start = samples.len().saturating_sub(n);
    let slice = &samples[start..];

    for i in 0..n {
        let val = slice
            .get(i.saturating_sub(n.saturating_sub(slice.len())))
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 100.0)
            / 100.0;
        let bx = (i as f64) * (bar_w + gap);
        let bh = (h * (0.08 + 0.92 * f64::from(val))).clamp(2.0, h);
        let by = h - bh;
        rounded_rect_path(cr, bx, by, bar_w, bh, bar_w * 0.45);
        if val > 0.78 {
            cr.set_source_rgba(1.0, 0.27, 0.23, 0.95);
        } else if val > 0.45 {
            cr.set_source_rgba(0.04, 0.52, 1.0, 0.90);
        } else {
            cr.set_source_rgba(0.56, 0.56, 0.58, 0.70);
        }
        let _ = cr.fill();
    }
}

/// Draws smooth dual-line network throughput sparkline.
pub fn draw_network_sparkline(cr: &cairo::Context, w: f64, h: f64, rx: &[u64], tx: &[u64]) {
    cr.set_antialias(cairo::Antialias::Best);
    let max_val = rx
        .iter()
        .chain(tx.iter())
        .copied()
        .max()
        .unwrap_or(1024)
        .max(1024) as f64;

    draw_spark_series(cr, w, h, tx, max_val, (0.68, 0.48, 0.98, 0.55), false);
    draw_spark_series(cr, w, h, rx, max_val, (0.19, 0.82, 0.35, 0.95), true);
}

fn draw_spark_series(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    data: &[u64],
    max_val: f64,
    rgba: (f64, f64, f64, f64),
    fill_area: bool,
) {
    if data.len() < 2 {
        cr.set_source_rgba(rgba.0, rgba.1, rgba.2, 0.35);
        cr.set_line_width(1.2);
        cr.move_to(0.0, h - 2.0);
        cr.line_to(w, h - 2.0);
        let _ = cr.stroke();
        return;
    }
    let step = w / ((data.len() - 1) as f64);
    cr.new_path();
    for (i, &v) in data.iter().enumerate() {
        let px = (i as f64) * step;
        let norm = (v as f64 / max_val).clamp(0.0, 1.0);
        let py = (h - 2.5) - norm * (h - 5.0);
        if i == 0 {
            cr.move_to(px, py);
        } else {
            cr.line_to(px, py);
        }
    }
    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.set_line_width(1.5);
    cr.set_line_join(cairo::LineJoin::Round);
    let _ = cr.stroke_preserve();

    if fill_area {
        cr.line_to(w, h);
        cr.line_to(0.0, h);
        cr.close_path();
        let g = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
        g.add_color_stop_rgba(0.0, rgba.0, rgba.1, rgba.2, 0.22);
        g.add_color_stop_rgba(1.0, rgba.0, rgba.1, rgba.2, 0.0);
        let _ = cr.set_source(&g);
        let _ = cr.fill();
    } else {
        cr.new_path();
    }
}

/// Draws Apple-style circular battery ring gauge.
pub fn draw_battery_ring(cr: &cairo::Context, w: f64, h: f64, pct: f64, charging: bool) {
    cr.set_antialias(cairo::Antialias::Best);
    let mut fo = cairo::FontOptions::new().unwrap();
    fo.set_antialias(cairo::Antialias::Subpixel);
    fo.set_subpixel_order(cairo::SubpixelOrder::Rgb);
    fo.set_hint_style(cairo::HintStyle::Slight);
    fo.set_hint_metrics(cairo::HintMetrics::On);
    cr.set_font_options(&fo);

    let cx = w * 0.5;
    let cy = h * 0.5;
    let r = (w.min(h) * 0.5 - 4.0).max(10.0);
    let p = pct.clamp(0.0, 100.0);

    let (cr_r, cr_g, cr_b) = if charging || p >= 40.0 {
        (0.19, 0.82, 0.35) // Apple #30D158
    } else if p >= 20.0 {
        (1.0, 0.62, 0.04)
    } else {
        (1.0, 0.23, 0.19)
    };

    cr.set_line_width(3.4);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.arc(cx, cy, r, 0.0, TAU);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.16);
    let _ = cr.stroke();

    let sweep = (p / 100.0) * TAU;
    cr.arc(cx, cy, r, -PI * 0.5, -PI * 0.5 + sweep);
    cr.set_source_rgba(cr_r, cr_g, cr_b, 0.98);
    let _ = cr.stroke();

    let txt = format!("{}", p.round() as i32);
    cr.select_font_face("Noto Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size((r * 0.72).clamp(11.0, 14.0).round());
    if let Ok(ext) = cr.text_extents(&txt) {
        cr.move_to(
            (cx - ext.width() * 0.5 - ext.x_bearing()).round(),
            (cy + ext.height() * 0.5).round(),
        );
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
        let _ = cr.show_text(&txt);
    }
}

/// Draws sleek Volume / Brightness horizontal HUD bar.
pub fn draw_hud_bar(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    fraction: f64,
    is_brightness: bool,
    muted: bool,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let track_h = 5.5;
    let ty = (h - track_h) * 0.5;
    let r = track_h * 0.5;
    rounded_rect_path(cr, 0.0, ty, w, track_h, r);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.22);
    let _ = cr.fill();

    let frac = if muted { 0.0 } else { fraction.clamp(0.0, 1.0) };
    let fw = (w * frac).max(if frac > 0.0 { track_h } else { 0.0 });
    if fw > 0.5 {
        rounded_rect_path(cr, 0.0, ty, fw, track_h, r);
        if is_brightness {
            cr.set_source_rgba(1.0, 0.84, 0.24, 1.0);
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.98);
        }
        let _ = cr.fill();
    }
}

/// Draws a subtle, organic dark-AMOLED ambient gradient tinted by the active song's album-art
/// palette directly inside the clipped black notch surface, drifting along non-repeating 2D paths.
#[allow(clippy::too_many_arguments)]
fn draw_amoled_media_sweep(
    cr: &cairo::Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    er: f64,
    neon_phase: f64,
    palette: NeonPalette,
    neon: f64,
) {
    let intensity = neon.clamp(0.0, 1.0);
    if intensity <= 0.005 {
        return;
    }

    let t = neon_phase;
    let span = w.max(h).max(160.0);

    // Three softly breathing radial color clouds wandering along mutually-prime harmonic 2D paths
    // (organic, random-feeling AMOLED mesh motion with zero linear sweep or obvious loop).
    let blobs = [
        (
            palette.primary,
            x + w * (0.50 + 0.36 * (t * 0.41 + 0.3).sin() * (t * 0.17 + 1.1).cos() + 0.12 * (t * 0.73).cos()),
            y + h * (0.48 + 0.34 * (t * 0.33 + 1.7).cos() + 0.14 * (t * 0.67 + 0.5).sin()),
            span * (0.52 + 0.09 * (t * 0.53 + 0.8).sin()),
            (0.34 + 0.07 * (t * 0.47).sin()) * intensity,
        ),
        (
            palette.secondary,
            x + w * (0.50 + 0.38 * (t * 0.29 + 2.4).cos() + 0.14 * (t * 0.61 + 0.9).sin()),
            y + h * (0.52 + 0.32 * (t * 0.47 + 0.4).sin() * (t * 0.23 + 2.0).cos()),
            span * (0.48 + 0.08 * (t * 0.39 + 2.1).cos()),
            (0.27 + 0.06 * (t * 0.59 + 1.4).cos()) * intensity,
        ),
        (
            palette.tertiary,
            x + w * (0.50 + 0.34 * (t * 0.53 + 4.1).sin() + 0.15 * (t * 0.23 + 1.5).cos()),
            y + h * (0.46 + 0.34 * (t * 0.37 + 3.0).cos() + 0.12 * (t * 0.71 + 0.2).sin()),
            span * (0.44 + 0.07 * (t * 0.43 + 3.2).sin()),
            (0.21 + 0.05 * (t * 0.31 + 2.6).sin()) * intensity,
        ),
    ];

    for (rgb, cx, cy, rad, alpha) in blobs {
        let g = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, rad.max(48.0));
        g.add_color_stop_rgba(0.00, rgb.r, rgb.g, rgb.b, alpha);
        g.add_color_stop_rgba(0.48, rgb.r, rgb.g, rgb.b, alpha * 0.52);
        g.add_color_stop_rgba(1.00, rgb.r, rgb.g, rgb.b, 0.0);
        let _ = cr.set_source(&g);
        cr.rectangle(x - er - 2.0, y - 2.0, w + er * 2.0 + 4.0, h + 4.0);
        let _ = cr.fill();
    }

    // Gentle AMOLED depth vignette so the glow dissolves cleanly into pure black near the edges.
    let vig = cairo::LinearGradient::new(x, y, x, y + h);
    vig.add_color_stop_rgba(0.0, 0.0, 0.0, 0.0, 0.10);
    vig.add_color_stop_rgba(0.35, 0.0, 0.0, 0.0, 0.0);
    vig.add_color_stop_rgba(0.78, 0.0, 0.0, 0.0, 0.08);
    vig.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.22);
    let _ = cr.set_source(&vig);
    cr.rectangle(x - er - 2.0, y - 2.0, w + er * 2.0 + 4.0, h + 4.0);
    let _ = cr.fill();
}

pub fn rounded_rect_path(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.clamp(0.0, (w * 0.5).min(h * 0.5));
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI * 0.5, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI * 0.5);
    cr.arc(x + r, y + h - r, r, PI * 0.5, PI);
    cr.arc(x + r, y + r, r, PI, PI * 1.5);
    cr.close_path();
}

// =============================================================================
// Anti-Aliased Vector Module Icons, App Badges & Circular Pin/Action Buttons
// =============================================================================

/// Crisp vector UI icons rendered directly via Cairo (`Antialias::Best`) with zero bitmap scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiVectorIcon {
    Pin { pinned: bool },
    Close,
    Check,
    Plus,
    Refresh,
    Volume { muted: bool },
    Brightness,
    Screenshot,
    Record,
    Folder,
    ImageFile,
    ArchiveFile,
    DocFile,
}

/// Draws a band-free Gaussian drop shadow + pure `#000000` body + crisp 1px rim for the floating bottom dock pill.
pub fn draw_dock_pill_backdrop(
    cr: &cairo::Context,
    canvas_w: f64,
    canvas_h: f64,
    appearance: NotchAppearance,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let pad_x = 12.0;
    let pad_top = 4.0;
    let pad_bot = 14.0;
    let w = (canvas_w - pad_x * 2.0).max(40.0).round();
    let h = (canvas_h - pad_top - pad_bot).max(24.0).round();
    let x = pad_x;
    let y = pad_top;
    let r = (h * 0.5).clamp(12.0, 20.0);

    for i in (1..=14).rev() {
        let t = f64::from(i) / 14.0;
        let spread = 0.8 + 14.0 * t * t;
        let dy = 0.8 + 5.5 * t;
        let alpha = 0.016 * (-2.5 * t * t).exp();
        rounded_rect_path(
            cr,
            x - spread * 0.5,
            y + dy - spread * 0.25,
            w + spread,
            h + spread * 0.65,
            r + spread * 0.45,
        );
        cr.set_source_rgba(0.0, 0.0, 0.0, alpha);
        let _ = cr.fill();
    }

    rounded_rect_path(cr, x, y, w, h, r);
    match appearance {
        NotchAppearance::LiquidGlass => cr.set_source_rgba(0.11, 0.15, 0.22, 0.44),
        _ => cr.set_source_rgba(0.0, 0.0, 0.0, 1.0),
    }
    let _ = cr.fill();

    rounded_rect_path(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, (r - 0.5).max(8.0));
    if matches!(appearance, NotchAppearance::LiquidGlass) {
        let rim = cairo::LinearGradient::new(x, y, x + w * 0.35, y + h);
        rim.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.72);
        rim.add_color_stop_rgba(0.32, 1.0, 1.0, 1.0, 0.22);
        rim.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.40);
        let _ = cr.set_source(&rim);
        cr.set_line_width(1.2);
    } else {
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.set_line_width(1.0);
    }
    let _ = cr.stroke();
}

/// Returns the signature Apple vibrant badge color for each module.
pub const fn module_badge_rgb(module: NotchModule) -> Rgb {
    match module {
        NotchModule::Home => Rgb::new(0.04, 0.52, 1.00),      // #0A84FF Blue
        NotchModule::Media => Rgb::new(1.00, 0.18, 0.33),     // #FF2D55 Pink
        NotchModule::Timer => Rgb::new(1.00, 0.62, 0.04),     // #FF9F0A Amber
        NotchModule::Clipboard => Rgb::new(0.37, 0.36, 0.90), // #5E5CE6 Indigo
        NotchModule::Files => Rgb::new(0.20, 0.68, 0.90),     // #32ADE6 Sky
        NotchModule::Notes => Rgb::new(0.95, 0.72, 0.06),     // #F3B80F Gold
        NotchModule::Calendar => Rgb::new(1.00, 0.23, 0.19),  // #FF3B30 Coral Red
        NotchModule::Tools => Rgb::new(0.56, 0.56, 0.58),     // #8E8E93 Graphite
        NotchModule::Stats => Rgb::new(0.19, 0.69, 0.78),     // #30B0C7 Teal
        NotchModule::Battery => Rgb::new(0.19, 0.82, 0.35),   // #30D158 Green
        NotchModule::Awake => Rgb::new(1.00, 0.58, 0.00),     // #FF9500 Orange
        NotchModule::Hud => Rgb::new(0.39, 0.82, 1.00),       // #64D2FF Cyan
        NotchModule::Store => Rgb::new(0.04, 0.52, 1.00),
    }
}

/// Draws a sleek rounded-square macOS/iOS app icon badge with a crisp white vector glyph inside.
pub fn draw_module_badge_tile(cr: &cairo::Context, w: f64, h: f64, module: NotchModule) {
    cr.set_antialias(cairo::Antialias::Best);
    let r = (w.min(h) * 0.28).clamp(6.0, 10.0);
    let base = module_badge_rgb(module);

    let grad = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
    grad.add_color_stop_rgba(
        0.0,
        (base.r * 1.12).min(1.0),
        (base.g * 1.12).min(1.0),
        (base.b * 1.12).min(1.0),
        1.0,
    );
    grad.add_color_stop_rgba(1.0, base.r * 0.86, base.g * 0.86, base.b * 0.86, 1.0);

    rounded_rect_path(cr, 0.0, 0.0, w, h, r);
    let _ = cr.set_source(&grad);
    let _ = cr.fill();

    // Subtle specular hairline border
    rounded_rect_path(cr, 0.5, 0.5, w - 1.0, h - 1.0, (r - 0.5).max(4.0));
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.18);
    cr.set_line_width(1.0);
    let _ = cr.stroke();

    let inner = (w.min(h) * 0.64).round();
    let ox = ((w - inner) * 0.5).round();
    let oy = ((h - inner) * 0.5).round();
    let _ = cr.save();
    cr.translate(ox, oy);
    draw_module_vector_icon(cr, inner, inner, module, (1.0, 1.0, 1.0, 0.98));
    let _ = cr.restore();
}

/// Draws a crisp circular Pin / Unpin toggle button (`Antialias::Best`).
pub fn draw_round_pin_button(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    pinned: bool,
    hovered: bool,
    accent: Rgb,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let cx = w * 0.5;
    let cy = h * 0.5;
    let r = (w.min(h) * 0.5 - 0.5).max(8.0);

    cr.new_sub_path();
    cr.arc(cx, cy, r, 0.0, TAU);
    if pinned {
        let boost = if hovered { 1.12 } else { 1.0 };
        cr.set_source_rgba(
            (accent.r * boost).min(1.0),
            (accent.g * boost).min(1.0),
            (accent.b * boost).min(1.0),
            1.0,
        );
        let _ = cr.fill();
    } else {
        let v = if hovered { 0.25 } else { 0.18 };
        cr.set_source_rgba(v, v, v + 0.015, 1.0);
        let _ = cr.fill();

        cr.new_sub_path();
        cr.arc(cx, cy, r - 0.5, 0.0, TAU);
        cr.set_source_rgba(1.0, 1.0, 1.0, if hovered { 0.20 } else { 0.10 });
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    }

    let fg = if pinned || hovered {
        (1.0, 1.0, 1.0, 0.98)
    } else {
        (0.72, 0.72, 0.75, 0.95)
    };
    let inner = (w.min(h) * 0.64).round();
    let ox = ((w - inner) * 0.5).round();
    let oy = ((h - inner) * 0.5).round();
    let _ = cr.save();
    cr.translate(ox, oy);
    draw_ui_vector_icon(cr, inner, inner, UiVectorIcon::Pin { pinned }, fg);
    let _ = cr.restore();
}

/// Draws a crisp circular icon button (e.g. Close `×`, Check `✓`, Remove badge).
pub fn draw_round_circle_button(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    bg: (f64, f64, f64, f64),
    fg: (f64, f64, f64, f64),
    icon: UiVectorIcon,
) {
    cr.set_antialias(cairo::Antialias::Best);
    let cx = w * 0.5;
    let cy = h * 0.5;
    let r = (w.min(h) * 0.5 - 0.5).max(6.0);

    cr.new_sub_path();
    cr.arc(cx, cy, r, 0.0, TAU);
    cr.set_source_rgba(bg.0, bg.1, bg.2, bg.3);
    let _ = cr.fill();

    let inner = (w.min(h) * 0.62).round();
    let ox = ((w - inner) * 0.5).round();
    let oy = ((h - inner) * 0.5).round();
    let _ = cr.save();
    cr.translate(ox, oy);
    draw_ui_vector_icon(cr, inner, inner, icon, fg);
    let _ = cr.restore();
}

/// Draws razor-sharp anti-aliased SF-style vector glyphs for all 13 `NotchModule`s.
/// Engineered for 1× (`1920×1080`) clarity: fills ~80% of the canvas with bold silhouettes & clean gaps.
pub fn draw_module_vector_icon(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    module: NotchModule,
    rgba: (f64, f64, f64, f64),
) {
    cr.set_antialias(cairo::Antialias::Best);
    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);

    let cx = w * 0.5;
    let cy = h * 0.5;
    // Scale onto a 16-unit icon grid so coordinates in [-6.5, +6.5] fill ~81% of `w × h`!
    let s = (w.min(h) / 16.0).clamp(0.75, 2.5);
    let lw = (1.55 * s).clamp(1.4, 2.8);
    cr.set_line_width(lw);

    match module {
        NotchModule::Home => {
            // Bold SF-style filled house with crisp arched roof & bottom cutout door
            cr.new_sub_path();
            cr.move_to(cx, cy - 6.2 * s);
            cr.line_to(cx + 6.4 * s, cy - 0.4 * s);
            cr.line_to(cx + 4.8 * s, cy - 0.4 * s);
            cr.line_to(cx + 4.8 * s, cy + 5.6 * s);
            cr.line_to(cx + 1.5 * s, cy + 5.6 * s);
            cr.line_to(cx + 1.5 * s, cy + 1.6 * s);
            cr.line_to(cx - 1.5 * s, cy + 1.6 * s);
            cr.line_to(cx - 1.5 * s, cy + 5.6 * s);
            cr.line_to(cx - 4.8 * s, cy + 5.6 * s);
            cr.line_to(cx - 4.8 * s, cy - 0.4 * s);
            cr.line_to(cx - 6.4 * s, cy - 0.4 * s);
            cr.close_path();
            let _ = cr.fill();
        }
        NotchModule::Media => {
            // Bold Apple Music double eighth-note silhouette
            cr.new_sub_path();
            cr.move_to(cx - 2.4 * s, cy - 4.8 * s);
            cr.line_to(cx + 5.4 * s, cy - 6.4 * s);
            cr.line_to(cx + 5.4 * s, cy - 3.6 * s);
            cr.line_to(cx - 2.4 * s, cy - 2.0 * s);
            cr.close_path();
            let _ = cr.fill();

            cr.set_line_width((1.7 * s).max(1.6));
            cr.move_to(cx - 1.6 * s, cy - 3.6 * s);
            cr.line_to(cx - 1.6 * s, cy + 4.2 * s);
            cr.move_to(cx + 4.6 * s, cy - 5.0 * s);
            cr.line_to(cx + 4.6 * s, cy + 2.8 * s);
            let _ = cr.stroke();

            cr.new_sub_path();
            cr.arc(cx - 3.6 * s, cy + 4.4 * s, 2.2 * s, 0.0, TAU);
            let _ = cr.fill();
            cr.new_sub_path();
            cr.arc(cx + 2.6 * s, cy + 3.0 * s, 2.2 * s, 0.0, TAU);
            let _ = cr.fill();
        }
        NotchModule::Timer => {
            // Crisp stopwatch ring + top crown + bold dial hands
            cr.set_line_width((1.65 * s).max(1.55));
            cr.new_sub_path();
            cr.arc(cx, cy + 1.0 * s, 5.2 * s, 0.0, TAU);
            let _ = cr.stroke();
            rounded_rect_path(cr, cx - 2.2 * s, cy - 6.6 * s, 4.4 * s, 1.7 * s, 0.85 * s);
            let _ = cr.fill();
            cr.move_to(cx, cy + 1.0 * s);
            cr.line_to(cx, cy - 1.8 * s);
            cr.move_to(cx, cy + 1.0 * s);
            cr.line_to(cx + 2.4 * s, cy + 1.0 * s);
            let _ = cr.stroke();
        }
        NotchModule::Clipboard => {
            // Crisp rounded clipboard with bold top clasp & two clean lines
            rounded_rect_path(cr, cx - 4.8 * s, cy - 4.4 * s, 9.6 * s, 10.6 * s, 2.0 * s);
            let _ = cr.stroke();
            rounded_rect_path(cr, cx - 2.6 * s, cy - 6.3 * s, 5.2 * s, 3.0 * s, 1.3 * s);
            let _ = cr.fill();
            cr.set_line_width((1.45 * s).max(1.4));
            cr.move_to(cx - 2.2 * s, cy + 0.2 * s);
            cr.line_to(cx + 2.2 * s, cy + 0.2 * s);
            cr.move_to(cx - 2.2 * s, cy + 3.2 * s);
            cr.line_to(cx + 1.2 * s, cy + 3.2 * s);
            let _ = cr.stroke();
        }
        NotchModule::Files => {
            // Bold SF-style filled rounded folder silhouette (razor sharp at 16px!)
            let r = 1.6 * s;
            let left = cx - 6.2 * s;
            let right = cx + 6.2 * s;
            let top = cy - 4.8 * s;
            let tab_y = cy - 2.4 * s;
            let bot = cy + 5.0 * s;
            cr.new_sub_path();
            cr.move_to(left, top + r);
            cr.arc(left + r, top + r, r, PI, 1.5 * PI);
            cr.line_to(cx - 1.5 * s, top);
            cr.line_to(cx + 0.4 * s, tab_y);
            cr.line_to(right - r, tab_y);
            cr.arc(right - r, tab_y + r, r, -0.5 * PI, 0.0);
            cr.line_to(right, bot - r);
            cr.arc(right - r, bot - r, r, 0.0, 0.5 * PI);
            cr.line_to(left + r, bot);
            cr.arc(left + r, bot - r, r, 0.5 * PI, PI);
            cr.close_path();
            let _ = cr.fill();
        }
        NotchModule::Notes => {
            // Crisp rounded notepad + bold diagonal pencil
            rounded_rect_path(cr, cx - 5.4 * s, cy - 4.8 * s, 9.8 * s, 10.4 * s, 2.0 * s);
            let _ = cr.stroke();
            cr.set_line_width((1.8 * s).max(1.7));
            cr.move_to(cx - 1.0 * s, cy + 1.6 * s);
            cr.line_to(cx + 5.4 * s, cy - 4.8 * s);
            let _ = cr.stroke();
        }
        NotchModule::Calendar => {
            // Crisp calendar frame + solid top bar + date dots
            rounded_rect_path(cr, cx - 5.4 * s, cy - 4.6 * s, 10.8 * s, 10.4 * s, 2.0 * s);
            let _ = cr.stroke();
            rounded_rect_path(cr, cx - 5.2 * s, cy - 4.4 * s, 10.4 * s, 3.4 * s, 1.4 * s);
            let _ = cr.fill();
            cr.move_to(cx - 2.6 * s, cy - 6.2 * s);
            cr.line_to(cx - 2.6 * s, cy - 3.6 * s);
            cr.move_to(cx + 2.6 * s, cy - 6.2 * s);
            cr.line_to(cx + 2.6 * s, cy - 3.6 * s);
            let _ = cr.stroke();
            cr.new_sub_path();
            cr.arc(cx - 2.1 * s, cy + 2.2 * s, 1.15 * s, 0.0, TAU);
            let _ = cr.fill();
            cr.new_sub_path();
            cr.arc(cx + 2.1 * s, cy + 2.2 * s, 1.15 * s, 0.0, TAU);
            let _ = cr.fill();
        }
        NotchModule::Tools => {
            // Well-separated sleek horizontal control sliders with bold filled knobs
            cr.set_line_width((1.6 * s).max(1.5));
            cr.move_to(cx - 5.6 * s, cy - 3.2 * s);
            cr.line_to(cx + 5.6 * s, cy - 3.2 * s);
            cr.move_to(cx - 5.6 * s, cy + 3.2 * s);
            cr.line_to(cx + 5.6 * s, cy + 3.2 * s);
            let _ = cr.stroke();
            cr.new_sub_path();
            cr.arc(cx - 1.8 * s, cy - 3.2 * s, 2.2 * s, 0.0, TAU);
            let _ = cr.fill();
            cr.new_sub_path();
            cr.arc(cx + 2.0 * s, cy + 3.2 * s, 2.2 * s, 0.0, TAU);
            let _ = cr.fill();
        }
        NotchModule::Stats => {
            // Bold activity waveform pulse
            cr.set_line_width((1.75 * s).max(1.65));
            cr.new_sub_path();
            cr.move_to(cx - 6.4 * s, cy + 0.6 * s);
            cr.line_to(cx - 3.0 * s, cy + 0.6 * s);
            cr.line_to(cx - 1.1 * s, cy - 4.8 * s);
            cr.line_to(cx + 1.3 * s, cy + 4.8 * s);
            cr.line_to(cx + 3.2 * s, cy + 0.6 * s);
            cr.line_to(cx + 6.4 * s, cy + 0.6 * s);
            let _ = cr.stroke();
        }
        NotchModule::Battery => {
            // Bold rounded battery + terminal nub + charge bar
            rounded_rect_path(cr, cx - 6.2 * s, cy - 3.8 * s, 10.8 * s, 7.6 * s, 2.0 * s);
            let _ = cr.stroke();
            rounded_rect_path(cr, cx + 5.4 * s, cy - 1.7 * s, 1.8 * s, 3.4 * s, 0.9 * s);
            let _ = cr.fill();
            rounded_rect_path(cr, cx - 4.2 * s, cy - 1.8 * s, 6.6 * s, 3.6 * s, 1.0 * s);
            let _ = cr.fill();
        }
        NotchModule::Awake => {
            // Bold smooth crescent moon
            cr.new_sub_path();
            cr.arc(cx + 0.3 * s, cy, 5.4 * s, 0.22 * PI, 1.62 * PI);
            cr.arc_negative(cx + 3.1 * s, cy - 1.8 * s, 4.4 * s, 1.48 * PI, 0.36 * PI);
            cr.close_path();
            let _ = cr.fill();
        }
        NotchModule::Hud => {
            // Bold SF speaker cone + two well-spaced crisp sound waves
            cr.new_sub_path();
            cr.move_to(cx - 6.0 * s, cy - 2.2 * s);
            cr.line_to(cx - 3.4 * s, cy - 2.2 * s);
            cr.line_to(cx - 0.4 * s, cy - 5.0 * s);
            cr.line_to(cx - 0.4 * s, cy + 5.0 * s);
            cr.line_to(cx - 3.4 * s, cy + 2.2 * s);
            cr.line_to(cx - 6.0 * s, cy + 2.2 * s);
            cr.close_path();
            let _ = cr.fill();

            cr.set_line_width((1.55 * s).max(1.5));
            cr.new_sub_path();
            cr.arc(cx + 0.2 * s, cy, 3.1 * s, -0.32 * PI, 0.32 * PI);
            let _ = cr.stroke();
            cr.new_sub_path();
            cr.arc(cx + 0.2 * s, cy, 5.7 * s, -0.34 * PI, 0.34 * PI);
            let _ = cr.stroke();
        }
        NotchModule::Store => {
            // Bold 2×2 filled rounded squircle grid (crystal clear at 16px—zero 1px holes!)
            let box_s = 4.6 * s;
            let gap = 2.2 * s;
            let r = 1.4 * s;
            for (dx, dy) in [
                (-gap * 0.5 - box_s, -gap * 0.5 - box_s),
                (gap * 0.5, -gap * 0.5 - box_s),
                (-gap * 0.5 - box_s, gap * 0.5),
                (gap * 0.5, gap * 0.5),
            ] {
                rounded_rect_path(cr, cx + dx, cy + dy, box_s, box_s, r);
                let _ = cr.fill();
            }
        }
    }
}

/// Draws crisp anti-aliased UI glyphs (`Pin`, `Close`, `Check`, `Plus`, `Screenshot`, `Record`, file icons).
pub fn draw_ui_vector_icon(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    kind: UiVectorIcon,
    rgba: (f64, f64, f64, f64),
) {
    cr.set_antialias(cairo::Antialias::Best);
    cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);

    let cx = w * 0.5;
    let cy = h * 0.5;
    let s = (w.min(h) / 16.0).clamp(0.70, 2.6);
    let lw = (1.65 * s).clamp(1.45, 3.0);
    cr.set_line_width(lw);

    match kind {
        UiVectorIcon::Pin { pinned } => {
            // Clean 45°-angled macOS/iOS pushpin!
            let _ = cr.save();
            cr.translate(cx, cy);
            cr.rotate(PI * 0.22);
            rounded_rect_path(cr, -3.2 * s, -5.6 * s, 6.4 * s, 2.2 * s, 1.0 * s);
            if pinned {
                let _ = cr.fill();
            } else {
                let _ = cr.stroke();
            }
            cr.new_sub_path();
            cr.move_to(-2.1 * s, -3.4 * s);
            cr.line_to(-3.6 * s, 1.0 * s);
            cr.line_to(3.6 * s, 1.0 * s);
            cr.line_to(2.1 * s, -3.4 * s);
            cr.close_path();
            if pinned {
                let _ = cr.fill();
            } else {
                let _ = cr.stroke();
            }
            cr.move_to(0.0, 1.2 * s);
            cr.line_to(0.0, 5.8 * s);
            let _ = cr.stroke();
            let _ = cr.restore();
        }
        UiVectorIcon::Close => {
            let d = 3.8 * s;
            cr.move_to(cx - d, cy - d);
            cr.line_to(cx + d, cy + d);
            cr.move_to(cx + d, cy - d);
            cr.line_to(cx - d, cy + d);
            let _ = cr.stroke();
        }
        UiVectorIcon::Check => {
            cr.set_line_width((2.0 * s).max(1.8));
            cr.new_sub_path();
            cr.move_to(cx - 4.4 * s, cy + 0.2 * s);
            cr.line_to(cx - 1.1 * s, cy + 3.6 * s);
            cr.line_to(cx + 4.6 * s, cy - 3.2 * s);
            let _ = cr.stroke();
        }
        UiVectorIcon::Plus => {
            let d = 4.4 * s;
            cr.move_to(cx - d, cy);
            cr.line_to(cx + d, cy);
            cr.move_to(cx, cy - d);
            cr.line_to(cx, cy + d);
            let _ = cr.stroke();
        }
        UiVectorIcon::Refresh => {
            cr.new_sub_path();
            cr.arc(cx, cy, 4.6 * s, -0.35 * PI, 1.35 * PI);
            let _ = cr.stroke();
            cr.new_sub_path();
            cr.move_to(cx + 1.8 * s, cy - 5.2 * s);
            cr.line_to(cx + 4.8 * s, cy - 4.6 * s);
            cr.line_to(cx + 3.6 * s, cy - 1.6 * s);
            let _ = cr.stroke();
        }
        UiVectorIcon::Volume { muted } => {
            draw_module_vector_icon(cr, w, h, NotchModule::Hud, rgba);
            if muted {
                cr.set_line_width((1.8 * s).max(1.6));
                cr.move_to(cx - 5.2 * s, cy - 5.2 * s);
                cr.line_to(cx + 5.2 * s, cy + 5.2 * s);
                let _ = cr.stroke();
            }
        }
        UiVectorIcon::Brightness => {
            // Bold Apple SF sun: solid center + 8 cleanly separated tapered rays
            cr.new_sub_path();
            cr.arc(cx, cy, 3.0 * s, 0.0, TAU);
            let _ = cr.fill();
            cr.set_line_width((1.55 * s).max(1.5));
            for k in 0..8 {
                let a = (k as f64) * (TAU / 8.0);
                cr.move_to(cx + a.cos() * 4.8 * s, cy + a.sin() * 4.8 * s);
                cr.line_to(cx + a.cos() * 6.5 * s, cy + a.sin() * 6.5 * s);
            }
            let _ = cr.stroke();
        }
        UiVectorIcon::Screenshot => {
            rounded_rect_path(cr, cx - 5.8 * s, cy - 3.6 * s, 11.6 * s, 8.4 * s, 2.2 * s);
            let _ = cr.stroke();
            cr.new_sub_path();
            cr.arc(cx, cy + 0.6 * s, 2.2 * s, 0.0, TAU);
            let _ = cr.fill();
            cr.move_to(cx - 2.2 * s, cy - 5.0 * s);
            cr.line_to(cx + 2.2 * s, cy - 5.0 * s);
            let _ = cr.stroke();
        }
        UiVectorIcon::Record => {
            cr.new_sub_path();
            cr.arc(cx, cy, 5.4 * s, 0.0, TAU);
            let _ = cr.stroke();
            cr.set_source_rgba(1.0, 0.23, 0.19, 0.98);
            cr.new_sub_path();
            cr.arc(cx, cy, 2.8 * s, 0.0, TAU);
            let _ = cr.fill();
        }
        UiVectorIcon::Folder => {
            draw_module_vector_icon(cr, w, h, NotchModule::Files, rgba);
        }
        UiVectorIcon::ImageFile => {
            rounded_rect_path(cr, cx - 5.6 * s, cy - 5.0 * s, 11.2 * s, 10.0 * s, 2.0 * s);
            let _ = cr.stroke();
            cr.new_sub_path();
            cr.arc(cx - 2.2 * s, cy - 1.6 * s, 1.3 * s, 0.0, TAU);
            let _ = cr.fill();
            cr.new_sub_path();
            cr.move_to(cx - 4.4 * s, cy + 3.6 * s);
            cr.line_to(cx - 1.0 * s, cy + 0.2 * s);
            cr.line_to(cx + 1.5 * s, cy + 2.4 * s);
            cr.line_to(cx + 3.2 * s, cy + 0.6 * s);
            cr.line_to(cx + 4.8 * s, cy + 3.6 * s);
            let _ = cr.stroke();
        }
        UiVectorIcon::ArchiveFile => {
            rounded_rect_path(cr, cx - 5.0 * s, cy - 5.6 * s, 10.0 * s, 11.2 * s, 2.0 * s);
            let _ = cr.stroke();
            for k in -2..=2 {
                let yy = cy + f64::from(k) * 1.8 * s;
                cr.move_to(cx - 1.2 * s, yy);
                cr.line_to(cx + 1.2 * s, yy);
            }
            let _ = cr.stroke();
        }
        UiVectorIcon::DocFile => {
            rounded_rect_path(cr, cx - 4.8 * s, cy - 5.6 * s, 9.6 * s, 11.2 * s, 2.0 * s);
            let _ = cr.stroke();
            cr.move_to(cx - 2.4 * s, cy - 1.8 * s);
            cr.line_to(cx + 2.4 * s, cy - 1.8 * s);
            cr.move_to(cx - 2.4 * s, cy + 0.8 * s);
            cr.line_to(cx + 2.4 * s, cy + 0.8 * s);
            cr.move_to(cx - 2.4 * s, cy + 3.4 * s);
            cr.line_to(cx + 0.8 * s, cy + 3.4 * s);
            let _ = cr.stroke();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vibrant_album_art_extracts_distinct_neon_colors() {
        let (w, h) = (32, 32);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 4;
                if x < w / 2 {
                    rgba[idx] = 240;
                    rgba[idx + 1] = 35;
                    rgba[idx + 2] = 120;
                    rgba[idx + 3] = 255;
                } else {
                    rgba[idx] = 20;
                    rgba[idx + 1] = 210;
                    rgba[idx + 2] = 245;
                    rgba[idx + 3] = 255;
                }
            }
        }
        let pal = extract_neon_palette_from_rgba(&rgba, w, h, w * 4);
        assert!(pal.primary.distance_sq(pal.secondary) > 0.25);
    }

    #[test]
    fn velocity_preserving_m3_spring_overshoots_and_settles_cleanly() {
        let mut sp = Spring1D::new(148.0);
        sp.set_target(420.0);
        let mut peak = 148.0_f64;
        for _ in 0..120 {
            let active = sp.step(1.0 / 60.0, 0.72, 20.5, 0.12);
            peak = peak.max(sp.value);
            if !active {
                break;
            }
        }
        assert!(peak > 422.0 && peak < 460.0, "expected M3 overshoot, got {peak}");
        assert!((sp.value - 420.0).abs() < 0.5);
    }

    #[test]
    fn apple_ios_and_notch_backdrops_and_sf_icons_render_cleanly() {
        let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 680, 320).unwrap();
        let cr = cairo::Context::new(&surf).unwrap();
        for appearance in [
            NotchAppearance::Black,
            NotchAppearance::BlackGlass,
            NotchAppearance::LiquidGlass,
        ] {
            draw_notch_backdrop(
                &cr,
                680.0,
                10.0,
                416.0,
                196.0,
                36.0,
                14.0,
                appearance,
                IslandAuraKind::Media,
                1.0,
                NeonPalette::default(),
                0.85,
                1.2,
                Rgb::from_hex("#0A84FF"),
                false,
            );
        }
        draw_sf_skip_icon(&cr, 32.0, 24.0, false);
        draw_sf_skip_icon(&cr, 32.0, 24.0, true);
        draw_sf_play_pause_icon(&cr, 32.0, 32.0, true);
        draw_sf_play_pause_icon(&cr, 32.0, 32.0, false);
        draw_sf_airplay_icon(&cr, 24.0, 24.0);
        draw_apple_battery_glyph(&cr, 34.0, 17.0, 75.0, true);
        draw_apple_silent_pill(&cr, 56.0, 30.0, true);
        draw_apple_fallback_art(&cr, 58.0, 58.0, NeonPalette::default());
        draw_dock_pill_backdrop(&cr, 236.0, 48.0, NotchAppearance::Black);
        for &m in NotchModule::ALL_MODULES {
            draw_module_badge_tile(&cr, 34.0, 34.0, m);
            draw_module_vector_icon(&cr, 20.0, 20.0, m, (1.0, 1.0, 1.0, 1.0));
        }
        draw_module_vector_icon(&cr, 20.0, 20.0, NotchModule::Store, (1.0, 1.0, 1.0, 1.0));
        draw_round_pin_button(&cr, 26.0, 26.0, true, false, Rgb::from_hex("#0A84FF"));
        draw_round_pin_button(&cr, 26.0, 26.0, false, true, Rgb::from_hex("#0A84FF"));
        draw_round_circle_button(
            &cr,
            22.0,
            22.0,
            (0.18, 0.18, 0.20, 1.0),
            (1.0, 1.0, 1.0, 1.0),
            UiVectorIcon::Close,
        );
    }

    #[test]
    fn liquid_glass_backdrop_fallback_and_gpu_modes_render_cleanly() {
        let wp_surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 684, 456).unwrap();
        {
            let cr_wp = cairo::Context::new(&wp_surf).unwrap();
            cr_wp.set_source_rgba(0.25, 0.65, 0.95, 1.0);
            let _ = cr_wp.paint();
        }

        let surf_fb = cairo::ImageSurface::create(cairo::Format::ARgb32, 684, 240).unwrap();
        let cr_fb = cairo::Context::new(&surf_fb).unwrap();
        let paint_fb = LiquidGlassPaint {
            wallpaper_crop: Some(&wp_surf),
            wallpaper_avg_lum: 0.52,
            gpu_backed: false,
            aura_kind: IslandAuraKind::Idle,
            expanded_alpha: 1.0,
            palette: NeonPalette::default(),
            neon_intensity: 0.0,
            neon_phase: 0.0,
            drag_hover: false,
        };
        draw_notch_liquid_glass(&cr_fb, 684.0, 10.0, 520.0, 180.0, 28.0, &paint_fb);
        draw_dock_pill_liquid_glass(&cr_fb, 260.0, 54.0, 212.0, 194.0, &paint_fb);

        let mut surf_gpu = cairo::ImageSurface::create(cairo::Format::ARgb32, 684, 240).unwrap();
        let cr_gpu = cairo::Context::new(&surf_gpu).unwrap();
        let paint_gpu = LiquidGlassPaint {
            wallpaper_crop: None,
            wallpaper_avg_lum: 0.35,
            gpu_backed: true,
            aura_kind: IslandAuraKind::Idle,
            expanded_alpha: 1.0,
            palette: NeonPalette::default(),
            neon_intensity: 0.0,
            neon_phase: 0.0,
            drag_hover: false,
        };
        draw_notch_liquid_glass(&cr_gpu, 684.0, 10.0, 520.0, 180.0, 28.0, &paint_gpu);
        // When `gpu_backed` is true and idle, Cairo skips opaque fill so GPU optical shader shows cleanly.
        drop(cr_gpu);
        let data = surf_gpu.data().unwrap();
        assert!(data.iter().all(|&b| b == 0));
    }
}
