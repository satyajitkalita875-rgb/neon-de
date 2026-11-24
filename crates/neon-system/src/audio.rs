//! PipeWire / WirePlumber audio integration (`wpctl` primary, `pactl` fallback).

use std::process::Command;

use serde::{Deserialize, Serialize};
use tracing::debug;

/// A single PipeWire audio sink (speaker / headphones / HDMI output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSink {
    pub id: u32,
    pub name: String,
    pub is_default: bool,
    pub volume: Option<f32>,
    pub muted: bool,
}

/// Snapshot of system audio state from PipeWire / WirePlumber.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStatus {
    pub available: bool,
    pub volume: f32,
    pub muted: bool,
    pub mic_volume: f32,
    pub mic_muted: bool,
    pub default_sink_name: Option<String>,
    pub sinks: Vec<AudioSink>,
}

impl Default for AudioStatus {
    fn default() -> Self {
        Self {
            available: false,
            volume: 0.0,
            muted: false,
            mic_volume: 0.0,
            mic_muted: false,
            default_sink_name: None,
            sinks: Vec::new(),
        }
    }
}

impl AudioStatus {
    /// Volume percentage rounded to integer (0–150).
    pub fn volume_percent(&self) -> u32 {
        (self.volume.clamp(0.0, 1.5) * 100.0).round() as u32
    }

    /// Compact label for the Top Panel status pill.
    pub fn panel_label(&self) -> String {
        if !self.available {
            return "Vol: --".to_string();
        }
        if self.muted {
            "Vol: Muted".to_string()
        } else {
            format!("Vol: {}%", self.volume_percent())
        }
    }
}

/// Queries live PipeWire audio status via `wpctl` (WirePlumber).
pub fn query_audio_status() -> AudioStatus {
    let Some((volume, muted)) = query_wpctl_volume("@DEFAULT_AUDIO_SINK@") else {
        return AudioStatus::default();
    };
    let (mic_volume, mic_muted) =
        query_wpctl_volume("@DEFAULT_AUDIO_SOURCE@").unwrap_or((1.0, false));
    let sinks = query_wpctl_sinks();
    let default_sink_name = sinks
        .iter()
        .find(|s| s.is_default)
        .or_else(|| sinks.first())
        .map(|s| s.name.clone());

    AudioStatus {
        available: true,
        volume,
        muted,
        mic_volume,
        mic_muted,
        default_sink_name,
        sinks,
    }
}

/// Sets the default PipeWire output volume (`0.0..=1.5`).
pub fn set_sink_volume(volume: f32) -> Result<(), String> {
    let clamped = volume.clamp(0.0, 1.5);
    let arg = format!("{clamped:.2}");
    run_wpctl(&["set-volume", "@DEFAULT_AUDIO_SINK@", &arg])?;
    debug!(volume = clamped, "Updated PipeWire default sink volume");
    Ok(())
}

/// Sets or clears mute on the default PipeWire audio sink.
pub fn set_sink_mute(muted: bool) -> Result<(), String> {
    let flag = if muted { "1" } else { "0" };
    run_wpctl(&["set-mute", "@DEFAULT_AUDIO_SINK@", flag])?;
    debug!(muted, "Updated PipeWire default sink mute state");
    Ok(())
}

