//! Production session supervisor for **NEON DE**.
//!
//! Manages the lifecycle of `neon-compositor` and `neon-shell`, provides automatic
//! `neon-shell` crash recovery with crash-loop protection, supports `--safe-mode`
//! recovery boot, and exposes `--diagnostics` for bare-metal & nested QA.

use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Child, Command, ExitCode, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Parser;
use neon_config::{default_config_path, load_or_default};
use neon_ipc::{IpcClient, IpcPayload, IpcRequest, IpcResult, default_socket_path};
use neon_system::query_system_snapshot;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const MAX_SHELL_RESTARTS_IN_WINDOW: usize = 5;
const RESTART_WINDOW: Duration = Duration::from_secs(30);

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_shutdown_signal(_sig: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    // SAFETY: `handle_shutdown_signal` is async-signal-safe (only performs an atomic store).
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_shutdown_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_shutdown_signal as *const () as libc::sighandler_t,
        );
    }
}

/// NEON DE Production Session Supervisor
#[derive(Debug, Parser)]
#[command(name = "neon-session", version, about)]
struct Cli {
    /// Print a full NEON DE health, hardware, D-Bus, and compositor diagnostics report and exit.
    #[arg(long, default_value_t = false)]
    diagnostics: bool,

    /// Force the nested `winit` backend inside an existing Wayland/X11 session.
    #[arg(long, default_value_t = false)]
    nested: bool,

    /// Force the bare-metal DRM/KMS backend (for TTY / display manager sessions).
    #[arg(long, default_value_t = false)]
    drm: bool,

    /// Start in minimal recovery safe mode (disables blur, shadows, and animations).
    #[arg(long, default_value_t = false)]
    safe_mode: bool,

    /// Skip launching `neon-shell` (compositor-only debugging / recovery mode).
    #[arg(long, default_value_t = false)]
    no_shell: bool,

    /// Explicit Wayland display socket name (auto-allocated when omitted).
    #[arg(long)]
    socket: Option<String>,

    /// Nested window width (when using `winit` backend).
    #[arg(long, default_value_t = 1280)]
    width: u32,

    /// Nested window height (when using `winit` backend).
    #[arg(long, default_value_t = 800)]
    height: u32,

    /// Optional initial client application to launch inside the NEON DE session.
    #[arg(short = 'c', long = "command")]
    command: Option<String>,
}

/// Tracks child crash timestamps to allow bounded automatic restarts without infinite crash loops.
#[derive(Debug)]
struct RestartTracker {
    max_restarts: usize,
    window: Duration,
    timestamps: VecDeque<Instant>,
}

impl RestartTracker {
    fn new(max_restarts: usize, window: Duration) -> Self {
        Self {
            max_restarts,
            window,
            timestamps: VecDeque::new(),
        }
    }

