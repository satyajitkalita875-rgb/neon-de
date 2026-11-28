//! Native hardware-accelerated compositor screenshot and live Wayland screencopy capture.
//!
//! Renders the current output scene into a cached offscreen `GlesTexture`, reads back RGBA
//! pixels via Smithay's `ExportMem`, and either writes a lossless PNG atomically or copies
//! directly into a `zwlr_screencopy_frame_v1` client `wl_shm` buffer (powering OBS Studio,
//! `xdg-desktop-portal-wlr`, `grim`, and browser screen sharing).

use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use neon_scene::SceneGraph;
use neon_window_manager::WindowId;
use smithay::{
    backend::{
        allocator::{Fourcc, dmabuf::Dmabuf},
        renderer::{
            Bind, Color32F, ExportMem, Offscreen,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, surface::WaylandSurfaceRenderElement, utils::RescaleRenderElement,
            },
            gles::{GlesRenderer, GlesTexture},
        },
    },
    desktop::{Window, layer_map_for_output},
    input::pointer::CursorImageStatus,
    output::Output,
    utils::{Buffer as BufferCoord, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{session_lock::LockSurface, shell::wlr_layer::Layer as WlrLayer},
};
use tracing::{info, warn};

use crate::{BlurSettings, FALLBACK_CLEAR_COLOR, LOCK_CURTAIN_COLOR, NeonRenderElement, NeonRenderer};

/// Clean obsidian letterbox background used behind isolated window captures.
pub const WINDOW_CAPTURE_BG_COLOR: Color32F = Color32F::new(0.043, 0.055, 0.078, 1.0);

/// Renders an arbitrary front-to-back list of [`NeonRenderElement`]s directly into a client
/// `Dmabuf` with hardware fence synchronization.
pub fn render_elements_to_dmabuf(
    renderer: &mut GlesRenderer,
    output: &Output,
    elements: &[NeonRenderElement],
    clear: Color32F,
    dmabuf: &mut Dmabuf,
) -> Result<(), String> {
    let mut tracker = OutputDamageTracker::from_output(output);
    let mut target = renderer
        .bind(dmabuf)
        .map_err(|e| format!("DMA-BUF screencopy framebuffer bind failed: {e:?}"))?;
    let render_res = tracker
        .render_output(renderer, &mut target, 0, elements, clear)
        .map_err(|e| format!("DMA-BUF screencopy render failed: {e:?}"))?;
    let _ = render_res.sync.wait();
    Ok(())
}

/// Collects front-to-back render elements for a single isolated [`Window`], aspect-fitted and
/// centered inside `(target_w, target_h)` physical pixels (with optional mapped cursor overlay).
#[allow(clippy::too_many_arguments)]
pub fn collect_window_capture_elements(
    renderer: &mut GlesRenderer,
    neon_renderer: &mut NeonRenderer,
    window: &Window,
    target_w: i32,
    target_h: i32,
    output_scale: f64,
    cursor_info: Option<(Point<f64, Logical>, Point<i32, Logical>, &CursorImageStatus)>,
) -> Vec<NeonRenderElement> {
    let mut elements = Vec::new();
    let win_geo = window.geometry();
    let win_w = win_geo.size.w.max(1) as f64;
    let win_h = win_geo.size.h.max(1) as f64;
    if target_w <= 0 || target_h <= 0 {
        return elements;
    }

    let out_s = output_scale.max(0.01);
    let nat_w = win_w * out_s;
    let nat_h = win_h * out_s;
    let fit = (target_w as f64 / nat_w).min(target_h as f64 / nat_h);
    let dest_w = (nat_w * fit).round() as i32;
    let dest_h = (nat_h * fit).round() as i32;
    let dest_x = (target_w - dest_w) / 2;
    let dest_y = (target_h - dest_h) / 2;

    // Optional cursor mapped cleanly into the isolated window's fitted coordinate space.
    if let Some((pointer_loc, window_screen_loc, cursor_status)) = cursor_info {
        let rel_x = pointer_loc.x - window_screen_loc.x as f64;
        let rel_y = pointer_loc.y - window_screen_loc.y as f64;
        if rel_x >= 0.0 && rel_x <= win_w && rel_y >= 0.0 && rel_y <= win_h {
            let mapped_logical: Point<f64, Logical> = (
                (dest_x as f64 / out_s) + rel_x * fit,
                (dest_y as f64 / out_s) + rel_y * fit,
            )
                .into();
            elements.extend(neon_renderer.cursor_elements(
                renderer,
                mapped_logical,
                (0, 0).into(),
                out_s,
                cursor_status,
            ));
        }
    }

    let base_phys: Point<i32, Physical> = (
        dest_x - (win_geo.loc.x as f64 * out_s).round() as i32,
        dest_y - (win_geo.loc.y as f64 * out_s).round() as i32,
    )
        .into();

    let surf_elems = AsRenderElements::<GlesRenderer>::render_elements::<
        WaylandSurfaceRenderElement<GlesRenderer>,
    >(window, renderer, base_phys, Scale::from(out_s), 1.0);

    if (fit - 1.0).abs() > 0.001 {
        let origin_phys: Point<i32, Physical> = (dest_x, dest_y).into();
        let rescale = Scale::from((fit, fit));
        elements.extend(surf_elems.into_iter().map(|elem| {
            NeonRenderElement::from(RescaleRenderElement::from_element(
                elem,
                origin_phys,
                rescale,
            ))
        }));
    } else {
        elements.extend(surf_elems.into_iter().map(NeonRenderElement::from));
    }
    elements
}

