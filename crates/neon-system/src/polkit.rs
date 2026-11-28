//! PolicyKit Authentication Agent integration (`org.freedesktop.PolicyKit1`).
//!
//! Provides a complete, production-ready PolicyKit1 Authentication Agent for NEON DE:
//! 1. Status discovery of `org.freedesktop.PolicyKit1` on the system bus.
//! 2. An async `org.freedesktop.PolicyKit1.AuthenticationAgent` D-Bus object interface
//!    (`BeginAuthentication` / `CancelAuthentication`) that keeps `BeginAuthentication` pending
//!    until authentication completes or is cancelled, while allowing concurrent cancellation.
//! 3. Session registration (`RegisterAuthenticationAgent` / `UnregisterAuthenticationAgent`) with
//!    `org.freedesktop.PolicyKit1.Authority` tied to the live `logind` session.
//! 4. Native PAM authentication via Polkit 125+ systemd socket-activated helper
//!    (`/run/polkit/agent-helper.socket`) with automatic fallback to classic setuid
//!    `polkit-agent-helper-1`.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{CStr, CString},
    future::Future,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    pin::Pin,
    process::{Command, Stdio},
    ptr,
    sync::{Arc, Mutex, mpsc},
    task::{Context, Poll, Waker},
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use zbus::{
    DBusError,
    blocking::{Connection, connection::Builder},
    interface,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

use crate::notifications::bus_name_has_owner;

pub const POLKIT_DEST: &str = "org.freedesktop.PolicyKit1";
pub const POLKIT_AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
pub const POLKIT_AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
pub const POLKIT_AGENT_PATH: &str = "/org/neonde/PolicyKit1/AuthenticationAgent";
pub const POLKIT_HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";

const POLKIT_HELPER_CANDIDATES: &[&str] = &[
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-agent-helper-1",
    "/usr/lib/policykit-1/polkit-agent-helper-1",
];

/// D-Bus errors returned from `org.freedesktop.PolicyKit1.AuthenticationAgent`.
#[derive(Debug, DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
pub enum PolkitDbusError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Cancelled(String),
    Failed(String),
}

/// A Unix user identity eligible to satisfy a Polkit authentication request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolkitIdentity {
    pub uid: u32,
    pub username: String,
    pub full_name: String,
}

impl PolkitIdentity {
    pub fn display_label(&self) -> String {
        if self.full_name.is_empty() || self.full_name == self.username {
            self.username.clone()
        } else {
            format!("{} ({})", self.full_name, self.username)
        }
    }
}

/// A privilege-elevation authentication prompt initiated by `polkitd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolkitAuthRequest {
    pub action_id: String,
    pub message: String,
    pub icon_name: String,
    pub details: Vec<(String, String)>,
    pub cookie: String,
    pub identities: Vec<PolkitIdentity>,
    pub cancelled: bool,
}

/// Event delivered to the NEON DE shell when `polkitd` begins, updates, or cancels authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolkitEvent {
    Begin(PolkitAuthRequest),
    Cancel { cookie: String },
    AuthSucceeded { cookie: String },
    AuthFailed { cookie: String, message: String },
}

/// Final outcome resolving an awaited `BeginAuthentication` D-Bus method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolkitOutcome {
    Authenticated,
    Cancelled,
    Failed(String),
}

/// Snapshot of system PolicyKit daemon availability.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PolkitStatus {
    pub daemon_available: bool,
    pub agent_object_path: String,
    pub pending_requests: usize,
}

#[derive(Debug, Default)]
struct OneshotSlot {
    outcome: Option<PolkitOutcome>,
    waker: Option<Waker>,
}

/// Sender half of the zero-dependency async oneshot resolving `BeginAuthentication`.
#[derive(Debug, Clone)]
struct OutcomeSender {
    slot: Arc<Mutex<OneshotSlot>>,
}

