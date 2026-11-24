//! Native hardware-accelerated compositor screenshot and live Wayland screencopy capture.
//!
//! Renders the current output scene into a cached offscreen `GlesTexture`, reads back RGBA
//! pixels via Smithay's `ExportMem`, and either writes a lossless PNG atomically or copies
//! directly into a `zwlr_screencopy_frame_v1` client `wl_shm` buffer (powering OBS Studio,
//! `xdg-desktop-portal-wlr`, `grim`, and browser screen sharing).

use std::{
    fs::{self, File},
    io::BufWriter,
    path::Path,
};

use neon_scene::SceneGraph;
use neon_window_manager::WindowId;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, ExportMem, Offscreen,
            damage::OutputDamageTracker,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    desktop::Window,
    output::Output,
    utils::{Buffer as BufferCoord, Logical, Point, Rectangle, Size},
    wayland::session_lock::LockSurface,
};
use tracing::info;

use crate::{BlurSettings, FALLBACK_CLEAR_COLOR, LOCK_CURTAIN_COLOR, NeonRenderElement, NeonRenderer};

/// Renders the current output scene into a reusable offscreen `GlesTexture`, reads back RGBA8
/// pixels, and passes `(rgba_pixels, width, height)` to `on_rgba` without an extra heap copy.
#[allow(clippy::too_many_arguments)]
pub fn capture_output_rgba<'a, F, C>(
    renderer: &mut GlesRenderer,
    neon_renderer: &mut NeonRenderer,
    output: &Output,
    scene: &SceneGraph,
    blur: BlurSettings,
    is_locked: bool,
    lock_surface: Option<&LockSurface>,
    override_redirect_popups: &[(&Window, Point<i32, Logical>)],
    extra_top_elements: Vec<NeonRenderElement>,
    resolve_window: F,
    on_rgba: C,
) -> Result<(), String>
where
    F: FnMut(WindowId) -> Option<&'a Window>,
    C: FnOnce(&[u8], u32, u32) -> Result<(), String>,
{
    let mode = output
        .current_mode()
        .ok_or_else(|| "Output has no active mode".to_string())?;
    let width = mode.size.w;
    let height = mode.size.h;
    if width <= 0 || height <= 0 {
        return Err("Output dimensions are invalid".to_string());
    }
    let buf_size: Size<i32, BufferCoord> = (width, height).into();

    let mut elements = extra_top_elements;
    let clear = if is_locked {
        elements.extend(neon_renderer.collect_locked_elements(output, renderer, lock_surface));
        LOCK_CURTAIN_COLOR
    } else {
        elements.extend(neon_renderer.collect_scene_elements_with_popups(
            output,
            renderer,
            scene,
            blur,
            override_redirect_popups,
            resolve_window,
        ));
        FALLBACK_CLEAR_COLOR
    };

    // Reuse cached offscreen GLES texture across frames so 30/60 FPS OBS capture has zero alloc churn.
    let needs_alloc = neon_renderer
        .screencopy_offscreen
        .as_ref()
        .is_none_or(|(sz, _)| *sz != buf_size);
    if needs_alloc {
        let tex: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, buf_size)
            .map_err(|e| format!("Offscreen capture buffer allocation failed: {e:?}"))?;
        neon_renderer.screencopy_offscreen = Some((buf_size, tex));
    }

    let (_, offscreen) = neon_renderer
        .screencopy_offscreen
        .as_mut()
        .ok_or_else(|| "Offscreen capture buffer missing".to_string())?;

    let mut tracker = OutputDamageTracker::from_output(output);
    let mapping = {
        let mut target = renderer
            .bind(offscreen)
            .map_err(|e| format!("Offscreen framebuffer bind failed: {e:?}"))?;
        tracker
            .render_output(renderer, &mut target, 0, &elements, clear)
            .map_err(|e| format!("Offscreen capture render failed: {e:?}"))?;
        renderer
            .copy_framebuffer(&target, Rectangle::from_size(buf_size), Fourcc::Abgr8888)
            .map_err(|e| format!("Offscreen framebuffer readback failed: {e:?}"))?
    };

    let pixels = renderer
        .map_texture(&mapping)
        .map_err(|e| format!("Capture pixel map failed: {e:?}"))?;

    on_rgba(pixels, width as u32, height as u32)
}