/// Collects front-to-back render elements for an arbitrary workspace (including inactive
/// background workspaces) or downscaled display preview at `render_scale`.
pub fn collect_workspace_capture_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    windows_front_to_back: &[(&Window, Point<i32, Logical>, bool)],
    render_scale: f64,
    include_top_chrome: bool,
) -> Vec<NeonRenderElement> {
    let scale = Scale::from(render_scale);
    let mut elements = Vec::new();
    let has_fullscreen = windows_front_to_back.iter().any(|(_, _, fs)| *fs);

    let (upper_layers, lower_layers) = {
        let layer_map = layer_map_for_output(output);
        let mut upper = Vec::new();
        let mut lower = Vec::new();
        if include_top_chrome && !has_fullscreen {
            for layer in layer_map.layers_on(WlrLayer::Top).rev() {
                if matches!(layer.namespace(), "neon-overview" | "neon-workspaces-panel") {
                    continue;
                }
                if let Some(geo) = layer_map.layer_geometry(layer) {
                    upper.push((layer.clone(), geo.loc));
                }
            }
        }
        for wlr_layer in [WlrLayer::Bottom, WlrLayer::Background] {
            for layer in layer_map.layers_on(wlr_layer).rev() {
                if let Some(geo) = layer_map.layer_geometry(layer) {
                    lower.push((layer.clone(), geo.loc));
                }
            }
        }
        (upper, lower)
    };

    for (layer, loc) in &upper_layers {
        let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(render_scale);
        let layer_elems = AsRenderElements::<GlesRenderer>::render_elements::<
            WaylandSurfaceRenderElement<GlesRenderer>,
        >(layer, renderer, phys_loc, scale, 1.0);
        elements.extend(layer_elems.into_iter().map(NeonRenderElement::from));
    }

    for &(window, loc, _fs) in windows_front_to_back {
        let win_geo = window.geometry();
        let render_loc: Point<i32, Logical> = (loc.x - win_geo.loc.x, loc.y - win_geo.loc.y).into();
        let phys_loc: Point<i32, Physical> = render_loc.to_physical_precise_round(render_scale);
        let surf_elems = AsRenderElements::<GlesRenderer>::render_elements::<
            WaylandSurfaceRenderElement<GlesRenderer>,
        >(window, renderer, phys_loc, scale, 1.0);
        elements.extend(surf_elems.into_iter().map(NeonRenderElement::from));
    }

    for (layer, loc) in &lower_layers {
        let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(render_scale);
        let layer_elems = AsRenderElements::<GlesRenderer>::render_elements::<
            WaylandSurfaceRenderElement<GlesRenderer>,
        >(layer, renderer, phys_loc, scale, 1.0);
        elements.extend(layer_elems.into_iter().map(NeonRenderElement::from));
    }

    elements
}

