//! Local persistent stores, classifiers, and helpers for NEON DE's Little Notch modules.
//!
//! 100% local, privacy-first storage under XDG config/data directories:
//! - Notch appearance (`Black` / `BlackGlass` / `LiquidGlass`) & 5 pinned dock modules
//! - Quick Notes (persistent Markdown-friendly scratchpad)
//! - Calendar quick events & natural-time input parser
//! - File Shelf (drag-and-drop staging + recent Downloads/Desktop + real Zip/Unzip)
//! - Clipboard item classifier (`Color` swatch, `Link`, `Code`, `Text`)
//! - Quick Tools live math & percentage evaluator (`12 * 4.5 + 8%`)

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::island_visuals::NotchAppearance;

/// All real interactive modules available inside NEON DE's Little Notch Dynamic Island.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotchModule {
    Home,
    Media,
    Timer,
    Clipboard,
    Files,
    Notes,
    Calendar,
    Tools,
    Stats,
    Battery,
    Awake,
    Hud,
    Store,
}

impl NotchModule {
    /// All 12 user-selectable modules (excluding the `Store` drawer itself).
    pub const ALL_MODULES: &'static [NotchModule] = &[
        Self::Home,
        Self::Media,
        Self::Timer,
        Self::Clipboard,
        Self::Files,
        Self::Notes,
        Self::Calendar,
        Self::Tools,
        Self::Stats,
        Self::Battery,
        Self::Awake,
        Self::Hud,
    ];

    pub const fn stack_name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Media => "media",
            Self::Timer => "timer",
            Self::Clipboard => "clipboard",
            Self::Files => "files",
            Self::Notes => "notes",
            Self::Calendar => "calendar",
            Self::Tools => "tools",
            Self::Stats => "stats",
            Self::Battery => "battery",
            Self::Awake => "awake",
            Self::Hud => "hud",
            Self::Store => "store",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Media => "Now Playing",
            Self::Timer => "Timer",
            Self::Clipboard => "Clipboard",
            Self::Files => "File Shelf",
            Self::Notes => "Quick Notes",
            Self::Calendar => "Calendar",
            Self::Tools => "Quick Tools",
            Self::Stats => "System Stats",
            Self::Battery => "Battery",
            Self::Awake => "Keep Awake",
            Self::Hud => "Audio & Display",
            Self::Store => "All Modules",
        }
    }

    pub const fn subtitle(self) -> &'static str {
        match self {
            Self::Home => "Today & live status",
            Self::Media => "Album art & neon wave",
            Self::Timer => "Scrubbable focus ruler",
            Self::Clipboard => "Search, colors & pin",
            Self::Files => "Drop, zip & drag out",
            Self::Notes => "Markdown scratchpad",
            Self::Calendar => "7-day agenda & events",
            Self::Tools => "Calc, capture & colors",
            Self::Stats => "CPU, RAM, Net & Disk",
            Self::Battery => "Power & Bluetooth",
            Self::Awake => "Prevent system sleep",
            Self::Hud => "Volume & brightness",
            Self::Store => "Pin modules & style",
        }
    }
}

/// Persistent user preferences for the Little Notch Dynamic Island.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotchPrefs {
    pub appearance: NotchAppearance,
    pub pinned: Vec<NotchModule>,
}

impl Default for NotchPrefs {
    fn default() -> Self {
        Self {
            appearance: NotchAppearance::Black,
            pinned: vec![
                NotchModule::Home,
                NotchModule::Media,
                NotchModule::Timer,
                NotchModule::Clipboard,
                NotchModule::Files,
            ],
        }
    }
}

impl NotchPrefs {
    pub fn load() -> Self {
        let path = notch_prefs_path();
        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(mut parsed) = serde_json::from_str::<Self>(&raw)
        {
            parsed.sanitize();
            return parsed;
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = notch_prefs_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, bytes).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
        }
    }

    pub fn toggle_pin(&mut self, module: NotchModule) {
        if module == NotchModule::Store {
            return;
        }
        if let Some(pos) = self.pinned.iter().position(|&m| m == module) {
            if self.pinned.len() > 1 {
                self.pinned.remove(pos);
            }
        } else {
            if self.pinned.len() >= 5 {
                self.pinned.pop();
            }
            self.pinned.push(module);
        }
        self.save();
    }

    fn sanitize(&mut self) {
        self.pinned.retain(|&m| m != NotchModule::Store);
        let mut dedup = Vec::new();
        for m in self.pinned.drain(..) {
            if !dedup.contains(&m) && dedup.len() < 5 {
                dedup.push(m);
            }
        }
        if dedup.is_empty() {
            dedup = Self::default().pinned;
        }
        self.pinned = dedup;
    }
}

