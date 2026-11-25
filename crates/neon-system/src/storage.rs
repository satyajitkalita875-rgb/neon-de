//! Storage (UDisks2 / lsblk / df), CUPS printer (`lpstat`), and scanner (`scanimage`) integration.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// A mounted or block storage device discovered on the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageDevice {
    pub device: String,
    pub model: String,
    pub fstype: String,
    pub mountpoint: Option<String>,
    pub size_human: String,
    pub used_human: String,
    pub avail_human: String,
    pub used_percent: u8,
    pub removable: bool,
}

/// A CUPS printer discovered via `lpstat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrinterInfo {
    pub name: String,
    pub status: String,
    pub is_default: bool,
    pub uri: Option<String>,
}

/// Combined snapshot of storage, printers, and scanner capability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageAndDevicesSnapshot {
    pub udisks_available: bool,
    pub devices: Vec<StorageDevice>,
    pub cups_available: bool,
    pub printers: Vec<PrinterInfo>,
    pub sane_available: bool,
    pub scanners: Vec<String>,
}

/// Queries live mounted storage devices, removable media, and CUPS printers without blocking
/// on slow USB/mDNS SANE scanner probing (`scanimage -L`).
pub fn query_storage_and_devices() -> StorageAndDevicesSnapshot {
    let udisks_available = which_exists("udisksctl");
    let devices = query_block_devices();
    let (cups_available, printers) = query_printers();

    StorageAndDevicesSnapshot {
        udisks_available,
        devices,
        cups_available,
        printers,
        sane_available: false,
        scanners: Vec::new(),
    }
}

/// Safely unmounts and powers off (ejects) a removable block device via `udisksctl`.
pub fn eject_removable_device(device_path: &str) -> Result<(), String> {
    if !device_path.starts_with("/dev/")
        || device_path.contains("..")
        || !device_path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_'))
    {
        return Err("invalid block device path".into());
    }
    let umount = Command::new("udisksctl")
        .args(["unmount", "-b", device_path])
        .output()
        .map_err(|e| format!("failed to run udisksctl: {e}"))?;
    if !umount.status.success() {
        let stderr = String::from_utf8_lossy(&umount.stderr);
        if !stderr.contains("not mounted") {
            return Err(format!("unmount failed: {}", stderr.trim()));
        }
    }
    let _ = Command::new("udisksctl")
        .args(["power-off", "-b", device_path])
        .output();
    Ok(())
}

fn query_block_devices() -> Vec<StorageDevice> {
    let df_map = query_df_usage();
    let Ok(out) = Command::new("lsblk")
        .args(["-P", "-b", "-o", "PATH,MODEL,FSTYPE,MOUNTPOINT,SIZE,RM,TYPE"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_lsblk_pairs(&text, &df_map)
}

#[derive(Debug, Clone)]
struct DfEntry {
    used: String,
    avail: String,
    pct: u8,
}

fn query_df_usage() -> std::collections::HashMap<String, DfEntry> {
    let mut map = std::collections::HashMap::new();
    let Ok(out) = Command::new("df")
        .args(["-h", "--output=source,used,avail,pcent"])
        .output()
    else {
        return map;
    };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let pct = parts[3]
                .trim_end_matches('%')
                .parse::<u8>()
                .unwrap_or(0)
                .min(100);
            map.insert(
                parts[0].to_string(),
                DfEntry {
                    used: parts[1].to_string(),
                    avail: parts[2].to_string(),
                    pct,
                },
            );
        }
    }
    map
}

