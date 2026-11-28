//! Real Linux MPRIS2 media playback inspection & control (`org.mpris.MediaPlayer2.*` via D-Bus).
//!
//! Automatically discovers active media players (browsers, Spotify, VLC, MPV, Rhythmbox, etc.)
//! on the user's session D-Bus, preferring whichever player is actively playing, exposes full
//! track metadata + live position/duration + cached album art (`file://`, `data:image/...`, and
//! async `https://` artwork), and dispatches `PlayPause`, `Next`, `Previous`, `SetPosition`, and
//! `Seek` commands directly over D-Bus.

use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{ObjectPath, OwnedValue},
};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const MPRIS_PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const MAX_CACHED_ART_FILES: usize = 32;

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
    /// Optional album name (`xesam:album`).
    #[serde(default)]
    pub album: Option<String>,
    /// Raw `mpris:artUrl` string exposed by the player.
    #[serde(default)]
    pub art_url: Option<String>,
    /// Optional verified local image path decoded/cached from `mpris:artUrl`.
    pub art_path: Option<String>,
    /// Optional MPRIS D-Bus track object path (`mpris:trackid`) used by `SetPosition`.
    #[serde(default)]
    pub track_id: Option<String>,
    /// Current playback position in microseconds (`Position` property).
    #[serde(default)]
    pub position_us: Option<i64>,
    /// Total track duration in microseconds (`mpris:length`).
    #[serde(default)]
    pub length_us: Option<i64>,
    /// Whether the active player supports seeking (`CanSeek`).
    #[serde(default)]
    pub can_seek: bool,
    /// Epoch milliseconds when `position_us` was sampled (0 when paused/unavailable so idle
    /// snapshots remain stably equal).
    #[serde(default)]
    pub observed_unix_ms: u64,
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
        if best.is_none()
            || (best.as_ref().is_some_and(|b| b.title.is_none()) && status.title.is_some())
        {
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
    let target = resolve_target_bus(&conn, preferred_bus)?;

    let proxy = Proxy::new(&conn, target.as_str(), MPRIS_PATH, MPRIS_PLAYER_IFACE)
        .map_err(|e| format!("failed to connect to MPRIS player: {e}"))?;
    proxy
        .call_method(method, &())
        .map(|_| ())
        .map_err(|e| format!("MPRIS {method} failed: {e}"))
}

/// Seeks the active MPRIS2 player to `position_us` via `SetPosition` (with `Seek` delta fallback).
pub fn invoke_media_seek(
    preferred_bus: Option<&str>,
    track_id: Option<&str>,
    position_us: i64,
    current_position_us: Option<i64>,
) -> Result<(), String> {
    let pos = position_us.max(0);
    let conn = Connection::session().map_err(|e| format!("session bus unavailable: {e}"))?;
    let target = resolve_target_bus(&conn, preferred_bus)?;
    let proxy = Proxy::new(&conn, target.as_str(), MPRIS_PATH, MPRIS_PLAYER_IFACE)
        .map_err(|e| format!("failed to connect to MPRIS player: {e}"))?;

    if let Some(tid) = track_id
        && let Ok(obj_path) = ObjectPath::try_from(tid)
        && proxy.call_method("SetPosition", &(obj_path, pos)).is_ok()
    {
        return Ok(());
    }

    let cur = current_position_us
        .or_else(|| proxy.get_property::<i64>("Position").ok())
        .unwrap_or(0);
    let delta = pos.saturating_sub(cur);
    proxy
        .call_method("Seek", &(delta,))
        .map(|_| ())
        .map_err(|e| format!("MPRIS Seek failed: {e}"))
}

fn resolve_target_bus(conn: &Connection, preferred_bus: Option<&str>) -> Result<String, String> {
    match preferred_bus {
        Some(name) if name.starts_with(MPRIS_PREFIX) => Ok(name.to_string()),
        _ => query_media_with_connection(conn)
            .bus_name
            .ok_or_else(|| "no active MPRIS media player found".to_string()),
    }
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
    let album = extract_string(&metadata, "xesam:album");
    let art_url = extract_string(&metadata, "mpris:artUrl");
    let art_path = art_url.as_deref().and_then(resolve_art_url);
    let track_id = extract_track_id(&metadata);
    let length_us = extract_i64_like(&metadata, "mpris:length").filter(|&v| v > 0);
    let position_us = player_proxy
        .get_property::<i64>("Position")
        .ok()
        .map(|v| v.max(0));
    let can_seek = player_proxy.get_property::<bool>("CanSeek").unwrap_or(false);
    let observed_unix_ms = if playing && position_us.is_some() {
        now_unix_ms()
    } else {
        0
    };

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
        album,
        art_url,
        art_path,
        track_id,
        position_us,
        length_us,
        can_seek,
        observed_unix_ms,
        playing,
    })
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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

