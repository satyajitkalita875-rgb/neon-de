//! Bare-metal DRM/KMS + GBM + EGL/GLES + libseat + libinput + udev backend for **NEON DE**.
//!
//! Enables standalone bare-metal Wayland sessions (e.g. launched from SDDM/GDM via `neon-session`)
//! with multi-connector discovery, VBlank-synchronized pageflips, DMA-BUF acceleration,
//! software cursor rendering, and safe VT pause/resume.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    os::fd::AsFd,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use neon_ipc::default_socket_path;
use neon_renderer::{
    BlurSettings, FALLBACK_CLEAR_COLOR, LOCK_CURTAIN_COLOR, NeonRenderElement, NeonRenderer,
    screenshot::capture_output_to_png,
};
use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportDma, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::UdevBackend,
    },
    desktop::layer_map_for_output,
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop, Interest, Mode as TriggerMode, PostAction,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{Device as ControlDevice, connector, crtc},
        input::{DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_server::Display,
    },
    utils::{DeviceFd, Transform},
    wayland::socket::ListeningSocketSource,
};
use tracing::{error, info, warn};

use crate::{
    input::process_input_event, ipc::IpcServer, state::NeonState, wayland::xwayland::spawn_xwayland,
};

type NeonDrmManager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;
type NeonDrmOutput =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// Configuration for running the bare-metal DRM/KMS backend.
#[derive(Debug, Clone)]
pub struct DrmConfig {
    pub socket_name: Option<String>,
    pub ipc_socket: Option<PathBuf>,
    pub spawn_shell: bool,
    pub enable_xwayland: bool,
    pub startup_command: Option<String>,
    pub startup_args: Vec<String>,
    pub reduced_motion: bool,
    pub shadows_enabled: bool,
    pub blur_enabled: bool,
    pub safe_mode: bool,
}

struct ActiveDrmOutput {
    crtc: crtc::Handle,
    output: Output,
    drm_output: NeonDrmOutput,
    neon_renderer: NeonRenderer,
    pending_vblank_since: Option<std::time::Instant>,
}

struct DrmRuntime {
    manager: NeonDrmManager,
    renderer: GlesRenderer,
    outputs: HashMap<crtc::Handle, ActiveDrmOutput>,
    paused: bool,
}

