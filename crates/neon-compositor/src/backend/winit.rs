//! Nested `winit` + OpenGL ES (`GlesRenderer`) backend driven by `NeonRenderer`, `SceneGraph`,
//! `zwlr_layer_shell_v1`, and the authenticated NEON DE IPC server.

use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

use anyhow::{Context, Result};
use neon_ipc::default_socket_path;
use neon_renderer::NeonRenderer;
use smithay::{
    backend::{
        renderer::{ImportDma, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    desktop::layer_map_for_output,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop, Interest, Mode as TriggerMode, PostAction,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::Display,
        winit::{dpi::LogicalSize, window::Window as WinitWindow},
    },
    utils::{Rectangle, Transform},
    wayland::socket::ListeningSocketSource,
};
use tracing::{error, info, warn};

use crate::{input::process_input_event, ipc::IpcServer, state::NeonState};

/// Configuration for running the nested `winit` backend.
#[derive(Debug, Clone)]
pub struct WinitConfig {
    pub width: u32,
    pub height: u32,
    pub socket_name: Option<String>,
    pub ipc_socket: Option<PathBuf>,
    pub spawn_shell: bool,
    pub startup_command: Option<String>,
    pub startup_args: Vec<String>,
    pub reduced_motion: bool,
    pub shadows_enabled: bool,
}

/// Runs the NEON DE Wayland compositor inside a nested `winit` window on the host desktop.
pub fn run_winit(config: WinitConfig) -> Result<()> {
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
        width = config.width,
        height = config.height,
        reduced_motion = config.reduced_motion,
        shadows = config.shadows_enabled,
        "Starting NEON DE nested compositor"
    );

    let mut state = NeonState::new(
        &display,
        event_loop.handle(),
        event_loop.get_signal(),
        socket_name.clone(),
    );
    state.effects.config.reduced_motion = config.reduced_motion;
    state.effects.config.shadows_enabled = config.shadows_enabled;
    state.ipc_socket_path = Some(ipc_path.clone());

    let ipc_server = Rc::new(RefCell::new(
        IpcServer::bind(ipc_path).context("Failed to bind NEON DE IPC socket")?,
    ));

    // Initialize nested winit window + GLES renderer.
    let window_attrs = WinitWindow::default_attributes()
        .with_inner_size(LogicalSize::new(config.width as f64, config.height as f64))
        .with_title(format!(
            "NEON DE — Nested ({})",
            socket_name.to_string_lossy()
        ))
        .with_visible(true);

    let (mut backend, winit_events) = winit::init_from_attributes::<GlesRenderer>(window_attrs)
        .map_err(|err| anyhow::anyhow!("Failed to initialize winit GLES backend: {err}"))?;

    let initial_size = backend.window_size();
    let mode = Mode {
        size: initial_size,
        refresh: 60_000,
    };

    let output = Output::new(
        "NEON-Winit-1".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "NEON DE".into(),
            model: "Nested Winit Display".into(),
        },
    );
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(smithay::output::Scale::Fractional(backend.scale_factor())),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.register_output(output.clone(), (0, 0).into());

    // Initialize NEON DE's modular GPU renderer and compile custom GLES shaders.
    let neon_renderer = NeonRenderer::new(backend.renderer(), &output);
    let dmabuf_formats = backend.renderer().dmabuf_formats();

    let backend_cell = Rc::new(RefCell::new((backend, neon_renderer, false)));
    let display_cell = Rc::new(RefCell::new(display));

    // Advertise zwp_linux_dmabuf_v1 if the EGL context supports DMA-BUF textures.
    {
        let backend_for_dma = Rc::clone(&backend_cell);
        state.enable_dmabuf(
            dmabuf_formats,
            Box::new(move |dmabuf| {
                let mut guard = backend_for_dma.borrow_mut();
                guard.0.renderer().import_dmabuf(dmabuf, None).is_ok()
            }),
        );
    }

    // Register incoming IPC socket accepts into calloop.
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

    // Register incoming Wayland client socket connections.
    event_loop
        .handle()
        .insert_source(listening_socket, |client_stream, _, state| {
            state.insert_client(client_stream);
        })
        .map_err(|err| anyhow::anyhow!("Failed to insert listening socket source: {err}"))?;

    // Register Wayland display fd dispatch.
    {
        let display_for_source = Rc::clone(&display_cell);
        let fd = display_for_source
            .borrow_mut()
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .context("Failed to clone Wayland display poll fd")?;

        event_loop
            .handle()
            .insert_source(
                Generic::new(fd, Interest::READ, TriggerMode::Level),
                move |_, _, state| {
                    let mut display = display_for_source.borrow_mut();
                    if let Err(err) = display.dispatch_clients(state) {
                        error!(?err, "Error dispatching Wayland clients");
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| anyhow::anyhow!("Failed to insert Wayland display source: {err}"))?;
    }

    // Register nested winit window + input events.
    {
        let backend_for_winit = Rc::clone(&backend_cell);
        let output_for_winit = output.clone();
        event_loop
            .handle()
            .insert_source(winit_events, move |event, _, state| match event {
                WinitEvent::Resized { size, scale_factor } => {
                    let mode = Mode {
                        size,
                        refresh: 60_000,
                    };
                    output_for_winit.change_current_state(
                        Some(mode),
                        None,
                        Some(smithay::output::Scale::Fractional(scale_factor)),
                        None,
                    );
                    output_for_winit.set_preferred(mode);
                    layer_map_for_output(&output_for_winit).arrange();
                    state.relayout_non_floating_windows();
                }
                WinitEvent::Input(input_event) => {
                    process_input_event(state, input_event);
                }
                WinitEvent::CloseRequested => {
                    info!("Host nested window closed; stopping NEON DE compositor");
                    state.loop_signal.stop();
                }
                WinitEvent::Redraw => {
                    let mut guard = backend_for_winit.borrow_mut();
                    let (ref mut backend, ref mut neon_renderer, ref mut presented_once) = *guard;
                    render_frame(
                        backend,
                        neon_renderer,
                        presented_once,
                        &output_for_winit,
                        state,
                    );
                }
                WinitEvent::Focus(_) => {}
            })
            .map_err(|err| anyhow::anyhow!("Failed to insert winit event source: {err}"))?;
    }

    // Frame pacing & IPC polling timer (~60 Hz).
    {
        let backend_for_timer = Rc::clone(&backend_cell);
        let ipc_for_timer = Rc::clone(&ipc_server);
        let output_for_timer = output.clone();
        event_loop
            .handle()
            .insert_source(
                Timer::from_duration(Duration::from_millis(16)),
                move |_, _, state| {
                    ipc_for_timer.borrow_mut().poll_requests(state);
                    let mut guard = backend_for_timer.borrow_mut();
                    let (ref mut backend, ref mut neon_renderer, ref mut presented_once) = *guard;
                    render_frame(
                        backend,
                        neon_renderer,
                        presented_once,
                        &output_for_timer,
                        state,
                    );
                    TimeoutAction::ToDuration(Duration::from_millis(16))
                },
            )
            .map_err(|err| anyhow::anyhow!("Failed to insert frame pacing timer: {err}"))?;
    }

    // Optionally spawn `neon-shell` alongside the compositor.
    if config.spawn_shell {
        spawn_sibling_binary(&state, "neon-shell");
    }

    // Spawn optional startup command wired strictly to NEON DE's WAYLAND_DISPLAY.
    if let Some(program) = &config.startup_command {
        state.spawn_client(program, &config.startup_args);
    }

    info!(
        wayland_display = ?state.socket_name,
        "NEON DE compositor is ready (connect clients with WAYLAND_DISPLAY={})",
        state.socket_name.to_string_lossy()
    );

    // Main deterministic calloop event loop.
    event_loop
        .run(None, &mut state, |state| {
            state.cleanup_dead_windows();
            ipc_server.borrow_mut().poll_requests(state);
            let mut display = display_cell.borrow_mut();
            if let Err(err) = display.dispatch_clients(state) {
                warn!(?err, "Error dispatching Wayland clients during idle");
            }
            if let Err(err) = display.flush_clients() {
                warn!(?err, "Error flushing Wayland clients");
            }
        })
        .context("Error while running calloop event loop")?;

    info!("NEON DE nested compositor shut down cleanly");
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

fn render_frame(
    backend: &mut winit::WinitGraphicsBackend<GlesRenderer>,
    neon_renderer: &mut NeonRenderer,
    presented_once: &mut bool,
    output: &Output,
    state: &mut NeonState,
) {
    let _animating = state.sync_scene_graph();
    let _dirty_regions = state.scene.take_dirty_regions();

    let size = backend.window_size();
    let full_damage = Rectangle::from_size(size);

    let (should_submit, render_damage) = {
        let age = backend.buffer_age().unwrap_or(0);
        match backend.bind() {
            Ok((renderer, mut framebuffer)) => {
                match neon_renderer.render_scene(
                    output,
                    renderer,
                    &mut framebuffer,
                    age,
                    &state.scene,
                    |id| state.managed_windows.get(&id),
                ) {
                    Ok(res) => match res.damage {
                        Some(damage) => (true, Some(damage.clone())),
                        None => (!*presented_once, None),
                    },
                    Err(err) => {
                        warn!(?err, "Scene render error");
                        (true, Some(vec![full_damage]))
                    }
                }
            }
            Err(err) => {
                warn!(?err, "Failed to bind winit EGL surface");
                return;
            }
        }
    };

    if should_submit {
        if let Err(err) = backend.submit(render_damage.as_deref()) {
            warn!(?err, "Failed to submit winit swapchain frame");
        } else {
            *presented_once = true;
        }
    }

    let now = state.clock.now();
    for window in state.space.elements() {
        window.send_frame(output, now, Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
    for layer in layer_map_for_output(output).layers() {
        layer.send_frame(output, now, Some(Duration::ZERO), |_, _| {
            Some(output.clone())
        });
    }
}
