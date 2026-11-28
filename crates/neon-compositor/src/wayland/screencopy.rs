//! `zwlr_screencopy_manager_v1` (v3) server implementation for **NEON DE**.
//!
//! Supports BOTH zero-copy hardware `linux_dmabuf` (DMA-BUF) rendering (60 FPS OBS Studio,
//! `xdg-desktop-portal-wlr`, PipeWire) AND `wl_shm` CPU-buffer fallback (`grim`, regions).

use std::{sync::Mutex, time::Duration};

use neon_ipc::ScreencastTarget;
use neon_renderer::{
    BlurSettings, FALLBACK_CLEAR_COLOR, NeonRenderElement, NeonRenderer,
    screenshot::{
        WINDOW_CAPTURE_BG_COLOR, capture_output_to_dmabuf, capture_preview_png_async,
        capture_window_to_dmabuf, capture_workspace_to_dmabuf, collect_window_capture_elements,
        collect_workspace_capture_elements, copy_rgba_region_to_shm,
    },
};
use neon_window_manager::WindowId;
use smithay::{
    backend::{
        allocator::{Buffer as AllocBuffer, Fourcc, dmabuf::Dmabuf},
        renderer::gles::GlesRenderer,
    },
    desktop::Window,
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, Flags as FrameFlags, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            protocol::{wl_buffer::WlBuffer, wl_shm},
        },
    },
    utils::{Logical, Point},
    wayland::{
        dmabuf::get_dmabuf,
        session_lock::LockSurface,
        shm::{BufferData, with_buffer_contents, with_buffer_contents_mut},
    },
};
use tracing::{debug, warn};

use crate::{
    state::{NeonState, PendingPreviewCapture},
    window::from_wm_point,
};

/// Per-frame protocol state attached to each `zwlr_screencopy_frame_v1` resource.
#[derive(Debug)]
pub struct ScreencopyFrameData {
    pub output_name: String,
    /// Physical pixel capture box `[x, y, width, height]` within the output buffer.
    pub region: [i32; 4],
    pub is_full_output: bool,
    pub overlay_cursor: bool,
    pub used: bool,
}

/// Target backing buffer for a queued screencopy frame.
#[derive(Debug)]
pub enum ScreencopyTarget {
    /// Hardware zero-copy DMA-BUF (`zwp_linux_dmabuf_v1` buffer).
    Dmabuf(Dmabuf),
    /// Shared-memory CPU buffer (`wl_shm`).
    Shm(WlBuffer),
}

/// Queued screencopy copy request awaiting the compositor's GPU render pass.
#[derive(Debug)]
pub struct PendingScreencopyFrame {
    pub frame: ZwlrScreencopyFrameV1,
    pub target: ScreencopyTarget,
    pub output_name: String,
    pub region: [i32; 4],
    pub overlay_cursor: bool,
    pub with_damage: bool,
}

