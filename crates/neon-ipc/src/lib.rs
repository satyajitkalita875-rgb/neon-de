//! Versioned, authenticated Unix domain socket IPC for **NEON DE**.
//!
//! # Security Guarantees
//! - Socket directory is created with `0700` permissions; socket file is `0600`.
//! - Every connecting peer is verified via Linux `SO_PEERCRED` to enforce same-UID access.
//! - Framed UTF-8 JSON messages are strictly size-bounded (`MAX_FRAME_BYTES`) and schema-validated.
//! - Zero arbitrary shell command execution endpoints exist in the protocol.

use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::fd::AsRawFd,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// Current NEON DE IPC wire protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum accepted single JSON line frame size (256 KiB).
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// Computes the standard isolated IPC socket path for a given `WAYLAND_DISPLAY` socket name.
pub fn default_socket_path(wayland_display: &str) -> PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", current_uid())));
    // Keep only a safe single-segment filename from wayland_display.
    let sanitized: String = wayland_display
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    let suffix = if sanitized.is_empty() {
        "default"
    } else {
        &sanitized
    };
    runtime_dir
        .join("neon-de")
        .join(format!("ipc-{suffix}.sock"))
}

/// Returns the current process effective UID.
pub fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

/// Verified Linux `SO_PEERCRED` peer credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// Reads Linux `SO_PEERCRED` from a connected `UnixStream` and verifies same-UID ownership.
pub fn verify_peer_credentials(stream: &UnixStream) -> io::Result<PeerCredentials> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let expected_uid = current_uid();
    if cred.uid != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "IPC peer UID {} does not match compositor UID {}",
                cred.uid, expected_uid
            ),
        ));
    }
    Ok(PeerCredentials {
        pid: cred.pid,
        uid: cred.uid,
        gid: cred.gid,
    })
}

