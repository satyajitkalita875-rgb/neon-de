//! Real Linux MPRIS2 media playback inspection & control (`org.mpris.MediaPlayer2.*` via D-Bus).
//!
//! Automatically discovers active media players (browsers, Spotify, VLC, MPV, Rhythmbox, etc.)
//! on the user's session D-Bus, preferring whichever player is actively playing, and dispatches
//! `PlayPause`, `Next`, and `Previous` commands directly over D-Bus without external CLI tools.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::OwnedValue,
};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const MPRIS_PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

/// Live playback state of the active MPRIS2 media player.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaStatus {
    /// Whether at least one MPRIS2 media player is currently registered on session D-Bus.
    pub available: bool,
    /// Active player D-Bus name (e.g. `org.mpris.MediaPlayer2.chromium.instance123`).
    pub bus_name: Option<String>,
    /// Friendly application name (e.g. `Chromium`, `Spotify`, `VLC`).
    pub player_name: Option<String>,
    /// Track or video title (`xesam:title`).
    pub title: Option<String>,
    /// Primary artist or channel (`xesam:artist`).
    pub artist: Option<String>,
    /// Optional local image path decoded from `mpris:artUrl` (`file://...`).
    pub art_path: Option<String>,
    /// `true` when `PlaybackStatus == "Playing"`.
    pub playing: bool,
}

/// Queries the active MPRIS2 media player on the session D-Bus.
pub fn query_media_status() -> MediaStatus {
    let Ok(conn) = Connection::session() else {
        return MediaStatus::default();
    };
    query_media_with_connection(&conn)
}

pub fn query_media_with_connection(conn: &Connection) -> MediaStatus {
    let players = list_mpris_players(conn);
    if players.is_empty() {
        return MediaStatus::default();
    }

    let mut best: Option<MediaStatus> = None;
    for bus_name in players {
        let Some(status) = inspect_player(conn, &bus_name) else {
            continue;
        };
        if status.playing {
            return status;
        }
        if best.is_none() || (best.as_ref().is_some_and(|b| b.title.is_none()) && status.title.is_some()) {
            best = Some(status);
        }
    }

    best.unwrap_or_default()
}

/// Dispatches an MPRIS2 transport command (`PlayPause`, `Next`, `Previous`) to the active player.
pub fn invoke_media_command(preferred_bus: Option<&str>, method: &str) -> Result<(), String> {
    match method {
        "PlayPause" | "Next" | "Previous" => {}
        other => return Err(format!("unsupported MPRIS command: {other}")),
    }

    let conn = Connection::session().map_err(|e| format!("session bus unavailable: {e}"))?;
    let target = match preferred_bus {
        Some(name) if name.starts_with(MPRIS_PREFIX) => name.to_string(),
        _ => query_media_with_connection(&conn)
            .bus_name
            .ok_or_else(|| "no active MPRIS media player found".to_string())?,
    };

    let proxy = Proxy::new(&conn, target.as_str(), MPRIS_PATH, MPRIS_PLAYER_IFACE)
        .map_err(|e| format!("failed to connect to MPRIS player: {e}"))?;
    proxy
        .call_method(method, &())
        .map(|_| ())
        .map_err(|e| format!("MPRIS {method} failed: {e}"))
}

fn list_mpris_players(conn: &Connection) -> Vec<String> {
    let Ok(proxy) = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) else {
        return Vec::new();
    };
    let Ok(names): Result<Vec<String>, _> = proxy.call("ListNames", &()) else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter(|n| n.starts_with(MPRIS_PREFIX))
        .collect()
}

fn inspect_player(conn: &Connection, bus_name: &str) -> Option<MediaStatus> {
    let player_proxy = Proxy::new(conn, bus_name, MPRIS_PATH, MPRIS_PLAYER_IFACE).ok()?;
    let playback: String = player_proxy
        .get_property("PlaybackStatus")
        .unwrap_or_else(|_| "Stopped".to_string());
    let playing = playback.eq_ignore_ascii_case("Playing");

    let metadata: HashMap<String, OwnedValue> = player_proxy
        .get_property("Metadata")
        .unwrap_or_default();

    let title = extract_string(&metadata, "xesam:title");
    let artist = extract_artist(&metadata);
    let art_path = extract_string(&metadata, "mpris:artUrl")
        .and_then(|u| decode_local_art_url(&u));

    let player_name = Proxy::new(conn, bus_name, MPRIS_PATH, MPRIS_ROOT_IFACE)
        .ok()
        .and_then(|root| root.get_property::<String>("Identity").ok())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            bus_name
                .strip_prefix(MPRIS_PREFIX)
                .map(|s| s.split('.').next().unwrap_or(s).to_string())
        });

    Some(MediaStatus {
        available: true,
        bus_name: Some(bus_name.to_string()),
        player_name,
        title,
        artist,
        art_path,
        playing,
    })
}

fn extract_string(map: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let val = map.get(key)?;
    let s: String = val.try_clone().ok()?.try_into().ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn extract_artist(map: &HashMap<String, OwnedValue>) -> Option<String> {
    let val = map.get("xesam:artist")?;
    if let Ok(list) = Vec::<String>::try_from(val.try_clone().ok()?) {
        let joined = list
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        if !joined.is_empty() {
            return Some(joined);
        }
    }
    extract_string(map, "xesam:artist")
}

/// Decodes a `file://` MPRIS album-art URI into a verified local filesystem path.
pub fn decode_local_art_url(url: &str) -> Option<String> {
    let stripped = url.strip_prefix("file://")?;
    let mut decoded = String::with_capacity(stripped.len());
    let bytes = stripped.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            decoded.push(byte as char);
            i += 3;
            continue;
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    if std::path::Path::new(&decoded).is_file() {
        Some(decoded)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_local_art_url_rejects_remote_or_missing_files() {
        assert_eq!(decode_local_art_url("https://example.com/cover.png"), None);
        assert_eq!(
            decode_local_art_url("file:///nonexistent/neon-de-test-art.png"),
            None
        );
    }

    #[test]
    fn invoke_media_command_rejects_unknown_method() {
        assert!(invoke_media_command(None, "Explode").is_err());
    }
}