/// Hardware zero-copy capture of a single isolated window directly into a client `Dmabuf`.
#[allow(clippy::too_many_arguments)]
pub fn capture_window_to_dmabuf(
    renderer: &mut GlesRenderer,
    neon_renderer: &mut NeonRenderer,
    output: &Output,
    window: Option<&Window>,
    cursor_info: Option<(Point<f64, Logical>, Point<i32, Logical>, &CursorImageStatus)>,
    dmabuf: &mut Dmabuf,
) -> Result<(), String> {
    let mode = output
        .current_mode()
        .ok_or_else(|| "Output has no active mode".to_string())?;
    let scale_f = output.current_scale().fractional_scale();
    let elements = if let Some(win) = window {
        collect_window_capture_elements(
            renderer,
            neon_renderer,
            win,
            mode.size.w,
            mode.size.h,
            scale_f,
            cursor_info,
        )
    } else {
        Vec::new()
    };
    render_elements_to_dmabuf(renderer, output, &elements, WINDOW_CAPTURE_BG_COLOR, dmabuf)
}

/// Hardware zero-copy capture of a specific workspace directly into a client `Dmabuf`.
pub fn capture_workspace_to_dmabuf(
    renderer: &mut GlesRenderer,
    output: &Output,
    windows_front_to_back: &[(&Window, Point<i32, Logical>, bool)],
    extra_top_elements: Vec<NeonRenderElement>,
    dmabuf: &mut Dmabuf,
) -> Result<(), String> {
    let scale_f = output.current_scale().fractional_scale();
    let mut elements = extra_top_elements;
    elements.extend(collect_workspace_capture_elements(
        renderer,
        output,
        windows_front_to_back,
        scale_f,
        true,
    ));
    render_elements_to_dmabuf(renderer, output, &elements, FALLBACK_CLEAR_COLOR, dmabuf)
}

/// Renders a front-to-back list of preview elements into a compact offscreen GPU texture
/// (`width x height`), reads back the small RGBA buffer, and encodes/writes the PNG atomically
/// on a background thread so the compositor render loop never stalls.
pub fn capture_preview_png_async(
    renderer: &mut GlesRenderer,
    elements: &[NeonRenderElement],
    clear: Color32F,
    width: i32,
    height: i32,
    scale_f: f64,
    dest_path: PathBuf,
) -> Result<(), String> {
    if width <= 0 || height <= 0 {
        return Err("Invalid preview thumbnail dimensions".to_string());
    }
    let buf_size: Size<i32, BufferCoord> = (width, height).into();
    let mut offscreen: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buf_size)
        .map_err(|e| format!("Preview texture alloc failed: {e:?}"))?;

    let mut tracker = OutputDamageTracker::new(
        Size::from((width, height)),
        Scale::from(scale_f.max(0.01)),
        Transform::Normal,
    );
    let mapping = {
        let mut target = renderer
            .bind(&mut offscreen)
            .map_err(|e| format!("Preview framebuffer bind failed: {e:?}"))?;
        tracker
            .render_output(renderer, &mut target, 0, elements, clear)
            .map_err(|e| format!("Preview render failed: {e:?}"))?;
        renderer
            .copy_framebuffer(&target, Rectangle::from_size(buf_size), Fourcc::Abgr8888)
            .map_err(|e| format!("Preview readback failed: {e:?}"))?
    };

    let pixels = renderer
        .map_texture(&mapping)
        .map_err(|e| format!("Preview pixel map failed: {e:?}"))?
        .to_vec();

    let w = width as u32;
    let h = height as u32;
    std::thread::spawn(move || {
        if let Err(err) = write_rgba_png(&dest_path, w, h, &pixels) {
            warn!(path = %dest_path.display(), %err, "Failed to write screencast preview PNG");
        }
    });
    Ok(())
}

/// Hardware zero-copy render of an output directly into a client `Dmabuf` (powering 60 FPS
/// OBS Studio / `xdg-desktop-portal-wlr` / PipeWire screencast with zero CPU readback).
#[allow(clippy::too_many_arguments)]
pub fn capture_output_to_dmabuf<'a, F>(
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
    dmabuf: &mut Dmabuf,
) -> Result<(), String>
where
    F: FnMut(WindowId) -> Option<&'a Window>,
{
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

    render_elements_to_dmabuf(renderer, output, &elements, clear, dmabuf)
}

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
