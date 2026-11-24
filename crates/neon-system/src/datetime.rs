//! Date, time, timezone (`org.freedesktop.timedate1` / `timedatectl`), and locale integration.

use std::process::Command;

use serde::{Deserialize, Serialize};
use zbus::blocking::{Connection, Proxy};

const TIMEDATE_DEST: &str = "org.freedesktop.timedate1";
const TIMEDATE_PATH: &str = "/org/freedesktop/timedate1";
const TIMEDATE_IFACE: &str = "org.freedesktop.timedate1";

/// Snapshot of system date, time, NTP, timezone, and locale configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DateTimeLocaleStatus {
    pub timedated_available: bool,
    pub timezone: String,
    pub ntp_enabled: bool,
    pub ntp_synchronized: bool,
    pub local_time_display: String,
    pub system_locale: String,
    pub available_locales: Vec<String>,
}

/// Queries live timezone, NTP status, and active system locale.
pub fn query_datetime_locale() -> DateTimeLocaleStatus {
    let mut status = DateTimeLocaleStatus {
        timezone: "UTC".into(),
        local_time_display: current_time_string(),
        system_locale: std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()),
        available_locales: query_available_locales(),
        ..Default::default()
    };

    if let Ok(conn) = Connection::system()
        && let Ok(td) = Proxy::new(&conn, TIMEDATE_DEST, TIMEDATE_PATH, TIMEDATE_IFACE)
        && let Ok(tz) = td.get_property::<String>("Timezone")
    {
        status.timedated_available = true;
        status.timezone = tz;
        status.ntp_enabled = td.get_property::<bool>("NTP").unwrap_or(false);
        status.ntp_synchronized = td.get_property::<bool>("NTPSynchronized").unwrap_or(false);
    }

    if let Ok(out) = Command::new("localectl").arg("status").output()
        && out.status.success()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(rest) = line.trim().strip_prefix("System Locale:") {
                if let Some(lang) = rest.split_whitespace().find_map(|tok| tok.strip_prefix("LANG=")) {
                    status.system_locale = lang.trim().to_string();
                }
            }
        }
    }

    status
}

/// Toggles automatic network time synchronization via `org.freedesktop.timedate1` (polkit-gated).
pub fn set_ntp_enabled(enabled: bool) -> Result<(), String> {
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let td = Proxy::new(&conn, TIMEDATE_DEST, TIMEDATE_PATH, TIMEDATE_IFACE)
        .map_err(|e| format!("timedate1 proxy failed: {e}"))?;
    td.call::<_, _, ()>("SetNTP", &(enabled, true))
        .map_err(|e| format!("SetNTP failed (may require polkit authorization): {e}"))?;
    Ok(())
}

/// Sets the system timezone via `org.freedesktop.timedate1` (polkit-gated).
pub fn set_timezone(tz: &str) -> Result<(), String> {
    let trimmed = tz.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
    {
        return Err("invalid timezone identifier".into());
    }
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let td = Proxy::new(&conn, TIMEDATE_DEST, TIMEDATE_PATH, TIMEDATE_IFACE)
        .map_err(|e| format!("timedate1 proxy failed: {e}"))?;
    td.call::<_, _, ()>("SetTimezone", &(trimmed, true))
        .map_err(|e| format!("SetTimezone failed (may require polkit authorization): {e}"))?;
    Ok(())
}

/// Lists curated and installed IANA timezones from `timedatectl list-timezones`.
pub fn list_timezones() -> Vec<String> {
    if let Ok(out) = Command::new("timedatectl").arg("list-timezones").output()
        && out.status.success()
    {
        let zones: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !zones.is_empty() {
            return zones;
        }
    }
    vec![
        "UTC".into(),
        "America/New_York".into(),
        "America/Los_Angeles".into(),
        "Europe/London".into(),
        "Europe/Berlin".into(),
        "Asia/Kolkata".into(),
        "Asia/Tokyo".into(),
    ]
}

fn query_available_locales() -> Vec<String> {
    let Ok(out) = Command::new("locale").arg("-a").output() else {
        return vec!["en_US.UTF-8".into()];
    };
    if !out.status.success() {
        return vec!["en_US.UTF-8".into()];
    }
    let mut locales: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| l.to_ascii_lowercase().contains("utf"))
        .map(str::to_string)
        .collect();
    if locales.is_empty() {
        locales.push("en_US.UTF-8".into());
    }
    locales.sort();
    locales.dedup();
    locales
}

fn current_time_string() -> String {
    Command::new("date")
        .arg("+%A, %B %e, %Y · %H:%M")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "System Clock".into())
}
