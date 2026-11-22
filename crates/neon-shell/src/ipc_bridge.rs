//! Non-blocking IPC bridge connecting `neon-shell`'s GTK4 UI loop to `neon-compositor`.

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use neon_ipc::{
    DesktopSnapshot, EventTopic, IpcClient, IpcEvent, IpcPayload, IpcRequest, IpcResult,
    IpcServerMessage, default_socket_path,
};
use tracing::{debug, info, warn};

/// Incoming updates delivered from `neon-compositor` to the GTK4 main loop.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ShellIpcUpdate {
    Snapshot(DesktopSnapshot),
    OverviewToggled(bool),
    ScreenshotUiRequested,
}

/// Handle used by GTK4 widgets to send fire-and-forget commands to `neon-compositor`.
#[derive(Clone)]
pub struct IpcCommandSender {
    tx: Sender<IpcRequest>,
}

impl IpcCommandSender {
    pub fn send(&self, request: IpcRequest) {
        let _ = self.tx.send(request);
    }
}

/// Resolves the NEON DE IPC socket path from CLI override, `NEON_IPC_SOCKET`, or `WAYLAND_DISPLAY`.
pub fn resolve_ipc_socket_path(override_path: Option<PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p;
    }
    if let Some(env_path) = std::env::var_os("NEON_IPC_SOCKET") {
        return PathBuf::from(env_path);
    }
    let wayland_display =
        std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-1".to_string());
    default_socket_path(&wayland_display)
}

/// Spawns dedicated background command + subscription threads so GTK4 never blocks on socket I/O.
pub fn start_ipc_bridge(socket_path: PathBuf) -> (IpcCommandSender, Receiver<ShellIpcUpdate>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<IpcRequest>();
    let (update_tx, update_rx) = mpsc::channel::<ShellIpcUpdate>();

    // Command sender thread
    {
        let cmd_socket = socket_path.clone();
        thread::spawn(move || {
            let mut client: Option<IpcClient> = None;
            while let Ok(req) = cmd_rx.recv() {
                if client.is_none() {
                    client = IpcClient::connect(&cmd_socket).ok();
                }
                if let Some(c) = client.as_mut() {
                    match c.request(req.clone()) {
                        Ok(res) => debug!(?req, ?res, "IPC command completed"),
                        Err(err) => {
                            warn!(?err, "IPC command failed; reconnecting next time");
                            client = None;
                        }
                    }
                }
            }
        });
    }

    // Subscription / event listener thread with automatic reconnect
    {
        let sub_socket = socket_path;
        thread::spawn(move || {
            loop {
                match IpcClient::connect(&sub_socket) {
                    Ok(mut client) => {
                        info!(path = %sub_socket.display(), "Connected neon-shell IPC subscriber");
                        let sub = client.subscribe(vec![
                            EventTopic::Windows,
                            EventTopic::Workspaces,
                            EventTopic::Outputs,
                            EventTopic::Overview,
                            EventTopic::Effects,
                        ]);
                        if let Ok(IpcResult::Ok {
                            payload: IpcPayload::State { snapshot },
                        }) = sub
                        {
                            let _ = update_tx.send(ShellIpcUpdate::Snapshot(snapshot));
                        }
                        let _ = client.set_read_timeout(None);

                        loop {
                            match client.read_next_message() {
                                Ok(IpcServerMessage::Event { event, .. }) => match event {
                                    IpcEvent::StateChanged { snapshot } => {
                                        if update_tx
                                            .send(ShellIpcUpdate::Snapshot(snapshot))
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    IpcEvent::OverviewToggled { open } => {
                                        if update_tx
                                            .send(ShellIpcUpdate::OverviewToggled(open))
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    IpcEvent::ScreenshotUiRequested => {
                                        if update_tx
                                            .send(ShellIpcUpdate::ScreenshotUiRequested)
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                },
                                Ok(IpcServerMessage::Response { .. }) => {}
                                Err(err) => {
                                    debug!(?err, "IPC subscriber disconnected; retrying");
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(250));
                    }
                }
            }
        });
    }

    (IpcCommandSender { tx: cmd_tx }, update_rx)
}