/// Copies a physical `[x, y, w, h]` sub-rectangle from an RGBA8 source frame into a client
/// `wl_shm` pixel buffer (optionally swizzling R<->B for `ARGB8888`/`XRGB8888` formats).
pub fn copy_rgba_region_to_shm(
    src_rgba: &[u8],
    src_w: u32,
    src_h: u32,
    region: [i32; 4],
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    dst_stride: usize,
    swap_rb: bool,
    force_opaque_alpha: bool,
) -> Result<(), String> {
    let x0 = region[0].clamp(0, src_w.saturating_sub(1) as i32) as usize;
    let y0 = region[1].clamp(0, src_h.saturating_sub(1) as i32) as usize;
    let copy_w = (dst_w as usize).min((src_w as usize).saturating_sub(x0));
    let copy_h = (dst_h as usize).min((src_h as usize).saturating_sub(y0));
    let src_stride = (src_w as usize) * 4;

    if dst_stride < copy_w * 4 || dst.len() < copy_h * dst_stride {
        return Err("Destination wl_shm buffer too small for screencopy region".to_string());
    }

    for row in 0..copy_h {
        let src_start = (y0 + row) * src_stride + x0 * 4;
        let dst_start = row * dst_stride;
        let row_bytes = copy_w * 4;
        if src_start + row_bytes > src_rgba.len() {
            break;
        }
        let s = &src_rgba[src_start..src_start + row_bytes];
        let d = &mut dst[dst_start..dst_start + row_bytes];
        if !swap_rb && !force_opaque_alpha {
            d.copy_from_slice(s);
        } else {
            for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
                if swap_rb {
                    dp[0] = sp[2];
                    dp[1] = sp[1];
                    dp[2] = sp[0];
                } else {
                    dp[0] = sp[0];
                    dp[1] = sp[1];
                    dp[2] = sp[2];
                }
                dp[3] = if force_opaque_alpha { 0xFF } else { sp[3] };
            }
        }
    }
    Ok(())
}

/// Captures the composited output (or an optional `[x, y, w, h]` logical crop region) to PNG.
#[allow(clippy::too_many_arguments)]
pub fn capture_output_to_png<'a, F>(
    renderer: &mut GlesRenderer,
    neon_renderer: &mut NeonRenderer,
    output: &Output,
    scene: &SceneGraph,
    blur: BlurSettings,
    is_locked: bool,
    crop_logical: Option<[i32; 4]>,
    resolve_window: F,
    dest_path: &Path,
) -> Result<(), String>
where
    F: FnMut(WindowId) -> Option<&'a Window>,
{
    let scale_f = output.current_scale().fractional_scale();
    let origin = output.current_location();

    capture_output_rgba(
        renderer,
        neon_renderer,
        output,
        scene,
        blur,
        is_locked,
        None,
        &[],
        Vec::new(),
        resolve_window,
        |pixels, width, height| {
            if let Some([lx, ly, lw, lh]) = crop_logical {
                let px = (((lx - origin.x) as f64) * scale_f).round() as i32;
                let py = (((ly - origin.y) as f64) * scale_f).round() as i32;
                let pw = ((lw as f64) * scale_f).round() as i32;
                let ph = ((lh as f64) * scale_f).round() as i32;
                let (cw, ch, cropped) = crop_rgba(pixels, width, height, px, py, pw, ph);
                write_rgba_png(dest_path, cw, ch, &cropped)?;
                info!(path = %dest_path.display(), width = cw, height = ch, "Captured cropped NEON DE screenshot");
            } else {
                write_rgba_png(dest_path, width, height, pixels)?;
                info!(path = %dest_path.display(), width, height, "Captured full NEON DE screenshot");
            }
            Ok(())
        },
    )
}