fn notch_prefs_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("neon-de").join("notch-prefs.json")
}

fn notch_data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("neon-de")
}

/// Semantic category for Little Notch's Clipboard History card strip & filter tabs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardItemKind {
    Color(String),
    Link,
    Code,
    Text,
}

impl ClipboardItemKind {
    pub const fn badge(&self) -> &'static str {
        match self {
            Self::Color(_) => "Color",
            Self::Link => "Link",
            Self::Code => "Code",
            Self::Text => "Text",
        }
    }

    pub fn matches_filter(&self, filter: &str) -> bool {
        match filter {
            "all" => true,
            "text" => matches!(self, Self::Text),
            "links" => matches!(self, Self::Link),
            "colors" => matches!(self, Self::Color(_)),
            "code" => matches!(self, Self::Code),
            _ => true,
        }
    }
}

/// Classifies a clipboard snippet for rich card rendering (hex color swatch, link, code, text).
pub fn classify_clipboard_text(text: &str) -> ClipboardItemKind {
    let trimmed = text.trim();
    if let Some(hex) = normalize_hex_color(trimmed) {
        return ClipboardItemKind::Color(hex);
    }
    if (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        && !trimmed.contains(char::is_whitespace)
    {
        return ClipboardItemKind::Link;
    }
    let looks_like_code = trimmed.contains('\n')
        && (trimmed.contains('{')
            || trimmed.contains("fn ")
            || trimmed.contains("def ")
            || trimmed.contains("const ")
            || trimmed.contains("let ")
            || trimmed.contains("=>")
            || trimmed.contains("::"))
        || trimmed.starts_with("cargo ")
        || trimmed.starts_with("git ")
        || trimmed.starts_with("sudo ")
        || trimmed.starts_with("docker ")
        || trimmed.starts_with("npm ")
        || trimmed.contains("();");
    if looks_like_code {
        return ClipboardItemKind::Code;
    }
    ClipboardItemKind::Text
}

/// Recognizes `#RGB` or `#RRGGBB` hex colors and returns normalized uppercase `#RRGGBB`.
pub fn normalize_hex_color(s: &str) -> Option<String> {
    let raw = s.trim().strip_prefix('#')?;
    if !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match raw.len() {
        3 => {
            let b = raw.as_bytes();
            let expanded = format!(
                "#{}{}{}{}{}{}",
                b[0] as char,
                b[0] as char,
                b[1] as char,
                b[1] as char,
                b[2] as char,
                b[2] as char
            );
            Some(expanded.to_ascii_uppercase())
        }
        6 => Some(format!("#{}", raw.to_ascii_uppercase())),
        _ => None,
    }
}

/// A single local Quick Note stored inside the Dynamic Island.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickNote {
    pub id: u64,
    pub body: String,
    pub updated_unix: u64,
}

impl QuickNote {
    pub fn title_line(&self) -> String {
        self.body
            .lines()
            .map(|l| l.trim().trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .unwrap_or("Untitled Note")
            .to_string()
    }

    pub fn preview_line(&self) -> String {
        let mut non_empty = self
            .body
            .lines()
            .map(|l| l.trim().trim_start_matches('#').trim())
            .filter(|l| !l.is_empty());
        let _title = non_empty.next();
        non_empty.next().unwrap_or("Empty note").to_string()
    }
}

/// Persistent local store for Little Notch Quick Notes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickNotesStore {
    next_id: u64,
    pub notes: Vec<QuickNote>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl QuickNotesStore {
    pub fn load() -> Self {
        let path = notch_data_dir().join("notch-notes.json");
        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(mut parsed) = serde_json::from_str::<Self>(&raw)
        {
            parsed.path = Some(path);
            if parsed.notes.is_empty() {
                parsed.ensure_initial_note();
            }
            return parsed;
        }
        let mut store = Self {
            next_id: 1,
            notes: Vec::new(),
            path: Some(path),
        };
        store.ensure_initial_note();
        store
    }

    fn ensure_initial_note(&mut self) {
        if self.notes.is_empty() {
            self.notes.push(QuickNote {
                id: self.next_id,
                body: "# Quick Scratchpad\n- [ ] Jot ideas without leaving your app\n- [ ] Press Super+N anytime".into(),
                updated_unix: now_unix(),
            });
            self.next_id += 1;
        }
    }

    pub fn create(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.notes.insert(
            0,
            QuickNote {
                id,
                body: "# New Note\n".into(),
                updated_unix: now_unix(),
            },
        );
        self.save();
        id
    }

    pub fn update_body(&mut self, id: u64, body: String) {
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
            if n.body == body {
                return;
            }
            n.body = body;
            n.updated_unix = now_unix();
            self.save();
        }
    }

