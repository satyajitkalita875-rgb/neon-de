//! Real-time Wayland clipboard history monitor & store for NEON DE.
//!
//! Uses `zwlr_data_control_manager_v1` (via `wl-paste --watch` + `wl-copy`) exposed by
//! `neon-compositor` so text copied from any Wayland or XWayland application is captured
//! immediately with zero idle CPU usage and zero orphaned helper processes.

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Maximum number of clipboard items kept in history.
pub const MAX_CLIPBOARD_ENTRIES: usize = 50;

/// Maximum UTF-8 byte length stored per clipboard entry (64 KiB).
pub const MAX_ENTRY_BYTES: usize = 64 * 1024;

/// A single captured text entry in the NEON DE clipboard history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub id: u64,
    pub text: String,
    pub preview: String,
    pub line_count: usize,
    pub char_count: usize,
    pub copied_at_unix: u64,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedClipboard {
    next_id: u64,
    entries: Vec<ClipboardEntry>,
}

struct StoreInner {
    next_id: u64,
    entries: Vec<ClipboardEntry>,
    persist_path: Option<PathBuf>,
}

/// Thread-safe, bounded, deduplicating clipboard history store with atomic 0600 persistence.
pub struct ClipboardStore {
    inner: Mutex<StoreInner>,
}

impl ClipboardStore {
    /// Creates a store backed by `${XDG_CACHE_HOME:-$HOME/.cache}/neon-de/clipboard-history.json`.
    pub fn new() -> Self {
        Self::new_with_path(Some(default_clipboard_history_path()))
    }

    /// Creates an ephemeral in-memory store (useful for unit tests and isolated sessions).
    pub fn new_in_memory() -> Self {
        Self::new_with_path(None)
    }

    /// Creates a store persisted to an explicit path (loads existing history if present).
    pub fn new_with_path(persist_path: Option<PathBuf>) -> Self {
        let mut next_id = 1u64;
        let mut entries = Vec::new();
        if let Some(ref p) = persist_path
            && let Ok(raw) = fs::read_to_string(p)
            && let Ok(mut parsed) = serde_json::from_str::<PersistedClipboard>(&raw)
        {
            parsed.entries.retain(|e| !e.text.trim().is_empty());
            parsed.entries.truncate(MAX_CLIPBOARD_ENTRIES);
            let max_seen = parsed.entries.iter().map(|e| e.id).max().unwrap_or(0);
            next_id = parsed.next_id.max(max_seen.saturating_add(1)).max(1);
            entries = parsed.entries;
        }
        Self {
            inner: Mutex::new(StoreInner {
                next_id,
                entries,
                persist_path,
            }),
        }
    }

    /// Inserts or refreshes a clipboard text snippet. Returns `Some(entry)` if history changed.
    pub fn push(&self, text: String) -> Option<ClipboardEntry> {
        self.push_at(text, now_unix_secs())
    }

    /// Inserts or refreshes a clipboard text snippet with an explicit Unix timestamp.
    pub fn push_at(&self, raw_text: String, now_unix: u64) -> Option<ClipboardEntry> {
        if raw_text.trim().is_empty() {
            return None;
        }
        let text = truncate_utf8(&raw_text, MAX_ENTRY_BYTES);
        let mut guard = self.inner.lock().ok()?;

        // If identical text is already at the top of its section and copied within the same second,
        // avoid redundant churn.
        if let Some(pos) = guard.entries.iter().position(|e| e.text == text) {
            let mut existing = guard.entries.remove(pos);
            existing.copied_at_unix = now_unix;
            let insert_idx = if existing.pinned {
                0
            } else {
                guard.entries.iter().take_while(|e| e.pinned).count()
            };
            let entry = existing.clone();
            guard.entries.insert(insert_idx, existing);
            persist_locked(&guard);
            return Some(entry);
        }

        let id = guard.next_id;
        guard.next_id = guard.next_id.saturating_add(1);
        let entry = ClipboardEntry {
            id,
            preview: build_preview(&text),
            line_count: text.lines().count().max(1),
            char_count: text.chars().count(),
            text,
            copied_at_unix: now_unix,
            pinned: false,
        };

        let insert_idx = guard.entries.iter().take_while(|e| e.pinned).count();
        guard.entries.insert(insert_idx, entry.clone());

        // Enforce capacity by evicting the oldest unpinned entry first.
        while guard.entries.len() > MAX_CLIPBOARD_ENTRIES {
            if let Some(unpinned_idx) = guard.entries.iter().rposition(|e| !e.pinned) {
                guard.entries.remove(unpinned_idx);
            } else {
                guard.entries.pop();
            }
        }

        persist_locked(&guard);
        Some(entry)
    }

    /// Returns a cloned snapshot of all entries (pinned first, then newest first).
    pub fn entries(&self) -> Vec<ClipboardEntry> {
        self.inner
            .lock()
            .map(|g| g.entries.clone())
            .unwrap_or_default()
    }

