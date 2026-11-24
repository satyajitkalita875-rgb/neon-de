# NEON DE Settings — Implementation Status

This document tracks the exact implementation, verification, and capability status of every feature across all 22 categories in [`crates/neon-settings`](crates/neon-settings/src/main.rs), [`crates/neon-config`](crates/neon-config/src/lib.rs), and [`crates/neon-system`](crates/neon-system/src/lib.rs).

**Status legend**
- **Tested** — Implemented end-to-end and covered by automated unit/integration tests (`cargo test --workspace`).
- **Implemented** — Wired to real compositor IPC, XDG config, GTK/gsettings, or Linux system services (D-Bus / PipeWire / XDG / logind / UDisks2 / CUPS / APT).
- **Partially implemented** — Configuration and UI controls are persisted cleanly; live daemon/compositor runtime hook is partially supported and marked honestly in the UI.
- **Unsupported** — Not supported by the current renderer/compositor/host backend; clearly labeled in the UI with a human-readable explanation instead of a fake control.

---

## Core Architecture & Navigation

| Feature | Status | Implementation Notes |
| --- | --- | --- |
| Two-panel macOS-inspired GTK4 interface | Implemented | `src/main.rs`, `src/ui/style.rs` (Light & Dark modes, NEON DE emblem, macOS traffic lights, hairline boxed cards) |
| All 22 categories in categorized sidebar + pinned About NEON DE | Tested | `src/navigation.rs` (`all_22_categories_exist_with_unique_ids`) |
| Back navigation (`go-previous-symbolic` & `Alt+Left`) | Tested | `NavigationHistory` in `src/navigation.rs` (`navigation_history_back_stack_works`) |
| Global setting-level search across names, categories, keywords & services | Tested | `src/metadata.rs` (`SETTINGS_REGISTRY`, `search_settings`) |
| Lazy page building & scroll restoration | Implemented | Pages built on first visit and cached in `Stack` (`src/main.rs`) |
| Remember last opened settings page | Implemented | Persisted in `config.session.last_settings_page` |
| Off-UI-thread async system queries | Implemented | `run_async` in `src/ui/mod.rs` (background thread + GTK main-loop delivery) |
| Versioned atomic config + migration + category reset | Tested | `crates/neon-config/src/lib.rs` (schema v2, clamping, corrupt backup, `reset_category`) |
| Honest capability badges & zero fake controls | Implemented | `CapabilityStatus` badges (`Active`, `Partial`, `Admin`, `Unavailable`, `Unsupported`) |

---

## Settings Categories (5.1 – 5.22)

### 5.1 Appearance (`src/pages/appearance.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Light / Dark / Automatic theme | Implemented | Live `UpdateAppearance` IPC + GTK CSS reload |
| Accent color swatches + custom `#RRGGBB` hex | Tested | Validated in `neon-config`, applied live via IPC & CSS |
| Wallpaper gallery, file picker, scaling & slideshow prefs | Implemented | Downscaled `gdk_pixbuf` thumbnails + live `UpdateAppearance` IPC |
| Reduce Blur & Blur Intensity | Implemented | Live `UpdateEffects` + `UpdateAppearance` IPC |
| GTK theme & Icon theme | Implemented | Live GTK4 `Settings` + `gsettings` |
| Cursor theme & Pointer size | Implemented | Live `UpdateAppearance` IPC + GTK4 + `gsettings` |
| System fonts, font size, font scaling & UI density | Implemented | Live `gsettings` + `neon-config` |
| Window corner radius & drop shadows | Implemented | Live `UpdateEffects` IPC |
| Login screen appearance | Unsupported / Admin | Marked `RequiresElevation` (externally managed by GDM/SDDM) |

