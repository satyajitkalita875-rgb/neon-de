//! Versioned, authenticated Unix domain socket IPC for **NEON DE**.
//!
//! # Security Guarantees
//! - Socket directory is created with `0700` permissions; socket file is `0600`.
//! - Every connecting peer is verified via Linux `SO_PEERCRED` to enforce same-UID access.
//! - Framed UTF-8 JSON messages are strictly size-bounded (`MAX_FRAME_BYTES`) and schema-validated.
//! - Zero arbitrary shell command execution endpoints exist in the protocol.

use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::fd::AsRawFd,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// Current NEON DE IPC wire protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum accepted single JSON line frame size (256 KiB).
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Computes the standard isolated IPC socket path for a given `WAYLAND_DISPLAY` socket name.
pub fn default_socket_path(wayland_display: &str) -> PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", current_uid())));
    // Keep only a safe single-segment filename from wayland_display.
    let sanitized: String = wayland_display
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    let suffix = if sanitized.is_empty() {
        "default"
    } else {
        &sanitized
    };
    runtime_dir
        .join("neon-de")
        .join(format!("ipc-{suffix}.sock"))
}

/// Returns the current process effective UID.
pub fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

/// Verified Linux `SO_PEERCRED` peer credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// Reads Linux `SO_PEERCRED` from a connected `UnixStream` and verifies same-UID ownership.
pub fn verify_peer_credentials(stream: &UnixStream) -> io::Result<PeerCredentials> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let expected_uid = current_uid();
    if cred.uid != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "IPC peer UID {} does not match compositor UID {}",
                cred.uid, expected_uid
            ),
        ));
    }
    Ok(PeerCredentials {
        pid: cred.pid,
        uid: cred.uid,
        gid: cred.gid,
    })
}

/// Binds a Unix domain socket listener with restricted `0700` directory and `0600` socket permissions.
pub fn bind_ipc_listener(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Event subscription topics supported by `neon-compositor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTopic {
    Windows,
    Workspaces,
    Outputs,
    Overview,
    Effects,
}

/// Snapshot of a single managed toplevel window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    pub workspace_id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub focused: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// Snapshot of a dynamic workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub index: usize,
    pub active: bool,
    pub window_ids: Vec<u64>,
}

/// Snapshot of a logical output/monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub refresh_mhz: i32,
}

/// Snapshot of live compositor effects settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectsInfo {
    pub enabled: bool,
    pub reduced_motion: bool,
    pub shadows_enabled: bool,
    pub borders_enabled: bool,
    pub corner_radius: f32,
    pub speed_scale: f64,
}

/// Complete desktop state snapshot consumed by `neon-shell` and `neon-settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopSnapshot {
    pub active_workspace_index: usize,
    pub overview_open: bool,
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub outputs: Vec<OutputInfo>,
    pub effects: EffectsInfo,
}

/// Typed request envelope sent by IPC clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcRequestEnvelope {
    pub version: u32,
    pub id: u64,
    pub request: IpcRequest,
}

/// All validated operations exposed by `neon-compositor` over IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    GetState,
    GetWindows,
    GetWorkspaces,
    GetOutputs,
    SwitchWorkspace {
        index: usize,
    },
    MoveWindowToWorkspace {
        window_id: u64,
        workspace_index: usize,
    },
    FocusWindow {
        window_id: u64,
    },
    CloseWindow {
        window_id: u64,
    },
    MinimizeWindow {
        window_id: u64,
    },
    RestoreWindow {
        window_id: u64,
    },
    ToggleOverview,
    SetOverview {
        open: bool,
    },
    UpdateEffects {
        enabled: Option<bool>,
        reduced_motion: Option<bool>,
        shadows_enabled: Option<bool>,
        borders_enabled: Option<bool>,
        corner_radius: Option<f32>,
        speed_scale: Option<f64>,
    },
    Subscribe {
        topics: Vec<EventTopic>,
    },
}

/// Structured server message written back to IPC clients (either a response or pushed event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcServerMessage {
    Response {
        version: u32,
        id: u64,
        result: IpcResult,
    },
    Event {
        version: u32,
        event: IpcEvent,
    },
}

/// Result payload of an IPC request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IpcResult {
    Ok { payload: IpcPayload },
    Err { code: String, message: String },
}

/// Typed response payload variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "payload_type", rename_all = "snake_case")]
pub enum IpcPayload {
    Ack,
    State { snapshot: DesktopSnapshot },
    Windows { windows: Vec<WindowInfo> },
    Workspaces { workspaces: Vec<WorkspaceInfo> },
    Outputs { outputs: Vec<OutputInfo> },
    Overview { open: bool },
    Effects { effects: EffectsInfo },
}