    /// Records a crash at `now` and returns `Some(backoff)` if a restart is permitted,
    /// or `None` if the crash-loop threshold has been reached.
    fn record_crash(&mut self, now: Instant) -> Option<Duration> {
        while let Some(&front) = self.timestamps.front() {
            if now.saturating_duration_since(front) > self.window {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }
        if self.timestamps.len() >= self.max_restarts {
            return None;
        }
        self.timestamps.push_back(now);
        let attempt = self.timestamps.len() as u64;
        Some(Duration::from_millis(250 * attempt.min(6)))
    }
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("neon_session=info,neon_system=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    let cli = Cli::parse();
    if cli.diagnostics {
        print_diagnostics_report();
        return ExitCode::SUCCESS;
    }

    match run_session(cli) {
        Ok(code) => code,
        Err(err) => {
            error!(?err, "NEON DE session terminated with error");
            ExitCode::FAILURE
        }
    }
}

fn run_session(cli: Cli) -> Result<ExitCode> {
    let compositor_bin = resolve_binary("neon-compositor");
    let shell_bin = resolve_binary("neon-shell");

    let socket_name = cli.socket.clone().unwrap_or_else(allocate_free_socket_name);
    let ipc_socket = default_socket_path(&socket_name);

    let state_dir = neon_state_dir();
    let _ = fs::create_dir_all(&state_dir);
    let session_log_path = state_dir.join("session.log");
    let standalone_drm = cli.drm || (std::env::var_os("WAYLAND_DISPLAY").is_none() && !cli.nested);
    install_signal_handlers();

    info!(
        compositor = %compositor_bin.display(),
        shell = %shell_bin.display(),
        wayland_display = %socket_name,
        ipc_socket = %ipc_socket.display(),
        safe_mode = cli.safe_mode,
        standalone_drm,
        "Starting NEON DE supervised session"
    );

    let session_cfg = load_or_default(&default_config_path());
    let cursor_theme = if session_cfg.appearance.cursor_theme.trim().is_empty() {
        "macOS".to_string()
    } else {
        session_cfg.appearance.cursor_theme.clone()
    };
    let cursor_size = session_cfg.appearance.cursor_size.clamp(16, 48).to_string();

    let mut comp_cmd = Command::new(&compositor_bin);
    comp_cmd
        .arg("--socket")
        .arg(&socket_name)
        .arg("--ipc-socket")
        .arg(&ipc_socket)
        .env("XCURSOR_THEME", &cursor_theme)
        .env("XCURSOR_SIZE", &cursor_size);

    if standalone_drm
        && let Ok(log_file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&session_log_path)
        && let Ok(err_file) = log_file.try_clone()
    {
        comp_cmd.stdout(Stdio::from(log_file));
        comp_cmd.stderr(Stdio::from(err_file));
    }

    if cli.nested {
        comp_cmd
            .arg("--backend")
            .arg("winit")
            .arg("--width")
            .arg(cli.width.to_string())
            .arg("--height")
            .arg(cli.height.to_string());
    } else if cli.drm {
        comp_cmd.arg("--backend").arg("drm");
    } else {
        comp_cmd
            .arg("--backend")
            .arg("auto")
            .arg("--width")
            .arg(cli.width.to_string())
            .arg("--height")
            .arg(cli.height.to_string());
    }

    if cli.safe_mode {
        comp_cmd.arg("--safe-mode");
    }
    if let Some(ref startup_cmd) = cli.command {
        comp_cmd.arg("--command").arg(startup_cmd);
    }

    let mut compositor = comp_cmd
        .spawn()
        .with_context(|| format!("Failed to launch {}", compositor_bin.display()))?;

    // Wait for compositor IPC readiness (or early exit).
    let ready = wait_for_compositor_ready(&mut compositor, &ipc_socket, Duration::from_secs(12))?;
    if !ready {
        error!("Compositor exited before becoming ready");
        return Ok(ExitCode::FAILURE);
    }

    // Export session environment to user systemd/D-Bus activation when in standalone DRM mode.
    if cli.drm || (std::env::var_os("WAYLAND_DISPLAY").is_none() && !cli.nested) {
        let _ = Command::new("dbus-update-activation-environment")
            .args([
                "--systemd",
                "WAYLAND_DISPLAY",
                "XDG_CURRENT_DESKTOP",
                "XDG_SESSION_TYPE",
                "XCURSOR_THEME",
                "XCURSOR_SIZE",
            ])
            .env("WAYLAND_DISPLAY", &socket_name)
            .env("XDG_CURRENT_DESKTOP", "NEON-DE")
            .env("XDG_SESSION_TYPE", "wayland")
            .env("XCURSOR_THEME", &cursor_theme)
            .env("XCURSOR_SIZE", &cursor_size)
            .status();
        // Refresh XDG Desktop Portals so ScreenCast (xdg-desktop-portal-wlr for OBS/WebRTC)
        // binds cleanly to the live WAYLAND_DISPLAY and XDG_CURRENT_DESKTOP=NEON-DE.
        let _ = Command::new("systemctl")
            .args([
                "--user",
                "try-restart",
                "xdg-desktop-portal.service",
                "xdg-desktop-portal-wlr.service",
            ])
            .spawn();
    }

    let mut shell_child: Option<Child> = if !cli.no_shell {
        Some(spawn_shell(
            &shell_bin,
            &socket_name,
            &ipc_socket,
            &cursor_theme,
            &cursor_size,
        )?)
    } else {
        None
    };

    let mut restart_tracker = RestartTracker::new(MAX_SHELL_RESTARTS_IN_WINDOW, RESTART_WINDOW);

    // Supervisor poll loop: monitors compositor health & auto-restarts `neon-shell` on crash.
    let exit_code = loop {
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            info!("Shutdown signal received; terminating NEON DE session cleanly");
            terminate_child(&mut shell_child);
            terminate_process(&mut compositor);
            break ExitCode::SUCCESS;
        }

        if let Some(status) = compositor.try_wait()? {
            info!(?status, "neon-compositor exited; shutting down session");
            terminate_child(&mut shell_child);
            break if status.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }

        if let Some(ref mut shell) = shell_child
            && let Some(status) = shell.try_wait()?
        {
            warn!(
                ?status,
                "neon-shell exited unexpectedly while compositor is alive"
            );
            shell_child = None;
            if let Some(backoff) = restart_tracker.record_crash(Instant::now()) {
                warn!(
                    delay_ms = backoff.as_millis(),
                    "Automatically restarting neon-shell (crash recovery)"
                );
                thread::sleep(backoff);
                if compositor.try_wait()?.is_none() {
                    match spawn_shell(
                        &shell_bin,
                        &socket_name,
                        &ipc_socket,
                        &cursor_theme,
                        &cursor_size,
                    ) {
                        Ok(child) => {
                            info!(pid = child.id(), "Restarted neon-shell successfully");
                            shell_child = Some(child);
                        }
                        Err(err) => {
                            error!(?err, "Failed to restart neon-shell");
                        }
                    }
                }
            } else {
                error!(
                    max_restarts = MAX_SHELL_RESTARTS_IN_WINDOW,
                    "neon-shell hit crash-loop limit; continuing in compositor-only recovery mode"
                );
            }
        }

        thread::sleep(Duration::from_millis(120));
    };

    Ok(exit_code)
}

fn spawn_shell(
    shell_bin: &Path,
    socket_name: &str,
    ipc_socket: &Path,
    cursor_theme: &str,
    cursor_size: &str,
) -> Result<Child> {
    let mut cmd = Command::new(shell_bin);
    cmd.arg("--ipc-socket")
        .arg(ipc_socket)
        .env("WAYLAND_DISPLAY", socket_name)
        .env("NEON_IPC_SOCKET", ipc_socket)
        .env("XDG_CURRENT_DESKTOP", "NEON-DE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XCURSOR_THEME", cursor_theme)
        .env("XCURSOR_SIZE", cursor_size);
    if let Some(disp) = detect_xwayland_display() {
        cmd.env("DISPLAY", disp);
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn {}", shell_bin.display()))?;
    info!(pid = child.id(), "Launched supervised neon-shell");
    Ok(child)
}

fn detect_xwayland_display() -> Option<String> {
    if let Ok(existing) = std::env::var("DISPLAY")
        && !existing.trim().is_empty()
    {
        return Some(existing);
    }
    if let Ok(entries) = fs::read_dir("/tmp/.X11-unix") {
        let mut sockets: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_prefix('X').map(|n| format!(":{n}"))
            })
            .collect();
        sockets.sort();
        if let Some(first) = sockets.into_iter().next() {
            return Some(first);
        }
    }
    None
}

