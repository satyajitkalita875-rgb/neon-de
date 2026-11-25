//! XWayland lifecycle (`XWayland`, `X11Wm`, `XWaylandShellHandler`, `XwmHandler`).

use std::{os::unix::io::OwnedFd, process::Stdio};

use smithay::{
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{
        selection::SelectionTarget,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, WmWindowProperty, WmWindowType, XwmId},
    },
};
use tracing::{debug, info, warn};

use crate::{
    state::NeonState,
    wayland::seat::FocusTarget,
    window::{
        WindowId, WindowMode, attach_window_id, from_wm_point, from_wm_size, to_wm_point,
        to_wm_rect, to_wm_size,
    },
};

/// Returns `true` when a non-fullscreen X11 window wants NEON DE's server-side macOS titlebar.
///
/// Note: in Smithay 0.7, [`X11Surface::is_decorated`] returns `true` when `_MOTIF_WM_HINTS`
/// specifies `decorations = 0` (meaning the client is frameless or draws its own CSD, such as
/// DaVinci Resolve's splash, login, and main windows). Server-side decorations must therefore
/// only be attached when `!surface.is_decorated()` and the window is a regular managed type.
pub fn x11_wants_ssd_non_fullscreen(surface: &X11Surface) -> bool {
    if surface.is_decorated() || surface.is_popup() {
        return false;
    }
    !matches!(
        surface.window_type(),
        Some(
            WmWindowType::Splash
                | WmWindowType::Notification
                | WmWindowType::Tooltip
                | WmWindowType::PopupMenu
                | WmWindowType::DropdownMenu
                | WmWindowType::Menu
                | WmWindowType::Toolbar
                | WmWindowType::Utility
        )
    )
}

/// Returns `true` when an X11 window should receive NEON DE's server-side titlebar right now.
pub fn x11_wants_ssd(surface: &X11Surface) -> bool {
    !surface.is_fullscreen() && x11_wants_ssd_non_fullscreen(surface)
}

fn is_special_non_focused_popup(surface: &X11Surface) -> bool {
    matches!(
        surface.window_type(),
        Some(WmWindowType::Tooltip | WmWindowType::Notification)
    )
}

fn is_non_parent_window_type(win_type: Option<WmWindowType>) -> bool {
    matches!(
        win_type,
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
}

fn is_client_positioned_type(surface: &X11Surface) -> bool {
    surface.is_popup()
        || matches!(
            surface.window_type(),
            Some(
                WmWindowType::DropdownMenu
                    | WmWindowType::PopupMenu
                    | WmWindowType::Tooltip
                    | WmWindowType::Menu
                    | WmWindowType::Notification
                    | WmWindowType::Toolbar
            )
        )
}

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
                            let disp_str = format!(":{display_number}");
                            // Do NOT call `wm.set_randr_primary_output` here: in Smithay 0.7,
                            // `randr_set_output_primary` pushes its request sequence into
                            // `sequences_to_ignore`, causing X11Wm to silently ignore subsequent
                            // `CreateNotify` / `MapRequest` events from X11 clients such as
                            // DaVinci Resolve.
                            state.xwm = Some(wm);
                            state.xwayland_display = Some(display_number);
                            info!(
                                display = %disp_str,
                                "XWayland ready and X11 window manager attached"
                            );
                            // Export DISPLAY to systemd/D-Bus activation environment for X11 apps
                            // such as DaVinci Resolve launched from shell or portals.
                            std::thread::spawn(move || {
                                let _ = std::process::Command::new(
                                    "dbus-update-activation-environment",
                                )
                                .args(["--systemd", "DISPLAY"])
                                .env("DISPLAY", &disp_str)
                                .status();
                            });
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
        let xid = surface.window_id();
        debug!(x11_window = xid, "Associated XWayland wl_surface");
        self.sync_x11_stacking_order();
        // Re-deliver seat keyboard focus and refresh pointer hover once the X11 window's Wayland
        // surface attaches so XWayland immediately receives `wl_keyboard.enter` + `wl_pointer.enter`
        // alongside `XSetInputFocus` and `WM_TAKE_FOCUS` (critical for Qt5/DaVinci Resolve modal
        // dialogs such as "Create New Project").
        let serial = SERIAL_COUNTER.next_serial();
        if let Some((id, window)) = self.find_managed_x11(xid)
            && self.wm.focused_id() == Some(id)
            && let Some(keyboard) = self.seat.get_keyboard()
        {
            keyboard.set_focus(self, None, serial);
            if let Some(x11) = window.x11_surface() {
                let _ = x11.set_activated(true);
                keyboard.set_focus(self, Some(FocusTarget::X11(x11.clone())), serial);
            }
            self.needs_redraw = true;
        }
        let pos = self.pointer_location;
        let under = self.surface_under(pos);
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(
                self,
                under,
                &smithay::input::pointer::MotionEvent {
                    location: pos,
                    serial,
                    time: self.clock.now().as_millis(),
                },
            );
            pointer.frame(self);
        }
    }
}

