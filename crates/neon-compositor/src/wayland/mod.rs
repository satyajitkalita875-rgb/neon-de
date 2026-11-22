//! Smithay Wayland protocol handler implementations for **NEON DE**.

mod compositor;
mod seat;
mod xdg_shell;

use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
};

use crate::state::NeonState;

delegate_compositor!(NeonState);
delegate_xdg_shell!(NeonState);
delegate_shm!(NeonState);
delegate_seat!(NeonState);
delegate_data_device!(NeonState);
delegate_output!(NeonState);
