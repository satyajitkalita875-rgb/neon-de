//! XWayland lifecycle (`XWayland`, `X11Wm`, `XWaylandShellHandler`, `XwmHandler`).

use std::{os::unix::io::OwnedFd, process::Stdio};

use smithay::{
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        selection::SelectionTarget,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId},
    },
};
use tracing::{debug, info, warn};

use crate::{
    state::NeonState,
    window::{attach_window_id, from_wm_point, from_wm_size, to_wm_size},
};

/// Spawns `/usr/bin/Xwayland` attached to `NeonState`'s `calloop` and `DisplayHandle`.
pub fn spawn_xwayland(state: &mut NeonState) {
    let loop_handle = state.loop_handle.clone();
    match XWayland::spawn(
        &state.display_handle,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    ) {
        Ok((xwayland, client)) => {
            let wm_handle = loop_handle.clone();
            if let Err(err) =
                loop_handle.insert_source(xwayland, move |event, _, state| match event {
                    XWaylandEvent::Ready {
                        x11_socket,
                        display_number,
                    } => match X11Wm::start_wm(wm_handle.clone(), x11_socket, client.clone()) {
                        Ok(wm) => {
                            state.xwm = Some(wm);
                            state.xwayland_display = Some(display_number);
                            info!(
                                display = format!(":{display_number}"),
                                "XWayland ready and X11 window manager attached"
                            );
                        }
                        Err(err) => {
                            warn!(?err, "Failed to start X11Wm for XWayland");
                        }
                    },
                    XWaylandEvent::Error => {
                        warn!("XWayland server exited with an error");
                        state.xwm = None;
                        state.xwayland_display = None;
                    }
                })
            {
                warn!(
                    ?err,
                    "Failed to register XWayland event source into calloop"
                );
            }
        }
        Err(err) => {
            warn!(
                ?err,
                "XWayland could not be started (continuing Wayland-only)"
            );
        }
    }
}

impl XWaylandShellHandler for NeonState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm: XwmId, _wl_surface: WlSurface, surface: X11Surface) {
        debug!(
            x11_window = surface.window_id(),
            "Associated XWayland wl_surface"
        );
    }
}

impl XwmHandler for NeonState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("X11Wm initialized when handling XWM events")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(err) = surface.set_mapped(true) {
            warn!(?err, "Failed to mark X11 window mapped");
            return;
        }

        let work_area = self.work_area();
        let record = self.wm.register_window(work_area, None);
        let id = record.id;
        let loc = from_wm_point(record.current_geometry.loc);

        let mut geo = surface.geometry();
        geo.loc = loc;
        if geo.size.w <= 0 || geo.size.h <= 0 {
            geo.size = (640, 480).into();
        }
        let _ = surface.configure(geo);

        if let Some(rec) = self.wm.get_mut(id) {
            rec.title = Some(surface.title());
            rec.app_id = Some(surface.class());
            rec.mapped = true;
            rec.current_geometry.size = to_wm_size(geo.size);
            rec.floating_geometry.size = to_wm_size(geo.size);
        }

        let window = Window::new_x11_window(surface);
        attach_window_id(&window, id);
        self.managed_windows.insert(id, window.clone());

        if record.is_visible_on(self.wm.active_workspace_id()) {
            self.space.map_element(window.clone(), loc, true);
            let now = self.now_duration();
            self.effects.animate_window_open(id, now);
            if !self.is_locked() {
                self.focus_window(Some(&window), SERIAL_COUNTER.next_serial());
            }
        }
        info!(?id, ?loc, "Mapped XWayland X11 window");
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let loc = surface.geometry().loc;
        let window = Window::new_x11_window(surface);
        self.space.map_element(window, loc, true);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let target_xid = surface.window_id();
        let matching = self
            .managed_windows
            .iter()
            .find(|(_, w)| {
                w.x11_surface()
                    .map(|s| s.window_id() == target_xid)
                    .unwrap_or(false)
            })
            .map(|(&id, w)| (id, w.clone()));

        if let Some((id, window)) = matching {
            let was_focused = self.wm.focused_id() == Some(id);
            self.space.unmap_elem(&window);
            self.managed_windows.remove(&id);
            self.wm.remove_window(id);
            self.effects.remove_window(id);
            self.scene.remove_window(id);
            if !surface.is_override_redirect() {
                let _ = surface.set_mapped(false);
            }
            if was_focused && !self.is_locked() {
                self.focus_topmost_window(SERIAL_COUNTER.next_serial());
            }
            debug!(?id, "Unmapped XWayland X11 window");
        }
    }

    fn destroyed_window(&mut self, xwm: XwmId, surface: X11Surface) {
        self.unmapped_window(xwm, surface);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let mut geo = window.geometry();
        if let Some(x) = x {
            geo.loc.x = x;
        }
        if let Some(y) = y {
            geo.loc.y = y;
        }
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        let xid = surface.window_id();
        if let Some((&id, window)) = self.managed_windows.iter().find(|(_, w)| {
            w.x11_surface()
                .map(|s| s.window_id() == xid)
                .unwrap_or(false)
        }) {
            let window = window.clone();
            let work_area = self.work_area();
            self.wm
                .update_committed_size(id, to_wm_size(geometry.size), work_area);
            if self
                .wm
                .get(id)
                .map(|r| r.is_visible_on(self.wm.active_workspace_id()))
                .unwrap_or(false)
            {
                self.space.map_element(window, geometry.loc, false);
            }
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((&id, _)) = self.managed_windows.iter().find(|(_, w)| {
            w.x11_surface()
                .map(|s| s.window_id() == xid)
                .unwrap_or(false)
        }) {
            let work_area = self.work_area();
            if let Some(update) = self.wm.set_maximized(id, true, work_area) {
                let _ = surface.set_maximized(true);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
            }
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((&id, _)) = self.managed_windows.iter().find(|(_, w)| {
            w.x11_surface()
                .map(|s| s.window_id() == xid)
                .unwrap_or(false)
        }) {
            let work_area = self.work_area();
            if let Some(update) = self.wm.set_maximized(id, false, work_area) {
                let _ = surface.set_maximized(false);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
            }
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((&id, _)) = self.managed_windows.iter().find(|(_, w)| {
            w.x11_surface()
                .map(|s| s.window_id() == xid)
                .unwrap_or(false)
        }) {
            let work_area = self.work_area();
            if let Some(update) = self.wm.set_fullscreen(id, true, work_area) {
                let _ = surface.set_fullscreen(true);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
            }
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((&id, _)) = self.managed_windows.iter().find(|(_, w)| {
            w.x11_surface()
                .map(|s| s.window_id() == xid)
                .unwrap_or(false)
        }) {
            let work_area = self.work_area();
            if let Some(update) = self.wm.set_fullscreen(id, false, work_area) {
                let _ = surface.set_fullscreen(false);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
            }
        }
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _resize_edge: X11ResizeEdge,
    ) {
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        _selection: SelectionTarget,
        _mime_type: String,
        _fd: OwnedFd,
    ) {
    }
}
