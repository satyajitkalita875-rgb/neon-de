//! `xdg_wm_base`, `xdg_surface`, `xdg_toplevel`, and `xdg_popup` handlers wired to `WindowManager`.

use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
        find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{
        Seat,
        pointer::{Focus, GrabStartData as PointerGrabStartData},
    },
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
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
            XdgToplevelSurfaceData, decoration::XdgDecorationHandler,
        },
    },
};
use tracing::{debug, info, warn};

use crate::{
    grabs::{MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
    state::NeonState,
    window::{WindowId, attach_window_id, from_wm_point, window_id},
};

impl XdgShellHandler for NeonState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let parent_id = surface
            .parent()
            .and_then(|parent_surface| self.find_window_by_surface(&parent_surface))
            .map(|(pid, _)| pid);

        let window = Window::new_wayland_window(surface);
        let work_area = self.work_area();
        let record = self.wm.register_window(work_area, parent_id);
        let id = record.id;
        attach_window_id(&window, id);

        self.managed_windows.insert(id, window.clone());

        let loc = from_wm_point(record.current_geometry.loc);
        if record.is_visible_on(self.wm.active_workspace_id()) {
            self.space.map_element(window.clone(), loc, true);
            let serial = SERIAL_COUNTER.next_serial();
            self.focus_window(Some(&window), serial);
        }

        info!(
            ?id,
            workspace = ?record.workspace_id,
            ?loc,
            ?parent_id,
            "Mapped new xdg_toplevel window"
        );
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some((id, window)) = self.find_window_by_surface(surface.wl_surface()) {
            let was_focused = self.wm.focused_id() == Some(id);
            self.queue_close_ghost_if_visible(id, &window);
            self.space.unmap_elem(&window);
            self.managed_windows.remove(&id);
            self.ssd_windows.remove(&id);
            self.last_rendered_rects.remove(&id);
            self.pending_geometry_anims.remove(&id);
            self.window_dock_anchors.remove(&id);
            self.minimizing_windows.remove(&id);
            self.closing_windows.remove(&id);
            if self.hovered_titlebar_window == Some(id) {
                self.hovered_titlebar_window = None;
            }
            self.wm.remove_window(id);
            self.effects.remove_window(id);
            self.scene.remove_window(id);
            self.needs_redraw = true;
            info!(?id, "Destroyed xdg_toplevel window");
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
        if let Some((id, _)) = self.find_window_by_surface(surface.wl_surface()) {
            if let Some(record) = self.wm.get_mut(id) {
                debug!(?id, ?title, "Window title updated");
                record.title = title;
            }
            self.maybe_apply_rules(id);
        }
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let app_id = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().unwrap().app_id.clone())
        });
        if let Some((id, _)) = self.find_window_by_surface(surface.wl_surface()) {
            if let Some(record) = self.wm.get_mut(id) {
                debug!(?id, ?app_id, "Window app_id updated");
                record.app_id = app_id;
            }
            self.maybe_apply_rules(id);
        }
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        let parent_id = surface
            .parent()
            .and_then(|parent_surface| self.find_window_by_surface(&parent_surface))
            .map(|(pid, _)| pid);

        if let Some((id, window)) = self.find_window_by_surface(surface.wl_surface()) {
            let work_area = self.work_area();
            if let Some(new_loc) = self.wm.set_parent(id, parent_id, work_area)
                && self
                    .wm
                    .get(id)
                    .map(|w| w.is_visible_on(self.wm.active_workspace_id()))
                    .unwrap_or(false)
            {
                self.map_preserving_z_order(window.clone(), from_wm_point(new_loc));
            }
            if parent_id.is_some()
                && self
                    .wm
                    .get(id)
                    .map(|w| w.is_visible_on(self.wm.active_workspace_id()))
                    .unwrap_or(false)
            {
                self.space.raise_element(&window, false);
                self.sync_x11_stacking_order();
            }
            debug!(?id, ?parent_id, "Window parent updated");
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

    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap_or_else(|| self.seat.clone());
        let popup = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let Ok(mut grab) = self.popups.grab_popup(root.into(), popup, &seat, serial) else {
            return;
        };

        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
        self.needs_redraw = true;
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap_or_else(|| self.seat.clone());
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some((id, window)) = self.find_window_by_surface(surface.wl_surface()) else {
            return;
        };

        // Defer restoring maximized/snapped windows until the user actually drags (> 6 px)
        // so double-clicking a CSD titlebar never falsely unmaximizes on click 1.
        let is_non_floating = self
            .wm
            .get(id)
            .map(|r| !r.mode.is_floating())
            .unwrap_or(false);

        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };

        let grab = MoveSurfaceGrab {
            start_data,
            window,
            initial_window_location,
            pending_unmaximize: is_non_floating,
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
        let Some((id, window)) = self.find_window_by_surface(surface.wl_surface()) else {
            return;
        };
        // Interactive resize transitions snapped/maximized windows cleanly back to floating.
        let work_area = self.work_area();
        if let Some(update) = self.wm.set_floating(id, work_area) {
            self.apply_layout_update(&update);
        }

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
        if let Some((id, _)) = self.find_window_by_surface(surface.wl_surface()) {
            let work_area = self.effective_work_area_for(id);
            if let Some(update) = self.wm.set_maximized(id, true, work_area) {
                self.begin_geometry_transition(id, update.configure_size);
                self.apply_layout_update(&update);
                self.needs_redraw = true;
                return;
            }
        }
        surface.send_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some((id, _)) = self.find_window_by_surface(surface.wl_surface()) {
            let work_area = self.effective_work_area_for(id);
            if let Some(update) = self.wm.set_maximized(id, false, work_area) {
                self.begin_geometry_transition(id, update.configure_size);
                self.apply_layout_update(&update);
                self.needs_redraw = true;
                return;
            }
        }
        surface.send_configure();
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if let Some((id, _)) = self.find_window_by_surface(surface.wl_surface()) {
            let full = self.output_full_rect();
            if let Some(update) = self.wm.set_fullscreen(id, true, full) {
                self.begin_geometry_transition(id, update.configure_size);
                self.apply_layout_update(&update);
                self.needs_redraw = true;
                return;
            }
        }
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some((id, _)) = self.find_window_by_surface(surface.wl_surface()) {
            let work_area = self.effective_work_area_for(id);
            if let Some(update) = self.wm.set_fullscreen(id, false, work_area) {
                self.begin_geometry_transition(id, update.configure_size);
                self.apply_layout_update(&update);
                self.needs_redraw = true;
                return;
            }
        }
        surface.send_configure();
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some((id, _)) = self.find_window_by_surface(surface.wl_surface()) {
            self.minimize_window(id, SERIAL_COUNTER.next_serial());
        }
    }
}

