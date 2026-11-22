//! Centralized, versioned, validated XDG configuration service for **NEON DE**.
//!
//! # Guarantees
//! - Stored at `${XDG_CONFIG_HOME:-$HOME/.config}/neon-de/config.toml` (overridable via `NEON_CONFIG_PATH`).
//! - Explicit `schema_version` with forward/backward migration hooks.
//! - Bounds-checked validation (`validate_and_clamp`) so invalid values never crash the desktop.
//! - Atomic write-and-rename persistence (`config.toml.tmp` -> `sync_all` -> `rename`).
//! - Corrupt configuration files are backed up cleanly to `*.corrupt` and replaced with safe defaults.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Current NEON DE configuration schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

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

/// Dock placement edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    #[default]
    Bottom,
    Left,
    Right,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    pub theme: ThemeMode,
    /// Hex RGB accent color (`#RRGGBB`, default NEON Cyan `#38BDF8`).
    pub accent_color: String,
    pub font_family: String,
    pub font_size: u8,
    pub icon_theme: String,
    pub cursor_theme: String,
    pub ui_density: UiDensity,
    pub wallpaper_path: Option<String>,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::NeonDark,
            accent_color: "#38BDF8".into(),
            font_family: "Inter".into(),
            font_size: 11,
            icon_theme: "WhiteSur-dark".into(),
            cursor_theme: "Bibata-Modern-Ice".into(),
            ui_density: UiDensity::Comfortable,
            wallpaper_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectsConfigData {
    pub enabled: bool,
    pub reduced_motion: bool,
    pub shadows_enabled: bool,
    pub borders_enabled: bool,
    pub blur_enabled: bool,
    pub blur_strength: f32,
    pub inactive_opacity: f32,
    pub corner_radius: f32,
    pub speed_scale: f64,
}

impl Default for EffectsConfigData {
    fn default() -> Self {
        Self {
            enabled: true,
            reduced_motion: false,
            shadows_enabled: true,
            borders_enabled: true,
            blur_enabled: true,
            blur_strength: 1.0,
            inactive_opacity: 0.96,
            corner_radius: 12.0,
            speed_scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfigData {
    pub focus_mode: FocusMode,
    pub snap_gap: i32,
    pub dynamic_workspaces: bool,
    pub static_workspace_count: usize,
}

impl Default for WindowConfigData {
    fn default() -> Self {
        Self {
            focus_mode: FocusMode::ClickToFocus,
            snap_gap: 0,
            dynamic_workspaces: true,
            static_workspace_count: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockConfigData {
    pub position: DockPosition,
    pub icon_size: u32,
    pub auto_hide: bool,
    pub magnification: bool,
    pub transparency: f32,
}

impl Default for DockConfigData {
    fn default() -> Self {
        Self {
            position: DockPosition::Bottom,
            icon_size: 44,
            auto_hide: false,
            magnification: true,
            transparency: 0.84,
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
    pub show_workspaces: bool,
}

impl Default for PanelConfigData {
    fn default() -> Self {
        Self {
            position: PanelPosition::Top,
            height: 36,
            visible: true,
            transparency: 0.86,
            show_clock: true,
            show_workspaces: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfigData {
    pub keyboard_repeat_rate: i32,
    pub keyboard_repeat_delay: i32,
    pub natural_scroll: bool,
    pub tap_to_click: bool,
    pub pointer_speed: f64,
}

impl Default for InputConfigData {
    fn default() -> Self {
        Self {
            keyboard_repeat_rate: 25,
            keyboard_repeat_delay: 600,
            natural_scroll: true,
            tap_to_click: true,
            pointer_speed: 0.0,
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
}

impl Default for AccessibilityPrivacyConfig {
    fn default() -> Self {
        Self {
            high_contrast: false,
            text_scale: 1.0,
            auto_lock_seconds: 300,
            screen_timeout_seconds: 600,
        }
    }
}

/// Complete versioned NEON DE configuration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NeonConfig {
    pub schema_version: u32,
    pub appearance: AppearanceConfig,
    pub effects: EffectsConfigData,
    pub windows: WindowConfigData,
    pub dock: DockConfigData,
    pub panel: PanelConfigData,
    pub input: InputConfigData,
    pub privacy_accessibility: AccessibilityPrivacyConfig,
}

impl Default for NeonConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            appearance: AppearanceConfig::default(),
            effects: EffectsConfigData::default(),
            windows: WindowConfigData::default(),
            dock: DockConfigData::default(),
            panel: PanelConfigData::default(),
            input: InputConfigData::default(),
            privacy_accessibility: AccessibilityPrivacyConfig::default(),
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

        self.privacy_accessibility.text_scale =
            clamp_finite_f64(self.privacy_accessibility.text_scale, 0.75, 2.0, 1.0);
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

        save_atomic(&cfg_path, &cfg).unwrap();
        let loaded = load_or_default(&cfg_path);

        assert_eq!(loaded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(loaded.appearance.accent_color, "#38BDF8");
        assert_eq!(loaded.effects.corner_radius, 32.0);
        assert_eq!(loaded.effects.speed_scale, 0.1);
        assert_eq!(loaded.windows.snap_gap, 48);
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
    fn legacy_schema_zero_migrates_cleanly_to_v1() {
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
    }
}