/// Binds a Unix domain socket listener with restricted `0700` directory and `0600` socket permissions.
pub fn bind_ipc_listener(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Event subscription topics supported by `neon-compositor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventTopic {
    Windows,
    Workspaces,
    Outputs,
    Overview,
    Effects,
}

/// Snapshot of a single managed toplevel window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    pub workspace_id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub focused: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default)]
    pub tiled: bool,
}

/// Snapshot of a dynamic workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub index: usize,
    pub active: bool,
    #[serde(default)]
    pub pinned: bool,
    pub window_ids: Vec<u64>,
}

/// Snapshot of a logical output/monitor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub refresh_mhz: i32,
    pub primary: bool,
    pub transform: String,
}

/// Snapshot of live compositor effects settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectsInfo {
    pub enabled: bool,
    pub reduced_motion: bool,
    pub shadows_enabled: bool,
    pub borders_enabled: bool,
    pub blur_enabled: bool,
    pub blur_strength: f32,
    pub inactive_opacity: f32,
    pub corner_radius: f32,
    pub speed_scale: f64,
    #[serde(default = "default_true")]
    pub open_animation: bool,
    #[serde(default = "default_true")]
    pub close_animation: bool,
    #[serde(default = "default_true")]
    pub minimize_animation: bool,
    #[serde(default = "default_true")]
    pub maximize_animation: bool,
    #[serde(default = "default_true")]
    pub workspace_animation: bool,
    #[serde(default = "default_true")]
    pub overview_animation: bool,
    #[serde(default = "default_true")]
    pub dock_animation: bool,
}

impl Default for EffectsInfo {
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

/// Appearance & effects payload revision emitted by current `neon-compositor` builds.
/// Older pre-restart compositors omit or emit a lower revision, allowing `neon-shell`
/// to gracefully overlay newly added shell preferences from XDG config until next login.
pub const APPEARANCE_PROTOCOL_REV: u32 = 7;

/// Screen-space Liquid Glass region reported by `neon-shell` for `neon-dynamic-island`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlassShapeRect {
    /// Horizontal offset inside the `neon-dynamic-island` layer surface (logical px).
    pub x: f32,
    /// Vertical offset inside the `neon-dynamic-island` layer surface (logical px).
    pub y: f32,
    /// Width of the glass capsule (logical px).
    pub width: f32,
    /// Height of the glass capsule (logical px).
    pub height: f32,
    /// Top corner radius (logical px).
    pub top_radius: f32,
    /// Bottom corner radius (logical px).
    pub bottom_radius: f32,
    /// Optional concave top outer ear radius (`0.0` for floating Mac pills).
    pub ear_radius: f32,
    /// Overall visibility alpha (`0.0..=1.0`).
    pub alpha: f32,
    /// Optional subtle accent/album RGB tint (`0.0..=1.0` per channel).
    pub tint_rgb: [f32; 3],
    /// Strength of `tint_rgb` blend (`0.0..=1.0`).
    pub tint_strength: f32,
}

fn default_blur_strength() -> f32 {
    1.0
}

fn default_dock_position() -> String {
    "bottom".into()
}

fn default_dock_icon_size() -> u32 {
    36
}

fn default_dock_height() -> u32 {
    52
}

fn default_panel_mode() -> String {
    "top_and_dock".into()
}

fn default_true() -> bool {
    true
}

fn default_desktop_sort_by() -> String {
    "name".into()
}

/// Snapshot of live appearance, dock, desktop, and panel shell settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppearanceInfo {
    #[serde(default)]
    pub protocol_rev: u32,
    pub theme: String,
    pub accent_color: String,
    pub dock_transparency: f32,
    #[serde(default)]
    pub dock_auto_hide: bool,
    #[serde(default)]
    pub dock_smart_hide: bool,
    #[serde(default = "default_dock_position")]
    pub dock_position: String,
    #[serde(default = "default_dock_icon_size")]
    pub dock_icon_size: u32,
    #[serde(default = "default_dock_height")]
    pub dock_height: u32,
    #[serde(default = "default_true")]
    pub dock_magnification: bool,
    #[serde(default)]
    pub dock_pinned_apps: Vec<String>,
    pub panel_transparency: f32,
    pub panel_visible: bool,
    #[serde(default)]
    pub panel_smart_hide: bool,
    #[serde(default = "default_panel_mode")]
    pub panel_mode: String,
    #[serde(default = "default_true")]
    pub dynamic_island_enabled: bool,
    pub show_clock: bool,
    #[serde(default = "default_true")]
    pub clock_24h: bool,
    pub show_workspaces: bool,
    #[serde(default = "default_true")]
    pub show_app_title: bool,
    #[serde(default = "default_true")]
    pub show_system_tray: bool,
    #[serde(default = "default_true")]
    pub show_battery: bool,
    #[serde(default = "default_true")]
    pub show_quick_settings: bool,
    #[serde(default = "default_true")]
    pub show_desktop_icons: bool,
    #[serde(default = "default_desktop_sort_by")]
    pub desktop_sort_by: String,
    #[serde(default = "default_true")]
    pub desktop_auto_arrange: bool,
    #[serde(default)]
    pub high_contrast: bool,
    #[serde(default)]
    pub wallpaper: Option<String>,
    #[serde(default)]
    pub reduce_blur: bool,
    #[serde(default = "default_blur_strength")]
    pub blur_strength: f32,
    #[serde(default)]
    pub cursor_theme: Option<String>,
    #[serde(default)]
    pub cursor_size: Option<u32>,
}

impl Default for AppearanceInfo {
    fn default() -> Self {
        Self {
            protocol_rev: APPEARANCE_PROTOCOL_REV,
            theme: "neon_dark".into(),
            accent_color: "#38BDF8".into(),
            dock_transparency: 0.78,
            dock_auto_hide: false,
            dock_smart_hide: false,
            dock_position: default_dock_position(),
            dock_icon_size: default_dock_icon_size(),
            dock_height: default_dock_height(),
            dock_magnification: true,
            dock_pinned_apps: Vec::new(),
            panel_transparency: 0.76,
            panel_visible: true,
            panel_smart_hide: false,
            panel_mode: default_panel_mode(),
            dynamic_island_enabled: true,
            show_clock: true,
            clock_24h: true,
            show_workspaces: true,
            show_app_title: true,
            show_system_tray: true,
            show_battery: true,
            show_quick_settings: true,
            show_desktop_icons: true,
            desktop_sort_by: default_desktop_sort_by(),
            desktop_auto_arrange: true,
            high_contrast: false,
            wallpaper: None,
            reduce_blur: false,
            blur_strength: 1.0,
            cursor_theme: Some("macOS".into()),
            cursor_size: Some(20),
        }
    }
}

/// Ergonomic partial patch builder for [`IpcRequest::UpdateAppearance`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppearanceUpdate {
    pub theme: Option<String>,
    pub accent_color: Option<String>,
    pub dock_transparency: Option<f32>,
    pub dock_auto_hide: Option<bool>,
    pub dock_smart_hide: Option<bool>,
    pub dock_position: Option<String>,
    pub dock_icon_size: Option<u32>,
    pub dock_height: Option<u32>,
    pub dock_magnification: Option<bool>,
    pub dock_pinned_apps: Option<Vec<String>>,
    pub panel_transparency: Option<f32>,
    pub panel_visible: Option<bool>,
    pub panel_smart_hide: Option<bool>,
    pub panel_mode: Option<String>,
    pub dynamic_island_enabled: Option<bool>,
    pub show_clock: Option<bool>,
    pub clock_24h: Option<bool>,
    pub show_workspaces: Option<bool>,
    pub show_app_title: Option<bool>,
    pub show_system_tray: Option<bool>,
    pub show_battery: Option<bool>,
    pub show_quick_settings: Option<bool>,
    pub show_desktop_icons: Option<bool>,
    pub desktop_sort_by: Option<String>,
    pub desktop_auto_arrange: Option<bool>,
    pub high_contrast: Option<bool>,
    pub wallpaper: Option<String>,
    pub reduce_blur: Option<bool>,
    pub blur_strength: Option<f32>,
    pub cursor_theme: Option<String>,
    pub cursor_size: Option<u32>,
}

impl From<AppearanceUpdate> for IpcRequest {
    fn from(u: AppearanceUpdate) -> Self {
        Self::UpdateAppearance {
            theme: u.theme,
            accent_color: u.accent_color,
            dock_transparency: u.dock_transparency,
            dock_auto_hide: u.dock_auto_hide,
            dock_smart_hide: u.dock_smart_hide,
            dock_position: u.dock_position,
            dock_icon_size: u.dock_icon_size,
            dock_height: u.dock_height,
            dock_magnification: u.dock_magnification,
            dock_pinned_apps: u.dock_pinned_apps,
            panel_transparency: u.panel_transparency,
            panel_visible: u.panel_visible,
            panel_smart_hide: u.panel_smart_hide,
            panel_mode: u.panel_mode,
            dynamic_island_enabled: u.dynamic_island_enabled,
            show_clock: u.show_clock,
            clock_24h: u.clock_24h,
            show_workspaces: u.show_workspaces,
            show_app_title: u.show_app_title,
            show_system_tray: u.show_system_tray,
            show_battery: u.show_battery,
            show_quick_settings: u.show_quick_settings,
            show_desktop_icons: u.show_desktop_icons,
            desktop_sort_by: u.desktop_sort_by,
            desktop_auto_arrange: u.desktop_auto_arrange,
            high_contrast: u.high_contrast,
            wallpaper: u.wallpaper,
            reduce_blur: u.reduce_blur,
            blur_strength: u.blur_strength,
            cursor_theme: u.cursor_theme,
            cursor_size: u.cursor_size,
        }
    }
}

/// Live compositor frame-timing, uptime, and runtime diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsInfo {
    pub backend: String,
    pub uptime_secs: u64,
    pub frames_rendered: u64,
    pub idle_frames_skipped: u64,
    pub last_frame_time_us: u64,
    pub window_count: usize,
    pub workspace_count: usize,
    pub output_count: usize,
    pub safe_mode: bool,
}

fn default_button_layout() -> String {
    "left".into()
}

fn default_tiling_layout() -> String {
    "dwindle".into()
}

fn default_tiling_gap() -> i32 {
    6
}

/// Snapshot of live window manager behavior settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowBehaviorInfo {
    pub focus_follows_mouse: bool,
    pub snap_gap: i32,
    #[serde(default = "default_tiling_gap")]
    pub tiling_gap: i32,
    #[serde(default = "default_button_layout")]
    pub button_layout: String,
    #[serde(default)]
    pub auto_tiling: bool,
    #[serde(default = "default_tiling_layout")]
    pub tiling_layout: String,
    #[serde(default = "default_true")]
    pub auto_snap: bool,
    #[serde(default = "default_true")]
    pub snap_layouts_bar: bool,
}

impl Default for WindowBehaviorInfo {
    fn default() -> Self {
        Self {
            focus_follows_mouse: false,
            snap_gap: 6,
            tiling_gap: default_tiling_gap(),
            button_layout: default_button_layout(),
            auto_tiling: false,
            tiling_layout: default_tiling_layout(),
            auto_snap: true,
            snap_layouts_bar: true,
        }
    }
}

/// Complete desktop state snapshot consumed by `neon-shell` and `neon-settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopSnapshot {
    pub active_workspace_index: usize,
    pub overview_open: bool,
    pub locked: bool,
    pub windows: Vec<WindowInfo>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub outputs: Vec<OutputInfo>,
    pub effects: EffectsInfo,
    pub appearance: AppearanceInfo,
    pub window_behavior: WindowBehaviorInfo,
    #[serde(default)]
    pub xwayland_display: Option<String>,
}

/// Typed request envelope sent by IPC clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcRequestEnvelope {
    pub version: u32,
    pub id: u64,
    pub request: IpcRequest,
}

