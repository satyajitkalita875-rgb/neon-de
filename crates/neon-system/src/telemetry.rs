//! Real Linux system telemetry (`/proc/stat`, `/proc/meminfo`, `/proc/net/dev`,
//! `/sys/class/thermal`, `statvfs`) and `systemd-inhibit` Keep Awake service for NEON DE.

use std::{
    ffi::CString,
    fs,
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::Instant,
};

use serde::{Deserialize, Serialize};

/// Snapshot of live Linux CPU, memory, network, and root filesystem stats.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    /// CPU utilization percentage (`0.0..=100.0`).
    pub cpu_percent: f32,
    /// Optional CPU package temperature in °C.
    pub cpu_temp_c: Option<f32>,
    /// Used RAM in gigabytes (GiB).
    pub mem_used_gb: f32,
    /// Total RAM in gigabytes (GiB).
    pub mem_total_gb: f32,
    /// Network download throughput in bytes/sec.
    pub net_rx_bps: u64,
    /// Network upload throughput in bytes/sec.
    pub net_tx_bps: u64,
    /// Free disk space on `/` in gigabytes (GB, base-10 / GiB).
    pub disk_free_gb: f32,
    /// Total disk capacity on `/` in gigabytes.
    pub disk_total_gb: f32,
}

#[derive(Default)]
struct PrevCounters {
    cpu_total: u64,
    cpu_idle: u64,
    net_rx: u64,
    net_tx: u64,
    sampled_at: Option<Instant>,
}

/// Stateful lightweight Linux telemetry sampler (zero external subprocesses).
pub struct TelemetrySampler {
    prev: PrevCounters,
}

impl TelemetrySampler {
    pub fn new() -> Self {
        let mut s = Self {
            prev: PrevCounters::default(),
        };
        let _ = s.sample();
        s
    }

    pub fn sample(&mut self) -> TelemetrySnapshot {
        let now = Instant::now();
        let dt_secs = self
            .prev
            .sampled_at
            .map(|t| now.duration_since(t).as_secs_f64().max(0.05))
            .unwrap_or(1.0);
        self.prev.sampled_at = Some(now);

        let (cpu_total, cpu_idle) = read_proc_stat_cpu().unwrap_or((0, 0));
        let cpu_percent = if self.prev.cpu_total > 0 && cpu_total > self.prev.cpu_total {
            let d_total = cpu_total - self.prev.cpu_total;
            let d_idle = cpu_idle.saturating_sub(self.prev.cpu_idle);
            let busy = d_total.saturating_sub(d_idle) as f64;
            ((busy / d_total as f64) * 100.0).clamp(0.0, 100.0) as f32
        } else {
            0.0
        };
        self.prev.cpu_total = cpu_total;
        self.prev.cpu_idle = cpu_idle;

        let cpu_temp_c = read_cpu_temp_celsius();
        let (mem_used_gb, mem_total_gb) = read_proc_meminfo_gb().unwrap_or((0.0, 0.0));

        let (net_rx, net_tx) = read_proc_net_dev_bytes().unwrap_or((0, 0));
        let (net_rx_bps, net_tx_bps) = if self.prev.net_rx > 0 || self.prev.net_tx > 0 {
            let drx = net_rx.saturating_sub(self.prev.net_rx) as f64;
            let dtx = net_tx.saturating_sub(self.prev.net_tx) as f64;
            ((drx / dt_secs).round() as u64, (dtx / dt_secs).round() as u64)
        } else {
            (0, 0)
        };
        self.prev.net_rx = net_rx;
        self.prev.net_tx = net_tx;

        let (disk_free_gb, disk_total_gb) = read_root_disk_gb().unwrap_or((0.0, 0.0));

        TelemetrySnapshot {
            cpu_percent,
            cpu_temp_c,
            mem_used_gb,
            mem_total_gb,
            net_rx_bps,
            net_tx_bps,
            disk_free_gb,
            disk_total_gb,
        }
    }
}

impl Default for TelemetrySampler {
    fn default() -> Self {
        Self::new()
    }
}

/// Formats bytes/sec into compact human-readable rate (`0 B/s`, `42 KB/s`, `3.4 MB/s`).
pub fn format_net_rate(bps: u64) -> String {
    if bps < 1024 {
        format!("{bps} B/s")
    } else if bps < 1024 * 1024 {
        format!("{} KB/s", bps / 1024)
    } else {
        format!("{:.1} MB/s", bps as f64 / (1024.0 * 1024.0))
    }
}

/// Manages a real `systemd-inhibit` sleep/idle lock with automatic parent-death cleanup.
pub struct KeepAwakeInhibitor {
    child: Mutex<Option<Child>>,
}