    pub fn delete(&mut self, id: u64) -> Option<u64> {
        self.notes.retain(|n| n.id != id);
        if self.notes.is_empty() {
            self.ensure_initial_note();
        }
        self.save();
        self.notes.first().map(|n| n.id)
    }

    pub fn save(&self) {
        let Some(ref path) = self.path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, bytes).is_ok() {
                let _ = fs::rename(&tmp, path);
            }
        }
    }
}

/// A single local Calendar reminder/event shown in Little Notch's Calendar module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: u64,
    pub date_iso: String,
    pub time_label: String,
    pub title: String,
}

/// Persistent local store for Little Notch Calendar events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEventStore {
    next_id: u64,
    pub events: Vec<CalendarEvent>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl CalendarEventStore {
    pub fn load() -> Self {
        let path = notch_data_dir().join("notch-events.json");
        if let Ok(raw) = fs::read_to_string(&path)
            && let Ok(mut parsed) = serde_json::from_str::<Self>(&raw)
        {
            parsed.path = Some(path);
            return parsed;
        }
        Self {
            next_id: 1,
            events: Vec::new(),
            path: Some(path),
        }
    }

    pub fn add_quick(&mut self, date_iso: &str, input: &str) -> Option<CalendarEvent> {
        let (title, time_label) = parse_quick_event_input(input)?;
        let ev = CalendarEvent {
            id: self.next_id,
            date_iso: date_iso.to_string(),
            time_label,
            title,
        };
        self.next_id += 1;
        self.events.push(ev.clone());
        self.save();
        Some(ev)
    }

    pub fn remove(&mut self, id: u64) {
        self.events.retain(|e| e.id != id);
        self.save();
    }

    pub fn for_date(&self, date_iso: &str) -> Vec<CalendarEvent> {
        self.events
            .iter()
            .filter(|e| e.date_iso == date_iso)
            .cloned()
            .collect()
    }

    pub fn has_events_on(&self, date_iso: &str) -> bool {
        self.events.iter().any(|e| e.date_iso == date_iso)
    }

    pub fn save(&self) {
        let Some(ref path) = self.path else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, bytes).is_ok() {
                let _ = fs::rename(&tmp, path);
            }
        }
    }
}

/// Parses quick-add calendar text such as `"Design review 3:30pm"` or `"Lunch with Sam 1pm"`.
pub fn parse_quick_event_input(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let mut time_idx: Option<usize> = None;
    let mut parsed_time = String::new();

    for (i, tok) in tokens.iter().enumerate() {
        let lower = tok.to_ascii_lowercase();
        let is_ampm = (lower.ends_with("am") || lower.ends_with("pm"))
            && lower.len() >= 3
            && lower.as_bytes()[0].is_ascii_digit();
        let is_24h = lower.contains(':')
            && lower.len() >= 4
            && lower.len() <= 5
            && lower.bytes().all(|b| b.is_ascii_digit() || b == b':');
        if is_ampm || is_24h {
            time_idx = Some(i);
            parsed_time = tok.to_ascii_uppercase();
            break;
        }
    }

    if let Some(idx) = time_idx {
        let title = tokens
            .iter()
            .enumerate()
            .filter(|&(i, tok)| {
                i != idx && !tok.eq_ignore_ascii_case("at")
            })
            .map(|(_, tok)| *tok)
            .collect::<Vec<_>>()
            .join(" ");
        let final_title = if title.is_empty() {
            "Event".to_string()
        } else {
            title
        };
        Some((final_title, parsed_time))
    } else {
        Some((trimmed.to_string(), "All day".to_string()))
    }
}