/// All validated operations exposed by `neon-compositor` over IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    GetState,
    GetDiagnostics,
    GetWindows,
    GetWorkspaces,
    GetOutputs,
    SwitchWorkspace {
        index: usize,
    },
    AddWorkspace,
    RemoveWorkspace {
        index: usize,
    },
    TogglePinWorkspace {
        index: usize,
    },
    MoveWindowToWorkspace {
        window_id: u64,
        workspace_index: usize,
    },
    FocusWindow {
        window_id: u64,
    },
    CloseWindow {
        window_id: u64,
    },
    MinimizeWindow {
        window_id: u64,
    },
    RestoreWindow {
        window_id: u64,
    },
    ToggleMaximizeWindow {
        window_id: u64,
    },
    ToggleFullscreenWindow {
        window_id: u64,
    },
    SnapWindow {
        window_id: u64,
        region: String,
    },
    ToggleAlwaysOnTop {
        window_id: u64,
    },
    CenterWindow {
        window_id: u64,
    },
    SetFloatingWindow {
        window_id: u64,
    },
    ToggleWindowFloat {
        #[serde(default)]
        window_id: Option<u64>,
    },
    MoveWindowToOutput {
        window_id: u64,
        output_name: String,
    },
    SetWindowDockAnchor {
        window_id: u64,
        x: i32,
        y: i32,
    },
    /// Records the exact screen coordinates of a clicked Dock, App Drawer, or Desktop icon
    /// so the next newly mapped application window opens with an Android-style Material 3
    /// Expressive container-transform spring directly out of that icon.
    SetLaunchOrigin {
        x: i32,
        y: i32,
    },
    ToggleOverview,
    SetOverview {
        open: bool,
    },
    LockSession,
    OpenScreenshotUi,
    ToggleClipboard,
    ToggleDynamicIsland,
    AnimateOverlay {
        namespace: String,
        opening: bool,
    },
    TakeScreenshot {
        path: Option<String>,
        #[serde(default)]
        region: Option<[i32; 4]>,
    },
    GetScreencastTarget,
    SetScreencastTarget {
        target: ScreencastTarget,
    },
    CaptureScreencastPreview {
        target: ScreencastTarget,
        path: String,
        #[serde(default)]
        max_width: Option<u32>,
        #[serde(default)]
        max_height: Option<u32>,
    },
    ConfigureOutput {
        name: String,
        x: Option<i32>,
        y: Option<i32>,
        scale: Option<f64>,
        primary: Option<bool>,
        transform: Option<String>,
    },
    UpdateEffects {
        enabled: Option<bool>,
        reduced_motion: Option<bool>,
        shadows_enabled: Option<bool>,
        borders_enabled: Option<bool>,
        blur_enabled: Option<bool>,
        blur_strength: Option<f32>,
        inactive_opacity: Option<f32>,
        corner_radius: Option<f32>,
        speed_scale: Option<f64>,
        #[serde(default)]
        open_animation: Option<bool>,
        #[serde(default)]
        close_animation: Option<bool>,
        #[serde(default)]
        minimize_animation: Option<bool>,
        #[serde(default)]
        maximize_animation: Option<bool>,
        #[serde(default)]
        workspace_animation: Option<bool>,
        #[serde(default)]
        overview_animation: Option<bool>,
        #[serde(default)]
        dock_animation: Option<bool>,
    },
    UpdateAppearance {
        theme: Option<String>,
        accent_color: Option<String>,
        dock_transparency: Option<f32>,
        #[serde(default)]
        dock_auto_hide: Option<bool>,
        #[serde(default)]
        dock_smart_hide: Option<bool>,
        #[serde(default)]
        dock_position: Option<String>,
        #[serde(default)]
        dock_icon_size: Option<u32>,
        #[serde(default)]
        dock_height: Option<u32>,
        #[serde(default)]
        dock_magnification: Option<bool>,
        #[serde(default)]
        dock_pinned_apps: Option<Vec<String>>,
        panel_transparency: Option<f32>,
        panel_visible: Option<bool>,
        #[serde(default)]
        panel_smart_hide: Option<bool>,
        #[serde(default)]
        panel_mode: Option<String>,
        #[serde(default)]
        dynamic_island_enabled: Option<bool>,
        show_clock: Option<bool>,
        #[serde(default)]
        clock_24h: Option<bool>,
        show_workspaces: Option<bool>,
        #[serde(default)]
        show_app_title: Option<bool>,
        #[serde(default)]
        show_system_tray: Option<bool>,
        #[serde(default)]
        show_battery: Option<bool>,
        #[serde(default)]
        show_quick_settings: Option<bool>,
        #[serde(default)]
        show_desktop_icons: Option<bool>,
        #[serde(default)]
        desktop_sort_by: Option<String>,
        #[serde(default)]
        desktop_auto_arrange: Option<bool>,
        #[serde(default)]
        high_contrast: Option<bool>,
        #[serde(default)]
        wallpaper: Option<String>,
        #[serde(default)]
        reduce_blur: Option<bool>,
        #[serde(default)]
        blur_strength: Option<f32>,
        #[serde(default)]
        cursor_theme: Option<String>,
        #[serde(default)]
        cursor_size: Option<u32>,
    },
    UpdateWindowBehavior {
        focus_follows_mouse: Option<bool>,
        snap_gap: Option<i32>,
        #[serde(default)]
        tiling_gap: Option<i32>,
        #[serde(default)]
        button_layout: Option<String>,
        #[serde(default)]
        auto_tiling: Option<bool>,
        #[serde(default)]
        tiling_layout: Option<String>,
        #[serde(default)]
        auto_snap: Option<bool>,
        #[serde(default)]
        snap_layouts_bar: Option<bool>,
    },
    Subscribe {
        topics: Vec<EventTopic>,
    },
    /// Set screen-space Liquid Glass shapes for the `neon-dynamic-island` layer surface.
    SetDynamicIslandGlass {
        regions: Vec<GlassShapeRect>,
    },
}

