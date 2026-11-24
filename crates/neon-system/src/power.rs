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
const LOGIN1_SESSION_AUTO: &str = "/org/freedesktop/login1/session/auto";
const LOGIN1_SESSION_IFACE: &str = "org.freedesktop.login1.Session";

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
    pub battery_health_percent: Option<f64>,
    pub battery_state: BatteryState,
    pub time_to_empty_secs: i64,
    pub time_to_full_secs: i64,
    pub active_profile: Option<String>,
    pub available_profiles: Vec<String>,
    pub can_suspend: bool,
    pub can_hibernate: bool,
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
            battery_health_percent: None,
            battery_state: BatteryState::Unknown,
            time_to_empty_secs: 0,
            time_to_full_secs: 0,
            active_profile: None,
            available_profiles: Vec::new(),
            can_suspend: false,
            can_hibernate: false,
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
            let cap = dev.get_property::<f64>("Capacity").unwrap_or(0.0);
            if cap > 1.0 && cap <= 120.0 {
                status.battery_health_percent = Some(cap);
            }
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
        status.can_hibernate = login1
            .call::<_, _, String>("CanHibernate", &())
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

/// Invokes a safe `org.freedesktop.login1.Manager` power action (`Suspend`, `Hibernate`, `Reboot`, `PowerOff`).
pub fn invoke_logind_action(method: &str) -> Result<(), String> {
    if !matches!(method, "Suspend" | "Hibernate" | "Reboot" | "PowerOff") {
        return Err(format!("unsupported logind action '{method}'"));
    }
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let login1 = Proxy::new(&conn, LOGIN1_DEST, LOGIN1_PATH, LOGIN1_IFACE)
        .map_err(|e| format!("logind proxy failed: {e}"))?;
    login1
        .call::<_, _, ()>(method, &(true,))
        .map_err(|e| format!("logind {method} failed: {e}"))?;
    Ok(())
}

/// Terminates the active user desktop session cleanly via `systemd-logind` (Log Out).
pub fn terminate_current_session() -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    if let Ok(sess) = Proxy::new(&conn, LOGIN1_DEST, LOGIN1_SESSION_AUTO, LOGIN1_SESSION_IFACE)
        && sess.call::<_, _, ()>("Terminate", &()).is_ok()
    {
        return Ok(());
    }
    if let Ok(session_id) = std::env::var("XDG_SESSION_ID")
        && !session_id.trim().is_empty()
    {
        let mgr = Proxy::new(&conn, LOGIN1_DEST, LOGIN1_PATH, LOGIN1_IFACE)
            .map_err(|e| format!("logind manager proxy failed: {e}"))?;
        mgr.call::<_, _, ()>("TerminateSession", &(session_id.trim(),))
            .map_err(|e| format!("TerminateSession failed: {e}"))?;
        return Ok(());
    }
    Err("could not determine active systemd-logind session to terminate".into())
}

/// Discovers the primary Linux backlight device `(device_name, current_raw, max_raw)` under `/sys/class/backlight`.
fn discover_backlight_device() -> Option<(String, u32, u32)> {
    let entries = std::fs::read_dir("/sys/class/backlight").ok()?;
    let mut best: Option<(u8, String, u32, u32)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let cur: u32 = std::fs::read_to_string(path.join("brightness"))
            .ok()
            .and_then(|s| s.trim().parse().ok())?;
        let max: u32 = std::fs::read_to_string(path.join("max_brightness"))
            .ok()
            .and_then(|s| s.trim().parse().ok())?;
        if max == 0 {
            continue;
        }
        // Prefer raw (e.g. intel_backlight / amdgpu_bl) > platform > firmware.
        let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        let rank = match kind.trim() {
            "raw" => 0,
            "platform" => 1,
            _ => 2,
        };
        if best.as_ref().is_none_or(|b| rank < b.0) {
            best = Some((rank, name, cur, max));
        }
    }
    best.map(|(_, name, cur, max)| (name, cur, max))
}

/// Converts a display brightness percentage (`5..=100`) to a clamped hardware raw register value.
pub fn percent_to_backlight_raw(pct: u32, max_brightness: u32) -> u32 {
    if max_brightness == 0 {
        return 0;
    }
    let clamped = pct.clamp(5, 100);
    let min_safe = (max_brightness / 100).max(1);
    let raw = ((f64::from(clamped) / 100.0) * f64::from(max_brightness)).round() as u32;
    raw.clamp(min_safe, max_brightness)
}

/// Reads the current display backlight percentage (`1..=100`) from `/sys/class/backlight`.
pub fn read_backlight_percent() -> Option<u32> {
    let (_, cur, max) = discover_backlight_device()?;
    if max == 0 {
        return None;
    }
    let pct = ((f64::from(cur) / f64::from(max)) * 100.0)
        .round()
        .clamp(1.0, 100.0) as u32;
    Some(pct)
}

/// Sets display backlight brightness (`5..=100%`) without requiring root privileges via
/// `org.freedesktop.login1.Session.SetBrightness`, falling back to sysfs or `brightnessctl`.
pub fn set_backlight_percent(pct: u32) -> Result<(), String> {
    let (dev_name, _, max_brightness) =
        discover_backlight_device().ok_or_else(|| "no backlight device found".to_string())?;
    let raw = percent_to_backlight_raw(pct, max_brightness);

    // 1. Primary: systemd-logind Session.SetBrightness (works cleanly without root/video group!)
    if let Ok(conn) = Connection::system() {
        if let Ok(sess) =
            Proxy::new(&conn, LOGIN1_DEST, LOGIN1_SESSION_AUTO, LOGIN1_SESSION_IFACE)
            && sess
                .call::<_, _, ()>("SetBrightness", &("backlight", dev_name.as_str(), raw))
                .is_ok()
        {
            debug!(device = %dev_name, pct, raw, "Updated display brightness via logind");
            return Ok(());
        }
        if let Ok(mgr) = Proxy::new(&conn, LOGIN1_DEST, LOGIN1_PATH, LOGIN1_IFACE)
            && let Ok(sess_path) =
                mgr.call::<_, _, OwnedObjectPath>("GetSessionByPID", &(std::process::id(),))
            && let Ok(sess) =
                Proxy::new(&conn, LOGIN1_DEST, sess_path.as_str(), LOGIN1_SESSION_IFACE)
            && sess
                .call::<_, _, ()>("SetBrightness", &("backlight", dev_name.as_str(), raw))
                .is_ok()
        {
            debug!(device = %dev_name, pct, raw, "Updated display brightness via logind session");
            return Ok(());
        }
    }

    // 2. Fallback: direct sysfs write if user has video group permissions.
    let sysfs_path = format!("/sys/class/backlight/{dev_name}/brightness");
    if std::fs::write(&sysfs_path, raw.to_string()).is_ok() {
        return Ok(());
    }

    // 3. Fallback: brightnessctl CLI.
    let clamped = pct.clamp(5, 100);
    let out = std::process::Command::new("brightnessctl")
        .args(["set", &format!("{clamped}%")])
        .output()
        .map_err(|e| format!("failed to set brightness: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
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

    #[test]
    fn backlight_percent_conversion_clamps_safely() {
        assert_eq!(percent_to_backlight_raw(100, 24000), 24000);
        assert_eq!(percent_to_backlight_raw(50, 24000), 12000);
        assert_eq!(percent_to_backlight_raw(0, 24000), 1200);
        assert_eq!(percent_to_backlight_raw(150, 24000), 24000);
        assert_eq!(percent_to_backlight_raw(50, 0), 0);
    }
}
