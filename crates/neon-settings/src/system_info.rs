//! Real Linux hardware, OS, and system service discovery for the System settings page.

use std::{fs, process::Command};

#[derive(Debug, Clone)]
pub struct HardwareSummary {
    pub os_name: String,
    pub kernel: String,
    pub cpu_model: String,
    pub memory_gib: String,
    pub services: Vec<(&'static str, bool)>,
}

pub fn collect_hardware_summary() -> HardwareSummary {
    let os_name = read_os_pretty_name().unwrap_or_else(|| "Linux".into());
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let cpu_model = read_cpu_model().unwrap_or_else(|| "x86_64 Processor".into());
    let memory_gib = read_memory_gib().unwrap_or_else(|| "Unknown".into());

    let services = vec![
        ("PipeWire Audio", service_or_proc_active("pipewire")),
        ("WirePlumber", service_or_proc_active("wireplumber")),
        ("NetworkManager", service_or_proc_active("NetworkManager")),
        ("BlueZ Bluetooth", service_or_proc_active("bluetoothd")),
        ("UPower Battery", service_or_proc_active("upowerd")),
    ];

    HardwareSummary {
        os_name,
        kernel,
        cpu_model,
        memory_gib,
        services,
    }
}

fn read_os_pretty_name() -> Option<String> {
    let content = fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
            return Some(rest.trim_matches('"').to_string());
        }
    }
    None
}

fn read_cpu_model() -> Option<String> {
    let content = fs::read_to_string("/proc/cpuinfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("model name")
            && let Some((_, val)) = rest.split_once(':')
        {
            return Some(val.trim().to_string());
        }
    }
    None
}

fn read_memory_gib() -> Option<String> {
    let content = fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: f64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(format!("{:.1} GiB", kb / (1024.0 * 1024.0)));
        }
    }
    None
}

fn service_or_proc_active(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
