//! Typed setting metadata registry, honest capability states, and global search engine
//! across all 22 NEON DE Settings categories.

/// Honest runtime capability state exposed by NEON DE Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityStatus {
    /// Fully backed by live compositor IPC, XDG config, or native Linux system services.
    Implemented,
    /// Config is persisted and partially live; full runtime hook depends on compositor/service support.
    PartiallyImplemented,
    /// Action requires PolicyKit / administrator authorization on the host.
    RequiresElevation,
    /// Underlying daemon/hardware is temporarily unavailable or offline.
    TemporarilyUnavailable,
    /// Feature is not supported by the current renderer/compositor/system backend.
    Unsupported(&'static str),
}

impl CapabilityStatus {
    pub fn badge_label(&self) -> &'static str {
        match self {
            Self::Implemented => "Active",
            Self::PartiallyImplemented => "Partial",
            Self::RequiresElevation => "Admin",
            Self::TemporarilyUnavailable => "Unavailable",
            Self::Unsupported(_) => "Unsupported",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Implemented => "ok",
            Self::PartiallyImplemented => "partial",
            Self::RequiresElevation => "elevated",
            Self::TemporarilyUnavailable => "unavailable",
            Self::Unsupported(_) => "unsupported",
        }
    }
}

/// Metadata descriptor for a single searchable setting entry.
#[derive(Debug, Clone, Copy)]
pub struct SettingMetadata {
    pub id: &'static str,
    pub category_id: &'static str,
    pub category_title: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub keywords: &'static str,
    pub service: &'static str,
    pub capability: CapabilityStatus,
}

/// Ranked global search hit returned by [`search_settings`].
#[derive(Debug, Clone, Copy)]
pub struct SearchHit {
    pub setting: &'static SettingMetadata,
    pub score: u32,
}