impl OutcomeSender {
    fn resolve(&self, outcome: PolkitOutcome) -> bool {
        let waker = {
            let Ok(mut guard) = self.slot.lock() else {
                return false;
            };
            if guard.outcome.is_some() {
                return false;
            }
            guard.outcome = Some(outcome);
            guard.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
        true
    }
}

/// Future half awaited inside `NeonPolkitAgentIface::begin_authentication`.
struct OutcomeWaiter {
    slot: Arc<Mutex<OneshotSlot>>,
}

impl Future for OutcomeWaiter {
    type Output = PolkitOutcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Ok(mut guard) = self.slot.lock() else {
            return Poll::Ready(PolkitOutcome::Cancelled);
        };
        if let Some(outcome) = guard.outcome.take() {
            return Poll::Ready(outcome);
        }
        guard.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

fn outcome_channel() -> (OutcomeSender, OutcomeWaiter) {
    let slot = Arc::new(Mutex::new(OneshotSlot::default()));
    (
        OutcomeSender {
            slot: Arc::clone(&slot),
        },
        OutcomeWaiter { slot },
    )
}

struct PendingEntry {
    request: PolkitAuthRequest,
    responder: Option<OutcomeSender>,
    verifying: bool,
}

/// Shared state tracking pending PolicyKit authentication prompts and bridging D-Bus ↔ UI.
#[derive(Default)]
pub struct PolkitAgentState {
    pending: Mutex<VecDeque<PendingEntry>>,
    event_tx: Mutex<Option<mpsc::Sender<PolkitEvent>>>,
}

impl std::fmt::Debug for PolkitAgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.pending.lock().map(|p| p.len()).unwrap_or(0);
        f.debug_struct("PolkitAgentState")
            .field("pending_count", &count)
            .finish()
    }
}

