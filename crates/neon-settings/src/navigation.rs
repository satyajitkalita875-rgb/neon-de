//! Streamlined NEON DE Settings categories (only genuine, working desktop settings)
//! and macOS-style Back/Forward navigation history.

/// Sidebar category descriptor with macOS colorful rounded icon badge styling.
#[derive(Debug, Clone, Copy)]
pub struct CategoryDef {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub badge_class: &'static str,
    pub group: u8,
    pub keywords: &'static str,
}

/// Main scrollable sidebar categories (11 working sections; `about` is pinned at the bottom).
pub const SIDEBAR_CATEGORIES: &[CategoryDef] = &[
    // Group 0: Connectivity
    CategoryDef {
        id: "network",
        title: "Wi-Fi & Network",
        description: "Manage Wi-Fi networks, Ethernet, IP/DNS details, and saved connections.",
        icon: "network-wireless-symbolic",
        badge_class: "nav-badge-blue",
        group: 0,
        keywords: "wifi wi-fi ethernet internet ip dns vpn networkmanager",
    },
    CategoryDef {
        id: "bluetooth",
        title: "Bluetooth",
        description: "Discover, pair, and manage wireless Bluetooth devices via BlueZ.",
        icon: "bluetooth-active-symbolic",
        badge_class: "nav-badge-blue",
        group: 0,
        keywords: "wireless headphones speaker mouse keyboard pair scan bluez",
    },
    // Group 1: Personalization & Desktop
    CategoryDef {
        id: "appearance",
        title: "Appearance",
        description: "Theme mode, accent color, wallpaper, glass blur, cursor, icons, and fonts.",
        icon: "preferences-desktop-appearance-symbolic",
        badge_class: "nav-badge-violet",
        group: 1,
        keywords: "theme dark light auto accent color wallpaper icon gtk cursor pointer font blur contrast",
    },
    CategoryDef {
        id: "dock",
        title: "Desktop & Dock",
        description: "Floating dock, top bar, window behavior, focus, snap gap, and compositor animations.",
        icon: "user-desktop-symbolic",
        badge_class: "nav-badge-sky",
        group: 1,
        keywords: "dock panel topbar clock workspaces window focus snap opacity animations effects motion preset",
    },
    // Group 2: Hardware
    CategoryDef {
        id: "displays",
        title: "Displays",
        description: "Monitor arrangement, brightness, fractional scaling, rotation, and primary display.",
        icon: "video-display-symbolic",
        badge_class: "nav-badge-teal",
        group: 2,
        keywords: "monitor display screen brightness resolution refresh scale rotation primary",
    },
    CategoryDef {
        id: "sound",
        title: "Sound",
        description: "PipeWire output devices, microphones, per-app volume streams, and speaker test.",
        icon: "audio-speakers-symbolic",
        badge_class: "nav-badge-pink",
        group: 2,
        keywords: "audio volume speakers headphones microphone pipewire wireplumber mute test",
    },
    CategoryDef {
        id: "power",
        title: "Battery & Power",
        description: "Battery health, power profiles, and session power controls.",
        icon: "battery-good-symbolic",
        badge_class: "nav-badge-green",
        group: 2,
        keywords: "battery energy profile saver balanced performance lock sleep suspend logout restart shutdown",
    },
    CategoryDef {
        id: "storage",
        title: "Storage & Printers",
        description: "Mounted disks, removable drive ejection, and CUPS printers.",
        icon: "drive-harddisk-symbolic",
        badge_class: "nav-badge-gray",
        group: 2,
        keywords: "storage disk drive partition mount usb eject printer cups",
    },
    // Group 3: Apps & System
    CategoryDef {
        id: "applications",
        title: "Applications",
        description: "Default XDG applications, Login Items (Autostart), and installed apps.",
        icon: "system-software-install-symbolic",
        badge_class: "nav-badge-orange",
        group: 3,
        keywords: "applications default browser email file manager terminal media player startup autostart login items",
    },
    CategoryDef {
        id: "shortcuts",
        title: "Keyboard Shortcuts",
        description: "Active NEON DE compositor keyboard shortcuts reference.",
        icon: "preferences-desktop-keyboard-shortcuts-symbolic",
        badge_class: "nav-badge-slate",
        group: 3,
        keywords: "shortcuts keybindings hotkeys super alt workspace screenshot terminal lock",
    },
    CategoryDef {
        id: "datetime",
        title: "Date & Time",
        description: "Automatic network time (NTP), IANA timezone, and system locale.",
        icon: "preferences-system-time-symbolic",
        badge_class: "nav-badge-red",
        group: 3,
        keywords: "date time clock timezone ntp language locale timedated",
    },
];

