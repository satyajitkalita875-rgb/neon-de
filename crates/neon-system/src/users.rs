//! User account discovery and AccountsService (`org.freedesktop.Accounts`) integration.
//!
//! Never handles or stores user passwords directly; privileged account updates rely on
//! AccountsService + PolicyKit1 authorization.

use std::{fs, process::Command};

use serde::{Deserialize, Serialize};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::OwnedObjectPath,
};

const ACCOUNTS_DEST: &str = "org.freedesktop.Accounts";
const ACCOUNTS_PATH: &str = "/org/freedesktop/Accounts";
const ACCOUNTS_IFACE: &str = "org.freedesktop.Accounts";
const ACCOUNTS_USER_IFACE: &str = "org.freedesktop.Accounts.User";

/// Information about a Linux desktop user account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAccountInfo {
    pub uid: u32,
    pub username: String,
    pub real_name: String,
    pub is_admin: bool,
    pub home_dir: String,
    pub shell: String,
    pub icon_file: Option<String>,
    pub is_current: bool,
    pub object_path: Option<String>,
}

/// Snapshot of AccountsService availability and local human user accounts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsersSnapshot {
    pub accounts_service_available: bool,
    pub current_user: Option<UserAccountInfo>,
    pub all_users: Vec<UserAccountInfo>,
}

/// Queries AccountsService D-Bus with safe `/etc/passwd` fallback.
pub fn query_users() -> UsersSnapshot {
    let current_uid = unsafe { libc::geteuid() };
    let mut snap = UsersSnapshot::default();

    if let Ok(conn) = Connection::system()
        && let Ok(mgr) = Proxy::new(&conn, ACCOUNTS_DEST, ACCOUNTS_PATH, ACCOUNTS_IFACE)
        && let Ok(paths) = mgr.call::<_, _, Vec<OwnedObjectPath>>("ListCachedUsers", &())
    {
        snap.accounts_service_available = true;
        for path in paths {
            let Ok(u) = Proxy::new(&conn, ACCOUNTS_DEST, path.as_str(), ACCOUNTS_USER_IFACE) else {
                continue;
            };
            let uid = u.get_property::<u64>("Uid").unwrap_or(0) as u32;
            let username = u.get_property::<String>("UserName").unwrap_or_default();
            let real_name = u
                .get_property::<String>("RealName")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| username.clone());
            let account_type = u.get_property::<i32>("AccountType").unwrap_or(0);
            let home_dir = u.get_property::<String>("HomeDirectory").unwrap_or_default();
            let shell = u.get_property::<String>("Shell").unwrap_or_default();
            let icon_file = u
                .get_property::<String>("IconFile")
                .ok()
                .filter(|p| !p.is_empty() && std::path::Path::new(p).exists());

            let info = UserAccountInfo {
                uid,
                username,
                real_name,
                is_admin: account_type == 1,
                home_dir,
                shell,
                icon_file,
                is_current: uid == current_uid,
                object_path: Some(path.as_str().to_string()),
            };
            if info.is_current {
                snap.current_user = Some(info.clone());
            }
            snap.all_users.push(info);
        }
    }

    if snap.all_users.is_empty() {
        let admin_members = read_admin_group_members();
        if let Ok(passwd) = fs::read_to_string("/etc/passwd") {
            for line in passwd.lines() {
                let cols: Vec<&str> = line.split(':').collect();
                if cols.len() < 7 {
                    continue;
                }
                let Ok(uid) = cols[2].parse::<u32>() else {
                    continue;
                };
                if uid < 1000 || uid == 65534 {
                    continue;
                }
                let username = cols[0].to_string();
                let real_name = cols[4]
                    .split(',')
                    .next()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&username)
                    .to_string();
                let info = UserAccountInfo {
                    uid,
                    is_admin: admin_members.contains(&username),
                    username,
                    real_name,
                    home_dir: cols[5].to_string(),
                    shell: cols[6].to_string(),
                    icon_file: None,
                    is_current: uid == current_uid,
                    object_path: None,
                };
                if info.is_current {
                    snap.current_user = Some(info.clone());
                }
                snap.all_users.push(info);
            }
        }
    }

    snap
}

/// Updates a user's display name via AccountsService D-Bus (polkit-authorized).
pub fn set_user_real_name(object_path: &str, real_name: &str) -> Result<(), String> {
    let trimmed = real_name.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.contains(['\n', '\r', ':']) {
        return Err("invalid display name".into());
    }
    let conn = Connection::system().map_err(|e| format!("system D-Bus unavailable: {e}"))?;
    let u = Proxy::new(&conn, ACCOUNTS_DEST, object_path, ACCOUNTS_USER_IFACE)
        .map_err(|e| format!("AccountsService user proxy failed: {e}"))?;
    u.call::<_, _, ()>("SetRealName", &(trimmed,))
        .map_err(|e| format!("SetRealName failed (polkit authorization may be required): {e}"))?;
    Ok(())
}

fn read_admin_group_members() -> std::collections::HashSet<String> {
    let mut members = std::collections::HashSet::new();
    if let Ok(out) = Command::new("getent").args(["group", "sudo", "wheel"]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(list) = line.split(':').nth(3) {
                for m in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    members.insert(m.to_string());
                }
            }
        }
    }
    members
}