/// Registers the `zwlr_screencopy_manager_v1` (version 3) Wayland global on `display_handle`.
pub fn register_screencopy_global(display_handle: &DisplayHandle) {
    display_handle.create_global::<NeonState, ZwlrScreencopyManagerV1, ()>(3, ());
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for NeonState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for NeonState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let supports_dmabuf = state.dmabuf_global.is_some();
        match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => {
                init_screencopy_frame(
                    frame,
                    overlay_cursor != 0,
                    &output,
                    None,
                    supports_dmabuf,
                    data_init,
                );
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => {
                init_screencopy_frame(
                    frame,
                    overlay_cursor != 0,
                    &output,
                    Some([x, y, width, height]),
                    false,
                    data_init,
                );
            }
            zwlr_screencopy_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

fn init_screencopy_frame(
    frame_new: New<ZwlrScreencopyFrameV1>,
    overlay_cursor: bool,
    wl_output: &smithay::reexports::wayland_server::protocol::wl_output::WlOutput,
    logical_region: Option<[i32; 4]>,
    advertise_dmabuf: bool,
    data_init: &mut DataInit<'_, NeonState>,
) {
    let Some(output) = Output::from_resource(wl_output) else {
        let frame = data_init.init(
            frame_new,
            Mutex::new(ScreencopyFrameData {
                output_name: String::new(),
                region: [0, 0, 0, 0],
                is_full_output: false,
                overlay_cursor,
                used: true,
            }),
        );
        frame.failed();
        return;
    };

    let Some(mode) = output.current_mode() else {
        let frame = data_init.init(
            frame_new,
            Mutex::new(ScreencopyFrameData {
                output_name: output.name(),
                region: [0, 0, 0, 0],
                is_full_output: false,
                overlay_cursor,
                used: true,
            }),
        );
        frame.failed();
        return;
    };

    let out_w = mode.size.w;
    let out_h = mode.size.h;
    let scale = output.current_scale().fractional_scale();

    let is_full_output = logical_region.is_none();
    let region = if let Some([lx, ly, lw, lh]) = logical_region {
        let px = ((lx as f64) * scale).round() as i32;
        let py = ((ly as f64) * scale).round() as i32;
        let pw = ((lw as f64) * scale).round() as i32;
        let ph = ((lh as f64) * scale).round() as i32;
        let x0 = px.clamp(0, out_w);
        let y0 = py.clamp(0, out_h);
        let x1 = (px + pw).clamp(x0, out_w);
        let y1 = (py + ph).clamp(y0, out_h);
        [x0, y0, x1 - x0, y1 - y0]
    } else {
        [0, 0, out_w, out_h]
    };

    let pw = region[2];
    let ph = region[3];
    let frame = data_init.init(
        frame_new,
        Mutex::new(ScreencopyFrameData {
            output_name: output.name(),
            region,
            is_full_output,
            overlay_cursor,
            used: pw <= 0 || ph <= 0,
        }),
    );

    if pw <= 0 || ph <= 0 {
        frame.failed();
        return;
    }

    let stride = (pw as u32).saturating_mul(4);
    frame.buffer(wl_shm::Format::Xrgb8888, pw as u32, ph as u32, stride);
    if frame.version() >= 3 {
        if advertise_dmabuf && is_full_output {
            frame.linux_dmabuf(Fourcc::Xrgb8888 as u32, pw as u32, ph as u32);
        }
        frame.buffer_done();
    }
    debug!(
        output = %output.name(),
        width = pw,
        height = ph,
        overlay_cursor,
        dmabuf = advertise_dmabuf && is_full_output,
        "Initialized zwlr_screencopy_frame_v1"
    );
}

impl Dispatch<ZwlrScreencopyFrameV1, Mutex<ScreencopyFrameData>> for NeonState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &Mutex<ScreencopyFrameData>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy => return,
            _ => return,
        };

        let (output_name, region, is_full_output, overlay_cursor) = {
            let mut guard = data.lock().expect("screencopy frame lock");
            if guard.used {
                resource.post_error(
                    zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                    "screencopy frame has already been used",
                );
                return;
            }
            guard.used = true;
            (
                guard.output_name.clone(),
                guard.region,
                guard.is_full_output,
                guard.overlay_cursor,
            )
        };

        // 1. Zero-copy DMA-BUF path (preferred by xdg-desktop-portal-wlr + OBS PipeWire)
        if let Ok(dmabuf) = get_dmabuf(&buffer) {
            let sz = dmabuf.size();
            if is_full_output && sz.w == region[2] && sz.h == region[3] {
                state.pending_screencopy_frames.push(PendingScreencopyFrame {
                    frame: resource.clone(),
                    target: ScreencopyTarget::Dmabuf(dmabuf.clone()),
                    output_name,
                    region,
                    overlay_cursor,
                    with_damage,
                });
                state.needs_redraw = true;
                return;
            }
            resource.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "invalid dmabuf dimensions for screencopy",
            );
            return;
        }

        // 2. Fallback wl_shm buffer path (grim, region captures, software clients)
        let valid_shm = with_buffer_contents(&buffer, |_, pool_len, bdata: BufferData| {
            let supported_fmt = matches!(
                bdata.format,
                wl_shm::Format::Argb8888
                    | wl_shm::Format::Xrgb8888
                    | wl_shm::Format::Abgr8888
                    | wl_shm::Format::Xbgr8888
            );
            let dims_match = bdata.width == region[2] && bdata.height == region[3];
            let stride_ok = bdata.stride >= bdata.width.saturating_mul(4);
            let offset_ok = bdata.offset >= 0;
            let bounds_ok = if offset_ok && stride_ok && bdata.height > 0 {
                (bdata.offset as usize)
                    .checked_add((bdata.stride as usize).saturating_mul(bdata.height as usize))
                    .is_some_and(|end| end <= pool_len)
            } else {
                false
            };
            supported_fmt && dims_match && stride_ok && bounds_ok
        })
        .unwrap_or(false);

        if !valid_shm {
            resource.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "invalid buffer dimensions, stride, or format for screencopy",
            );
            return;
        }

        state.pending_screencopy_frames.push(PendingScreencopyFrame {
            frame: resource.clone(),
            target: ScreencopyTarget::Shm(buffer),
            output_name,
            region,
            overlay_cursor,
            with_damage,
        });
        state.needs_redraw = true;
    }
}

