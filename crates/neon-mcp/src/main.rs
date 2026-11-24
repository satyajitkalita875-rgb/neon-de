//! Entrypoint for `neon-mcp` — NEON DE's Model Context Protocol (MCP) stdio control server.

mod ipc;
mod protocol;
mod tools;

use std::path::PathBuf;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::ipc::NeonBridge;

/// NEON DE Model Context Protocol (MCP) stdio control server for AI agents.
#[derive(Debug, Parser)]
#[command(name = "neon-mcp", version, about)]
struct Args {
    /// Optional explicit path to the NEON DE compositor IPC socket (auto-discovered if omitted).
    #[arg(long)]
    socket: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // MCP stdio strictly reserves stdout for JSON-RPC 2.0 frames; send all logs to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let bridge = NeonBridge::new(args.socket);
    info!("Starting neon-mcp stdio JSON-RPC 2.0 server");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    protocol::run_stdio_loop(stdin.lock(), stdout.lock(), &bridge)?;
    Ok(())
}
