//! Category definitions (all 22 NEON DE Settings sections) and Back-button navigation history.

/// Sidebar category descriptor.
#[derive(Debug, Clone, Copy)]
pub struct CategoryDef {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub group: u8,
    pub keywords: &'static str,
}

/// Main scrollable sidebar categories (21 categories; `about` is pinned at the bottom of the sidebar).
pub const SIDEBAR_CATEGORIES: &[CategoryDef] = &[
    // Group 0: Connectivity
    CategoryDef {
        id: "network",
        title: "Network",
        description: "Manage Wi-Fi, Ethernet, VPN, DNS, and NetworkManager connections.",
        icon: "network-wireless-symbolic",
        group: 0,
        keywords: "wifi wi-fi ethernet internet ip dns vpn proxy hotspot networkmanager",
    },
    CategoryDef {
        id: "bluetooth",
        title: "Bluetooth",
        description: "Discover, pair, and manage wireless Bluetooth devices via BlueZ.",
        icon: "bluetooth-active-symbolic",
        group: 0,
        keywords: "wireless headphones speaker mouse keyboard pair scan bluez",
    },
    // Group 1: Hardware & Devices
    CategoryDef {
        id: "displays",
        title: "Displays",
        description: "Arrange monitors, scaling, rotation, Night Light, and display profiles.",
        icon: "video-display-symbolic",
        group: 1,
        keywords: "monitor screen resolution refresh fractional scale rotation night light hdr",
    },
    CategoryDef {
        id: "sound",
        title: "Sound",
        description: "Configure PipeWire output devices, microphones, per-app volumes, and audio tests.",
        icon: "audio-speakers-symbolic",
        group: 1,
        keywords: "audio volume speakers headphones microphone pipewire wireplumber mute test",
    },
    CategoryDef {
        id: "power",
        title: "Power & Battery",
        description: "Monitor battery status, choose power profiles, and set sleep timeouts.",
        icon: "battery-good-symbolic",
        group: 1,
        keywords: "battery energy profile saver balanced performance sleep lid suspend upower",
    },
    CategoryDef {
        id: "storage",
        title: "Storage & Devices",
        description: "Inspect disks, partitions, removable media, CUPS printers, and scanners.",
        icon: "drive-harddisk-symbolic",
        group: 1,
        keywords: "storage disk drive partition mount usb eject printer scanner cups udisks2",
    },
    // Group 2: Personalization & Desktop
    CategoryDef {
        id: "appearance",
        title: "Appearance",
        description: "Customize theme mode, accent colors, wallpapers, icons, cursors, fonts, and blur.",
        icon: "preferences-desktop-appearance-symbolic",
        group: 2,
        keywords: "theme dark light auto accent color wallpaper icon gtk cursor pointer font blur",
    },
    CategoryDef {
        id: "desktop",
        title: "Desktop & Workspaces",
        description: "Configure dynamic workspaces, workspace grid, Overview, hot corners, and desktop menu.",
        icon: "preferences-desktop-wallpaper-symbolic",
        group: 2,
        keywords: "desktop workspace virtual overview mission control hot corner gesture icons",
    },
    CategoryDef {
        id: "dock",
        title: "Dock & Panels",
        description: "Customize the floating dock, top panel, clock, system tray, and secondary panels.",
        icon: "view-grid-symbolic",
        group: 2,
        keywords: "dock panel menu bar topbar clock date workspaces autohide tray transparency",
    },
    CategoryDef {
        id: "windows",
        title: "Windows & Management",
        description: "Control focus policy, snapping, titlebar buttons, decorations, and per-app window rules.",
        icon: "preferences-system-windows-symbolic",
        group: 2,
        keywords: "window focus snap tiling placement titlebar traffic lights rules maximized",
    },
    CategoryDef {
        id: "effects",
        title: "Effects & Animations",
        description: "Select Performance, Balanced, or Visual Quality presets and tune compositor shaders.",
        icon: "applications-graphics-symbolic",
        group: 2,
        keywords: "effects animation blur shadow corner radius opacity speed reduced motion preset",
    },
    // Group 3: Input & Accessibility
    CategoryDef {
        id: "input",
        title: "Input Devices",
        description: "Tune Mouse, Touchpad, and Keyboard behavior backed by libinput and XKB.",
        icon: "input-mouse-symbolic",
        group: 3,
        keywords: "mouse touchpad trackpad keyboard pointer speed acceleration scroll tap layout repeat",
    },
    CategoryDef {
        id: "shortcuts",
        title: "Keyboard Shortcuts",
        description: "View, search, customize, add, and detect conflicts across desktop shortcuts.",
        icon: "preferences-desktop-keyboard-shortcuts-symbolic",
        group: 3,
        keywords: "shortcuts keybindings hotkeys custom conflict reset super alt",
    },
    CategoryDef {
        id: "accessibility",
        title: "Accessibility",
        description: "Enable high contrast, reduced motion, text scaling, large pointer, and typing aids.",
        icon: "preferences-desktop-accessibility-symbolic",
        group: 3,
        keywords: "high contrast reduced motion text scale large pointer sticky slow bounce screen reader",
    },
    // Group 4: Notifications, Apps & System
    CategoryDef {
        id: "notifications",
        title: "Notifications",
        description: "Control Do Not Disturb, banner previews, position, sounds, and per-app muting.",
        icon: "preferences-system-notifications-symbolic",
        group: 4,
        keywords: "alerts toasts banners do not disturb dnd previews duration dbus",
    },
    CategoryDef {
        id: "applications",
        title: "Applications & Default Apps",
        description: "Browse installed applications and configure default XDG MIME handlers.",
        icon: "system-software-install-symbolic",
        group: 4,
        keywords: "applications default browser email file manager terminal media player mime xdg",
    },
    CategoryDef {
        id: "datetime",
        title: "Date, Time & Language",
        description: "Set automatic network time (NTP), timezone, clock format, and system locale.",
        icon: "preferences-system-time-symbolic",
        group: 4,
        keywords: "date time clock timezone ntp language locale region 24h timedated",
    },
    CategoryDef {
        id: "startup",
        title: "Startup & Session",
        description: "Manage XDG autostart apps, session restore, and logind power actions.",
        icon: "system-run-symbolic",
        group: 4,
        keywords: "startup autostart session restore login logout suspend hibernate restart shutdown",
    },
    // Group 5: Security, Accounts & Updates
    CategoryDef {
        id: "privacy",
        title: "Privacy & Security",
        description: "Manage screen lock, auto-lock timeout, polkit/portal status, and recent files.",
        icon: "security-high-symbolic",
        group: 5,
        keywords: "lock screen password polkit peercred portal camera microphone location recent files",
    },
    CategoryDef {
        id: "users",
        title: "Users & Accounts",
        description: "View local accounts, account roles, and update user display details via AccountsService.",
        icon: "system-users-symbolic",
        group: 5,
        keywords: "users accounts profile avatar name administrator sudo accountsservice",
    },
    CategoryDef {
        id: "updates",
        title: "Updates & Software",
        description: "Check available system updates, inspect package repositories, and launch the updater.",
        icon: "software-update-available-symbolic",
        group: 5,
        keywords: "updates upgrade software apt dpkg repositories packages security",
    },
];