fn extract_track_id(map: &HashMap<String, OwnedValue>) -> Option<String> {
    let val = map.get("mpris:trackid")?;
    if let Ok(path) = zbus::zvariant::OwnedObjectPath::try_from(val.try_clone().ok()?) {
        let s = path.as_str().trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    extract_string(map, "mpris:trackid")
}

fn extract_i64_like(map: &HashMap<String, OwnedValue>, key: &str) -> Option<i64> {
    let val = map.get(key)?;
    let cloned = val.try_clone().ok()?;
    if let Ok(v) = i64::try_from(cloned) {
        return Some(v);
    }
    let cloned = val.try_clone().ok()?;
    if let Ok(v) = u64::try_from(cloned) {
        return i64::try_from(v).ok();
    }
    let cloned = val.try_clone().ok()?;
    if let Ok(v) = i32::try_from(cloned) {
        return Some(i64::from(v));
    }
    None
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

/// Resolves an MPRIS `mpris:artUrl` (`file://`, `data:image/...;base64,...`, or `https://`)
/// to a verified local image file path without blocking D-Bus polling on network I/O.
pub fn resolve_art_url(url: &str) -> Option<String> {
    if url.starts_with("file://") {
        return decode_local_art_url(url);
    }
    if url.starts_with("data:image/") {
        return decode_data_uri_art(url);
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return resolve_remote_art_cached(url);
    }
    None
}

/// Decodes a `file://` MPRIS album-art URI into a verified local filesystem path.
pub fn decode_local_art_url(url: &str) -> Option<String> {
    let stripped = url.strip_prefix("file://")?;
    let mut raw_bytes = Vec::with_capacity(stripped.len());
    let bytes = stripped.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            raw_bytes.push(byte);
            i += 3;
            continue;
        }
        raw_bytes.push(bytes[i]);
        i += 1;
    }
    let decoded = String::from_utf8_lossy(&raw_bytes).into_owned();
    if Path::new(&decoded).is_file() {
        Some(decoded)
    } else {
        None
    }
}

fn art_cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("neon-de").join("art-cache")
}

fn fnv1a_64(input: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in input.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn has_valid_image_magic(bytes: &[u8]) -> bool {
    if bytes.len() < 12 {
        return false;
    }
    // PNG, JPEG, GIF, WebP, BMP, AVIF/HEIF
    bytes.starts_with(&[0x89, b'P', b'N', b'G'])
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF8")
        || (&bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        || bytes.starts_with(b"BM")
}

fn decode_data_uri_art(url: &str) -> Option<String> {
    let (_, payload) = url.split_once(";base64,")?;
    let cache_dir = art_cache_dir();
    let target = cache_dir.join(format!("data-{:016x}.img", fnv1a_64(url)));
    if target.is_file() {
        return Some(target.to_string_lossy().into_owned());
    }
    let decoded = decode_base64(payload)?;
    if !has_valid_image_magic(&decoded) {
        return None;
    }
    let _ = fs::create_dir_all(&cache_dir);
    let _ = fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o700));
    let tmp = target.with_extension("tmp");
    if fs::write(&tmp, &decoded).is_ok() && fs::rename(&tmp, &target).is_ok() {
        prune_art_cache(&cache_dir);
        Some(target.to_string_lossy().into_owned())
    } else {
        let _ = fs::remove_file(&tmp);
        None
    }
}

fn resolve_remote_art_cached(url: &str) -> Option<String> {
    let cache_dir = art_cache_dir();
    let hash = fnv1a_64(url);
    let target = cache_dir.join(format!("net-{hash:016x}.img"));
    if target.is_file()
        && fs::metadata(&target)
            .map(|m| m.len() >= 12)
            .unwrap_or(false)
    {
        return Some(target.to_string_lossy().into_owned());
    }

    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let lock = IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()));
    let should_spawn = lock
        .lock()
        .map(|mut set| set.insert(url.to_string()))
        .unwrap_or(false);

    if should_spawn {
        let url_owned = url.to_string();
        let target_owned = target.clone();
        let cache_owned = cache_dir;
        let _ = thread::Builder::new()
            .name("neon-art-fetch".into())
            .spawn(move || {
                let _ = fs::create_dir_all(&cache_owned);
                let _ = fs::set_permissions(&cache_owned, fs::Permissions::from_mode(0o700));
                let tmp = target_owned.with_extension("tmp");
                let ok = Command::new("curl")
                    .args([
                        "-fsSL",
                        "--max-time",
                        "4",
                        "--max-filesize",
                        "5242880",
                        "-o",
                    ])
                    .arg(&tmp)
                    .arg(&url_owned)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if ok
                    && let Ok(bytes) = fs::read(&tmp)
                    && has_valid_image_magic(&bytes)
                {
                    let _ = fs::rename(&tmp, &target_owned);
                    prune_art_cache(&cache_owned);
                } else {
                    let _ = fs::remove_file(&tmp);
                }
                if let Ok(mut set) = lock.lock() {
                    set.remove(&url_owned);
                }
            });
    }

    None
}

fn prune_art_cache(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, SystemTime)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("img") {
                return None;
            }
            let mt = e.metadata().ok()?.modified().ok()?;
            Some((p, mt))
        })
        .collect();
    if files.len() <= MAX_CACHED_ART_FILES {
        return;
    }
    files.sort_by_key(|(_, mt)| *mt);
    let to_remove = files.len().saturating_sub(MAX_CACHED_ART_FILES);
    for (p, _) in files.into_iter().take(to_remove) {
        let _ = fs::remove_file(p);
    }
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity((input.len() * 3) / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for b in input.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
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
    fn decode_data_uri_art_decodes_valid_png() {
        // 1x1 transparent PNG
        let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        let uri = format!("data:image/png;base64,{png_b64}");
        let path = resolve_art_url(&uri).expect("should decode data URI PNG");
        assert!(Path::new(&path).is_file());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invoke_media_command_rejects_unknown_method() {
        assert!(invoke_media_command(None, "Explode").is_err());
    }
}
