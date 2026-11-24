//! Host distribution package-manager and OS update inspection (safe read-only + native integration).

use std::{fs, process::Command};

use serde::{Deserialize, Serialize};

/// A pending software update reported by the host distribution package manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpdate {
    pub package: String,
    pub new_version: String,
    pub detail: String,
}

/// Snapshot of OS/distribution info, installed packages, sources, and pending updates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatesSnapshot {
    pub os_pretty_name: String,
    pub distro_id: String,
    pub version_id: String,
    pub kernel: String,
    pub package_manager: String,
    pub installed_package_count: usize,
    pub pending_updates: Vec<PendingUpdate>,
    pub software_sources: Vec<String>,
}

/// Queries host OS metadata, package count, software sources, and available updates.
pub fn query_updates_snapshot() -> UpdatesSnapshot {
    let (os_pretty_name, distro_id, version_id) = read_os_release();
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());

    let (package_manager, installed_package_count, pending_updates, software_sources) =
        if which_exists("apt") {
            (
                "APT (Debian / Ubuntu)".to_string(),
                count_dpkg_packages(),
                query_apt_upgradable(),
                list_apt_sources(),
            )
        } else if which_exists("dnf") {
            (
                "DNF (Fedora / RHEL)".to_string(),
                count_rpm_packages(),
                Vec::new(),
                Vec::new(),
            )
        } else if which_exists("pacman") {
            (
                "Pacman (Arch Linux)".to_string(),
                count_pacman_packages(),
                Vec::new(),
                Vec::new(),
            )
        } else {
            ("Unknown".to_string(), 0, Vec::new(), Vec::new())
        };

    UpdatesSnapshot {
        os_pretty_name,
        distro_id,
        version_id,
        kernel,
        package_manager,
        installed_package_count,
        pending_updates,
        software_sources,
    }
}

fn read_os_release() -> (String, String, String) {
    let mut pretty = "Linux".to_string();
    let mut id = "linux".to_string();
    let mut ver = String::new();
    if let Ok(raw) = fs::read_to_string("/etc/os-release") {
        for line in raw.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                pretty = v.trim_matches('"').to_string();
            } else if let Some(v) = line.strip_prefix("ID=") {
                id = v.trim_matches('"').to_string();
            } else if let Some(v) = line.strip_prefix("VERSION_ID=") {
                ver = v.trim_matches('"').to_string();
            }
        }
    }
    (pretty, id, ver)
}

fn count_dpkg_packages() -> usize {
    Command::new("dpkg-query")
        .args(["-f", ".\n", "-W"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(0)
}

fn count_rpm_packages() -> usize {
    Command::new("rpm")
        .arg("-qa")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(0)
}

fn count_pacman_packages() -> usize {
    Command::new("pacman")
        .arg("-Qq")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(0)
}

fn query_apt_upgradable() -> Vec<PendingUpdate> {
    let Ok(out) = Command::new("apt").args(["list", "--upgradable"]).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_apt_upgradable(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_apt_upgradable(output: &str) -> Vec<PendingUpdate> {
    let mut list = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Listing") {
            continue;
        }
        let Some((pkg_part, rest)) = trimmed.split_once(' ') else {
            continue;
        };
        let package = pkg_part.split('/').next().unwrap_or(pkg_part).to_string();
        let mut parts = rest.splitn(2, ' ');
        let new_version = parts.next().unwrap_or("").to_string();
        let detail = parts.next().unwrap_or("").trim().to_string();
        if !package.is_empty() {
            list.push(PendingUpdate {
                package,
                new_version,
                detail,
            });
        }
    }
    list
}

fn list_apt_sources() -> Vec<String> {
    let mut sources = Vec::new();
    if let Ok(entries) = fs::read_dir("/etc/apt/sources.list.d") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".sources") || name.ends_with(".list") {
                sources.push(name);
            }
        }
    }
    sources.sort();
    sources
}

fn which_exists(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apt_upgradable_lines() {
        let sample = "Listing...\nmesa-vulkan-drivers/resolute-updates 26.0.1-1 amd64 [upgradable from: 26.0.0-1]\n";
        let updates = parse_apt_upgradable(sample);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].package, "mesa-vulkan-drivers");
        assert_eq!(updates[0].new_version, "26.0.1-1");
    }
}