impl PolkitAgentState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> mpsc::Receiver<PolkitEvent> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut guard) = self.event_tx.lock() {
            *guard = Some(tx);
        }
        rx
    }

    fn emit(&self, event: PolkitEvent) {
        if let Ok(guard) = self.event_tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(event);
        }
    }

    /// Records a new authentication prompt and returns a future that resolves once
    /// authentication succeeds or is cancelled.
    fn register_begin(&self, req: PolkitAuthRequest) -> OutcomeWaiter {
        let (tx, rx) = outcome_channel();
        if let Ok(mut pending) = self.pending.lock() {
            if let Some(pos) = pending.iter().position(|e| e.request.cookie == req.cookie)
                && let Some(old) = pending.remove(pos)
                && let Some(resp) = old.responder
            {
                resp.resolve(PolkitOutcome::Cancelled);
            }
            pending.push_back(PendingEntry {
                request: req.clone(),
                responder: Some(tx),
                verifying: false,
            });
        }
        self.emit(PolkitEvent::Begin(req));
        rx
    }

    /// Synchronous helper for recording a begin event (e.g. UI testing or unit tests).
    pub fn record_begin(&self, req: PolkitAuthRequest) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|e| e.request.cookie != req.cookie);
            pending.push_back(PendingEntry {
                request: req.clone(),
                responder: None,
                verifying: false,
            });
        }
        self.emit(PolkitEvent::Begin(req));
    }

    /// Handles `CancelAuthentication` sent by `polkitd`.
    pub fn record_cancel(&self, cookie: &str) {
        let responder = self.remove_pending(cookie);
        if let Some(resp) = responder {
            resp.resolve(PolkitOutcome::Cancelled);
        }
        self.emit(PolkitEvent::Cancel {
            cookie: cookie.to_string(),
        });
    }

    /// Dismisses a pending request when the user clicks Cancel or presses Escape in the UI.
    pub fn cancel_by_user(&self, cookie: &str) {
        let responder = self.remove_pending(cookie);
        if let Some(resp) = responder {
            resp.resolve(PolkitOutcome::Cancelled);
        }
    }

    /// Cancels all pending requests (e.g. during agent shutdown).
    pub fn cancel_all(&self) {
        let responders: Vec<(String, Option<OutcomeSender>)> = self
            .pending
            .lock()
            .map(|mut p| {
                p.drain(..)
                    .map(|e| (e.request.cookie, e.responder))
                    .collect()
            })
            .unwrap_or_default();
        for (cookie, responder) in responders {
            if let Some(resp) = responder {
                resp.resolve(PolkitOutcome::Cancelled);
            }
            self.emit(PolkitEvent::Cancel { cookie });
        }
    }

    /// Submits the user's password asynchronously via the system Polkit PAM helper.
    ///
    /// Never blocks the caller (runs on a dedicated short-lived worker thread).
    /// Delivers [`PolkitEvent::AuthSucceeded`] or [`PolkitEvent::AuthFailed`] over the event channel.
    pub fn submit_password(self: &Arc<Self>, cookie: String, username: String, password: String) {
        {
            let Ok(mut pending) = self.pending.lock() else {
                return;
            };
            let Some(entry) = pending.iter_mut().find(|e| e.request.cookie == cookie) else {
                debug!(%cookie, "Ignoring password submission for unknown/completed cookie");
                return;
            };
            if entry.verifying {
                debug!(%cookie, "Ignoring duplicate password submission while PAM verification is active");
                return;
            }
            entry.verifying = true;
        }

        let state = Arc::clone(self);
        let secret = ZeroizingString::new(password);
        let _ = thread::Builder::new()
            .name("neon-polkit-helper".into())
            .spawn(move || {
                let result = authenticate_with_helper(&username, &cookie, secret.as_str());
                drop(secret);

                match result {
                    Ok(()) => {
                        info!(%username, "PolicyKit PAM authentication succeeded");
                        let responder = state.remove_pending(&cookie);
                        if let Some(resp) = responder {
                            resp.resolve(PolkitOutcome::Authenticated);
                        }
                        state.emit(PolkitEvent::AuthSucceeded { cookie });
                    }
                    Err(message) => {
                        warn!(%username, %message, "PolicyKit PAM authentication failed");
                        let still_pending = state
                            .pending
                            .lock()
                            .map(|mut pending| {
                                if let Some(entry) =
                                    pending.iter_mut().find(|e| e.request.cookie == cookie)
                                {
                                    entry.verifying = false;
                                    true
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);
                        if still_pending {
                            state.emit(PolkitEvent::AuthFailed { cookie, message });
                        }
                    }
                }
            });
    }

    fn remove_pending(&self, cookie: &str) -> Option<OutcomeSender> {
        let mut pending = self.pending.lock().ok()?;
        let pos = pending.iter().position(|e| e.request.cookie == cookie)?;
        pending.remove(pos).and_then(|e| e.responder)
    }

    pub fn pending(&self) -> Vec<PolkitAuthRequest> {
        self.pending
            .lock()
            .map(|p| p.iter().map(|e| e.request.clone()).collect())
            .unwrap_or_default()
    }
}

/// Sensitive string wrapper that scrubs its backing bytes with zeroes on drop.
struct ZeroizingString(String);

impl ZeroizingString {
    fn new(s: String) -> Self {
        Self(s)
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for ZeroizingString {
    fn drop(&mut self) {
        // SAFETY: Overwriting owned String bytes with 0x00 keeps valid UTF-8 and scrubs memory.
        unsafe {
            let bytes = self.0.as_bytes_mut();
            for b in bytes.iter_mut() {
                ptr::write_volatile(b, 0);
            }
        }
        self.0.clear();
    }
}

/// Parsed wire line emitted by `polkit-agent-helper-1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperLine {
    PromptEchoOff(String),
    PromptEchoOn(String),
    ErrorMsg(String),
    TextInfo(String),
    Success,
    Failure,
    Unknown(String),
}

/// Parses a single newline-stripped line from `polkit-agent-helper-1`.
pub fn parse_helper_line(raw: &str) -> HelperLine {
    let line = raw.trim_end_matches(['\r', '\n']);
    if line == "SUCCESS" {
        return HelperLine::Success;
    }
    if line == "FAILURE" {
        return HelperLine::Failure;
    }
    if let Some(rest) = line.strip_prefix("PAM_PROMPT_ECHO_OFF") {
        return HelperLine::PromptEchoOff(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("PAM_PROMPT_ECHO_ON") {
        return HelperLine::PromptEchoOn(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("PAM_ERROR_MSG") {
        return HelperLine::ErrorMsg(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("PAM_TEXT_INFO") {
        return HelperLine::TextInfo(rest.trim().to_string());
    }
    HelperLine::Unknown(line.to_string())
}

fn friendly_auth_error(raw: Option<String>) -> String {
    match raw {
        Some(msg) if !msg.trim().is_empty() => {
            let trimmed = msg.trim();
            if trimmed.eq_ignore_ascii_case("authentication failure")
                || trimmed.eq_ignore_ascii_case("sorry, try again.")
            {
                "Incorrect password. Please try again.".to_string()
            } else {
                trimmed.to_string()
            }
        }
        _ => "Incorrect password. Please try again.".to_string(),
    }
}

/// Drives the PAM prompt/response loop against an already-initialized helper stream.
pub fn run_helper_conversation<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    password: &str,
) -> Result<(), String> {
    let mut line = String::new();
    let mut password_sent = false;
    let mut last_error: Option<String> = None;

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("Failed reading authentication helper: {e}"))?;
        if n == 0 {
            return Err(friendly_auth_error(last_error));
        }

        match parse_helper_line(&line) {
            HelperLine::PromptEchoOff(_) | HelperLine::PromptEchoOn(_) => {
                if !password_sent {
                    writer
                        .write_all(password.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"))
                        .and_then(|_| writer.flush())
                        .map_err(|e| format!("Failed sending credentials to helper: {e}"))?;
                    password_sent = true;
                } else {
                    // Additional PAM prompt (e.g. fallback retry inside same transaction)
                    let _ = writer.write_all(b"\n").and_then(|_| writer.flush());
                }
            }
            HelperLine::ErrorMsg(msg) => {
                if !msg.is_empty() {
                    last_error = Some(msg);
                }
            }
            HelperLine::TextInfo(info_msg) => {
                debug!(%info_msg, "Polkit PAM info message");
            }
            HelperLine::Success => return Ok(()),
            HelperLine::Failure => return Err(friendly_auth_error(last_error)),
            HelperLine::Unknown(other) => {
                if !other.is_empty() {
                    debug!(%other, "Ignoring unrecognized polkit helper output");
                }
            }
        }
    }
}

/// Authenticates `(username, cookie)` with `password` using the system Polkit helper.
///
/// Prefers Polkit 125+ systemd socket activation (`/run/polkit/agent-helper.socket`) and falls
/// back to spawning the classic setuid `polkit-agent-helper-1` binary on older distributions.
fn authenticate_with_helper(username: &str, cookie: &str, password: &str) -> Result<(), String> {
    if Path::new(POLKIT_HELPER_SOCKET).exists() {
        match authenticate_via_socket(POLKIT_HELPER_SOCKET, username, cookie, password) {
            Ok(()) => return Ok(()),
            Err(SocketHelperError::Connect(err)) => {
                warn!(
                    socket = POLKIT_HELPER_SOCKET,
                    %err,
                    "Could not connect to socket-activated polkit helper; falling back to setuid helper"
                );
            }
            Err(SocketHelperError::Auth(msg)) => return Err(msg),
        }
    }

    authenticate_via_setuid_binary(username, cookie, password)
}

enum SocketHelperError {
    Connect(String),
    Auth(String),
}

fn authenticate_via_socket(
    socket_path: &str,
    username: &str,
    cookie: &str,
    password: &str,
) -> Result<(), SocketHelperError> {
    let mut stream =
        UnixStream::connect(socket_path).map_err(|e| SocketHelperError::Connect(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    // Polkit 125+ `--socket-activated` protocol:
    //   Line 1: <username>\n
    //   Line 2: <cookie>\n
    let handshake = format!("{username}\n{cookie}\n");
    stream
        .write_all(handshake.as_bytes())
        .and_then(|_| stream.flush())
        .map_err(|e| SocketHelperError::Auth(format!("Helper handshake failed: {e}")))?;

    let read_stream = stream
        .try_clone()
        .map_err(|e| SocketHelperError::Auth(format!("Helper stream clone failed: {e}")))?;
    let mut reader = BufReader::new(read_stream);
    run_helper_conversation(&mut reader, &mut stream, password).map_err(SocketHelperError::Auth)
}

fn authenticate_via_setuid_binary(
    username: &str,
    cookie: &str,
    password: &str,
) -> Result<(), String> {
    let helper_bin = POLKIT_HELPER_CANDIDATES
        .iter()
        .find(|p| Path::new(p).is_file())
        .copied()
        .ok_or_else(|| "System PolicyKit authentication helper not found".to_string())?;

    let mut child = Command::new(helper_bin)
        .arg(username)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start {helper_bin}: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "PolicyKit helper stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "PolicyKit helper stdout unavailable".to_string())?;

    stdin
        .write_all(format!("{cookie}\n").as_bytes())
        .and_then(|_| stdin.flush())
        .map_err(|e| format!("Failed writing cookie to helper: {e}"))?;

    let mut reader = BufReader::new(stdout);
    let result = run_helper_conversation(&mut reader, &mut stdin, password);
    drop(stdin);
    let _ = child.wait();
    result
}

/// Returns the [`PolkitIdentity`] for the current process's real UID.
pub fn current_user_identity() -> Option<PolkitIdentity> {
    let uid = unsafe { libc::getuid() };
    lookup_user_by_uid(uid)
}

/// Resolves a Unix UID to a [`PolkitIdentity`] using thread-safe `getpwuid_r`.
pub fn lookup_user_by_uid(uid: u32) -> Option<PolkitIdentity> {
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut buf = vec![0u8; 4096];
    let mut result: *mut libc::passwd = ptr::null_mut();

    // SAFETY: `pwd` and `buf` are valid writable buffers for `getpwuid_r`.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }

    // SAFETY: `getpwuid_r` succeeded and populated `pwd` with valid C strings inside `buf`.
    let pwd = unsafe { pwd.assume_init() };
    let username = if pwd.pw_name.is_null() {
        return None;
    } else {
        unsafe { CStr::from_ptr(pwd.pw_name) }
            .to_string_lossy()
            .into_owned()
    };
    let gecos = if pwd.pw_gecos.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(pwd.pw_gecos) }
            .to_string_lossy()
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let full_name = if gecos.is_empty() {
        username.clone()
    } else {
        gecos
    };

    Some(PolkitIdentity {
        uid,
        username,
        full_name,
    })
}

fn lookup_user_by_name(name: &str) -> Option<PolkitIdentity> {
    let cname = CString::new(name).ok()?;
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut buf = vec![0u8; 4096];
    let mut result: *mut libc::passwd = ptr::null_mut();

    // SAFETY: `cname`, `pwd`, and `buf` are valid buffers for `getpwnam_r`.
    let rc = unsafe {
        libc::getpwnam_r(
            cname.as_ptr(),
            pwd.as_mut_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: `getpwnam_r` succeeded and populated `pwd`.
    let pwd = unsafe { pwd.assume_init() };
    lookup_user_by_uid(pwd.pw_uid)
}

fn lookup_group_members(gid: u32) -> Vec<PolkitIdentity> {
    let mut grp = std::mem::MaybeUninit::<libc::group>::zeroed();
    let mut buf = vec![0u8; 8192];
    let mut result: *mut libc::group = ptr::null_mut();

    // SAFETY: `grp` and `buf` are valid writable buffers for `getgrgid_r`.
    let rc = unsafe {
        libc::getgrgid_r(
            gid,
            grp.as_mut_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return Vec::new();
    }

    // SAFETY: `getgrgid_r` succeeded and populated `grp.gr_mem` as a NULL-terminated array.
    let grp = unsafe { grp.assume_init() };
    let mut members = Vec::new();
    if !grp.gr_mem.is_null() {
        let mut cursor = grp.gr_mem;
        while unsafe { !(*cursor).is_null() } {
            let name = unsafe { CStr::from_ptr(*cursor) }.to_string_lossy();
            if let Some(id) = lookup_user_by_name(&name) {
                members.push(id);
            }
            cursor = unsafe { cursor.add(1) };
        }
    }
    members
}

fn value_as_u32(val: &OwnedValue) -> Option<u32> {
    u32::try_from(val)
        .ok()
        .or_else(|| i32::try_from(val).ok().and_then(|v| u32::try_from(v).ok()))
        .or_else(|| u64::try_from(val).ok().and_then(|v| u32::try_from(v).ok()))
}

/// Resolves Polkit `identities` (`a(sa{sv})`) into deduplicated Unix user accounts,
/// placing the current session user first whenever eligible.
pub fn resolve_identities(
    raw: &[(String, HashMap<String, OwnedValue>)],
) -> Vec<PolkitIdentity> {
    let mut seen_uids = HashSet::new();
    let mut resolved = Vec::new();

    for (kind, details) in raw {
        match kind.as_str() {
            "unix-user" => {
                if let Some(uid) = details.get("uid").and_then(value_as_u32)
                    && seen_uids.insert(uid)
                    && let Some(id) = lookup_user_by_uid(uid)
                {
                    resolved.push(id);
                }
            }
            "unix-group" => {
                if let Some(gid) = details.get("gid").and_then(value_as_u32) {
                    for member in lookup_group_members(gid) {
                        if seen_uids.insert(member.uid) {
                            resolved.push(member);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let current_uid = unsafe { libc::getuid() };
    if resolved.is_empty()
        && let Some(me) = lookup_user_by_uid(current_uid)
    {
        resolved.push(me);
    } else if let Some(pos) = resolved.iter().position(|id| id.uid == current_uid)
        && pos > 0
    {
        let me = resolved.remove(pos);
        resolved.insert(0, me);
    }

    resolved
}

fn filter_user_details(details: HashMap<String, String>) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = details
        .into_iter()
        .filter(|(k, v)| !v.trim().is_empty() && !k.starts_with("polkit.gettext"))
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
}

struct NeonPolkitAgentIface {
    state: Arc<PolkitAgentState>,
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl NeonPolkitAgentIface {
    #[allow(clippy::too_many_arguments)]
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> Result<(), PolkitDbusError> {
        let resolved_ids = resolve_identities(&identities);
        info!(
            %action_id,
            %message,
            identity_count = resolved_ids.len(),
            "Received PolicyKit BeginAuthentication request"
        );
        let waiter = self.state.register_begin(PolkitAuthRequest {
            action_id,
            message,
            icon_name,
            details: filter_user_details(details),
            cookie,
            identities: resolved_ids,
            cancelled: false,
        });

        match waiter.await {
            PolkitOutcome::Authenticated => Ok(()),
            PolkitOutcome::Cancelled => Err(PolkitDbusError::Cancelled(
                "Authentication dialog was dismissed by the user".to_string(),
            )),
            PolkitOutcome::Failed(msg) => Err(PolkitDbusError::Failed(msg)),
        }
    }

    fn cancel_authentication(&self, cookie: String) {
        info!(%cookie, "Received PolicyKit CancelAuthentication");
        self.state.record_cancel(&cookie);
    }
}

/// Handle holding the exported and optionally registered PolicyKit AuthenticationAgent alive.
pub struct PolkitAgentHandle {
    connection: Connection,
    registered_session_id: Option<String>,
    pub state: Arc<PolkitAgentState>,
}

impl PolkitAgentHandle {
    pub fn registered_session_id(&self) -> Option<&str> {
        self.registered_session_id.as_deref()
    }
}

impl Drop for PolkitAgentHandle {
    fn drop(&mut self) {
        self.state.cancel_all();
        if let Some(ref session_id) = self.registered_session_id {
            let mut details: HashMap<&str, Value<'_>> = HashMap::new();
            details.insert("session-id", Value::from(session_id.as_str()));
            let subject: (&str, HashMap<&str, Value<'_>>) = ("unix-session", details);
            if let Err(err) = self.connection.call_method(
                Some(POLKIT_DEST),
                POLKIT_AUTHORITY_PATH,
                Some(POLKIT_AUTHORITY_IFACE),
                "UnregisterAuthenticationAgent",
                &(subject, POLKIT_AGENT_PATH),
            ) {
                debug!(%err, %session_id, "PolicyKit UnregisterAuthenticationAgent on drop failed");
            } else {
                info!(%session_id, "Unregistered NEON DE PolicyKit1 AuthenticationAgent");
            }
        }
    }
}

/// Resolves the current systemd-logind session ID (`XDG_SESSION_ID` or `login1.GetSessionByPID`).
pub fn resolve_session_id(conn: &Connection) -> Option<String> {
    if let Ok(id) = std::env::var("XDG_SESSION_ID") {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let pid = std::process::id();
    let reply = conn
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "GetSessionByPID",
            &(pid,),
        )
        .ok()?;
    let session_path: OwnedObjectPath = reply.body().deserialize().ok()?;
    let id_reply = conn
        .call_method(
            Some("org.freedesktop.login1"),
            session_path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.freedesktop.login1.Session", "Id"),
        )
        .ok()?;
    let val: OwnedValue = id_reply.body().deserialize().ok()?;
    String::try_from(val).ok().filter(|s| !s.trim().is_empty())
}

fn current_locale() -> String {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "en_US.UTF-8".to_string())
}

/// Exports the NEON DE `AuthenticationAgent` object on the system D-Bus without registering it
/// with `org.freedesktop.PolicyKit1.Authority`.
pub fn export_polkit_agent(state: Arc<PolkitAgentState>) -> Result<PolkitAgentHandle, String> {
    let iface = NeonPolkitAgentIface {
        state: Arc::clone(&state),
    };
    let connection = Builder::system()
        .map_err(|e| format!("system D-Bus unavailable: {e}"))?
        .serve_at(POLKIT_AGENT_PATH, iface)
        .map_err(|e| format!("failed to serve {POLKIT_AGENT_PATH}: {e}"))?
        .build()
        .map_err(|e| format!("failed to export PolicyKit agent: {e}"))?;

    info!(
        path = POLKIT_AGENT_PATH,
        "Exported NEON DE PolicyKit1 AuthenticationAgent interface"
    );
    Ok(PolkitAgentHandle {
        connection,
        registered_session_id: None,
        state,
    })
}

/// Exports and registers the NEON DE PolicyKit1 AuthenticationAgent for the active desktop session.
///
/// Honors `NEON_DISABLE_POLKIT_AGENT=1` so nested verification runs never touch a host session.
pub fn start_polkit_agent(state: Arc<PolkitAgentState>) -> Result<PolkitAgentHandle, String> {
    if std::env::var("NEON_DISABLE_POLKIT_AGENT")
        .ok()
        .is_some_and(|v| !v.is_empty() && v != "0")
    {
        return Err("PolicyKit agent disabled by NEON_DISABLE_POLKIT_AGENT".to_string());
    }

    let mut handle = export_polkit_agent(state)?;
    let session_id = resolve_session_id(&handle.connection)
        .ok_or_else(|| "Could not determine active logind session ID".to_string())?;
    let locale = current_locale();

    let mut details: HashMap<&str, Value<'_>> = HashMap::new();
    details.insert("session-id", Value::from(session_id.as_str()));
    let subject: (&str, HashMap<&str, Value<'_>>) = ("unix-session", details);

    handle
        .connection
        .call_method(
            Some(POLKIT_DEST),
            POLKIT_AUTHORITY_PATH,
            Some(POLKIT_AUTHORITY_IFACE),
            "RegisterAuthenticationAgent",
            &(subject, locale.as_str(), POLKIT_AGENT_PATH),
        )
        .map_err(|e| format!("RegisterAuthenticationAgent failed for session '{session_id}': {e}"))?;

    handle.registered_session_id = Some(session_id.clone());
    info!(
        session_id = %session_id,
        locale = %locale,
        path = POLKIT_AGENT_PATH,
        "Registered NEON DE PolicyKit1 AuthenticationAgent with polkitd"
    );
    Ok(handle)
}

/// Queries whether `org.freedesktop.PolicyKit1` is active on the system D-Bus.
pub fn query_polkit_status() -> PolkitStatus {
    let daemon_available = Connection::system()
        .map(|conn| bus_name_has_owner(&conn, POLKIT_DEST))
        .unwrap_or(false);
    PolkitStatus {
        daemon_available,
        agent_object_path: POLKIT_AGENT_PATH.to_string(),
        pending_requests: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn polkit_state_tracks_begin_and_cancel_events() {
        let state = PolkitAgentState::new();
        let rx = state.subscribe();

        state.record_begin(PolkitAuthRequest {
            action_id: "org.freedesktop.NetworkManager.settings.modify.system".into(),
            message: "Authentication is required to modify system network settings".into(),
            icon_name: "network-wired".into(),
            details: Vec::new(),
            cookie: "cookie-1".into(),
            identities: Vec::new(),
            cancelled: false,
        });
        assert_eq!(state.pending().len(), 1);
        assert!(matches!(rx.try_recv(), Ok(PolkitEvent::Begin(_))));

        state.record_cancel("cookie-1");
        assert!(state.pending().is_empty());
        assert!(matches!(
            rx.try_recv(),
            Ok(PolkitEvent::Cancel { ref cookie }) if cookie == "cookie-1"
        ));
    }

    #[test]
    fn polkit_helper_line_parser_handles_all_pam_messages() {
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_OFF Password: \n"),
            HelperLine::PromptEchoOff("Password:".into())
        );
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_ON Verification code: \n"),
            HelperLine::PromptEchoOn("Verification code:".into())
        );
        assert_eq!(
            parse_helper_line("PAM_ERROR_MSG Authentication failure\n"),
            HelperLine::ErrorMsg("Authentication failure".into())
        );
        assert_eq!(
            parse_helper_line("PAM_TEXT_INFO Place finger on reader\n"),
            HelperLine::TextInfo("Place finger on reader".into())
        );
        assert_eq!(parse_helper_line("SUCCESS\n"), HelperLine::Success);
        assert_eq!(parse_helper_line("FAILURE\r\n"), HelperLine::Failure);
    }

    #[test]
    fn polkit_helper_conversation_succeeds_and_fails_cleanly() {
        let mut ok_in = Cursor::new(b"PAM_PROMPT_ECHO_OFF Password: \nSUCCESS\n".to_vec());
        let mut ok_out = Vec::new();
        assert_eq!(
            run_helper_conversation(&mut ok_in, &mut ok_out, "my-secret"),
            Ok(())
        );
        assert_eq!(String::from_utf8(ok_out).unwrap(), "my-secret\n");

        let mut bad_in = Cursor::new(
            b"PAM_PROMPT_ECHO_OFF Password: \nPAM_ERROR_MSG Authentication failure\nFAILURE\n"
                .to_vec(),
        );
        let mut bad_out = Vec::new();
        let err = run_helper_conversation(&mut bad_in, &mut bad_out, "wrong")
            .expect_err("wrong password must fail");
        assert!(err.contains("Incorrect password"));
    }

    #[test]
    fn polkit_oneshot_resolves_on_user_cancel() {
        let state = PolkitAgentState::new();
        let _rx = state.subscribe();
        let mut waiter = state.register_begin(PolkitAuthRequest {
            action_id: "io.snapcraft.snapd.manage".into(),
            message: "Authentication is required to refresh snaps".into(),
            icon_name: String::new(),
            details: Vec::new(),
            cookie: "cookie-snap".into(),
            identities: Vec::new(),
            cancelled: false,
        });
        state.cancel_by_user("cookie-snap");
        assert!(state.pending().is_empty());

        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert_eq!(
            Pin::new(&mut waiter).poll(&mut cx),
            Poll::Ready(PolkitOutcome::Cancelled)
        );
    }

    #[test]
    fn resolve_identities_prefers_current_user() {
        let uid = unsafe { libc::getuid() };
        let mut root_map = HashMap::new();
        root_map.insert("uid".into(), OwnedValue::from(0u32));
        let mut me_map = HashMap::new();
        me_map.insert("uid".into(), OwnedValue::from(uid));

        let ids = resolve_identities(&[
            ("unix-user".into(), root_map),
            ("unix-user".into(), me_map),
        ]);
        assert!(!ids.is_empty());
        assert_eq!(ids[0].uid, uid);
    }
}
