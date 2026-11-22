//! `wl_compositor`, `wl_subcompositor`, `wl_shm`, `zwp_linux_dmabuf_v1`, and `wl_buffer` handlers.

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::on_commit_buffer_handler},
    reexports::wayland_server::{
        Client,
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
        shell::xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData},
        shm::{ShmHandler, ShmState},
    },
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

impl CompositorHandler for NeonState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<NeonClientData>()
            .expect("NeonClientData must be attached to every Wayland client")
            .compositor_state
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
                    // Evaluate any matching window rules before sending the initial configure so
                    // maximized/snapped/sized rules take effect on the very first frame.
                    let work_area = self.work_area();
                    if let Some((_eval, update)) = self.wm.apply_rules_if_needed(id, work_area) {
                        self.apply_layout_update(&update);
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
                    self.space
                        .map_element(window.clone(), from_wm_point(new_loc), false);
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
                    let now = self.now_duration();
                    self.effects.animate_window_open(id, now);
                    let serial = SERIAL_COUNTER.next_serial();
                    self.focus_window(Some(&window), serial);
                }
            }
        }

        // Handle popup commit and initial configure.
        self.popups.commit(surface);
        if let Some(popup) = self.popups.find_popup(surface)
            && let smithay::desktop::PopupKind::Xdg(xdg_popup) = popup
        {
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

    fn destroyed(&mut self, _surface: &WlSurface) {
        self.cleanup_dead_windows();
    }
}
