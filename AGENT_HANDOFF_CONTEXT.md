# NEON DE — Agent Memory & Model Handoff Context

> **For the next AI model / new conversation:** Read this file first. It is a complete context-compaction snapshot of the **NEON DE** workspace (`/home/satyajit/.gemini/antigravity/scratch/neon-de`), summarizing the architecture, everything implemented so far, host environment details, known quirks, verification state, and pending actions so you can continue seamlessly without re-discovering the project.

**Last updated:** 2026-11-24  
**Workspace root:** `/home/satyajit/.gemini/antigravity/scratch/neon-de`  
**User config file:** `~/.config/neon-de/config.toml` (do **not** silently overwrite)

---

## 1. Project Overview & Architecture

**NEON DE** is a custom Linux Wayland desktop environment written in **Rust (edition 2024)** using **Smithay 0.7** (DRM/KMS + EGL/Glow + libinput + XWayland) for the compositor and **GTK4 + gtk4-layer-shell** for the shell and settings UI.

### Workspace Crates (14 total)
| Crate | Role |
| --- | --- |
| `crates/neon-compositor` | Wayland compositor (`neon-compositor`) — DRM/KMS & Winit backends, input, IPC server, XDG shell, layer-shell, `zxdg_decoration_manager_v1` SSD titlebars, XWayland, `ext_session_lock_v1` |
| `crates/neon-renderer` | OpenGL/Glow renderer, shaders, rounded corners, drop shadows, blur, GPU-rendered macOS-style SSD titlebars, cursor rendering, screenshots |
| `crates/neon-scene` | Retained scene graph with layer ordering and dirty-region tracking |
| `crates/neon-window-manager` | Window placement, state machine, snapping/tiling geometry, per-app window rules |
| `crates/neon-workspaces` | Dynamic & fixed virtual workspace management |
| `crates/neon-output` | Multi-monitor layout, scaling, transform, mirroring, hotplug |
| `crates/neon-effects` | Animation curves, retargeting, reduced motion |
| `crates/neon-config` | Versioned (`CURRENT_SCHEMA_VERSION = 2`), clamped, atomic-write TOML config (`~/.config/neon-de/config.toml`) with automatic migration & corrupt-file backup |
| `crates/neon-ipc` | Authenticated Unix domain socket IPC (`0700` dir, `0600` socket, `SO_PEERCRED` same-UID check, bounded JSON frames, zero arbitrary shell exec) |
| `crates/neon-system` | Native Linux system adapters: NetworkManager (`zbus`/`nmcli`), BlueZ (`zbus`), PipeWire/WirePlumber (`wpctl`), UPower/PowerProfiles/logind (`zbus`), Notifications v1.2, Polkit, XDG Portals, Storage (`udisksctl`/`lsblk`/`df`/CUPS/SANE), DateTime (`timedate1`/`localectl`), Apps (`xdg-mime`/`.desktop`/autostart), Users (`AccountsService`), Updates (`apt`/`dpkg`) |
| `crates/neon-shell` | GTK4 + layer-shell desktop UI: top panel, floating dock, wallpaper, overview/launchpad, quick settings, native right-click context menu (`context_menu.rs`) |
| `crates/neon-settings` | Complete macOS-inspired × GNOME × KDE-depth GTK4 Settings app (22 categories, modular architecture, lazy page loading, global search, capability badges) |
| `crates/neon-lockscreen` | Wayland `ext_session_lock_v1` lock screen |
| `crates/neon-session` | Session launcher/supervisor (`neon-session`) with crash-loop recovery & XWayland `DISPLAY` export |

### Design Philosophy & Hard Constraints
- **Visual identity**: macOS System Settings polish (two-panel layout, sidebar, cards, traffic-light window controls, Light & Dark modes) + GNOME clarity + KDE depth — with **NEON DE's own branding** (do not copy Apple branding/assets verbatim).
- **Do NOT recreate the project** or replace the compositor, renderer, shell, or window manager.
- **Zero fake controls**: Never simulate successful changes or expose unsupported effects as working. Use honest capability states (`Active` / `Partial` / `Admin` / `Unavailable` / `Unsupported`).
- **Security**: Never run `neon-settings` as root, never store Wi-Fi or user passwords in plain text, never invoke `sh -c` with user input, use PolicyKit/D-Bus for privileged actions.
- **Live session safety**: The user is actively running **NEON-DE** right now — **never kill `neon-compositor`** and **never overwrite `~/.config/neon-de/config.toml` silently**.

