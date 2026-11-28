//! Central compositor state (`NeonState`) integrating Smithay, `neon-window-manager`,
//! `neon-workspaces`, `neon-scene`, and `neon-effects`.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::Duration,
};

use neon_config::{
    DockPosition, FocusMode, NeonConfig, ThemeMode, TilingLayout as ConfigTilingLayout,
    WindowButtonLayout,
};
use neon_effects::{EffectsConfig, EffectsManager};
use neon_output::{DisplayMode, OutputConfigPatch, OutputId, OutputManager, OutputTransform};
use neon_scene::{
    BorderStyle, MACOS_TITLEBAR_HEIGHT, SceneGraph, SceneLayer, ShadowStyle, TitlebarStyle,
};
use neon_window_manager::tiling::TilingLayout as WmTilingLayout;
use neon_workspaces::{WorkspaceId, WorkspacePolicy};
use smithay::{
    backend::allocator::{Format, dmabuf::Dmabuf},
    desktop::{PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{
        Seat, SeatState,
        keyboard::XkbConfig,
        pointer::{CursorImageStatus, MotionEvent},
    },
    output::{Mode as OutputMode, Output, Scale as OutputScale},
    reexports::{
        calloop::{LoopHandle, LoopSignal},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{
        Clock, IsAlive, Logical, Monotonic, Point, Rectangle, SERIAL_COUNTER, Serial, Size,
        Transform,
    },
    wayland::{
        compositor::{CompositorClientState, CompositorState, with_states},
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
        fractional_scale::{FractionalScaleManagerState, with_fractional_scale},
        output::OutputManagerState,
        pointer_gestures::PointerGesturesState,
        presentation::PresentationState,
        seat::WaylandFocus,
        selection::{data_device::DataDeviceState, wlr_data_control::DataControlState},
        session_lock::SessionLockManagerState,
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer as WlrLayer, LayerSurfaceCachedState,
                WlrLayerShellState,
            },
            xdg::{XdgShellState, decoration::XdgDecorationState},
        },
        shm::ShmState,
        viewporter::ViewporterState,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::{X11Wm, xwm::WmWindowType},
};
use tracing::{debug, info, warn};

use crate::{
    snap_overlay::{DragSnapOverlayState, DragSnapTarget},
    wayland::{seat::FocusTarget, session_lock::SessionLockState},
    window::{
        SnapRegion, WindowId, WindowLayoutUpdate, WindowManager, WindowMode, WmRect, from_wm_point,
        from_wm_size, to_wm_point, to_wm_rect, to_wm_size, window_id, window_matches_surface,
    },
};

/// Accumulator for compositor-intercepted 3-finger touchpad swipe gestures.
#[derive(Debug, Default, Clone, Copy)]
pub struct SwipeGestureState {
    pub active: bool,
    pub fingers: u32,
    pub dx: f64,
    pub dy: f64,
    pub triggered: bool,
}

/// Per-client data attached to every connected Wayland client.
#[derive(Default, Debug)]
pub struct NeonClientData {
    pub compositor_state: CompositorClientState,
}

impl ClientData for NeonClientData {
    fn initialized(&self, client_id: ClientId) {
        debug!(?client_id, "Wayland client connected");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        debug!(?client_id, ?reason, "Wayland client disconnected");
    }
}

/// Callback invoked by `DmabufHandler::dmabuf_imported` to validate GPU buffer import.
pub type DmabufImportHook = Box<dyn FnMut(&Dmabuf) -> bool>;

/// Hit-test region inside a server-side macOS window titlebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlebarHit {
    Close,
    Minimize,
    Maximize,
    Drag,
}

/// Snapshot of a window from the previous workspace kept visually alive during a
/// Hyprland-style dual-workspace `slidefade` transition (strictly non-interactive).
#[derive(Debug, Clone)]
pub struct OutgoingWorkspaceWindow {
    pub id: WindowId,
    pub bounds: WmRect,
    pub z_index: i32,
    pub fullscreen: bool,
    pub maximized: bool,
    pub was_focused: bool,
    pub is_frameless_special: bool,
}

/// Queued fast GPU preview thumbnail capture requested by `neon-screencast-picker`.
#[derive(Debug, Clone)]
pub struct PendingPreviewCapture {
    pub target: neon_ipc::ScreencastTarget,
    pub path: PathBuf,
    pub max_width: u32,
    pub max_height: u32,
}

/// Captures a window's pre-transition geometry right before maximize/unmaximize/snap/fullscreen
/// so the compositor holds the visual frame smoothly until the client commits its resized buffer
/// and then drives the exact same Material 3 Expressive morph whether triggered by button or
/// double-click.
#[derive(Debug, Clone, Copy)]
pub struct PendingGeometryAnim {
    pub old_rect: WmRect,
    pub expected_size: Option<crate::window::WmSize>,
    pub queued_at: Duration,
}

/// Callback invoked when the user presses `Ctrl+Alt+F1..F12` on a bare-metal TTY session.
pub type VtSwitchHook = Box<dyn FnMut(i32)>;

/// Core NEON DE compositor state passed through `calloop` and `wayland-server` dispatch.
pub struct NeonState {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, Self>,
    pub loop_signal: LoopSignal,
    pub clock: Clock<Monotonic>,
    pub socket_name: OsString,

    // Wayland protocol states
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub session_lock_manager_state: SessionLockManagerState,
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub viewporter_state: ViewporterState,
    pub pointer_gestures_state: PointerGesturesState,
    pub cursor_shape_manager_state: CursorShapeManagerState,
    pub presentation_state: PresentationState,
    pub xwayland_shell_state: XWaylandShellState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub dmabuf_import_hook: Option<DmabufImportHook>,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub data_control_state: DataControlState,
    pub popups: PopupManager,

    // XWayland state
    pub xwm: Option<X11Wm>,
    pub xwayland_display: Option<u32>,

    // Multi-monitor output management
    pub output_manager: OutputManager,
    pub output_handles: HashMap<OutputId, Output>,

    // Desktop, window management, scene graph, effects, lock, and shell coordination
    pub space: Space<Window>,
    /// All live managed toplevel windows across all workspaces (including minimized windows).
    pub managed_windows: HashMap<WindowId, Window>,
    /// Mapped XWayland override-redirect windows (X11 context menus, dropdowns, tooltips).
    pub override_redirect_windows: Vec<Window>,
    /// Windows using NEON DE's GPU-rendered macOS server-side titlebar (traffic-light controls).
    pub ssd_windows: HashSet<WindowId>,
    /// Last rendered logical rect per visible window (drives smooth Hyprland/M3 geometry morphs).
    pub last_rendered_rects: HashMap<WindowId, WmRect>,
    /// Pre-transition geometry snapshots awaiting client buffer resize before starting morph.
    pub pending_geometry_anims: HashMap<WindowId, PendingGeometryAnim>,
    /// Last click timestamp & location on an SSD titlebar drag strip for double-click maximize.
    pub last_titlebar_click: Option<(WindowId, std::time::Instant, Point<f64, Logical>)>,
    /// Exact Dock icon screen center per running window (reported by `neon-shell` for minimize/close).
    pub window_dock_anchors: HashMap<WindowId, Point<i32, Logical>>,
    /// Screen center of the most recently clicked Dock/Drawer/Desktop app icon for Android-style
    /// container-transform window launch animations.
    pub recent_launch_origin: Option<(Point<i32, Logical>, std::time::Instant)>,
    /// Client-initiated window closes queued for GPU offscreen texture capture + fade animation.
    pub pending_close_ghosts: Vec<neon_renderer::PendingCloseGhost>,
    /// Windows currently playing their dock-swoop minimize animation (`id -> unmap_at`).
    pub minimizing_windows: HashMap<WindowId, Duration>,
    /// Windows currently playing their popout close animation (`id -> (deadline, close_sent)`).
    pub closing_windows: HashMap<WindowId, (Duration, bool)>,
    /// Outgoing workspace windows gliding off-screen simultaneously during a Hyprland workspace switch.
    pub outgoing_workspace_windows: Vec<OutgoingWorkspaceWindow>,
    /// Window whose macOS SSD traffic-light button cluster is currently hovered by the pointer.
    pub hovered_titlebar_window: Option<WindowId>,
    /// Flag indicating fresh input/state changes that warrant an immediate VBlank-synced frame.
    pub needs_redraw: bool,
    /// NEON DE window manager & dynamic workspace manager.
    pub wm: WindowManager,
    /// Retained scene graph synchronized every frame before GPU composition.
    pub scene: SceneGraph,
    /// Frame-driven interruptible animation engine.
    pub effects: EffectsManager,
    /// Live centralized NEON DE configuration synchronized with `neon-settings`.
    pub config: NeonConfig,
    /// Active `ext_session_lock_v1` lock state.
    pub lock_state: SessionLockState,
    /// 3-finger touchpad swipe gesture accumulator.
    pub swipe_gesture: SwipeGestureState,
    /// Whether the Activities Overview is currently open (synced with `neon-shell` over IPC).
    pub overview_open: bool,
    /// Bound IPC socket path passed to spawned shell/settings clients via `NEON_IPC_SOCKET`.
    pub ipc_socket_path: Option<PathBuf>,
    pub seat: Seat<Self>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: CursorImageStatus,
    pub primary_output: Option<Output>,

    // Runtime diagnostics & frame telemetry (Milestone 8)
    pub backend_name: String,
    pub safe_mode: bool,
    pub started_at: std::time::Instant,
    pub frames_rendered: u64,
    pub idle_frames_skipped: u64,
    pub last_frame_time_us: u64,
    /// Optional VT switching hook (wired to `LibSeatSession::change_vt` on DRM/KMS).
    pub vt_switch_hook: Option<VtSwitchHook>,
    /// Queued native screenshot destination + optional `[x, y, w, h]` logical crop region.
    pub pending_screenshot: Option<(PathBuf, Option<[i32; 4]>)>,
    /// Queued `zwlr_screencopy_frame_v1` frames awaiting GPU frame capture (OBS / portals / grim).
    pub pending_screencopy_frames: Vec<crate::wayland::screencopy::PendingScreencopyFrame>,
    /// Active PipeWire/DMA-BUF screencast capture target (Display, isolated Window, or Workspace).
    pub screencast_target: neon_ipc::ScreencastTarget,
    /// Queued live preview thumbnail captures for `neon-screencast-picker`.
    pub pending_screencast_previews: Vec<PendingPreviewCapture>,
    /// Signals `neon-shell` to open the GNOME-style interactive screenshot overlay.
    pub pending_screenshot_ui: bool,
    /// Signals `neon-shell` to open or close Clipboard History inside the Dynamic Island.
    pub pending_clipboard_toggle: bool,
    /// Signals `neon-shell` to open or close the Dynamic Island hub (`Super+N`).
    pub pending_island_toggle: bool,
    /// Active Wayland Drag-and-Drop icon surface (`wl_data_device.start_drag`), if any.
    pub dnd_icon: Option<WlSurface>,
    /// Signals `neon-shell` when a desktop Drag-and-Drop session begins or ends.
    pub pending_drag_state: Option<bool>,
    /// Signals `neon-shell` to open the OS-level right-click context menu at `(x, y)`.
    pub pending_context_menu: Option<(i32, i32, neon_ipc::ContextMenuTarget)>,
    /// Last committed logical height of `neon-panel` (0 until a top panel has mapped).
    pub last_panel_height: i32,
    /// Last committed bottom exclusive zone of `neon-dock` (0 until a dock has mapped).
    pub last_dock_exclusive: i32,
    /// Queued compositor-driven GPU open/close animation triggers `(namespace, opening)`.
    pub pending_overlay_anims: Vec<(String, bool)>,
    /// Active drag-to-snap glass preview & Windows 11 Snap Layouts drop bar state.
    pub drag_snap: DragSnapOverlayState,
    /// `true` while the pointer is hovering an interactive Auto-Tiling split divider.
    pub hovering_tile_divider: bool,
}