fn wait_for_compositor_ready(
    compositor: &mut Child,
    ipc_socket: &Path,
    timeout: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if compositor.try_wait()?.is_some() {
            return Ok(false);
        }
        if ipc_socket.exists()
            && let Ok(mut client) = IpcClient::connect(ipc_socket)
            && matches!(
                client.request(IpcRequest::GetDiagnostics),
                Ok(IpcResult::Ok {
                    payload: IpcPayload::Diagnostics { .. }
                })
            )
        {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(80));
    }
    Ok(ipc_socket.exists())
}

fn terminate_child(child: &mut Option<Child>) {
    if let Some(mut c) = child.take() {
        terminate_process(&mut c);
    }
}

fn terminate_process(child: &mut Child) {
    let pid = child.id() as libc::pid_t;
    if pid > 0 {
        // SAFETY: sending SIGTERM to our own direct child process.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn resolve_binary(name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(name);
        if sibling.exists() {
            return sibling;
        }
    }
    PathBuf::from(name)
}

fn allocate_free_socket_name() -> String {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    for idx in 0..64 {
        let candidate = format!("wayland-{idx}");
        if !runtime_dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("wayland-{}", std::process::id())
}

fn neon_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(dir).join("neon-de");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("neon-de");
    }
    PathBuf::from("/tmp/neon-de-state")
}