    /// Returns the total number of stored entries.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.entries.len()).unwrap_or(0)
    }

    /// Returns true if no clipboard entries are stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Looks up a single entry by ID.
    pub fn get(&self, id: u64) -> Option<ClipboardEntry> {
        self.inner
            .lock()
            .ok()?
            .entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
    }

    /// Toggles the pinned status of an entry and re-orders cleanly (pinned items at top).
    pub fn toggle_pin(&self, id: u64) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        let Some(pos) = guard.entries.iter().position(|e| e.id == id) else {
            return false;
        };
        let mut item = guard.entries.remove(pos);
        item.pinned = !item.pinned;
        if item.pinned {
            guard.entries.insert(0, item);
        } else {
            // Re-insert among unpinned entries sorted by recency.
            let pinned_count = guard.entries.iter().take_while(|e| e.pinned).count();
            let offset = guard.entries[pinned_count..]
                .iter()
                .position(|e| e.copied_at_unix <= item.copied_at_unix)
                .unwrap_or(guard.entries.len() - pinned_count);
            guard.entries.insert(pinned_count + offset, item);
        }
        persist_locked(&guard);
        true
    }

    /// Removes a single entry by ID.
    pub fn remove(&self, id: u64) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        let before = guard.entries.len();
        guard.entries.retain(|e| e.id != id);
        if guard.entries.len() != before {
            persist_locked(&guard);
            true
        } else {
            false
        }
    }

    /// Clears all unpinned entries (or clears everything if only pinned entries remain).
    pub fn clear_unpinned_or_all(&self) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        if guard.entries.is_empty() {
            return false;
        }
        let has_unpinned = guard.entries.iter().any(|e| !e.pinned);
        if has_unpinned {
            guard.entries.retain(|e| e.pinned);
        } else {
            guard.entries.clear();
        }
        persist_locked(&guard);
        true
    }

    /// Removes all entries including pinned ones.
    pub fn clear_all(&self) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        if guard.entries.is_empty() {
            return false;
        }
        guard.entries.clear();
        persist_locked(&guard);
        true
    }
}

impl Default for ClipboardStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Default persistence location for clipboard history (`0600` private file).
pub fn default_clipboard_history_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("neon-de").join("clipboard-history.json")
}

/// Builds a clean, compact single/two-line preview string suitable for the Dynamic Island.
pub fn build_preview(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(180));
    let mut prev_space = false;
    let mut chars_written = 0usize;
    const MAX_PREVIEW_CHARS: usize = 160;

    for ch in text.trim().chars() {
        if chars_written >= MAX_PREVIEW_CHARS {
            out.push('…');
            break;
        }
        if ch == '\n' || ch == '\r' || ch == '\t' {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                chars_written += 1;
                prev_space = true;
            }
        } else if ch.is_control() {
            continue;
        } else if ch == ' ' {
            if !prev_space {
                out.push(' ');
                chars_written += 1;
                prev_space = true;
            }
        } else {
            out.push(ch);
            chars_written += 1;
            prev_space = false;
        }
    }
    out
}

/// Formats a compact human-readable relative timestamp (`Just now`, `3m ago`, `2h ago`, etc.).
pub fn format_relative_age(copied_at_unix: u64, now_unix: u64) -> String {
    let diff = now_unix.saturating_sub(copied_at_unix);
    if diff < 45 {
        "Just now".into()
    } else if diff < 3600 {
        format!("{}m ago", (diff / 60).max(1))
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 172_800 {
        "Yesterday".into()
    } else {
        format!("{}d ago", diff / 86_400)
    }
}

/// Returns current Unix timestamp in seconds.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Reads the current Wayland UTF-8 clipboard text using `wl-paste`.
pub fn read_wayland_clipboard_text() -> Option<String> {
    read_wl_paste_with_args(&["--no-newline", "--type", "text/plain;charset=utf-8"])
        .or_else(|| read_wl_paste_with_args(&["--no-newline", "--type", "text"]))
}

fn read_wl_paste_with_args(args: &[&str]) -> Option<String> {
    let mut child = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut buf = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout
            .take((MAX_ENTRY_BYTES + 1) as u64)
            .read_to_end(&mut buf);
    }
    let status = child.wait().ok()?;
    if !status.success() || buf.is_empty() {
        return None;
    }
    let text = String::from_utf8(buf).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(truncate_utf8(&text, MAX_ENTRY_BYTES))
    }
}

/// Writes UTF-8 text to the Wayland clipboard via `wl-copy` (non-blocking daemonized offer).
pub fn write_wayland_clipboard_text(text: &str) -> Result<(), String> {
    let mut child = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to launch wl-copy: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("failed to write to wl-copy: {e}"))?;
    }
    let status = child
        .wait()
        .map_err(|e| format!("wl-copy wait failed: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("wl-copy exited with status {status}"))
    }
}