impl NeonState {
    fn find_managed_x11(&self, xid: u32) -> Option<(WindowId, Window)> {
        self.managed_windows
            .iter()
            .find(|(_, w)| {
                w.x11_surface()
                    .map(|s| s.window_id() == xid)
                    .unwrap_or(false)
            })
            .map(|(&id, w)| (id, w.clone()))
    }

    fn raise_window_with_children(&mut self, id: WindowId, window: &Window) {
        self.space.raise_element(window, false);
        for child_id in self.wm.visible_transient_children(id) {
            if let Some(child_win) = self.find_window_by_id(child_id) {
                self.space.raise_element(&child_win, false);
            }
        }
        self.sync_x11_stacking_order();
    }

    fn stack_window_above_sibling(&mut self, id: WindowId, window: &Window, sibling_xid: u32) {
        let Some((_, sibling_win)) = self.find_managed_x11(sibling_xid) else {
            self.raise_window_with_children(id, window);
            return;
        };
        let elements: Vec<Window> = self.space.elements().cloned().collect();
        let Some(sib_idx) = elements.iter().position(|w| w == &sibling_win) else {
            self.raise_window_with_children(id, window);
            return;
        };
        let above_sibling: Vec<Window> = elements[sib_idx + 1..]
            .iter()
            .filter(|w| *w != window)
            .cloned()
            .collect();
        self.space.raise_element(window, false);
        for w in &above_sibling {
            self.space.raise_element(w, false);
        }
        for child_id in self.wm.visible_transient_children(id) {
            if let Some(child_win) = self.find_window_by_id(child_id) {
                self.space.raise_element(&child_win, false);
            }
        }
        self.sync_x11_stacking_order();
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
        let xid = surface.window_id();
        // If this X11 window is already managed (e.g. repeated MapRequest), raise & focus it
        // cleanly instead of creating a duplicate WindowId.
        if let Some((existing_id, existing_win)) = self.find_managed_x11(xid) {
            let _ = surface.set_mapped(true);
            if self
                .wm
                .get(existing_id)
                .map(|r| r.is_visible_on(self.wm.active_workspace_id()))
                .unwrap_or(false)
            {
                if !self.is_locked() && !is_special_non_focused_popup(&surface) {
                    self.focus_window(Some(&existing_win), SERIAL_COUNTER.next_serial());
                } else {
                    self.raise_window_with_children(existing_id, &existing_win);
                }
                self.needs_redraw = true;
            }
            return;
        }

        if let Err(err) = surface.set_mapped(true) {
            warn!(?err, "Failed to mark X11 window mapped");
            return;
        }

        let work_area = self.work_area();
        let output_full = self.output_full_rect();
        let output_size = self.output_logical_size();
        let win_type = surface.window_type();
        let raw_geo = surface.geometry();
        let app_class = surface.class();

        // Resolve transient parent window (WM_TRANSIENT_FOR). Qt5/DaVinci Resolve dialogs set
        // WM_TRANSIENT_FOR to a hidden 1x1 group-leader window instead of the visible parent;
        // associate such dialogs with the topmost visible non-splash window of the same
        // application class so they center cleanly, stay stacked above the parent, and are
        // protected against focus_follows_mouse. Never treat a Splash screen as a dialog parent!
        let mut parent_id = surface
            .is_transient_for()
            .and_then(|parent_xid| self.find_x11_window_id(parent_xid))
            .filter(|pid| {
                self.find_window_by_id(*pid)
                    .and_then(|w| w.x11_surface().map(|s| !is_non_parent_window_type(s.window_type())))
                    .unwrap_or(true)
            });
        if parent_id.is_none()
            && matches!(win_type, Some(WmWindowType::Dialog))
            && !app_class.is_empty()
        {
            let is_eligible_parent = |cand_id: WindowId| -> bool {
                let Some(rec) = self.wm.get(cand_id) else {
                    return false;
                };
                if !rec.is_visible_on(self.wm.active_workspace_id()) {
                    return false;
                }
                if !rec
                    .app_id
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(&app_class))
                {
                    return false;
                }
                self.find_window_by_id(cand_id)
                    .and_then(|w| {
                        w.x11_surface()
                            .map(|s| !is_non_parent_window_type(s.window_type()))
                    })
                    .unwrap_or(true)
            };

            if let Some(focused_id) = self.wm.focused_id()
                && is_eligible_parent(focused_id)
            {
                parent_id = Some(focused_id);
            } else {
                parent_id = self
                    .wm
                    .active_visible_windows_mru_order()
                    .into_iter()
                    .rev()
                    .find(|&cand_id| is_eligible_parent(cand_id));
            }
        }