/// Toggles mute on the default PipeWire audio sink.
pub fn toggle_sink_mute() -> Result<(), String> {
    run_wpctl(&["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])?;
    debug!("Toggled PipeWire default sink mute");
    Ok(())
}

/// Toggles mute on the default PipeWire microphone source.
pub fn toggle_source_mute() -> Result<(), String> {
    run_wpctl(&["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"])?;
    debug!("Toggled PipeWire default source mute");
    Ok(())
}

/// Switches the default PipeWire output sink by WirePlumber node ID.
pub fn set_default_sink(sink_id: u32) -> Result<(), String> {
    let id_str = sink_id.to_string();
    run_wpctl(&["set-default", &id_str])?;
    debug!(sink_id, "Switched PipeWire default audio sink");
    Ok(())
}

/// Sets the default PipeWire microphone source volume (`0.0..=1.5`).
pub fn set_source_volume(volume: f32) -> Result<(), String> {
    let clamped = volume.clamp(0.0, 1.5);
    let arg = format!("{clamped:.2}");
    run_wpctl(&["set-volume", "@DEFAULT_AUDIO_SOURCE@", &arg])?;
    Ok(())
}

/// Sets or clears mute on the default PipeWire microphone source.
pub fn set_source_mute(muted: bool) -> Result<(), String> {
    let flag = if muted { "1" } else { "0" };
    run_wpctl(&["set-mute", "@DEFAULT_AUDIO_SOURCE@", flag])?;
    Ok(())
}

/// Switches the default PipeWire audio input source by WirePlumber node ID.
pub fn set_default_source(source_id: u32) -> Result<(), String> {
    let id_str = source_id.to_string();
    run_wpctl(&["set-default", &id_str])?;
    Ok(())
}

/// Sets the volume of a specific PipeWire audio stream node ID (`0.0..=1.5`).
pub fn set_node_volume(node_id: u32, volume: f32) -> Result<(), String> {
    let id_str = node_id.to_string();
    let arg = format!("{:.2}", volume.clamp(0.0, 1.5));
    run_wpctl(&["set-volume", &id_str, &arg])?;
    Ok(())
}

/// Plays a real stereo speaker test sound via PipeWire / freedesktop sound assets.
pub fn test_speakers() -> Result<(), String> {
    for sound in [
        "/usr/share/sounds/freedesktop/stereo/bell.oga",
        "/usr/share/sounds/freedesktop/stereo/audio-volume-change.oga",
        "/usr/share/sounds/freedesktop/stereo/complete.oga",
    ] {
        if std::path::Path::new(sound).exists() {
            if Command::new("paplay").arg(sound).spawn().is_ok()
                || Command::new("pw-play").arg(sound).spawn().is_ok()
            {
                return Ok(());
            }
        }
    }
    Err("no test sound player (paplay / pw-play) or sound asset found".into())
}

/// Lists PipeWire microphone / input sources parsed from `wpctl status`.
pub fn query_audio_sources() -> Vec<AudioSink> {
    match run_wpctl(&["status"]) {
        Ok(stdout) => parse_wpctl_section(&stdout, "Sources:"),
        Err(_) => Vec::new(),
    }
}

/// Lists active per-application PipeWire playback streams parsed from `wpctl status`.
pub fn query_audio_streams() -> Vec<AudioSink> {
    match run_wpctl(&["status"]) {
        Ok(stdout) => parse_wpctl_section(&stdout, "Streams:"),
        Err(_) => Vec::new(),
    }
}

fn parse_wpctl_section(output: &str, header: &str) -> Vec<AudioSink> {
    let mut items = Vec::new();
    let mut in_audio = false;
    let mut in_section = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed == "Audio" {
            in_audio = true;
            in_section = false;
            continue;
        }
        if in_audio && (trimmed == "Video" || trimmed == "Settings") {
            break;
        }
        if !in_audio {
            continue;
        }
        if trimmed.contains(header) {
            in_section = true;
            continue;
        }
        if in_section
            && (trimmed.contains("Sinks:")
                || trimmed.contains("Sources:")
                || trimmed.contains("Filters:")
                || trimmed.contains("Streams:")
                || trimmed.contains("Devices:"))
        {
            in_section = false;
            continue;
        }
        if !in_section {
            continue;
        }
        let cleaned = trimmed.trim_start_matches(['│', '├', '└', '─', ' ']).trim();
        if cleaned.is_empty() {
            continue;
        }
        let (is_default, after_star) = if let Some(rest) = cleaned.strip_prefix('*') {
            (true, rest.trim_start())
        } else {
            (false, cleaned)
        };
        let Some((id_str, rest)) = after_star.split_once('.') else {
            continue;
        };
        let Ok(id) = id_str.trim().parse::<u32>() else {
            continue;
        };
        let (name_part, meta_part) = match rest.split_once("[vol:") {
            Some((n, m)) => (n.trim(), Some(m)),
            None => (rest.trim(), None),
        };
        let muted = meta_part.is_some_and(|m| m.contains("MUTED"));
        let volume = meta_part.and_then(|m| {
            m.trim_start()
                .split([' ', ']'])
                .next()
                .and_then(|v| v.parse::<f32>().ok())
        });
        if !name_part.is_empty() {
            items.push(AudioSink {
                id,
                name: name_part.to_string(),
                is_default,
                volume,
                muted,
            });
        }
    }
    items
}

fn run_wpctl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("wpctl")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run wpctl: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("wpctl {:?} failed: {}", args, stderr.trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn query_wpctl_volume(target: &str) -> Option<(f32, bool)> {
    let stdout = run_wpctl(&["get-volume", target]).ok()?;
    parse_wpctl_volume(&stdout)
}

fn query_wpctl_sinks() -> Vec<AudioSink> {
    match run_wpctl(&["status"]) {
        Ok(stdout) => parse_wpctl_sinks(&stdout),
        Err(_) => Vec::new(),
    }
}

/// Parses `wpctl get-volume` output like `Volume: 0.73` or `Volume: 0.45 [MUTED]`.
pub fn parse_wpctl_volume(output: &str) -> Option<(f32, bool)> {
    let line = output
        .lines()
        .find(|l| l.trim_start().starts_with("Volume:"))?;
    let rest = line.trim_start().strip_prefix("Volume:")?.trim();
    let mut parts = rest.split_whitespace();
    let vol: f32 = parts.next()?.parse().ok()?;
    let muted = rest.contains("[MUTED]");
    Some((vol.clamp(0.0, 1.5), muted))
}

/// Parses the `Audio -> Sinks:` section of `wpctl status`.
pub fn parse_wpctl_sinks(output: &str) -> Vec<AudioSink> {
    let mut sinks = Vec::new();
    let mut in_audio = false;
    let mut in_sinks = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed == "Audio" {
            in_audio = true;
            in_sinks = false;
            continue;
        }
        if in_audio && (trimmed == "Video" || trimmed == "Settings") {
            break;
        }
        if !in_audio {
            continue;
        }
        if trimmed.contains("Sinks:") {
            in_sinks = true;
            continue;
        }
        if in_sinks
            && (trimmed.contains("Sources:")
                || trimmed.contains("Filters:")
                || trimmed.contains("Streams:"))
        {
            in_sinks = false;
            continue;
        }
        if !in_sinks {
            continue;
        }

        // Strip tree drawing prefixes like `│  *   56. Built-in Audio Analog Stereo [vol: 0.73]`
        let cleaned = trimmed.trim_start_matches(['│', '├', '└', '─', ' ']).trim();
        if cleaned.is_empty() {
            continue;
        }

        let (is_default, after_star) = if let Some(rest) = cleaned.strip_prefix('*') {
            (true, rest.trim_start())
        } else {
            (false, cleaned)
        };

        let Some((id_str, rest)) = after_star.split_once('.') else {
            continue;
        };
        let Ok(id) = id_str.trim().parse::<u32>() else {
            continue;
        };

        let (name_part, meta_part) = match rest.split_once("[vol:") {
            Some((n, m)) => (n.trim(), Some(m)),
            None => (rest.trim(), None),
        };
        let muted = meta_part.is_some_and(|m| m.contains("MUTED"));
        let volume = meta_part.and_then(|m| {
            m.trim_start()
                .split([' ', ']'])
                .next()
                .and_then(|v| v.parse::<f32>().ok())
        });

        if !name_part.is_empty() {
            sinks.push(AudioSink {
                id,
                name: name_part.to_string(),
                is_default,
                volume,
                muted,
            });
        }
    }

    sinks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl_volume_and_mute_state() {
        assert_eq!(parse_wpctl_volume("Volume: 0.73\n"), Some((0.73, false)));
        assert_eq!(
            parse_wpctl_volume("Volume: 0.42 [MUTED]\n"),
            Some((0.42, true))
        );
        assert_eq!(parse_wpctl_volume("garbage"), None);
    }

    #[test]
    fn parses_wpctl_status_sinks_section() {
        let sample = "\
Audio
 ├─ Devices:
 │      49. Built-in Audio                      [alsa]
 │  
 ├─ Sinks:
 │  *   56. Built-in Audio Analog Stereo        [vol: 0.73]
 │      61. HDMI / DisplayPort                  [vol: 0.50 MUTED]
 │  
 ├─ Sources:
 │  *   57. Internal Microphone                 [vol: 1.00]
Video
";
        let sinks = parse_wpctl_sinks(sample);
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].id, 56);
        assert!(sinks[0].is_default);
        assert_eq!(sinks[0].name, "Built-in Audio Analog Stereo");
        assert_eq!(sinks[0].volume, Some(0.73));
        assert!(!sinks[0].muted);

        assert_eq!(sinks[1].id, 61);
        assert!(!sinks[1].is_default);
        assert!(sinks[1].muted);
    }
}
