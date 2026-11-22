//! NetworkManager D-Bus integration (`org.freedesktop.NetworkManager`).

use serde::{Deserialize, Serialize};
use tracing::debug;
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, Value},
};

const NM_DEST: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const NM_ACTIVE_CONN_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const NM_DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const NM_AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";

/// High-level connectivity state reported by NetworkManager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkConnectivity {
    Offline,
    Connecting,
    ConnectedLocal,
    ConnectedSite,
    ConnectedGlobal,
    Unavailable,
}

impl NetworkConnectivity {
    pub fn from_nm_state(state: u32) -> Self {
        match state {
            20 => Self::Offline,
            40 => Self::Connecting,
            50 => Self::ConnectedLocal,
            60 => Self::ConnectedSite,
            70 => Self::ConnectedGlobal,
            _ => Self::Unavailable,
        }
    }

    pub fn is_connected(self) -> bool {
        matches!(
            self,
            Self::ConnectedLocal | Self::ConnectedSite | Self::ConnectedGlobal
        )
    }
}

/// Active connection kind (Wi-Fi, Ethernet, VPN, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Wifi,
    Ethernet,
    Vpn,
    Other,
    None,
}

/// Snapshot of system networking state queried over D-Bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkStatus {
    pub available: bool,
    pub connectivity: NetworkConnectivity,
    pub wifi_enabled: bool,
    pub primary_name: Option<String>,
    pub primary_kind: ConnectionKind,
    pub wifi_strength: Option<u8>,
    pub active_connections: Vec<String>,
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self {
            available: false,
            connectivity: NetworkConnectivity::Unavailable,
            wifi_enabled: false,
            primary_name: None,
            primary_kind: ConnectionKind::None,
            wifi_strength: None,
            active_connections: Vec::new(),
        }
    }
}

impl NetworkStatus {
    /// Compact label for the Top Panel status pill.
    pub fn panel_label(&self) -> String {
        if !self.available {
            return "Net: --".to_string();
        }
        if !self.connectivity.is_connected() {
            return if self.wifi_enabled {
                "Wi-Fi: Offln".to_string()
            } else {
                "Wi-Fi: Off".to_string()
            };
        }
        match self.primary_kind {
            ConnectionKind::Wifi => {
                let name = self.primary_name.as_deref().unwrap_or("Wi-Fi");
                match self.wifi_strength {
                    Some(pct) => format!("Wi-Fi {name} ({pct}%)"),
                    None => format!("Wi-Fi {name}"),
                }
            }
            ConnectionKind::Ethernet => "Wired".to_string(),
            ConnectionKind::Vpn => "VPN".to_string(),
            ConnectionKind::Other => self
                .primary_name
                .clone()
                .unwrap_or_else(|| "Online".to_string()),
            ConnectionKind::None => "Online".to_string(),
        }
    }
}

/// Queries live network status from `org.freedesktop.NetworkManager` on the system D-Bus.
pub fn query_network_status() -> NetworkStatus {
    let Ok(conn) = Connection::system() else {
        return NetworkStatus::default();
    };
    query_network_with_connection(&conn)
}