/// Structured server message written back to IPC clients (either a response or pushed event).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IpcServerMessage {
    Response {
        version: u32,
        id: u64,
        result: IpcResult,
    },
    Event {
        version: u32,
        event: IpcEvent,
    },
}

/// Result payload of an IPC request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IpcResult {
    Ok { payload: IpcPayload },
    Err { code: String, message: String },
}

/// Typed response payload variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "payload_type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum IpcPayload {
    Ack,
    State { snapshot: DesktopSnapshot },
    Diagnostics { diagnostics: DiagnosticsInfo },
    Windows { windows: Vec<WindowInfo> },
    Workspaces { workspaces: Vec<WorkspaceInfo> },
    Outputs { outputs: Vec<OutputInfo> },
    Overview { open: bool },
    Effects { effects: EffectsInfo },
    Appearance { appearance: AppearanceInfo },
    WindowBehavior { behavior: WindowBehaviorInfo },
    Screenshot { path: String },
    ScreencastTarget { target: ScreencastTarget },
}

/// Active live capture target for PipeWire / `zwlr_screencopy_manager_v1` DMA-BUF screencasts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScreencastTarget {
    /// Capture an entire physical display/monitor by connector name (or primary if omitted).
    Output { name: Option<String> },
    /// Capture a single managed toplevel window isolated from all other windows and workspaces.
    Window { window_id: u64 },
    /// Capture a specific desktop/workspace by zero-based index (even while viewing another).
    Workspace { index: usize },
}

