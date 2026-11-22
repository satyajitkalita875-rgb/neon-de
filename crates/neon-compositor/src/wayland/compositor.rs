//! `wl_compositor`, `wl_subcompositor`, `wl_shm`, and `wl_buffer` handlers.

use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
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
        shell::xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData},
        shm::{ShmHandler, ShmState},
    },
};
use tracing::trace;

use crate::{
    state::{NeonClientData, NeonState},
    window::{window_id, window_matches_surface},
};

impl BufferHandler for NeonState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for NeonState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
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
        let matching_window = self
            .space
            .elements()
            .find(|window| window_matches_surface(window, surface))
            .cloned();

        if let Some(window) = matching_window {
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
                    trace!("Sending initial xdg_toplevel configure");
                    toplevel.send_configure();
                }

                // Update window metadata size & mapped status once a buffer/geometry is committed.
                let bbox = window.geometry();
                if let Some(id) = window_id(&window)
                    && let Some(record) = self.windows.get_mut(id)
                {
                    record.size = bbox.size;
                    if !record.mapped && bbox.size.w > 0 && bbox.size.h > 0 {
                        record.mapped = true;
                        let serial = SERIAL_COUNTER.next_serial();
                        self.focus_window(Some(&window), serial);
                    }
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
