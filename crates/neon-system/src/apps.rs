//! XDG Desktop Entry, XDG MIME default applications, and XDG Autostart management.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

/// Installed application entry discovered from standard XDG `.desktop` directories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledApp {
    pub desktop_id: String,
    pub name: String,
    pub comment: Option<String>,
    pub exec: String,
    pub icon: Option<String>,
    pub categories: Vec<String>,
    pub mime_types: Vec<String>,
}

/// An XDG autostart `.desktop` entry under `~/.config/autostart` or `/etc/xdg/autostart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartEntry {
    pub file_name: String,
    pub name: String,
    pub exec: String,
    pub enabled: bool,
    pub user_owned: bool,
}

/// Discovers installed XDG applications across user and system application directories.
pub fn discover_installed_apps() -> Vec<InstalledApp> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for part in std::env::split_paths(&data_dirs) {
            dirs.push(part.join("applications"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/share/applications"));
        dirs.push(PathBuf::from("/usr/share/applications"));
    }

    let mut seen: HashMap<String, InstalledApp> = HashMap::new();
    let mut hidden: HashSet<String> = HashSet::new();

    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let id = file_name.to_string();
            if seen.contains_key(&id) || hidden.contains(&id) {
                continue;
            }
            match parse_desktop_entry(&path, &id) {
                Some(app) => {
                    seen.insert(id, app);
                }
                None => {
                    hidden.insert(id);
                }
            }
        }
    }

    let mut apps: Vec<InstalledApp> = seen.into_values().collect();
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

fn parse_desktop_entry(path: &Path, desktop_id: &str) -> Option<InstalledApp> {
    let raw = fs::read_to_string(path).ok()?;
    let mut in_main = false;
    let mut entry_type = None;
    let mut name = None;
    let mut comment = None;
    let mut exec = None;
    let mut icon = None;
    let mut categories = Vec::new();
    let mut mime_types = Vec::new();
    let mut no_display = false;
    let mut hidden = false;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "Type" => entry_type = Some(v.trim().to_string()),
            "Name" => name = Some(v.trim().to_string()),
            "Comment" => comment = Some(v.trim().to_string()),
            "Exec" => exec = Some(v.trim().to_string()),
            "Icon" => icon = Some(v.trim().to_string()),
            "NoDisplay" => no_display = v.trim().eq_ignore_ascii_case("true"),
            "Hidden" => hidden = v.trim().eq_ignore_ascii_case("true"),
            "Categories" => {
                categories = v
                    .split(';')
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string())
                    .collect();
            }
            "MimeType" => {
                mime_types = v
                    .split(';')
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string())
                    .collect();
            }
            _ => {}
        }
    }

    if no_display || hidden || entry_type.as_deref() != Some("Application") {
        return None;
    }
    Some(InstalledApp {
        desktop_id: desktop_id.to_string(),
        name: name?,
        comment,
        exec: exec?,
        icon,
        categories,
        mime_types,
    })
}

/// Queries the default handler `.desktop` ID for a MIME type via `xdg-mime`.
pub fn query_default_mime_app(mime_type: &str) -> Option<String> {
    let out = Command::new("xdg-mime")
        .args(["query", "default", mime_type])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if val.is_empty() { None } else { Some(val) }
}

/// Updates the default handler `.desktop` ID for one or more MIME types via `xdg-mime`.
pub fn set_default_mime_app(desktop_id: &str, mime_types: &[&str]) -> Result<(), String> {
    if !desktop_id.ends_with(".desktop") || desktop_id.contains('/') || desktop_id.contains("..") {
        return Err("invalid desktop entry identifier".into());
    }
    let mut cmd = Command::new("xdg-mime");
    cmd.arg("default").arg(desktop_id);
    for m in mime_types {
        cmd.arg(m);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run xdg-mime: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("xdg-mime default failed: {}", stderr.trim()));
    }
    Ok(())
}