/// Master registry of individual searchable settings across all 22 NEON DE categories.
pub const SETTINGS_REGISTRY: &[SettingMetadata] = &[
    // 5.1 Appearance
    SettingMetadata {
        id: "appearance.theme",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Theme Mode (Light / Dark / Automatic)",
        description: "Switch between Neon Light, Neon Dark, and Automatic appearance.",
        keywords: "light dark auto theme mode style color scheme",
        service: "NEON IPC & config",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "appearance.accent",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Accent Color",
        description: "Choose a curated highlight color or custom hex accent across the desktop.",
        keywords: "accent color blue cyan violet emerald rose pink hex highlight",
        service: "NEON IPC & config",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "appearance.wallpaper",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Wallpaper, Scaling & Slideshow",
        description: "Pick desktop wallpaper, scaling mode, and slideshow interval.",
        keywords: "wallpaper background image picture slideshow cover fit stretch",
        service: "NEON IPC & shell",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "appearance.blur",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Reduce Blur & Blur Intensity",
        description: "Dial back or disable background glass blur for faster GPU performance.",
        keywords: "blur transparency glass lag performance gpu",
        service: "NEON IPC & renderer",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "appearance.icons_gtk",
        category_id: "appearance",
        category_title: "Appearance",
        name: "GTK Theme & Icon Theme",
        description: "Select GTK toolkit theme and desktop icon pack.",
        keywords: "gtk theme icon pack whitesur papirus adwaita",
        service: "GTK / gsettings",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "appearance.cursor",
        category_id: "appearance",
        category_title: "Appearance",
        name: "Cursor Theme & Pointer Size",
        description: "Set mouse pointer theme (macOS, Bibata, Adwaita) and pixel size.",
        keywords: "cursor pointer mouse size macos bibata theme",
        service: "NEON IPC & gsettings",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "appearance.fonts",
        category_id: "appearance",
        category_title: "Appearance",
        name: "System Fonts, Scaling & Interface Density",
        description: "Configure base font family, point size, scaling factor, and widget density.",
        keywords: "font typography size scale density compact spacious",
        service: "GTK & config",
        capability: CapabilityStatus::Implemented,
    },
    // 5.2 Displays
    SettingMetadata {
        id: "displays.outputs",
        category_id: "displays",
        category_title: "Displays",
        name: "Monitor Detection, Arrangement & Resolution",
        description: "Inspect connected monitors, spatial layout, resolution, and refresh rate.",
        keywords: "monitor display screen arrangement resolution refresh hz edid",
        service: "NEON IPC (DRM/KMS)",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "displays.scale_orient",
        category_id: "displays",
        category_title: "Displays",
        name: "Display Scaling, Rotation & Primary Monitor",
        description: "Apply fractional scaling, orientation, and primary display with revert timer.",
        keywords: "fractional scale hidpi dpi rotation orientation primary revert countdown",
        service: "NEON IPC (ConfigureOutput)",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "displays.night_light",
        category_id: "displays",
        category_title: "Displays",
        name: "Night Light & Color Temperature",
        description: "Warm display color temperature at night (Kelvin slider).",
        keywords: "night light blue light color temperature kelvin warm redshift",
        service: "DRM Gamma LUT",
        capability: CapabilityStatus::PartiallyImplemented,
    },
    SettingMetadata {
        id: "displays.hdr",
        category_id: "displays",
        category_title: "Displays",
        name: "HDR Output",
        description: "High Dynamic Range output metadata support.",
        keywords: "hdr high dynamic range 10bit wide color",
        service: "DRM/KMS renderer",
        capability: CapabilityStatus::Unsupported("DRM HDR color management protocol is not yet exposed by neon-renderer."),
    },
    // 5.3 Desktop & Workspaces
    SettingMetadata {
        id: "desktop.workspaces",
        category_id: "desktop",
        category_title: "Desktop & Workspaces",
        name: "Dynamic Workspaces, Count & Names",
        description: "Switch between dynamic and fixed workspaces and customize workspace names.",
        keywords: "workspace virtual desktop dynamic static count names grid",
        service: "NEON WM & config",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "desktop.overview_corners",
        category_id: "desktop",
        category_title: "Desktop & Workspaces",
        name: "Overview, Hot Corners & Desktop Interaction",
        description: "Configure Mission Control overview, hot corner triggers, and right-click menu.",
        keywords: "overview mission control hot corners desktop icons right click context menu",
        service: "NEON shell & config",
        capability: CapabilityStatus::Implemented,
    },
    // 5.4 Dock & Panels
    SettingMetadata {
        id: "dock.behavior",
        category_id: "dock",
        category_title: "Dock & Panels",
        name: "Dock Position, Size, Auto-Hide & Style",
        description: "Customize dock edge, icon size, auto-hide, magnification, transparency, and icons.",
        keywords: "dock position bottom left right icon size autohide intelligent hide magnification trash recent",
        service: "NEON IPC & shell",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "dock.panel",
        category_id: "dock",
        category_title: "Dock & Panels",
        name: "Top Panel, Clock, System Tray & Multi-Panel Layout",
        description: "Configure panel edge, height, transparency, clock format, tray, and extra panels.",
        keywords: "panel topbar menu bar clock date seconds tray notifications quick settings multi panel",
        service: "NEON IPC & shell",
        capability: CapabilityStatus::Implemented,
    },
    // 5.5 Windows & Window Management
    SettingMetadata {
        id: "windows.focus_snap",
        category_id: "windows",
        category_title: "Windows & Window Management",
        name: "Focus Policy, Window Placement & Snapping",
        description: "Configure Click-to-Focus, Focus-Follows-Mouse, snapping, tiling gap, and placement.",
        keywords: "window focus mouse raise placement smart cascade snap tiling gap",
        service: "NEON IPC (UpdateWindowBehavior)",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "windows.titlebar",
        category_id: "windows",
        category_title: "Windows & Window Management",
        name: "Titlebar Buttons, Decorations & Double-Click Action",
        description: "Choose macOS left traffic lights or right controls, SSD decorations, and titlebar action.",
        keywords: "titlebar close minimize maximize traffic lights macos decorations ssd",
        service: "GTK / gsettings & compositor",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "windows.rules",
        category_id: "windows",
        category_title: "Windows & Window Management",
        name: "Per-Application Window Rules",
        description: "Assign workspace, preferred monitor, maximized state, always-on-top, or size bounds per app.",
        keywords: "window rules per app always on top maximized workspace monitor min max size",
        service: "NEON WM config",
        capability: CapabilityStatus::PartiallyImplemented,
    },
    // 5.6 Effects & Animations
    SettingMetadata {
        id: "effects.presets",
        category_id: "effects",
        category_title: "Effects & Animations",
        name: "Performance Presets (Performance / Balanced / Visual Quality)",
        description: "One-click compositor effect profiles balancing smoothness and visual richness.",
        keywords: "effects preset performance balanced visual quality gpu speed",
        service: "NEON IPC (UpdateEffects)",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "effects.animations",
        category_id: "effects",
        category_title: "Effects & Animations",
        name: "Window & Workspace Animations, Shadows, Corners & Blur",
        description: "Control open/close/minimize/maximize animations, shadows, corner radius, and speed.",
        keywords: "animation open close minimize maximize workspace dock shadow corner radius opacity speed reduced motion",
        service: "NEON IPC & renderer",
        capability: CapabilityStatus::Implemented,
    },
    // 5.7 Input Devices
    SettingMetadata {
        id: "input.mouse",
        category_id: "input",
        category_title: "Input Devices",
        name: "Mouse Speed, Acceleration, Left-Handed & Scrolling",
        description: "Configure libinput mouse pointer speed, acceleration profile, natural scroll, and buttons.",
        keywords: "mouse pointer speed acceleration flat adaptive left handed scroll double click",
        service: "libinput & config",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "input.touchpad",
        category_id: "input",
        category_title: "Input Devices",
        name: "Touchpad Tap-to-Click, Two-Finger Scroll & Gestures",
        description: "Configure touchpad enablement, tap-to-click, click method, palm rejection, and gestures.",
        keywords: "touchpad trackpad tap click two finger scroll clickfinger disable typing gestures three four finger",
        service: "libinput & config",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "input.keyboard",
        category_id: "input",
        category_title: "Input Devices",
        name: "Keyboard Layouts, Repeat Rate, Compose & Caps Lock",
        description: "Manage XKB layouts, repeat rate/delay, Compose key, and Caps Lock mapping.",
        keywords: "keyboard layout xkb repeat rate delay compose caps lock input source",
        service: "XKB & config",
        capability: CapabilityStatus::Implemented,
    },
    // 5.8 Network
    SettingMetadata {
        id: "network.wifi",
        category_id: "network",
        category_title: "Network",
        name: "Wi-Fi Networks, Connect & Saved Profiles",
        description: "Toggle Wi-Fi radio, scan access points, connect securely via NetworkManager, and manage profiles.",
        keywords: "wifi wi-fi wireless network ssid password connect disconnect saved metered",
        service: "NetworkManager D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "network.details",
        category_id: "network",
        category_title: "Network",
        name: "Ethernet, VPN, IP Address, DNS & Proxy",
        description: "Inspect Ethernet status, IPv4 address, gateway, DNS servers, VPNs, and metered settings.",
        keywords: "ethernet lan vpn ip address dns gateway mac proxy hotspot",
        service: "NetworkManager D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    // 5.9 Bluetooth
    SettingMetadata {
        id: "bluetooth.devices",
        category_id: "bluetooth",
        category_title: "Bluetooth",
        name: "Bluetooth Adapter, Discovery, Pairing & Devices",
        description: "Enable Bluetooth, scan for nearby devices, pair, connect, disconnect, or remove devices.",
        keywords: "bluetooth bluez wireless pair scan discovery headphones speaker adapter",
        service: "BlueZ D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    // 5.10 Sound
    SettingMetadata {
        id: "sound.output",
        category_id: "sound",
        category_title: "Sound",
        name: "Output Devices, Master Volume & Speaker Test",
        description: "Select output sink, adjust volume or mute, and run a speaker audio test.",
        keywords: "sound audio output speaker headphone volume mute test pipewire wireplumber",
        service: "PipeWire / WirePlumber",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "sound.input_apps",
        category_id: "sound",
        category_title: "Sound",
        name: "Microphone Input & Per-Application Volume",
        description: "Choose default microphone, set input level, and control volume per running audio app.",
        keywords: "microphone input sensitivity recording per app stream volume balance profile",
        service: "PipeWire / WirePlumber",
        capability: CapabilityStatus::Implemented,
    },
    // 5.11 Notifications
    SettingMetadata {
        id: "notifications.prefs",
        category_id: "notifications",
        category_title: "Notifications",
        name: "Do Not Disturb, Previews, Position & Duration",
        description: "Pause alerts, adjust banner position, duration, sounds, lock-screen previews, and muted apps.",
        keywords: "notifications do not disturb dnd alerts banners previews duration position sound history",
        service: "Freedesktop Notifications D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    // 5.12 Power & Battery
    SettingMetadata {
        id: "power.battery",
        category_id: "power",
        category_title: "Power & Battery",
        name: "Battery Status, Health & Power Modes",
        description: "View battery percentage, charging status, health capacity, and select power profile.",
        keywords: "battery charge health percentage power mode saver balanced performance upower",
        service: "UPower & PowerProfiles D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "power.timeouts",
        category_id: "power",
        category_title: "Power & Battery",
        name: "Screen Timeout, Suspend, Lid & Power Button",
        description: "Configure screen dimming, blanking, automatic suspend, lid-close, and power-button actions.",
        keywords: "screen dim blank timeout sleep suspend lid close power button",
        service: "NEON session & logind",
        capability: CapabilityStatus::Implemented,
    },
    // 5.13 Privacy & Security
    SettingMetadata {
        id: "privacy.lock",
        category_id: "privacy",
        category_title: "Privacy & Security",
        name: "Screen Lock, Auto-Lock & Polkit Security",
        description: "Lock session immediately, set auto-lock timeout, and inspect IPC/Polkit/Portal status.",
        keywords: "privacy security lock screen password polkit peercred portal camera microphone location",
        service: "ext_session_lock_v1 & Polkit",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "privacy.history",
        category_id: "privacy",
        category_title: "Privacy & Security",
        name: "Recent File History & Usage Privacy",
        description: "Toggle recent file tracking or clear recent file history cleanly.",
        keywords: "recent files history clear usage tracking privacy",
        service: "XDG recently-used.xbel",
        capability: CapabilityStatus::Implemented,
    },
    // 5.14 Applications & Default Apps
    SettingMetadata {
        id: "applications.defaults",
        category_id: "applications",
        category_title: "Applications & Default Apps",
        name: "Default Web Browser, Email, Files, Terminal & Media",
        description: "Set system-wide XDG MIME default applications for links, mail, folders, and media.",
        keywords: "default apps browser email mail file manager terminal media player mime xdg",
        service: "XDG MIME (xdg-mime)",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "applications.installed",
        category_id: "applications",
        category_title: "Applications & Default Apps",
        name: "Installed Applications & Startup Behavior",
        description: "Browse installed XDG desktop applications, view details, or add to autostart.",
        keywords: "installed applications desktop entries list details autostart",
        service: "XDG Desktop Entry",
        capability: CapabilityStatus::Implemented,
    },
    // 5.15 Users & Accounts
    SettingMetadata {
        id: "users.accounts",
        category_id: "users",
        category_title: "Users & Accounts",
        name: "Current User Profile, Display Name & Local Accounts",
        description: "Inspect user identity, account type (Administrator/Standard), and edit display name.",
        keywords: "user account profile avatar name username administrator sudo password accountsservice",
        service: "AccountsService D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    // 5.16 Keyboard Shortcuts
    SettingMetadata {
        id: "shortcuts.manager",
        category_id: "shortcuts",
        category_title: "Keyboard Shortcuts",
        name: "Shortcut Manager, Custom Bindings & Conflict Detection",
        description: "Search, edit, add custom shortcuts, detect conflicts, and reset defaults.",
        keywords: "shortcuts keybindings hotkeys custom super alt conflict reset",
        service: "NEON WM & config",
        capability: CapabilityStatus::Implemented,
    },
    // 5.17 Accessibility
    SettingMetadata {
        id: "accessibility.vision",
        category_id: "accessibility",
        category_title: "Accessibility",
        name: "High Contrast, Reduce Motion, Text Scale & Large Pointer",
        description: "Improve visual legibility and pointer visibility across NEON DE.",
        keywords: "accessibility high contrast reduce motion text scale large pointer vision",
        service: "NEON IPC & GTK",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "accessibility.typing",
        category_id: "accessibility",
        category_title: "Accessibility",
        name: "Sticky Keys, Slow Keys, Bounce Keys & Assistive Tools",
        description: "Configure typing aids and check Screen Reader / On-Screen Keyboard availability.",
        keywords: "sticky keys slow keys bounce keys mouse keys screen reader orca magnifier osk",
        service: "XKB / Assistive services",
        capability: CapabilityStatus::PartiallyImplemented,
    },
    // 5.18 Startup & Session
    SettingMetadata {
        id: "startup.autostart",
        category_id: "startup",
        category_title: "Startup & Session",
        name: "Autostart Applications",
        description: "Enable or disable XDG autostart programs launched at login.",
        keywords: "startup autostart login applications boot xdg",
        service: "XDG Autostart (~/.config/autostart)",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "startup.power_session",
        category_id: "startup",
        category_title: "Startup & Session",
        name: "Session Restore, Logout, Suspend, Hibernate, Restart & Shutdown",
        description: "Manage session restore behavior and execute confirmed systemd-logind power actions.",
        keywords: "session restore logout suspend hibernate restart reboot shutdown poweroff logind",
        service: "systemd-logind D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    // 5.19 Date, Time & Language
    SettingMetadata {
        id: "datetime.clock",
        category_id: "datetime",
        category_title: "Date, Time & Language",
        name: "Automatic Time (NTP), Timezone & Clock Format",
        description: "Configure systemd-timedated NTP sync, system timezone, and 12h/24h format.",
        keywords: "date time timezone clock ntp automatic 24h 12h timedated",
        service: "systemd-timedated D-Bus",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "datetime.locale",
        category_id: "datetime",
        category_title: "Date, Time & Language",
        name: "System Language & Regional Formats",
        description: "Inspect active system locale, regional formatting, and installed locales.",
        keywords: "language locale region formats utf8 localectl",
        service: "systemd-localed",
        capability: CapabilityStatus::Implemented,
    },
    // 5.20 Storage & Devices
    SettingMetadata {
        id: "storage.disks",
        category_id: "storage",
        category_title: "Storage & Devices",
        name: "Disks, Partitions, Mount Points & Removable Media Eject",
        description: "Inspect storage capacity, mounted volumes, auto-mount preferences, and safely eject drives.",
        keywords: "storage disk drive partition mount usb removable eject udisks2 df lsblk",
        service: "UDisks2 / lsblk",
        capability: CapabilityStatus::Implemented,
    },
    SettingMetadata {
        id: "storage.printers",
        category_id: "storage",
        category_title: "Storage & Devices",
        name: "Printers & Scanners",
        description: "View configured CUPS printers and SANE scanner availability.",
        keywords: "printer scanner cups lpstat sane peripheral device",
        service: "CUPS / SANE",
        capability: CapabilityStatus::Implemented,
    },
    // 5.21 Updates & Software
    SettingMetadata {
        id: "updates.packages",
        category_id: "updates",
        category_title: "Updates & Software",
        name: "System Updates, Repositories & Package Manager Integration",
        description: "Check available APT updates, inspect configured sources, and open the host updater.",
        keywords: "updates software upgrade packages apt repositories sources security",
        service: "APT / dpkg",
        capability: CapabilityStatus::Implemented,
    },
    // 5.22 About NEON DE
    SettingMetadata {
        id: "about.system",
        category_id: "about",
        category_title: "About NEON DE",
        name: "NEON DE Version, Hardware Info, Diagnostics & Backup",
        description: "View OS, kernel, CPU, GPU, memory, compositor telemetry, export/import config, or reset.",
        keywords: "about neon de version kernel cpu gpu ram memory diagnostics backup export import reset defaults",
        service: "NEON IPC & /proc",
        capability: CapabilityStatus::Implemented,
    },
];

/// Searches the metadata registry across name, category, description, keywords, and service.
pub fn search_settings(query: &str) -> Vec<SearchHit> {
    let trimmed = query.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let mut hits = Vec::new();

    for setting in SETTINGS_REGISTRY {
        let name = setting.name.to_ascii_lowercase();
        let cat = setting.category_title.to_ascii_lowercase();
        let desc = setting.description.to_ascii_lowercase();
        let kw = setting.keywords.to_ascii_lowercase();
        let svc = setting.service.to_ascii_lowercase();

        let mut score = 0u32;
        let mut all_matched = true;
        for token in &tokens {
            let mut token_score = 0u32;
            if name.contains(token) {
                token_score += if name.starts_with(token) { 40 } else { 25 };
            }
            if cat.contains(token) {
                token_score += 20;
            }
            if kw.contains(token) {
                token_score += 15;
            }
            if desc.contains(token) {
                token_score += 10;
            }
            if svc.contains(token) {
                token_score += 8;
            }
            if token_score == 0 {
                all_matched = false;
                break;
            }
            score += token_score;
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
    use crate::navigation::find_category;

    #[test]
    fn every_setting_references_a_valid_category() {
        for item in SETTINGS_REGISTRY {
            assert!(
                find_category(item.category_id).is_some(),
                "Unknown category_id {} in setting {}",
                item.category_id,
                item.id
            );
        }
    }

    #[test]
    fn global_search_matches_names_keywords_and_services() {
        let hits = search_settings("blur");
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|h| h.setting.category_id == "appearance"));

        let hits_nm = search_settings("networkmanager");
        assert!(hits_nm.iter().any(|h| h.setting.category_id == "network"));

        let hits_multi = search_settings("shortcut conflict");
        assert_eq!(hits_multi[0].setting.category_id, "shortcuts");
    }

    #[test]
    fn empty_query_returns_no_hits() {
        assert!(search_settings("   ").is_empty());
    }
}