/// Pinned bottom category in the sidebar ("About NEON DE").
pub const ABOUT_CATEGORY: CategoryDef = CategoryDef {
    id: "about",
    title: "About NEON DE",
    description: "System version, hardware overview, compositor telemetry, and config backup.",
    icon: "computer-symbolic",
    badge_class: "nav-badge-indigo",
    group: 99,
    keywords: "about neon de system hardware cpu gpu memory diagnostics backup export import reset",
};

/// Finds any category (including pinned `about` and graceful aliases for removed/merged legacy ids).
pub fn find_category(id: &str) -> Option<&'static CategoryDef> {
    let normalized = match id {
        "system" | "updates" | "users" => "about",
        "effects" | "windows" | "desktop" => "dock",
        "startup" => "applications",
        "accessibility" | "input" | "mouse" | "keyboard" | "touchpad" | "notifications" => {
            "appearance"
        }
        "privacy" => "power",
        other => other,
    };
    if normalized == ABOUT_CATEGORY.id {
        return Some(&ABOUT_CATEGORY);
    }
    SIDEBAR_CATEGORIES.iter().find(|c| c.id == normalized)
}

/// macOS-style Back/Forward navigation history.
#[derive(Debug, Default)]
pub struct NavigationHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
}

impl NavigationHistory {
    pub fn visit(&mut self, category_id: &str) {
        if let Some(idx) = self.cursor {
            if self.entries.get(idx).map(String::as_str) == Some(category_id) {
                return;
            }
            self.entries.truncate(idx + 1);
        }
        self.entries.push(category_id.to_string());
        if self.entries.len() > 64 {
            self.entries.remove(0);
        }
        self.cursor = Some(self.entries.len() - 1);
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor.is_some_and(|i| i > 0)
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor.is_some_and(|i| i + 1 < self.entries.len())
    }

    pub fn go_back(&mut self) -> Option<String> {
        let idx = self.cursor?;
        if idx == 0 {
            return None;
        }
        self.cursor = Some(idx - 1);
        self.entries.get(idx - 1).cloned()
    }

    pub fn go_forward(&mut self) -> Option<String> {
        let idx = self.cursor?;
        if idx + 1 >= self.entries.len() {
            return None;
        }
        self.cursor = Some(idx + 1);
        self.entries.get(idx + 1).cloned()
    }

    #[cfg(test)]
    pub fn current(&self) -> Option<&str> {
        self.cursor
            .and_then(|i| self.entries.get(i))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_12_categories_exist_with_unique_ids() {
        let mut ids = std::collections::HashSet::new();
        for c in SIDEBAR_CATEGORIES {
            assert!(ids.insert(c.id), "Duplicate category id: {}", c.id);
        }
        assert!(ids.insert(ABOUT_CATEGORY.id));
        assert_eq!(ids.len(), 12);
    }

    #[test]
    fn navigation_history_back_and_forward_works() {
        let mut hist = NavigationHistory::default();
        assert!(!hist.can_go_back());
        assert!(!hist.can_go_forward());
        hist.visit("appearance");
        hist.visit("appearance"); // dedupe consecutive
        assert!(!hist.can_go_back());
        hist.visit("displays");
        hist.visit("network");
        assert!(hist.can_go_back());
        assert!(!hist.can_go_forward());
        assert_eq!(hist.go_back().as_deref(), Some("displays"));
        assert_eq!(hist.current(), Some("displays"));
        assert!(hist.can_go_forward());
        assert_eq!(hist.go_forward().as_deref(), Some("network"));
        assert!(!hist.can_go_forward());
        assert_eq!(hist.go_back().as_deref(), Some("displays"));
        assert_eq!(hist.go_back().as_deref(), Some("appearance"));
        assert!(!hist.can_go_back());
        assert_eq!(hist.go_back(), None);
    }
}