impl NeonState {
    /// Initializes Wayland globals, seat capabilities, window manager, scene graph, and effects.
    pub fn new(
        display: &Display<Self>,
        loop_handle: LoopHandle<'static, Self>,
        loop_signal: LoopSignal,
        socket_name: OsString,
    ) -> Self {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);
        let session_lock_manager_state =
            SessionLockManagerState::new::<Self, _>(&display_handle, |_| true);
        let fractional_scale_manager_state =
            FractionalScaleManagerState::new::<Self>(&display_handle);
        let viewporter_state = ViewporterState::new::<Self>(&display_handle);
        let pointer_gestures_state = PointerGesturesState::new::<Self>(&display_handle);
        let cursor_shape_manager_state = CursorShapeManagerState::new::<Self>(&display_handle);
        let clock = Clock::<Monotonic>::new();
        let presentation_state = PresentationState::new::<Self>(&display_handle, clock.id() as u32);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let dmabuf_state = DmabufState::new();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let data_control_state = DataControlState::new::<Self, _>(&display_handle, None, |_| true);
        crate::wayland::screencopy::register_screencopy_global(&display_handle);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "neon-seat-0");

        if let Err(err) = seat.add_keyboard(XkbConfig::default(), 600, 25) {
            warn!(?err, "Failed to initialize XKB keyboard on neon-seat-0");
        }
        seat.add_pointer();

        Self {
            display_handle,
            loop_handle,
            loop_signal,
            clock,
            socket_name,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            session_lock_manager_state,
            fractional_scale_manager_state,
            viewporter_state,
            pointer_gestures_state,
            cursor_shape_manager_state,
            presentation_state,
            xwayland_shell_state,
            shm_state,
            dmabuf_state,
            dmabuf_global: None,
            dmabuf_import_hook: None,
            output_manager_state,
            seat_state,
            data_device_state,
            data_control_state,
            popups: PopupManager::default(),
            xwm: None,
            xwayland_display: None,
            output_manager: OutputManager::new(),
            output_handles: HashMap::new(),
            space: Space::default(),
            managed_windows: HashMap::new(),
            override_redirect_windows: Vec::new(),
            ssd_windows: HashSet::new(),
            last_rendered_rects: HashMap::new(),
            pending_geometry_anims: HashMap::new(),
            last_titlebar_click: None,
            window_dock_anchors: HashMap::new(),
            recent_launch_origin: None,
            pending_close_ghosts: Vec::new(),
            minimizing_windows: HashMap::new(),
            closing_windows: HashMap::new(),
            outgoing_workspace_windows: Vec::new(),
            hovered_titlebar_window: None,
            needs_redraw: true,
            wm: WindowManager::new(WorkspacePolicy::default()),
            scene: SceneGraph::new(),
            effects: EffectsManager::new(EffectsConfig::default()),
            config: NeonConfig::default(),
            lock_state: SessionLockState::Unlocked,
            swipe_gesture: SwipeGestureState::default(),
            overview_open: false,
            ipc_socket_path: None,
            seat,
            pointer_location: (640.0, 400.0).into(),
            cursor_status: CursorImageStatus::default_named(),
            primary_output: None,
            backend_name: "headless".to_string(),
            safe_mode: false,
            started_at: std::time::Instant::now(),
            frames_rendered: 0,
            idle_frames_skipped: 0,
            last_frame_time_us: 0,
            vt_switch_hook: None,
            pending_screenshot: None,
            pending_screencopy_frames: Vec::new(),
            screencast_target: neon_ipc::ScreencastTarget::default(),
            pending_screencast_previews: Vec::new(),
            pending_screenshot_ui: false,
            pending_clipboard_toggle: false,
            pending_island_toggle: false,
            dnd_icon: None,
            pending_drag_state: None,
            pending_context_menu: None,
            last_panel_height: 0,
            last_dock_exclusive: 0,
            pending_overlay_anims: Vec::new(),
            drag_snap: DragSnapOverlayState::default(),
            hovering_tile_divider: false,
        }
    }

    /// Applies a validated [`NeonConfig`] live to compositor effects and window manager state.
    pub fn apply_config(&mut self, mut cfg: NeonConfig) {
        cfg.validate_and_clamp();
        self.effects.config.enabled = cfg.effects.enabled;
        self.effects.config.reduced_motion = cfg.effects.reduced_motion;
        self.effects.config.shadows_enabled = cfg.effects.shadows_enabled;
        self.effects.config.borders_enabled = cfg.effects.borders_enabled;
        self.effects.config.blur_enabled = cfg.effects.blur_enabled;
        self.effects.config.blur_strength = cfg.effects.blur_strength;
        self.effects.config.inactive_opacity = cfg.effects.inactive_opacity;
        self.effects.config.corner_radius = cfg.effects.corner_radius;
        self.effects.config.speed_scale = cfg.effects.speed_scale;
        self.effects.config.open_animation = cfg.effects.open_animation;
        self.effects.config.close_animation = cfg.effects.close_animation;
        self.effects.config.minimize_animation = cfg.effects.minimize_animation;
        self.effects.config.maximize_animation = cfg.effects.maximize_animation;
        self.effects.config.workspace_animation = cfg.effects.workspace_animation;
        self.effects.config.overview_animation = cfg.effects.overview_animation;
        self.effects.config.dock_animation = cfg.effects.dock_animation;

        let policy = match cfg.windows.focus_mode {
            FocusMode::ClickToFocus => crate::window::FocusPolicy::ClickToFocus,
            FocusMode::FocusFollowsMouse => crate::window::FocusPolicy::FocusFollowsMouse,
        };
        self.wm.set_focus_policy(policy);
        self.wm.set_snap_gap(cfg.windows.snap_gap);
        self.wm.set_tiling_gap(cfg.windows.tiling_gap);
        let wm_layout = match cfg.windows.tiling_layout {
            ConfigTilingLayout::Dwindle => WmTilingLayout::Dwindle,
            ConfigTilingLayout::MasterStack => WmTilingLayout::MasterStack,
        };
        let auto_tiling = cfg.windows.auto_tiling;
        self.config = cfg;
        let wa = self.work_area();
        let _ = self.wm.set_tiling_layout(wm_layout, wa);
        let tile_updates = self.wm.set_auto_tiling(auto_tiling, wa);
        self.apply_layout_updates_batch(tile_updates, false);
        self.relayout_non_floating_windows();
    }

    /// Monotonic compositor timestamp as [`Duration`] for animation sampling.
    pub fn now_duration(&self) -> Duration {
        self.clock.now().into()
    }

    /// Advertises `zwp_linux_dmabuf_v1` (version 4 with `DmabufFeedback` when a DRM render node
    /// is available, falling back to version 3) once the graphics backend reports GPU formats.
    pub fn enable_dmabuf<I>(
        &mut self,
        formats: I,
        render_node_dev_id: Option<u64>,
        import_hook: DmabufImportHook,
    ) where
        I: IntoIterator<Item = Format>,
    {
        let format_vec: Vec<Format> = formats.into_iter().collect();
        if format_vec.is_empty() {
            debug!(
                "Renderer reported no DMA-BUF formats; keeping zwp_linux_dmabuf_v1 unadvertised"
            );
            return;
        }
        let count = format_vec.len();
        let (global, version) = if let Some(dev_id) = render_node_dev_id
            && let Ok(feedback) = DmabufFeedbackBuilder::new(dev_id, format_vec.clone()).build()
        {
            (
                self.dmabuf_state
                    .create_global_with_default_feedback::<Self>(&self.display_handle, &feedback),
                4u32,
            )
        } else {
            (
                self.dmabuf_state
                    .create_global::<Self>(&self.display_handle, format_vec),
                3u32,
            )
        };
        self.dmabuf_global = Some(global);
        self.dmabuf_import_hook = Some(import_hook);
        info!(
            format_count = count,
            dmabuf_version = version,
            "Advertised zwp_linux_dmabuf_v1 global"
        );
    }

    /// Synchronizes the retained [`SceneGraph`] from active workspace windows and animations.
    ///
    /// Returns `true` if any animations remain in flight.
    pub fn sync_scene_graph(&mut self) -> bool {
        let now = self.now_duration();

        // 1. Complete finished minimize animations by unmapping their windows from Space.
        let finished_min: Vec<WindowId> = self
            .minimizing_windows
            .iter()
            .filter(|&(_, &done_at)| now >= done_at)
            .map(|(&id, _)| id)
            .collect();
        for id in finished_min {
            self.minimizing_windows.remove(&id);
            if let Some(window) = self.find_window_by_id(id)
                && self.wm.get(id).map(|w| w.mode.is_minimized()).unwrap_or(true)
            {
                self.space.unmap_elem(&window);
            }
            self.last_rendered_rects.remove(&id);
        }

        // 2. Dispatch deferred close requests once popout animation finishes, or cancel if the
        //    application stayed open (e.g. "Save unsaved changes?" dialog).
        let closing_ids: Vec<(WindowId, Duration, bool)> = self
            .closing_windows
            .iter()
            .map(|(&id, &(deadline, sent))| (id, deadline, sent))
            .collect();
        for (id, deadline, sent) in closing_ids {
            if now < deadline {
                continue;
            }
            if !sent {
                let sent_ok = self.send_native_close(id);
                if sent_ok {
                    self.closing_windows
                        .insert(id, (now + Duration::from_millis(280), true));
                } else {
                    self.closing_windows.remove(&id);
                }
            } else {
                self.closing_windows.remove(&id);
                self.effects.animate_window_cancel_close(id, now);
            }
        }

        let mut animating = self.effects.tick(now)
            || !self.minimizing_windows.is_empty()
            || !self.closing_windows.is_empty()
            || !self.pending_geometry_anims.is_empty();

        let output_size = self.output_logical_size();
        self.scene.set_backdrop(to_wm_size(output_size));

        let fx_cfg = self.effects.config;
        let focused_id = self.wm.focused_id();
        let buttons_right = self.config.windows.button_layout == WindowButtonLayout::RightStandard;
        let pointer_grabbed = self
            .seat
            .get_pointer()
            .map(|p| p.is_grabbed())
            .unwrap_or(false);
        let mut visible_ids: HashSet<WindowId> = HashSet::new();
        let mut active_space_ids: HashSet<WindowId> = HashSet::new();

        for (z_idx, window) in self.space.elements().enumerate() {
            let Some(id) = window_id(window) else {
                continue;
            };
            let Some(loc) = self.space.element_location(window) else {
                continue;
            };
            let win_size = window.geometry().size;
            let bounds = WmRect::new(to_wm_point(loc), to_wm_size(win_size));
            let mut render_bounds = bounds;

            // Detect maximize / unmaximize / snap / fullscreen / resize transitions on
            // already-visible windows and drive a smooth Material 3 Expressive spatial morph.
            let is_exiting = self.minimizing_windows.contains_key(&id)
                || self.closing_windows.contains_key(&id);
            if win_size.w > 0 && win_size.h > 0 && !is_exiting {
                if let Some(pending) = self.pending_geometry_anims.get(&id).copied() {
                    let size_changed = bounds.size != pending.old_rect.size;
                    let expects_resize =
                        pending.expected_size.is_some_and(|s| s != pending.old_rect.size);
                    let ready = if expects_resize {
                        size_changed || now >= pending.queued_at + Duration::from_millis(180)
                    } else {
                        bounds != pending.old_rect
                            || now >= pending.queued_at + Duration::from_millis(50)
                    };
                    if ready {
                        self.pending_geometry_anims.remove(&id);
                        if pending.old_rect != bounds {
                            self.effects
                                .animate_window_geometry(id, pending.old_rect, bounds, now);
                            animating = true;
                        }
                        self.last_rendered_rects.insert(id, bounds);
                    } else {
                        // Hold visual frame at `old_rect` for the ~1 frame before the client
                        // commits its resized buffer so double-click & button maximize never
                        // flicker to the new origin at the old size before morphing.
                        render_bounds = pending.old_rect;
                        animating = true;
                    }
                } else {
                    if !pointer_grabbed
                        && let Some(&prev_bounds) = self.last_rendered_rects.get(&id)
                        && prev_bounds != bounds
                    {
                        self.effects.animate_window_geometry(id, prev_bounds, bounds, now);
                        animating = true;
                    }
                    self.last_rendered_rects.insert(id, bounds);
                }
            }

            let is_frameless_special = is_window_frameless_special(window);

            let is_focused = focused_id == Some(id);
            let mode = self
                .wm
                .get(id)
                .map(|w| w.mode.clone())
                .unwrap_or(WindowMode::Floating);
            let fullscreen = mode.is_fullscreen();
            let maximized = mode.is_maximized();
            let visual = self.effects.sample_window(id, is_focused, now);
            let opacity = if is_focused || fullscreen || is_frameless_special {
                visual.opacity
            } else {
                visual.opacity * fx_cfg.inactive_opacity.clamp(0.4, 1.0)
            };

            let shadow =
                if fx_cfg.shadows_enabled && !fullscreen && !maximized && !is_frameless_special {
                    Some(ShadowStyle::for_window(
                        visual.focus_progress,
                        fx_cfg.corner_radius,
                    ))
                } else {
                    None
                };

            let border =
                if fx_cfg.borders_enabled && !fullscreen && !maximized && !is_frameless_special {
                    let mut style =
                        BorderStyle::for_window(visual.focus_progress, fx_cfg.corner_radius);
                    if let Some([ar, ag, ab]) = parse_hex_rgb(&self.config.appearance.accent_color)
                    {
                        let f = visual.focus_progress.clamp(0.0, 1.0);
                        style.color = [
                            0.28 * (1.0 - f) + ar * f,
                            0.32 * (1.0 - f) + ag * f,
                            0.40 * (1.0 - f) + ab * f,
                            0.35 * (1.0 - f) + 0.88 * f,
                        ];
                    }
                    Some(style)
                } else {
                    None
                };

            // Keep minimizing/closing windows elevated above other windows while in flight
            // so `focus_topmost_window` raising the next window never hides the dock swoop.
            let scene_z = if is_exiting {
                z_idx as i32 + 10_000
            } else {
                z_idx as i32
            };

            self.scene.upsert_window(
                id,
                render_bounds,
                scene_z,
                true,
                opacity,
                (visual.offset_x, visual.offset_y),
                (visual.scale_x, visual.scale_y),
                is_focused,
                visual.focus_progress,
                fullscreen,
                shadow,
                border,
            );
            let titlebar =
                if !fullscreen && !is_frameless_special && self.ssd_windows.contains(&id) {
                    Some(TitlebarStyle {
                        height: MACOS_TITLEBAR_HEIGHT,
                        corner_radius: fx_cfg.corner_radius,
                        focused: is_focused,
                        light_theme: self.config.appearance.theme == ThemeMode::NeonLight,
                        hover_controls: self.hovered_titlebar_window == Some(id),
                        buttons_right,
                    })
                } else {
                    None
                };
            self.scene.set_window_titlebar(id, titlebar);
            let window_corner_radius = if !fullscreen && !is_frameless_special {
                fx_cfg.corner_radius
            } else {
                0.0
            };
            self.scene.set_window_corner_radius(id, window_corner_radius);
            visible_ids.insert(id);
            active_space_ids.insert(id);
        }

        // Emit outgoing workspace windows and full-desktop slide state (for wallpaper + dock panning)
        // during a Hyprland-style horizontal workspace switch.
        if let Some(out_ws) = self.effects.sample_workspace_outgoing(now) {
            let in_ws = self.effects.sample_workspace_incoming(now);
            self.scene
                .set_workspace_slide(Some(neon_scene::WorkspaceSlide {
                    outgoing_dx: out_ws.offset_x,
                    incoming_dx: in_ws.offset_x,
                }));
            let accent_rgb = parse_hex_rgb(&self.config.appearance.accent_color);
            let light_theme = self.config.appearance.theme == ThemeMode::NeonLight;
            for out in &self.outgoing_workspace_windows {
                if visible_ids.contains(&out.id)
                    || !self.managed_windows.contains_key(&out.id)
                    || out.bounds.size.w <= 0
                    || out.bounds.size.h <= 0
                {
                    continue;
                }
                let focus_prog = if out.was_focused { 1.0_f32 } else { 0.0_f32 };
                let opacity = if out.was_focused || out.fullscreen || out.is_frameless_special {
                    out_ws.opacity
                } else {
                    out_ws.opacity * fx_cfg.inactive_opacity.clamp(0.4, 1.0)
                };
                if opacity <= 0.005 {
                    continue;
                }
                let shadow = if fx_cfg.shadows_enabled
                    && !out.fullscreen
                    && !out.maximized
                    && !out.is_frameless_special
                {
                    Some(ShadowStyle::for_window(focus_prog, fx_cfg.corner_radius))
                } else {
                    None
                };
                let border = if fx_cfg.borders_enabled
                    && !out.fullscreen
                    && !out.maximized
                    && !out.is_frameless_special
                {
                    let mut style = BorderStyle::for_window(focus_prog, fx_cfg.corner_radius);
                    if let Some([ar, ag, ab]) = accent_rgb {
                        style.color = [
                            0.28 * (1.0 - focus_prog) + ar * focus_prog,
                            0.32 * (1.0 - focus_prog) + ag * focus_prog,
                            0.40 * (1.0 - focus_prog) + ab * focus_prog,
                            0.35 * (1.0 - focus_prog) + 0.88 * focus_prog,
                        ];
                    }
                    Some(style)
                } else {
                    None
                };
                self.scene.upsert_window(
                    out.id,
                    out.bounds,
                    -1000 + out.z_index,
                    true,
                    opacity,
                    (out_ws.offset_x, 0.0),
                    (out_ws.scale, out_ws.scale),
                    out.was_focused,
                    focus_prog,
                    out.fullscreen,
                    shadow,
                    border,
                );
                let titlebar = if !out.fullscreen
                    && !out.is_frameless_special
                    && self.ssd_windows.contains(&out.id)
                {
                    Some(TitlebarStyle {
                        height: MACOS_TITLEBAR_HEIGHT,
                        corner_radius: fx_cfg.corner_radius,
                        focused: out.was_focused,
                        light_theme,
                        hover_controls: false,
                        buttons_right,
                    })
                } else {
                    None
                };
                self.scene.set_window_titlebar(out.id, titlebar);
                let corner_radius = if !out.fullscreen && !out.is_frameless_special {
                    fx_cfg.corner_radius
                } else {
                    0.0
                };
                self.scene.set_window_corner_radius(out.id, corner_radius);
                visible_ids.insert(out.id);
            }
            animating = true;
        } else {
            self.scene.set_workspace_slide(None);
            if !self.outgoing_workspace_windows.is_empty() {
                self.outgoing_workspace_windows.clear();
            }
        }

        self.last_rendered_rects
            .retain(|id, _| active_space_ids.contains(id));
        self.pending_geometry_anims
            .retain(|id, _| active_space_ids.contains(id));
        self.scene.retain_windows(|id| visible_ids.contains(&id));

        // 3. Emit GPU-native Windows 11 Snap Preview highlight & top-center Snap Layouts drop bar.
        if self.backend_name != "drm"
            && !self.drag_snap.is_active()
            && let Ok(spec) = std::env::var("NEON_TEST_SNAP_BAR_PREVIEW")
            && let Some(fid) = self.wm.focused_id()
        {
            let wa = self.work_area();
            let cx = (wa.loc.x + wa.size.w / 2) as f64;
            // First reveal the top-center Snap Layouts bar, then hover requested card:zone.
            self.update_drag_snap(fid, (cx, (wa.loc.y + 4) as f64).into());
            let mut parts = spec.split(':');
            let ci = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
            let zi = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            let geo = crate::snap_overlay::compute_snap_bar_geometry(wa, 1.0);
            if let Some(&(_, _, _, zr)) = geo.zones.iter().find(|z| z.0 == ci && z.1 == zi) {
                let zx = (zr.loc.x + zr.size.w / 2) as f64;
                let zy = (zr.loc.y + zr.size.h / 2) as f64;
                self.update_drag_snap(fid, (zx, zy).into());
            }
        }
        self.scene.clear_rounded_overlays();
        if self.drag_snap.is_active() {
            let anim_on = fx_cfg.enabled && !fx_cfg.reduced_motion;
            let [ar, ag, ab] =
                parse_hex_rgb(&self.config.appearance.accent_color).unwrap_or([0.22, 0.74, 0.97]);
            let light_theme = self.config.appearance.theme == ThemeMode::NeonLight;

            if let Some((preview_rect, alpha, p_anim)) = self.drag_snap.current_preview(anim_on) {
                animating |= p_anim;
                // Place right under the dragged window (which is raised to top of Windows layer)
                // and above all other desktop windows, mirroring Windows 11 Snap Assist.
                let preview_z = if self.drag_snap.dragged_window.is_some() {
                    let count = self.space.elements().count() as i32;
                    (count - 1).max(0) * 10 - 1
                } else {
                    500
                };
                self.scene.insert_rounded_overlay(
                    SceneLayer::Windows,
                    preview_z,
                    preview_rect,
                    fx_cfg.corner_radius.max(12.0),
                    [ar, ag, ab, 0.22],
                    [ar, ag, ab, 0.90],
                    2.0,
                    alpha,
                );
            }

            let work_area = self.work_area();
            if let Some((bar_geo, bar_alpha, b_anim)) =
                self.drag_snap.current_bar(work_area, anim_on)
            {
                animating |= b_anim;
                let (bar_fill, bar_border, card_fill, card_border, zone_idle, zone_idle_border) =
                    if light_theme {
                        (
                            [0.95, 0.96, 0.99, 0.90],
                            [0.10, 0.14, 0.24, 0.16],
                            [0.86, 0.89, 0.95, 0.65],
                            [0.10, 0.14, 0.24, 0.12],
                            [0.72, 0.77, 0.86, 0.82],
                            [0.10, 0.14, 0.24, 0.20],
                        )
                    } else {
                        (
                            [0.07, 0.09, 0.14, 0.92],
                            [1.0, 1.0, 1.0, 0.20],
                            [0.13, 0.16, 0.24, 0.72],
                            [1.0, 1.0, 1.0, 0.12],
                            [0.24, 0.29, 0.40, 0.78],
                            [1.0, 1.0, 1.0, 0.18],
                        )
                    };

                // Top-center floating Liquid Glass Snap Layouts tray
                self.scene.insert_rounded_overlay(
                    SceneLayer::Overview,
                    10,
                    bar_geo.bar_rect,
                    16.0,
                    bar_fill,
                    bar_border,
                    1.2,
                    bar_alpha,
                );
                for crect in &bar_geo.cards {
                    self.scene.insert_rounded_overlay(
                        SceneLayer::Overview,
                        20,
                        *crect,
                        9.0,
                        card_fill,
                        card_border,
                        1.0,
                        bar_alpha,
                    );
                }
                for &(ci, zi, _region, zrect) in &bar_geo.zones {
                    let hovered = self.drag_snap.hovered_bar_zone == Some((ci, zi));
                    let (zf, zb, zw) = if hovered {
                        ([ar, ag, ab, 0.94], [1.0, 1.0, 1.0, 0.90], 1.4)
                    } else {
                        (zone_idle, zone_idle_border, 0.9)
                    };
                    self.scene.insert_rounded_overlay(
                        SceneLayer::Overview,
                        30,
                        zrect,
                        5.0,
                        zf,
                        zb,
                        zw,
                        bar_alpha,
                    );
                }
            }
        }
        animating
    }

    /// Returns `true` when the desktop session is locked via `ext_session_lock_v1`.
    pub fn is_locked(&self) -> bool {
        self.lock_state.is_locked()
    }

    /// Launches `neon-lockscreen` to lock the session via `ext_session_lock_v1`.
    pub fn lock_session(&self) {
        if self.is_locked() {
            return;
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let sibling = dir.join("neon-lockscreen");
            if sibling.exists() {
                self.spawn_client(&sibling.to_string_lossy(), &[]);
                return;
            }
        }
        self.spawn_client("neon-lockscreen", &[]);
    }

    /// Registers an output into `OutputManager`, the Wayland global registry, and `Space`.
    pub fn register_output(&mut self, output: Output, location: Point<i32, Logical>) -> OutputId {
        let mode = output.current_mode().unwrap_or(OutputMode {
            size: (1280, 800).into(),
            refresh: 60_000,
        });
        let display_mode = DisplayMode {
            width: mode.size.w,
            height: mode.size.h,
            refresh_mhz: mode.refresh,
        };
        let scale = output.current_scale().fractional_scale();
        let props = output.physical_properties();
        let record = self.output_manager.register_output(
            output.name(),
            props.make,
            props.model,
            vec![display_mode],
            display_mode,
            scale,
        );

        let effective_loc: Point<i32, Logical> = if location != (0, 0).into() {
            location
        } else {
            from_wm_point(record.position)
        };

        output.create_global::<Self>(&self.display_handle);
        self.space.map_output(&output, effective_loc);
        self.output_handles.insert(record.id, output.clone());
        if record.primary || self.primary_output.is_none() {
            self.primary_output = Some(output);
        }
        record.id
    }

    /// Safely handles monitor hotplug disconnection, migrating windows onto a surviving output.
    pub fn disconnect_output(&mut self, id: OutputId) -> bool {
        let Some(outcome) = self.output_manager.disconnect_output(id) else {
            return false;
        };
        if let Some(removed_output) = self.output_handles.remove(&id) {
            self.space.unmap_output(&removed_output);
        }
        self.primary_output = outcome
            .fallback_output
            .and_then(|fallback_id| self.output_handles.get(&fallback_id).cloned());

        // Clamp/migrate all active windows safely into the surviving work area so none are stranded.
        let work_area = self.work_area();
        for wid in self.wm.all_window_ids() {
            if let Some(rec) = self.wm.get(wid).cloned() {
                let clamped =
                    work_area.clamp_origin(rec.current_geometry.loc, rec.current_geometry.size);
                self.wm.set_floating_location(wid, clamped);
                if let Some(window) = self.find_window_by_id(wid)
                    && rec.is_visible_on(self.wm.active_workspace_id())
                {
                    self.map_preserving_z_order(window, from_wm_point(clamped));
                }
            }
        }
        self.relayout_non_floating_windows();
        true
    }

    /// Applies live display configuration updates (scale, fractional scale, position, rotation, primary).
    pub fn configure_output_by_name(
        &mut self,
        name: &str,
        patch: &OutputConfigPatch,
    ) -> Option<neon_output::OutputRecord> {
        let updated = self.output_manager.configure_by_name(name, patch)?;
        if let Some(output) = self.output_handles.get(&updated.id).cloned() {
            let transform = to_smithay_transform(updated.transform);
            let mode = OutputMode {
                size: (updated.current_mode.width, updated.current_mode.height).into(),
                refresh: updated.current_mode.refresh_mhz,
            };
            let loc = from_wm_point(updated.position);
            output.change_current_state(
                Some(mode),
                Some(transform),
                Some(OutputScale::Fractional(updated.scale)),
                Some(loc),
            );
            self.space.map_output(&output, loc);
            layer_map_for_output(&output).arrange();

            if updated.primary {
                self.primary_output = Some(output);
            }
            self.broadcast_fractional_scale(updated.scale);
            self.relayout_non_floating_windows();
        }
        Some(updated)
    }

    /// Notifies all live client surfaces of an updated preferred fractional scale factor.
    pub fn broadcast_fractional_scale(&self, scale: f64) {
        for window in self.managed_windows.values() {
            if let Some(surface) = window.wl_surface() {
                with_states(&surface, |states| {
                    with_fractional_scale(states, |fs| {
                        fs.set_preferred_scale(scale);
                    });
                });
            }
        }
    }

    /// Returns the logical size of the active primary output, defaulting to 1280x800.
    pub fn output_logical_size(&self) -> Size<i32, Logical> {
        self.primary_output
            .as_ref()
            .and_then(|output| self.space.output_geometry(output))
            .map(|geo| geo.size)
            .unwrap_or_else(|| (1280, 800).into())
    }

    /// Returns the full logical rectangle `(0, 0, w, h)` of the active primary output
    /// (used for true edge-to-edge fullscreen windows).
    pub fn output_full_rect(&self) -> WmRect {
        to_wm_rect(Rectangle::from_size(self.output_logical_size()))
    }

    /// Returns the active output work area excluding `zwlr_layer_shell_v1` exclusive zones
    /// (such as the top panel and dock).
    ///
    /// When the top bar is enabled and `panel.smart_hide` is OFF, guarantees `loc.y` stays
    /// strictly below the top bar height so maximized/snapped windows never extend behind
    /// or above the top bar even during fullscreen transitions or exclusive-zone renegotiation.
    pub fn work_area(&self) -> WmRect {
        let mut rect = if let Some(output) = self.primary_output.as_ref() {
            let map = layer_map_for_output(output);
            let zone = map.non_exclusive_zone();
            if zone.size.w > 0 && zone.size.h > 0 {
                to_wm_rect(zone)
            } else {
                self.output_full_rect()
            }
        } else {
            self.output_full_rect()
        };

        if self.config.panel.visible
            && self.config.panel.mode == neon_config::PanelMode::TopAndDock
            && !self.config.panel.smart_hide
        {
            let panel_h = self
                .primary_output
                .as_ref()
                .and_then(|output| {
                    let map = layer_map_for_output(output);
                    map.layers()
                        .find(|l| l.namespace() == "neon-panel")
                        .and_then(|l| map.layer_geometry(l))
                        .map(|g| g.loc.y + g.size.h)
                })
                .filter(|&h| h > 0)
                .unwrap_or(self.last_panel_height);

            if panel_h > 0 && rect.loc.y < panel_h {
                let delta = panel_h - rect.loc.y;
                rect.loc.y = panel_h;
                rect.size.h = (rect.size.h - delta).max(200);
            }
        }

        // Also protect the bottom dock zone whenever a bottom dock is Always Visible so maximized
        // and tiled apps never overlap the floating dock or combined taskbar.
        if self.config.dock.position == DockPosition::Bottom
            && !self.config.dock.auto_hide
            && !self.config.dock.intelligent_hide
            && self.last_dock_exclusive > 0
        {
            let full = self.output_full_rect();
            let max_bottom = (full.loc.y + full.size.h - self.last_dock_exclusive).max(rect.loc.y + 200);
            let cur_bottom = rect.loc.y + rect.size.h;
            if cur_bottom > max_bottom {
                rect.size.h = (max_bottom - rect.loc.y).max(200);
            }
        }

        rect
    }

    /// Returns `true` when any visible window on the active workspace is in fullscreen mode
    /// and the App Drawer (Overview) is closed.
    pub fn has_visible_fullscreen_window(&self) -> bool {
        if self.overview_open {
            return false;
        }
        let active_ws = self.wm.active_workspace_id();
        self.space.elements().any(|w| {
            window_id(w)
                .and_then(|id| self.wm.get(id))
                .map(|rec| rec.is_visible_on(active_ws) && rec.mode.is_fullscreen())
                .unwrap_or(false)
        })
    }

    /// Moves or maps `window` to `loc` in `self.space` WITHOUT disturbing existing window Z-order.
    ///
    /// In Smithay 0.7, `Space::map_element` always removes and re-pushes the element to the top
    /// of the stack even when `activate == false`. Preserving relative Z-order avoids windows
    /// jumping behind/over each other on relayouts, App Drawer transitions, and X11 configure events.
    pub fn map_preserving_z_order(&mut self, window: Window, loc: Point<i32, Logical>) {
        if let Some(current_loc) = self.space.element_location(&window) {
            if current_loc == loc {
                return;
            }
            let elements: Vec<Window> = self.space.elements().cloned().collect();
            if let Some(idx) = elements.iter().position(|w| w == &window) {
                let above = &elements[idx + 1..];
                self.space.map_element(window, loc, false);
                for w in above {
                    self.space.raise_element(w, false);
                }
                self.sync_x11_stacking_order();
                return;
            }
        }
        self.space.map_element(window, loc, false);
        self.sync_x11_stacking_order();
    }

    /// Synchronizes XWayland's internal X11 window stacking order with `self.space` (bottom -> top)
    /// so overlapping X11 windows and transient dialogs route pointer/keyboard input and paint
    /// consistently.
    ///
    /// Note: in Smithay 0.7, `X11Wm::update_stacking_order_upwards` uses `StackMode::ABOVE` and
    /// expects windows in **bottom -> top** order (matching `Space::elements()`), whereas
    /// `update_stacking_order_downwards` uses `StackMode::BELOW` (top -> bottom).
    pub fn sync_x11_stacking_order(&mut self) {
        if let Some(xwm) = self.xwm.as_mut() {
            let x11_order: Vec<&smithay::xwayland::X11Surface> = self
                .space
                .elements()
                .filter_map(|w| w.x11_surface())
                .collect();
            if !x11_order.is_empty() {
                let _ = xwm.update_stacking_order_upwards(x11_order.into_iter());
            }
        }
    }

    /// Resolves a managed NEON DE [`WindowId`] from an X11 window ID (`XID`).
    pub fn find_x11_window_id(&self, xid: u32) -> Option<WindowId> {
        self.managed_windows
            .iter()
            .find(|(_, w)| {
                w.x11_surface()
                    .map(|s| s.window_id() == xid)
                    .unwrap_or(false)
            })
            .map(|(&id, _)| id)
    }

    /// Returns visible windows on workspace `ws_index` ordered front-to-back with their logical
    /// locations and fullscreen flags (used for isolated Workspace screencasts & live previews).
    pub fn workspace_windows_front_to_back(
        &self,
        ws_index: usize,
    ) -> Vec<(&Window, Point<i32, Logical>, bool)> {
        let Some(ws) = self.wm.workspaces().workspaces().get(ws_index) else {
            return Vec::new();
        };
        let ws_id = ws.id;
        self.wm
            .visible_windows_mru_order_for(ws_id)
            .into_iter()
            .rev()
            .filter_map(|wid| {
                let win = self.managed_windows.get(&wid)?;
                let rec = self.wm.get(wid)?;
                let loc = self
                    .space
                    .element_location(win)
                    .unwrap_or_else(|| from_wm_point(rec.current_geometry.loc));
                Some((win, loc, rec.mode.is_fullscreen()))
            })
            .collect()
    }

    /// Checks whether an upper `zwlr_layer_shell_v1` surface sits above regular windows at `pos`.
    ///
    /// When a fullscreen window is active (and Overview is closed), regular `Layer::Top` surfaces
    /// (such as the top panel and dock) yield visually and interactively to the fullscreen window,
    /// while `Layer::Overlay` surfaces (notifications, quick settings, context menus) remain on top.
    pub fn upper_layer_surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>, KeyboardInteractivity)> {
        if self.has_visible_fullscreen_window() && !self.overview_open {
            self.layer_surface_under(pos, &[WlrLayer::Overlay])
        } else {
            self.layer_surface_under(pos, &[WlrLayer::Overlay, WlrLayer::Top])
        }
    }

    /// Checks whether a `zwlr_layer_shell_v1` surface sits under `pos` on one of `layers`.
    pub fn layer_surface_under(
        &self,
        pos: Point<f64, Logical>,
        layers: &[WlrLayer],
    ) -> Option<(WlSurface, Point<f64, Logical>, KeyboardInteractivity)> {
        let output = self.primary_output.as_ref()?;
        let map = layer_map_for_output(output);
        for &wlr_layer in layers {
            // Check foreground layers first, then fullscreen click-catcher backdrops so
            // `"neon-quick-settings"` and `"neon-workspaces-panel"` always receive input above
            // their click-catcher backdrops.
            let ordered_layers: Vec<_> = {
                let mut top_fg = Vec::new();
                let mut fg = Vec::new();
                let mut bg = Vec::new();
                let mut chrome_bg = Vec::new();
                for layer in map.layers_on(wlr_layer).rev() {
                    match layer.namespace() {
                        "neon-context-submenu" => top_fg.push(layer.clone()),
                        "neon-context-menu" => fg.insert(0, layer.clone()),
                        "neon-quick-settings-backdrop"
                        | "neon-notification-center-backdrop"
                        | "neon-workspaces-backdrop"
                        | "neon-screencast-picker-backdrop"
                        | "neon-context-menu-backdrop"
                        | "neon-screenshot-ui" => bg.push(layer.clone()),
                        "neon-panel" | "neon-dock" if wlr_layer == WlrLayer::Overlay => {
                            chrome_bg.push(layer.clone());
                        }
                        _ => fg.push(layer.clone()),
                    }
                }
                top_fg.extend(fg);
                top_fg.extend(bg);
                top_fg.extend(chrome_bg);
                top_fg
            };
            for layer in ordered_layers {
                if let Some(geo) = map.layer_geometry(&layer)
                    && geo.to_f64().contains(pos)
                    && let Some((surface, surf_loc)) =
                        layer.surface_under(pos - geo.loc.to_f64(), WindowSurfaceType::ALL)
                {
                    let interactivity = with_states(layer.wl_surface(), |states| {
                        states
                            .cached_state
                            .get::<LayerSurfaceCachedState>()
                            .current()
                            .keyboard_interactivity
                    });
                    return Some((surface, (geo.loc + surf_loc).to_f64(), interactivity));
                }
            }
        }
        None
    }

    /// Resolves the Wayland surface and its logical origin under the given compositor coordinates,
    /// respecting `zwlr_layer_shell_v1` Z-order (`Overlay`/`Top` above override-redirect popups &
    /// windows, `Bottom`/`Background` below).
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(FocusTarget, Point<f64, Logical>)> {
        if let Some((surf, loc, _)) = self.upper_layer_surface_under(pos) {
            return Some((FocusTarget::Wayland(surf), loc));
        }

        // Check XWayland override-redirect popups (context menus / dropdowns) above regular windows.
        for or_win in self.override_redirect_windows.iter().rev() {
            if !or_win.alive() {
                continue;
            }
            if let Some(loc) = self
                .space
                .element_location(or_win)
                .or_else(|| or_win.x11_surface().map(|s| s.geometry().loc))
                && let Some((surface, surface_loc)) =
                    or_win.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)
            {
                return Some((FocusTarget::Wayland(surface), (loc + surface_loc).to_f64()));
            }
        }

        if let Some(win_hit) = self
            .space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surface_loc)| {
                        (FocusTarget::Wayland(surface), (location + surface_loc).to_f64())
                    })
            })
        {
            return Some(win_hit);
        }

        self.layer_surface_under(pos, &[WlrLayer::Bottom, WlrLayer::Background])
            .map(|(surf, loc, _)| (FocusTarget::Wayland(surf), loc))
    }

    /// Toggles Activities Overview state (synced with `neon-shell` over IPC).
    pub fn toggle_overview(&mut self, serial: Serial) -> bool {
        let next = !self.overview_open;
        self.set_overview_open(next, serial);
        next
    }

    /// Explicitly opens or closes Activities Overview.
    pub fn set_overview_open(&mut self, open: bool, serial: Serial) {
        if self.overview_open == open {
            return;
        }
        self.overview_open = open;
        info!(open, "Updated Activities Overview state");
        if !open {
            self.focus_topmost_window(serial);
        }
    }

    /// Finds the mapped [`Window`] whose surface tree sits at `pos` on the active workspace.
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Point<i32, Logical>)> {
        self.space
            .element_under(pos)
            .map(|(window, loc)| (window.clone(), loc))
    }

    /// Finds any live managed [`Window`] (active, inactive workspace, or minimized) by [`WindowId`].
    pub fn find_window_by_id(&self, id: WindowId) -> Option<Window> {
        self.managed_windows.get(&id).cloned()
    }

    /// Finds any live managed `(WindowId, Window)` matching the given `WlSurface`.
    pub fn find_window_by_surface(&self, surface: &WlSurface) -> Option<(WindowId, Window)> {
        self.managed_windows
            .iter()
            .find(|(_, w)| window_matches_surface(w, surface))
            .map(|(&id, w)| (id, w.clone()))
    }

    /// Applies a [`WindowLayoutUpdate`] from `WindowManager` to Smithay's `Space` and `xdg_toplevel`.
    pub fn apply_layout_update(&mut self, update: &WindowLayoutUpdate) {
        let Some(window) = self.find_window_by_id(update.id) else {
            return;
        };

        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                // Reset exclusive layout flags first so no conflicting states can ever be emitted.
                state.states.unset(xdg_toplevel::State::Maximized);
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.states.unset(xdg_toplevel::State::TiledLeft);
                state.states.unset(xdg_toplevel::State::TiledRight);
                state.states.unset(xdg_toplevel::State::TiledTop);
                state.states.unset(xdg_toplevel::State::TiledBottom);

                match &update.mode {
                    WindowMode::Floating => {
                        state.size = update.configure_size.map(from_wm_size);
                    }
                    WindowMode::Maximized => {
                        state.states.set(xdg_toplevel::State::Maximized);
                        state.size = update.configure_size.map(from_wm_size);
                    }
                    WindowMode::Fullscreen => {
                        state.states.set(xdg_toplevel::State::Fullscreen);
                        state.size = update.configure_size.map(from_wm_size);
                    }
                    WindowMode::Tiled => {
                        state.states.set(xdg_toplevel::State::TiledLeft);
                        state.states.set(xdg_toplevel::State::TiledRight);
                        state.states.set(xdg_toplevel::State::TiledTop);
                        state.states.set(xdg_toplevel::State::TiledBottom);
                        state.size = update.configure_size.map(from_wm_size);
                    }
                    WindowMode::Snapped(region) => {
                        match region {
                            SnapRegion::LeftHalf
                            | SnapRegion::LeftTwoThirds
                            | SnapRegion::LeftThird => {
                                state.states.set(xdg_toplevel::State::TiledLeft);
                                state.states.set(xdg_toplevel::State::TiledTop);
                                state.states.set(xdg_toplevel::State::TiledBottom);
                            }
                            SnapRegion::RightHalf
                            | SnapRegion::RightOneThird
                            | SnapRegion::RightThird => {
                                state.states.set(xdg_toplevel::State::TiledRight);
                                state.states.set(xdg_toplevel::State::TiledTop);
                                state.states.set(xdg_toplevel::State::TiledBottom);
                            }
                            SnapRegion::CenterThird => {
                                state.states.set(xdg_toplevel::State::TiledLeft);
                                state.states.set(xdg_toplevel::State::TiledRight);
                                state.states.set(xdg_toplevel::State::TiledTop);
                                state.states.set(xdg_toplevel::State::TiledBottom);
                            }
                            SnapRegion::TopHalf => {
                                state.states.set(xdg_toplevel::State::TiledLeft);
                                state.states.set(xdg_toplevel::State::TiledRight);
                                state.states.set(xdg_toplevel::State::TiledTop);
                            }
                            SnapRegion::BottomHalf => {
                                state.states.set(xdg_toplevel::State::TiledLeft);
                                state.states.set(xdg_toplevel::State::TiledRight);
                                state.states.set(xdg_toplevel::State::TiledBottom);
                            }
                            SnapRegion::TopLeft => {
                                state.states.set(xdg_toplevel::State::TiledLeft);
                                state.states.set(xdg_toplevel::State::TiledTop);
                            }
                            SnapRegion::TopRight => {
                                state.states.set(xdg_toplevel::State::TiledRight);
                                state.states.set(xdg_toplevel::State::TiledTop);
                            }
                            SnapRegion::BottomLeft => {
                                state.states.set(xdg_toplevel::State::TiledLeft);
                                state.states.set(xdg_toplevel::State::TiledBottom);
                            }
                            SnapRegion::BottomRight => {
                                state.states.set(xdg_toplevel::State::TiledRight);
                                state.states.set(xdg_toplevel::State::TiledBottom);
                            }
                        }
                        state.size = update.configure_size.map(from_wm_size);
                    }
                    WindowMode::Minimized { .. } => {}
                }
            });

            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        }

        if update.visible_on_active_workspace {
            if let Some(loc) = update.target_location {
                self.map_preserving_z_order(window, from_wm_point(loc));
            }
        } else {
            self.space.unmap_elem(&window);
        }
    }

    /// Synchronizes `Space` membership with the active workspace and restores focus & pointer hover.
    pub fn sync_active_workspace_space(&mut self, serial: Serial) {
        self.last_rendered_rects.clear();
        let active_ws = self.wm.active_workspace_id();
        let all_ids = self.wm.all_window_ids();

        for id in all_ids {
            let Some(window) = self.find_window_by_id(id) else {
                continue;
            };
            let Some(record) = self.wm.get(id) else {
                continue;
            };
            if !record.is_visible_on(active_ws) {
                self.space.unmap_elem(&window);
            }
        }

        // Map active workspace windows in deterministic MRU order (oldest -> newest) so the
        // most-recently focused window always stacks on top after workspace switches.
        for id in self.wm.active_visible_windows_mru_order() {
            let Some(window) = self.find_window_by_id(id) else {
                continue;
            };
            let Some(record) = self.wm.get(id) else {
                continue;
            };
            let loc = from_wm_point(record.current_geometry.loc);
            self.space.map_element(window, loc, false);
        }
        self.sync_x11_stacking_order();

        self.focus_topmost_window(serial);

        // Refresh pointer hover so stale surfaces from the previous workspace release pointer focus.
        let pos = self.pointer_location;
        let under = self.surface_under(pos);
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(
                self,
                under,
                &MotionEvent {
                    location: pos,
                    serial,
                    time: self.clock.now().as_millis(),
                },
            );
            pointer.frame(self);
        }
    }

    /// Focuses and optionally raises the given window, updating `xdg_toplevel::State::Activated`.
    pub fn focus_window(&mut self, target: Option<&Window>, serial: Serial) {
        self.focus_window_with_raise(target, serial, true);
    }

    pub fn focus_window_with_raise(
        &mut self,
        target: Option<&Window>,
        serial: Serial,
        raise: bool,
    ) {
        if self.is_locked() {
            // Security invariant: never shift focus onto a regular desktop window while locked.
            return;
        }
        let prev_focused = self.wm.focused_id();
        let target_id = target.and_then(window_id);
        let target_focus: Option<FocusTarget> = target.and_then(|w| {
            if let Some(x11) = w.x11_surface() {
                Some(FocusTarget::X11(x11.clone()))
            } else {
                w.wl_surface().map(|s| FocusTarget::Wayland(s.into_owned()))
            }
        });

        for (&win_id, window) in &self.managed_windows {
            let is_target = target_id.is_some_and(|tid| tid == win_id);

            if let Some(toplevel) = window.toplevel() {
                let changed = toplevel.with_pending_state(|state| {
                    if is_target {
                        state.states.set(xdg_toplevel::State::Activated)
                    } else {
                        state.states.unset(xdg_toplevel::State::Activated)
                    }
                });
                if changed && toplevel.is_initial_configure_sent() {
                    toplevel.send_pending_configure();
                }
            } else if let Some(x11) = window.x11_surface() {
                let _ = x11.set_activated(is_target);
            }
        }

        if raise && let Some(window) = target {
            self.space.raise_element(window, true);
            // Keep any visible transient child dialogs stacked above their parent so focusing
            // or clicking a parent window never buries its modal / sub-dialogs (e.g. DaVinci Resolve).
            if let Some(tid) = target_id {
                for child_id in self.wm.visible_transient_children(tid) {
                    if let Some(child_win) = self.find_window_by_id(child_id) {
                        self.space.raise_element(&child_win, false);
                    }
                }
            }
            // Keep floating windows (e.g. Settings, dialogs, Super+V floated apps) stacked above
            // tiled windows when focusing a tiled window in Auto-Tiling mode.
            let active_ws = self.wm.active_workspace_id();
            let target_is_tiled = target_id
                .and_then(|tid| self.wm.get(tid))
                .map(|w| w.mode.is_tiled())
                .unwrap_or(false);
            if self.wm.is_auto_tiling() && target_is_tiled {
                for float_id in self.wm.visible_floating_above_tiled_windows(active_ws) {
                    if Some(float_id) != target_id
                        && let Some(float_win) = self.find_window_by_id(float_id)
                    {
                        self.space.raise_element(&float_win, false);
                    }
                }
            }
            // Keep any visible `always_on_top` windows stacked above regular windows.
            let target_is_aot = target_id
                .and_then(|tid| self.wm.get(tid))
                .map(|w| w.always_on_top)
                .unwrap_or(false);
            if !target_is_aot {
                for aot_id in self.wm.visible_always_on_top_windows(active_ws) {
                    if Some(aot_id) != target_id
                        && let Some(aot_win) = self.find_window_by_id(aot_id)
                    {
                        self.space.raise_element(&aot_win, false);
                    }
                }
            }
            self.sync_x11_stacking_order();
        }

        let now = self.now_duration();
        if prev_focused != target_id {
            if let Some(old_id) = prev_focused {
                self.effects.animate_focus(old_id, false, now);
            }
            if let Some(new_id) = target_id {
                self.effects.animate_focus(new_id, true, now);
            }
        }

        self.wm.mark_focused(target_id);

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, target_focus, serial);
        }
    }

    /// Restores keyboard focus to the MRU visible window on the active workspace (or clears focus).
    pub fn focus_topmost_window(&mut self, serial: Serial) {
        let next = self
            .wm
            .active_mru_window()
            .and_then(|id| self.find_window_by_id(id));
        self.focus_window(next.as_ref(), serial);
    }

    /// Cycles focus (`Alt + Tab`) across visible windows on the active workspace.
    pub fn cycle_focus(&mut self, serial: Serial) {
        if let Some(next_id) = self.wm.next_cycle_target()
            && let Some(window) = self.find_window_by_id(next_id)
        {
            debug!(?next_id, "Cycling window focus");
            self.focus_window(Some(&window), serial);
        }
    }

    fn send_native_close(&self, id: WindowId) -> bool {
        let Some(window) = self.find_window_by_id(id) else {
            return false;
        };
        if let Some(toplevel) = window.toplevel() {
            info!(?id, "Sending xdg_toplevel.close to window");
            toplevel.send_close();
            true
        } else if let Some(x11) = window.x11_surface() {
            info!(?id, "Sending WM_DELETE_WINDOW to XWayland window");
            let _ = x11.close();
            true
        } else {
            false
        }
    }

    /// Captures a window's pre-transition logical rect right before maximize/unmaximize/snap/
    /// fullscreen/center so button clicks and titlebar double-clicks play the exact same smooth
    /// Material 3 Expressive spatial morph once the client commits its resized buffer.
    pub fn begin_geometry_transition(
        &mut self,
        id: WindowId,
        expected_size: Option<crate::window::WmSize>,
    ) {
        let now = self.now_duration();
        let old_rect = self
            .pending_geometry_anims
            .get(&id)
            .map(|p| p.old_rect)
            .or_else(|| self.last_rendered_rects.get(&id).copied())
            .or_else(|| {
                self.find_window_by_id(id).and_then(|w| {
                    let loc = self.space.element_location(&w)?;
                    let sz = w.geometry().size;
                    (sz.w > 0 && sz.h > 0).then(|| WmRect::new(to_wm_point(loc), to_wm_size(sz)))
                })
            })
            .or_else(|| self.wm.get(id).map(|r| r.current_geometry));

        if let Some(old_rect) = old_rect
            && old_rect.size.w > 0
            && old_rect.size.h > 0
        {
            self.pending_geometry_anims.insert(
                id,
                PendingGeometryAnim {
                    old_rect,
                    expected_size,
                    queued_at: now,
                },
            );
            self.needs_redraw = true;
        }
    }

    /// Starts an Android-style Material 3 Expressive window open animation springing directly out
    /// of the most recently clicked Dock, App Drawer, or Desktop icon (when within 4 seconds).
    pub fn animate_window_open_android(&mut self, id: WindowId, window: &Window) {
        let now = self.now_duration();
        let origin = self
            .recent_launch_origin
            .take()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(4))
            .map(|(pt, _)| pt);

        if let Some(origin) = origin {
            let rect = self
                .space
                .element_location(window)
                .map(|loc| Rectangle::new(loc, window.geometry().size))
                .or_else(|| {
                    self.wm
                        .get(id)
                        .map(|r| Rectangle::new(from_wm_point(r.current_geometry.loc), from_wm_size(r.current_geometry.size)))
                });
            if let Some(rect) = rect {
                let win_cx = rect.loc.x as f64 + rect.size.w.max(120) as f64 * 0.5;
                let win_cy = rect.loc.y as f64 + rect.size.h.max(90) as f64 * 0.5;
                let dx = (origin.x as f64 - win_cx).clamp(-640.0, 640.0);
                let dy = (origin.y as f64 - win_cy).clamp(-480.0, 480.0);
                self.effects.animate_window_open_from(id, dx, dy, 0.26, now);
                return;
            }
        }
        self.effects.animate_window_open(id, now);
    }

    /// Queues a one-shot GPU closing ghost for a client-initiated window close (`×` button,
    /// `Ctrl+Q`, etc.) whenever the window was visible on the active workspace and not already
    /// playing a compositor-driven close or minimize animation.
    pub fn queue_close_ghost_if_visible(&mut self, id: WindowId, window: &Window) {
        if self.backend_name == "headless"
            || !self.effects.config.close_animation
            || self.closing_windows.contains_key(&id)
            || self.minimizing_windows.contains_key(&id)
            || is_window_frameless_special(window)
        {
            return;
        }
        let dur = self.effects.scaled_duration(290);
        if dur.is_zero() {
            return;
        }
        let rect = self
            .space
            .element_location(window)
            .and_then(|loc| {
                let sz = window.geometry().size;
                (sz.w > 24 && sz.h > 24).then(|| Rectangle::new(loc, sz))
            })
            .or_else(|| {
                self.last_rendered_rects
                    .get(&id)
                    .filter(|r| r.size.w > 24 && r.size.h > 24)
                    .map(|r| Rectangle::new(from_wm_point(r.loc), from_wm_size(r.size)))
            });
        let Some(rect) = rect else {
            return;
        };
        let fullscreen = self
            .wm
            .get(id)
            .map(|r| r.mode.is_fullscreen())
            .unwrap_or(false);
        let target_offset = self.close_swoop_delta_for(id);
        self.pending_close_ghosts.push(neon_renderer::PendingCloseGhost {
            window: window.clone(),
            rect,
            corner_radius: if fullscreen {
                0.0
            } else {
                self.effects.config.corner_radius
            },
            target_offset,
            duration: dur,
        });
        self.needs_redraw = true;
    }

    /// Requests graceful closure of a window with a smooth Hyprland/M3 popout close animation.
    pub fn close_window_by_id(&mut self, id: WindowId) -> bool {
        let Some(window) = self.find_window_by_id(id) else {
            return false;
        };
        if self.closing_windows.contains_key(&id) {
            return true;
        }
        self.pending_geometry_anims.remove(&id);
        // If the window is not currently mapped on the active workspace (e.g. minimized or on
        // another workspace), close it immediately without waiting for a hidden animation.
        let is_visible = self.space.element_location(&window).is_some()
            && !self.minimizing_windows.contains_key(&id);
        if !is_visible || self.backend_name == "headless" {
            self.needs_redraw = true;
            return self.send_native_close(id);
        }
        let (dx, dy) = self.close_swoop_delta_for(id);
        let now = self.now_duration();
        let dur = self.effects.animate_window_close_toward(id, dx, dy, now);
        self.needs_redraw = true;
        if dur.is_zero() {
            self.send_native_close(id)
        } else {
            self.closing_windows.insert(id, (now + dur, false));
            true
        }
    }

    /// Requests graceful closure of the currently focused toplevel window.
    pub fn close_focused_window(&mut self) {
        if let Some(focused_id) = self.wm.focused_id() {
            self.close_window_by_id(focused_id);
        }
    }

    /// Enables NEON DE's macOS server-side titlebar for `id` and ensures it does not overlap the panel.
    pub fn enable_ssd_for_window(&mut self, id: WindowId) {
        self.ssd_windows.insert(id);
        let work_area = self.work_area();
        let min_y = work_area.loc.y + MACOS_TITLEBAR_HEIGHT;
        if let Some(rec) = self.wm.get(id).cloned()
            && rec.current_geometry.loc.y < min_y
            && rec.mode.is_floating()
        {
            let mut new_loc = rec.current_geometry.loc;
            new_loc.y = min_y;
            self.wm.set_floating_location(id, new_loc);
            if let Some(window) = self.find_window_by_id(id)
                && rec.is_visible_on(self.wm.active_workspace_id())
            {
                self.map_preserving_z_order(window, from_wm_point(new_loc));
            }
        }
        self.needs_redraw = true;
    }

    /// Computes effective work area for a window, reserving space for a macOS SSD titlebar if active.
    pub fn effective_work_area_for(&self, id: WindowId) -> WmRect {
        let mut wa = self.work_area();
        if self.ssd_windows.contains(&id) {
            wa.loc.y += MACOS_TITLEBAR_HEIGHT;
            wa.size.h = (wa.size.h - MACOS_TITLEBAR_HEIGHT).max(200);
        }
        wa
    }

    /// Hit-tests `pos` against front-to-back SSD macOS titlebars and traffic-light controls.
    pub fn ssd_titlebar_hit_test(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WindowId, Window, TitlebarHit)> {
        let elements: Vec<&Window> = self.space.elements().collect();
        for window in elements.into_iter().rev() {
            let Some(id) = window_id(window) else {
                continue;
            };
            if !self.ssd_windows.contains(&id) {
                continue;
            }
            if self
                .wm
                .get(id)
                .map(|r| r.mode.is_fullscreen())
                .unwrap_or(false)
            {
                continue;
            }
            let Some(loc) = self.space.element_location(window) else {
                continue;
            };
            let w = window.geometry().size.w;
            if w <= 0 {
                continue;
            }
            let tb_top = (loc.y - MACOS_TITLEBAR_HEIGHT) as f64;
            let tb_bot = loc.y as f64;
            let tb_left = loc.x as f64;
            let tb_right = (loc.x + w) as f64;
            if pos.x >= tb_left && pos.x <= tb_right && pos.y >= tb_top && pos.y <= tb_bot {
                let lx = pos.x - tb_left;
                let ly = pos.y - tb_top;
                let cy = MACOS_TITLEBAR_HEIGHT as f64 * 0.5;
                let dist_sq = |cx: f64| (lx - cx).powi(2) + (ly - cy).powi(2);
                let hit_r_sq = 8.5_f64.powi(2);
                let buttons_right =
                    self.config.windows.button_layout == WindowButtonLayout::RightStandard;
                let (close_x, min_x, max_x) = if buttons_right {
                    (w as f64 - 16.0, w as f64 - 56.0, w as f64 - 36.0)
                } else {
                    (16.0, 36.0, 56.0)
                };
                let hit = if dist_sq(close_x) <= hit_r_sq {
                    TitlebarHit::Close
                } else if dist_sq(min_x) <= hit_r_sq {
                    TitlebarHit::Minimize
                } else if dist_sq(max_x) <= hit_r_sq {
                    TitlebarHit::Maximize
                } else {
                    TitlebarHit::Drag
                };
                return Some((id, window.clone(), hit));
            }
        }
        None
    }

    /// Explicitly sets maximize state for any window ID (Wayland or XWayland), cleanly
    /// reflowing Hyprland Auto-Tiling workspaces both on enter and on exit.
    pub fn set_window_maximized(&mut self, id: WindowId, maximize: bool) -> bool {
        let ws_id = self.wm.get(id).map(|w| w.workspace_id);
        let work_area = self.effective_work_area_for(id);
        let Some(update) = self.wm.set_maximized(id, maximize, work_area) else {
            return false;
        };
        info!(?id, mode = ?update.mode, "Updated maximize state on window");
        if update.mode.is_tiled() && self.wm.is_auto_tiling() {
            // Returning directly to Auto-Tiling: let workspace relayout issue one single
            // authoritative configure + M3 morph back into the window's preserved tile slot.
            if let Some(ws) = ws_id {
                self.relayout_auto_tiling_for_workspace(ws);
            }
            self.needs_redraw = true;
            return true;
        }
        self.begin_geometry_transition(id, update.configure_size);
        if let Some(window) = self.find_window_by_id(id)
            && let Some(x11) = window.x11_surface()
        {
            let _ = x11.set_maximized(update.mode.is_maximized());
            if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
            }
        }
        self.apply_layout_update(&update);
        if self.wm.is_auto_tiling()
            && let Some(ws) = ws_id
        {
            self.relayout_auto_tiling_for_workspace(ws);
            // Keep the newly maximized window raised cleanly above the reflowed tiles behind it.
            if update.mode.is_maximized()
                && let Some(window) = self.find_window_by_id(id)
            {
                self.space.raise_element(&window, false);
                self.sync_x11_stacking_order();
            }
        }
        self.needs_redraw = true;
        true
    }

    /// Toggles maximize state for any window ID (supports both Wayland and XWayland windows).
    pub fn toggle_maximize_window(&mut self, id: WindowId) {
        let next = !self
            .wm
            .get(id)
            .map(|r| r.mode.is_maximized())
            .unwrap_or(false);
        self.set_window_maximized(id, next);
    }

    // --- Window Management & Dynamic Workspace Actions ---

    pub fn toggle_maximize_focused(&mut self) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        self.toggle_maximize_window(id);
    }

    pub fn toggle_fullscreen_focused(&mut self) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        self.toggle_fullscreen_window(id);
    }

    pub fn set_window_fullscreen(&mut self, id: WindowId, entering_fs: bool) -> bool {
        let ws_id = self.wm.get(id).map(|w| w.workspace_id);
        if let Some(window) = self.find_window_by_id(id)
            && let Some(x11) = window.x11_surface()
        {
            if entering_fs {
                self.ssd_windows.remove(&id);
            } else if crate::wayland::xwayland::x11_wants_ssd_non_fullscreen(x11) {
                self.ssd_windows.insert(id);
            }
        }

        let target_rect = if entering_fs {
            self.output_full_rect()
        } else {
            self.effective_work_area_for(id)
        };
        let Some(update) = self.wm.set_fullscreen(id, entering_fs, target_rect) else {
            return false;
        };
        info!(?id, mode = ?update.mode, "Updated fullscreen state on window");
        if update.mode.is_tiled() && self.wm.is_auto_tiling() {
            if let Some(ws) = ws_id {
                self.relayout_auto_tiling_for_workspace(ws);
            }
            self.needs_redraw = true;
            return true;
        }
        self.begin_geometry_transition(id, update.configure_size);
        if let Some(window) = self.find_window_by_id(id)
            && let Some(x11) = window.x11_surface()
        {
            let _ = x11.set_fullscreen(update.mode.is_fullscreen());
            if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
            }
        }
        self.apply_layout_update(&update);
        if self.wm.is_auto_tiling()
            && let Some(ws) = ws_id
        {
            self.relayout_auto_tiling_for_workspace(ws);
            if update.mode.is_fullscreen()
                && let Some(window) = self.find_window_by_id(id)
            {
                self.space.raise_element(&window, false);
                self.sync_x11_stacking_order();
            }
        }
        self.needs_redraw = true;
        true
    }

    pub fn toggle_fullscreen_window(&mut self, id: WindowId) {
        let currently_fs = self
            .wm
            .get(id)
            .map(|r| r.mode.is_fullscreen())
            .unwrap_or(false);
        self.set_window_fullscreen(id, !currently_fs);
    }

    pub fn snap_window_by_id(&mut self, id: WindowId, region: SnapRegion) {
        let work_area = self.effective_work_area_for(id);
        if let Some(update) = self.wm.toggle_snap(id, region, work_area) {
            self.begin_geometry_transition(id, update.configure_size);
            info!(?id, mode = ?update.mode, "Updated window snap state");
            if let Some(window) = self.find_window_by_id(id)
                && let Some(x11) = window.x11_surface()
                && let (Some(loc), Some(sz)) = (update.target_location, update.configure_size)
            {
                let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
            }
            self.apply_layout_update(&update);
            self.needs_redraw = true;
        }
    }

    pub fn snap_window_explicit(&mut self, id: WindowId, region: SnapRegion) {
        let work_area = self.effective_work_area_for(id);
        if let Some(update) = self.wm.snap_to_region(id, region, work_area) {
            self.begin_geometry_transition(id, update.configure_size);
            info!(?id, mode = ?update.mode, "Snapped window via drag/layout bar");
            if let Some(window) = self.find_window_by_id(id)
                && let Some(x11) = window.x11_surface()
                && let (Some(loc), Some(sz)) = (update.target_location, update.configure_size)
            {
                let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
            }
            self.apply_layout_update(&update);
            self.needs_redraw = true;
        }
    }

    pub fn snap_focused(&mut self, region: SnapRegion) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        self.snap_window_by_id(id, region);
    }

    /// Applies a batch of layout updates (such as Hyprland Auto-Tiling reflow) with optional
    /// Material 3 Expressive geometry morphs.
    ///
    /// - Reserves `MACOS_TITLEBAR_HEIGHT` inside tiled rects for SSD windows so their titlebars
    ///   never protrude above the tile or overlap the Top Bar / sibling panes.
    /// - Skips redundant configures and animations for windows whose geometry and state did not change.
    pub fn apply_layout_updates_batch(
        &mut self,
        updates: Vec<WindowLayoutUpdate>,
        animate: bool,
    ) {
        let mut any_applied = false;
        for mut update in updates {
            if update.mode.is_tiled() && self.ssd_windows.contains(&update.id) {
                if let Some(loc) = &mut update.target_location {
                    loc.y += MACOS_TITLEBAR_HEIGHT;
                }
                if let Some(sz) = &mut update.configure_size {
                    sz.h = (sz.h - MACOS_TITLEBAR_HEIGHT).max(100);
                }
            }

            let unchanged = if let Some(window) = self.find_window_by_id(update.id) {
                let needs_initial = window
                    .toplevel()
                    .map(|t| !t.is_initial_configure_sent())
                    .unwrap_or(false);
                let cur_loc = self.space.element_location(&window);
                let cur_size = window.geometry().size;
                let loc_same = match (update.visible_on_active_workspace, update.target_location) {
                    (true, Some(loc)) => cur_loc == Some(from_wm_point(loc)),
                    (false, _) => cur_loc.is_none(),
                    _ => false,
                };
                let size_same = update
                    .configure_size
                    .is_none_or(|sz| cur_size == from_wm_size(sz));
                let state_same = window
                    .toplevel()
                    .map(|t| {
                        let st = t.current_state().states;
                        match &update.mode {
                            WindowMode::Tiled => {
                                st.contains(xdg_toplevel::State::TiledLeft)
                                    && st.contains(xdg_toplevel::State::TiledRight)
                                    && st.contains(xdg_toplevel::State::TiledTop)
                                    && st.contains(xdg_toplevel::State::TiledBottom)
                                    && !st.contains(xdg_toplevel::State::Maximized)
                                    && !st.contains(xdg_toplevel::State::Fullscreen)
                            }
                            WindowMode::Floating => {
                                !st.contains(xdg_toplevel::State::TiledLeft)
                                    && !st.contains(xdg_toplevel::State::TiledRight)
                                    && !st.contains(xdg_toplevel::State::Maximized)
                                    && !st.contains(xdg_toplevel::State::Fullscreen)
                            }
                            _ => false,
                        }
                    })
                    .unwrap_or(false);
                !needs_initial && loc_same && size_same && state_same
            } else {
                false
            };

            if unchanged {
                continue;
            }
            any_applied = true;

            if animate {
                self.begin_geometry_transition(update.id, update.configure_size);
            }
            if let Some(window) = self.find_window_by_id(update.id)
                && let Some(x11) = window.x11_surface()
            {
                let _ = x11.set_maximized(update.mode.is_maximized());
                let _ = x11.set_fullscreen(update.mode.is_fullscreen());
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
            }
            self.apply_layout_update(&update);
        }
        if any_applied {
            self.needs_redraw = true;
        }
    }

    pub fn relayout_auto_tiling_for_workspace(&mut self, ws_id: WorkspaceId) {
        if !self.wm.is_auto_tiling() {
            return;
        }
        let work_area = self.work_area();
        let updates = self.wm.relayout_workspace_tiles(ws_id, work_area);
        self.apply_layout_updates_batch(updates, true);
    }

    /// Enables or disables global Hyprland Auto-Tiling, reflowing all workspaces and persisting config.
    pub fn set_auto_tiling(&mut self, enabled: bool, persist: bool) {
        self.config.windows.auto_tiling = enabled;
        let work_area = self.work_area();
        let updates = self.wm.set_auto_tiling(enabled, work_area);
        self.apply_layout_updates_batch(updates, true);
        info!(enabled, layout = ?self.wm.tiling_layout(), "Updated Hyprland Auto-Tiling");
        if persist && self.backend_name != "headless" {
            let cfg_path = neon_config::default_config_path();
            if let Err(err) = neon_config::save_atomic(&cfg_path, &self.config) {
                warn!(?err, "Failed to persist auto-tiling toggle");
            }
        }
    }

    pub fn toggle_auto_tiling(&mut self) {
        let next = !self.wm.is_auto_tiling();
        self.set_auto_tiling(next, true);
    }

    pub fn cycle_tiling_layout(&mut self) {
        let next = match self.config.windows.tiling_layout {
            ConfigTilingLayout::Dwindle => ConfigTilingLayout::MasterStack,
            ConfigTilingLayout::MasterStack => ConfigTilingLayout::Dwindle,
        };
        self.set_tiling_layout(next, true);
    }

    pub fn set_tiling_layout(&mut self, layout: ConfigTilingLayout, persist: bool) {
        self.config.windows.tiling_layout = layout;
        let wm_layout = match layout {
            ConfigTilingLayout::Dwindle => WmTilingLayout::Dwindle,
            ConfigTilingLayout::MasterStack => WmTilingLayout::MasterStack,
        };
        let work_area = self.work_area();
        let updates = self.wm.set_tiling_layout(wm_layout, work_area);
        self.apply_layout_updates_batch(updates, true);
        info!(?layout, "Updated Hyprland Auto-Tiling layout");
        if persist && self.backend_name != "headless" {
            let cfg_path = neon_config::default_config_path();
            if let Err(err) = neon_config::save_atomic(&cfg_path, &self.config) {
                warn!(?err, "Failed to persist tiling layout");
            }
        }
    }

    pub fn toggle_window_float(&mut self, target_id: Option<WindowId>) {
        let Some(id) = target_id.or_else(|| self.wm.focused_id()) else {
            return;
        };
        let work_area = self.effective_work_area_for(id);
        let updates = self.wm.toggle_window_float(id, work_area);
        info!(?id, count = updates.len(), "Toggled window floating override");
        self.apply_layout_updates_batch(updates, true);
        // Ensure a newly floated window rises cleanly above the tiled layer.
        if self.wm.is_auto_tiling()
            && self.wm.get(id).is_some_and(|w| w.mode.is_floating())
            && let Some(window) = self.find_window_by_id(id)
        {
            self.space.raise_element(&window, false);
            self.sync_x11_stacking_order();
        }
    }

    pub fn focus_tiled_in_direction(&mut self, dx: i32, dy: i32, serial: Serial) {
        if !self.wm.is_auto_tiling() {
            return;
        }
        let Some(cur_id) = self.wm.focused_id() else {
            self.focus_topmost_window(serial);
            return;
        };
        if let Some(target_id) = self.wm.focus_tiled_in_direction(cur_id, dx, dy)
            && let Some(window) = self.find_window_by_id(target_id)
        {
            self.focus_window(Some(&window), serial);
            self.needs_redraw = true;
        }
    }

    pub fn swap_focused_tile_in_direction(&mut self, dx: i32, dy: i32) {
        if !self.wm.is_auto_tiling() {
            return;
        }
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let work_area = self.work_area();
        let updates = self.wm.swap_tiled_in_direction(id, dx, dy, work_area);
        if !updates.is_empty() {
            self.apply_layout_updates_batch(updates, true);
        }
    }

    pub fn resize_focused_tile(&mut self, dx: f32, dy: f32) {
        if !self.wm.is_auto_tiling() {
            return;
        }
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let work_area = self.work_area();
        let updates = self.wm.resize_tiled_window(id, dx, dy, work_area);
        if !updates.is_empty() {
            self.apply_layout_updates_batch(updates, true);
        }
    }

    pub fn reset_active_workspace_tile_ratios(&mut self) {
        if !self.wm.is_auto_tiling() {
            return;
        }
        let ws = self.wm.active_workspace_id();
        let work_area = self.work_area();
        let updates = self.wm.reset_workspace_split_ratios(ws, work_area);
        if !updates.is_empty() {
            self.apply_layout_updates_batch(updates, true);
        }
    }

    pub fn toggle_focused_split_orientation(&mut self) {
        if !self.wm.is_auto_tiling() {
            return;
        }
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let work_area = self.work_area();
        let updates = self.wm.toggle_split_orientation(id, work_area);
        if !updates.is_empty() {
            self.apply_layout_updates_batch(updates, true);
        }
    }

    pub fn promote_focused_to_master(&mut self) {
        if !self.wm.is_auto_tiling() {
            return;
        }
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let work_area = self.work_area();
        let updates = self.wm.promote_to_master(id, work_area);
        if !updates.is_empty() {
            self.apply_layout_updates_batch(updates, true);
        }
    }

    /// Updates live drag-to-edge / Windows 11 Snap Layouts bar / Auto-Tiling drag-swap preview.
    pub fn update_drag_snap(&mut self, dragged_id: WindowId, pointer: Point<f64, Logical>) {
        let anim_on = self.effects.config.enabled && !self.effects.config.reduced_motion;
        let is_tiled = self
            .wm
            .get(dragged_id)
            .map(|w| w.mode.is_tiled())
            .unwrap_or(false);
        if self.wm.is_auto_tiling() && is_tiled {
            let ws_id = self
                .wm
                .get(dragged_id)
                .map(|w| w.workspace_id)
                .unwrap_or_else(|| self.wm.active_workspace_id());
            let target = self.wm.tiled_window_at_point(
                ws_id,
                pointer.x.round() as i32,
                pointer.y.round() as i32,
                dragged_id,
            );
            self.drag_snap
                .update_tiling_swap(dragged_id, target, anim_on);
        } else {
            let work_area = self.effective_work_area_for(dragged_id);
            self.drag_snap.update_floating_drag(
                dragged_id,
                pointer.x,
                pointer.y,
                work_area,
                self.wm.snap_gap(),
                self.config.windows.auto_snap,
                self.config.windows.snap_layouts_bar,
                anim_on,
            );
        }
        self.needs_redraw = true;
    }

    /// Commits the active drag snap/maximize/tile-swap target on pointer release and clears overlay.
    pub fn commit_drag_snap(&mut self, dragged_id: WindowId) {
        let target = self.drag_snap.target;
        self.drag_snap.clear();

        let is_tiled = self
            .wm
            .get(dragged_id)
            .map(|w| w.mode.is_tiled())
            .unwrap_or(false);

        match target {
            Some(DragSnapTarget::Region(region)) => {
                self.snap_window_explicit(dragged_id, region);
            }
            Some(DragSnapTarget::Maximize) => {
                let is_max = self
                    .wm
                    .get(dragged_id)
                    .map(|w| w.mode.is_maximized())
                    .unwrap_or(false);
                if !is_max {
                    self.toggle_maximize_window(dragged_id);
                }
            }
            Some(DragSnapTarget::SwapTile { target_id, .. }) => {
                let work_area = self.work_area();
                let updates = self
                    .wm
                    .swap_tiled_windows(dragged_id, target_id, work_area);
                self.apply_layout_updates_batch(updates, true);
            }
            None => {
                // If dragging a tiled window and released without swapping, snap smoothly back to its tile.
                if is_tiled
                    && let Some(ws_id) = self.wm.get(dragged_id).map(|w| w.workspace_id)
                {
                    self.relayout_auto_tiling_for_workspace(ws_id);
                }
            }
        }
        self.needs_redraw = true;
    }

    pub fn toggle_always_on_top_by_id(&mut self, id: WindowId, serial: Serial) -> Option<bool> {
        let enabled = self.wm.toggle_always_on_top(id)?;
        info!(?id, always_on_top = enabled, "Toggled window Always on Top");
        self.sync_active_workspace_space(serial);
        self.needs_redraw = true;
        Some(enabled)
    }

    pub fn center_window_by_id(&mut self, id: WindowId) -> bool {
        let ws_id = self.wm.get(id).map(|w| w.workspace_id);
        let was_tiled = self.wm.get(id).is_some_and(|w| w.mode.is_tiled());
        let work_area = self.effective_work_area_for(id);
        let Some(update) = self.wm.center_window(id, work_area) else {
            return false;
        };
        self.begin_geometry_transition(id, update.configure_size);
        if let Some(window) = self.find_window_by_id(id)
            && let Some(x11) = window.x11_surface()
        {
            let _ = x11.set_maximized(false);
            let _ = x11.set_fullscreen(false);
            if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
            }
        }
        self.apply_layout_update(&update);
        if was_tiled
            && self.wm.is_auto_tiling()
            && let Some(ws) = ws_id
        {
            self.relayout_auto_tiling_for_workspace(ws);
            if let Some(window) = self.find_window_by_id(id) {
                self.space.raise_element(&window, false);
                self.sync_x11_stacking_order();
            }
        }
        self.needs_redraw = true;
        true
    }

    pub fn set_floating_window_by_id(&mut self, id: WindowId) -> bool {
        let ws_id = self.wm.get(id).map(|w| w.workspace_id);
        let was_tiled = self.wm.get(id).is_some_and(|w| w.mode.is_tiled());
        if self.wm.is_auto_tiling() {
            self.wm.set_float_override(id, true);
        }
        let work_area = self.effective_work_area_for(id);
        let Some(update) = self.wm.set_floating(id, work_area) else {
            return false;
        };
        self.begin_geometry_transition(id, update.configure_size);
        if let Some(window) = self.find_window_by_id(id)
            && let Some(x11) = window.x11_surface()
        {
            let _ = x11.set_maximized(false);
            let _ = x11.set_fullscreen(false);
            if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
            }
        }
        self.apply_layout_update(&update);
        if was_tiled
            && self.wm.is_auto_tiling()
            && let Some(ws) = ws_id
        {
            self.relayout_auto_tiling_for_workspace(ws);
            if let Some(window) = self.find_window_by_id(id) {
                self.space.raise_element(&window, false);
                self.sync_x11_stacking_order();
            }
        }
        self.needs_redraw = true;
        true
    }

    pub fn move_window_to_output_by_name(&mut self, id: WindowId, output_name: &str) -> bool {
        let Some(out) = self
            .output_manager
            .all_outputs()
            .into_iter()
            .find(|o| o.name == output_name)
        else {
            return false;
        };
        let logical = out.logical_size();
        let top_pad = if out.primary && self.config.panel.visible && !self.config.panel.smart_hide {
            31
        } else {
            0
        };
        let bot_pad = if out.primary && !self.config.dock.auto_hide && !self.config.dock.intelligent_hide
        {
            self.last_dock_exclusive.max(0)
        } else {
            0
        };
        let target_area = WmRect::new(
            crate::window::WmPoint::new(out.position.x, out.position.y + top_pad),
            crate::window::WmSize::new(logical.w.max(320), (logical.h - top_pad - bot_pad).max(200)),
        );
        let Some(update) = self.wm.move_window_to_area(id, target_area) else {
            return false;
        };
        if let Some(window) = self.find_window_by_id(id)
            && let Some(x11) = window.x11_surface()
            && let (Some(loc), Some(sz)) = (update.target_location, update.configure_size)
        {
            let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
        }
        self.apply_layout_update(&update);
        self.needs_redraw = true;
        true
    }

    pub fn unmaximize_or_minimize_focused(&mut self, serial: Serial) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let mode = self
            .wm
            .get(id)
            .map(|w| w.mode.clone())
            .unwrap_or(WindowMode::Floating);
        if self.wm.is_auto_tiling() {
            match mode {
                WindowMode::Maximized => {
                    self.set_window_maximized(id, false);
                }
                WindowMode::Fullscreen => {
                    self.set_window_fullscreen(id, false);
                }
                _ => {
                    self.minimize_window(id, serial);
                }
            }
            return;
        }
        if mode.is_floating() {
            self.minimize_window(id, serial);
        } else {
            let work_area = self.effective_work_area_for(id);
            if let Some(update) = self.wm.set_floating(id, work_area) {
                self.begin_geometry_transition(id, update.configure_size);
                info!(?id, "Restored window to floating mode");
                self.apply_layout_update(&update);
                self.needs_redraw = true;
            }
        }
    }

    fn dock_swoop_delta_for(&self, id: WindowId) -> (f64, f64) {
        let out_sz = self.output_logical_size();
        let (dock_x, dock_y) = if let Some(anchor) = self.window_dock_anchors.get(&id) {
            (anchor.x as f64, anchor.y as f64)
        } else {
            match self.config.dock.position {
                DockPosition::Left => (28.0, out_sz.h as f64 * 0.5),
                DockPosition::Right => ((out_sz.w - 28).max(100) as f64, out_sz.h as f64 * 0.5),
                DockPosition::Bottom => (out_sz.w as f64 * 0.5, (out_sz.h - 28).max(100) as f64),
            }
        };
        if let Some(rect) = self
            .last_rendered_rects
            .get(&id)
            .copied()
            .or_else(|| self.wm.get(id).map(|r| r.current_geometry))
        {
            let cx = rect.loc.x as f64 + rect.size.w as f64 * 0.5;
            let cy = rect.loc.y as f64 + rect.size.h as f64 * 0.5;
            (dock_x - cx, dock_y - cy)
        } else {
            match self.config.dock.position {
                DockPosition::Left => (-260.0, 0.0),
                DockPosition::Right => (260.0, 0.0),
                DockPosition::Bottom => (0.0, 260.0),
            }
        }
    }

    /// Android-style return-to-icon directional glide toward the window's Dock icon on close.
    fn close_swoop_delta_for(&self, id: WindowId) -> (f64, f64) {
        let (dx, dy) = self.dock_swoop_delta_for(id);
        let pull_x = (dx * 0.56).clamp(-300.0, 300.0);
        let pull_y = match self.config.dock.position {
            DockPosition::Bottom => (dy * 0.56).clamp(64.0, 260.0),
            DockPosition::Left | DockPosition::Right => (dy * 0.56).clamp(-220.0, 220.0),
        };
        (pull_x, pull_y)
    }

    pub fn minimize_window(&mut self, id: WindowId, serial: Serial) {
        let (dx, dy) = self.dock_swoop_delta_for(id);
        let ws_id = self.wm.get(id).map(|w| w.workspace_id);
        let work_area = self.work_area();
        if let Some(update) = self.wm.minimize(id, work_area) {
            info!(?id, "Minimized window");
            self.pending_geometry_anims.remove(&id);
            let now = self.now_duration();
            let dur = self.effects.animate_window_minimize(id, dx, dy, now);
            if dur.is_zero() || self.backend_name == "headless" {
                self.apply_layout_update(&update);
            } else {
                // Keep window visible in Space while it swoops smoothly into its Dock icon.
                self.minimizing_windows.insert(id, now + dur);
                self.needs_redraw = true;
            }
            if let Some(ws) = ws_id {
                self.relayout_auto_tiling_for_workspace(ws);
            }
            self.focus_topmost_window(serial);
        }
    }

    pub fn minimize_focused(&mut self, serial: Serial) {
        if let Some(id) = self.wm.focused_id() {
            self.minimize_window(id, serial);
        }
    }

    /// Restores a specific minimized window with a smooth Material 3 spring rise from the Dock.
    pub fn restore_minimized_window(&mut self, id: WindowId, serial: Serial) -> bool {
        let restores_to_fs = self
            .wm
            .get(id)
            .map(|r| {
                matches!(
                    &r.mode,
                    WindowMode::Minimized { restore_mode } if restore_mode.is_fullscreen()
                )
            })
            .unwrap_or(false);
        let work_area = if restores_to_fs {
            self.output_full_rect()
        } else {
            self.effective_work_area_for(id)
        };
        let Some(update) = self.wm.restore_window(id, work_area) else {
            return false;
        };
        self.minimizing_windows.remove(&id);
        self.pending_geometry_anims.remove(&id);
        self.last_rendered_rects.remove(&id);
        let tile_on_restore = self.wm.is_auto_tiling() && self.wm.is_tile_eligible(id);
        if tile_on_restore {
            if let Some(ws) = self.wm.get(id).map(|w| w.workspace_id) {
                self.relayout_auto_tiling_for_workspace(ws);
            }
        } else {
            self.apply_layout_update(&update);
            if let Some(ws) = self.wm.get(id).map(|w| w.workspace_id) {
                self.relayout_auto_tiling_for_workspace(ws);
            }
        }
        let (dx, dy) = self.dock_swoop_delta_for(id);
        let now = self.now_duration();
        self.effects.animate_window_restore(id, dx, dy, now);
        self.needs_redraw = true;
        if let Some(window) = self.find_window_by_id(id) {
            self.focus_window(Some(&window), serial);
        }
        true
    }

    pub fn restore_last_minimized(&mut self, serial: Serial) {
        let work_area = self.work_area();
        if let Some(mut update) = self.wm.restore_last_minimized(work_area) {
            if update.mode.is_fullscreen() {
                let full = self.output_full_rect();
                if let Some(fs_upd) = self.wm.apply_mode(update.id, WindowMode::Fullscreen, full) {
                    update = fs_upd;
                }
            }
            info!(id = ?update.id, mode = ?update.mode, "Restored minimized window");
            self.minimizing_windows.remove(&update.id);
            self.pending_geometry_anims.remove(&update.id);
            self.last_rendered_rects.remove(&update.id);
            let tile_on_restore = self.wm.is_auto_tiling() && self.wm.is_tile_eligible(update.id);
            if tile_on_restore {
                if let Some(ws) = self.wm.get(update.id).map(|w| w.workspace_id) {
                    self.relayout_auto_tiling_for_workspace(ws);
                }
            } else {
                self.apply_layout_update(&update);
                if let Some(ws) = self.wm.get(update.id).map(|w| w.workspace_id) {
                    self.relayout_auto_tiling_for_workspace(ws);
                }
            }
            let (dx, dy) = self.dock_swoop_delta_for(update.id);
            let now = self.now_duration();
            self.effects.animate_window_restore(update.id, dx, dy, now);
            self.needs_redraw = true;
            if let Some(window) = self.find_window_by_id(update.id) {
                self.focus_window(Some(&window), serial);
            }
        }
    }

    /// Captures currently mapped windows on the departing workspace and initiates a
    /// Hyprland-style simultaneous dual-workspace `slidefade` animation across the display width.
    fn begin_workspace_transition(&mut self, direction: i32, exclude_id: Option<WindowId>) {
        let now = self.now_duration();
        let output_w = self.output_logical_size().w.max(640) as f64;
        let focused_id = self.wm.focused_id();

        let snapshot: Vec<OutgoingWorkspaceWindow> = self
            .space
            .elements()
            .enumerate()
            .filter_map(|(z_idx, window)| {
                let id = window_id(window)?;
                if exclude_id == Some(id) || self.minimizing_windows.contains_key(&id) {
                    return None;
                }
                let loc = self.space.element_location(window)?;
                let win_size = window.geometry().size;
                if win_size.w <= 0 || win_size.h <= 0 {
                    return None;
                }
                let mode = self
                    .wm
                    .get(id)
                    .map(|w| w.mode.clone())
                    .unwrap_or(WindowMode::Floating);
                Some(OutgoingWorkspaceWindow {
                    id,
                    bounds: WmRect::new(to_wm_point(loc), to_wm_size(win_size)),
                    z_index: z_idx as i32,
                    fullscreen: mode.is_fullscreen(),
                    maximized: mode.is_maximized(),
                    was_focused: focused_id == Some(id),
                    is_frameless_special: is_window_frameless_special(window),
                })
            })
            .collect();

        if self
            .effects
            .animate_workspace_switch_with_width(direction, output_w, now)
        {
            self.outgoing_workspace_windows = snapshot;
            self.needs_redraw = true;
        } else {
            self.outgoing_workspace_windows.clear();
        }
    }

    pub fn switch_workspace_index(&mut self, index: usize, serial: Serial) -> Option<WorkspaceId> {
        let old_idx = self.wm.workspaces().active_index();
        let old_ws = self.wm.active_workspace_id();
        if old_idx == index {
            return Some(old_ws);
        }
        let new_ws = self.wm.switch_workspace_index_or_create(index)?;
        if new_ws != old_ws {
            let direction = if index > old_idx { 1 } else { -1 };
            self.begin_workspace_transition(direction, None);
        }
        info!(
            workspace = ?new_ws,
            index = self.wm.workspaces().active_index(),
            total = self.wm.workspaces().len(),
            "Switched active workspace"
        );
        self.sync_active_workspace_space(serial);
        Some(new_ws)
    }

    pub fn switch_workspace_next(&mut self, serial: Serial) -> Option<WorkspaceId> {
        let old_ws = self.wm.active_workspace_id();
        let new_ws = self.wm.switch_workspace_next()?;
        if new_ws != old_ws {
            self.begin_workspace_transition(1, None);
        }
        info!(
            workspace = ?new_ws,
            index = self.wm.workspaces().active_index(),
            total = self.wm.workspaces().len(),
            "Switched to next workspace"
        );
        self.sync_active_workspace_space(serial);
        Some(new_ws)
    }

    pub fn switch_workspace_prev(&mut self, serial: Serial) -> Option<WorkspaceId> {
        let old_ws = self.wm.active_workspace_id();
        let new_ws = self.wm.switch_workspace_prev()?;
        if new_ws != old_ws {
            self.begin_workspace_transition(-1, None);
        }
        info!(
            workspace = ?new_ws,
            index = self.wm.workspaces().active_index(),
            total = self.wm.workspaces().len(),
            "Switched to previous workspace"
        );
        self.sync_active_workspace_space(serial);
        Some(new_ws)
    }

    /// Adds a new persistent desktop (up to 10 total).
    pub fn add_workspace(&mut self) -> Option<(WorkspaceId, usize)> {
        let res = self.wm.add_workspace()?;
        self.needs_redraw = true;
        info!(
            workspace = ?res.0,
            index = res.1,
            total = self.wm.workspaces().len(),
            "Created new desktop"
        );
        Some(res)
    }

    /// Removes the desktop at `index` (keeping at least 1 desktop) and syncs active space.
    pub fn remove_workspace(&mut self, index: usize, serial: Serial) -> bool {
        if !self.wm.remove_workspace(index) {
            return false;
        }
        self.outgoing_workspace_windows.clear();
        if self.wm.is_auto_tiling() {
            let wa = self.work_area();
            let updates = self.wm.relayout_all_tiles(wa);
            self.apply_layout_updates_batch(updates, true);
        }
        self.sync_active_workspace_space(serial);
        self.needs_redraw = true;
        info!(
            index,
            total = self.wm.workspaces().len(),
            "Removed desktop"
        );
        true
    }

    /// Toggles pinned persistence on the desktop at `index` (COSMIC-style workspace pin).
    pub fn toggle_pin_workspace(&mut self, index: usize, serial: Serial) -> Option<bool> {
        let pinned = self.wm.toggle_pin_workspace(index)?;
        self.sync_active_workspace_space(serial);
        self.needs_redraw = true;
        info!(
            index,
            pinned,
            total = self.wm.workspaces().len(),
            "Toggled workspace pin"
        );
        Some(pinned)
    }

    pub fn move_focused_to_next_workspace(
        &mut self,
        follow: bool,
        serial: Serial,
    ) -> Option<WorkspaceId> {
        let id = self.wm.focused_id()?;
        let old_ws = self.wm.active_workspace_id();
        if let Some(new_ws) = self.wm.move_window_to_next_workspace(id, follow) {
            if follow && new_ws != old_ws {
                self.begin_workspace_transition(1, Some(id));
            }
            self.relayout_auto_tiling_for_workspace(old_ws);
            self.relayout_auto_tiling_for_workspace(new_ws);
            info!(
                ?id,
                workspace = ?new_ws,
                follow,
                total = self.wm.workspaces().len(),
                "Moved focused window to next workspace"
            );
            self.sync_active_workspace_space(serial);
            if follow && let Some(window) = self.find_window_by_id(id) {
                self.focus_window(Some(&window), serial);
            }
            return Some(new_ws);
        }
        None
    }

    pub fn move_focused_to_prev_workspace(
        &mut self,
        follow: bool,
        serial: Serial,
    ) -> Option<WorkspaceId> {
        let id = self.wm.focused_id()?;
        let old_ws = self.wm.active_workspace_id();
        let new_ws = self.wm.move_window_to_prev_workspace(id, follow)?;
        if follow && new_ws != old_ws {
            self.begin_workspace_transition(-1, Some(id));
        }
        self.relayout_auto_tiling_for_workspace(old_ws);
        self.relayout_auto_tiling_for_workspace(new_ws);
        info!(
            ?id,
            workspace = ?new_ws,
            follow,
            total = self.wm.workspaces().len(),
            "Moved focused window to previous workspace"
        );
        self.sync_active_workspace_space(serial);
        if follow && let Some(window) = self.find_window_by_id(id) {
            self.focus_window(Some(&window), serial);
        }
        Some(new_ws)
    }

    /// Re-lays out maximized/fullscreen/snapped/tiled windows when the output or exclusive zone changes,
    /// strictly preserving existing window Z-order.
    pub fn relayout_non_floating_windows(&mut self) {
        if self.wm.is_auto_tiling() {
            let wa = self.work_area();
            let updates = self.wm.relayout_all_tiles(wa);
            self.apply_layout_updates_batch(updates, false);
        }
        let full_rect = self.output_full_rect();
        let ids: Vec<(WindowId, WindowMode)> = self
            .wm
            .all_window_ids()
            .into_iter()
            .filter_map(|id| {
                let mode = self.wm.get(id)?.mode.clone();
                (!mode.is_floating() && !mode.is_minimized() && !mode.is_tiled())
                    .then_some((id, mode))
            })
            .collect();

        for (id, mode) in ids {
            let target_rect = if mode.is_fullscreen() {
                full_rect
            } else {
                self.effective_work_area_for(id)
            };
            if let Some(update) = self.wm.apply_mode(id, mode, target_rect) {
                if let Some(window) = self.find_window_by_id(id)
                    && let Some(x11) = window.x11_surface()
                    && let (Some(loc), Some(sz)) = (update.target_location, update.configure_size)
                {
                    let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
            }
        }
    }

    /// Spawns a Wayland client process wired strictly to NEON DE's Wayland & IPC sockets.
    pub fn spawn_client(&self, program: &str, args: &[String]) {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env_remove("DISPLAY")
            .env("XDG_SESSION_TYPE", "wayland")
            .env("XDG_CURRENT_DESKTOP", "NEON-DE");

        if let Some(ipc_path) = &self.ipc_socket_path {
            cmd.env("NEON_IPC_SOCKET", ipc_path);
        }
        if let Some(x_display) = self.xwayland_display {
            cmd.env("DISPLAY", format!(":{x_display}"));
        }

        match cmd.spawn() {
            Ok(child) => {
                info!(
                    pid = child.id(),
                    program,
                    wayland_display = ?self.socket_name,
                    "Spawned Wayland client inside NEON DE"
                );
            }
            Err(err) => {
                warn!(program, ?err, "Failed to spawn Wayland client");
            }
        }
    }

    /// Clamps a logical pointer coordinate cleanly within the active output layout bounds.
    pub fn clamp_pointer_location(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let outputs = self.output_manager.all_outputs();
        if outputs.is_empty() {
            let sz = self.output_logical_size();
            return (
                pos.x.clamp(0.0, (sz.w - 1).max(0) as f64),
                pos.y.clamp(0.0, (sz.h - 1).max(0) as f64),
            )
                .into();
        }
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for out in outputs {
            let r = out.logical_rect();
            min_x = min_x.min(r.loc.x as f64);
            min_y = min_y.min(r.loc.y as f64);
            max_x = max_x.max((r.loc.x + r.size.w - 1).max(r.loc.x) as f64);
            max_y = max_y.max((r.loc.y + r.size.h - 1).max(r.loc.y) as f64);
        }
        (pos.x.clamp(min_x, max_x), pos.y.clamp(min_y, max_y)).into()
    }

    /// Requests a VT switch (`1..=12`) on bare-metal sessions via `libseat`.
    pub fn switch_vt(&mut self, vt: i32) {
        if let Some(hook) = self.vt_switch_hook.as_mut() {
            info!(vt, "Switching virtual terminal via libseat");
            hook(vt);
        }
    }

    /// Queues a native PNG screenshot (full display or `[x, y, w, h]` crop) for the next render pass.
    pub fn request_screenshot(
        &mut self,
        custom_path: Option<PathBuf>,
        region: Option<[i32; 4]>,
    ) -> PathBuf {
        let dest = custom_path.unwrap_or_else(|| {
            let base = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("Pictures/Screenshots");
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            base.join(format!("NEON-Screenshot-{ts}.png"))
        });
        info!(path = %dest.display(), ?region, "Queued native NEON DE screenshot");
        self.pending_screenshot = Some((dest.clone(), region));
        dest
    }

    /// Post-processes a saved screenshot in the background: copies to clipboard and notifies user.
    pub fn notify_screenshot_saved(&self, path: PathBuf) {
        let wl_display = self.socket_name.clone();
        std::thread::spawn(move || {
            // Copy PNG to Wayland clipboard via wl-copy if available.
            if let Ok(file) = std::fs::File::open(&path) {
                let _ = Command::new("wl-copy")
                    .args(["--type", "image/png"])
                    .env("WAYLAND_DISPLAY", &wl_display)
                    .stdin(std::process::Stdio::from(file))
                    .status();
            }
            // Deliver desktop notification toast cleanly via gdbus.
            let body = format!("Saved to {} (copied to clipboard)", path.display());
            for dest in ["org.freedesktop.Notifications", "org.neonde.Notifications"] {
                if Command::new("gdbus")
                    .args([
                        "call",
                        "--session",
                        "--dest",
                        dest,
                        "--object-path",
                        "/org/freedesktop/Notifications",
                        "--method",
                        "org.freedesktop.Notifications.Notify",
                        "NEON DE",
                        "0",
                        "camera-photo-symbolic",
                        "Screenshot Saved",
                        &body,
                        "[]",
                        "{}",
                        "4000",
                    ])
                    .output()
                    .is_ok_and(|o| o.status.success())
                {
                    break;
                }
            }
        });
    }

    /// Spawns the first available Wayland-native terminal inside NEON DE.
    pub fn spawn_terminal(&self) {
        const CANDIDATES: &[&str] = &[
            "alacritty",
            "kitty",
            "weston-terminal",
            "foot",
            "gnome-terminal",
            "xterm",
        ];
        for candidate in CANDIDATES {
            let found = Command::new("which")
                .arg(candidate)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if found {
                self.spawn_client(candidate, &[]);
                return;
            }
        }
        warn!(
            "No supported Wayland terminal found in PATH (tried weston-terminal, alacritty, kitty, foot)"
        );
    }

    /// Inserts an accepted Unix stream as a new Wayland client.
    pub fn insert_client(&mut self, stream: std::os::unix::net::UnixStream) {
        if let Err(err) = self
            .display_handle
            .insert_client(stream, Arc::new(NeonClientData::default()))
        {
            warn!(?err, "Failed to insert Wayland client into display");
        }
    }

    /// Removes dead windows across all workspaces and refreshes Space, SceneGraph, and Effects.
    pub fn cleanup_dead_windows(&mut self) {
        let dead_ids: Vec<WindowId> = self
            .managed_windows
            .iter()
            .filter(|(_, w)| !w.alive())
            .map(|(&id, _)| id)
            .collect();

        let mut lost_focus = false;
        let mut affected_workspaces: HashSet<WorkspaceId> = HashSet::new();
        for id in dead_ids {
            if let Some(ws_id) = self.wm.get(id).map(|w| w.workspace_id) {
                affected_workspaces.insert(ws_id);
            }
            if let Some(window) = self.managed_windows.remove(&id) {
                self.queue_close_ghost_if_visible(id, &window);
                self.space.unmap_elem(&window);
            }
            self.ssd_windows.remove(&id);
            self.last_rendered_rects.remove(&id);
            self.pending_geometry_anims.remove(&id);
            self.window_dock_anchors.remove(&id);
            self.minimizing_windows.remove(&id);
            self.closing_windows.remove(&id);
            if self.hovered_titlebar_window == Some(id) {
                self.hovered_titlebar_window = None;
            }
            if self.drag_snap.dragged_window == Some(id) {
                self.drag_snap.clear();
            }
            if self.wm.focused_id() == Some(id) {
                lost_focus = true;
            }
            self.wm.remove_window(id);
            self.effects.remove_window(id);
            self.scene.remove_window(id);
            self.outgoing_workspace_windows.retain(|w| w.id != id);
            debug!(?id, "Cleaned up dead window");
        }

        if self.wm.is_auto_tiling() {
            for ws_id in affected_workspaces {
                self.relayout_auto_tiling_for_workspace(ws_id);
            }
        }

        self.override_redirect_windows.retain(|w| {
            let alive = w.alive();
            if !alive {
                self.space.unmap_elem(w);
            }
            alive
        });
        self.space.refresh();
        self.popups.cleanup();

        if lost_focus {
            self.focus_topmost_window(SERIAL_COUNTER.next_serial());
        }
    }
}