impl Default for ScreencastTarget {
    fn default() -> Self {
        Self::Output { name: None }
    }
}

/// Target surface kind for an OS-level right-click context menu requested by the compositor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextMenuTarget {
    Desktop,
    Window { window_id: u64 },
}

/// Asynchronous state-change events pushed to subscribed clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum IpcEvent {
    StateChanged {
        snapshot: DesktopSnapshot,
    },
    OverviewToggled {
        open: bool,
    },
    ScreenshotUiRequested,
    ClipboardToggleRequested,
    DynamicIslandToggleRequested,
    DragStateChanged {
        active: bool,
    },
    ContextMenuRequested {
        x: i32,
        y: i32,
        target: ContextMenuTarget,
    },
}

/// Encodes a serializable IPC message as a newline-delimited JSON line.
pub fn encode_frame<T: Serialize>(msg: &T) -> io::Result<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec(msg).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if bytes.len() + 1 > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame exceeds MAX_FRAME_BYTES",
        ));
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Reads a single bounded newline-delimited JSON frame from a buffered reader.
pub fn read_frame<R: BufRead, T: for<'de> Deserialize<'de>>(reader: &mut R) -> io::Result<T> {
    let mut line = String::new();
    let mut total = 0usize;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "IPC connection closed",
            ));
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let chunk = &buf[..=pos];
            total += chunk.len();
            if total > MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "IPC frame exceeds MAX_FRAME_BYTES",
                ));
            }
            let s = std::str::from_utf8(chunk)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            line.push_str(s);
            let consume = pos + 1;
            reader.consume(consume);
            break;
        } else {
            total += buf.len();
            if total > MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "IPC frame exceeds MAX_FRAME_BYTES",
                ));
            }
            let s = std::str::from_utf8(buf)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            line.push_str(s);
            let len = buf.len();
            reader.consume(len);
        }
    }

    serde_json::from_str(line.trim_end())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Synchronous IPC client used by `neon-shell`, `neon-settings`, and tests.