### 5.2 Displays (`src/pages/displays.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Automatic monitor detection & arrangement preview | Implemented | Live `GetOutputs` IPC + Cairo `DrawingArea` spatial map |
| Resolution & refresh rate readout | Implemented | Live DRM/KMS mode reported by `neon-compositor` |
| Fractional display scaling, orientation & primary monitor | Implemented | Live `ConfigureOutput` IPC |
| 15-second confirmation countdown & automatic revert | Implemented | `display_confirm_countdown_dialog` automatically sends prior `ConfigureOutput` on Revert/Close/Timeout |
| Mirror displays & per-monitor workspace preference | Implemented | Persisted in `config.displays` |
| Night Light & color temperature (Kelvin) | Partially implemented | Persisted in `config.displays`; live DRM gamma LUT hook noted as in progress |
| HDR output | Unsupported | Honestly labeled `Unsupported` (SDR 8-bit pipeline in `neon-renderer`) |

### 5.3 Desktop & Workspaces (`src/pages/desktop.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Dynamic vs fixed workspaces & workspace count | Implemented | Persisted in `config.windows` |
| Custom workspace names & grid columns | Implemented | Persisted in `config.desktop` |
| Workspace transition animations | Implemented | Synced to `UpdateEffects` IPC + `config.desktop` |
| Overview configuration & hot corners | Implemented | Persisted in `config.desktop` |
| Desktop right-click context menu toggle | Implemented | Persisted in `config.desktop` (backed by `neon-shell/src/context_menu.rs`) |
| Desktop file icons | Unsupported | Honestly labeled `Unsupported` (no icon grid layer in `neon-shell`) |

### 5.4 Dock & Panels (`src/pages/dock_panels.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Dock position, icon size, auto-hide, intelligent hide, magnification | Implemented | Persisted in `config.dock` |
| Dock opacity, background style (Glass/Solid/Minimal), indicators, grouping, recent, Trash | Implemented | Live `UpdateAppearance` IPC + `config.dock` |
| Top Panel visibility, position, height, opacity, clock, 24h, date, workspaces, tray, quick settings | Implemented | Live `UpdateAppearance` IPC + `config.panel` |
| Multi-panel schema (`extra_panels` Add/Remove) | Implemented | Forward-compatible `Vec<ExtraPanelConfig>` in `config.panel` |

### 5.5 Windows & Window Management (`src/pages/windows.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Click-to-focus / Focus-follows-mouse & snap gap | Implemented | Live `UpdateWindowBehavior` IPC |
| Raise on focus, placement policy, edge snapping, center dialogs | Implemented | Persisted in `config.windows` |
| Window button layout (macOS Left Traffic Lights vs Right Standard) | Implemented | Live GTK4 `gtk-decoration-layout` + `gsettings` |
| Double-click titlebar action, SSD toggle, shadows & inactive opacity | Implemented | Live `UpdateEffects` IPC + `config.windows` |
| Per-application window rules (always-on-top, maximized, workspace, monitor, min size) | Partially implemented | Full add/remove editor persisted in `config.windows.rules` |

### 5.6 Effects & Animations (`src/pages/effects.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| 3 Performance Presets (`Performance`, `Balanced`, `Visual Quality`) | Tested | `EffectsConfigData::apply_preset` + live `UpdateEffects` & `UpdateAppearance` IPC |
| Master effects toggle, reduced motion, blur, shadows, borders, corner radius, speed multiplier | Implemented | Live `UpdateEffects` IPC |
| Per-action open/close/minimize/maximize/workspace/overview/dock animation toggles | Implemented | Persisted in `config.effects` |

### 5.7 Input Devices (`src/pages/input.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Mouse speed, acceleration, Adaptive/Flat profile, left-handed, natural scroll, double-click, pointer size | Implemented | Pointer size live via IPC/gsettings; libinput prefs in `config.input` |
| Touchpad enable, speed, tap-to-click, two-finger scroll, click method, palm rejection, 3/4-finger gestures | Implemented | Persisted in `config.input` |
| Keyboard XKB layout, repeat rate/delay, Compose key, Caps Lock mapping | Implemented | Persisted in `config.input` |

