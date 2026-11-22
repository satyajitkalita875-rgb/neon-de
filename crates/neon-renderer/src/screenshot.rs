//! Native hardware-accelerated compositor screenshot capture to PNG with optional region crop.
//!
//! Renders the current output scene into an offscreen `GlesTexture`, reads back RGBA pixels
//! via Smithay's `ExportMem`, crops cleanly when a logical selection region is provided, and
//! writes a lossless PNG atomically (`.tmp` + rename) on both DRM/KMS and winit.

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
    utils::{Buffer as BufferCoord, Rectangle, Size},
};
use tracing::info;

use crate::{BlurSettings, FALLBACK_CLEAR_COLOR, LOCK_CURTAIN_COLOR, NeonRenderer};

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
    let mode = output
        .current_mode()
        .ok_or_else(|| "Output has no active mode".to_string())?;
    let width = mode.size.w;
    let height = mode.size.h;
    if width <= 0 || height <= 0 {
        return Err("Output dimensions are invalid".to_string());
    }
    let scale_f = output.current_scale().fractional_scale();
    let buf_size: Size<i32, BufferCoord> = (width, height).into();

    let mut offscreen: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buf_size)
        .map_err(|e| format!("Offscreen screenshot buffer allocation failed: {e:?}"))?;

    let elements = if is_locked {
        neon_renderer.collect_locked_elements(output, renderer, None)
    } else {
        neon_renderer.collect_scene_elements(output, renderer, scene, blur, resolve_window)
    };
    let clear = if is_locked {
        LOCK_CURTAIN_COLOR
    } else {
        FALLBACK_CLEAR_COLOR
    };

    let mut tracker = OutputDamageTracker::from_output(output);
    let mapping = {
        let mut target = renderer
            .bind(&mut offscreen)
            .map_err(|e| format!("Offscreen framebuffer bind failed: {e:?}"))?;
        tracker
            .render_output(renderer, &mut target, 0, &elements, clear)
            .map_err(|e| format!("Offscreen screenshot render failed: {e:?}"))?;
        renderer
            .copy_framebuffer(&target, Rectangle::from_size(buf_size), Fourcc::Abgr8888)
            .map_err(|e| format!("Screenshot framebuffer readback failed: {e:?}"))?
    };

    let pixels = renderer
        .map_texture(&mapping)
        .map_err(|e| format!("Screenshot pixel map failed: {e:?}"))?;

    if let Some([lx, ly, lw, lh]) = crop_logical {
        let origin = output.current_location();
        let px = (((lx - origin.x) as f64) * scale_f).round() as i32;
        let py = (((ly - origin.y) as f64) * scale_f).round() as i32;
        let pw = ((lw as f64) * scale_f).round() as i32;
        let ph = ((lh as f64) * scale_f).round() as i32;
        let (cw, ch, cropped) = crop_rgba(pixels, width as u32, height as u32, px, py, pw, ph);
        write_rgba_png(dest_path, cw, ch, &cropped)?;
        info!(path = %dest_path.display(), width = cw, height = ch, "Captured cropped NEON DE screenshot");
    } else {
        write_rgba_png(dest_path, width as u32, height as u32, pixels)?;
        info!(path = %dest_path.display(), width, height, "Captured full NEON DE screenshot");
    }
    Ok(())
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
        let pixels = vec![
            56, 189, 248, 255, // (0,0) cyan
            168, 85, 247, 255, // (1,0) purple
            15, 23, 42, 255, // (0,1) slate
            255, 255, 255, 255, // (1,1) white
        ];
        let (cw, ch, cropped) = crop_rgba(&pixels, 2, 2, 1, 1, 1, 1);
        assert_eq!((cw, ch), (1, 1));
        assert_eq!(cropped, vec![255, 255, 255, 255]);

        write_rgba_png(&tmp, 2, 2, &pixels).expect("PNG encoding must succeed");
        let bytes = fs::read(&tmp).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let _ = fs::remove_file(&tmp);
    }
}