---

## 2. Host Machine & Live Runtime Environment

- **OS**: Ubuntu 26.04.1 LTS (Linux), running live `NEON-DE` Wayland session.
- **GPUs**: Hybrid Intel UHD + NVIDIA GTX 1650 (DRM/KMS).
- **Active system services verified**: `NetworkManager`, `bluetooth` (BlueZ), `upower`, `udisks2`, `cups`, `accounts-daemon`, `systemd-timedated` are active; `systemd-localed` activates on demand via `localectl`.
- **Available CLI tools**: `nmcli`, `bluetoothctl`, `wpctl`, `pactl`, `upower`, `timedatectl`, `localectl`, `lpstat`, `udisksctl`, `lsblk`, `xdg-mime`, `apt`, `dpkg-query`, `powerprofilesctl`, `busctl`, `gsettings`, `xdg-open`, `pkexec`, `lspci`.
- **Installed custom assets / wrappers**:
  - macOS pointer cursor theme installed at `~/.local/share/icons/macOS` (configured to `20 px` default).
  - DaVinci Resolve launcher wrapper at `/home/satyajit/.local/bin/davinci-resolve` (auto-detects live XWayland `DISPLAY` so Resolve opens cleanly inside NEON DE).
- **Sudo**: Interactive password required (`sudo -n true` fails; no passwordless sudo). Always provide `sudo` install commands for the user to run when updating `/usr/local/bin/`.

---

## 3. Completed Work History

### A. Performance, DaVinci Resolve, macOS Chrome & Cursor (Installed in `/usr/local/bin/`)
1. **UI Lag / Responsiveness**:
   - Downscaled wallpapers & thumbnails via `gdk_pixbuf::Pixbuf::from_file_at_scale` instead of full-resolution image decoding.
   - VBlank-synchronized DRM frames + proper IPC fd registration in `neon-compositor`.
   - Widget diffing in `neon-shell` to prevent unnecessary rebuilds.
2. **DaVinci Resolve Launch Fix**:
   - Exported XWayland `DISPLAY` across `neon-compositor`, `neon-shell`, and `neon-session` + added `/home/satyajit/.local/bin/davinci-resolve`.
3. **Blur Controls & macOS Cursor**:
   - Added **Reduce Blur** + **Blur Intensity** (live IPC `UpdateEffects` + `UpdateAppearance`) and **Pointer Size** (`macOS` cursor at `20 px`).
4. **macOS Traffic-Light Buttons Everywhere**:
   - Styled GTK3/GTK4 CSS, `neon-settings`, and GPU-rendered SSD titlebars (`zxdg_decoration_manager_v1`) in `neon-compositor` + `neon-renderer` with macOS left traffic lights (Close/Minimize/Maximize).
5. **System-Wide Right-Click Context Menus**:
   - Implemented Wayland `xdg_popup` grabs + XWayland override-redirect popup rendering/hit-testing in `neon-compositor`.
   - Added native OS right-click context menu (`crates/neon-shell/src/context_menu.rs`) triggered via `IpcEvent::ContextMenuRequested`.

### B. Complete NEON DE Settings Application (Built & Tested — Ready to Install)
Refactored `crates/neon-settings` out of the old monolithic `pages.rs` / `pages_system.rs` / `style.rs` (deleted) into a clean modular crate covering **all 22 categories**:

