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

use neon_config::{FocusMode, NeonConfig, ThemeMode};
use neon_effects::{EffectsConfig, EffectsManager};
use neon_output::{DisplayMode, OutputConfigPatch, OutputId, OutputManager, OutputTransform};
use neon_scene::{BorderStyle, MACOS_TITLEBAR_HEIGHT, SceneGraph, ShadowStyle, TitlebarStyle};
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
            Display, DisplayHandle, Resource,
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
        dmabuf::{DmabufGlobal, DmabufState},
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
    xwayland::X11Wm,
};
use tracing::{debug, info, warn};

use crate::{
    wayland::session_lock::SessionLockState,
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
    /// Windows currently playing their dock-swoop minimize animation (`id -> unmap_at`).
    pub minimizing_windows: HashMap<WindowId, Duration>,
    /// Windows currently playing their popout close animation (`id -> (deadline, close_sent)`).
    pub closing_windows: HashMap<WindowId, (Duration, bool)>,
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
    /// Signals `neon-shell` to open the GNOME-style interactive screenshot overlay.
    pub pending_screenshot_ui: bool,
    /// Signals `neon-shell` to open the OS-level right-click context menu at `(x, y)`.
    pub pending_context_menu: Option<(i32, i32, neon_ipc::ContextMenuTarget)>,
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
            minimizing_windows: HashMap::new(),
            closing_windows: HashMap::new(),
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
            pending_screenshot_ui: false,
            pending_context_menu: None,
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

        let policy = match cfg.windows.focus_mode {
            FocusMode::ClickToFocus => crate::window::FocusPolicy::ClickToFocus,
            FocusMode::FocusFollowsMouse => crate::window::FocusPolicy::FocusFollowsMouse,
        };
        self.wm.set_focus_policy(policy);
        self.wm.set_snap_gap(cfg.windows.snap_gap);
        self.config = cfg;
        self.relayout_non_floating_windows();
    }

    /// Monotonic compositor timestamp as [`Duration`] for animation sampling.
    pub fn now_duration(&self) -> Duration {
        self.clock.now().into()
    }

    /// Advertises `zwp_linux_dmabuf_v1` once the graphics backend reports supported GPU formats.
    pub fn enable_dmabuf<I>(&mut self, formats: I, import_hook: DmabufImportHook)
    where
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
        let global = self
            .dmabuf_state
            .create_global::<Self>(&self.display_handle, format_vec);
        self.dmabuf_global = Some(global);
        self.dmabuf_import_hook = Some(import_hook);
        info!(
            format_count = count,
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
            || !self.closing_windows.is_empty();

        let output_size = self.output_logical_size();
        self.scene.set_backdrop(to_wm_size(output_size));

        let fx_cfg = self.effects.config;
        let focused_id = self.wm.focused_id();
        let pointer_grabbed = self
            .seat
            .get_pointer()
            .map(|p| p.is_grabbed())
            .unwrap_or(false);
        let mut visible_ids: HashSet<WindowId> = HashSet::new();

        for (z_idx, window) in self.space.elements().enumerate() {
            let Some(id) = window_id(window) else {
                continue;
            };
            let Some(loc) = self.space.element_location(window) else {
                continue;
            };
            let win_size = window.geometry().size;
            let bounds = WmRect::new(to_wm_point(loc), to_wm_size(win_size));

            // Detect maximize / unmaximize / snap / fullscreen / resize transitions on
            // already-visible windows and drive a smooth Material 3 Expressive spatial morph.
            if win_size.w > 0
                && win_size.h > 0
                && !self.minimizing_windows.contains_key(&id)
                && !self.closing_windows.contains_key(&id)
            {
                if !pointer_grabbed
                    && let Some(&prev_bounds) = self.last_rendered_rects.get(&id)
                    && prev_bounds != bounds
                {
                    self.effects.animate_window_geometry(id, prev_bounds, bounds, now);
                    animating = true;
                }
                self.last_rendered_rects.insert(id, bounds);
            }

            let is_focused = focused_id == Some(id);
            let mode = self
                .wm
                .get(id)
                .map(|w| w.mode.clone())
                .unwrap_or(WindowMode::Floating);
            let fullscreen = mode.is_fullscreen();
            let maximized = mode.is_maximized();
            let visual = self.effects.sample_window(id, is_focused, now);
            let opacity = if is_focused || fullscreen {
                visual.opacity
            } else {
                visual.opacity * fx_cfg.inactive_opacity.clamp(0.4, 1.0)
            };

            let shadow = if fx_cfg.shadows_enabled && !fullscreen && !maximized {
                Some(ShadowStyle::for_window(
                    visual.focus_progress,
                    fx_cfg.corner_radius,
                ))
            } else {
                None
            };

            let border = if fx_cfg.borders_enabled && !fullscreen && !maximized {
                let mut style =
                    BorderStyle::for_window(visual.focus_progress, fx_cfg.corner_radius);
                if let Some([ar, ag, ab]) = parse_hex_rgb(&self.config.appearance.accent_color) {
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

            self.scene.upsert_window(
                id,
                bounds,
                z_idx as i32,
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
            let titlebar = if !fullscreen && self.ssd_windows.contains(&id) {
                Some(TitlebarStyle {
                    height: MACOS_TITLEBAR_HEIGHT,
                    corner_radius: if maximized { 0.0 } else { fx_cfg.corner_radius },
                    focused: is_focused,
                    light_theme: self.config.appearance.theme == ThemeMode::NeonLight,
                    hover_controls: self.hovered_titlebar_window == Some(id),
                })
            } else {
                None
            };
            self.scene.set_window_titlebar(id, titlebar);
            visible_ids.insert(id);
        }

        self.last_rendered_rects
            .retain(|id, _| visible_ids.contains(id));
        self.scene.retain_windows(|id| visible_ids.contains(&id));
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
                    self.space
                        .map_element(window, from_wm_point(clamped), false);
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

    /// Returns the active output work area excluding `zwlr_layer_shell_v1` exclusive zones
    /// (such as the top panel and dock).
    pub fn work_area(&self) -> WmRect {
        if let Some(output) = self.primary_output.as_ref() {
            let map = layer_map_for_output(output);
            let zone = map.non_exclusive_zone();
            if zone.size.w > 0 && zone.size.h > 0 {
                return to_wm_rect(zone);
            }
        }
        to_wm_rect(Rectangle::from_size(self.output_logical_size()))
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
            if let Some(layer) = map.layer_under(wlr_layer, pos)
                && let Some(geo) = map.layer_geometry(layer)
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
        None
    }

    /// Resolves the Wayland surface and its logical origin under the given compositor coordinates,
    /// respecting `zwlr_layer_shell_v1` Z-order (`Overlay`/`Top` above override-redirect popups &
    /// windows, `Bottom`/`Background` below).
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if let Some((surf, loc, _)) =
            self.layer_surface_under(pos, &[WlrLayer::Overlay, WlrLayer::Top])
        {
            return Some((surf, loc));
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
                return Some((surface, (loc + surface_loc).to_f64()));
            }
        }

        if let Some(win_hit) = self
            .space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surface_loc)| (surface, (location + surface_loc).to_f64()))
            })
        {
            return Some(win_hit);
        }

        self.layer_surface_under(pos, &[WlrLayer::Bottom, WlrLayer::Background])
            .map(|(surf, loc, _)| (surf, loc))
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
                    WindowMode::Snapped(region) => {
                        match region {
                            SnapRegion::LeftHalf => {
                                state.states.set(xdg_toplevel::State::TiledLeft);
                                state.states.set(xdg_toplevel::State::TiledTop);
                                state.states.set(xdg_toplevel::State::TiledBottom);
                            }
                            SnapRegion::RightHalf => {
                                state.states.set(xdg_toplevel::State::TiledRight);
                                state.states.set(xdg_toplevel::State::TiledTop);
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
                self.space.map_element(window, from_wm_point(loc), false);
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
            if record.is_visible_on(active_ws) {
                let loc = from_wm_point(record.current_geometry.loc);
                self.space.map_element(window, loc, false);
            } else {
                self.space.unmap_elem(&window);
            }
        }

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
        let target_surface = target.and_then(|w| w.wl_surface().map(|s| s.into_owned()));

        for window in self.managed_windows.values() {
            let is_target = target
                .and_then(|t| t.wl_surface().map(|s| s.id()))
                .zip(window.wl_surface().map(|s| s.id()))
                .map(|(a, b)| a == b)
                .unwrap_or(false);

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
            keyboard.set_focus(self, target_surface, serial);
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

    /// Requests graceful closure of a window with a smooth Hyprland/M3 popout close animation.
    pub fn close_window_by_id(&mut self, id: WindowId) -> bool {
        if self.find_window_by_id(id).is_none() {
            return false;
        }
        if self.closing_windows.contains_key(&id) {
            return true;
        }
        let now = self.now_duration();
        let dur = self.effects.animate_window_close(id, now);
        self.needs_redraw = true;
        if dur.is_zero() || self.backend_name == "headless" {
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
                self.space.map_element(window, from_wm_point(new_loc), false);
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
                let hit = if dist_sq(16.0) <= hit_r_sq {
                    TitlebarHit::Close
                } else if dist_sq(36.0) <= hit_r_sq {
                    TitlebarHit::Minimize
                } else if dist_sq(56.0) <= hit_r_sq {
                    TitlebarHit::Maximize
                } else {
                    TitlebarHit::Drag
                };
                return Some((id, window.clone(), hit));
            }
        }
        None
    }

    /// Toggles maximize state for any window ID (supports both Wayland and XWayland windows).
    pub fn toggle_maximize_window(&mut self, id: WindowId) {
        let work_area = self.effective_work_area_for(id);
        if let Some(update) = self.wm.toggle_maximized(id, work_area) {
            info!(?id, mode = ?update.mode, "Toggled maximize on window");
            if let Some(window) = self.find_window_by_id(id)
                && let Some(x11) = window.x11_surface()
            {
                let _ = x11.set_maximized(update.mode.is_maximized());
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = x11.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
            }
            self.apply_layout_update(&update);
            self.needs_redraw = true;
        }
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
        let work_area = self.work_area();
        if let Some(update) = self.wm.toggle_fullscreen(id, work_area) {
            info!(?id, mode = ?update.mode, "Toggled fullscreen on focused window");
            self.apply_layout_update(&update);
        }
    }

    pub fn snap_window_by_id(&mut self, id: WindowId, region: SnapRegion) {
        let work_area = self.effective_work_area_for(id);
        if let Some(update) = self.wm.toggle_snap(id, region, work_area) {
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

    pub fn snap_focused(&mut self, region: SnapRegion) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        self.snap_window_by_id(id, region);
    }

    pub fn unmaximize_or_minimize_focused(&mut self, serial: Serial) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let is_floating = self
            .wm
            .get(id)
            .map(|w| w.mode.is_floating())
            .unwrap_or(false);
        if is_floating {
            self.minimize_window(id, serial);
        } else {
            let work_area = self.work_area();
            if let Some(update) = self.wm.set_floating(id, work_area) {
                info!(?id, "Restored window to floating mode");
                self.apply_layout_update(&update);
            }
        }
    }

    fn dock_swoop_delta_for(&self, id: WindowId) -> (f64, f64) {
        let out_sz = self.output_logical_size();
        let dock_x = out_sz.w as f64 * 0.5;
        let dock_y = (out_sz.h - 32).max(100) as f64;
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
            (0.0, 260.0)
        }
    }

    pub fn minimize_window(&mut self, id: WindowId, serial: Serial) {
        let (dx, dy) = self.dock_swoop_delta_for(id);
        let work_area = self.work_area();
        if let Some(update) = self.wm.minimize(id, work_area) {
            info!(?id, "Minimized window");
            let now = self.now_duration();
            let dur = self.effects.animate_window_minimize(id, dx, dy, now);
            if dur.is_zero() || self.backend_name == "headless" {
                self.apply_layout_update(&update);
            } else {
                // Keep window visible in Space while it swoops smoothly down into the Dock.
                self.minimizing_windows.insert(id, now + dur);
                self.needs_redraw = true;
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
        let work_area = self.work_area();
        let Some(update) = self.wm.restore_window(id, work_area) else {
            return false;
        };
        self.minimizing_windows.remove(&id);
        self.last_rendered_rects.remove(&id);
        self.apply_layout_update(&update);
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
        if let Some(update) = self.wm.restore_last_minimized(work_area) {
            info!(id = ?update.id, mode = ?update.mode, "Restored minimized window");
            self.minimizing_windows.remove(&update.id);
            self.last_rendered_rects.remove(&update.id);
            self.apply_layout_update(&update);
            let (dx, dy) = self.dock_swoop_delta_for(update.id);
            let now = self.now_duration();
            self.effects.animate_window_restore(update.id, dx, dy, now);
            self.needs_redraw = true;
            if let Some(window) = self.find_window_by_id(update.id) {
                self.focus_window(Some(&window), serial);
            }
        }
    }

    pub fn switch_workspace_index(&mut self, index: usize, serial: Serial) -> Option<WorkspaceId> {
        let old_idx = self.wm.workspaces().active_index();
        let new_ws = self.wm.switch_workspace_index(index)?;
        let direction = if index >= old_idx { 1 } else { -1 };
        let now = self.now_duration();
        self.effects.animate_workspace_switch(direction, now);
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
        let new_ws = self.wm.switch_workspace_next()?;
        let now = self.now_duration();
        self.effects.animate_workspace_switch(1, now);
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
        let new_ws = self.wm.switch_workspace_prev()?;
        let now = self.now_duration();
        self.effects.animate_workspace_switch(-1, now);
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
        self.sync_active_workspace_space(serial);
        self.needs_redraw = true;
        info!(
            index,
            total = self.wm.workspaces().len(),
            "Removed desktop"
        );
        true
    }

    pub fn move_focused_to_next_workspace(
        &mut self,
        follow: bool,
        serial: Serial,
    ) -> Option<WorkspaceId> {
        let id = self.wm.focused_id()?;
        let new_ws = self.wm.move_window_to_next_workspace(id, follow)?;
        if follow {
            let now = self.now_duration();
            self.effects.animate_workspace_switch(1, now);
        }
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
        Some(new_ws)
    }

    pub fn move_focused_to_prev_workspace(
        &mut self,
        follow: bool,
        serial: Serial,
    ) -> Option<WorkspaceId> {
        let id = self.wm.focused_id()?;
        let new_ws = self.wm.move_window_to_prev_workspace(id, follow)?;
        if follow {
            let now = self.now_duration();
            self.effects.animate_workspace_switch(-1, now);
        }
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

    /// Re-lays out maximized/fullscreen/snapped windows when the output is resized.
    pub fn relayout_non_floating_windows(&mut self) {
        let work_area = self.work_area();
        let ids: Vec<(WindowId, WindowMode)> = self
            .wm
            .all_window_ids()
            .into_iter()
            .filter_map(|id| {
                let mode = self.wm.get(id)?.mode.clone();
                (!mode.is_floating() && !mode.is_minimized()).then_some((id, mode))
            })
            .collect();

        for (id, mode) in ids {
            if let Some(update) = self.wm.apply_mode(id, mode, work_area) {
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
        for id in dead_ids {
            if let Some(window) = self.managed_windows.remove(&id) {
                self.space.unmap_elem(&window);
            }
            self.ssd_windows.remove(&id);
            self.last_rendered_rects.remove(&id);
            self.minimizing_windows.remove(&id);
            self.closing_windows.remove(&id);
            if self.hovered_titlebar_window == Some(id) {
                self.hovered_titlebar_window = None;
            }
            if self.wm.focused_id() == Some(id) {
                lost_focus = true;
            }
            self.wm.remove_window(id);
            self.effects.remove_window(id);
            self.scene.remove_window(id);
            debug!(?id, "Cleaned up dead window");
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
