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
        self.relayout_non_floating_windows();
    }

    fn new_popup(&mut self, parent: WlrLayerSurface, popup: PopupSurface) {
        // Unconstrain layer-shell popups (panel/dock/desktop menus) within the active output.
        if let Some(output) = self.primary_output.as_ref()
            && let Some(output_geo) = self.space.output_geometry(output)
        {
            let map = layer_map_for_output(output);
            let parent_id = parent.wl_surface().id();
            if let Some(layer) = map.layers().find(|l| l.wl_surface().id() == parent_id)
                && let Some(layer_geo) = map.layer_geometry(layer)
            {
                let mut target = output_geo;
                target.loc -= layer_geo.loc;
                popup.with_pending_state(|state| {
                    state.geometry = state.positioner.get_unconstrained_geometry(target);
                });
            }
        }
        if let Err(err) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            warn!(?err, "Failed to track popup for layer surface");
        }
        self.needs_redraw = true;
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let destroyed_id = surface.wl_surface().id();
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
            self.relayout_non_floating_windows();

            let kbd_held_by_destroyed = self
                .seat
                .get_keyboard()
                .and_then(|k| k.current_focus())
                .map(|s| s.id() == destroyed_id)
                .unwrap_or(false);
            if kbd_held_by_destroyed {
                self.focus_topmost_window(SERIAL_COUNTER.next_serial());
            }
        }
    }
}
