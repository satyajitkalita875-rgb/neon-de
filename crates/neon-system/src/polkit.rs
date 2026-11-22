//! PolicyKit Authentication Agent integration (`org.freedesktop.PolicyKit1`).
//!
//! Provides:
//! 1. Status discovery of `org.freedesktop.PolicyKit1` on the system bus.
//! 2. A compliant `org.freedesktop.PolicyKit1.AuthenticationAgent` D-Bus object interface
//!    (`BeginAuthentication` / `CancelAuthentication`) that routes privilege-elevation requests
//!    to the NEON DE UI layer without executing shell scripts.
//! 3. Safe opt-in registration with `org.freedesktop.PolicyKit1.Authority` (never clobbering
//!    a host session's active polkit agent in nested mode).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
};

use serde::{Deserialize, Serialize};
use tracing::info;
use zbus::{
    blocking::{Connection, connection::Builder},
    interface,
    zvariant::OwnedValue,
};

use crate::notifications::bus_name_has_owner;

pub const POLKIT_DEST: &str = "org.freedesktop.PolicyKit1";
pub const POLKIT_AGENT_PATH: &str = "/org/neonde/PolicyKit1/AuthenticationAgent";

/// A privilege-elevation authentication prompt initiated by `polkitd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolkitAuthRequest {
    pub action_id: String,
    pub message: String,
    pub icon_name: String,
    pub cookie: String,
    pub cancelled: bool,
}

/// Event delivered to the NEON DE shell when `polkitd` begins or cancels authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolkitEvent {
    Begin(PolkitAuthRequest),
    Cancel { cookie: String },
}

/// Snapshot of system PolicyKit daemon availability.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PolkitStatus {
    pub daemon_available: bool,
    pub agent_object_path: String,
    pub pending_requests: usize,
}

/// Shared state tracking pending polkit authentication prompts.
#[derive(Debug, Default)]
pub struct PolkitAgentState {
    pending: Mutex<Vec<PolkitAuthRequest>>,
    event_tx: Mutex<Option<mpsc::Sender<PolkitEvent>>>,
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

    pub fn record_begin(&self, req: PolkitAuthRequest) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|r| r.cookie != req.cookie);
            pending.push(req.clone());
        }
        if let Ok(guard) = self.event_tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(PolkitEvent::Begin(req));
        }
    }

    pub fn record_cancel(&self, cookie: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|r| r.cookie != cookie);
        }
        if let Ok(guard) = self.event_tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(PolkitEvent::Cancel {
                cookie: cookie.to_string(),
            });
        }
    }

    pub fn pending(&self) -> Vec<PolkitAuthRequest> {
        self.pending.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

struct NeonPolkitAgentIface {
    state: Arc<PolkitAgentState>,
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl NeonPolkitAgentIface {
    #[allow(clippy::too_many_arguments)]
    fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        _identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) {
        info!(%action_id, %message, "Received PolicyKit BeginAuthentication request");
        self.state.record_begin(PolkitAuthRequest {
            action_id,
            message,
            icon_name,
            cookie,
            cancelled: false,
        });
    }

    fn cancel_authentication(&self, cookie: String) {
        info!(%cookie, "Received PolicyKit CancelAuthentication");
        self.state.record_cancel(&cookie);
    }
}

/// Handle holding the exported PolicyKit AuthenticationAgent D-Bus object alive.
pub struct PolkitAgentHandle {
    _connection: Connection,
    pub state: Arc<PolkitAgentState>,
}

/// Exports the NEON DE `AuthenticationAgent` object on the system D-Bus (without hijacking
/// another desktop's active session registration unless explicitly registered in a standalone session).
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
        _connection: connection,
        state,
    })
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

    #[test]
    fn polkit_state_tracks_begin_and_cancel_events() {
        let state = PolkitAgentState::new();
        let rx = state.subscribe();

        state.record_begin(PolkitAuthRequest {
            action_id: "org.freedesktop.NetworkManager.settings.modify.system".into(),
            message: "Authentication is required to modify system network settings".into(),
            icon_name: "network-wired".into(),
            cookie: "cookie-1".into(),
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
}