        let record = self.wm.register_window(work_area, parent_id);
        let id = record.id;

        let is_splash = matches!(win_type, Some(WmWindowType::Splash));
        let is_dialog = matches!(win_type, Some(WmWindowType::Dialog)) || parent_id.is_some();
        let is_client_pos = is_client_positioned_type(&surface);

        // Honor initial _NET_WM_STATE_FULLSCREEN as well as borderless full-output Qt/X11 windows
        // (such as DaVinci Resolve's main workspace window).
        let wants_fullscreen = surface.is_fullscreen()
            || (surface.is_decorated()
                && !is_splash
                && !is_dialog
                && !is_client_pos
                && raw_geo.size.w >= output_size.w
                && raw_geo.size.h >= output_size.h);
        let wants_maximized = !wants_fullscreen && surface.is_maximized();
        let wants_ssd = !wants_fullscreen && x11_wants_ssd(&surface);

        if wants_ssd {
            self.ssd_windows.insert(id);
        } else {
            self.ssd_windows.remove(&id);
        }

        let geo: Rectangle<i32, Logical> = if wants_fullscreen {
            let full_geo =
                Rectangle::new(from_wm_point(output_full.loc), from_wm_size(output_full.size));
            let _ = self.wm.set_fullscreen(id, true, output_full);
            let _ = surface.set_fullscreen(true);
            full_geo
        } else if wants_maximized {
            let eff_wa = self.effective_work_area_for(id);
            let max_geo = Rectangle::new(from_wm_point(eff_wa.loc), from_wm_size(eff_wa.size));
            let _ = self.wm.set_maximized(id, true, eff_wa);
            let _ = surface.set_maximized(true);
            max_geo
        } else {
            let mut size = raw_geo.size;
            if size.w <= 0 || size.h <= 0 {
                size = (640, 480).into();
            }
            let wm_size = to_wm_size(size);
            let eff_wa = self.effective_work_area_for(id);

            let mut loc: Point<i32, Logical> = if is_splash {
                from_wm_point(output_full.clamp_origin(output_full.center_size(wm_size), wm_size))
            } else if is_client_pos {
                raw_geo.loc
            } else if is_dialog {
                let anchor_rect = parent_id
                    .and_then(|pid| self.wm.get(pid).map(|p| p.current_geometry))
                    .unwrap_or(eff_wa);
                let centered = eff_wa.clamp_origin(anchor_rect.center_size(wm_size), wm_size);
                from_wm_point(centered)
            } else {
                let placed = self
                    .wm
                    .place_floating_window(id, eff_wa, wm_size)
                    .unwrap_or_else(|| eff_wa.clamp_origin(eff_wa.center_size(wm_size), wm_size));
                from_wm_point(placed)
            };

            if wants_ssd {
                let min_y = work_area.loc.y + neon_scene::MACOS_TITLEBAR_HEIGHT;
                if loc.y < min_y {
                    loc.y = min_y;
                }
            }
            Rectangle::new(loc, size)
        };