- **Core modules**:
  - `src/main.rs` — Two-panel window, NEON DE sidebar brand header, persistent search (`Ctrl+F`/`Escape`), Back button (`Alt+Left`), lazy-built cached `Stack`, pinned `About NEON DE`, last-page memory, `--page <id>` CLI deep-linking.
  - `src/metadata.rs` — `CapabilityStatus`, `SettingMetadata`, `SETTINGS_REGISTRY` (45+ entries spanning all 22 categories), ranked `search_settings()`.
  - `src/navigation.rs` — All 22 `CategoryDef` definitions + `NavigationHistory` back-stack.
  - `src/state.rs` — `SettingsController` (atomic save, live IPC sync, category reset, status bar feedback, `apply_system_cursor`, `apply_toolkit_appearance`, `apply_window_button_layout`).
  - `src/ui/mod.rs` & `src/ui/style.rs` — `page_shell`, `boxed_card`, `toggle_row`, `slider_row`, `dropdown_row`, `info_row`, `capability_row`, `run_async` (background thread + GTK main-loop polling), `confirm_action_dialog`, and `display_confirm_countdown_dialog` (15s live timer with auto-revert).
  - `src/system_info.rs` — Real OS, hostname, kernel, arch, CPU, GPUs (`lspci`), RAM, uptime.
- **22 Page modules (`crates/neon-settings/src/pages/`)**:
  1. `appearance.rs` (5.1) — Light/Dark/Auto cards, accent swatches + custom hex, wallpaper gallery/picker/scaling/slideshow, Reduce Blur & intensity, corner radius, shadows, GTK/Icon/Cursor/Pointer size/Fonts/Density, Login screen (`RequiresElevation`).
  2. `displays.rs` (5.2) — Live `GetOutputs`, Cairo arrangement preview, scaling/orientation/primary with **15s confirmation countdown & auto-revert**, mirror/per-monitor workspaces, Night Light (`PartiallyImplemented`), HDR (`Unsupported`).
  3. `desktop.rs` (5.3) — Dynamic/fixed workspaces, names, grid columns, animations, Overview, hot corners, desktop right-click toggle, desktop icons (`Unsupported`).
  4. `dock_panels.rs` (5.4) — Full Dock & Top Panel controls + multi-panel `extra_panels` add/remove schema.
  5. `windows.rs` (5.5) — Focus policy, placement, snapping/gap, macOS vs right window buttons, titlebar double-click, SSD toggle, inactive opacity, and Per-Application Window Rules editor.
  6. `effects.rs` (5.6) — 3 presets (**Performance**, **Balanced**, **Visual Quality**) + live shader & per-action animation toggles.
  7. `input.rs` (5.7) — Segmented `Mouse | Touchpad | Keyboard` tabs.
  8. `network.rs` (5.8) — Async NetworkManager D-Bus + Wi-Fi scan + secure `PasswordEntry` dialog + saved connections + live IPv4/Gateway/DNS/MAC.
  9. `bluetooth.rs` (5.9) — Async BlueZ power, discovery scan, Pair/Connect/Disconnect/Remove.
  10. `sound.rs` (5.10) — Async PipeWire/WirePlumber sinks, volume/mute, Test Speakers, microphones, per-app stream volume sliders.
  11. `notifications.rs` (5.11) — D-Bus service status, Test Notification, DND, previews, position, duration, sounds, muted apps.
  12. `power.rs` (5.12) — UPower battery bar & health, PowerProfiles selector, dim/blank/suspend timeouts, lid/power button.
  13. `privacy.rs` (5.13) — Lock Screen Now (`ext_session_lock_v1`), auto-lock, IPC/Polkit/Portal status, recent files toggle + confirmed Clear Recent Files.
  14. `applications.rs` (5.14) — Real `xdg-mime` default Browser/Email/Files/Terminal/Media + searchable installed `.desktop` apps + Add to Startup.
  15. `users.rs` (5.15) — AccountsService D-Bus current user + polkit-gated display name edit + local accounts list.
  16. `shortcuts.rs` (5.16) — Grouped searchable Shortcut Manager + inline editor with **live conflict detection** + Add/Delete custom shortcuts + Reset.
  17. `accessibility.rs` (5.17) — High Contrast, Reduce Motion, Text Scale, Large Pointer (32px), typing aids, honest `orca`/`squeekboard` host check, Magnifier (`Unsupported`).
  18. `startup_session.rs` (5.18) — Real XDG autostart toggles + session restore + confirmed `systemd-logind` Suspend/Hibernate/Restart/Shutdown.
  19. `datetime.rs` (5.19) — `timedate1` NTP toggle, searchable IANA timezone selector, 12h/24h clock, `localectl` locale info.
  20. `storage.rs` (5.20) — `lsblk`/`df` disks with usage bars, `udisksctl` Eject, auto-mount prefs, CUPS printers, SANE scanner check.
  21. `updates.rs` (5.21) — OS/kernel/package count, async `apt list --upgradable`, APT sources, native updater launcher.
  22. `about.rs` (5.22) — NEON DE identity, hardware specs, live `GetDiagnostics`, Export/Import/Reset All Defaults.