### 5.8 Network (`src/pages/network.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Wi-Fi radio toggle & live NetworkManager status | Tested | `org.freedesktop.NetworkManager` D-Bus |
| Async Wi-Fi scanning, signal strength & security badges | Implemented | `nmcli` via `neon-system::network` |
| Secure Wi-Fi password dialog (never stored in plain text) | Implemented | `PasswordEntry` passed directly to NetworkManager |
| Saved connections (Wi-Fi, Ethernet, VPN) connect/disconnect | Implemented | `nmcli con up/down` |
| Active IPv4 address, gateway, DNS & MAC details | Implemented | Live `nmcli dev show` parser |
| System Proxy & Wi-Fi Hotspot GUI | Partially implemented | Marked `Partial` in UI |

### 5.9 Bluetooth (`src/pages/bluetooth.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Adapter power switch & discovery scan toggle | Implemented | `org.bluez.Adapter1` (`Powered`, `StartDiscovery`, `StopDiscovery`) |
| Paired & nearby device listing | Implemented | `org.freedesktop.DBus.ObjectManager.GetManagedObjects` |
| Pair, Connect, Disconnect & confirmed Remove device | Implemented | `org.bluez.Device1` (`Pair`, `Connect`, `Disconnect`) & `RemoveDevice` |

### 5.10 Sound (`src/pages/sound.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| PipeWire/WirePlumber output sink selection, master volume & mute | Tested | `wpctl` (`neon-system::audio`) |
| Speaker audio test | Implemented | Plays freedesktop stereo chime via `paplay`/`pw-play` |
| Input microphone device selection, gain & mute | Implemented | `wpctl` `@DEFAULT_AUDIO_SOURCE@` |
| Per-application stream volume sliders | Implemented | Live `wpctl status` Streams parser + `set_node_volume` |
| Channel balance & hardware card profile switcher | Partially implemented | Marked `Partial` in UI |

### 5.11 Notifications (`src/pages/notifications.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Freedesktop Notifications v1.2 bus status & Send Test Notification | Tested | `org.freedesktop.Notifications` D-Bus (`system_integration.rs`) |
| Enable/disable, Do Not Disturb, previews, sounds, lock-screen alerts, position, duration | Implemented | Persisted in `config.notifications` |
| Per-application notification muting | Implemented | Add/remove muted app list in `config.notifications.muted_apps` |

### 5.12 Power & Battery (`src/pages/power.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| UPower battery percentage, charging status & health capacity | Tested | `org.freedesktop.UPower` D-Bus |
| Power Mode selector (Power Saver / Balanced / Performance) | Implemented | `org.freedesktop.UPower.PowerProfiles` D-Bus |
| Screen dim, blank, suspend timeouts, lid-close, power button & low-battery alerts | Implemented | Persisted in `config.power` |

### 5.13 Privacy & Security (`src/pages/privacy.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Lock Screen Now (`ext_session_lock_v1`) & auto-lock timeout | Implemented | Live `LockSession` IPC + `config.privacy_accessibility` |
| IPC `SO_PEERCRED`, PolicyKit1 & XDG Desktop Portals live status | Tested | `neon-system::polkit` & `neon-system::portals` |
| Recent files & usage history toggles + confirmed Clear Recent Files | Implemented | Safely clears `~/.local/share/recently-used.xbel` |
| Remote desktop / screen-sharing server | Unsupported | Honestly labeled `Unsupported` |

### 5.14 Applications & Default Apps (`src/pages/applications.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Default Web Browser, Email, File Manager, Terminal & Media Player | Implemented | Live `xdg-mime query/default` (`neon-system::apps`) |
| Searchable Installed Applications list (XDG `.desktop`) | Implemented | Scans user & system XDG application dirs |
| One-click Add to Startup (`~/.config/autostart`) | Implemented | Writes compliant XDG autostart `.desktop` entry |

### 5.15 Users & Accounts (`src/pages/users.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Current user profile & local desktop accounts list | Implemented | `org.freedesktop.Accounts` D-Bus with `/etc/passwd` fallback |
| Polkit-gated display name editing (`SetRealName`) | Implemented | `org.freedesktop.Accounts.User.SetRealName` |
| Password changes & account creation/deletion | Requires Elevation | Labeled `Admin` (never handles user passwords directly) |

