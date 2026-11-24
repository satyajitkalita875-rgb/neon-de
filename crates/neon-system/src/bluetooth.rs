//! BlueZ Bluetooth D-Bus integration (`org.bluez`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::debug;
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const BLUEZ_DEST: &str = "org.bluez";
const OBJ_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// A paired or connected Bluetooth device discovered from BlueZ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothDevice {
    pub object_path: String,
    pub name: String,
    pub address: String,
    pub paired: bool,
    pub connected: bool,
    pub icon: Option<String>,
}

/// Snapshot of system Bluetooth adapter & device status.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BluetoothStatus {
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    pub adapter_path: Option<String>,
    pub adapter_alias: Option<String>,
    pub devices: Vec<BluetoothDevice>,
}

impl BluetoothStatus {
    pub fn connected_count(&self) -> usize {
        self.devices.iter().filter(|d| d.connected).count()
    }
}

/// Queries BlueZ adapters and paired/connected devices over the system D-Bus.
pub fn query_bluetooth_status() -> BluetoothStatus {
    let Ok(conn) = Connection::system() else {
        return BluetoothStatus::default();
    };
    query_bluetooth_with_connection(&conn)
}

pub(crate) fn query_bluetooth_with_connection(conn: &Connection) -> BluetoothStatus {
    let Ok(mgr) = Proxy::new(conn, BLUEZ_DEST, "/", OBJ_MANAGER_IFACE) else {
        return BluetoothStatus::default();
    };
    let Ok(objects) = mgr.call::<_, _, ManagedObjects>("GetManagedObjects", &()) else {
        return BluetoothStatus::default();
    };

    let mut status = BluetoothStatus::default();

    for (path, ifaces) in &objects {
        if let Some(adapter) = ifaces.get(ADAPTER_IFACE)
            && status.adapter_path.is_none()
        {
            status.available = true;
            status.adapter_path = Some(path.as_str().to_string());
            status.powered = adapter
                .get("Powered")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);
            status.discovering = adapter
                .get("Discovering")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);
            status.adapter_alias = adapter
                .get("Alias")
                .and_then(|v| String::try_from(v.clone()).ok());
        }

        if let Some(dev) = ifaces.get(DEVICE_IFACE) {
            let paired = dev
                .get("Paired")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);
            let connected = dev
                .get("Connected")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);
            if !paired && !connected {
                continue;
            }
            let address = dev
                .get("Address")
                .and_then(|v| String::try_from(v.clone()).ok())
                .unwrap_or_default();
            let name = dev
                .get("Alias")
                .or_else(|| dev.get("Name"))
                .and_then(|v| String::try_from(v.clone()).ok())
                .unwrap_or_else(|| address.clone());
            let icon = dev
                .get("Icon")
                .and_then(|v| String::try_from(v.clone()).ok());

            status.devices.push(BluetoothDevice {
                object_path: path.as_str().to_string(),
                name,
                address,
                paired,
                connected,
                icon,
            });
        }
    }

    status.devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    status
}

/// Powers the primary BlueZ Bluetooth adapter on or off.
pub fn set_bluetooth_powered(powered: bool) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let current = query_bluetooth_with_connection(&conn);
    let adapter_path = current
        .adapter_path
        .ok_or_else(|| "no Bluetooth adapter detected".to_string())?;
    let adapter = Proxy::new(&conn, BLUEZ_DEST, adapter_path.as_str(), ADAPTER_IFACE)
        .map_err(|e| format!("BlueZ adapter proxy failed: {e}"))?;
    adapter
        .set_property("Powered", Value::Bool(powered))
        .map_err(|e| format!("failed to set Bluetooth Powered={powered}: {e}"))?;
    debug!(adapter_path, powered, "Updated BlueZ adapter power state");
    Ok(())
}