/// Runs the NEON DE Wayland compositor directly on bare-metal Linux DRM/KMS + libseat + libinput.
pub fn run_drm(config: DrmConfig) -> Result<()> {
    let mut event_loop: EventLoop<NeonState> =
        EventLoop::try_new().context("Failed to create calloop event loop")?;
    let display: Display<NeonState> = Display::new().context("Failed to create Wayland display")?;

    let listening_socket = match &config.socket_name {
        Some(name) => ListeningSocketSource::with_name(name)
            .with_context(|| format!("Failed to bind Wayland socket '{name}'"))?,
        None => {
            ListeningSocketSource::new_auto().context("Failed to bind automatic Wayland socket")?
        }
    };
    let socket_name = listening_socket.socket_name().to_os_string();
    let ipc_path = config
        .ipc_socket
        .clone()
        .unwrap_or_else(|| default_socket_path(&socket_name.to_string_lossy()));

    info!(
        wayland_display = ?socket_name,
        ipc_socket = %ipc_path.display(),
        safe_mode = config.safe_mode,
        "Starting NEON DE bare-metal DRM/KMS compositor"
    );

    // 1. Open libseat session (handles seatd / systemd-logind DRM & input fd privileges safely).
    let (mut session, session_notifier) = LibSeatSession::new()
        .context("Failed to initialize libseat session (is seatd or systemd-logind active?)")?;
    let seat_name = session.seat();

    let mut state = NeonState::new(
        &display,
        event_loop.handle(),
        event_loop.get_signal(),
        socket_name.clone(),
    );
    state.backend_name = "drm-kms".to_string();
    state.safe_mode = config.safe_mode;
    {
        let mut session_for_vt = session.clone();
        state.vt_switch_hook = Some(Box::new(move |vt| {
            if let Err(err) = session_for_vt.change_vt(vt) {
                warn!(vt, ?err, "Failed to switch VT via libseat");
            }
        }));
    }

    let loaded_cfg = neon_config::load_or_default(&neon_config::default_config_path());
    state.apply_config(loaded_cfg);
    if config.safe_mode {
        state.effects.config.reduced_motion = true;
        state.effects.config.shadows_enabled = false;
        state.effects.config.blur_enabled = false;
    }
    if config.reduced_motion {
        state.effects.config.reduced_motion = true;
    }
    if !config.shadows_enabled {
        state.effects.config.shadows_enabled = false;
    }
    if !config.blur_enabled {
        state.effects.config.blur_enabled = false;
    }
    state.ipc_socket_path = Some(ipc_path.clone());

    let ipc_server = Rc::new(RefCell::new(
        IpcServer::bind(ipc_path).context("Failed to bind NEON DE IPC socket")?,
    ));

    // 2. Discover GPUs via udev and open the primary DRM node with connected outputs.
    let udev_backend = UdevBackend::new(&seat_name)
        .with_context(|| format!("Failed to initialize udev DRM backend for seat '{seat_name}'"))?;
    let gpu_paths: Vec<PathBuf> = udev_backend
        .device_list()
        .map(|(_, path)| path.to_path_buf())
        .collect();
    if gpu_paths.is_empty() {
        bail!("No DRM GPU devices found on seat '{seat_name}'");
    }

    let mut selected_drm = None;
    for path in &gpu_paths {
        let fd = match session.open(
            path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        ) {
            Ok(fd) => fd,
            Err(err) => {
                warn!(?path, ?err, "Skipping DRM device that could not be opened");
                continue;
            }
        };
        let drm_fd = DrmDeviceFd::new(DeviceFd::from(fd));
        let Ok((drm_dev, drm_notifier)) = DrmDevice::new(drm_fd.clone(), false) else {
            continue;
        };
        let has_connected = drm_dev
            .resource_handles()
            .ok()
            .map(|res| {
                res.connectors().iter().any(|&c| {
                    drm_dev
                        .get_connector(c, false)
                        .is_ok_and(|info| info.state() == connector::State::Connected)
                })
            })
            .unwrap_or(false);
        if has_connected || selected_drm.is_none() {
            info!(?path, has_connected, "Selected DRM KMS device");
            selected_drm = Some((path.clone(), drm_fd, drm_dev, drm_notifier));
            if has_connected {
                break;
            }
        }
    }

    let (gpu_path, drm_fd, drm_device, drm_notifier) =
        selected_drm.context("Failed to open any usable DRM KMS device")?;

    // 3. Initialize GBM + EGL + GLES renderer + DrmOutputManager.
    let gbm = GbmDevice::new(drm_fd.clone())
        .with_context(|| format!("Failed to create GBM device on {}", gpu_path.display()))?;
    let egl_display = unsafe { EGLDisplay::new(gbm.clone()) }
        .context("Failed to initialize EGLDisplay on GBM device")?;
    let egl_context = EGLContext::new(&egl_display).context("Failed to create EGLContext")?;
    let mut renderer = unsafe { GlesRenderer::new(egl_context) }
        .context("Failed to initialize GlesRenderer on DRM/GBM")?;

    let render_formats: Vec<_> = renderer
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect();
    let dmabuf_formats = renderer.dmabuf_formats();

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let drm_node = DrmNode::from_file(drm_fd.as_fd()).ok();
    let exporter = GbmFramebufferExporter::new(gbm.clone(), drm_node);
    let mut manager = DrmOutputManager::new(
        drm_device,
        allocator,
        exporter,
        Some(gbm),
        [Fourcc::Argb8888, Fourcc::Xrgb8888],
        render_formats,
    );

    // 4. Discover connected connectors, bind CRTCs, and create NEON DE outputs.
    let mut active_outputs: HashMap<crtc::Handle, ActiveDrmOutput> = HashMap::new();
    let mut used_crtcs: HashSet<crtc::Handle> = HashSet::new();
    let mut next_output_x: i32 = 0;
    let resources = manager
        .device()
        .resource_handles()
        .context("Failed to query DRM resource handles")?;

    for &conn_handle in resources.connectors() {
        let Ok(conn_info) = manager.device().get_connector(conn_handle, true) else {
            continue;
        };
        if conn_info.state() != connector::State::Connected {
            continue;
        }
        let Some(&mode) = conn_info
            .modes()
            .iter()
            .find(|m| {
                m.mode_type()
                    .contains(smithay::reexports::drm::control::ModeTypeFlags::PREFERRED)
            })
            .or_else(|| conn_info.modes().first())
        else {
            continue;
        };

        let chosen_crtc = conn_info
            .current_encoder()
            .and_then(|enc| manager.device().get_encoder(enc).ok())
            .and_then(|enc| enc.crtc())
            .filter(|c| !used_crtcs.contains(c))
            .or_else(|| {
                conn_info.encoders().iter().find_map(|&enc_h| {
                    let enc = manager.device().get_encoder(enc_h).ok()?;
                    resources
                        .filter_crtcs(enc.possible_crtcs())
                        .into_iter()
                        .find(|c| !used_crtcs.contains(c))
                })
            });

        let Some(crtc_handle) = chosen_crtc else {
            warn!(?conn_handle, "No free CRTC available for connected output");
            continue;
        };

        let conn_name = format!("{:?}-{}", conn_info.interface(), conn_info.interface_id());
        let (phys_w, phys_h) = conn_info.size().unwrap_or((0, 0));
        let output = Output::new(
            conn_name.clone(),
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "NEON DE".into(),
                model: conn_name.clone(),
            },
        );
        let output_mode = OutputMode::from(mode);
        output.change_current_state(
            Some(output_mode),
            Some(Transform::Normal),
            Some(smithay::output::Scale::Fractional(1.0)),
            Some((0, 0).into()),
        );
        output.set_preferred(output_mode);

        let empty_elements: DrmOutputRenderElements<GlesRenderer, NeonRenderElement> =
            DrmOutputRenderElements::default();
        match manager.initialize_output(
            crtc_handle,
            mode,
            &[conn_handle],
            &output,
            None,
            &mut renderer,
            &empty_elements,
        ) {
            Ok(drm_output) => {
                let neon_renderer = NeonRenderer::new(&mut renderer, &output);
                state.register_output(output.clone(), (next_output_x, 0).into());
                next_output_x += output_mode.size.w;
                info!(
                    connector = %conn_name,
                    width = output_mode.size.w,
                    height = output_mode.size.h,
                    refresh_mhz = output_mode.refresh,
                    "Initialized bare-metal DRM/KMS output"
                );
                used_crtcs.insert(crtc_handle);
                active_outputs.insert(
                    crtc_handle,
                    ActiveDrmOutput {
                        crtc: crtc_handle,
                        output,
                        drm_output,
                        neon_renderer,
                        pending_vblank_since: None,
                    },
                );
            }
            Err(err) => {
                warn!(connector = %conn_name, ?err, "Failed to initialize DRM output");
            }
        }
    }

    if active_outputs.is_empty() {
        bail!("No connected DRM/KMS displays could be initialized");
    }

    let runtime = Rc::new(RefCell::new(DrmRuntime {
        manager,
        renderer,
        outputs: active_outputs,
        paused: false,
    }));

    // Advertise DMA-BUF formats backed by the DRM GLES renderer.
    {
        let rt_for_dmabuf = Rc::clone(&runtime);
        state.enable_dmabuf(
            dmabuf_formats,
            Box::new(move |dmabuf| {
                rt_for_dmabuf
                    .borrow_mut()
                    .renderer
                    .import_dmabuf(dmabuf, None)
                    .is_ok()
            }),
        );
    }

    // 5. Initialize libinput bound to the libseat session.
    let mut libinput_ctx =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_ctx
        .udev_assign_seat(&seat_name)
        .map_err(|()| anyhow::anyhow!("Failed to assign libinput seat '{seat_name}'"))?;
    let libinput_for_session = libinput_ctx.clone();
    let libinput_backend = LibinputInputBackend::new(libinput_ctx);

    event_loop
        .handle()
        .insert_source(libinput_backend, move |mut event, _, state| {
            if let InputEvent::DeviceAdded { ref mut device } = event {
                let is_kbd = device.has_capability(DeviceCapability::Keyboard);
                let is_ptr = device.has_capability(DeviceCapability::Pointer);
                let is_touch = device.has_capability(DeviceCapability::Touch);
                if device.config_tap_finger_count() > 0 {
                    let _ = device.config_tap_set_enabled(true);
                    let _ = device.config_dwt_set_enabled(true);
                }
                info!(
                    device = %device.name(),
                    keyboard = is_kbd,
                    pointer = is_ptr,
                    touch = is_touch,
                    "Attached libinput device"
                );
            }
            process_input_event(state, event);
        })
        .map_err(|err| anyhow::anyhow!("Failed to insert libinput event source: {err}"))?;

    // 6. Handle VT switch pause/resume via libseat.
    {
        let rt_for_session = Rc::clone(&runtime);
        let mut libinput_ctrl = libinput_for_session;
        event_loop
            .handle()
            .insert_source(session_notifier, move |event, _, _state| match event {
                SessionEvent::PauseSession => {
                    info!("VT switched away — pausing DRM and libinput");
                    libinput_ctrl.suspend();
                    let mut rt = rt_for_session.borrow_mut();
                    rt.paused = true;
                    rt.manager.pause();
                }
                SessionEvent::ActivateSession => {
                    info!("VT reactivated — resuming DRM and libinput");
                    libinput_ctrl.suspend();
                    if let Err(()) = libinput_ctrl.resume() {
                        warn!("Failed to resume libinput context");
                    }
                    let mut rt = rt_for_session.borrow_mut();
                    if let Err(err) = rt.manager.activate(false) {
                        warn!(?err, "Failed to reactivate DRM device");
                    }
                    for out in rt.outputs.values_mut() {
                        out.pending_vblank_since = None;
                        out.drm_output.reset_buffers();
                    }
                    rt.paused = false;
                }
            })
            .map_err(|err| anyhow::anyhow!("Failed to insert libseat notifier: {err}"))?;
    }

    // 7. Handle DRM VBlank completion notifications (triggers immediate VSync-locked redraw if dirty).
    {
        let rt_for_drm = Rc::clone(&runtime);
        event_loop
            .handle()
            .insert_source(drm_notifier, move |event, _meta, state| match event {
                DrmEvent::VBlank(crtc_handle) => {
                    let mut rt = rt_for_drm.borrow_mut();
                    if let Some(out) = rt.outputs.get_mut(&crtc_handle) {
                        if let Err(err) = out.drm_output.frame_submitted() {
                            warn!(?crtc_handle, ?err, "DRM frame_submitted error");
                        }
                        out.pending_vblank_since = None;
                    }
                    if state.needs_redraw {
                        render_all_drm_outputs(&mut rt, state);
                    }
                }
                DrmEvent::Error(err) => {
                    error!(?err, "DRM device event error");
                }
            })
            .map_err(|err| anyhow::anyhow!("Failed to insert DRM notifier: {err}"))?;
    }

    // 7b. Register incoming IPC socket accepts into calloop for instant non-polling IPC response.
    {
        let ipc_for_accept = Rc::clone(&ipc_server);
        let fd = ipc_for_accept
            .borrow()
            .listener()
            .try_clone()
            .context("Failed to clone IPC listener socket")?;
        event_loop
            .handle()
            .insert_source(
                Generic::new(fd, Interest::READ, TriggerMode::Level),
                move |_, _, state| {
                    ipc_for_accept.borrow_mut().poll_requests(state);
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| anyhow::anyhow!("Failed to insert IPC listener source: {err}"))?;
    }

    // 8. Wire Wayland socket & client dispatch.
    event_loop
        .handle()
        .insert_source(listening_socket, |client_stream, _, state| {
            state.insert_client(client_stream);
        })
        .map_err(|err| anyhow::anyhow!("Failed to insert Wayland socket source: {err}"))?;

    let display_cell = Rc::new(RefCell::new(display));
    {
        let display_for_fd = Rc::clone(&display_cell);
        let poll_fd = display_for_fd
            .borrow_mut()
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .context("Failed to clone Wayland display poll fd")?;

        event_loop
            .handle()
            .insert_source(
                Generic::new(poll_fd, Interest::READ, TriggerMode::Level),
                move |_, _, state| {
                    let mut disp = display_for_fd.borrow_mut();
                    if let Err(err) = disp.dispatch_clients(state) {
                        error!(?err, "Failed to dispatch Wayland clients");
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| anyhow::anyhow!("Failed to insert display poll source: {err}"))?;
    }

    // 9. Frame pacing & IPC timer (~60 Hz, synchronized with DRM VBlank).
    {
        let rt_for_timer = Rc::clone(&runtime);
        let ipc_for_timer = Rc::clone(&ipc_server);
        let mut tick_count: u64 = 0;
        event_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_millis(16)),
                move |_, _, state| {
                    tick_count = tick_count.wrapping_add(1);
                    if tick_count % 2 == 0 {
                        ipc_for_timer.borrow_mut().poll_requests(state);
                    }
                    render_all_drm_outputs(&mut rt_for_timer.borrow_mut(), state);
                    TimeoutAction::ToDuration(Duration::from_millis(16))
                },
            )
            .map_err(|err| anyhow::anyhow!("Failed to insert DRM frame timer: {err}"))?;
    }

    if config.enable_xwayland {
        spawn_xwayland(&mut state);
    }
    if config.spawn_shell {
        spawn_sibling_binary(&state, "neon-shell");
    }
    if let Some(program) = &config.startup_command {
        state.spawn_client(program, &config.startup_args);
    }

    info!(
        wayland_display = ?state.socket_name,
        "NEON DE bare-metal DRM/KMS compositor is ready"
    );

    event_loop
        .run(None, &mut state, |state| {
            state.cleanup_dead_windows();
            let mut display = display_cell.borrow_mut();
            if let Err(err) = display.dispatch_clients(state) {
                warn!(?err, "Error dispatching Wayland clients during idle");
            }
            if let Err(err) = display.flush_clients() {
                warn!(?err, "Error flushing Wayland clients");
            }
        })
        .context("Error while running DRM calloop event loop")?;

    info!("NEON DE bare-metal DRM/KMS compositor shut down cleanly");
    Ok(())
}