- **Status tracker**: `SETTINGS_IMPLEMENTATION_STATUS.md` documents every feature's status (`Tested` / `Implemented` / `Partially implemented` / `Unsupported`).

### C. Desktop Cursor, Dock Box, Quick Settings Brightness, Power Menu & Icon Alignment Fixes (Built & Tested — Ready to Install)
1. **Desktop Mouse Cursor (Huge & Pixelated on Desktop)**:
   - Added Wayland `wp_cursor_shape_manager_v1` protocol (`CursorShapeManagerState` + `delegate_cursor_shape!` + `TabletSeatHandler`) in `crates/neon-compositor/src/{state.rs,wayland/mod.rs,wayland/seat.rs}` and set `self.needs_redraw = true` on `cursor_image` updates so GTK4 4.22 (`neon-shell` wallpaper/dock/panel) delegates directly to the compositor's crisp `macOS` `20 px` cursor.
   - Fixed `crates/neon-renderer/src/cursor.rs` & `lib.rs` to upload XCursor pixels as `Fourcc::Argb8888` (matching XCursor little-endian BGRA and Smithay's zero-copy DRM hardware cursor plane fast-path), expanded `CursorIcon` alias coverage, and added `logical_size` clamping when a strike differs from target size.
   - Set explicit `window.set_cursor_from_name(Some("default"))` on `crates/neon-shell/src/wallpaper.rs`.
2. **Dock Black Unrounded Box Behind Dock**:
   - Wrapped `.neon-dock-container` inside transparent `.neon-dock-wrapper` (`padding: 16px 24px`) in `crates/neon-shell/src/{dock.rs,style.rs}`, cleared layer-shell window background/shadow/border, replaced `auto_exclusive_zone_enable()` with fixed `set_exclusive_zone(74)`, and tightened `.neon-dock-container` and `.neon-tahoe-shell` shadows so GTK4 never hard-clips blurred shadows against rectangular layer surface bounds.
3. **Display Brightness in Quick Settings**:
   - Diagnosed `brightnessctl` failing with `Permission denied` (`satyajit` not in `video` group) and implemented unprivileged `org.freedesktop.login1.Session.SetBrightness` D-Bus brightness control in `crates/neon-system/src/power.rs` (with `GetSessionByPID`, sysfs, and `brightnessctl` fallbacks) wired to coalesced `SystemAction::SetBrightness(u32)` in `crates/neon-system/src/lib.rs` and `crates/neon-shell/src/quick_settings.rs`.
4. **Quick Settings Shutdown / Restart / Sleep / Log Out Power Controls**:
   - Added Row 7 power action bar (`Sleep`, `Log Out`, `Restart`, `Shut Down`) plus an inline confirmation card (`Cancel` / `Confirm`, danger red styling for Restart/Shut Down, auto-reset on close) in `crates/neon-shell/src/{quick_settings.rs,style.rs}` wired to `SystemAction::InvokePowerAction` & `SystemAction::LogOut` (`org.freedesktop.login1`).
5. **Liquid Glass Floating Control Center Redesign (`crates/neon-shell/src/{quick_settings.rs,style.rs,panel.rs}` + `crates/neon-system/src/media.rs`)**:
   - Replaced the old heavy dark outer-boxed Quick Settings with a floating 4-column **Liquid Glass** grid (`CELL = 70px`, `GAP = 12px`, `HALF = 152px`, `FULL = 316px`) matching macOS Tahoe Control Center geometry:
     - Top 2×2 block: Wi-Fi stadium pill (`152×70`, bright white active badge) + Bluetooth & Network quick circles (`70×70`, crisp white active state) on left; real **MPRIS2 Now Playing** (`org.mpris.MediaPlayer2.*` via `zbus` in `crates/neon-system/src/media.rs`) `152×152` card on right with album art & working Prev/Play-Pause/Next.
     - Row 3: Focus (DND) stadium pill + Overview (`sidebar-show-symbolic`) & Displays (`video-joined-displays-symbolic`) circles.
     - Rows 4–5: Full-width Display & Sound Liquid Glass cards with sleek `6px` white sliders, `18px` round white thumbs, aligned icons, and circular Sound Output button.
     - Row 6: 4 utility circles (`70×70`): Dark/Light Theme (custom Cairo contrast half-circle), Lock Screen, Power Profile cycler (with live Battery tooltip), Screenshot (`applets-screenshooter-symbolic`).
     - Row 7 + Bottom: Compact System Power glass pill (`Sleep`, `Log Out`, `Restart`, `Shut Down` + inline confirmation card) and centered `"Open Settings"` capsule pill.
   - **Smooth Spring Open/Close Animation**: Synchronized `.cc-closed` → `.cc-open` (`translateY(-18px) scale(0.92) opacity: 0` → `translateY(0) scale(1.0) opacity: 1`) via GTK's `add_tick_callback` (2 frame ticks + 55ms fallback) so Wayland always commits the initial frame before animating.
   - **Click-Outside-to-Close**: Converted Control Center to a fullscreen transparent `Layer::Overlay` click-catcher backdrop (`Edge::Top|Bottom|Left|Right`, `exclusive_zone = -1`, `GestureClick` `pick()` outside `cc_surface`) plus `Escape` key dismissal.
6. **Fullscreen Blurred App Drawer / Launchpad with Top Bar & Dock Chrome Fade Choreography (`crates/neon-shell/src/{overview.rs,panel.rs,dock.rs,main.rs,style.rs}`)**:
   - Placed `neon-overview` on `Layer::Top` with `exclusive_zone(-1)` (100% fullscreen edge-to-edge blurred wallpaper beneath the Top Bar and Dock), minimalist centered translucent Search pill, 7×5 paged `72px` app icon grid, centered circular pagination dots above the dock, and discreet bottom-right `"Windows"` toggle.
   - Coordinated via `Overview::set_chrome_hooks` (`on_layer_elevate`, `on_bg_hidden`):
     - **On Open**: Elevates Top Bar & Dock live to `Layer::Overlay` (above the App Drawer), adds `.drawer-open` to fade their backgrounds/borders/shadows smoothly to transparent (`320ms`), and plays a frame-clock-synced (`add_tick_callback`) fullscreen blur + zoom-in animation (`280ms`/`300ms`).
     - **On Close** (manual `Esc`/click empty space/toggle OR automatic close when launching an app from the drawer or dock): Frame-synchronized simultaneous `280ms cubic-bezier(0.22, 1.0, 0.36, 1.0)` crossfade — blur fades off and Top Bar & Dock backgrounds fade back in on the exact same frame with identical duration.
   - **60 FPS App Drawer & Top-Aligned Pages (`wallpaper.rs`, `overview.rs`, `style.rs`)**: Replaced live per-frame CSS `filter: blur(...)` with one-time CPU 3-pass separable box blur (`load_blurred_texture` -> static `gdk::MemoryTexture`, CSS `filter: none;`), anchored all pages (Page 1, Page 2+, newly installed apps) cleanly to Row 0 Col 0 with 7-column spacers, and removed the bottom-right `"Windows"` button.
7. **Top Bar `NEON` Workspaces Panel + Up to 10 Desktops (`workspaces_panel.rs`, `panel.rs`, `neon-workspaces`, `neon-window-manager`, `neon-ipc`, `neon-compositor`)**:
   - Wired Top Bar `NEON` icon to open the new Liquid Glass `WorkspacesPanel` (click-outside + Esc dismiss, smooth open/close animation) displaying all desktops (`1 / 10`..`10 / 10`), inline `+ Add Desktop` button & `+ New Desktop` card (`IpcRequest::AddWorkspace`, `MAX_WORKSPACES = 10` with pinned persistence), remove desktop button (`IpcRequest::RemoveWorkspace`), and open windows list.
8. **Material 3 Expressive Spring & Hyprland-Style Window Animations (`neon-effects`, `neon-scene`, `neon-renderer`, `neon-compositor`)**:
   - Added official Material 3 Expressive Motion Scheme spring tokens (`M3_EXPRESSIVE_{FAST,DEFAULT,SLOW}_{SPATIAL,EFFECTS}`) in `neon-effects`, GPU `RescaleRenderElement` center-anchored scaling in `neon-scene` + `neon-renderer`, and full 60 FPS animations for **Open** (pop-in spring), **Close** (pop-out shrink+fade with cancel-close safety for Save dialogs), **Minimize / Restore** (macOS dock swoop down & spring rise up), **Maximize / Unmaximize / Snap / Fullscreen / Resize** (automatic Hyprland-style spatial rect morph on geometry change), and **Workspace Switch** (M3 Slow Spatial horizontal slide).

---

## 4. Build, Test & Install State (IMPORTANT GOTCHAS)

### Test Status
- `cargo test --workspace` passes **60/60 tests** across all 14 crates (`neon-config`: 5, `neon-system`: 14, `neon-settings`: 5, `neon-compositor`: 9, `neon-window-manager`: 6, `neon-effects`: 5, `neon-workspaces`: 4, `neon-renderer`: 3, `neon-ipc`: 2, `neon-output`: 2, `neon-scene`: 2, `neon-shell`: 2, `neon-session`: 1).

### Known Cargo `target/` Permission Gotcha
- Earlier `sudo scripts/install.sh` left root-owned build artifacts inside `target/release/deps/`.
- `target/debug` is owned by `satyajit`, so `cargo test --workspace` works normally.
- For non-root release builds, use:
  ```bash
  CARGO_TARGET_DIR=/home/satyajit/.gemini/antigravity/scratch/neon-de/target-user cargo build -p neon-compositor -p neon-shell -p neon-settings --release
  ```
  (Or have the user run `sudo chown -R satyajit:satyajit /home/satyajit/.gemini/antigravity/scratch/neon-de/target`.)

### Pending Install Action
- Freshly compiled release binaries are ready in `/home/satyajit/.gemini/antigravity/scratch/neon-de/target-user/release/`:
  - `neon-compositor` (14 MB)
  - `neon-shell` (6.7 MB)
  - `neon-settings` (8.3 MB)
- Install command for the user:
  ```bash
  sudo install -m 0755 \
    /home/satyajit/.gemini/antigravity/scratch/neon-de/target-user/release/neon-compositor \
    /home/satyajit/.gemini/antigravity/scratch/neon-de/target-user/release/neon-shell \
    /home/satyajit/.gemini/antigravity/scratch/neon-de/target-user/release/neon-settings \
    /usr/local/bin/
  ```
- Reloading: `pkill -x neon-shell` immediately reloads the fixed Dock & Quick Settings (auto-restarted by `neon-session`); logging out & back in activates the new `neon-compositor` (`wp_cursor_shape_v1` + hardware `Argb8888` cursor).
- Note: `git status` currently has all of the above changes uncommitted in the working tree (no git commit was requested).

---

## 5. Natural Next Steps (If Requested by the User)

1. **Install & visually test** the new `neon-settings` binary in the live NEON DE session.
2. **Wire remaining `PartiallyImplemented` runtime hooks** into `neon-compositor` / `neon-renderer` / `neon-shell`:
   - Live DRM gamma LUT for Night Light (`config.displays.night_light_enabled` + `color_temperature_k`).
   - Live runtime reload of `config.input` (libinput pointer speed, accel profile, natural scroll, touchpad settings, XKB layouts) and `config.input.shortcuts` in `neon-compositor/src/input.rs`.
   - Enforce `config.windows.rules` (always-on-top, start maximized, workspace assignment, min/max size) in `neon-compositor` + `neon-window-manager`.
   - Render user-created `config.panel.extra_panels` in `neon-shell`.
   - Fix the single minor dead-code warning (`SettingMetadata.id` in `crates/neon-settings/src/metadata.rs`) by wiring row-level highlight scrolling on search jump.
3. **Commit changes to Git** once the user verifies everything visually.