fn print_diagnostics_report() {
    println!(
        "=== NEON DE Diagnostics Report (v{}) ===",
        env!("CARGO_PKG_VERSION")
    );

    println!("\n[Binaries]");
    for bin in [
        "neon-compositor",
        "neon-shell",
        "neon-settings",
        "neon-lockscreen",
        "neon-session",
    ] {
        let resolved = resolve_binary(bin);
        println!("  {bin:<18} -> {}", resolved.display());
    }

    println!("\n[DRM / KMS Devices]");
    let mut cards = Vec::new();
    if let Ok(entries) = fs::read_dir("/dev/dri") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("card") || name.starts_with("renderD") {
                cards.push(format!("/dev/dri/{name}"));
            }
        }
    }
    cards.sort();
    if cards.is_empty() {
        println!("  (no /dev/dri nodes detected)");
    } else {
        for card in cards {
            println!("  {card}");
        }
    }

    println!("\n[Configuration]");
    let cfg_path = default_config_path();
    let cfg = load_or_default(&cfg_path);
    println!("  Path:              {}", cfg_path.display());
    println!("  Schema version:    v{}", cfg.schema_version);
    println!(
        "  Theme / Accent:    {:?} / {}",
        cfg.appearance.theme, cfg.appearance.accent_color
    );
    println!(
        "  High contrast:     {}",
        cfg.privacy_accessibility.high_contrast
    );

    println!("\n[Linux System Services]");
    let sys = query_system_snapshot();
    println!(
        "  NetworkManager:    available={} ({})",
        sys.network.available,
        sys.network.panel_label()
    );
    println!(
        "  PipeWire Audio:    available={} ({}, {} sinks)",
        sys.audio.available,
        sys.audio.panel_label(),
        sys.audio.sinks.len()
    );
    println!(
        "  BlueZ Bluetooth:   available={} (powered={}, {} paired)",
        sys.bluetooth.available,
        sys.bluetooth.powered,
        sys.bluetooth.devices.len()
    );
    println!(
        "  UPower Battery:    available={} ({}, profile={:?})",
        sys.power.available,
        sys.power.panel_label(),
        sys.power.active_profile
    );
    println!(
        "  Notifications:     standard={} neon={}",
        sys.notifications.standard_bus_owned, sys.notifications.neon_bus_owned
    );
    println!(
        "  PolicyKit1:        available={}",
        sys.polkit.daemon_available
    );
    println!(
        "  XDG Portals:       desktop={} backends={:?}",
        sys.portals.desktop_portal_running, sys.portals.installed_backends
    );

    println!("\n[Active Compositor IPC]");
    if let Some(ipc_env) = std::env::var_os("NEON_IPC_SOCKET") {
        let p = PathBuf::from(ipc_env);
        if let Ok(mut client) = IpcClient::connect(&p)
            && let Ok(IpcResult::Ok {
                payload: IpcPayload::Diagnostics { diagnostics },
            }) = client.request(IpcRequest::GetDiagnostics)
        {
            println!(
                "  backend={} uptime={}s frames={} skipped={} last_us={} windows={} safe_mode={}",
                diagnostics.backend,
                diagnostics.uptime_secs,
                diagnostics.frames_rendered,
                diagnostics.idle_frames_skipped,
                diagnostics.last_frame_time_us,
                diagnostics.window_count,
                diagnostics.safe_mode
            );
        } else {
            println!("  IPC socket {} unreachable", p.display());
        }
    } else {
        println!("  (no NEON_IPC_SOCKET in current environment)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_tracker_enforces_crash_loop_window_and_recovers_after_expiry() {
        let mut tracker = RestartTracker::new(3, Duration::from_secs(10));
        let t0 = Instant::now();

        assert!(tracker.record_crash(t0).is_some());
        assert!(tracker.record_crash(t0 + Duration::from_secs(1)).is_some());
        assert!(tracker.record_crash(t0 + Duration::from_secs(2)).is_some());
        // 4th crash within 10s hits crash-loop limit
        assert!(tracker.record_crash(t0 + Duration::from_secs(3)).is_none());

        // After the 10s window expires, restarts are permitted again
        assert!(tracker.record_crash(t0 + Duration::from_secs(15)).is_some());
    }
}
