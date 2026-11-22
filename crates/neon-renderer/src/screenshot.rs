//! Native hardware-accelerated compositor screenshot capture to PNG.
//!
//! Renders the current output scene into an offscreen `GlesTexture`, reads back RGBA pixels
//! via Smithay's `ExportMem`, and writes a lossless PNG file cleanly on both DRM/KMS and winit.

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

/// Captures the full composited output (wallpaper, windows, shadows, blur, panel, dock) into a PNG file.
#[allow(clippy::too_many_arguments)]
pub fn capture_output_to_png<'a, F>(
    renderer: &mut GlesRenderer,
    neon_renderer: &mut NeonRenderer,
    output: &Output,
    scene: &SceneGraph,
    blur: BlurSettings,
    is_locked: bool,
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

    write_rgba_png(dest_path, width as u32, height as u32, pixels)?;
    info!(path = %dest_path.display(), width, height, "Captured NEON DE screenshot");
    Ok(())
}

/// Writes raw RGBA8 pixel data to a PNG file (creating parent directories automatically).
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
    let file =
        File::create(path).map_err(|e| format!("Failed to create {}: {e}", path.display()))?;
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_rgba_png_file() {
        let tmp = std::env::temp_dir().join(format!("neon-shot-test-{}.png", std::process::id()));
        let pixels = vec![
            56, 189, 248, 255, // cyan
            168, 85, 247, 255, // purple
            15, 23, 42, 255, // dark slate
            255, 255, 255, 255, // white
        ];
        write_rgba_png(&tmp, 2, 2, &pixels).expect("PNG encoding must succeed");
        let bytes = fs::read(&tmp).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let _ = fs::remove_file(&tmp);
    }
}
