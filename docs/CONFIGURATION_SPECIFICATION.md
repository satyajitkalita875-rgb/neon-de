# NEON DE — Configuration Specification

> **Status**: **Implemented & Verified (Milestone 6 — Schema Version 1)** in `crates/neon-config`.

## 1. Storage Location

User configuration is stored at `${XDG_CONFIG_HOME:-$HOME/.config}/neon-de/config.toml` (overridable via `NEON_CONFIG_PATH` or `neon-settings --config <path>` for isolated testing).

## 2. Schema Version & Sections (`schema_version = 1`)

- `[appearance]` — `theme` (`neon_dark` / `neon_light` / `auto`), `accent_color` (`#RRGGBB`), `font_family`, `font_size`, `icon_theme`, `cursor_theme`, `ui_density`, `wallpaper_path`
- `[effects]` — `enabled`, `reduced_motion`, `shadows_enabled`, `borders_enabled`, `blur_enabled`, `blur_strength`, `inactive_opacity`, `corner_radius`, `speed_scale`
- `[windows]` — `focus_mode` (`click_to_focus` / `focus_follows_mouse`), `snap_gap`, `dynamic_workspaces`, `static_workspace_count`
- `[dock]` — `position`, `icon_size`, `auto_hide`, `magnification`, `transparency`
- `[panel]` — `position`, `height`, `visible`, `transparency`, `show_clock`, `show_workspaces`
- `[input]` — `keyboard_repeat_rate`, `keyboard_repeat_delay`, `natural_scroll`, `tap_to_click`, `pointer_speed`
- `[privacy_accessibility]` — `high_contrast`, `text_scale`, `auto_lock_seconds`, `screen_timeout_seconds`

## 3. Safety & Live Propagation Guarantees

- **Validation & Clamping (`NeonConfig::validate_and_clamp`)**: Every numeric range, finite float check, and hex color is sanitized before saving or applying.
- **Corrupt File Recovery**: Malformed TOML is backed up to `config.toml.corrupt` and safely replaced in memory with `NeonConfig::default()` without crashing `neon-compositor`.
- **Atomic Persistence (`save_atomic`)**: Writes to `config.toml.tmp`, calls `sync_all()`, and atomically renames to `config.toml`.
- **Live Updates**: `neon-settings` persists changes via `neon-config` and pushes live `UpdateEffects`, `UpdateAppearance`, `UpdateWindowBehavior`, and `ConfigureOutput` requests over `neon-ipc` so both `neon-compositor` and `neon-shell` update immediately without restarting.
- **Backup, Export & Reset**: Supports `export_config`, `import_config`, and full Reset to Defaults directly in `neon-settings`.
