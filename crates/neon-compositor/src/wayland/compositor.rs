//! `wl_compositor`, `wl_subcompositor`, `wl_shm`, `zwp_linux_dmabuf_v1`, and `wl_buffer` handlers.

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::on_commit_buffer_handler},
    desktop::{WindowSurfaceType, find_popup_root_surface, layer_map_for_output},
    reexports::wayland_server::{
        Client, Resource,
        protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
    },
    utils::SERIAL_COUNTER,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, is_sync_subsurface,
            with_states,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        shell::{
            wlr_layer::{KeyboardInteractivity, LayerSurfaceCachedState, LayerSurfaceData},
            xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData},
        },
        shm::{ShmHandler, ShmState},
    },
    xwayland::XWaylandClientData,
};
use tracing::{debug, trace};

use crate::{
    state::{NeonClientData, NeonState},
    window::{from_wm_point, to_wm_size},
};

impl BufferHandler for NeonState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for NeonState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for NeonState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let imported = self
            .dmabuf_import_hook
            .as_mut()
            .map(|hook| hook(&dmabuf))
            .unwrap_or(false);

        if imported {
            let _ = notifier.successful::<Self>();
        } else {
            debug!("Rejecting unsupported DMA-BUF import");
            notifier.failed();
        }
    }
}

impl FractionalScaleHandler for NeonState {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self
            .primary_output
            .as_ref()
            .map(|o| o.current_scale().fractional_scale())
            .unwrap_or(1.0);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fs| {
                fs.set_preferred_scale(scale);
            });
        });
    }
}