pub(crate) fn query_network_with_connection(conn: &Connection) -> NetworkStatus {
    let Ok(nm) = Proxy::new(conn, NM_DEST, NM_PATH, NM_IFACE) else {
        return NetworkStatus::default();
    };

    let Ok(nm_state) = nm.get_property::<u32>("State") else {
        return NetworkStatus::default();
    };
    let wifi_enabled = nm.get_property::<bool>("WirelessEnabled").unwrap_or(false);
    let connectivity = NetworkConnectivity::from_nm_state(nm_state);

    let active_paths = nm
        .get_property::<Vec<OwnedObjectPath>>("ActiveConnections")
        .unwrap_or_default();
    let primary_path = nm.get_property::<OwnedObjectPath>("PrimaryConnection").ok();

    let mut primary_name = None;
    let mut primary_kind = ConnectionKind::None;
    let mut active_connections = Vec::new();
    let mut wifi_strength = None;

    for path in &active_paths {
        let Ok(ac) = Proxy::new(conn, NM_DEST, path.as_str(), NM_ACTIVE_CONN_IFACE) else {
            continue;
        };
        let id = ac.get_property::<String>("Id").unwrap_or_default();
        let kind_str = ac.get_property::<String>("Type").unwrap_or_default();
        if !id.is_empty() {
            active_connections.push(id.clone());
        }
        let is_primary = primary_path.as_ref().is_some_and(|p| p == path);
        if is_primary || primary_name.is_none() {
            primary_name = if id.is_empty() { None } else { Some(id) };
            primary_kind = classify_nm_type(&kind_str);
            if primary_kind == ConnectionKind::Wifi {
                wifi_strength = query_active_wifi_strength(conn, &ac);
            }
        }
    }

    NetworkStatus {
        available: true,
        connectivity,
        wifi_enabled,
        primary_name,
        primary_kind,
        wifi_strength,
        active_connections,
    }
}

fn classify_nm_type(nm_type: &str) -> ConnectionKind {
    match nm_type {
        "802-11-wireless" => ConnectionKind::Wifi,
        "802-3-ethernet" => ConnectionKind::Ethernet,
        "vpn" | "wireguard" => ConnectionKind::Vpn,
        "" => ConnectionKind::None,
        _ => ConnectionKind::Other,
    }
}

fn query_active_wifi_strength(conn: &Connection, ac: &Proxy<'_>) -> Option<u8> {
    let devices = ac.get_property::<Vec<OwnedObjectPath>>("Devices").ok()?;
    for dev_path in devices {
        let dev = Proxy::new(conn, NM_DEST, dev_path.as_str(), NM_DEVICE_IFACE).ok()?;
        let dev_type = dev.get_property::<u32>("DeviceType").ok()?;
        if dev_type != 2 {
            // 2 = NM_DEVICE_TYPE_WIFI
            continue;
        }
        let wifi = Proxy::new(conn, NM_DEST, dev_path.as_str(), NM_WIRELESS_IFACE).ok()?;
        let ap_path = wifi
            .get_property::<OwnedObjectPath>("ActiveAccessPoint")
            .ok()?;
        if ap_path.as_str() == "/" {
            continue;
        }
        let ap = Proxy::new(conn, NM_DEST, ap_path.as_str(), NM_AP_IFACE).ok()?;
        if let Ok(strength) = ap.get_property::<u8>("Strength") {
            return Some(strength);
        }
    }
    None
}

/// Enables or disables Wi-Fi via `org.freedesktop.NetworkManager`'s `WirelessEnabled` property.
pub fn set_wifi_enabled(enabled: bool) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let nm = Proxy::new(&conn, NM_DEST, NM_PATH, NM_IFACE)
        .map_err(|e| format!("NetworkManager proxy failed: {e}"))?;
    nm.set_property("WirelessEnabled", Value::Bool(enabled))
        .map_err(|e| format!("failed to set WirelessEnabled={enabled}: {e}"))?;
    debug!(enabled, "Updated NetworkManager WirelessEnabled");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_network_states_and_formats_panel_labels() {
        assert_eq!(
            NetworkConnectivity::from_nm_state(70),
            NetworkConnectivity::ConnectedGlobal
        );
        assert!(NetworkConnectivity::ConnectedGlobal.is_connected());
        assert!(!NetworkConnectivity::Offline.is_connected());

        let status = NetworkStatus {
            available: true,
            connectivity: NetworkConnectivity::ConnectedGlobal,
            wifi_enabled: true,
            primary_name: Some("Studio5G".to_string()),
            primary_kind: ConnectionKind::Wifi,
            wifi_strength: Some(84),
            active_connections: vec!["Studio5G".to_string()],
        };
        assert_eq!(status.panel_label(), "Wi-Fi Studio5G (84%)");
    }
}