/// Lists autostart entries from `~/.config/autostart` and `/etc/xdg/autostart`.
pub fn list_autostart_entries() -> Vec<AutostartEntry> {
    let user_dir = user_autostart_dir();
    let mut by_name: HashMap<String, AutostartEntry> = HashMap::new();

    for (dir, is_user) in [
        (PathBuf::from("/etc/xdg/autostart"), false),
        (user_dir, true),
    ] {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Some(parsed) = parse_autostart_file(&path, file_name, is_user) {
                by_name.insert(file_name.to_string(), parsed);
            }
        }
    }

    let mut list: Vec<AutostartEntry> = by_name.into_values().collect();
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    list
}

/// Enables or disables an XDG autostart entry cleanly in `~/.config/autostart`.
pub fn set_autostart_enabled(file_name: &str, enabled: bool) -> Result<(), String> {
    if !file_name.ends_with(".desktop") || file_name.contains('/') || file_name.contains("..") {
        return Err("invalid autostart entry name".into());
    }
    let user_dir = user_autostart_dir();
    fs::create_dir_all(&user_dir).map_err(|e| format!("failed to create autostart dir: {e}"))?;
    let user_path = user_dir.join(file_name);

    let source_text = if user_path.exists() {
        fs::read_to_string(&user_path).unwrap_or_default()
    } else {
        let sys_path = PathBuf::from("/etc/xdg/autostart").join(file_name);
        fs::read_to_string(&sys_path)
            .unwrap_or_else(|_| format!("[Desktop Entry]\nType=Application\nName={file_name}\n"))
    };

    let mut lines: Vec<String> = source_text
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.starts_with("Hidden=") && !t.starts_with("X-GNOME-Autostart-enabled=")
        })
        .map(str::to_string)
        .collect();
    lines.push(format!("Hidden={}", if enabled { "false" } else { "true" }));
    lines.push(format!(
        "X-GNOME-Autostart-enabled={}",
        if enabled { "true" } else { "false" }
    ));

    let content = format!("{}\n", lines.join("\n"));
    fs::write(&user_path, content).map_err(|e| format!("failed to write autostart entry: {e}"))
}

/// Adds an installed application to `~/.config/autostart`.
pub fn add_user_autostart(app: &InstalledApp) -> Result<(), String> {
    let user_dir = user_autostart_dir();
    fs::create_dir_all(&user_dir).map_err(|e| format!("failed to create autostart dir: {e}"))?;
    let dest = user_dir.join(&app.desktop_id);
    let content = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={}\nIcon={}\nHidden=false\nX-GNOME-Autostart-enabled=true\n",
        app.name,
        app.exec,
        app.icon.as_deref().unwrap_or("application-x-executable")
    );
    fs::write(&dest, content).map_err(|e| format!("failed to create autostart file: {e}"))
}

fn user_autostart_dir() -> PathBuf {
    if let Some(cfg) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(cfg).join("autostart")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config/autostart")
    } else {
        PathBuf::from("/tmp/neon-autostart")
    }
}

fn parse_autostart_file(path: &Path, file_name: &str, user_owned: bool) -> Option<AutostartEntry> {
    let raw = fs::read_to_string(path).ok()?;
    let mut name = None;
    let mut exec = String::new();
    let mut hidden = false;
    let mut gnome_enabled = true;

    for line in raw.lines() {
        let t = line.trim();
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        match k.trim() {
            "Name" => name = Some(v.trim().to_string()),
            "Exec" => exec = v.trim().to_string(),
            "Hidden" => hidden = v.trim().eq_ignore_ascii_case("true"),
            "X-GNOME-Autostart-enabled" => gnome_enabled = !v.trim().eq_ignore_ascii_case("false"),
            _ => {}
        }
    }

    Some(AutostartEntry {
        file_name: file_name.to_string(),
        name: name.unwrap_or_else(|| file_name.trim_end_matches(".desktop").to_string()),
        exec,
        enabled: !hidden && gnome_enabled,
        user_owned,
    })
}
