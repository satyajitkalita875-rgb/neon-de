//! Real Linux hardware and OS discovery for About NEON DE.

use std::{fs, process::Command};

#[derive(Debug, Clone)]
pub struct HardwareSummary {
    pub os_name: String,
    pub hostname: String,
    pub kernel: String,
    pub architecture: String,
    pub cpu_model: String,
    pub gpu_models: Vec<String>,
    pub memory_gib: String,
    pub uptime: String,
}

pub fn collect_hardware_summary() -> HardwareSummary {
    let os_name = read_os_pretty_name().unwrap_or_else(|| "Linux".into());
    let hostname = fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into());
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let architecture = std::env::consts::ARCH.to_string();
    let cpu_model = read_cpu_model().unwrap_or_else(|| "Unknown Processor".into());
    let gpu_models = detect_gpus();
    let memory_gib = read_memory_gib().unwrap_or_else(|| "Unknown".into());
    let uptime = read_uptime().unwrap_or_else(|| "Unknown".into());

    HardwareSummary {
        os_name,
        hostname,
        kernel,
        architecture,
        cpu_model,
        gpu_models,
        memory_gib,
        uptime,
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
    let mut cores = 0usize;
    let mut model = None;
    for line in content.lines() {
        if line.starts_with("processor") {
            cores += 1;
        }
        if model.is_none()
            && let Some(rest) = line.strip_prefix("model name")
            && let Some((_, val)) = rest.split_once(':')
        {
            model = Some(val.trim().to_string());
        }
    }
    model.map(|m| {
        if cores > 0 {
            format!("{m} ({cores} threads)")
        } else {
            m
        }
    })
}

fn detect_gpus() -> Vec<String> {
    let out = Command::new("lspci").output();
    let Ok(out) = out else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut gpus = Vec::new();
    for line in text.lines() {
        let is_gpu = line.contains("VGA compatible controller:")
            || line.contains("3D controller:")
            || line.contains("Display controller:");
        if is_gpu && let Some((_, desc)) = line.split_once(": ") {
            gpus.push(desc.trim().to_string());
        }
    }
    gpus
}

fn read_memory_gib() -> Option<String> {
    let content = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total_kb = None;
    let mut avail_kb = None;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = rest.split_whitespace().next()?.parse::<f64>().ok();
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = rest.split_whitespace().next()?.parse::<f64>().ok();
        }
    }
    let total = total_kb? / (1024.0 * 1024.0);
    if let Some(avail) = avail_kb {
        let used = (total - avail / (1024.0 * 1024.0)).max(0.0);
        Some(format!("{used:.1} / {total:.1} GiB"))
    } else {
        Some(format!("{total:.1} GiB"))
    }
}

fn read_uptime() -> Option<String> {
    let raw = fs::read_to_string("/proc/uptime").ok()?;
    let secs = raw.split_whitespace().next()?.parse::<f64>().ok()? as u64;
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        Some(format!("{days}d {hours}h {mins}m"))
    } else if hours > 0 {
        Some(format!("{hours}h {mins}m"))
    } else {
        Some(format!("{mins}m"))
    }
}
