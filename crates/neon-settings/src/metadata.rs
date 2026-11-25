//! Typed setting metadata registry and global search engine covering only genuine,
//! working NEON DE settings across all 12 categories.

/// Metadata descriptor for a single searchable setting entry.
#[derive(Debug, Clone, Copy)]
pub struct SettingMetadata {
    #[allow(dead_code)]
    pub id: &'static str,
    pub category_id: &'static str,
    pub category_title: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub keywords: &'static str,
}

/// Ranked global search hit returned by [`search_settings`].
#[derive(Debug, Clone, Copy)]
pub struct SearchHit {
    pub setting: &'static SettingMetadata,
    pub score: u32,
}

/// Master registry of real, working settings across all 12 NEON DE categories.
pub const SETTINGS_REGISTRY: &[SettingMetadata] = &[
    // Network
    SettingMetadata {
        id: "network.wifi",
        category_id: "network",
        category_title: "Wi-Fi & Network",
        name: "Wi-Fi Radio & Available Networks",
        description: "Enable Wi-Fi, scan nearby access points, and connect securely.",
        keywords: "wifi wi-fi wireless ssid connect password disconnect networkmanager",
    },
    SettingMetadata {
        id: "network.saved",
        category_id: "network",
        category_title: "Wi-Fi & Network",
        name: "Active IP, DNS & Saved Connections",
        description: "Inspect IPv4 address, gateway, DNS servers, and manage saved profiles.",
        keywords: "ip ipv4 dns gateway ethernet vpn saved connections forget",
    },
    // Bluetooth
    SettingMetadata {
        id: "bluetooth.devices",
        category_id: "bluetooth",
        category_title: "Bluetooth",
        name: "Bluetooth Power, Discovery & Devices",
        description: "Toggle Bluetooth, scan nearby peripherals, pair, connect, or remove.",
        keywords: "bluetooth pair scan connect headphones speaker mouse keyboard bluez",
    },
    // Appearance
    SettingMetadata {
        id: "appearance.theme",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Theme & Accent Color",
        description: "Switch between Dark, Light, and Auto mode and pick an accent color.",
        keywords: "theme dark light auto accent color blue cyan violet emerald rose pink",
    },
    SettingMetadata {
        id: "appearance.wallpaper",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Desktop Wallpaper",
        description: "Choose a local wallpaper or pick any custom image file.",
        keywords: "wallpaper background image picture desktop",
    },
    SettingMetadata {
        id: "appearance.blur",
        category_id: "appearance",
        category_title: "Appearance",
        name: "System Blur, Corner Radius & Shadows",
        description: "Single unified slider controlling blur across Top Bar, Dock, Quick Settings, Workspaces, App Drawer, and glass panels, plus corner rounding and shadows.",
        keywords: "system blur glass transparency corner radius shadow high contrast top bar dock quick settings app drawer",
    },
    SettingMetadata {
        id: "appearance.cursor",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Cursor Theme & Pointer Size",
        description: "Choose mouse cursor theme and logical pixel size.",
        keywords: "cursor pointer mouse size macos bibata theme",
    },
    SettingMetadata {
        id: "appearance.toolkit",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Icon Theme, GTK Theme & Fonts",
        description: "Configure desktop icons, GTK theme, system font, size, and text scaling.",
        keywords: "gtk icon theme whitesur papirus adwaita font typography scale text",
    },
    // Desktop & Dock
    SettingMetadata {
        id: "dock.shell",
        category_id: "dock",
        category_title: "Desktop & Dock",
        name: "Dock & Top Bar",
        description: "Control dock opacity, top bar visibility, opacity, clock, and workspaces button.",
        keywords: "dock top bar panel transparency opacity clock workspaces menu bar",
    },
    SettingMetadata {
        id: "dock.effects",
        category_id: "dock",
        category_title: "Desktop & Dock",
        name: "Animations & Effects Preset",
        description: "Switch Performance/Balanced/Visual presets, reduced motion, speed, and borders.",
        keywords: "animation effects preset performance balanced visual reduced motion speed borders opacity",
    },
    SettingMetadata {
        id: "dock.windows",
        category_id: "dock",
        category_title: "Desktop & Dock",
        name: "Window Focus, Snap Gap & Titlebar Buttons",
        description: "Configure focus follows mouse, snap gap, inactive opacity, and button side.",
        keywords: "window focus mouse snap gap titlebar traffic lights buttons macos",
    },
    // Displays
    SettingMetadata {
        id: "displays.outputs",
        category_id: "displays",
        category_title: "Displays",
        name: "Monitor Layout, Scaling & Rotation",
        description: "Configure resolution, fractional scaling, orientation, and primary display.",
        keywords: "monitor display screen resolution refresh scale hidpi rotation orientation primary",
    },
    SettingMetadata {
        id: "displays.brightness",
        category_id: "displays",
        category_title: "Displays",
        name: "Display Brightness",
        description: "Adjust internal screen backlight brightness via systemd-logind.",
        keywords: "brightness backlight screen dim display",
    },
    // Sound
    SettingMetadata {
        id: "sound.output",
        category_id: "sound",
        category_title: "Sound",
        name: "Output Device, Volume & Speaker Test",
        description: "Select PipeWire playback device, volume, mute, and play a stereo test chime.",
        keywords: "sound audio volume speakers headphones mute test pipewire",
    },
    SettingMetadata {
        id: "sound.input",
        category_id: "sound",
        category_title: "Sound",
        name: "Microphone & Per-App Volume",
        description: "Choose microphone source, input level, mute, and active app audio streams.",
        keywords: "microphone mic input recording streams app volume wireplumber",
    },
    // Battery & Power
    SettingMetadata {
        id: "power.battery",
        category_id: "power",
        category_title: "Battery & Power",
        name: "Battery Status & Power Mode",
        description: "View battery level and health, and switch Power Saver, Balanced, or Performance.",
        keywords: "battery health charge power mode profile saver balanced performance upower",
    },
    SettingMetadata {
        id: "power.session",
        category_id: "power",
        category_title: "Battery & Power",
        name: "Lock, Suspend, Log Out, Restart & Shut Down",
        description: "Control the active desktop session and system power state.",
        keywords: "lock screen suspend sleep logout restart reboot shutdown poweroff",
    },
    // Storage & Printers
    SettingMetadata {
        id: "storage.disks",
        category_id: "storage",
        category_title: "Storage & Printers",
        name: "Mounted Volumes, USB Eject & Printers",
        description: "Inspect disk usage, safely eject removable drives, and view CUPS printers.",
        keywords: "storage disk drive partition usb eject printer cups",
    },
    // Applications
    SettingMetadata {
        id: "applications.defaults",
        category_id: "applications",
        category_title: "Applications",
        name: "Default Applications",
        description: "Set default browser, email client, file manager, terminal, and media player.",
        keywords: "default apps browser email file manager terminal media player xdg mime",
    },
    SettingMetadata {
        id: "applications.login_items",
        category_id: "applications",
        category_title: "Applications",
        name: "Login Items & Installed Applications",
        description: "Enable or disable autostart entries and browse installed desktop apps.",
        keywords: "startup autostart login items installed applications",
    },
    // Keyboard Shortcuts
    SettingMetadata {
        id: "shortcuts.reference",
        category_id: "shortcuts",
        category_title: "Keyboard Shortcuts",
        name: "Desktop Keyboard Shortcuts",
        description: "Reference of active NEON DE compositor shortcuts.",
        keywords: "shortcuts keybindings hotkeys super alt tab screenshot workspace terminal lock",
    },
    // Date & Time
    SettingMetadata {
        id: "datetime.ntp",
        category_id: "datetime",
        category_title: "Date & Time",
        name: "Automatic Time (NTP), Timezone & Locale",
        description: "Toggle NTP clock sync, change IANA timezone, and inspect system locale.",
        keywords: "date time clock ntp timezone locale language timedated",
    },
    // About
    SettingMetadata {
        id: "about.system",
        category_id: "about",
        category_title: "About NEON DE",
        name: "Hardware Overview, Telemetry & Config Backup",
        description: "Inspect CPU, GPU, RAM, compositor telemetry, export/import config, or reset.",
        keywords: "about hardware cpu gpu memory telemetry diagnostics backup export import reset",
    },
];