impl CompositorHandler for NeonState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(data) = client.get_data::<NeonClientData>() {
            &data.compositor_state
        } else if let Some(data) = client.get_data::<XWaylandClientData>() {
            &data.compositor_state
        } else {
            panic!("Unknown Wayland client data attached to client")
        }
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        let scale = self
            .primary_output
            .as_ref()
            .map(|o| o.current_scale().fractional_scale())
            .unwrap_or(1.0);
        with_states(surface, |states| {
            with_fractional_scale(states, |fs| {
                fs.set_preferred_scale(scale);
            });
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if is_sync_subsurface(surface) {
            return;
        }

        // Refresh window geometry & handle initial xdg_toplevel configure handshake.
        if let Some((id, window)) = self.find_window_by_surface(surface) {
            window.on_commit();

            if let Some(toplevel) = window.toplevel() {
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .map(|data| data.lock().unwrap().initial_configure_sent)
                        .unwrap_or(true)
                });

                if !initial_configure_sent {
                    // Auto-float fixed-size or small utility dialogs (e.g. calculators, password
                    // prompts, compact popups) so Auto-Tiling never stretches them awkwardly.
                    let is_fixed_or_tiny = with_states(surface, |states| {
                        let mut cached = states
                            .cached_state
                            .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
                        let cur = cached.current();
                        let fixed = cur.min_size.w > 0
                            && cur.min_size.h > 0
                            && cur.min_size == cur.max_size;
                        let tiny_max = cur.max_size.w > 0
                            && cur.max_size.w < 500
                            && cur.max_size.h > 0
                            && cur.max_size.h < 400;
                        fixed || tiny_max
                    });
                    if is_fixed_or_tiny {
                        self.wm.set_float_override(id, true);
                    }
                    // Evaluate any matching window rules before sending the initial configure so
                    // maximized/snapped/sized rules take effect on the very first frame.
                    let work_area = self.work_area();
                    if let Some((_eval, update)) = self.wm.apply_rules_if_needed(id, work_area) {
                        self.apply_layout_update(&update);
                    }
                    if self.wm.is_auto_tiling()
                        && let Some(ws_id) = self.wm.get(id).map(|w| w.workspace_id)
                    {
                        self.relayout_auto_tiling_for_workspace(ws_id);
                    }
                    trace!(?id, "Sending initial xdg_toplevel configure");
                    toplevel.send_configure();
                }

                // Update window metadata size & mapped status once a buffer/geometry is committed.
                let bbox = window.geometry();
                let was_mapped = self.wm.get(id).map(|w| w.mapped).unwrap_or(false);
                let work_area = self.work_area();
                let recentered =
                    self.wm
                        .update_committed_size(id, to_wm_size(bbox.size), work_area);

                if let Some(new_loc) = recentered
                    && self
                        .wm
                        .get(id)
                        .map(|w| w.is_visible_on(self.wm.active_workspace_id()))
                        .unwrap_or(false)
                {
                    self.map_preserving_z_order(window.clone(), from_wm_point(new_loc));
                }

                if !was_mapped
                    && bbox.size.w > 0
                    && bbox.size.h > 0
                    && self
                        .wm
                        .get(id)
                        .map(|w| w.is_visible_on(self.wm.active_workspace_id()))
                        .unwrap_or(false)
                {
                    self.animate_window_open_android(id, &window);
                    let serial = SERIAL_COUNTER.next_serial();
                    self.focus_window(Some(&window), serial);
                }
            }
        }

        // Handle zwlr_layer_shell_v1 surface commits, initial configure, and exclusive keyboard focus.
        if let Some(output) = self.primary_output.clone() {
            let prev_work_area = self.work_area();
            let (arranged_changed, exclusive_surface, released_surface_id) = {
                let mut map = layer_map_for_output(&output);
                if let Some(layer) = map
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .cloned()
                {
                    let changed = map.arrange();
                    let initial_configure_sent = with_states(surface, |states| {
                        states
                            .data_map
                            .get::<LayerSurfaceData>()
                            .map(|d| d.lock().unwrap().initial_configure_sent)
                            .unwrap_or(true)
                    });
                    if !initial_configure_sent {
                        layer.layer_surface().send_configure();
                    }
                    if layer.namespace() == "neon-panel"
                        && let Some(geo) = map.layer_geometry(&layer)
                        && geo.size.h > 0
                    {
                        self.last_panel_height = geo.size.h;
                    }
                    if layer.namespace() == "neon-dock" {
                        let ex = with_states(surface, |states| {
                            match states
                                .cached_state
                                .get::<LayerSurfaceCachedState>()
                                .current()
                                .exclusive_zone
                            {
                                smithay::wayland::shell::wlr_layer::ExclusiveZone::Exclusive(n) => {
                                    n as i32
                                }
                                _ => 0,
                            }
                        });
                        if ex > 0 {
                            self.last_dock_exclusive = ex;
                        }
                    }
                    let interactivity = with_states(surface, |states| {
                        states
                            .cached_state
                            .get::<LayerSurfaceCachedState>()
                            .current()
                            .keyboard_interactivity
                    });
                    let exclusive = (interactivity == KeyboardInteractivity::Exclusive)
                        .then(|| layer.wl_surface().clone());
                    let released = (interactivity == KeyboardInteractivity::None)
                        .then(|| layer.wl_surface().id());
                    (changed, exclusive, released)
                } else {
                    (false, None, None)
                }
            };

            if arranged_changed && self.work_area() != prev_work_area {
                self.relayout_non_floating_windows();
            }
            if let Some(layer_wl) = exclusive_surface
                && let Some(keyboard) = self.seat.get_keyboard()
            {
                keyboard.set_focus(self, Some(layer_wl.into()), SERIAL_COUNTER.next_serial());
            } else if let Some(released_id) = released_surface_id
                && self
                    .seat
                    .get_keyboard()
                    .and_then(|k| k.current_focus())
                    .and_then(|s| s.wl_surface_id())
                    .is_some_and(|id| id == released_id)
            {
                self.focus_topmost_window(SERIAL_COUNTER.next_serial());
            }
        }

        // Handle popup commit, initial configure, and refresh root window bbox so popups
        // extending outside parent windows render and hit-test accurately.
        self.popups.commit(surface);
        if let Some(popup) = self.popups.find_popup(surface) {
            if let Ok(root) = find_popup_root_surface(&popup)
                && let Some((_, win)) = self.find_window_by_surface(&root)
            {
                win.on_commit();
            }
            if let smithay::desktop::PopupKind::Xdg(xdg_popup) = popup {
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgPopupSurfaceData>()
                        .map(|data| data.lock().unwrap().initial_configure_sent)
                        .unwrap_or(true)
                });
                if !initial_configure_sent {
                    let _ = xdg_popup.send_configure();
                }
            }
        }
        self.needs_redraw = true;
    }

    fn destroyed(&mut self, _surface: &WlSurface) {
        self.cleanup_dead_windows();
    }
}