impl XdgDecorationHandler for NeonState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Prefer Client-Side Decorations (matching GNOME/Mutter) so modern applications
        // (Chrome, Electron/Antigravity, GTK, Qt/OBS, Kitty, Firefox) render their own
        // native integrated header/navigation bar with close/minimize/maximize controls
        // rather than a separate server-side strip glued above the app.
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ClientSide);
        });
        if let Some((id, _)) = self.find_window_by_surface(toplevel.wl_surface()) {
            self.ssd_windows.remove(&id);
        }
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ClientSide);
        });
        if let Some((id, _)) = self.find_window_by_surface(toplevel.wl_surface()) {
            self.ssd_windows.remove(&id);
        }
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ClientSide);
        });
        if let Some((id, _)) = self.find_window_by_surface(toplevel.wl_surface()) {
            self.ssd_windows.remove(&id);
        }
        toplevel.send_configure();
    }
}

impl NeonState {
    fn maybe_apply_rules(&mut self, id: WindowId) {
        let work_area = self.work_area();
        if let Some((eval, update)) = self.wm.apply_rules_if_needed(id, work_area) {
            info!(
                ?id,
                rules = ?eval.matched_names,
                mode = ?update.mode,
                "Applied window rules"
            );
            self.apply_layout_update(&update);
            if !update.visible_on_active_workspace || eval.focus_on_map == Some(false) {
                self.focus_topmost_window(SERIAL_COUNTER.next_serial());
            }
        }
    }

    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some((_, window)) = self.find_window_by_surface(&root) else {
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

    pub fn window_id_for_window(&self, window: &Window) -> Option<WindowId> {
        window_id(window)
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
    if !focus
        .wl_surface_id()
        .is_some_and(|fid| fid.same_client_as(&surface.id()))
    {
        return None;
    }
    Some(start_data)
}