impl KeepAwakeInhibitor {
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
        }
    }

    /// Starts inhibiting system idle/sleep cleanly (replaces any prior inhibitor).
    pub fn start(&self, duration_secs: Option<u64>) {
        self.stop();
        let sleep_arg = duration_secs
            .map(|s| s.max(1).to_string())
            .unwrap_or_else(|| "infinity".to_string());
        let mut cmd = Command::new("systemd-inhibit");
        cmd.args([
            "--what=idle:sleep",
            "--who=NEON Dynamic Island",
            "--why=Keep Awake enabled by user",
            "--mode=block",
            "sleep",
            &sleep_arg,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        if let Ok(child) = cmd.spawn()
            && let Ok(mut guard) = self.child.lock()
        {
            *guard = Some(child);
        }
    }

    /// Releases the active sleep/idle inhibitor immediately.
    pub fn stop(&self) {
        if let Ok(mut guard) = self.child.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Default for KeepAwakeInhibitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for KeepAwakeInhibitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_proc_stat_cpu() -> Option<(u64, u64)> {
    let raw = fs::read_to_string("/proc/stat").ok()?;
    let line = raw.lines().next()?;
    let mut parts = line.split_whitespace();
    if parts.next()? != "cpu" {
        return None;
    }
    let vals: Vec<u64> = parts.filter_map(|p| p.parse::<u64>().ok()).collect();
    if vals.len() < 4 {
        return None;
    }
    let idle = vals[3].saturating_add(*vals.get(4).unwrap_or(&0));
    let total: u64 = vals.iter().copied().sum();
    Some((total, idle))
}

fn read_proc_meminfo_gb() -> Option<(f32, f32)> {
    let raw = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total_kb = 0u64;
    let mut avail_kb = 0u64;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail_kb = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        }
    }
    if total_kb == 0 {
        return None;
    }
    let used_kb = total_kb.saturating_sub(avail_kb);
    let kib_per_gib = 1024.0 * 1024.0;
    Some((
        (used_kb as f64 / kib_per_gib) as f32,
        (total_kb as f64 / kib_per_gib) as f32,
    ))
}

fn read_proc_net_dev_bytes() -> Option<(u64, u64)> {
    let raw = fs::read_to_string("/proc/net/dev").ok()?;
    let mut rx_total = 0u64;
    let mut tx_total = 0u64;
    for line in raw.lines().skip(2) {
        let Some((iface, stats)) = line.split_once(':') else {
            continue;
        };
        let name = iface.trim();
        if name == "lo" || name.starts_with("docker") || name.starts_with("veth") {
            continue;
        }
        let cols: Vec<u64> = stats
            .split_whitespace()
            .filter_map(|v| v.parse::<u64>().ok())
            .collect();
        if cols.len() >= 9 {
            rx_total = rx_total.saturating_add(cols[0]);
            tx_total = tx_total.saturating_add(cols[8]);
        }
    }
    Some((rx_total, tx_total))
}

fn read_cpu_temp_celsius() -> Option<f32> {
    let rd = fs::read_dir("/sys/class/thermal").ok()?;
    let mut best: Option<f32> = None;
    for entry in rd.flatten() {
        let p = entry.path();
        if !entry.file_name().to_string_lossy().starts_with("thermal_zone") {
            continue;
        }
        let kind = fs::read_to_string(p.join("type")).unwrap_or_default();
        let raw_temp = fs::read_to_string(p.join("temp")).ok()?;
        let milli: i64 = raw_temp.trim().parse().ok()?;
        if !(10_000..=125_000).contains(&milli) {
            continue;
        }
        let c = milli as f32 / 1000.0;
        let k = kind.trim().to_ascii_lowercase();
        if k.contains("x86_pkg") || k.contains("cpu") || k.contains("tctl") || k.contains("core") {
            return Some(c);
        }
        best = Some(best.map_or(c, |prev| prev.max(c)));
    }
    best
}

fn read_root_disk_gb() -> Option<(f32, f32)> {
    let path = CString::new("/").ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let frsize = if stat.f_frsize > 0 {
        stat.f_frsize as f64
    } else {
        stat.f_bsize as f64
    };
    let total_bytes = (stat.f_blocks as f64) * frsize;
    let free_bytes = (stat.f_bavail as f64) * frsize;
    let gb = 1024.0 * 1024.0 * 1024.0;
    Some(((free_bytes / gb) as f32, (total_bytes / gb) as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_reads_live_linux_memory_and_disk() {
        let mut sampler = TelemetrySampler::new();
        let snap = sampler.sample();
        assert!(snap.mem_total_gb > 0.1, "expected positive system RAM");
        assert!(snap.disk_total_gb > 0.1, "expected positive root disk capacity");
        assert!((0.0..=100.0).contains(&snap.cpu_percent));
    }

    #[test]
    fn format_net_rate_formats_bytes_kb_mb() {
        assert_eq!(format_net_rate(512), "512 B/s");
        assert_eq!(format_net_rate(4096), "4 KB/s");
        assert_eq!(format_net_rate(3 * 1024 * 1024), "3.0 MB/s");
    }
}