/// A staged or recent file displayed on Little Notch's File Shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileShelfItem {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
    pub is_dir: bool,
    pub is_image: bool,
    pub is_archive: bool,
}

impl FileShelfItem {
    pub fn from_path(path: PathBuf) -> Option<Self> {
        let meta = fs::metadata(&path).ok()?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())?;
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let is_image = matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg"
        );
        let is_archive = matches!(ext.as_str(), "zip" | "tar" | "gz" | "xz" | "zst" | "7z");
        Some(Self {
            path,
            name,
            size_bytes: if meta.is_dir() { 0 } else { meta.len() },
            is_dir: meta.is_dir(),
            is_image,
            is_archive,
        })
    }
}

/// Persistent OnePlus-style File Dock store (`~/.config/neon-de/file-shelf.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SavedShelfList {
    paths: Vec<String>,
}

fn shelf_store_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("neon-de").join("file-shelf.json"))
}

fn shelf_drop_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    Some(base.join("neon-de").join("file-dock"))
}

/// Loads staged files from `~/.config/neon-de/file-shelf.json`, keeping only files that still exist.
pub fn load_persisted_shelf_items() -> Vec<FileShelfItem> {
    let Some(path) = shelf_store_path() else {
        return Vec::new();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(saved) = serde_json::from_str::<SavedShelfList>(&raw) else {
        return Vec::new();
    };
    saved
        .paths
        .into_iter()
        .filter_map(|s| FileShelfItem::from_path(PathBuf::from(s)))
        .take(24)
        .collect()
}

/// Saves the staged File Dock items to `~/.config/neon-de/file-shelf.json`.
pub fn save_persisted_shelf_items(items: &[FileShelfItem]) {
    let Some(path) = shelf_store_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = SavedShelfList {
        paths: items
            .iter()
            .take(24)
            .map(|i| i.path.to_string_lossy().into_owned())
            .collect(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&payload) {
        let _ = fs::write(path, json);
    }
}

/// Allocates a destination `.png` path inside `~/.local/share/neon-de/file-dock/` for raw images
/// dragged directly from web browsers or image viewers onto the Notch.
pub fn next_dropped_image_path() -> Option<PathBuf> {
    let dir = shelf_drop_cache_dir()?;
    let _ = fs::create_dir_all(&dir);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Some(dir.join(format!("docked-{ms}.png")))
}

/// Parses `text/uri-list` or plain `file://` strings into local filesystem paths.
pub fn parse_uri_list_paths(raw: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("file://") {
            let path_part = rest.strip_prefix("localhost").unwrap_or(rest);
            let decoded = percent_decode_uri_path(path_part);
            let pb = PathBuf::from(decoded);
            if pb.exists() {
                out.push(pb);
            }
        } else {
            let pb = PathBuf::from(trimmed);
            if pb.is_absolute() && pb.exists() {
                out.push(pb);
            }
        }
    }
    out
}

fn percent_decode_uri_path(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex_s) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(v) = u8::from_str_radix(hex_s, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Scans recent non-hidden files from `~/Downloads` and `~/Desktop` (newest first, up to `limit`).
pub fn list_recent_user_files(limit: usize) -> Vec<FileShelfItem> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut found: Vec<(FileShelfItem, SystemTime)> = Vec::new();
    for dir in [home.join("Downloads"), home.join("Desktop")] {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let fname = entry.file_name();
            if fname.to_string_lossy().starts_with('.') {
                continue;
            }
            let p = entry.path();
            if let Ok(meta) = entry.metadata()
                && let Ok(mt) = meta.modified()
                && let Some(item) = FileShelfItem::from_path(p)
            {
                found.push((item, mt));
            }
        }
    }
    found.sort_by(|a, b| b.1.cmp(&a.1));
    found.into_iter().take(limit).map(|(it, _)| it).collect()
}

