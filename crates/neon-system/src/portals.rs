//! XDG Desktop Portal discovery and NEON DE portal configuration support.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use zbus::blocking::Connection;

use crate::notifications::bus_name_has_owner;

pub const PORTAL_DESKTOP_BUS: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_DOCUMENTS_BUS: &str = "org.freedesktop.portal.Documents";

/// Runtime status of XDG Desktop Portals in the current Linux session.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PortalStatus {
    pub desktop_portal_running: bool,
    pub documents_portal_running: bool,
    pub current_desktop: String,
    pub installed_backends: Vec<String>,
}

/// Queries live XDG Desktop Portal bus availability and installed `.portal` backends.
pub fn query_portal_status() -> PortalStatus {
    let (desktop_portal_running, documents_portal_running) = match Connection::session() {
        Ok(conn) => (
            bus_name_has_owner(&conn, PORTAL_DESKTOP_BUS),
            bus_name_has_owner(&conn, PORTAL_DOCUMENTS_BUS),
        ),
        Err(_) => (false, false),
    };

    let current_desktop =
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "NEON-DE".to_string());
    let installed_backends =
        discover_portal_backends(Path::new("/usr/share/xdg-desktop-portal/portals"));

    PortalStatus {
        desktop_portal_running,
        documents_portal_running,
        current_desktop,
        installed_backends,
    }
}

/// Returns the canonical `neon-de-portals.conf` contents for standalone NEON DE sessions.
pub fn neon_portal_config_template() -> &'static str {
    "[preferred]\n\
     default=gtk\n\
     org.freedesktop.impl.portal.ScreenCast=wlr\n\
     org.freedesktop.impl.portal.Screenshot=wlr\n\
     org.freedesktop.impl.portal.Secret=gnome-keyring\n\
     org.freedesktop.impl.portal.FileChooser=gtk\n\
     org.freedesktop.impl.portal.Settings=gtk\n"
}

/// Propagates NEON DE's Light/Dark theme live to all third-party Wayland & X11 apps
/// (Chrome, Electron/Antigravity, Firefox, GTK3/GTK4/libadwaita, and Qt6) by updating:
/// 1. `gsettings org.gnome.desktop.interface color-scheme` (`prefer-light` / `prefer-dark`),
///    which immediately causes `xdg-desktop-portal-gtk` to emit
///    `org.freedesktop.portal.Settings.SettingChanged("org.freedesktop.appearance", "color-scheme")`
/// 2. `gsettings org.gnome.desktop.interface gtk-theme` & `icon-theme`
/// 3. `~/.config/gtk-3.0/settings.ini` and `~/.config/gtk-4.0/settings.ini`
pub fn sync_system_color_scheme(is_light: bool) {
    // Avoid mutating the host session when running in isolated headless/nested tests without D-Bus.
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        return;
    }

    let scheme = if is_light {
        "prefer-light"
    } else {
        "prefer-dark"
    };
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "color-scheme", scheme])
        .status();

    let gtk_theme = resolve_gtk_theme_for_mode(is_light);
    if let Some(ref theme) = gtk_theme {
        let _ = Command::new("gsettings")
            .args(["set", "org.gnome.desktop.interface", "gtk-theme", theme])
            .status();
    }

    let icon_theme = resolve_icon_theme_for_mode(is_light);
    if let Some(ref icon) = icon_theme {
        let _ = Command::new("gsettings")
            .args(["set", "org.gnome.desktop.interface", "icon-theme", icon])
            .status();
    }

    if let Some(config_home) = xdg_config_home() {
        for sub in ["gtk-3.0/settings.ini", "gtk-4.0/settings.ini"] {
            let path = config_home.join(sub);
            let _ = update_gtk_settings_ini(
                &path,
                is_light,
                gtk_theme.as_deref(),
                icon_theme.as_deref(),
            );
        }
    }
}

