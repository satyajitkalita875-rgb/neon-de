//! `zwlr_screencopy_manager_v1` (v3) server implementation for **NEON DE**.
//!
//! Enables live Wayland screen capture for `xdg-desktop-portal-wlr` (OBS Studio, browser
//! WebRTC screen sharing, Discord/Meet), `grim`, and `wf-recorder` via client `wl_shm` buffers.

use std::{sync::Mutex, time::Duration};

use neon_renderer::screenshot::copy_rgba_region_to_shm;
use smithay::{
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
    wayland::shm::{BufferData, with_buffer_contents, with_buffer_contents_mut},
};
use tracing::{debug, warn};

use crate::state::NeonState;

/// Per-frame protocol state attached to each `zwlr_screencopy_frame_v1` resource.
#[derive(Debug)]
pub struct ScreencopyFrameData {
    pub output_name: String,
    /// Physical pixel capture box `[x, y, width, height]` within the output buffer.
    pub region: [i32; 4],
    pub overlay_cursor: bool,
    pub used: bool,
}

/// Queued screencopy copy request awaiting the compositor's GPU render pass.
#[derive(Debug)]
pub struct PendingScreencopyFrame {
    pub frame: ZwlrScreencopyFrameV1,
    pub buffer: WlBuffer,
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
        _state: &mut Self,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => {
                init_screencopy_frame(frame, overlay_cursor != 0, &output, None, data_init);
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
    data_init: &mut DataInit<'_, NeonState>,
) {
    let Some(output) = Output::from_resource(wl_output) else {
        let frame = data_init.init(
            frame_new,
            Mutex::new(ScreencopyFrameData {
                output_name: String::new(),
                region: [0, 0, 0, 0],
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
            overlay_cursor,
            used: pw <= 0 || ph <= 0,
        }),
    );

    if pw <= 0 || ph <= 0 {
        frame.failed();
        return;
    }

    let stride = (pw as u32).saturating_mul(4);
    frame.buffer(wl_shm::Format::Argb8888, pw as u32, ph as u32, stride);
    if frame.version() >= 3 {
        frame.buffer_done();
    }
    debug!(
        output = %output.name(),
        width = pw,
        height = ph,
        overlay_cursor,
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

        let (output_name, region, overlay_cursor) = {
            let mut guard = data.lock().expect("screencopy frame lock");
            if guard.used {
                resource.post_error(
                    zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                    "screencopy frame has already been used",
                );
                return;
            }
            guard.used = true;
            (guard.output_name.clone(), guard.region, guard.overlay_cursor)
        };

        // Validate wl_shm buffer attributes strictly before queueing.
        let valid = with_buffer_contents(&buffer, |_, pool_len, bdata: BufferData| {
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

        if !valid {
            resource.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "invalid wl_shm buffer dimensions, stride, or format for screencopy",
            );
            return;
        }

        state.pending_screencopy_frames.push(PendingScreencopyFrame {
            frame: resource.clone(),
            buffer,
            output_name,
            region,
            overlay_cursor,
            with_damage,
        });
        state.needs_redraw = true;
    }
}

/// Completes a queued `zwlr_screencopy_frame_v1` by copying the captured RGBA8 frame
/// into its client `wl_shm` buffer and sending `flags`, optional `damage`, and `ready`.
pub fn complete_screencopy_frame(
    pending: &PendingScreencopyFrame,
    rgba_pixels: &[u8],
    src_w: u32,
    src_h: u32,
    timestamp: Duration,
) {
    if !pending.frame.is_alive() {
        return;
    }

    let copy_result = with_buffer_contents_mut(&pending.buffer, |ptr, pool_len, bdata| {
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
        Ok(Ok(())) => {
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
