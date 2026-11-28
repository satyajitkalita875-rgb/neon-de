//! `zwlr_layer_shell_v1` handler for desktop panels, dock, overview, and wallpapers.

use smithay::{
    desktop::{LayerSurface, PopupKind, layer_map_for_output},
    output::Output,
    reexports::wayland_server::{Resource, protocol::wl_output::WlOutput},
    utils::SERIAL_COUNTER,
    wayland::shell::{
        wlr_layer::{
            Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState,
        },
        xdg::PopupSurface,
    },
};
use tracing::{debug, info, warn};

use crate::state::NeonState;

impl WlrLayerShellHandler for NeonState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.primary_output.clone());

        let Some(output) = output else {
            warn!(
                namespace,
                "No active output available for new layer surface"
            );
            return;
        };

        let prev_work_area = self.work_area();
        let desktop_layer = LayerSurface::new(surface, namespace.clone());
        {
            let mut map = layer_map_for_output(&output);
            if let Err(err) = map.map_layer(&desktop_layer) {
                warn!(namespace, ?err, "Failed to map layer surface onto output");
                return;
            }
        }

        info!(
            namespace,
            ?layer,
            output = output.name(),
            "Mapped zwlr_layer_shell_v1 surface"
        );
        if self.work_area() != prev_work_area {
            self.relayout_non_floating_windows();
        }
        self.needs_redraw = true;
    }

    fn new_popup(&mut self, parent: WlrLayerSurface, popup: PopupSurface) {
        // Unconstrain layer-shell popups (panel/dock/desktop menus & tooltips) within the active output.
        self.unconstrain_popup_with_root(&popup, parent.wl_surface());
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            warn!(?err, "Failed to track popup for layer surface");
        }
        self.needs_redraw = true;
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let destroyed_id = surface.wl_surface().id();
        let prev_work_area = self.work_area();
        let mut removed = false;
        if let Some(output) = self.primary_output.clone() {
            let mut map = layer_map_for_output(&output);
            let target = map
                .layers()
                .find(|l| l.wl_surface().id() == destroyed_id)
                .cloned();
            if let Some(layer) = target {
                map.unmap_layer(&layer);
                removed = true;
            }
        }

        if removed {
            debug!("Unmapped destroyed zwlr_layer_shell_v1 surface");
            if self.work_area() != prev_work_area {
                self.relayout_non_floating_windows();
            }
            self.needs_redraw = true;

            let kbd_held_by_destroyed = self
                .seat
                .get_keyboard()
                .and_then(|k| k.current_focus())
                .and_then(|s| s.wl_surface_id())
                .map(|id| id == destroyed_id)
                .unwrap_or(false);
            if kbd_held_by_destroyed {
                self.focus_topmost_window(SERIAL_COUNTER.next_serial());
            }

            // Defer pointer hover refresh to the next idle turn (never synchronously inside
            // `layer_destroyed`, which may run inside `wl_client_destroy`'s C destructor) so
            // underlying windows regain hover immediately once an overlay/backdrop unmaps.
            self.loop_handle.insert_idle(|state| {
                if state.is_locked() {
                    return;
                }
                if let Some(pointer) = state.seat.get_pointer()
                    && !pointer.is_grabbed()
                {
                    let pos = state.pointer_location;
                    let under = state.surface_under(pos);
                    let cur_id = pointer.current_focus().and_then(|f| f.wl_surface_id());
                    let under_id = under.as_ref().and_then(|(f, _)| f.wl_surface_id());
                    if cur_id != under_id {
                        pointer.motion(
                            state,
                            under,
                            &smithay::input::pointer::MotionEvent {
                                location: pos,
                                serial: SERIAL_COUNTER.next_serial(),
                                time: state.clock.now().as_millis(),
                            },
                        );
                        pointer.frame(state);
                    }
                }
            });
        }
    }
}