/// Emits `flags`, optional `damage`, and `ready` on a completed `zwlr_screencopy_frame_v1`.
pub fn emit_screencopy_ready(pending: &PendingScreencopyFrame, timestamp: Duration) {
    if !pending.frame.is_alive() {
        return;
    }
    pending.frame.flags(FrameFlags::empty());
    if pending.with_damage && pending.frame.version() >= 2 {
        pending
            .frame
            .damage(0, 0, pending.region[2] as u32, pending.region[3] as u32);
    }
    let secs = timestamp.as_secs();
    let tv_sec_hi = (secs >> 32) as u32;
    let tv_sec_lo = (secs & 0xFFFF_FFFF) as u32;
    let tv_nsec = timestamp.subsec_nanos();
    pending.frame.ready(tv_sec_hi, tv_sec_lo, tv_nsec);
}

/// Completes a queued `wl_shm` `zwlr_screencopy_frame_v1` by copying captured RGBA8 pixels
/// into the client's `wl_shm` buffer and sending `flags`, optional `damage`, and `ready`.
pub fn complete_screencopy_shm_frame(
    pending: &PendingScreencopyFrame,
    buffer: &WlBuffer,
    rgba_pixels: &[u8],
    src_w: u32,
    src_h: u32,
    timestamp: Duration,
) {
    if !pending.frame.is_alive() {
        return;
    }

    let copy_result = with_buffer_contents_mut(buffer, |ptr, pool_len, bdata| {
        if bdata.offset < 0 || bdata.width <= 0 || bdata.height <= 0 || bdata.stride < bdata.width * 4 {
            return Err("Invalid shm buffer metadata".to_string());
        }
        let offset = bdata.offset as usize;
        let stride = bdata.stride as usize;
        let height = bdata.height as usize;
        let needed = stride.saturating_mul(height);
        if offset.checked_add(needed).is_none_or(|end| end > pool_len) {
            return Err("Shm buffer out of pool bounds".to_string());
        }

        let (swap_rb, force_opaque) = match bdata.format {
            wl_shm::Format::Argb8888 => (true, false),
            wl_shm::Format::Xrgb8888 => (true, true),
            wl_shm::Format::Abgr8888 => (false, false),
            wl_shm::Format::Xbgr8888 => (false, true),
            other => return Err(format!("Unsupported shm format: {other:?}")),
        };

        // SAFETY: `offset + needed <= pool_len` verified above while holding `with_buffer_contents_mut`.
        let dst = unsafe { std::slice::from_raw_parts_mut(ptr.add(offset), needed) };
        copy_rgba_region_to_shm(
            rgba_pixels,
            src_w,
            src_h,
            pending.region,
            dst,
            bdata.width as u32,
            bdata.height as u32,
            stride,
            swap_rb,
            force_opaque,
        )
    });

    match copy_result {
        Ok(Ok(())) => emit_screencopy_ready(pending, timestamp),
        Ok(Err(err)) => {
            warn!(%err, "Screencopy pixel copy failed");
            pending.frame.failed();
        }
        Err(err) => {
            warn!(?err, "Screencopy wl_shm buffer access failed");
            pending.frame.failed();
        }
    }
}

