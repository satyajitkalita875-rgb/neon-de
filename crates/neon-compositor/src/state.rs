//! Central compositor state (`NeonState`) integrating Smithay, `neon-window-manager`, and `neon-workspaces`.

use std::{collections::HashMap, ffi::OsString, process::Command, sync::Arc};

use neon_workspaces::{WorkspaceId, WorkspacePolicy};
use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{
        Seat, SeatState,
        keyboard::XkbConfig,
        pointer::{CursorImageStatus, MotionEvent},
    },
    output::Output,
    reexports::{
        calloop::{LoopHandle, LoopSignal},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Clock, IsAlive, Logical, Monotonic, Point, Rectangle, SERIAL_COUNTER, Serial, Size},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
    },
};
use tracing::{debug, info, warn};

use crate::window::{
    SnapRegion, WindowId, WindowLayoutUpdate, WindowManager, WindowMode, WmRect, from_wm_point,
    from_wm_size, to_wm_rect, window_id, window_matches_surface,
};

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
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,

    // Desktop & window management state
    pub space: Space<Window>,
    /// All live managed toplevel windows across all workspaces (including minimized windows).
    pub managed_windows: HashMap<WindowId, Window>,
    /// NEON DE window manager & dynamic workspace manager.
    pub wm: WindowManager,
    pub seat: Seat<Self>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: CursorImageStatus,
    pub primary_output: Option<Output>,
}

impl NeonState {
    /// Initializes Wayland globals, seat capabilities, window manager, and desktop space.
    pub fn new(
        display: &Display<Self>,
        loop_handle: LoopHandle<'static, Self>,
        loop_signal: LoopSignal,
        socket_name: OsString,
    ) -> Self {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);

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
            clock: Clock::new(),
            socket_name,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups: PopupManager::default(),
            space: Space::default(),
            managed_windows: HashMap::new(),
            wm: WindowManager::new(WorkspacePolicy::default()),
            seat,
            pointer_location: (640.0, 400.0).into(),
            cursor_status: CursorImageStatus::default_named(),
            primary_output: None,
        }
    }

    /// Registers an output into both the Wayland global registry and NEON DE's desktop space.
    pub fn register_output(&mut self, output: Output, location: Point<i32, Logical>) {
        output.create_global::<Self>(&self.display_handle);
        self.space.map_output(&output, location);
        if self.primary_output.is_none() {
            self.primary_output = Some(output);
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

    /// Returns the active output work area as a `neon-window-manager` rectangle.
    pub fn work_area(&self) -> WmRect {
        to_wm_rect(Rectangle::from_size(self.output_logical_size()))
    }

    /// Resolves the Wayland surface and its logical origin under the given compositor coordinates.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surface_loc)| (surface, (location + surface_loc).to_f64()))
            })
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
        let target_id = target.and_then(window_id);
        let target_surface = target.and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));

        for window in self.managed_windows.values() {
            let is_target = target
                .and_then(|t| t.toplevel().map(|tl| tl.wl_surface().id()))
                .zip(window.toplevel().map(|tl| tl.wl_surface().id()))
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
            }
        }

        if raise && let Some(window) = target {
            self.space.raise_element(window, true);
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

    /// Requests graceful closure of the currently focused toplevel window.
    pub fn close_focused_window(&mut self) {
        if let Some(focused_id) = self.wm.focused_id()
            && let Some(window) = self.find_window_by_id(focused_id)
            && let Some(toplevel) = window.toplevel()
        {
            info!(?focused_id, "Sending xdg_toplevel.close to focused window");
            toplevel.send_close();
        }
    }

    // --- Milestone 2 Window Management & Dynamic Workspace Actions ---

    pub fn toggle_maximize_focused(&mut self) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let work_area = self.work_area();
        if let Some(update) = self.wm.toggle_maximized(id, work_area) {
            info!(?id, mode = ?update.mode, "Toggled maximize on focused window");
            self.apply_layout_update(&update);
        }
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

    pub fn snap_focused(&mut self, region: SnapRegion) {
        let Some(id) = self.wm.focused_id() else {
            return;
        };
        let work_area = self.work_area();
        if let Some(update) = self.wm.toggle_snap(id, region, work_area) {
            info!(?id, mode = ?update.mode, "Updated window snap state");
            self.apply_layout_update(&update);
        }
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

    pub fn minimize_window(&mut self, id: WindowId, serial: Serial) {
        let work_area = self.work_area();
        if let Some(update) = self.wm.minimize(id, work_area) {
            info!(?id, "Minimized window");
            self.apply_layout_update(&update);
            self.focus_topmost_window(serial);
        }
    }

    pub fn minimize_focused(&mut self, serial: Serial) {
        if let Some(id) = self.wm.focused_id() {
            self.minimize_window(id, serial);
        }
    }

    pub fn restore_last_minimized(&mut self, serial: Serial) {
        let work_area = self.work_area();
        if let Some(update) = self.wm.restore_last_minimized(work_area) {
            info!(id = ?update.id, mode = ?update.mode, "Restored minimized window");
            self.apply_layout_update(&update);
            if let Some(window) = self.find_window_by_id(update.id) {
                self.focus_window(Some(&window), serial);
            }
        }
    }

    pub fn switch_workspace_index(&mut self, index: usize, serial: Serial) -> Option<WorkspaceId> {
        let new_ws = self.wm.switch_workspace_index(index)?;
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
        info!(
            workspace = ?new_ws,
            index = self.wm.workspaces().active_index(),
            total = self.wm.workspaces().len(),
            "Switched to previous workspace"
        );
        self.sync_active_workspace_space(serial);
        Some(new_ws)
    }

    pub fn move_focused_to_next_workspace(
        &mut self,
        follow: bool,
        serial: Serial,
    ) -> Option<WorkspaceId> {
        let id = self.wm.focused_id()?;
        let new_ws = self.wm.move_window_to_next_workspace(id, follow)?;
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

    /// Spawns a Wayland client process wired strictly to NEON DE's Wayland socket.
    pub fn spawn_client(&self, program: &str, args: &[String]) {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env_remove("DISPLAY")
            .env("XDG_SESSION_TYPE", "wayland")
            .env("XDG_CURRENT_DESKTOP", "NEON-DE");

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

    /// Spawns the first available Wayland-native terminal inside NEON DE.
    pub fn spawn_terminal(&self) {
        const CANDIDATES: &[&str] = &["weston-terminal", "alacritty", "kitty", "foot"];
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

    /// Removes dead windows across all workspaces and refreshes Space and PopupManager state.
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
            if self.wm.focused_id() == Some(id) {
                lost_focus = true;
            }
            self.wm.remove_window(id);
            debug!(?id, "Cleaned up dead window");
        }

        self.space.refresh();
        self.popups.cleanup();

        if lost_focus {
            self.focus_topmost_window(SERIAL_COUNTER.next_serial());
        }
    }
}