fn parse_lsblk_pairs(
    text: &str,
    df_map: &std::collections::HashMap<String, DfEntry>,
) -> Vec<StorageDevice> {
    let mut devices = Vec::new();
    for line in text.lines() {
        let mut path = String::new();
        let mut model = String::new();
        let mut fstype = String::new();
        let mut mountpoint = String::new();
        let mut size_bytes: u64 = 0;
        let mut rm = false;
        let mut kind = String::new();

        for (k, v) in parse_key_value_pairs(line) {
            match k.as_str() {
                "PATH" => path = v,
                "MODEL" => model = v,
                "FSTYPE" => fstype = v,
                "MOUNTPOINT" => mountpoint = v,
                "SIZE" => size_bytes = v.parse().unwrap_or(0),
                "RM" => rm = v == "1",
                "TYPE" => kind = v,
                _ => {}
            }
        }
        if path.is_empty() || kind == "loop" || fstype == "swap" {
            continue;
        }
        if mountpoint.is_empty() && !rm && kind != "disk" {
            continue;
        }
        if mountpoint.starts_with("/snap/") || mountpoint.starts_with("/boot/efi") {
            continue;
        }
        let usage = df_map.get(&path);
        devices.push(StorageDevice {
            device: path,
            model: if model.is_empty() {
                kind.clone()
            } else {
                model
            },
            fstype: if fstype.is_empty() {
                "—".into()
            } else {
                fstype
            },
            mountpoint: if mountpoint.is_empty() {
                None
            } else {
                Some(mountpoint)
            },
            size_human: format_bytes_human(size_bytes),
            used_human: usage.map(|u| u.used.clone()).unwrap_or_else(|| "—".into()),
            avail_human: usage.map(|u| u.avail.clone()).unwrap_or_else(|| "—".into()),
            used_percent: usage.map(|u| u.pct).unwrap_or(0),
            removable: rm,
        });
    }
    devices
}

fn parse_key_value_pairs(line: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut rest = line.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else {
            break;
        };
        let key = rest[..eq].trim().to_string();
        let after = &rest[eq + 1..];
        if let Some(stripped) = after.strip_prefix('"') {
            if let Some(end_q) = stripped.find('"') {
                let val = stripped[..end_q].trim().to_string();
                pairs.push((key, val));
                rest = stripped[end_q + 1..].trim_start();
            } else {
                break;
            }
        } else {
            let end = after.find(' ').unwrap_or(after.len());
            pairs.push((key, after[..end].to_string()));
            rest = after[end..].trim_start();
        }
    }
    pairs
}

pub fn format_bytes_human(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = GIB * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.2} TiB", b / TIB)
    } else if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.0} MiB", b / MIB)
    } else {
        format!("{bytes} B")
    }
}

fn query_printers() -> (bool, Vec<PrinterInfo>) {
    if !which_exists("lpstat") {
        return (false, Vec::new());
    }
    let default_name = Command::new("lpstat")
        .arg("-d")
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.split(':').nth(1).map(|v| v.trim().to_string())
        });
    let mut uris = std::collections::HashMap::new();
    if let Ok(out) = Command::new("lpstat").arg("-v").output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(rest) = line.strip_prefix("device for ")
                && let Some((name, uri)) = rest.split_once(':')
            {
                uris.insert(name.trim().to_string(), uri.trim().to_string());
            }
        }
    }
    let mut printers = Vec::new();
    let Ok(out) = Command::new("lpstat").arg("-p").output() else {
        return (true, printers);
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.strip_prefix("printer ") {
            let mut parts = rest.splitn(2, ' ');
            let name = parts.next().unwrap_or("").trim().to_string();
            let status = parts.next().unwrap_or("Ready").trim().to_string();
            if !name.is_empty() {
                let is_default = default_name.as_deref() == Some(name.as_str());
                let uri = uris.get(&name).cloned();
                printers.push(PrinterInfo {
                    name,
                    status,
                    is_default,
                    uri,
                });
            }
        }
    }
    (true, printers)
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
    fn parses_lsblk_output_and_formats_sizes() {
        assert_eq!(format_bytes_human(512 * 1024 * 1024 * 1024), "512.0 GiB");
        let sample = "PATH=\"/dev/nvme0n1p2\" MODEL=\"\" FSTYPE=\"ext4\" MOUNTPOINT=\"/\" SIZE=\"256000000000\" RM=\"0\" TYPE=\"part\"\n\
                      PATH=\"/dev/loop0\" MODEL=\"\" FSTYPE=\"squashfs\" MOUNTPOINT=\"/snap/core\" SIZE=\"64000000\" RM=\"0\" TYPE=\"loop\"\n";
        let mut df = std::collections::HashMap::new();
        df.insert(
            "/dev/nvme0n1p2".to_string(),
            DfEntry {
                used: "90G".into(),
                avail: "148G".into(),
                pct: 38,
            },
        );
        let devs = parse_lsblk_pairs(sample, &df);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].device, "/dev/nvme0n1p2");
        assert_eq!(devs[0].mountpoint.as_deref(), Some("/"));
        assert_eq!(devs[0].used_percent, 38);
    }
}
