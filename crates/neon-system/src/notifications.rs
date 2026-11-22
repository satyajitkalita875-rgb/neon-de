//! Freedesktop Desktop Notifications v1.2 D-Bus service (`org.freedesktop.Notifications`).
//!
//! Host-safety invariant:
//! - On a standalone NEON DE session where `org.freedesktop.Notifications` is unowned, NEON DE
//!   claims the standard bus name `org.freedesktop.Notifications`.
//! - Inside a nested session where the host desktop already owns `org.freedesktop.Notifications`,
//!   NEON DE NEVER steals or replaces the host owner; it automatically falls back to
//!   `org.neonde.Notifications` (with the identical `/org/freedesktop/Notifications` object path
//!   and `org.freedesktop.Notifications` interface) so in-session toasts, notification history,
//!   and end-to-end D-Bus tests work without disturbing the host desktop.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zbus::{
    blocking::{Connection, Proxy, connection::Builder},
    interface,
    names::WellKnownName,
    zvariant::{OwnedValue, Value},
};

pub const STANDARD_BUS_NAME: &str = "org.freedesktop.Notifications";
pub const NESTED_BUS_NAME: &str = "org.neonde.Notifications";
pub const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
pub const NOTIFICATIONS_IFACE: &str = "org.freedesktop.Notifications";

const MAX_HISTORY: usize = 64;

/// Notification urgency level from the Freedesktop Notifications spec (`urgency` hint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationUrgency {
    Low,
    Normal,
    Critical,
}

impl NotificationUrgency {
    fn from_hints(hints: &HashMap<String, OwnedValue>) -> Self {
        let Some(val) = hints.get("urgency") else {
            return Self::Normal;
        };
        let byte = u8::try_from(val)
            .ok()
            .or_else(|| i32::try_from(val).ok().map(|v| v as u8))
            .or_else(|| u32::try_from(val).ok().map(|v| v as u8))
            .unwrap_or(1);
        match byte {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}

/// A single desktop notification received over D-Bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopNotification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub urgency: NotificationUrgency,
    pub expire_timeout_ms: i32,
    pub received_unix_secs: u64,
    pub dismissed: bool,
}

/// Event emitted to UI consumers (`neon-shell`) whenever notifications arrive or close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationEvent {
    Added(DesktopNotification),
    Closed { id: u32, reason: u32 },
    Cleared,
}

/// Shared state for the notification service, accessible both by D-Bus and by the UI.
#[derive(Debug)]
pub struct NotificationStore {
    next_id: AtomicU32,
    do_not_disturb: AtomicBool,
    items: Mutex<VecDeque<DesktopNotification>>,
    event_tx: Mutex<Option<mpsc::Sender<NotificationEvent>>>,
}

