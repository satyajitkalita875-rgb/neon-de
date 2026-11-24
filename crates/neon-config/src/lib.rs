//! Centralized, versioned, validated XDG configuration service for **NEON DE**.
//!
//! # Guarantees
//! - Stored at `${XDG_CONFIG_HOME:-$HOME/.config}/neon-de/config.toml` (overridable via `NEON_CONFIG_PATH`).
//! - Explicit `schema_version` with forward/backward migration hooks.
//! - Bounds-checked validation (`validate_and_clamp`) so invalid values never crash the desktop.
//! - Atomic write-and-rename persistence (`config.toml.tmp` -> `sync_all` -> `rename`).
//! - Corrupt configuration files are backed up cleanly to `*.corrupt` and replaced with safe defaults.
//! - Never stores sensitive secrets (Wi-Fi passwords, user passwords) in plain-text config files.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Current NEON DE configuration schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Built-in NEON DE color theme modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    NeonDark,
    NeonLight,
    Auto,
}

/// UI spacing density presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiDensity {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

/// Wallpaper scaling and positioning modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperScaleMode {
    #[default]
    Cover,
    Fit,
    Stretch,
    Center,
}

/// Dock placement edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    #[default]
    Bottom,
    Left,
    Right,
}

/// Visual background surface style for the floating dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockBackgroundStyle {
    #[default]
    Glass,
    Solid,
    Minimal,
}

/// Panel placement edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelPosition {
    #[default]
    Top,
    Bottom,
}

/// Window focus behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusMode {
    #[default]
    ClickToFocus,
    FocusFollowsMouse,
}

/// Initial floating window placement policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowPlacement {
    #[default]
    Smart,
    Center,
    Cascade,
}

/// Action performed when double-clicking a window titlebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarAction {
    #[default]
    Maximize,
    Minimize,
    None,
}

/// Window decoration control button layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowButtonLayout {
    #[default]
    LeftTrafficLights,
    RightStandard,
}

/// Effects quality / performance presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectsPreset {
    Performance,
    #[default]
    Balanced,
    VisualQuality,
    Custom,
}

/// Mouse acceleration profile (libinput).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccelProfile {
    #[default]
    Adaptive,
    Flat,
}

/// Touchpad click method (libinput).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchpadClickMethod {
    #[default]
    Clickfinger,
    ButtonAreas,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub theme: ThemeMode,
    /// Hex RGB accent color (`#RRGGBB`, default NEON Cyan `#38BDF8`).
    pub accent_color: String,
    pub font_family: String,
    pub font_size: u8,
    pub font_scale: f64,
    pub gtk_theme: String,
    pub icon_theme: String,
    pub cursor_theme: String,
    pub cursor_size: u32,
    pub reduce_blur: bool,
    pub ui_density: UiDensity,
    pub wallpaper_path: Option<String>,
    pub wallpaper_scale_mode: WallpaperScaleMode,
    pub wallpaper_slideshow: bool,
    pub wallpaper_slideshow_minutes: u32,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::NeonDark,
            accent_color: "#38BDF8".into(),
            font_family: "Inter".into(),
            font_size: 11,
            font_scale: 1.0,
            gtk_theme: "adw-gtk3-dark".into(),
            icon_theme: "WhiteSur-dark".into(),
            cursor_theme: "macOS".into(),
            cursor_size: 20,
            reduce_blur: false,
            ui_density: UiDensity::Comfortable,
            wallpaper_path: None,
            wallpaper_scale_mode: WallpaperScaleMode::Cover,
            wallpaper_slideshow: false,
            wallpaper_slideshow_minutes: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayPrefsConfig {
    pub night_light_enabled: bool,
    pub color_temperature_k: u32,
    pub per_monitor_workspaces: bool,
    pub mirror_displays: bool,
}