### 5.16 Keyboard Shortcuts (`src/pages/shortcuts.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Grouped shortcut categories (System, Windows, Workspaces, Applications, Media, Custom) | Tested | `default_shortcuts()` in `neon-config` |
| Searchable shortcut list, inline accelerator editor & live conflict detection | Tested | `find_shortcut_conflict` (`shortcut_conflict_detection_is_case_insensitive`) |
| Add custom shortcut, delete custom shortcut & Reset Defaults | Implemented | Atomic persistence via `neon-config` |

### 5.17 Accessibility (`src/pages/accessibility.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| High Contrast, Reduce Motion, Text Scale, Large Pointer (32 px) | Implemented | Live IPC + `gsettings` |
| Sticky Keys, Slow Keys, Bounce Keys, Mouse Keys | Implemented | Persisted in `config.privacy_accessibility` |
| Screen Reader (`orca`) & On-Screen Keyboard (`squeekboard`/`onboard`) | Implemented | Honest host binary detection (`Unavailable` when not installed) |
| Compositor Screen Magnifier | Unsupported | Honestly labeled `Unsupported` (no magnifier shader in `neon-renderer`) |

### 5.18 Startup & Session (`src/pages/startup_session.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| XDG Autostart listing & live enable/disable toggles | Implemented | Reads `/etc/xdg/autostart` & `~/.config/autostart`, writes user overrides cleanly |
| Session restore & active session info | Implemented | Persisted in `config.session` |
| Confirmed Suspend, Hibernate (capability-gated), Restart & Shut Down | Implemented | `org.freedesktop.login1.Manager` D-Bus with modal confirmation |

### 5.19 Date, Time & Language (`src/pages/datetime.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Automatic network time (NTP) toggle & sync status | Implemented | `org.freedesktop.timedate1` (`SetNTP`, `NTPSynchronized`) |
| Searchable IANA timezone selector | Implemented | `timedatectl list-timezones` + `timedate1.SetTimezone` |
| 12h/24h clock & date display format | Implemented | Synced with `config.panel` |
| System locale & installed locales inspection | Implemented | `localectl status` + `locale -a` (system-wide change marked `Admin`) |

### 5.20 Storage & Devices (`src/pages/storage.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| Disks, partitions, mount points & usage progress bars | Tested | `lsblk -P -b` + `df` (`parses_lsblk_output_and_formats_sizes`) |
| Safe removable drive Eject & auto-mount preferences | Implemented | `udisksctl unmount` + `power-off` |
| CUPS printers listing & default badge | Implemented | `lpstat -p -d -v` |
| SANE scanners detection | Implemented / Unsupported | Runs `scanimage -L` if installed; otherwise honestly marked `Unsupported` |

### 5.21 Updates & Software (`src/pages/updates.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| OS, kernel, package manager & installed package count | Implemented | `/etc/os-release`, `dpkg-query`/`rpm`/`pacman` |
| Async available updates check (`apt list --upgradable`) | Tested | `parse_apt_upgradable` (`parses_apt_upgradable_lines`) |
| Configured software repositories list & update notifications toggle | Implemented | Lists `/etc/apt/sources.list.d` + `config.session` |
| Native host updater launch | Implemented | Launches `update-manager` / `gnome-software` / `plasma-discover` without replacing APT |

### 5.22 About NEON DE (`src/pages/about.rs`)
| Feature | Status | Backend / Notes |
| --- | --- | --- |
| NEON DE version, architecture, OS, hostname, kernel, CPU, GPU (`lspci`), RAM, displays & uptime | Implemented | `src/system_info.rs` + live `GetOutputs` IPC |
| Live compositor diagnostics (backend, uptime, frames rendered, idle skips, frame time) | Implemented | Live `GetDiagnostics` IPC |
| Configuration Export, Import & confirmed Reset All Defaults | Implemented | Atomic TOML backup/restore + live full config sync |
