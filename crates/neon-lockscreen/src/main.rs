//! Standalone `ext_session_lock_v1` lock screen client for **NEON DE**.
//!
//! Renders an obsidian lock curtain with a NEON cyan accent badge onto every output via
//! `ext_session_lock_v1` and unlocks cleanly upon user confirmation (Enter key) or
//! configured test timeout. Full PAM / polkit authentication integrates in Milestone 7.

use std::{
    io::Write,
    os::fd::AsFd,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm,
        wl_shm_pool, wl_surface,
    },
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};

#[derive(Debug, Parser)]
#[command(name = "neon-lockscreen", version, about)]
struct Cli {
    /// Optional automatic unlock delay in milliseconds (useful for automated verification).
    #[arg(long)]
    auto_unlock_ms: Option<u64>,
}

struct LockOutputSurface {
    surface: wl_surface::WlSurface,
    lock_surface: ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
    _file: Option<tempfile::NamedTempFile>,
    _pool: Option<wl_shm_pool::WlShmPool>,
    _buffer: Option<wl_buffer::WlBuffer>,
}

#[derive(Default)]
struct LockClientState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    lock_manager: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
    outputs: Vec<wl_output::WlOutput>,
    lock: Option<ext_session_lock_v1::ExtSessionLockV1>,
    lock_surfaces: Vec<LockOutputSurface>,
    locked: bool,
    finished: bool,
    unlock_requested: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for LockClientState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(5), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                }
                "wl_output" => {
                    state
                        .outputs
                        .push(registry.bind(name, version.min(4), qh, ()));
                }
                "ext_session_lock_manager_v1" => {
                    state.lock_manager = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for LockClientState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for LockClientState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for LockClientState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for LockClientState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for LockClientState {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for LockClientState {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for LockClientState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && caps.contains(wl_seat::Capability::Keyboard)
            && state.keyboard.is_none()
        {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for LockClientState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: WEnum::Value(wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            // Evdev KEY_ENTER (28), KEY_KPENTER (96), or KEY_SPACE (57) unlocks dev lockscreen.
            if matches!(key, 28 | 96 | 57) {
                info!(key, "Unlock key pressed on NEON DE lockscreen");
                state.unlock_requested = true;
            }
        }
    }
}

impl Dispatch<ext_session_lock_manager_v1::ExtSessionLockManagerV1, ()> for LockClientState {
    fn event(
        _: &mut Self,
        _: &ext_session_lock_manager_v1::ExtSessionLockManagerV1,
        _: ext_session_lock_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for LockClientState {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => {
                info!("Session lock confirmed by neon-compositor");
                state.locked = true;
            }
            ext_session_lock_v1::Event::Finished => {
                warn!("Session lock rejected or terminated by compositor");
                state.finished = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, usize> for LockClientState {
    fn event(
        state: &mut Self,
        lock_surface: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            lock_surface.ack_configure(serial);
            let w = width.max(1) as i32;
            let h = height.max(1) as i32;
            if let Some(shm) = state.shm.clone()
                && let Some(entry) = state.lock_surfaces.get_mut(*index)
                && let Ok((file, pool, buffer)) = render_lock_curtain(&shm, qh, w, h)
            {
                entry.surface.attach(Some(&buffer), 0, 0);
                entry.surface.damage_buffer(0, 0, w, h);
                entry.surface.commit();
                entry._file = Some(file);
                entry._pool = Some(pool);
                entry._buffer = Some(buffer);
            }
        }
    }
}

/// Renders NEON DE's dark obsidian lock screen with a centered luminous cyan lock pill.
fn render_lock_curtain(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<LockClientState>,
    width: i32,
    height: i32,
) -> Result<(
    tempfile::NamedTempFile,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
)> {
    let stride = width * 4;
    let total_bytes = (stride * height) as usize;
    let mut pixels = vec![0u8; total_bytes];

    let cx = width / 2;
    let cy = height / 2;
    let card_half_w = 180;
    let card_half_h = 64;

    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            // Subtle vertical obsidian gradient (#070A12 -> #0F172A)
            let t = y as f32 / (height.max(1) as f32);
            let mut r = (7.0 + 8.0 * t) as u8;
            let mut g = (10.0 + 13.0 * t) as u8;
            let mut b = (18.0 + 24.0 * t) as u8;

            // Centered glass lock card with NEON cyan (#38BDF8) border
            let dx = (x - cx).abs();
            let dy = (y - cy).abs();
            if dx <= card_half_w && dy <= card_half_h {
                let on_border = dx >= card_half_w - 2 || dy >= card_half_h - 2;
                if on_border {
                    r = 0x38;
                    g = 0xBD;
                    b = 0xF8;
                } else {
                    r = 0x13;
                    g = 0x1B;
                    b = 0x2E;
                }

                // Centered lock ring emblem inside card
                let r2 = dx * dx + dy * dy;
                if (16 * 16..=22 * 22).contains(&r2) {
                    r = 0x38;
                    g = 0xBD;
                    b = 0xF8;
                }
            }

            // ARGB8888 little-endian: [B, G, R, A]
            pixels[idx] = b;
            pixels[idx + 1] = g;
            pixels[idx + 2] = r;
            pixels[idx + 3] = 0xFF;
        }
    }

    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(&pixels)?;
    file.flush()?;
    let pool = shm.create_pool(file.as_file().as_fd(), total_bytes as i32, qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
    Ok((file, pool, buffer))
}

fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    let conn = Connection::connect_to_env().context("Failed to connect to Wayland display")?;
    let mut queue = conn.new_event_queue::<LockClientState>();
    let qh = queue.handle();
    let mut state = LockClientState::default();

    let _registry = conn.display().get_registry(&qh, ());
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;

    let compositor = state
        .compositor
        .clone()
        .context("Compositor does not advertise wl_compositor")?;
    let lock_manager = state
        .lock_manager
        .clone()
        .context("Compositor does not advertise ext_session_lock_manager_v1")?;

    let lock = lock_manager.lock(&qh, ());
    for (idx, output) in state.outputs.clone().into_iter().enumerate() {
        let surface = compositor.create_surface(&qh, ());
        let lock_surface = lock.get_lock_surface(&surface, &output, &qh, idx);
        state.lock_surfaces.push(LockOutputSurface {
            surface,
            lock_surface,
            _file: None,
            _pool: None,
            _buffer: None,
        });
    }
    state.lock = Some(lock);
    queue.roundtrip(&mut state)?;

    if state.finished {
        bail!("Session lock was rejected by compositor");
    }

    let start = Instant::now();
    while !state.finished && !state.unlock_requested {
        if let Some(ms) = cli.auto_unlock_ms
            && start.elapsed() >= Duration::from_millis(ms)
        {
            info!(ms, "Auto-unlock timer elapsed");
            break;
        }
        queue.roundtrip(&mut state)?;
        std::thread::sleep(Duration::from_millis(16));
    }

    if let Some(lock) = state.lock.take() {
        for entry in state.lock_surfaces.drain(..) {
            entry.lock_surface.destroy();
            entry.surface.destroy();
        }
        lock.unlock_and_destroy();
        let _ = queue.roundtrip(&mut state);
        conn.flush()?;
        info!("Unlocked NEON DE session cleanly");
    }

    Ok(())
}