/// Spawns an event-driven background Wayland clipboard watcher (`wl-paste --watch`).
///
/// Sends a unit notification on the returned receiver whenever `store` is updated.
/// Uses `PR_SET_PDEATHSIG(SIGTERM)` so the helper process terminates automatically when
/// the shell exits, leaving zero orphans.
pub fn start_clipboard_monitor(store: Arc<ClipboardStore>) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel::<()>();

    let _ = thread::Builder::new()
        .name("neon-clipboard-watcher".into())
        .spawn(move || {
            // Seed immediately with whatever is already on the clipboard at startup.
            if let Some(initial) = read_wayland_clipboard_text()
                && store.push(initial).is_some()
                && tx.send(()).is_err()
            {
                return;
            }

            loop {
                let mut cmd = Command::new("wl-paste");
                cmd.args(["--type", "text", "--watch", "echo", "CHANGED"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                // Safety: `prctl(PR_SET_PDEATHSIG, SIGTERM)` is async-signal-safe and ensures
                // `wl-paste` terminates automatically if this thread or `neon-shell` exits.
                unsafe {
                    cmd.pre_exec(|| {
                        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                        Ok(())
                    });
                }

                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(err) => {
                        warn!(%err, "wl-paste clipboard watcher unavailable; retrying shortly");
                        thread::sleep(Duration::from_secs(3));
                        continue;
                    }
                };

                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(l) if l.trim() == "CHANGED" => {
                                if let Some(text) = read_wayland_clipboard_text()
                                    && store.push(text).is_some()
                                    && tx.send(()).is_err()
                                {
                                    let _ = child.kill();
                                    let _ = child.wait();
                                    return;
                                }
                            }
                            Ok(_) => {}
                            Err(err) => {
                                debug!(%err, "wl-paste watcher stream ended");
                                break;
                            }
                        }
                    }
                }

                let _ = child.kill();
                let _ = child.wait();
                thread::sleep(Duration::from_millis(1500));
            }
        });

    rx
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn persist_locked(guard: &StoreInner) {
    let Some(ref path) = guard.persist_path else {
        return;
    };
    let payload = PersistedClipboard {
        next_id: guard.next_id,
        entries: guard.entries.clone(),
    };
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        return;
    };
    let _ = write_private_atomic(path, &bytes);
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = File::create(&tmp)?;
    let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_deduplicates_and_respects_pinned_ordering() {
        let store = ClipboardStore::new_in_memory();
        let a = store.push_at("first snippet".into(), 100).unwrap();
        let b = store.push_at("second snippet".into(), 105).unwrap();
        assert_eq!(store.entries().len(), 2);
        assert_eq!(store.entries()[0].id, b.id);

        // Pin `a` -> moves above unpinned items
        assert!(store.toggle_pin(a.id));
        assert_eq!(store.entries()[0].id, a.id);
        assert!(store.entries()[0].pinned);

        // Pushing duplicate of `b` refreshes timestamp without creating a 3rd entry
        store.push_at("second snippet".into(), 120);
        assert_eq!(store.entries().len(), 2);
        assert_eq!(store.entries()[0].id, a.id);
        assert_eq!(store.entries()[1].copied_at_unix, 120);
    }

    #[test]
    fn capacity_evicts_oldest_unpinned_and_preserves_pinned() {
        let store = ClipboardStore::new_in_memory();
        let pinned = store.push_at("keep me pinned".into(), 1).unwrap();
        store.toggle_pin(pinned.id);

        for i in 0..(MAX_CLIPBOARD_ENTRIES + 15) {
            store.push_at(format!("item {i}"), 10 + i as u64);
        }

        let entries = store.entries();
        assert_eq!(entries.len(), MAX_CLIPBOARD_ENTRIES);
        assert_eq!(entries[0].id, pinned.id);
        assert!(entries[0].pinned);
    }

    #[test]
    fn clear_unpinned_preserves_pinned_then_clears_all() {
        let store = ClipboardStore::new_in_memory();
        let p = store.push_at("pinned item".into(), 10).unwrap();
        store.push_at("normal item".into(), 20);
        store.toggle_pin(p.id);

        assert!(store.clear_unpinned_or_all());
        assert_eq!(store.len(), 1);
        assert_eq!(store.entries()[0].id, p.id);

        // Second clear when only pinned remain clears everything
        assert!(store.clear_unpinned_or_all());
        assert!(store.is_empty());
    }

    #[test]
    fn persistence_roundtrip_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.json");
        {
            let store = ClipboardStore::new_with_path(Some(path.clone()));
            let e = store.push_at("hello neon\nworld".into(), 500).unwrap();
            assert_eq!(e.line_count, 2);
            assert_eq!(e.preview, "hello neon world");
        }
        let reloaded = ClipboardStore::new_with_path(Some(path));
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.entries()[0].text, "hello neon\nworld");
    }

    #[test]
    fn relative_age_formatting_covers_ranges() {
        assert_eq!(format_relative_age(1000, 1020), "Just now");
        assert_eq!(format_relative_age(1000, 1180), "3m ago");
        assert_eq!(format_relative_age(1000, 8200), "2h ago");
        assert_eq!(format_relative_age(1000, 100_000), "Yesterday");
        assert_eq!(format_relative_age(1000, 300_000), "3d ago");
    }
}