/// Performs ranked case-insensitive multi-token search across all real settings.
pub fn search_settings(query: &str) -> Vec<SearchHit> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let mut hits = Vec::new();

    for setting in SETTINGS_REGISTRY {
        let name_l = setting.name.to_ascii_lowercase();
        let desc_l = setting.description.to_ascii_lowercase();
        let cat_l = setting.category_title.to_ascii_lowercase();
        let kw_l = setting.keywords.to_ascii_lowercase();

        let mut score: u32 = 0;
        let mut all_matched = true;
        for tok in &tokens {
            let mut tok_score = 0;
            if name_l.contains(tok) {
                tok_score += if name_l.starts_with(tok) { 120 } else { 80 };
            }
            if cat_l.contains(tok) {
                tok_score += 50;
            }
            if kw_l.contains(tok) {
                tok_score += 40;
            }
            if desc_l.contains(tok) {
                tok_score += 20;
            }
            if tok_score == 0 {
                all_matched = false;
                break;
            }
            score += tok_score;
        }
        if all_matched && score > 0 {
            hits.push(SearchHit { setting, score });
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.setting.name.cmp(b.setting.name))
    });
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ranks_direct_matches_first() {
        let hits = search_settings("wifi");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].setting.category_id, "network");

        let blur_hits = search_settings("blur");
        assert!(!blur_hits.is_empty());
        assert_eq!(blur_hits[0].setting.category_id, "appearance");

        let bright_hits = search_settings("brightness");
        assert!(!bright_hits.is_empty());
        assert_eq!(bright_hits[0].setting.category_id, "displays");
    }
}
