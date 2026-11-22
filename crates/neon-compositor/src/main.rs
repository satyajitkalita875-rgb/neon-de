//! Entrypoint for `neon-compositor` — the NEON DE Wayland compositor.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use neon_compositor::backend::{
    drm::{DrmConfig, run_drm},
    winit::{WinitConfig, run_winit},
};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BackendChoice {
    /// Automatically choose `winit` when `WAYLAND_DISPLAY`/`DISPLAY` is set, else `drm`.
    Auto,
    /// Nested window inside an existing Wayland or X11 host session.
    Winit,
    /// Bare-metal Linux DRM/KMS + GBM + libseat + libinput session.
    Drm,
}

/// NEON DE Wayland Compositor (Nested Winit + Bare-Metal DRM/KMS Backends)
#[derive(Debug, Parser)]
#[command(name = "neon-compositor", version, about)]
struct Cli {
    /// Display backend selection (`auto`, `winit`, or `drm`).
    #[arg(long, value_enum, default_value_t = BackendChoice::Auto)]
    backend: BackendChoice,

    /// Logical width of the nested compositor window (`winit` backend only).
    #[arg(long, default_value_t = 1280)]
    width: u32,

    /// Logical height of the nested compositor window (`winit` backend only).
    #[arg(long, default_value_t = 800)]
    height: u32,

    /// Optional explicit Wayland socket name (defaults to next free wayland-N socket).
    #[arg(long)]
    socket: Option<String>,

    /// Optional explicit NEON DE IPC socket path.
    #[arg(long)]
    ipc_socket: Option<PathBuf>,

    /// Automatically launch `neon-shell` alongside the compositor.
    #[arg(long, default_value_t = false)]
    shell: bool,

    /// Minimal recovery mode: disables blur, shadows, and motion animations.
    #[arg(long, default_value_t = false)]
    safe_mode: bool,

    /// Disable compositor animations (accessibility reduced-motion mode).
    #[arg(long, default_value_t = false)]
    reduced_motion: bool,

    /// Disable analytical GPU window shadows.
    #[arg(long, default_value_t = false)]
    no_shadows: bool,

    /// Disable GPU frosted-glass blur / translucency.
    #[arg(long, default_value_t = false)]
    no_blur: bool,

    /// Disable XWayland legacy X11 compatibility server.
    #[arg(long, default_value_t = false)]
    no_xwayland: bool,

    /// Optional Wayland client command to launch automatically inside NEON DE.
    #[arg(short = 'c', long = "command")]
    command: Option<String>,

    /// Additional arguments passed to `--command`.
    #[arg(last = true)]
    command_args: Vec<String>,
}

fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("neon_compositor=info,neon_renderer=info,smithay=warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    let cli = Cli::parse();

    let resolved_backend = match cli.backend {
        BackendChoice::Auto => {
            if std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var_os("DISPLAY").is_some()
            {
                BackendChoice::Winit
            } else {
                BackendChoice::Drm
            }
        }
        explicit => explicit,
    };

    info!(
        ?resolved_backend,
        safe_mode = cli.safe_mode,
        "Selected NEON DE backend"
    );

    match resolved_backend {
        BackendChoice::Winit | BackendChoice::Auto => run_winit(WinitConfig {
            width: cli.width,
            height: cli.height,
            socket_name: cli.socket,
            ipc_socket: cli.ipc_socket,
            spawn_shell: cli.shell,
            enable_xwayland: !cli.no_xwayland,
            startup_command: cli.command,
            startup_args: cli.command_args,
            reduced_motion: cli.reduced_motion || cli.safe_mode,
            shadows_enabled: !cli.no_shadows && !cli.safe_mode,
            blur_enabled: !cli.no_blur && !cli.safe_mode,
            safe_mode: cli.safe_mode,
        }),
        BackendChoice::Drm => run_drm(DrmConfig {
            socket_name: cli.socket,
            ipc_socket: cli.ipc_socket,
            spawn_shell: cli.shell,
            enable_xwayland: !cli.no_xwayland,
            startup_command: cli.command,
            startup_args: cli.command_args,
            reduced_motion: cli.reduced_motion || cli.safe_mode,
            shadows_enabled: !cli.no_shadows && !cli.safe_mode,
            blur_enabled: !cli.no_blur && !cli.safe_mode,
            safe_mode: cli.safe_mode,
        }),
    }
}