impl Default for NotificationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationStore {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU32::new(1),
            do_not_disturb: AtomicBool::new(false),
            items: Mutex::new(VecDeque::new()),
            event_tx: Mutex::new(None),
        }
    }

    pub fn subscribe(&self) -> mpsc::Receiver<NotificationEvent> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut guard) = self.event_tx.lock() {
            *guard = Some(tx);
        }
        rx
    }

    pub fn set_do_not_disturb(&self, enabled: bool) {
        self.do_not_disturb.store(enabled, Ordering::Relaxed);
    }

    pub fn is_do_not_disturb(&self) -> bool {
        self.do_not_disturb.load(Ordering::Relaxed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &self,
        replaces_id: u32,
        app_name: String,
        app_icon: String,
        summary: String,
        body: String,
        urgency: NotificationUrgency,
        expire_timeout_ms: i32,
    ) -> DesktopNotification {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let item = DesktopNotification {
            id,
            app_name,
            app_icon,
            summary,
            body,
            urgency,
            expire_timeout_ms: if expire_timeout_ms < 0 {
                5000
            } else {
                expire_timeout_ms
            },
            received_unix_secs: now,
            dismissed: false,
        };

        if let Ok(mut items) = self.items.lock() {
            items.retain(|existing| existing.id != id);
            items.push_front(item.clone());
            while items.len() > MAX_HISTORY {
                items.pop_back();
            }
        }
        self.emit(NotificationEvent::Added(item.clone()));
        item
    }

    pub fn dismiss(&self, id: u32, reason: u32) -> bool {
        let mut found = false;
        if let Ok(mut items) = self.items.lock() {
            for item in items.iter_mut() {
                if item.id == id {
                    item.dismissed = true;
                    found = true;
                }
            }
        }
        if found {
            self.emit(NotificationEvent::Closed { id, reason });
        }
        found
    }

    pub fn clear_all(&self) {
        if let Ok(mut items) = self.items.lock() {
            items.clear();
        }
        self.emit(NotificationEvent::Cleared);
    }

    pub fn history(&self) -> Vec<DesktopNotification> {
        self.items
            .lock()
            .map(|items| items.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn active_count(&self) -> usize {
        self.items
            .lock()
            .map(|items| items.iter().filter(|n| !n.dismissed).count())
            .unwrap_or(0)
    }

    fn emit(&self, event: NotificationEvent) {
        if let Ok(guard) = self.event_tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(event);
        }
    }
}

/// D-Bus `org.freedesktop.Notifications` v1.2 object exported on the session bus.
struct NeonNotificationsIface {
    store: Arc<NotificationStore>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NeonNotificationsIface {
    fn get_capabilities(&self) -> Vec<String> {
        vec![
            "body".to_string(),
            "actions".to_string(),
            "persistence".to_string(),
            "icon-static".to_string(),
        ]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "NEON DE Notifications".to_string(),
            "NEON DE".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = NotificationUrgency::from_hints(&hints);
        let item = self.store.push(
            replaces_id,
            app_name,
            app_icon,
            summary,
            body,
            urgency,
            expire_timeout,
        );
        item.id
    }

    fn close_notification(&self, id: u32) {
        // Reason 3 = closed by CloseNotification call per spec
        self.store.dismiss(id, 3);
    }
}

/// Live handle keeping the D-Bus notification daemon alive until dropped.
pub struct NotificationDaemonHandle {
    _connection: Connection,
    pub bus_name: String,
    pub store: Arc<NotificationStore>,
}

/// Runtime summary of the notification daemon for Settings / status inspection.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NotificationServiceStatus {
    pub standard_bus_owned: bool,
    pub neon_bus_owned: bool,
    pub active_bus_name: Option<String>,
}

/// Starts NEON DE's Freedesktop Notifications v1.2 D-Bus server on the session bus.
///
/// If `allow_standard_bus` is true and `org.freedesktop.Notifications` is currently unowned,
/// claims `org.freedesktop.Notifications`. Otherwise safely claims `org.neonde.Notifications`
/// (or a custom `explicit_name`) without ever replacing a host desktop's notification daemon.
pub fn start_notification_daemon(
    store: Arc<NotificationStore>,
    explicit_name: Option<&str>,
) -> Result<NotificationDaemonHandle, String> {
    let probe_conn =
        Connection::session().map_err(|e| format!("session D-Bus unavailable: {e}"))?;
    let target_name = if let Some(name) = explicit_name {
        name.to_string()
    } else if !bus_name_has_owner(&probe_conn, STANDARD_BUS_NAME) {
        STANDARD_BUS_NAME.to_string()
    } else {
        NESTED_BUS_NAME.to_string()
    };

    let well_known: WellKnownName<'_> = target_name
        .as_str()
        .try_into()
        .map_err(|e| format!("invalid D-Bus name '{target_name}': {e}"))?;

    let iface = NeonNotificationsIface {
        store: Arc::clone(&store),
    };

    let connection = Builder::session()
        .map_err(|e| format!("failed to build session D-Bus connection: {e}"))?
        .name(well_known)
        .map_err(|e| format!("failed to request D-Bus name '{target_name}': {e}"))?
        .serve_at(NOTIFICATIONS_PATH, iface)
        .map_err(|e| format!("failed to serve {NOTIFICATIONS_PATH}: {e}"))?
        .build()
        .map_err(|e| format!("failed to start notification D-Bus daemon: {e}"))?;

    info!(
        bus_name = %target_name,
        path = NOTIFICATIONS_PATH,
        "Started NEON DE Freedesktop Notifications v1.2 D-Bus daemon"
    );

    Ok(NotificationDaemonHandle {
        _connection: connection,
        bus_name: target_name,
        store,
    })
}

/// Sends a standard Freedesktop `Notify` call over the session D-Bus to `bus_name`.
pub fn send_dbus_notification(
    bus_name: &str,
    app_name: &str,
    summary: &str,
    body: &str,
    urgency: NotificationUrgency,
    expire_timeout_ms: i32,
) -> Result<u32, String> {
    let conn = Connection::session().map_err(|e| format!("session D-Bus unavailable: {e}"))?;
    let proxy = Proxy::new(&conn, bus_name, NOTIFICATIONS_PATH, NOTIFICATIONS_IFACE)
        .map_err(|e| format!("notification proxy failed: {e}"))?;

    let urgency_byte: u8 = match urgency {
        NotificationUrgency::Low => 0,
        NotificationUrgency::Normal => 1,
        NotificationUrgency::Critical => 2,
    };
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("urgency", Value::U8(urgency_byte));

    let actions: Vec<&str> = Vec::new();
    let id: u32 = proxy
        .call(
            "Notify",
            &(
                app_name,
                0u32,
                "dialog-information",
                summary,
                body,
                actions,
                hints,
                expire_timeout_ms,
            ),
        )
        .map_err(|e| format!("D-Bus Notify call to {bus_name} failed: {e}"))?;
    Ok(id)
}

/// Queries whether the standard or NEON DE notification bus name is active on the session bus.
pub fn query_notification_service_status() -> NotificationServiceStatus {
    let Ok(conn) = Connection::session() else {
        return NotificationServiceStatus::default();
    };
    let standard = bus_name_has_owner(&conn, STANDARD_BUS_NAME);
    let neon = bus_name_has_owner(&conn, NESTED_BUS_NAME);
    let active_bus_name = if neon {
        Some(NESTED_BUS_NAME.to_string())
    } else if standard {
        Some(STANDARD_BUS_NAME.to_string())
    } else {
        None
    };
    NotificationServiceStatus {
        standard_bus_owned: standard,
        neon_bus_owned: neon,
        active_bus_name,
    }
}

pub(crate) fn bus_name_has_owner(conn: &Connection, name: &str) -> bool {
    let Ok(dbus) = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) else {
        return false;
    };
    match dbus.call::<_, _, bool>("NameHasOwner", &(name,)) {
        Ok(owned) => owned,
        Err(err) => {
            warn!(name, ?err, "Failed NameHasOwner query");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_store_manages_replace_dismiss_history_and_dnd() {
        let store = NotificationStore::new();
        let rx = store.subscribe();

        let n1 = store.push(
            0,
            "NEON DE".into(),
            "".into(),
            "Wi-Fi Connected".into(),
            "Connected to Studio5G".into(),
            NotificationUrgency::Normal,
            4000,
        );
        assert_eq!(n1.id, 1);
        assert_eq!(store.active_count(), 1);

        let n1_replaced = store.push(
            n1.id,
            "NEON DE".into(),
            "".into(),
            "Wi-Fi Updated".into(),
            "Signal 90%".into(),
            NotificationUrgency::Low,
            3000,
        );
        assert_eq!(n1_replaced.id, 1);
        assert_eq!(store.history().len(), 1);
        assert_eq!(store.history()[0].summary, "Wi-Fi Updated");

        assert!(store.dismiss(1, 2));
        assert_eq!(store.active_count(), 0);

        store.set_do_not_disturb(true);
        assert!(store.is_do_not_disturb());

        assert!(matches!(rx.try_recv(), Ok(NotificationEvent::Added(_))));
        assert!(matches!(rx.try_recv(), Ok(NotificationEvent::Added(_))));
        assert!(matches!(
            rx.try_recv(),
            Ok(NotificationEvent::Closed { id: 1, reason: 2 })
        ));
    }
}
