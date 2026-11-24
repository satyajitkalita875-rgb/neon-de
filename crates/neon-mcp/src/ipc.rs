//! Live NEON DE compositor IPC socket discovery and request helper.
//!
//! Because MCP hosts (Antigravity, Claude, Cursor, etc.) may spawn `neon-mcp` outside the
//! compositor's direct process tree without `NEON_IPC_SOCKET`, this module automatically probes
//! explicit CLI/env paths first and then scans `$XDG_RUNTIME_DIR/neon-de/ipc-*.sock` for a live,
//! responsive compositor socket (skipping stale sockets left by earlier sessions).

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use neon_ipc::{IpcClient, IpcPayload, IpcRequest, IpcResult, current_uid, default_socket_path};

/// Connection context shared across MCP tool & resource handlers.
#[derive(Debug, Clone)]
pub struct NeonBridge {
    explicit_socket: Option<PathBuf>,
}

impl NeonBridge {
    pub fn new(explicit_socket: Option<PathBuf>) -> Self {
        Self { explicit_socket }
    }

    /// Resolves the active, connectable NEON DE IPC socket path.
    pub fn resolve_socket_path(&self) -> Result<PathBuf, String> {
        self.connect().map(|(path, _)| path)
    }

    /// Connects directly to the active NEON DE IPC socket in a single handshake (no extra probe).
    pub fn connect(&self) -> Result<(PathBuf, IpcClient), String> {
        if let Some(ref p) = self.explicit_socket {
            return IpcClient::connect(p)
                .map(|c| (p.clone(), c))
                .map_err(|e| {
                    format!(
                        "specified NEON DE IPC socket is not reachable ({}): {e}",
                        p.display()
                    )
                });
        }

        if let Some(env_path) = env::var_os("NEON_IPC_SOCKET") {
            let p = PathBuf::from(env_path);
            if let Ok(c) = IpcClient::connect(&p) {
                return Ok((p, c));
            }
        }

        if let Ok(wl) = env::var("WAYLAND_DISPLAY")
            && !wl.trim().is_empty()
        {
            let p = default_socket_path(&wl);
            if let Ok(c) = IpcClient::connect(&p) {
                return Ok((p, c));
            }
        }

        // Scan `$XDG_RUNTIME_DIR/neon-de/ipc-*.sock`, preferring newest live socket first.
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", current_uid())));
        let sock_dir = runtime_dir.join("neon-de");
        let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&sock_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.starts_with("ipc-") && name.ends_with(".sock") {
                    let mtime = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    candidates.push((mtime, path));
                }
            }
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in candidates {
            if let Ok(c) = IpcClient::connect(&path) {
                return Ok((path, c));
            }
        }

        Err(format!(
            "no live NEON DE compositor IPC socket found under {} (is neon-compositor running?)",
            sock_dir.display()
        ))
    }

    /// Infers the matching Wayland display name (e.g. `wayland-0`) from a resolved socket path.
    pub fn infer_wayland_display(socket_path: &Path) -> Option<String> {
        let name = socket_path.file_name()?.to_str()?;
        let stripped = name.strip_prefix("ipc-")?.strip_suffix(".sock")?;
        if stripped.is_empty() || stripped == "default" {
            None
        } else {
            Some(stripped.to_string())
        }
    }

    /// Executes a single authenticated [`IpcRequest`] against `neon-compositor`.
    pub fn request(&self, req: IpcRequest) -> Result<IpcPayload, String> {
        let (_sock, mut client) = self.connect()?;
        match client
            .request(req)
            .map_err(|e| format!("NEON DE IPC request failed: {e}"))?
        {
            IpcResult::Ok { payload } => Ok(payload),
            IpcResult::Err { code, message } => {
                Err(format!("compositor rejected request ({code}): {message}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_wayland_display_extracts_socket_suffix() {
        assert_eq!(
            NeonBridge::infer_wayland_display(Path::new("/run/user/1000/neon-de/ipc-wayland-0.sock")),
            Some("wayland-0".to_string())
        );
        assert_eq!(
            NeonBridge::infer_wayland_display(Path::new("/run/user/1000/neon-de/ipc-default.sock")),
            None
        );
    }
}