        let _ = surface.configure(geo);

        if let Some(rec) = self.wm.get_mut(id) {
            rec.title = Some(surface.title());
            rec.app_id = Some(app_class);
            rec.mapped = true;
            rec.current_geometry = to_wm_rect(geo);
            if rec.mode.is_floating() {
                rec.floating_geometry = to_wm_rect(geo);
            } else {
                // Keep a sensible fallback floating geometry so unfullscreening/unmaximizing
                // restores cleanly instead of collapsing to 0x0.
                let fallback_size = neon_window_manager::geometry::Size::new(
                    (output_size.w * 3 / 4).max(800),
                    (output_size.h * 3 / 4).max(600),
                );
                let fallback_loc =
                    work_area.clamp_origin(work_area.center_size(fallback_size), fallback_size);
                rec.floating_geometry =
                    neon_window_manager::geometry::Rect::new(fallback_loc, fallback_size);
            }
        }

        let skip_open_anim = is_splash || is_client_pos;
        let skip_focus = is_special_non_focused_popup(&surface);
        let window = Window::new_x11_window(surface);
        attach_window_id(&window, id);
        self.managed_windows.insert(id, window.clone());

        if self
            .wm
            .get(id)
            .map(|r| r.is_visible_on(self.wm.active_workspace_id()))
            .unwrap_or(false)
        {
            self.space.map_element(window.clone(), geo.loc, true);
            if !skip_open_anim {
                let now = self.now_duration();
                self.effects.animate_window_open(id, now);
            }
            if !self.is_locked() && !skip_focus {
                self.focus_window(Some(&window), SERIAL_COUNTER.next_serial());
            } else {
                self.sync_x11_stacking_order();
            }
            self.needs_redraw = true;
        }
        info!(
            ?id,
            ?geo,
            ?win_type,
            ?parent_id,
            wants_ssd,
            wants_fullscreen,
            wants_maximized,
            "Mapped XWayland X11 window"
        );
    }

    fn map_window_notify(&mut self, _xwm: XwmId, _surface: X11Surface) {
        self.sync_x11_stacking_order();
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let loc = surface.geometry().loc;
        let xid = surface.window_id();
        let window = Window::new_x11_window(surface);
        self.override_redirect_windows.retain(|w| {
            w.x11_surface()
                .map(|s| s.window_id() != xid)
                .unwrap_or(true)
        });
        self.override_redirect_windows.push(window.clone());
        self.space.map_element(window, loc, true);
        self.needs_redraw = true;
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let target_xid = surface.window_id();
        self.override_redirect_windows.retain(|w| {
            let matches = w
                .x11_surface()
                .map(|s| s.window_id() == target_xid)
                .unwrap_or(false);
            if matches {
                self.space.unmap_elem(w);
                self.needs_redraw = true;
            }
            !matches
        });

        if let Some((id, window)) = self.find_managed_x11(target_xid) {
            let was_focused = self.wm.focused_id() == Some(id);
            let parent_id = self.wm.get(id).and_then(|r| r.parent);
            self.space.unmap_elem(&window);
            self.managed_windows.remove(&id);
            self.ssd_windows.remove(&id);
            self.last_rendered_rects.remove(&id);
            self.minimizing_windows.remove(&id);
            self.closing_windows.remove(&id);
            if self.hovered_titlebar_window == Some(id) {
                self.hovered_titlebar_window = None;
            }
            self.wm.remove_window(id);
            self.effects.remove_window(id);
            self.scene.remove_window(id);
            if !surface.is_override_redirect() {
                let _ = surface.set_mapped(false);
            }
            if was_focused && !self.is_locked() {
                // Return focus to the transient parent window if still visible, otherwise MRU.
                let parent_win = parent_id.and_then(|pid| {
                    self.wm
                        .get(pid)
                        .filter(|r| r.is_visible_on(self.wm.active_workspace_id()))
                        .and_then(|_| self.find_window_by_id(pid))
                });
                if let Some(pwin) = parent_win {
                    self.focus_window(Some(&pwin), SERIAL_COUNTER.next_serial());
                } else {
                    self.focus_topmost_window(SERIAL_COUNTER.next_serial());
                }
            }
            self.needs_redraw = true;
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
        reorder: Option<Reorder>,
    ) {
        let xid = window.window_id();
        let managed = self.find_managed_x11(xid);

        let geo = if let Some((id, _)) = managed.as_ref() {
            let mode = self
                .wm
                .get(*id)
                .map(|r| r.mode.clone())
                .unwrap_or(WindowMode::Floating);
            match mode {
                WindowMode::Fullscreen => {
                    let full = self.output_full_rect();
                    Rectangle::new(from_wm_point(full.loc), from_wm_size(full.size))
                }
                WindowMode::Maximized => {
                    let eff_wa = self.effective_work_area_for(*id);
                    Rectangle::new(from_wm_point(eff_wa.loc), from_wm_size(eff_wa.size))
                }
                _ => {
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
                    if self.ssd_windows.contains(id) {
                        let min_y = self.work_area().loc.y + neon_scene::MACOS_TITLEBAR_HEIGHT;
                        if geo.loc.y < min_y {
                            geo.loc.y = min_y;
                        }
                    }
                    if let Some(rec) = self.wm.get_mut(*id) {
                        rec.current_geometry = to_wm_rect(geo);
                        if rec.mode.is_floating() {
                            rec.floating_geometry = to_wm_rect(geo);
                        }
                    }
                    geo
                }
            }
        } else {
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
            geo
        };

        let _ = window.configure(geo);

        if let (Some((id, managed_win)), Some(reorder)) = (managed, reorder)
            && self
                .wm
                .get(id)
                .map(|r| r.is_visible_on(self.wm.active_workspace_id()))
                .unwrap_or(false)
        {
            match reorder {
                Reorder::Top => {
                    self.raise_window_with_children(id, &managed_win);
                    self.needs_redraw = true;
                }
                Reorder::Above(sibling_xid) => {
                    self.stack_window_above_sibling(id, &managed_win, sibling_xid);
                    self.needs_redraw = true;
                }
                Reorder::Bottom | Reorder::Below(_) => {}
            }
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        let xid = surface.window_id();
        if surface.is_override_redirect() {
            if let Some(or_win) = self
                .override_redirect_windows
                .iter()
                .find(|w| {
                    w.x11_surface()
                        .map(|s| s.window_id() == xid)
                        .unwrap_or(false)
                })
                .cloned()
            {
                self.map_preserving_z_order(or_win, geometry.loc);
                self.needs_redraw = true;
            }
            return;
        }

        if let Some((id, window)) = self.find_managed_x11(xid) {
            let work_area = self.work_area();
            let is_floating = self
                .wm
                .get(id)
                .map(|r| r.mode.is_floating())
                .unwrap_or(true);
            if is_floating {
                self.wm
                    .update_committed_size(id, to_wm_size(geometry.size), work_area);
                self.wm.set_floating_location(id, to_wm_point(geometry.loc));
            }
            if self
                .wm
                .get(id)
                .map(|r| r.is_visible_on(self.wm.active_workspace_id()))
                .unwrap_or(false)
            {
                // Critically preserve existing Z-order so a main window reconfiguring (e.g.
                // DaVinci Resolve) NEVER jumps in front of its open child/modal dialog!
                self.map_preserving_z_order(window, geometry.loc);
                self.needs_redraw = true;
            }
        }
    }

    fn property_notify(&mut self, _xwm: XwmId, surface: X11Surface, property: WmWindowProperty) {
        let xid = surface.window_id();
        let Some((id, window)) = self.find_managed_x11(xid) else {
            return;
        };

        match property {
            WmWindowProperty::Title => {
                if let Some(rec) = self.wm.get_mut(id) {
                    rec.title = Some(surface.title());
                }
            }
            WmWindowProperty::Class => {
                if let Some(rec) = self.wm.get_mut(id) {
                    rec.app_id = Some(surface.class());
                }
            }
            WmWindowProperty::MotifHints | WmWindowProperty::WindowType => {
                let is_fs = self
                    .wm
                    .get(id)
                    .map(|r| r.mode.is_fullscreen())
                    .unwrap_or(false);
                if !is_fs && x11_wants_ssd(&surface) {
                    self.enable_ssd_for_window(id);
                } else {
                    self.ssd_windows.remove(&id);
                    self.needs_redraw = true;
                }
            }
            WmWindowProperty::TransientFor => {
                let parent_id = surface
                    .is_transient_for()
                    .and_then(|parent_xid| self.find_x11_window_id(parent_xid));
                let eff_wa = self.effective_work_area_for(id);
                if let Some(new_loc) = self.wm.set_parent(id, parent_id, eff_wa)
                    && self
                        .wm
                        .get(id)
                        .map(|r| r.is_visible_on(self.wm.active_workspace_id()))
                        .unwrap_or(false)
                {
                    let mut geo = surface.geometry();
                    geo.loc = from_wm_point(new_loc);
                    let _ = surface.configure(geo);
                    self.map_preserving_z_order(window.clone(), geo.loc);
                }
                if parent_id.is_some()
                    && self
                        .wm
                        .get(id)
                        .map(|r| r.is_visible_on(self.wm.active_workspace_id()))
                        .unwrap_or(false)
                {
                    self.raise_window_with_children(id, &window);
                    self.needs_redraw = true;
                }
            }
            _ => {}
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((id, _)) = self.find_managed_x11(xid) {
            let eff_wa = self.effective_work_area_for(id);
            if let Some(update) = self.wm.set_maximized(id, true, eff_wa) {
                let _ = surface.set_maximized(true);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
                self.needs_redraw = true;
            }
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((id, _)) = self.find_managed_x11(xid) {
            let eff_wa = self.effective_work_area_for(id);
            if let Some(update) = self.wm.set_maximized(id, false, eff_wa) {
                let _ = surface.set_maximized(false);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
                self.needs_redraw = true;
            }
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((id, _)) = self.find_managed_x11(xid) {
            self.ssd_windows.remove(&id);
            let full = self.output_full_rect();
            if let Some(update) = self.wm.set_fullscreen(id, true, full) {
                let _ = surface.set_fullscreen(true);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
                self.needs_redraw = true;
            }
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((id, _)) = self.find_managed_x11(xid) {
            if x11_wants_ssd_non_fullscreen(&surface) {
                self.ssd_windows.insert(id);
            } else {
                self.ssd_windows.remove(&id);
            }
            let eff_wa = self.effective_work_area_for(id);
            if let Some(update) = self.wm.set_fullscreen(id, false, eff_wa) {
                let _ = surface.set_fullscreen(false);
                if let (Some(loc), Some(sz)) = (update.target_location, update.configure_size) {
                    let _ = surface.configure(Rectangle::new(from_wm_point(loc), from_wm_size(sz)));
                }
                self.apply_layout_update(&update);
                self.needs_redraw = true;
            }
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((id, _)) = self.find_managed_x11(xid) {
            self.minimize_window(id, SERIAL_COUNTER.next_serial());
        }
    }

    fn unminimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let xid = surface.window_id();
        if let Some((id, _)) = self.find_managed_x11(xid) {
            self.restore_minimized_window(id, SERIAL_COUNTER.next_serial());
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