fn to_smithay_transform(t: OutputTransform) -> Transform {
    match t {
        OutputTransform::Normal => Transform::Normal,
        OutputTransform::Rotate90 => Transform::_90,
        OutputTransform::Rotate180 => Transform::_180,
        OutputTransform::Rotate270 => Transform::_270,
        OutputTransform::Flipped => Transform::Flipped,
        OutputTransform::Flipped90 => Transform::Flipped90,
        OutputTransform::Flipped180 => Transform::Flipped180,
        OutputTransform::Flipped270 => Transform::Flipped270,
    }
}

fn parse_hex_rgb(hex: &str) -> Option<[f32; 3]> {
    let stripped = hex.strip_prefix('#')?;
    if stripped.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&stripped[0..2], 16).ok()?;
    let g = u8::from_str_radix(&stripped[2..4], 16).ok()?;
    let b = u8::from_str_radix(&stripped[4..6], 16).ok()?;
    Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
}

fn is_window_frameless_special(window: &Window) -> bool {
    window
        .x11_surface()
        .map(|x11| {
            x11.is_popup()
                || matches!(
                    x11.window_type(),
                    Some(
                        WmWindowType::Splash
                            | WmWindowType::Notification
                            | WmWindowType::Tooltip
                            | WmWindowType::PopupMenu
                            | WmWindowType::DropdownMenu
                            | WmWindowType::Menu
                            | WmWindowType::Toolbar
                    )
                )
        })
        .unwrap_or(false)
}
