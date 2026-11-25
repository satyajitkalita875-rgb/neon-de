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
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub desktop_path: Option<PathBuf>,
}

/// Packaging origin detected for an installed desktop application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppInstallSource {
    FlatpakUser(String),
    FlatpakSystem(String),
    Snap(String),
    UserDesktop(PathBuf),
    SystemPackage(PathBuf),
    Unknown,
}

impl AppInstallSource {
    /// Short human-readable label describing how this app is installed.
    pub fn label(&self) -> &'static str {
        match self {
            Self::FlatpakUser(_) => "Flatpak (User)",
            Self::FlatpakSystem(_) => "Flatpak",
            Self::Snap(_) => "Snap",
            Self::UserDesktop(_) => "User App",
            Self::SystemPackage(_) => "System Package",
            Self::Unknown => "Application",
        }
    }
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

/// Discovers installed XDG applications across user, Flatpak, Snap, and system directories.
pub fn discover_installed_apps() -> Vec<InstalledApp> {
    let mut dirs = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    };
    if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        push_unique(h.join(".local/share/applications"));
        push_unique(h.join(".local/share/flatpak/exports/share/applications"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for part in std::env::split_paths(&data_dirs) {
            push_unique(part.join("applications"));
        }
    }
    push_unique(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    push_unique(PathBuf::from("/var/lib/snapd/desktop/applications"));
    push_unique(PathBuf::from("/usr/local/share/applications"));
    push_unique(PathBuf::from("/usr/share/applications"));

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
    let mut terminal = false;
    let mut working_dir = None;

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
            "Path" => working_dir = Some(v.trim().to_string()),
            "Icon" => icon = Some(v.trim().to_string()),
            "NoDisplay" => no_display = v.trim().eq_ignore_ascii_case("true"),
            "Hidden" => hidden = v.trim().eq_ignore_ascii_case("true"),
            "Terminal" => terminal = v.trim().eq_ignore_ascii_case("true"),
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
        terminal,
        working_dir,
        desktop_path: Some(path.to_path_buf()),
    })
}

/// Finds an executable binary on `$PATH` (or accepts an executable absolute path).
pub fn find_executable_in_path(bin: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let candidate = Path::new(bin);
    if candidate.is_absolute() {
        return candidate
            .metadata()
            .ok()
            .filter(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
            .map(|_| candidate.to_path_buf());
    }
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let full = dir.join(bin);
        if let Ok(meta) = full.metadata()
            && meta.is_file()
            && (meta.permissions().mode() & 0o111 != 0)
        {
            return Some(full);
        }
    }
    None
}

/// Wraps a `Terminal=true` command `(program, args)` in an available terminal emulator.
pub fn wrap_terminal_command(program: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    const CANDIDATES: &[&str] = &[
        "kitty",
        "alacritty",
        "ghostty",
        "foot",
        "wezterm",
        "gnome-terminal",
        "kgx",
        "konsole",
        "xfce4-terminal",
        "xterm",
        "x-terminal-emulator",
    ];

    let mut chosen: Option<String> = None;
    if let Ok(env_term) = std::env::var("TERMINAL") {
        let trimmed = env_term.trim();
        if !trimmed.is_empty() && find_executable_in_path(trimmed).is_some() {
            chosen = Some(trimmed.to_string());
        }
    }
    if chosen.is_none() {
        for cand in CANDIDATES {
            if find_executable_in_path(cand).is_some() {
                chosen = Some((*cand).to_string());
                break;
            }
        }
    }
    let term = chosen?;
    Some(build_terminal_argv(&term, program, args))
}