#[derive(Debug)]
pub struct IpcClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

impl IpcClient {
    /// Connects to a NEON DE IPC Unix domain socket at `path`.
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let writer = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
        })
    }

    /// Sends a typed [`IpcRequest`] and waits for its matching [`IpcResult`].
    pub fn request(&mut self, request: IpcRequest) -> io::Result<IpcResult> {
        let id = self.next_id;
        self.next_id += 1;
        let env = IpcRequestEnvelope {
            version: PROTOCOL_VERSION,
            id,
            request,
        };
        let frame = encode_frame(&env)?;
        self.writer.write_all(&frame)?;
        self.writer.flush()?;

        loop {
            match read_frame::<_, IpcServerMessage>(&mut self.reader)? {
                IpcServerMessage::Response {
                    id: resp_id,
                    result,
                    ..
                } if resp_id == id => return Ok(result),
                IpcServerMessage::Event { .. } | IpcServerMessage::Response { .. } => {
                    // Ignore interleaved events while awaiting a specific request response.
                }
            }
        }
    }

    /// Subscribes to event topics and returns the underlying blocking reader for pushed events.
    pub fn subscribe(&mut self, topics: Vec<EventTopic>) -> io::Result<IpcResult> {
        self.request(IpcRequest::Subscribe { topics })
    }

    /// Reads the next pushed [`IpcServerMessage`] (intended for dedicated subscriber threads).
    pub fn read_next_message(&mut self) -> io::Result<IpcServerMessage> {
        read_frame(&mut self.reader)
    }

    /// Removes read timeout on this connection so a subscriber thread can block indefinitely.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn peer_credentials_and_roundtrip_ipc_request_work() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("neon-test.sock");
        let listener = bind_ipc_listener(&sock_path).unwrap();
        listener.set_nonblocking(false).unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let creds = verify_peer_credentials(&stream).unwrap();
            assert_eq!(creds.uid, current_uid());

            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let req: IpcRequestEnvelope = read_frame(&mut reader).unwrap();
            assert_eq!(req.version, PROTOCOL_VERSION);
            assert_eq!(req.request, IpcRequest::ToggleOverview);

            let resp = IpcServerMessage::Response {
                version: PROTOCOL_VERSION,
                id: req.id,
                result: IpcResult::Ok {
                    payload: IpcPayload::Overview { open: true },
                },
            };
            stream.write_all(&encode_frame(&resp).unwrap()).unwrap();
            stream.flush().unwrap();
        });

        let mut client = IpcClient::connect(&sock_path).unwrap();
        let res = client.request(IpcRequest::ToggleOverview).unwrap();
        assert_eq!(
            res,
            IpcResult::Ok {
                payload: IpcPayload::Overview { open: true }
            }
        );
        server.join().unwrap();
    }

    #[test]
    fn default_socket_path_sanitizes_wayland_display() {
        let p = default_socket_path("../../evil/wayland-2");
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("ipc-"));
        assert!(!name.contains('/'));
    }
}