fn spawn_sibling_binary(state: &NeonState, binary_name: &str) {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(binary_name);
        if sibling.exists() {
            state.spawn_client(&sibling.to_string_lossy(), &[]);
            return;
        }
    }
    state.spawn_client(binary_name, &[]);
}

fn render_all_drm_outputs(rt: &mut DrmRuntime, state: &mut NeonState) {
    if rt.paused {
        return;
    }

    let is_locked = state.is_locked();
    let animating = if !is_locked {
        let anim = state.sync_scene_graph();
        let _dirty = state.scene.take_dirty_regions();
        anim
    } else {
        false
    };
    state.needs_redraw = animating;

    let blur = BlurSettings {
        enabled: state.effects.config.blur_enabled && !state.config.appearance.reduce_blur,
        strength: state.effects.config.blur_strength,
        corner_radius: state.effects.config.corner_radius,
    };
    let cursor_theme = state.config.appearance.cursor_theme.clone();
    let cursor_size = state.config.appearance.cursor_size;
    let pointer_loc = state.pointer_location;
    let now = state.clock.now();

    let crtcs: Vec<crtc::Handle> = rt.outputs.keys().copied().collect();
    // Fulfill queued native screenshot capture cleanly via offscreen GLES readback.
    if let Some((shot_path, region)) = state.pending_screenshot.take()
        && let Some(&primary_crtc) = crtcs.first()
        && let Some(active) = rt.outputs.get_mut(&primary_crtc)
    {
        match capture_output_to_png(
            &mut rt.renderer,
            &mut active.neon_renderer,
            &active.output,
            &state.scene,
            blur,
            is_locked,
            region,
            |id| state.managed_windows.get(&id),
            &shot_path,
        ) {
            Ok(()) => state.notify_screenshot_saved(shot_path),
            Err(err) => warn!(%err, "Failed to capture DRM screenshot"),
        }
    }

    for crtc_handle in crtcs {
        let Some(active) = rt.outputs.get_mut(&crtc_handle) else {
            continue;
        };
        if let Some(since) = active.pending_vblank_since {
            if since.elapsed() < Duration::from_millis(500) {
                continue;
            }
            warn!(crtc = ?active.crtc, "DRM VBlank watchdog expired; recovering output swapchain");
            let _ = active.drm_output.frame_submitted();
            active.pending_vblank_since = None;
        }

        let frame_start = std::time::Instant::now();
        active
            .neon_renderer
            .sync_cursor_config(&cursor_theme, cursor_size);
        let scale_f = active.output.current_scale().fractional_scale();
        let output_origin = active.output.current_location();
        let mut elements = active.neon_renderer.cursor_elements(
            &mut rt.renderer,
            pointer_loc,
            output_origin,
            scale_f,
            &state.cursor_status,
        );
        let clear_color = if is_locked {
            let lock_surface = state.lock_state.surface_for_output(&active.output.name());
            elements.extend(active.neon_renderer.collect_locked_elements(
                &active.output,
                &mut rt.renderer,
                lock_surface,
            ));
            LOCK_CURTAIN_COLOR
        } else {
            let or_popups: Vec<_> = state
                .override_redirect_windows
                .iter()
                .filter_map(|w| {
                    let loc = state
                        .space
                        .element_location(w)
                        .or_else(|| w.x11_surface().map(|s| s.geometry().loc))?;
                    Some((w, loc))
                })
                .collect();
            elements.extend(active.neon_renderer.collect_scene_elements_with_popups(
                &active.output,
                &mut rt.renderer,
                &state.scene,
                blur,
                &or_popups,
                |id| state.managed_windows.get(&id),
            ));
            FALLBACK_CLEAR_COLOR
        };

        match active.drm_output.render_frame(
            &mut rt.renderer,
            &elements,
            clear_color,
            FrameFlags::DEFAULT,
        ) {
            Ok(res) => {
                if !res.is_empty {
                    if let Err(err) = active.drm_output.queue_frame(()) {
                        warn!(crtc = ?active.crtc, ?err, "Failed to queue DRM frame");
                    } else {
                        active.pending_vblank_since = Some(std::time::Instant::now());
                        state.frames_rendered += 1;
                        state.last_frame_time_us = frame_start.elapsed().as_micros() as u64;
                    }
                } else {
                    state.idle_frames_skipped += 1;
                }
            }
            Err(err) => {
                warn!(crtc = ?active.crtc, ?err, "DRM render_frame failed");
            }
        }

        if !is_locked {
            for window in state.space.elements() {
                window.send_frame(&active.output, now, Some(Duration::ZERO), |_, _| {
                    Some(active.output.clone())
                });
            }
            for layer in layer_map_for_output(&active.output).layers() {
                layer.send_frame(&active.output, now, Some(Duration::ZERO), |_, _| {
                    Some(active.output.clone())
                });
            }
        }
    }
}