/// Asynchronous state-change events pushed to subscribed clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum IpcEvent {
    StateChanged { snapshot: DesktopSnapshot },
    OverviewToggled { open: bool },
}

/// Encodes a serializable IPC message as a newline-delimited JSON line.
pub fn encode_frame<T: Serialize>(msg: &T) -> io::Result<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec(msg).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if bytes.len() + 1 > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame exceeds MAX_FRAME_BYTES",
        ));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Reads a single bounded newline-delimited JSON frame from a buffered reader.
pub fn read_frame<R: BufRead, T: for<'de> Deserialize<'de>>(reader: &mut R) -> io::Result<T> {
    let mut line = String::new();
    let mut total = 0usize;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "IPC connection closed",
            ));
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let chunk = &buf[..=pos];
            total += chunk.len();
            if total > MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "IPC frame exceeds MAX_FRAME_BYTES",
                ));
            }
            let s = std::str::from_utf8(chunk)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            line.push_str(s);
            let consume = pos + 1;
            reader.consume(consume);
            break;
        } else {
            total += buf.len();
            if total > MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "IPC frame exceeds MAX_FRAME_BYTES",
                ));
            }
            let s = std::str::from_utf8(buf)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            line.push_str(s);
            let len = buf.len();
            reader.consume(len);
        }
    }

    serde_json::from_str(line.trim_end())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Synchronous IPC client used by `neon-shell`, `neon-settings`, and tests.
#[derive(Debug)]
pub struct IpcClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl IpcClient {
    /// Connects to a NEON DE IPC Unix domain socket at `path`.
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let writer = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
        })
    }

    /// Sends a typed [`IpcRequest`] and waits for its matching [`IpcResult`].
    pub fn request(&mut self, request: IpcRequest) -> io::Result<IpcResult> {
        let id = self.next_id;
        self.next_id += 1;
        let env = IpcRequestEnvelope {
            version: PROTOCOL_VERSION,
            id,
            request,
        };
        let frame = encode_frame(&env)?;
        self.writer.write_all(&frame)?;
        self.writer.flush()?;

        loop {
            match read_frame::<_, IpcServerMessage>(&mut self.reader)? {
                IpcServerMessage::Response {
                    id: resp_id,
                    result,
                    ..
                } if resp_id == id => return Ok(result),
                IpcServerMessage::Event { .. } | IpcServerMessage::Response { .. } => {
                    // Ignore interleaved events while awaiting a specific request response.
                }
            }
        }
    }

    /// Subscribes to event topics and returns the underlying blocking reader for pushed events.
    pub fn subscribe(&mut self, topics: Vec<EventTopic>) -> io::Result<IpcResult> {
        self.request(IpcRequest::Subscribe { topics })
    }

    /// Reads the next pushed [`IpcServerMessage`] (intended for dedicated subscriber threads).
    pub fn read_next_message(&mut self) -> io::Result<IpcServerMessage> {
        read_frame(&mut self.reader)
    }

    /// Removes read timeout on this connection so a subscriber thread can block indefinitely.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn peer_credentials_and_roundtrip_ipc_request_work() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("neon-test.sock");
        let listener = bind_ipc_listener(&sock_path).unwrap();
        listener.set_nonblocking(false).unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let creds = verify_peer_credentials(&stream).unwrap();
            assert_eq!(creds.uid, current_uid());

            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let req: IpcRequestEnvelope = read_frame(&mut reader).unwrap();
            assert_eq!(req.version, PROTOCOL_VERSION);
            assert_eq!(req.request, IpcRequest::ToggleOverview);

            let resp = IpcServerMessage::Response {
                version: PROTOCOL_VERSION,
                id: req.id,
                result: IpcResult::Ok {
                    payload: IpcPayload::Overview { open: true },
                },
            };
            stream.write_all(&encode_frame(&resp).unwrap()).unwrap();
            stream.flush().unwrap();
        });

        let mut client = IpcClient::connect(&sock_path).unwrap();
        let res = client.request(IpcRequest::ToggleOverview).unwrap();
        assert_eq!(
            res,
            IpcResult::Ok {
                payload: IpcPayload::Overview { open: true }
            }
        );
        server.join().unwrap();
    }

    #[test]
    fn default_socket_path_sanitizes_wayland_display() {
        let p = default_socket_path("../../evil/wayland-2");
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("ipc-"));
        assert!(!name.contains('/'));
    }
}