/// Crops an RGBA8 pixel buffer cleanly to `[x, y, w, h]`, clamped within image bounds.
pub fn crop_rgba(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> (u32, u32, Vec<u8>) {
    let x0 = x.clamp(0, src_w.saturating_sub(1) as i32) as u32;
    let y0 = y.clamp(0, src_h.saturating_sub(1) as i32) as u32;
    let cw = (w.max(1) as u32).min(src_w - x0).max(1);
    let ch = (h.max(1) as u32).min(src_h - y0).max(1);

    let mut out = vec![0u8; (cw as usize) * (ch as usize) * 4];
    let src_stride = (src_w as usize) * 4;
    let dst_stride = (cw as usize) * 4;
    for row in 0..ch as usize {
        let src_start = ((y0 as usize) + row) * src_stride + (x0 as usize) * 4;
        let dst_start = row * dst_stride;
        if src_start + dst_stride <= src.len() {
            out[dst_start..dst_start + dst_stride]
                .copy_from_slice(&src[src_start..src_start + dst_stride]);
        }
    }
    (cw, ch, out)
}

/// Writes raw RGBA8 pixel data atomically to a PNG file (temp file + rename).
pub fn write_rgba_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() < expected {
        return Err(format!(
            "RGBA pixel buffer too small: got {} bytes, need {}",
            rgba.len(),
            expected
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create screenshot directory: {e}"))?;
    }
    let tmp_path = path.with_extension("png.tmp");
    {
        let file = File::create(&tmp_path)
            .map_err(|e| format!("Failed to create {}: {e}", tmp_path.display()))?;
        let writer = BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut png_writer = encoder
            .write_header()
            .map_err(|e| format!("Failed to write PNG header: {e}"))?;
        png_writer
            .write_image_data(&rgba[..expected])
            .map_err(|e| format!("Failed to encode PNG pixels: {e}"))?;
        png_writer
            .finish()
            .map_err(|e| format!("Failed to finish PNG stream: {e}"))?;
    }
    fs::rename(&tmp_path, path)
        .map_err(|e| format!("Failed to move screenshot into place: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_rgba_png_file_and_crops_cleanly() {
        let tmp = std::env::temp_dir().join(format!("neon-shot-test-{}.png", std::process::id()));
        let pixels = vec![0x38, 0xBD, 0xF8, 0xFF, 0x10, 0x20, 0x30, 0xFF, 0, 0, 0, 255, 255, 255, 255, 255];
        let (cw, ch, cropped) = crop_rgba(&pixels, 2, 2, 1, 0, 1, 2);
        assert_eq!((cw, ch), (1, 2));
        assert_eq!(&cropped[..4], &[0x10, 0x20, 0x30, 0xFF]);
        write_rgba_png(&tmp, cw, ch, &cropped).expect("png write");
        assert!(tmp.exists());
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn copy_rgba_region_to_shm_swizzles_argb8888_and_crops() {
        // 2x2 RGBA source:
        // (0,0)=[R=10,G=20,B=30,A=40]  (1,0)=[R=50,G=60,B=70,A=80]
        // (0,1)=[R=90,G=100,B=110,A=120] (1,1)=[R=130,G=140,B=150,A=160]
        let src = vec![
            10, 20, 30, 40, 50, 60, 70, 80,
            90, 100, 110, 120, 130, 140, 150, 160,
        ];
        let mut dst = vec![0u8; 4];
        copy_rgba_region_to_shm(&src, 2, 2, [1, 1, 1, 1], &mut dst, 1, 1, 4, true, false)
            .expect("copy region");
        // ARGB8888 little-endian memory order = [B, G, R, A]
        assert_eq!(dst, vec![150, 140, 130, 160]);
    }
}
