//! UPower battery, PowerProfilesDaemon, and systemd-logind D-Bus integration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::debug;
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const UPOWER_DEST: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const UPOWER_IFACE: &str = "org.freedesktop.UPower";
const UPOWER_DEVICE_IFACE: &str = "org.freedesktop.UPower.Device";

const PROFILES_DEST: &str = "org.freedesktop.UPower.PowerProfiles";
const PROFILES_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
const PROFILES_IFACE: &str = "org.freedesktop.UPower.PowerProfiles";

const LOGIN1_DEST: &str = "org.freedesktop.login1";
const LOGIN1_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_IFACE: &str = "org.freedesktop.login1.Manager";

/// Battery charging state reported by UPower (`org.freedesktop.UPower.Device.State`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryState {
    Unknown,
    Charging,
    Discharging,
    Empty,
    FullyCharged,
    PendingCharge,
    PendingDischarge,
}

impl BatteryState {
    pub fn from_upower(code: u32) -> Self {
        match code {
            1 => Self::Charging,
            2 => Self::Discharging,
            3 => Self::Empty,
            4 => Self::FullyCharged,
            5 => Self::PendingCharge,
            6 => Self::PendingDischarge,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Charging => "Charging",
            Self::Discharging => "Discharging",
            Self::Empty => "Empty",
            Self::FullyCharged => "Full",
            Self::PendingCharge => "Plugged In",
            Self::PendingDischarge => "Battery",
            Self::Unknown => "Unknown",
        }
    }
}

/// Snapshot of system battery, power profile, and session power capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerStatus {
    pub available: bool,
    pub on_battery: bool,
    pub has_battery: bool,
    pub battery_percentage: f64,
    pub battery_state: BatteryState,
    pub time_to_empty_secs: i64,
    pub time_to_full_secs: i64,
    pub active_profile: Option<String>,
    pub available_profiles: Vec<String>,
    pub can_suspend: bool,
    pub can_reboot: bool,
    pub can_power_off: bool,
}

impl Default for PowerStatus {
    fn default() -> Self {
        Self {
            available: false,
            on_battery: false,
            has_battery: false,
            battery_percentage: 0.0,
            battery_state: BatteryState::Unknown,
            time_to_empty_secs: 0,
            time_to_full_secs: 0,
            active_profile: None,
            available_profiles: Vec::new(),
            can_suspend: false,
            can_reboot: false,
            can_power_off: false,
        }
    }
}

impl PowerStatus {
    /// Compact Top Panel battery/power indicator label.
    pub fn panel_label(&self) -> String {
        if !self.available {
            return "Pwr: --".to_string();
        }
        if !self.has_battery {
            return "AC".to_string();
        }
        let pct = self.battery_percentage.round() as u32;
        match self.battery_state {
            BatteryState::Charging => format!("Bat {pct}%+"),
            BatteryState::FullyCharged => format!("Bat {pct}%"),
            _ => format!("Bat {pct}%"),
        }
    }
}

/// Queries live UPower battery state, PowerProfiles, and logind capabilities.
pub fn query_power_status() -> PowerStatus {
    let Ok(conn) = Connection::system() else {
        return PowerStatus::default();
    };
    query_power_with_connection(&conn)
}

pub(crate) fn query_power_with_connection(conn: &Connection) -> PowerStatus {
    let mut status = PowerStatus::default();

    if let Ok(upower) = Proxy::new(conn, UPOWER_DEST, UPOWER_PATH, UPOWER_IFACE) {
        if let Ok(on_battery) = upower.get_property::<bool>("OnBattery") {
            status.available = true;
            status.on_battery = on_battery;
        }
        if let Ok(display_dev) = upower.call::<_, _, OwnedObjectPath>("GetDisplayDevice", &())
            && let Ok(dev) =
                Proxy::new(conn, UPOWER_DEST, display_dev.as_str(), UPOWER_DEVICE_IFACE)
        {
            status.has_battery = dev.get_property::<bool>("IsPresent").unwrap_or(false);
            status.battery_percentage = dev.get_property::<f64>("Percentage").unwrap_or(0.0);
            let state_code = dev.get_property::<u32>("State").unwrap_or(0);
            status.battery_state = BatteryState::from_upower(state_code);
            status.time_to_empty_secs = dev.get_property::<i64>("TimeToEmpty").unwrap_or(0);
            status.time_to_full_secs = dev.get_property::<i64>("TimeToFull").unwrap_or(0);
        }
    }

    if let Ok(pp) = Proxy::new(conn, PROFILES_DEST, PROFILES_PATH, PROFILES_IFACE) {
        if let Ok(active) = pp.get_property::<String>("ActiveProfile") {
            status.active_profile = Some(active);
        }
        if let Ok(profiles) = pp.get_property::<Vec<HashMap<String, OwnedValue>>>("Profiles") {
            for entry in profiles {
                if let Some(val) = entry.get("Profile")
                    && let Ok(name) = String::try_from(val.clone())
                {
                    status.available_profiles.push(name);
                }
            }
        }
    }

    if let Ok(login1) = Proxy::new(conn, LOGIN1_DEST, LOGIN1_PATH, LOGIN1_IFACE) {
        status.can_suspend = login1
            .call::<_, _, String>("CanSuspend", &())
            .is_ok_and(|s| s == "yes");
        status.can_reboot = login1
            .call::<_, _, String>("CanReboot", &())
            .is_ok_and(|s| s == "yes");
        status.can_power_off = login1
            .call::<_, _, String>("CanPowerOff", &())
            .is_ok_and(|s| s == "yes");
    }

    status
}

/// Switches the active system power profile (`power-saver`, `balanced`, `performance`).
pub fn set_power_profile(profile: &str) -> Result<(), String> {
    if !matches!(profile, "power-saver" | "balanced" | "performance") {
        return Err(format!("unsupported power profile '{profile}'"));
    }
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let pp = Proxy::new(&conn, PROFILES_DEST, PROFILES_PATH, PROFILES_IFACE)
        .map_err(|e| format!("PowerProfiles proxy failed: {e}"))?;
    pp.set_property("ActiveProfile", Value::from(profile))
        .map_err(|e| format!("failed to set ActiveProfile={profile}: {e}"))?;
    debug!(profile, "Updated UPower PowerProfile");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_battery_and_ac_panel_labels() {
        let charging = PowerStatus {
            available: true,
            has_battery: true,
            battery_percentage: 86.4,
            battery_state: BatteryState::Charging,
            ..Default::default()
        };
        assert_eq!(charging.panel_label(), "Bat 86%+");

        let desktop = PowerStatus {
            available: true,
            has_battery: false,
            ..Default::default()
        };
        assert_eq!(desktop.panel_label(), "AC");
    }
}
