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

/// A nearby Wi-Fi access point reported by NetworkManager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiAccessPoint {
    pub ssid: String,
    pub strength: u8,
    pub security: String,
    pub in_use: bool,
}

/// A saved NetworkManager connection profile (Wi-Fi, Ethernet, VPN).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedConnection {
    pub name: String,
    pub uuid: String,
    pub conn_type: String,
    pub device: String,
    pub active: bool,
}

/// Live IPv4 / DNS / interface details for the active primary network interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkDetails {
    pub device: String,
    pub ip4_address: String,
    pub gateway: String,
    pub dns_servers: Vec<String>,
    pub mac_address: String,
}

/// Lists nearby Wi-Fi access points from NetworkManager (`nmcli`).
pub fn list_wifi_networks(rescan: bool) -> Vec<WifiAccessPoint> {
    let mut args = vec![
        "-t",
        "-f",
        "IN-USE,SSID,SIGNAL,SECURITY",
        "dev",
        "wifi",
        "list",
    ];
    if rescan {
        args.extend(["--rescan", "yes"]);
    } else {
        args.extend(["--rescan", "no"]);
    }
    let Ok(out) = std::process::Command::new("nmcli").args(&args).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_nmcli_wifi_list(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_nmcli_wifi_list(output: &str) -> Vec<WifiAccessPoint> {
    let mut by_ssid: std::collections::HashMap<String, WifiAccessPoint> =
        std::collections::HashMap::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split(':').collect();
        if cols.len() < 4 {
            continue;
        }
        let in_use = cols[0].trim() == "*";
        let ssid = cols[1].trim().to_string();
        if ssid.is_empty() {
            continue;
        }
        let strength = cols[2].trim().parse::<u8>().unwrap_or(0).min(100);
        let security = cols[3..].join(":").trim().to_string();
        let entry = WifiAccessPoint {
            ssid: ssid.clone(),
            strength,
            security: if security.is_empty() || security == "--" {
                "Open".into()
            } else {
                security
            },
            in_use,
        };
        by_ssid
            .entry(ssid)
            .and_modify(|e| {
                if entry.in_use || (!e.in_use && entry.strength > e.strength) {
                    *e = entry.clone();
                }
            })
            .or_insert(entry);
    }
    let mut list: Vec<WifiAccessPoint> = by_ssid.into_values().collect();
    list.sort_by(|a, b| b.in_use.cmp(&a.in_use).then_with(|| b.strength.cmp(&a.strength)));
    list
}

/// Connects to a Wi-Fi SSID through NetworkManager without ever logging or storing the password.
pub fn connect_wifi(ssid: &str, password: Option<&str>) -> Result<(), String> {
    if ssid.trim().is_empty() || ssid.contains(['\n', '\r', '\0']) {
        return Err("invalid SSID".into());
    }
    let mut cmd = std::process::Command::new("nmcli");
    cmd.args(["dev", "wifi", "connect", ssid]);
    if let Some(pw) = password
        && !pw.is_empty()
    {
        cmd.args(["password", pw]);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run nmcli: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(())
}

/// Activates or disconnects a saved NetworkManager connection by name.
pub fn set_connection_active(name: &str, up: bool) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("empty connection name".into());
    }
    let verb = if up { "up" } else { "down" };
    let out = std::process::Command::new("nmcli")
        .args(["con", verb, name])
        .output()
        .map_err(|e| format!("failed to run nmcli: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// Lists saved NetworkManager profiles (Wi-Fi, Ethernet, VPN).
pub fn list_saved_connections() -> Vec<SavedConnection> {
    let Ok(out) = std::process::Command::new("nmcli")
        .args(["-t", "-f", "NAME,UUID,TYPE,DEVICE", "con", "show"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut list = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let cols: Vec<&str> = line.split(':').collect();
        if cols.len() < 4 {
            continue;
        }
        let device = cols[3].trim().to_string();
        let active = !device.is_empty() && device != "--";
        list.push(SavedConnection {
            name: cols[0].trim().to_string(),
            uuid: cols[1].trim().to_string(),
            conn_type: cols[2].trim().to_string(),
            device,
            active,
        });
    }
    list
}

/// Queries live IPv4 address, gateway, DNS servers, and MAC for active interfaces.
pub fn query_network_details() -> Vec<NetworkDetails> {
    let Ok(out) = std::process::Command::new("nmcli")
        .args([
            "-t",
            "-f",
            "GENERAL.DEVICE,GENERAL.HWADDR,IP4.ADDRESS,IP4.GATEWAY,IP4.DNS",
            "dev",
            "show",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut results = Vec::new();
    let mut cur = NetworkDetails::default();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !cur.device.is_empty() && cur.device != "lo" && !cur.ip4_address.is_empty() {
                results.push(std::mem::take(&mut cur));
            } else {
                cur = NetworkDetails::default();
            }
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let val = v.replace("\\:", ":").trim().to_string();
            if val.is_empty() || val == "--" {
                continue;
            }
            if k == "GENERAL.DEVICE" {
                cur.device = val;
            } else if k == "GENERAL.HWADDR" {
                cur.mac_address = val;
            } else if k.starts_with("IP4.ADDRESS") && cur.ip4_address.is_empty() {
                cur.ip4_address = val;
            } else if k == "IP4.GATEWAY" {
                cur.gateway = val;
            } else if k.starts_with("IP4.DNS") {
                cur.dns_servers.push(val);
            }
        }
    }
    if !cur.device.is_empty() && cur.device != "lo" && !cur.ip4_address.is_empty() {
        results.push(cur);
    }
    results
}

/// Marks a NetworkManager connection as metered (`yes`, `no`, `unknown`).
pub fn set_connection_metered(name: &str, metered: bool) -> Result<(), String> {
    let mode = if metered { "yes" } else { "no" };
    let out = std::process::Command::new("nmcli")
        .args(["con", "modify", name, "connection.metered", mode])
        .output()
        .map_err(|e| format!("failed to run nmcli: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
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