/// Pinned bottom category in the sidebar ("About NEON DE").
pub const ABOUT_CATEGORY: CategoryDef = CategoryDef {
    id: "about",
    title: "About NEON DE",
    description: "System version, hardware overview, live compositor diagnostics, and config backup.",
    icon: "computer-symbolic",
    group: 99,
    keywords: "about neon de system hardware cpu gpu memory diagnostics backup export import reset",
};

/// Finds any category (including pinned `about`, plus legacy aliases like `system`/`mouse`/`keyboard`).
pub fn find_category(id: &str) -> Option<&'static CategoryDef> {
    let normalized = match id {
        "system" => "about",
        "mouse" | "keyboard" | "touchpad" => "input",
        other => other,
    };
    if normalized == ABOUT_CATEGORY.id {
        return Some(&ABOUT_CATEGORY);
    }
    SIDEBAR_CATEGORIES.iter().find(|c| c.id == normalized)
}

/// Simple navigation back-stack for macOS-style Back button and `Alt+Left` history.
#[derive(Debug, Default)]
pub struct NavigationHistory {
    stack: Vec<String>,
}

impl NavigationHistory {
    pub fn visit(&mut self, category_id: &str) {
        if self.stack.last().map(String::as_str) == Some(category_id) {
            return;
        }
        self.stack.push(category_id.to_string());
        if self.stack.len() > 64 {
            self.stack.remove(0);
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.stack.len() > 1
    }

    pub fn go_back(&mut self) -> Option<String> {
        if self.stack.len() <= 1 {
            return None;
        }
        self.stack.pop();
        self.stack.last().cloned()
    }

    #[cfg(test)]
    pub fn current(&self) -> Option<&str> {
        self.stack.last().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_22_categories_exist_with_unique_ids() {
        let mut ids = std::collections::HashSet::new();
        for c in SIDEBAR_CATEGORIES {
            assert!(ids.insert(c.id), "Duplicate category id: {}", c.id);
        }
        assert!(ids.insert(ABOUT_CATEGORY.id));
        assert_eq!(ids.len(), 22);
    }

    #[test]
    fn navigation_history_back_stack_works() {
        let mut hist = NavigationHistory::default();
        assert!(!hist.can_go_back());
        hist.visit("appearance");
        hist.visit("appearance"); // dedupe consecutive
        assert!(!hist.can_go_back());
        hist.visit("displays");
        hist.visit("network");
        assert!(hist.can_go_back());
        assert_eq!(hist.go_back().as_deref(), Some("displays"));
        assert_eq!(hist.current(), Some("displays"));
        assert_eq!(hist.go_back().as_deref(), Some("appearance"));
        assert!(!hist.can_go_back());
        assert_eq!(hist.go_back(), None);
    }
}