fn xdg_config_home() -> Option<PathBuf> {
    if let Some(cfg) = std::env::var_os("XDG_CONFIG_HOME")
        && !cfg.is_empty()
    {
        return Some(PathBuf::from(cfg));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
}

fn theme_dir_exists(name: &str) -> bool {
    let mut roots = vec![
        PathBuf::from("/usr/share/themes"),
        PathBuf::from("/usr/local/share/themes"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        roots.push(h.join(".themes"));
        roots.push(h.join(".local/share/themes"));
    }
    roots.iter().any(|r| r.join(name).is_dir())
}

fn icon_theme_dir_exists(name: &str) -> bool {
    let mut roots = vec![
        PathBuf::from("/usr/share/icons"),
        PathBuf::from("/usr/local/share/icons"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        roots.push(h.join(".icons"));
        roots.push(h.join(".local/share/icons"));
    }
    roots.iter().any(|r| r.join(name).is_dir())
}

fn resolve_gtk_theme_for_mode(is_light: bool) -> Option<String> {
    let current = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .trim_matches('\'')
                .to_string()
        })
        .unwrap_or_default();

    let mut candidates: Vec<String> = Vec::new();
    if !current.is_empty() {
        let base = current
            .strip_suffix("-dark")
            .or_else(|| current.strip_suffix("-Dark"))
            .or_else(|| current.strip_suffix("-light"))
            .or_else(|| current.strip_suffix("-Light"))
            .unwrap_or(&current);
        if is_light {
            candidates.push(base.to_string());
            candidates.push(format!("{base}-light"));
            candidates.push(format!("{base}-Light"));
        } else {
            candidates.push(format!("{base}-dark"));
            candidates.push(format!("{base}-Dark"));
        }
    }
    if is_light {
        candidates.extend(["adw-gtk3".into(), "Adwaita".into(), "Yaru".into()]);
    } else {
        candidates.extend([
            "adw-gtk3-dark".into(),
            "Adwaita-dark".into(),
            "Yaru-dark".into(),
        ]);
    }
    candidates.into_iter().find(|c| theme_dir_exists(c))
}

fn resolve_icon_theme_for_mode(is_light: bool) -> Option<String> {
    let target = if is_light {
        "WhiteSur-light"
    } else {
        "WhiteSur-dark"
    };
    if icon_theme_dir_exists(target) {
        Some(target.to_string())
    } else {
        None
    }
}

/// Updates `gtk-application-prefer-dark-theme`, `gtk-theme-name`, and `gtk-icon-theme-name`
/// inside a GTK 3 / GTK 4 `settings.ini` file while preserving all other keys untouched.
pub fn update_gtk_settings_ini_text(
    existing: &str,
    is_light: bool,
    gtk_theme: Option<&str>,
    icon_theme: Option<&str>,
) -> String {
    let dark_val = if is_light { "0" } else { "1" };
    let mut out: Vec<String> = Vec::new();
    let mut in_settings = false;
    let mut saw_settings_header = false;
    let mut wrote_dark = false;
    let mut wrote_theme = false;
    let mut wrote_icon = false;

    for raw_line in existing.lines() {
        let trimmed = raw_line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_settings {
                if !wrote_dark {
                    out.push(format!("gtk-application-prefer-dark-theme={dark_val}"));
                    wrote_dark = true;
                }
                if !wrote_theme
                    && let Some(t) = gtk_theme
                {
                    out.push(format!("gtk-theme-name={t}"));
                    wrote_theme = true;
                }
                if !wrote_icon
                    && let Some(ic) = icon_theme
                {
                    out.push(format!("gtk-icon-theme-name={ic}"));
                    wrote_icon = true;
                }
            }
            in_settings = trimmed == "[Settings]";
            if in_settings {
                saw_settings_header = true;
            }
            out.push(raw_line.to_string());
            continue;
        }
        if in_settings
            && let Some((k, _)) = trimmed.split_once('=')
        {
            match k.trim() {
                "gtk-application-prefer-dark-theme" => {
                    out.push(format!("gtk-application-prefer-dark-theme={dark_val}"));
                    wrote_dark = true;
                    continue;
                }
                "gtk-theme-name" if gtk_theme.is_some() => {
                    if let Some(t) = gtk_theme {
                        out.push(format!("gtk-theme-name={t}"));
                        wrote_theme = true;
                        continue;
                    }
                }
                "gtk-icon-theme-name" if icon_theme.is_some() => {
                    if let Some(ic) = icon_theme {
                        out.push(format!("gtk-icon-theme-name={ic}"));
                        wrote_icon = true;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(raw_line.to_string());
    }

    if !saw_settings_header {
        out.push("[Settings]".to_string());
    }
    if !wrote_dark {
        out.push(format!("gtk-application-prefer-dark-theme={dark_val}"));
    }
    if !wrote_theme
        && let Some(t) = gtk_theme
    {
        out.push(format!("gtk-theme-name={t}"));
    }
    if !wrote_icon
        && let Some(ic) = icon_theme
    {
        out.push(format!("gtk-icon-theme-name={ic}"));
    }

    let mut result = out.join("\n");
    result.push('\n');
    result
}

fn update_gtk_settings_ini(
    path: &Path,
    is_light: bool,
    gtk_theme: Option<&str>,
    icon_theme: Option<&str>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing = fs::read_to_string(path).unwrap_or_default();
    let updated = update_gtk_settings_ini_text(&existing, is_light, gtk_theme, icon_theme);
    fs::write(path, updated)
}

fn discover_portal_backends(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) == Some("portal") {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_config_template_declares_preferred_gtk_and_secret_backends() {
        let cfg = neon_portal_config_template();
        assert!(cfg.contains("[preferred]"));
        assert!(cfg.contains("default=gtk"));
        assert!(cfg.contains("org.freedesktop.impl.portal.Secret=gnome-keyring"));
    }

    #[test]
    fn gtk_settings_ini_updates_dark_mode_and_preserves_other_keys() {
        let input = "[Settings]\ngtk-font-name=Inter 11\ngtk-theme-name=adw-gtk3-dark\ngtk-application-prefer-dark-theme=1\n";
        let light = update_gtk_settings_ini_text(
            input,
            true,
            Some("adw-gtk3"),
            Some("WhiteSur-light"),
        );
        assert!(light.contains("gtk-font-name=Inter 11"));
        assert!(light.contains("gtk-theme-name=adw-gtk3\n"));
        assert!(light.contains("gtk-application-prefer-dark-theme=0\n"));
        assert!(light.contains("gtk-icon-theme-name=WhiteSur-light\n"));

        let dark = update_gtk_settings_ini_text(&light, false, Some("adw-gtk3-dark"), Some("WhiteSur-dark"));
        assert!(dark.contains("gtk-theme-name=adw-gtk3-dark\n"));
        assert!(dark.contains("gtk-application-prefer-dark-theme=1\n"));
        assert!(dark.contains("gtk-icon-theme-name=WhiteSur-dark\n"));
    }
}