impl Default for DisplayPrefsConfig {
    fn default() -> Self {
        Self {
            night_light_enabled: false,
            color_temperature_k: 4500,
            per_monitor_workspaces: false,
            mirror_displays: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopWorkspaceConfig {
    pub workspace_names: Vec<String>,
    pub workspace_columns: u8,
    pub workspace_animations: bool,
    pub per_monitor_workspaces: bool,
    pub overview_show_all_workspaces: bool,
    pub hot_corner_top_left: String,
    pub hot_corner_top_right: String,
    pub show_desktop_icons: bool,
    pub desktop_right_click_menu: bool,
}

impl Default for DesktopWorkspaceConfig {
    fn default() -> Self {
        Self {
            workspace_names: vec![
                "Desktop 1".into(),
                "Desktop 2".into(),
                "Desktop 3".into(),
                "Desktop 4".into(),
            ],
            workspace_columns: 4,
            workspace_animations: true,
            per_monitor_workspaces: false,
            overview_show_all_workspaces: true,
            hot_corner_top_left: "overview".into(),
            hot_corner_top_right: "none".into(),
            show_desktop_icons: false,
            desktop_right_click_menu: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectsConfigData {
    pub preset: EffectsPreset,
    pub enabled: bool,
    pub reduced_motion: bool,
    pub shadows_enabled: bool,
    pub borders_enabled: bool,
    pub blur_enabled: bool,
    pub blur_strength: f32,
    pub inactive_opacity: f32,
    pub corner_radius: f32,
    pub speed_scale: f64,
    pub open_animation: bool,
    pub close_animation: bool,
    pub minimize_animation: bool,
    pub maximize_animation: bool,
    pub workspace_animation: bool,
    pub overview_animation: bool,
    pub dock_animation: bool,
}

impl Default for EffectsConfigData {
    fn default() -> Self {
        Self {
            preset: EffectsPreset::Balanced,
            enabled: true,
            reduced_motion: false,
            shadows_enabled: true,
            borders_enabled: true,
            blur_enabled: true,
            blur_strength: 0.7,
            inactive_opacity: 0.96,
            corner_radius: 12.0,
            speed_scale: 1.0,
            open_animation: true,
            close_animation: true,
            minimize_animation: true,
            maximize_animation: true,
            workspace_animation: true,
            overview_animation: true,
            dock_animation: true,
        }
    }
}

impl EffectsConfigData {
    /// Applies one of the 3 curated performance presets cleanly.
    pub fn apply_preset(&mut self, preset: EffectsPreset) {
        self.preset = preset;
        match preset {
            EffectsPreset::Performance => {
                self.enabled = true;
                self.reduced_motion = true;
                self.shadows_enabled = false;
                self.borders_enabled = true;
                self.blur_enabled = false;
                self.blur_strength = 0.0;
                self.inactive_opacity = 1.0;
                self.speed_scale = 1.5;
                self.open_animation = false;
                self.close_animation = false;
                self.minimize_animation = false;
                self.maximize_animation = false;
                self.workspace_animation = false;
                self.overview_animation = false;
                self.dock_animation = false;
            }
            EffectsPreset::Balanced => {
                self.enabled = true;
                self.reduced_motion = false;
                self.shadows_enabled = true;
                self.borders_enabled = true;
                self.blur_enabled = false;
                self.blur_strength = 0.5;
                self.inactive_opacity = 0.96;
                self.corner_radius = 12.0;
                self.speed_scale = 1.1;
                self.open_animation = true;
                self.close_animation = true;
                self.minimize_animation = true;
                self.maximize_animation = true;
                self.workspace_animation = true;
                self.overview_animation = true;
                self.dock_animation = true;
            }
            EffectsPreset::VisualQuality => {
                self.enabled = true;
                self.reduced_motion = false;
                self.shadows_enabled = true;
                self.borders_enabled = true;
                self.blur_enabled = true;
                self.blur_strength = 1.0;
                self.inactive_opacity = 0.93;
                self.corner_radius = 14.0;
                self.speed_scale = 1.0;
                self.open_animation = true;
                self.close_animation = true;
                self.minimize_animation = true;
                self.maximize_animation = true;
                self.workspace_animation = true;
                self.overview_animation = true;
                self.dock_animation = true;
            }
            EffectsPreset::Custom => {}
        }
    }
}

/// Per-application window management rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowRuleConfig {
    pub app_id: String,
    pub always_on_top: bool,
    pub start_maximized: bool,
    pub workspace: Option<usize>,
    pub preferred_monitor: Option<String>,
    pub min_width: Option<i32>,
    pub min_height: Option<i32>,
    pub max_width: Option<i32>,
    pub max_height: Option<i32>,
}

impl Default for WindowRuleConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            always_on_top: false,
            start_maximized: false,
            workspace: None,
            preferred_monitor: None,
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfigData {
    pub focus_mode: FocusMode,
    pub raise_on_focus: bool,
    pub placement: WindowPlacement,
    pub snap_gap: i32,
    pub auto_snap: bool,
    pub center_dialogs: bool,
    pub dynamic_workspaces: bool,
    pub static_workspace_count: usize,
    pub double_click_titlebar: TitlebarAction,
    pub button_layout: WindowButtonLayout,
    pub server_side_decorations: bool,
    pub rules: Vec<WindowRuleConfig>,
}

impl Default for WindowConfigData {
    fn default() -> Self {
        Self {
            focus_mode: FocusMode::ClickToFocus,
            raise_on_focus: true,
            placement: WindowPlacement::Smart,
            snap_gap: 0,
            auto_snap: true,
            center_dialogs: true,
            dynamic_workspaces: true,
            static_workspace_count: 4,
            double_click_titlebar: TitlebarAction::Maximize,
            button_layout: WindowButtonLayout::LeftTrafficLights,
            server_side_decorations: true,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockConfigData {
    pub position: DockPosition,
    pub icon_size: u32,
    pub auto_hide: bool,
    pub intelligent_hide: bool,
    pub magnification: bool,
    pub transparency: f32,
    pub background_style: DockBackgroundStyle,
    pub show_running_indicators: bool,
    pub show_recent_apps: bool,
    pub show_trash: bool,
    pub group_apps: bool,
    pub animations: bool,
    pub pinned_apps: Vec<String>,
}

impl Default for DockConfigData {
    fn default() -> Self {
        Self {
            position: DockPosition::Bottom,
            icon_size: 44,
            auto_hide: false,
            intelligent_hide: false,
            magnification: true,
            transparency: 0.84,
            background_style: DockBackgroundStyle::Glass,
            show_running_indicators: true,
            show_recent_apps: false,
            show_trash: false,
            group_apps: true,
            animations: true,
            pinned_apps: Vec::new(),
        }
    }
}

/// Schema entry supporting future user-created secondary panels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtraPanelConfig {
    pub id: String,
    pub position: PanelPosition,
    pub height: u32,
    pub visible: bool,
    pub transparency: f32,
    pub modules: Vec<String>,
}

impl Default for ExtraPanelConfig {
    fn default() -> Self {
        Self {
            id: "panel-2".into(),
            position: PanelPosition::Bottom,
            height: 32,
            visible: true,
            transparency: 0.86,
            modules: vec!["workspaces".into(), "clock".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelConfigData {
    pub position: PanelPosition,
    pub height: u32,
    pub visible: bool,
    pub transparency: f32,
    pub show_clock: bool,
    pub clock_24h: bool,
    pub show_date: bool,
    pub show_seconds: bool,
    pub show_workspaces: bool,
    pub show_system_tray: bool,
    pub show_notifications: bool,
    pub show_quick_settings: bool,
    pub extra_panels: Vec<ExtraPanelConfig>,
}

impl Default for PanelConfigData {
    fn default() -> Self {
        Self {
            position: PanelPosition::Top,
            height: 36,
            visible: true,
            transparency: 0.86,
            show_clock: true,
            clock_24h: true,
            show_date: true,
            show_seconds: false,
            show_workspaces: true,
            show_system_tray: true,
            show_notifications: true,
            show_quick_settings: true,
            extra_panels: Vec::new(),
        }
    }
}

/// Typed keyboard shortcut binding for the dedicated Shortcut Manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutBindingConfig {
    pub id: String,
    pub category: String,
    pub label: String,
    pub accelerator: String,
    pub command: Option<String>,
    pub custom: bool,
}

impl Default for ShortcutBindingConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            category: "System".into(),
            label: String::new(),
            accelerator: String::new(),
            command: None,
            custom: false,
        }
    }
}

/// Built-in default NEON DE keyboard shortcuts matching `neon-compositor/src/input.rs`.
pub fn default_shortcuts() -> Vec<ShortcutBindingConfig> {
    vec![
        ShortcutBindingConfig {
            id: "overview".into(),
            category: "System".into(),
            label: "Open Launchpad & Mission Control".into(),
            accelerator: "Super+Space".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "lock".into(),
            category: "System".into(),
            label: "Lock Screen".into(),
            accelerator: "Super+L".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "screenshot_ui".into(),
            category: "System".into(),
            label: "Interactive Screenshot".into(),
            accelerator: "Super+Shift+4".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "screenshot_full".into(),
            category: "System".into(),
            label: "Full-Screen Screenshot".into(),
            accelerator: "Super+Shift+3".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "terminal".into(),
            category: "Applications".into(),
            label: "Launch Terminal".into(),
            accelerator: "Ctrl+Alt+T".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "close_window".into(),
            category: "Windows".into(),
            label: "Close Focused Window".into(),
            accelerator: "Super+Q".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "maximize_window".into(),
            category: "Windows".into(),
            label: "Toggle Maximize Window".into(),
            accelerator: "Super+Up".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "minimize_window".into(),
            category: "Windows".into(),
            label: "Minimize Window".into(),
            accelerator: "Super+H".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "fullscreen_window".into(),
            category: "Windows".into(),
            label: "Toggle Fullscreen".into(),
            accelerator: "Super+F".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "snap_left".into(),
            category: "Windows".into(),
            label: "Snap Window Left Half".into(),
            accelerator: "Super+Left".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "snap_right".into(),
            category: "Windows".into(),
            label: "Snap Window Right Half".into(),
            accelerator: "Super+Right".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "cycle_focus".into(),
            category: "Windows".into(),
            label: "Switch Windows".into(),
            accelerator: "Alt+Tab".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "ws_next".into(),
            category: "Workspaces".into(),
            label: "Switch to Next Workspace".into(),
            accelerator: "Ctrl+Alt+Right".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "ws_prev".into(),
            category: "Workspaces".into(),
            label: "Switch to Previous Workspace".into(),
            accelerator: "Ctrl+Alt+Left".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "ws_move_next".into(),
            category: "Workspaces".into(),
            label: "Move Window to Next Workspace".into(),
            accelerator: "Ctrl+Alt+Shift+Right".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "ws_move_prev".into(),
            category: "Workspaces".into(),
            label: "Move Window to Previous Workspace".into(),
            accelerator: "Ctrl+Alt+Shift+Left".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "media_vol_mute".into(),
            category: "Media".into(),
            label: "Mute / Unmute Audio".into(),
            accelerator: "XF86AudioMute".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "media_vol_up".into(),
            category: "Media".into(),
            label: "Volume Up".into(),
            accelerator: "XF86AudioRaiseVolume".into(),
            command: None,
            custom: false,
        },
        ShortcutBindingConfig {
            id: "media_vol_down".into(),
            category: "Media".into(),
            label: "Volume Down".into(),
            accelerator: "XF86AudioLowerVolume".into(),
            command: None,
            custom: false,
        },
    ]
}

/// Detects duplicate accelerator conflicts across shortcut bindings.
pub fn find_shortcut_conflict<'a>(
    shortcuts: &'a [ShortcutBindingConfig],
    ignore_id: &str,
    candidate_accel: &str,
) -> Option<&'a ShortcutBindingConfig> {
    let norm = candidate_accel.trim().to_ascii_lowercase();
    if norm.is_empty() {
        return None;
    }
    shortcuts
        .iter()
        .find(|s| s.id != ignore_id && s.accelerator.trim().to_ascii_lowercase() == norm)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfigData {
    pub keyboard_layout: String,
    pub keyboard_layouts: Vec<String>,
    pub keyboard_repeat_rate: i32,
    pub keyboard_repeat_delay: i32,
    pub compose_key: String,
    pub caps_lock_behavior: String,
    pub natural_scroll: bool,
    pub tap_to_click: bool,
    pub pointer_speed: f64,
    pub mouse_accel_enabled: bool,
    pub mouse_accel_profile: AccelProfile,
    pub left_handed: bool,
    pub double_click_ms: u32,
    pub touchpad_enabled: bool,
    pub touchpad_speed: f64,
    pub two_finger_scroll: bool,
    pub click_method: TouchpadClickMethod,
    pub disable_while_typing: bool,
    pub three_finger_gestures: bool,
    pub four_finger_gestures: bool,
    pub shortcuts: Vec<ShortcutBindingConfig>,
}

impl Default for InputConfigData {
    fn default() -> Self {
        Self {
            keyboard_layout: "us".into(),
            keyboard_layouts: vec!["us".into()],
            keyboard_repeat_rate: 25,
            keyboard_repeat_delay: 600,
            compose_key: "none".into(),
            caps_lock_behavior: "default".into(),
            natural_scroll: true,
            tap_to_click: true,
            pointer_speed: 0.0,
            mouse_accel_enabled: true,
            mouse_accel_profile: AccelProfile::Adaptive,
            left_handed: false,
            double_click_ms: 400,
            touchpad_enabled: true,
            touchpad_speed: 0.0,
            two_finger_scroll: true,
            click_method: TouchpadClickMethod::Clickfinger,
            disable_while_typing: true,
            three_finger_gestures: true,
            four_finger_gestures: false,
            shortcuts: default_shortcuts(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfigData {
    pub enabled: bool,
    pub do_not_disturb: bool,
    pub show_previews: bool,
    pub duration_secs: u32,
    pub position: String,
    pub sounds_enabled: bool,
    pub lock_screen_notifications: bool,
    pub muted_apps: Vec<String>,
}

impl Default for NotificationsConfigData {
    fn default() -> Self {
        Self {
            enabled: true,
            do_not_disturb: false,
            show_previews: true,
            duration_secs: 5,
            position: "top_right".into(),
            sounds_enabled: true,
            lock_screen_notifications: false,
            muted_apps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerConfigData {
    pub screen_dim_seconds: u32,
    pub screen_blank_seconds: u32,
    pub suspend_timeout_seconds: u32,
    pub lid_close_action: String,
    pub power_button_action: String,
    pub low_battery_notifications: bool,
}

impl Default for PowerConfigData {
    fn default() -> Self {
        Self {
            screen_dim_seconds: 180,
            screen_blank_seconds: 600,
            suspend_timeout_seconds: 1800,
            lid_close_action: "suspend".into(),
            power_button_action: "suspend".into(),
            low_battery_notifications: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessibilityPrivacyConfig {
    pub high_contrast: bool,
    pub text_scale: f64,
    pub auto_lock_seconds: u32,
    pub screen_timeout_seconds: u32,
    pub recent_files_enabled: bool,
    pub usage_history_enabled: bool,
    pub sticky_keys: bool,
    pub slow_keys: bool,
    pub bounce_keys: bool,
    pub mouse_keys: bool,
    pub screen_reader: bool,
    pub on_screen_keyboard: bool,
    pub screen_magnifier: bool,
    pub large_pointer: bool,
}

impl Default for AccessibilityPrivacyConfig {
    fn default() -> Self {
        Self {
            high_contrast: false,
            text_scale: 1.0,
            auto_lock_seconds: 300,
            screen_timeout_seconds: 600,
            recent_files_enabled: true,
            usage_history_enabled: true,
            sticky_keys: false,
            slow_keys: false,
            bounce_keys: false,
            mouse_keys: false,
            screen_reader: false,
            on_screen_keyboard: false,
            screen_magnifier: false,
            large_pointer: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoragePrefsConfig {
    pub automount_removable: bool,
    pub automount_open_files: bool,
}

impl Default for StoragePrefsConfig {
    fn default() -> Self {
        Self {
            automount_removable: true,
            automount_open_files: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfigData {
    pub restore_session: bool,
    pub default_session: String,
    pub last_settings_page: String,
    pub update_notifications: bool,
}

impl Default for SessionConfigData {
    fn default() -> Self {
        Self {
            restore_session: false,
            default_session: "neon-de".into(),
            last_settings_page: "appearance".into(),
            update_notifications: true,
        }
    }
}

/// Complete versioned NEON DE configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NeonConfig {
    pub schema_version: u32,
    pub appearance: AppearanceConfig,
    pub displays: DisplayPrefsConfig,
    pub desktop: DesktopWorkspaceConfig,
    pub effects: EffectsConfigData,
    pub windows: WindowConfigData,
    pub dock: DockConfigData,
    pub panel: PanelConfigData,
    pub input: InputConfigData,
    pub notifications: NotificationsConfigData,
    pub power: PowerConfigData,
    pub privacy_accessibility: AccessibilityPrivacyConfig,
    pub storage: StoragePrefsConfig,
    pub session: SessionConfigData,
}

impl Default for NeonConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            appearance: AppearanceConfig::default(),
            displays: DisplayPrefsConfig::default(),
            desktop: DesktopWorkspaceConfig::default(),
            effects: EffectsConfigData::default(),
            windows: WindowConfigData::default(),
            dock: DockConfigData::default(),
            panel: PanelConfigData::default(),
            input: InputConfigData::default(),
            notifications: NotificationsConfigData::default(),
            power: PowerConfigData::default(),
            privacy_accessibility: AccessibilityPrivacyConfig::default(),
            storage: StoragePrefsConfig::default(),
            session: SessionConfigData::default(),
        }
    }
}

impl NeonConfig {
    /// Clamps and sanitizes every field to safe runtime bounds so invalid values never crash NEON DE.
    pub fn validate_and_clamp(&mut self) {
        self.schema_version = CURRENT_SCHEMA_VERSION;

        if !is_valid_hex_color(&self.appearance.accent_color) {
            warn!(
                invalid = %self.appearance.accent_color,
                "Invalid accent_color in config; resetting to NEON Cyan #38BDF8"
            );
            self.appearance.accent_color = "#38BDF8".into();
        }
        self.appearance.font_size = self.appearance.font_size.clamp(8, 24);
        self.appearance.font_scale = clamp_finite_f64(self.appearance.font_scale, 0.75, 2.0, 1.0);
        self.appearance.cursor_size = self.appearance.cursor_size.clamp(16, 48);
        self.appearance.wallpaper_slideshow_minutes =
            self.appearance.wallpaper_slideshow_minutes.clamp(1, 1440);

        self.displays.color_temperature_k = self.displays.color_temperature_k.clamp(1700, 6500);
        self.desktop.workspace_columns = self.desktop.workspace_columns.clamp(1, 8);

        self.effects.blur_strength = clamp_finite_f32(self.effects.blur_strength, 0.0, 2.0, 1.0);
        self.effects.inactive_opacity =
            clamp_finite_f32(self.effects.inactive_opacity, 0.4, 1.0, 0.96);
        self.effects.corner_radius = clamp_finite_f32(self.effects.corner_radius, 0.0, 32.0, 12.0);
        self.effects.speed_scale = clamp_finite_f64(self.effects.speed_scale, 0.1, 5.0, 1.0);

        self.windows.snap_gap = self.windows.snap_gap.clamp(0, 48);
        self.windows.static_workspace_count = self.windows.static_workspace_count.clamp(1, 16);

        self.dock.icon_size = self.dock.icon_size.clamp(28, 72);
        self.dock.transparency = clamp_finite_f32(self.dock.transparency, 0.25, 1.0, 0.84);

        self.panel.height = self.panel.height.clamp(24, 56);
        self.panel.transparency = clamp_finite_f32(self.panel.transparency, 0.25, 1.0, 0.86);

        self.input.keyboard_repeat_rate = self.input.keyboard_repeat_rate.clamp(5, 100);
        self.input.keyboard_repeat_delay = self.input.keyboard_repeat_delay.clamp(100, 2000);
        self.input.pointer_speed = clamp_finite_f64(self.input.pointer_speed, -1.0, 1.0, 0.0);
        self.input.touchpad_speed = clamp_finite_f64(self.input.touchpad_speed, -1.0, 1.0, 0.0);
        self.input.double_click_ms = self.input.double_click_ms.clamp(150, 1000);
        if self.input.keyboard_layouts.is_empty() {
            self.input
                .keyboard_layouts
                .push(self.input.keyboard_layout.clone());
        }
        if self.input.shortcuts.is_empty() {
            self.input.shortcuts = default_shortcuts();
        }

        self.notifications.duration_secs = self.notifications.duration_secs.clamp(1, 30);

        self.privacy_accessibility.text_scale =
            clamp_finite_f64(self.privacy_accessibility.text_scale, 0.75, 2.0, 1.0);
    }

    /// Resets a single named category back to its clean defaults without touching other categories.
    pub fn reset_category(&mut self, category_id: &str) -> bool {
        match category_id {
            "appearance" => self.appearance = AppearanceConfig::default(),
            "displays" => self.displays = DisplayPrefsConfig::default(),
            "desktop" => self.desktop = DesktopWorkspaceConfig::default(),
            "dock" => {
                self.dock = DockConfigData::default();
                self.panel = PanelConfigData::default();
            }
            "windows" => self.windows = WindowConfigData::default(),
            "effects" => self.effects = EffectsConfigData::default(),
            "input" | "mouse" | "keyboard" => self.input = InputConfigData::default(),
            "shortcuts" => self.input.shortcuts = default_shortcuts(),
            "notifications" => self.notifications = NotificationsConfigData::default(),
            "power" => self.power = PowerConfigData::default(),
            "privacy" | "accessibility" => {
                self.privacy_accessibility = AccessibilityPrivacyConfig::default()
            }
            "storage" => self.storage = StoragePrefsConfig::default(),
            "startup" => self.session = SessionConfigData::default(),
            _ => return false,
        }
        self.validate_and_clamp();
        true
    }

    /// Parses TOML text, migrating older schemas and clamping out-of-range fields safely.
    pub fn from_toml_str(raw: &str) -> Result<Self, toml::de::Error> {
        let mut value: toml::Value = toml::from_str(raw)?;
        migrate_toml_value(&mut value);
        let mut cfg: Self = value.try_into()?;
        cfg.validate_and_clamp();
        Ok(cfg)
    }

    /// Serializes this configuration into clean pretty-printed TOML.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

/// Resolves the standard XDG configuration file path (`~/.config/neon-de/config.toml`).
pub fn default_config_path() -> PathBuf {
    if let Some(override_path) = std::env::var_os("NEON_CONFIG_PATH") {
        return PathBuf::from(override_path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("neon-de").join("config.toml")
}

/// Loads [`NeonConfig`] from `path`, falling back safely to defaults if missing or corrupt.
pub fn load_or_default(path: &Path) -> NeonConfig {
    let Ok(raw) = fs::read_to_string(path) else {
        return NeonConfig::default();
    };
    match NeonConfig::from_toml_str(&raw) {
        Ok(cfg) => cfg,
        Err(err) => {
            warn!(
                path = %path.display(),
                ?err,
                "Corrupt NEON DE configuration file; backing up and using safe defaults"
            );
            let backup = path.with_extension("toml.corrupt");
            let _ = fs::copy(path, &backup);
            NeonConfig::default()
        }
    }
}

/// Atomically writes `config` to `path` using a temporary sibling file + `sync_all` + `rename`.
pub fn save_atomic(path: &Path, config: &NeonConfig) -> io::Result<()> {
    let mut sanitized = config.clone();
    sanitized.validate_and_clamp();
    let content = sanitized
        .to_toml_string()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("toml.tmp");
    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    info!(path = %path.display(), "Saved NEON DE configuration atomically");
    Ok(())
}

/// Exports `config` to an arbitrary user-chosen backup file.
pub fn export_config(path: &Path, config: &NeonConfig) -> io::Result<()> {
    save_atomic(path, config)
}

/// Imports and validates a [`NeonConfig`] from an exported file.
pub fn import_config(path: &Path) -> io::Result<NeonConfig> {
    let raw = fs::read_to_string(path)?;
    NeonConfig::from_toml_str(&raw).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn migrate_toml_value(value: &mut toml::Value) {
    if let Some(table) = value.as_table_mut() {
        let version = table
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .unwrap_or(0);
        if version < CURRENT_SCHEMA_VERSION as i64 {
            table.insert(
                "schema_version".into(),
                toml::Value::Integer(CURRENT_SCHEMA_VERSION as i64),
            );
        }
    }
}

fn is_valid_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn clamp_finite_f32(v: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        fallback
    }
}

fn clamp_finite_f64(v: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_save_load_and_validation_clamping() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("neon-de/config.toml");

        let mut cfg = NeonConfig::default();
        cfg.appearance.accent_color = "not-a-color".into();
        cfg.effects.corner_radius = 999.0;
        cfg.effects.speed_scale = -10.0;
        cfg.windows.snap_gap = 200;
        cfg.displays.color_temperature_k = 500;

        save_atomic(&cfg_path, &cfg).unwrap();
        let loaded = load_or_default(&cfg_path);

        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.appearance.accent_color, "#38BDF8");
        assert_eq!(loaded.effects.corner_radius, 32.0);
        assert_eq!(loaded.effects.speed_scale, 0.1);
        assert_eq!(loaded.windows.snap_gap, 48);
        assert_eq!(loaded.displays.color_temperature_k, 1700);
    }

    #[test]
    fn corrupt_config_backs_up_and_returns_safe_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        fs::write(&cfg_path, "this = [not valid toml").unwrap();

        let loaded = load_or_default(&cfg_path);
        assert_eq!(loaded, NeonConfig::default());
        assert!(cfg_path.with_extension("toml.corrupt").exists());
    }

    #[test]
    fn legacy_schema_zero_migrates_cleanly_to_current() {
        let raw = r##"
[appearance]
accent_color = "#A855F7"
[effects]
shadows_enabled = false
"##;
        let cfg = NeonConfig::from_toml_str(raw).unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(cfg.appearance.accent_color, "#A855F7");
        assert!(!cfg.effects.shadows_enabled);
        assert!(cfg.effects.blur_enabled);
        assert!(!cfg.input.shortcuts.is_empty());
    }

    #[test]
    fn effects_presets_and_category_reset_work() {
        let mut cfg = NeonConfig::default();
        cfg.effects.apply_preset(EffectsPreset::Performance);
        assert!(cfg.effects.reduced_motion);
        assert!(!cfg.effects.shadows_enabled);
        assert!(!cfg.effects.blur_enabled);

        cfg.appearance.accent_color = "#EC4899".into();
        assert!(cfg.reset_category("effects"));
        assert_eq!(cfg.effects.preset, EffectsPreset::Balanced);
        assert_eq!(cfg.appearance.accent_color, "#EC4899");
    }

    #[test]
    fn shortcut_conflict_detection_is_case_insensitive() {
        let list = default_shortcuts();
        let hit = find_shortcut_conflict(&list, "custom_1", "super+space");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().id, "overview");
        assert!(find_shortcut_conflict(&list, "overview", "super+space").is_none());
    }
}