/// Connects or disconnects a paired Bluetooth device by D-Bus object path.
pub fn set_device_connected(object_path: &str, connect: bool) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let device = Proxy::new(&conn, BLUEZ_DEST, object_path, DEVICE_IFACE)
        .map_err(|e| format!("BlueZ device proxy failed: {e}"))?;
    let method = if connect { "Connect" } else { "Disconnect" };
    device
        .call::<_, _, ()>(method, &())
        .map_err(|e| format!("BlueZ {method} failed for {object_path}: {e}"))?;
    debug!(object_path, connect, "Updated BlueZ device connection");
    Ok(())
}

/// Lists ALL BlueZ devices (paired, connected, and newly discovered nearby devices).
pub fn list_all_bluetooth_devices() -> Vec<BluetoothDevice> {
    let Ok(conn) = Connection::system() else {
        return Vec::new();
    };
    let Ok(mgr) = Proxy::new(&conn, BLUEZ_DEST, "/", OBJ_MANAGER_IFACE) else {
        return Vec::new();
    };
    let Ok(objects) = mgr.call::<_, _, ManagedObjects>("GetManagedObjects", &()) else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for (path, ifaces) in &objects {
        let Some(dev) = ifaces.get(DEVICE_IFACE) else {
            continue;
        };
        let paired = dev
            .get("Paired")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);
        let connected = dev
            .get("Connected")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);
        let address = dev
            .get("Address")
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_default();
        let name = dev
            .get("Alias")
            .or_else(|| dev.get("Name"))
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_else(|| address.clone());
        let icon = dev
            .get("Icon")
            .and_then(|v| String::try_from(v.clone()).ok());
        devices.push(BluetoothDevice {
            object_path: path.as_str().to_string(),
            name,
            address,
            paired,
            connected,
            icon,
        });
    }
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| b.paired.cmp(&a.paired))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    devices
}

/// Starts or stops BlueZ nearby device discovery on the primary adapter.
pub fn set_bluetooth_discovery(enable: bool) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let current = query_bluetooth_with_connection(&conn);
    let adapter_path = current
        .adapter_path
        .ok_or_else(|| "no Bluetooth adapter detected".to_string())?;
    let adapter = Proxy::new(&conn, BLUEZ_DEST, adapter_path.as_str(), ADAPTER_IFACE)
        .map_err(|e| format!("BlueZ adapter proxy failed: {e}"))?;
    let method = if enable {
        "StartDiscovery"
    } else {
        "StopDiscovery"
    };
    adapter
        .call::<_, _, ()>(method, &())
        .map_err(|e| format!("BlueZ {method} failed: {e}"))?;
    Ok(())
}

/// Pairs a discovered Bluetooth device via BlueZ D-Bus.
pub fn pair_device(object_path: &str) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let device = Proxy::new(&conn, BLUEZ_DEST, object_path, DEVICE_IFACE)
        .map_err(|e| format!("BlueZ device proxy failed: {e}"))?;
    device
        .call::<_, _, ()>("Pair", &())
        .map_err(|e| format!("BlueZ Pair failed: {e}"))?;
    Ok(())
}

/// Removes a paired Bluetooth device from the primary BlueZ adapter.
pub fn remove_device(object_path: &str) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let current = query_bluetooth_with_connection(&conn);
    let adapter_path = current
        .adapter_path
        .ok_or_else(|| "no Bluetooth adapter detected".to_string())?;
    let adapter = Proxy::new(&conn, BLUEZ_DEST, adapter_path.as_str(), ADAPTER_IFACE)
        .map_err(|e| format!("BlueZ adapter proxy failed: {e}"))?;
    let dev_path = OwnedObjectPath::try_from(object_path.to_string())
        .map_err(|e| format!("invalid device path: {e}"))?;
    adapter
        .call::<_, _, ()>("RemoveDevice", &(dev_path,))
        .map_err(|e| format!("BlueZ RemoveDevice failed: {e}"))?;
    Ok(())
}
