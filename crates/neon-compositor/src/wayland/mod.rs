//! Smithay Wayland protocol handler implementations for **NEON DE**.

mod compositor;
mod layer_shell;
mod seat;
pub mod session_lock;
mod xdg_shell;
pub mod xwayland;

use smithay::{
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_fractional_scale,
    delegate_layer_shell, delegate_output, delegate_pointer_gestures, delegate_presentation,
    delegate_seat, delegate_session_lock, delegate_shm, delegate_viewporter, delegate_xdg_shell,
    delegate_xwayland_shell,
};

use crate::state::NeonState;

delegate_compositor!(NeonState);
delegate_xdg_shell!(NeonState);
delegate_layer_shell!(NeonState);
delegate_session_lock!(NeonState);
delegate_fractional_scale!(NeonState);
delegate_viewporter!(NeonState);
delegate_pointer_gestures!(NeonState);
delegate_presentation!(NeonState);
delegate_xwayland_shell!(NeonState);
delegate_shm!(NeonState);
delegate_dmabuf!(NeonState);
delegate_seat!(NeonState);
delegate_data_device!(NeonState);
delegate_output!(NeonState);
