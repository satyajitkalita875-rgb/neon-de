//! `xdg_wm_base`, `xdg_surface`, `xdg_toplevel`, and `xdg_popup` handlers.

use smithay::{
    desktop::{PopupKind, Window, find_popup_root_surface, get_popup_toplevel_coords},
    input::{
        Seat,
        pointer::{Focus, GrabStartData as PointerGrabStartData},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Resource,
            protocol::{wl_output::WlOutput, wl_seat::WlSeat},
        },
    },
    utils::{Rectangle, SERIAL_COUNTER, Serial},
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};
use tracing::{debug, info, warn};

use crate::{
    grabs::{MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
    state::NeonState,
    window::{attach_window_id, window_id},
};

impl XdgShellHandler for NeonState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        let output_size = self.output_logical_size();
        let location = self.windows.next_floating_placement(output_size);
        let id = self.windows.register(location);
        attach_window_id(&window, id);

        self.space.map_element(window.clone(), location, true);
        info!(?id, ?location, "Mapped new xdg_toplevel window");

        let serial = SERIAL_COUNTER.next_serial();
        self.focus_window(Some(&window), serial);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let surface_id = surface.wl_surface().id();
        let target = self
            .space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|tl| tl.wl_surface().id() == surface_id)
                    .unwrap_or(false)
            })
            .cloned();

        if let Some(window) = target {
            let id = window_id(&window);
            let was_focused = id.is_some() && self.windows.focused_id() == id;
            self.space.unmap_elem(&window);
            if let Some(win_id) = id {
                self.windows.remove(win_id);
                info!(?win_id, "Destroyed xdg_toplevel window");
            }
            if was_focused {
                self.focus_topmost_window(SERIAL_COUNTER.next_serial());
            }
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let title = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().unwrap().title.clone())
        });
        if let Some(window) = self.find_window_for_toplevel(&surface)
            && let Some(id) = window_id(&window)
            && let Some(record) = self.windows.get_mut(id)
        {
            debug!(?id, ?title, "Window title updated");
            record.title = title;
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let app_id = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().unwrap().app_id.clone())
        });
        if let Some(window) = self.find_window_for_toplevel(&surface)
            && let Some(id) = window_id(&window)
            && let Some(record) = self.windows.get_mut(id)
        {
            debug!(?id, ?app_id, "Window app_id updated");
            record.app_id = app_id;
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            warn!(?err, "Failed to track xdg_popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        // Popup pointer grabs will be expanded in Milestone 2.
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap_or_else(|| self.seat.clone());
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.find_window_for_toplevel(&surface) else {
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };

        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
        };
        if let Some(pointer) = seat.get_pointer() {
            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let seat = Seat::from_resource(&seat).unwrap_or_else(|| self.seat.clone());
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.find_window_for_toplevel(&surface) else {
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };
        let initial_window_size = window.geometry().size;

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();

        let grab = ResizeSurfaceGrab::start(
            start_data,
            window,
            ResizeEdge::from(edges),
            Rectangle::new(initial_window_location, initial_window_size),
        );
        if let Some(pointer) = seat.get_pointer() {
            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        let output_size = self.output_logical_size();
        if let Some(window) = self.find_window_for_toplevel(&surface)
            && let Some(id) = window_id(&window)
        {
            if let Some(loc) = self.space.element_location(&window)
                && let Some(record) = self.windows.get_mut(id)
                && !record.maximized
            {
                record.saved_floating_geometry = Some(Rectangle::new(loc, window.geometry().size));
                record.maximized = true;
            }
            self.space.map_element(window, (0, 0), true);
        }

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some(output_size);
        });
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        let mut restored_size = None;
        if let Some(window) = self.find_window_for_toplevel(&surface)
            && let Some(id) = window_id(&window)
            && let Some(record) = self.windows.get_mut(id)
        {
            record.maximized = false;
            if let Some(geo) = record.saved_floating_geometry.take() {
                restored_size = Some(geo.size);
                let loc = geo.loc;
                self.space.map_element(window, loc, true);
            }
        }

        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = restored_size;
        });
        surface.send_configure();
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        let output_size = self.output_logical_size();
        if let Some(window) = self.find_window_for_toplevel(&surface)
            && let Some(id) = window_id(&window)
        {
            if let Some(loc) = self.space.element_location(&window)
                && let Some(record) = self.windows.get_mut(id)
                && !record.fullscreen
            {
                record.saved_floating_geometry = Some(Rectangle::new(loc, window.geometry().size));
                record.fullscreen = true;
            }
            self.space.map_element(window, (0, 0), true);
        }

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(output_size);
        });
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let mut restored_size = None;
        if let Some(window) = self.find_window_for_toplevel(&surface)
            && let Some(id) = window_id(&window)
            && let Some(record) = self.windows.get_mut(id)
        {
            record.fullscreen = false;
            if let Some(geo) = record.saved_floating_geometry.take() {
                restored_size = Some(geo.size);
                let loc = geo.loc;
                self.space.map_element(window, loc, true);
            }
        }

        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = restored_size;
        });
        surface.send_configure();
    }
}

impl NeonState {
    fn find_window_for_toplevel(&self, surface: &ToplevelSurface) -> Option<Window> {
        let target_id = surface.wl_surface().id();
        self.space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|tl| tl.wl_surface().id() == target_id)
                    .unwrap_or(false)
            })
            .cloned()
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|tl| tl.wl_surface().id() == root.id())
                    .unwrap_or(false)
            })
            .cloned()
        else {
            return;
        };

        let Some(output) = self.primary_output.as_ref() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(&window) else {
            return;
        };

        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

fn check_grab(
    seat: &Seat<NeonState>,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<NeonState>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}