/// Formats `(terminal_program, argv)` for the chosen terminal emulator binary name.
pub fn build_terminal_argv(
    terminal: &str,
    program: &str,
    args: &[String],
) -> (String, Vec<String>) {
    let base = Path::new(terminal)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(terminal);
    let mut out = Vec::with_capacity(args.len() + 3);
    match base {
        "kitty" | "foot" | "gnome-terminal" | "kgx" => {
            out.push("--".to_string());
        }
        "wezterm" => {
            out.push("start".to_string());
            out.push("--".to_string());
        }
        "xfce4-terminal" => {
            out.push("-x".to_string());
        }
        _ => {
            // alacritty, ghostty, konsole, xterm, x-terminal-emulator, etc.
            out.push("-e".to_string());
        }
    }
    out.push(program.to_string());
    out.extend(args.iter().cloned());
    (terminal.to_string(), out)
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

/// Detects the package source of an installed application without spawning subprocesses.
pub fn detect_install_source(app: &InstalledApp) -> AppInstallSource {
    let path_str = app
        .desktop_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let app_stem = app
        .desktop_id
        .strip_suffix(".desktop")
        .unwrap_or(&app.desktop_id);

    if path_str.contains("/.local/share/flatpak/") {
        return AppInstallSource::FlatpakUser(app_stem.to_string());
    }
    if path_str.contains("/var/lib/flatpak/")
        || app.exec.starts_with("flatpak run ")
        || app.exec.starts_with("/usr/bin/flatpak run ")
    {
        return AppInstallSource::FlatpakSystem(app_stem.to_string());
    }
    if path_str.starts_with("/var/lib/snapd/desktop/")
        || app.exec.contains("/snap/bin/")
        || app.exec.contains("/var/lib/snapd/")
    {
        let snap_name = app_stem
            .split('_')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(app_stem)
            .to_string();
        return AppInstallSource::Snap(snap_name);
    }
    if let Some(ref p) = app.desktop_path {
        if let Some(home) = std::env::var_os("HOME") {
            let user_apps = PathBuf::from(home).join(".local/share/applications");
            if p.starts_with(&user_apps) {
                return AppInstallSource::UserDesktop(p.clone());
            }
        }
        if path_str.contains("/.local/share/applications/") {
            return AppInstallSource::UserDesktop(p.clone());
        }
        if p.starts_with("/usr/share/applications") || p.starts_with("/usr/local/share/applications")
        {
            return AppInstallSource::SystemPackage(p.clone());
        }
    }
    AppInstallSource::Unknown
}

/// Returns `true` if `pkg` is a core OS or NEON DE package that must never be uninstalled from Settings.
pub fn is_protected_system_package(pkg: &str) -> bool {
    let p = pkg.trim().to_ascii_lowercase();
    if p.starts_with("neon-") {
        return true;
    }
    matches!(
        p.as_str(),
        "systemd"
            | "systemd-sysv"
            | "sudo"
            | "apt"
            | "dpkg"
            | "bash"
            | "libc6"
            | "coreutils"
            | "polkitd"
            | "dbus"
            | "dbus-daemon"
            | "dbus-user-session"
            | "login"
            | "init"
            | "base-files"
    )
}

fn is_safe_pkg_token(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

fn resolve_dpkg_owner(path: &Path) -> Option<String> {
    let mut candidates = vec![path.to_path_buf()];
    if let Ok(canon) = fs::canonicalize(path)
        && canon != path
    {
        candidates.push(canon);
    }
    for cand in candidates {
        let Ok(out) = Command::new("dpkg").arg("-S").arg(&cand).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines() {
            if let Some((pkg_raw, _)) = line.split_once(':') {
                // Take the first package (stripping any `:arch` or comma-separated multi-owner)
                let pkg = pkg_raw
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .trim();
                if is_safe_pkg_token(pkg) {
                    return Some(pkg.to_string());
                }
            }
        }
    }
    None
}

/// Uninstalls an application cleanly using its native package manager (Flatpak, Snap, APT via
/// Polkit `pkexec`) or removes a user-local `.desktop` launcher.
pub fn uninstall_app(app: &InstalledApp) -> Result<String, String> {
    match detect_install_source(app) {
        AppInstallSource::FlatpakUser(app_id) => {
            if !is_safe_pkg_token(&app_id) {
                return Err("Invalid Flatpak application ID".into());
            }
            let out = Command::new("flatpak")
                .args(["uninstall", "-y", "--noninteractive", "--user", &app_id])
                .output()
                .map_err(|e| format!("Failed to run flatpak: {e}"))?;
            if out.status.success() {
                Ok(format!("Uninstalled {}", app.name))
            } else {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(if err.is_empty() {
                    "Flatpak uninstall failed".into()
                } else {
                    err
                })
            }
        }
        AppInstallSource::FlatpakSystem(app_id) => {
            if !is_safe_pkg_token(&app_id) {
                return Err("Invalid Flatpak application ID".into());
            }
            // Prefer --user if installed per-user, otherwise elevate cleanly via pkexec.
            let is_user = Command::new("flatpak")
                .args(["info", "--user", &app_id])
                .output()
                .is_ok_and(|o| o.status.success());
            let out = if is_user {
                Command::new("flatpak")
                    .args(["uninstall", "-y", "--noninteractive", "--user", &app_id])
                    .output()
            } else {
                Command::new("pkexec")
                    .args(["flatpak", "uninstall", "-y", "--noninteractive", &app_id])
                    .output()
            }
            .map_err(|e| format!("Failed to uninstall Flatpak app: {e}"))?;
            if out.status.success() {
                Ok(format!("Uninstalled {}", app.name))
            } else {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(if err.is_empty() {
                    "Uninstall cancelled or failed".into()
                } else {
                    err
                })
            }
        }
        AppInstallSource::Snap(snap_name) => {
            if !is_safe_pkg_token(&snap_name) {
                return Err("Invalid Snap package name".into());
            }
            let out = Command::new("pkexec")
                .args(["snap", "remove", &snap_name])
                .output()
                .map_err(|e| format!("Failed to run snap remove: {e}"))?;
            if out.status.success() {
                Ok(format!("Uninstalled {}", app.name))
            } else {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(if err.is_empty() {
                    "Snap removal cancelled or failed".into()
                } else {
                    err
                })
            }
        }
        AppInstallSource::UserDesktop(path) => {
            fs::remove_file(&path)
                .map_err(|e| format!("Failed to remove user launcher {}: {e}", path.display()))?;
            Ok(format!("Removed {}", app.name))
        }
        AppInstallSource::SystemPackage(path) => {
            let Some(pkg) = resolve_dpkg_owner(&path) else {
                return Err(format!(
                    "{} is not managed by APT, Flatpak, or Snap and must be removed with its own uninstaller.",
                    app.name
                ));
            };
            if is_protected_system_package(&pkg) {
                return Err(format!(
                    "Cannot uninstall protected core system package '{pkg}'."
                ));
            }
            let out = Command::new("pkexec")
                .args([
                    "env",
                    "DEBIAN_FRONTEND=noninteractive",
                    "apt-get",
                    "remove",
                    "-y",
                    &pkg,
                ])
                .output()
                .map_err(|e| format!("Failed to launch pkexec apt-get: {e}"))?;
            if out.status.success() {
                Ok(format!("Uninstalled {} ({pkg})", app.name))
            } else {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(if err.is_empty() {
                    "Uninstall cancelled or failed".into()
                } else {
                    err
                })
            }
        }
        AppInstallSource::Unknown => Err(format!(
            "Could not determine package source for {}.",
            app.name
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_app(id: &str, exec: &str, path: &str) -> InstalledApp {
        InstalledApp {
            desktop_id: id.into(),
            name: "Sample".into(),
            comment: None,
            exec: exec.into(),
            icon: None,
            categories: Vec::new(),
            mime_types: Vec::new(),
            terminal: false,
            working_dir: None,
            desktop_path: Some(PathBuf::from(path)),
        }
    }

    #[test]
    fn detects_flatpak_snap_user_and_system_sources() {
        let fp_u = sample_app(
            "com.spotify.Client.desktop",
            "/usr/bin/flatpak run com.spotify.Client",
            "/home/test/.local/share/flatpak/exports/share/applications/com.spotify.Client.desktop",
        );
        assert_eq!(
            detect_install_source(&fp_u),
            AppInstallSource::FlatpakUser("com.spotify.Client".into())
        );

        let fp_s = sample_app(
            "org.videolan.VLC.desktop",
            "flatpak run org.videolan.VLC",
            "/var/lib/flatpak/exports/share/applications/org.videolan.VLC.desktop",
        );
        assert_eq!(
            detect_install_source(&fp_s),
            AppInstallSource::FlatpakSystem("org.videolan.VLC".into())
        );

        let snap = sample_app(
            "firefox_firefox.desktop",
            "/snap/bin/firefox %u",
            "/var/lib/snapd/desktop/applications/firefox_firefox.desktop",
        );
        assert_eq!(
            detect_install_source(&snap),
            AppInstallSource::Snap("firefox".into())
        );

        let usr = sample_app(
            "my-pwa.desktop",
            "/usr/bin/google-chrome --app-id=abc",
            "/home/test/.local/share/applications/my-pwa.desktop",
        );
        assert!(matches!(
            detect_install_source(&usr),
            AppInstallSource::UserDesktop(_)
        ));

        let deb = sample_app(
            "gimp.desktop",
            "gimp-2.10 %U",
            "/usr/share/applications/gimp.desktop",
        );
        assert!(matches!(
            detect_install_source(&deb),
            AppInstallSource::SystemPackage(_)
        ));
    }

    #[test]
    fn protects_core_system_and_neon_packages() {
        assert!(is_protected_system_package("neon-compositor"));
        assert!(is_protected_system_package("neon-shell"));
        assert!(is_protected_system_package("systemd"));
        assert!(is_protected_system_package("sudo"));
        assert!(!is_protected_system_package("vlc"));
        assert!(!is_protected_system_package("gimp"));
    }
}