/// Compresses a file or folder into a sibling `.zip` (or `.tar.gz` fallback) archive.
pub fn compress_shelf_item(path: &Path) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("no parent directory")?;
    let name = path
        .file_name()
        .ok_or("invalid file name")?
        .to_string_lossy()
        .into_owned();
    let zip_dest = parent.join(format!("{name}.zip"));

    if let Ok(st) = Command::new("zip")
        .current_dir(parent)
        .args(["-rq"])
        .arg(&zip_dest)
        .arg(&name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        && st.success()
        && zip_dest.is_file()
    {
        return Ok(zip_dest);
    }

    let tar_dest = parent.join(format!("{name}.tar.gz"));
    let st = Command::new("tar")
        .current_dir(parent)
        .args(["-czf"])
        .arg(&tar_dest)
        .arg(&name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("tar failed: {e}"))?;
    if st.success() && tar_dest.is_file() {
        Ok(tar_dest)
    } else {
        Err("Archive creation failed".into())
    }
}

/// Extracts a `.zip` or `.tar.*` archive into a sibling directory.
pub fn extract_shelf_archive(path: &Path) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or("no parent directory")?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "extracted".into());
    let dest = parent.join(stem);
    let _ = fs::create_dir_all(&dest);

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let ok = if ext == "zip" {
        Command::new("unzip")
            .args(["-oq"])
            .arg(path)
            .arg("-d")
            .arg(&dest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        Command::new("tar")
            .args(["-xf"])
            .arg(path)
            .arg("-C")
            .arg(&dest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    if ok {
        Ok(dest)
    } else {
        Err("Extraction failed".into())
    }
}

/// Formats a byte size concisely (`888 KB`, `25.5 MB`).
pub fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Evaluates Quick Tools inline arithmetic expressions (`12 * 4.5 + 8%`, `(100 - 15) / 2`, `2^10`).
pub fn eval_quick_math(input: &str) -> Option<f64> {
    let cleaned = input
        .replace('×', "*")
        .replace('÷', "/")
        .replace('−', "-")
        .replace(',', "");
    let tokens = tokenize_math(&cleaned)?;
    if tokens.is_empty() {
        return None;
    }
    let mut pos = 0usize;
    let val = parse_add_sub(&tokens, &mut pos)?;
    if pos == tokens.len() && val.is_finite() {
        Some(val)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
enum MathTok {
    Num(f64),
    Plus,
    Minus,
    Mul,
    Div,
    Pow,
    Percent,
    LParen,
    RParen,
}

fn tokenize_math(s: &str) -> Option<Vec<MathTok>> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b.is_ascii_digit() || b == b'.' {
            let start = i;
            let mut dot = b == b'.';
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || (!dot && bytes[i] == b'.')) {
                if bytes[i] == b'.' {
                    dot = true;
                }
                i += 1;
            }
            let n: f64 = s[start..i].parse().ok()?;
            out.push(MathTok::Num(n));
            continue;
        }
        let tok = match b {
            b'+' => MathTok::Plus,
            b'-' => MathTok::Minus,
            b'*' => MathTok::Mul,
            b'/' => MathTok::Div,
            b'^' => MathTok::Pow,
            b'%' => MathTok::Percent,
            b'(' => MathTok::LParen,
            b')' => MathTok::RParen,
            _ => return None,
        };
        out.push(tok);
        i += 1;
    }
    Some(out)
}

fn parse_add_sub(toks: &[MathTok], pos: &mut usize) -> Option<f64> {
    let mut left = parse_mul_div(toks, pos)?;
    while *pos < toks.len() {
        match toks[*pos] {
            MathTok::Plus => {
                *pos += 1;
                let (rhs, is_pct) = parse_mul_div_pct(toks, pos)?;
                left = if is_pct {
                    left + left * (rhs / 100.0)
                } else {
                    left + rhs
                };
            }
            MathTok::Minus => {
                *pos += 1;
                let (rhs, is_pct) = parse_mul_div_pct(toks, pos)?;
                left = if is_pct {
                    left - left * (rhs / 100.0)
                } else {
                    left - rhs
                };
            }
            _ => break,
        }
    }
    Some(left)
}

fn parse_mul_div_pct(toks: &[MathTok], pos: &mut usize) -> Option<(f64, bool)> {
    let start_pos = *pos;
    let val = parse_pow(toks, pos)?;
    if *pos < toks.len() && toks[*pos] == MathTok::Percent {
        *pos += 1;
        return Some((val, true));
    }
    // Re-parse full mul/div term if not a simple trailing `%`
    *pos = start_pos;
    Some((parse_mul_div(toks, pos)?, false))
}

fn parse_mul_div(toks: &[MathTok], pos: &mut usize) -> Option<f64> {
    let mut left = parse_pow(toks, pos)?;
    if *pos < toks.len() && toks[*pos] == MathTok::Percent {
        *pos += 1;
        left /= 100.0;
    }
    while *pos < toks.len() {
        match toks[*pos] {
            MathTok::Mul => {
                *pos += 1;
                let mut rhs = parse_pow(toks, pos)?;
                if *pos < toks.len() && toks[*pos] == MathTok::Percent {
                    *pos += 1;
                    rhs /= 100.0;
                }
                left *= rhs;
            }
            MathTok::Div => {
                *pos += 1;
                let mut rhs = parse_pow(toks, pos)?;
                if *pos < toks.len() && toks[*pos] == MathTok::Percent {
                    *pos += 1;
                    rhs /= 100.0;
                }
                if rhs.abs() < 1e-12 {
                    return None;
                }
                left /= rhs;
            }
            _ => break,
        }
    }
    Some(left)
}

fn parse_pow(toks: &[MathTok], pos: &mut usize) -> Option<f64> {
    let base = parse_unary(toks, pos)?;
    if *pos < toks.len() && toks[*pos] == MathTok::Pow {
        *pos += 1;
        let exp = parse_pow(toks, pos)?;
        Some(base.powf(exp))
    } else {
        Some(base)
    }
}

fn parse_unary(toks: &[MathTok], pos: &mut usize) -> Option<f64> {
    if *pos >= toks.len() {
        return None;
    }
    match toks[*pos] {
        MathTok::Minus => {
            *pos += 1;
            Some(-parse_unary(toks, pos)?)
        }
        MathTok::Plus => {
            *pos += 1;
            parse_unary(toks, pos)
        }
        MathTok::LParen => {
            *pos += 1;
            let v = parse_add_sub(toks, pos)?;
            if *pos < toks.len() && toks[*pos] == MathTok::RParen {
                *pos += 1;
                Some(v)
            } else {
                None
            }
        }
        MathTok::Num(n) => {
            *pos += 1;
            Some(n)
        }
        _ => None,
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_math_evaluates_arithmetic_and_percentages() {
        assert!((eval_quick_math("12 × 4.5 + 8%").unwrap() - 58.32).abs() < 1e-6);
        assert!((eval_quick_math("200 - 15%").unwrap() - 170.0).abs() < 1e-6);
        assert!((eval_quick_math("(10 + 5) * 2^3").unwrap() - 120.0).abs() < 1e-6);
        assert!(eval_quick_math("10 / 0").is_none());
        assert!(eval_quick_math("not math").is_none());
    }

    #[test]
    fn clipboard_classifier_detects_colors_links_code_and_text() {
        assert_eq!(
            classify_clipboard_text("#ff6b3d"),
            ClipboardItemKind::Color("#FF6B3D".into())
        );
        assert_eq!(
            classify_clipboard_text("#0af"),
            ClipboardItemKind::Color("#00AAFF".into())
        );
        assert_eq!(
            classify_clipboard_text("https://littlenotch.app/modules"),
            ClipboardItemKind::Link
        );
        assert_eq!(
            classify_clipboard_text("cargo build --release"),
            ClipboardItemKind::Code
        );
        assert_eq!(
            classify_clipboard_text("Remember to call Sam"),
            ClipboardItemKind::Text
        );
    }

    #[test]
    fn calendar_quick_event_parser_extracts_time_and_title() {
        assert_eq!(
            parse_quick_event_input("Design review 3:30pm"),
            Some(("Design review".into(), "3:30PM".into()))
        );
        assert_eq!(
            parse_quick_event_input("Lunch with Sam at 1pm"),
            Some(("Lunch with Sam".into(), "1PM".into()))
        );
        assert_eq!(
            parse_quick_event_input("Product offsite"),
            Some(("Product offsite".into(), "All day".into()))
        );
    }

    #[test]
    fn uri_list_percent_decoder_handles_spaces_and_comments() {
        assert_eq!(
            percent_decode_uri_path("/home/user/My%20File%20%281%29.png"),
            "/home/user/My File (1).png"
        );
    }
}