/// Fulfills a zero-copy DMA-BUF screencopy frame honoring `state.screencast_target`
/// (`Output`, isolated `Window`, or isolated `Workspace`).
#[allow(clippy::too_many_arguments)]
pub fn fulfill_screencopy_dmabuf(
    renderer: &mut GlesRenderer,
    neon_renderer: &mut NeonRenderer,
    output: &Output,
    state: &NeonState,
    blur: BlurSettings,
    is_locked: bool,
    lock_surface: Option<&LockSurface>,
    or_popups: &[(&Window, Point<i32, Logical>)],
    extra_top: Vec<NeonRenderElement>,
    overlay_cursor: bool,
    now_dur: Duration,
    dmabuf: &mut Dmabuf,
) -> Result<(), String> {
    if is_locked {
        return capture_output_to_dmabuf(
            renderer,
            neon_renderer,
            output,
            &state.scene,
            blur,
            true,
            lock_surface,
            or_popups,
            extra_top,
            |id| state.managed_windows.get(&id),
            dmabuf,
        );
    }

    match &state.screencast_target {
        ScreencastTarget::Output { .. } => capture_output_to_dmabuf(
            renderer,
            neon_renderer,
            output,
            &state.scene,
            blur,
            false,
            None,
            or_popups,
            extra_top,
            |id| state.managed_windows.get(&id),
            dmabuf,
        ),
        ScreencastTarget::Window { window_id } => {
            let wid = WindowId(*window_id);
            let win = state.managed_windows.get(&wid);
            let cursor_info = if overlay_cursor
                && let Some(w) = win
                && let Some(rec) = state.wm.get(wid)
                && rec.is_visible_on(state.wm.active_workspace_id())
            {
                let win_loc = state
                    .space
                    .element_location(w)
                    .unwrap_or_else(|| from_wm_point(rec.current_geometry.loc));
                Some((state.pointer_location, win_loc, &state.cursor_status))
            } else {
                None
            };
            let res =
                capture_window_to_dmabuf(renderer, neon_renderer, output, win, cursor_info, dmabuf);
            if let Some(w) = win {
                w.send_frame(output, now_dur, Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
            res
        }
        ScreencastTarget::Workspace { index } => {
            if *index == state.wm.workspaces().active_index() {
                capture_output_to_dmabuf(
                    renderer,
                    neon_renderer,
                    output,
                    &state.scene,
                    blur,
                    false,
                    None,
                    or_popups,
                    extra_top,
                    |id| state.managed_windows.get(&id),
                    dmabuf,
                )
            } else {
                let ws_wins = state.workspace_windows_front_to_back(*index);
                let res =
                    capture_workspace_to_dmabuf(renderer, output, &ws_wins, Vec::new(), dmabuf);
                for (w, _, _) in &ws_wins {
                    w.send_frame(output, now_dur, Some(Duration::ZERO), |_, _| {
                        Some(output.clone())
                    });
                }
                res
            }
        }
    }
}

/// Renders queued fast GPU preview thumbnails (Displays, Windows, Workspaces) for
/// `neon-screencast-picker` and encodes PNGs asynchronously on background threads.
pub fn fulfill_screencast_previews(
    renderer: &mut GlesRenderer,
    neon_renderer: &mut NeonRenderer,
    output: &Output,
    state: &NeonState,
    previews: Vec<PendingPreviewCapture>,
) {
    let out_log = state.output_logical_size();
    let out_w = out_log.w.max(1) as f64;
    let out_h = out_log.h.max(1) as f64;

    for req in previews {
        let mw = req.max_width as i32;
        let mh = req.max_height as i32;

        let result = match req.target {
            ScreencastTarget::Output { .. } => {
                let s = (mw as f64 / out_w).min(mh as f64 / out_h).min(1.0);
                let pw = ((out_w * s).round() as i32).max(16);
                let ph = ((out_h * s).round() as i32).max(16);
                let active_idx = state.wm.workspaces().active_index();
                let ws_wins: Vec<_> = state
                    .workspace_windows_front_to_back(active_idx)
                    .into_iter()
                    .filter(|(w, _, _)| {
                        crate::window::window_id(w)
                            .and_then(|id| state.wm.get(id))
                            .and_then(|r| r.app_id.as_deref())
                            .map(|a| !a.to_lowercase().contains("screencast-picker"))
                            .unwrap_or(true)
                    })
                    .collect();
                let elements =
                    collect_workspace_capture_elements(renderer, output, &ws_wins, s, true);
                capture_preview_png_async(
                    renderer,
                    &elements,
                    FALLBACK_CLEAR_COLOR,
                    pw,
                    ph,
                    s,
                    req.path,
                )
            }
            ScreencastTarget::Window { window_id } => {
                let wid = WindowId(window_id);
                if let Some(win) = state.managed_windows.get(&wid) {
                    // Preserve each window's true aspect ratio with zero letterboxing so
                    // Mission Control Exposé cards match wide and narrow windows cleanly.
                    let win_sz = win.geometry().size;
                    let (pw, ph) = if win_sz.w > 0 && win_sz.h > 0 {
                        let s = (mw as f64 / win_sz.w as f64)
                            .min(mh as f64 / win_sz.h as f64)
                            .min(1.0);
                        (
                            ((win_sz.w as f64 * s).round() as i32).max(16),
                            ((win_sz.h as f64 * s).round() as i32).max(16),
                        )
                    } else {
                        (mw, mh)
                    };
                    let elements =
                        collect_window_capture_elements(renderer, neon_renderer, win, pw, ph, 1.0, None);
                    capture_preview_png_async(
                        renderer,
                        &elements,
                        smithay::backend::renderer::Color32F::TRANSPARENT,
                        pw,
                        ph,
                        1.0,
                        req.path,
                    )
                } else {
                    capture_preview_png_async(
                        renderer,
                        &[],
                        WINDOW_CAPTURE_BG_COLOR,
                        mw,
                        mh,
                        1.0,
                        req.path,
                    )
                }
            }
            ScreencastTarget::Workspace { index } => {
                let s = (mw as f64 / out_w).min(mh as f64 / out_h).min(1.0);
                let pw = ((out_w * s).round() as i32).max(16);
                let ph = ((out_h * s).round() as i32).max(16);
                let ws_wins: Vec<_> = state
                    .workspace_windows_front_to_back(index)
                    .into_iter()
                    .filter(|(w, _, _)| {
                        crate::window::window_id(w)
                            .and_then(|id| state.wm.get(id))
                            .and_then(|r| r.app_id.as_deref())
                            .map(|a| !a.to_lowercase().contains("screencast-picker"))
                            .unwrap_or(true)
                    })
                    .collect();
                let elements =
                    collect_workspace_capture_elements(renderer, output, &ws_wins, s, true);
                capture_preview_png_async(
                    renderer,
                    &elements,
                    FALLBACK_CLEAR_COLOR,
                    pw,
                    ph,
                    s,
                    req.path,
                )
            }
        };

        if let Err(err) = result {
            warn!(%err, "Failed to render screencast preview thumbnail");
        }
    }
}
