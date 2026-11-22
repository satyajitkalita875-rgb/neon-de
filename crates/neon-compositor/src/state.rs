//! Central compositor state (`NeonState`) and per-client Wayland state.

use std::{ffi::OsString, process::Command, sync::Arc};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState, keyboard::XkbConfig, pointer::CursorImageStatus},
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
    utils::{Clock, IsAlive, Logical, Monotonic, Point, SERIAL_COUNTER, Serial, Size},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
    },
};
use tracing::{debug, info, warn};

use crate::window::{WindowId, WindowRegistry, window_id};

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
    pub windows: WindowRegistry,
    pub seat: Seat<Self>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: CursorImageStatus,
    pub primary_output: Option<Output>,
}

impl NeonState {
    /// Initializes all Milestone 1 Wayland globals, seat capabilities, and desktop space.
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
            windows: WindowRegistry::new(),
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

    /// Finds the managed [`Window`] whose surface tree sits at `pos`.
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(Window, Point<i32, Logical>)> {
        self.space
            .element_under(pos)
            .map(|(window, loc)| (window.clone(), loc))
    }

    /// Finds a mapped [`Window`] by its stable NEON DE [`WindowId`].
    pub fn find_window_by_id(&self, id: WindowId) -> Option<Window> {
        self.space
            .elements()
            .find(|window| window_id(window) == Some(id))
            .cloned()
    }

    /// Focuses and raises the given window, updating `xdg_toplevel::State::Activated` across windows.
    pub fn focus_window(&mut self, target: Option<&Window>, serial: Serial) {
        let target_id = target.and_then(window_id);
        let target_surface = target.and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));

        for window in self.space.elements() {
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

        if let Some(window) = target {
            self.space.raise_element(window, true);
        }

        self.windows.mark_focused(target_id);

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, target_surface, serial);
        }
    }

    /// Restores keyboard focus to the most recently focused remaining window (or clears focus).
    pub fn focus_topmost_window(&mut self, serial: Serial) {
        let next = self
            .windows
            .most_recent_id()
            .and_then(|id| self.find_window_by_id(id))
            .or_else(|| self.space.elements().next_back().cloned());
        self.focus_window(next.as_ref(), serial);
    }

    /// Cycles focus (`Alt + Tab`) to the next tracked window.
    pub fn cycle_focus(&mut self, serial: Serial) {
        if let Some(next_id) = self.windows.next_cycle_target()
            && let Some(window) = self.find_window_by_id(next_id)
        {
            debug!(?next_id, "Cycling window focus");
            self.focus_window(Some(&window), serial);
        }
    }

    /// Requests graceful closure of the currently focused toplevel window.
    pub fn close_focused_window(&mut self) {
        if let Some(focused_id) = self.windows.focused_id()
            && let Some(window) = self.find_window_by_id(focused_id)
            && let Some(toplevel) = window.toplevel()
        {
            info!(?focused_id, "Sending xdg_toplevel.close to focused window");
            toplevel.send_close();
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

    /// Removes dead windows and refreshes Space and PopupManager state.
    pub fn cleanup_dead_windows(&mut self) {
        let dead_ids: Vec<(Window, Option<WindowId>)> = self
            .space
            .elements()
            .filter(|w| !w.alive())
            .map(|w| (w.clone(), window_id(w)))
            .collect();

        let mut lost_focus = false;
        for (window, id) in dead_ids {
            self.space.unmap_elem(&window);
            if let Some(win_id) = id {
                if self.windows.focused_id() == Some(win_id) {
                    lost_focus = true;
                }
                self.windows.remove(win_id);
                debug!(?win_id, "Cleaned up dead window");
            }
        }

        self.space.refresh();
        self.popups.cleanup();

        if lost_focus {
            self.focus_topmost_window(SERIAL_COUNTER.next_serial());
        }
    }
}
