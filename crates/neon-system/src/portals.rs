//! XDG Desktop Portal discovery and NEON DE portal configuration support.

use std::{fs, path::Path};

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
     org.freedesktop.impl.portal.Secret=gnome-keyring\n\
     org.freedesktop.impl.portal.FileChooser=gtk\n\
     org.freedesktop.impl.portal.Settings=gtk\n"
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
}
