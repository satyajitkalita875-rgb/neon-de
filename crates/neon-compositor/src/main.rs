//! Entrypoint for `neon-compositor` — the NEON DE Wayland compositor.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use neon_compositor::backend::winit::{WinitConfig, run_winit};
use tracing_subscriber::EnvFilter;

/// NEON DE Wayland Compositor (Nested Development Backend)
#[derive(Debug, Parser)]
#[command(name = "neon-compositor", version, about)]
struct Cli {
    /// Logical width of the nested compositor window.
    #[arg(long, default_value_t = 1280)]
    width: u32,

    /// Logical height of the nested compositor window.
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

    /// Disable compositor animations (accessibility reduced-motion mode).
    #[arg(long, default_value_t = false)]
    reduced_motion: bool,

    /// Disable analytical GPU window shadows.
    #[arg(long, default_value_t = false)]
    no_shadows: bool,

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

    run_winit(WinitConfig {
        width: cli.width,
        height: cli.height,
        socket_name: cli.socket,
        ipc_socket: cli.ipc_socket,
        spawn_shell: cli.shell,
        startup_command: cli.command,
        startup_args: cli.command_args,
        reduced_motion: cli.reduced_motion,
        shadows_enabled: !cli.no_shadows,
    })
}
