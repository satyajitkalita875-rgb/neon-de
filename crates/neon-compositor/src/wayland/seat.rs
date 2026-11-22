//! `wl_seat`, `wl_data_device_manager`, and `wl_output` handlers.

use smithay::{
    input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus},
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    wayland::{
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
    },
};

use crate::state::NeonState;

impl SeatHandler for NeonState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }
}

impl SelectionHandler for NeonState {
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for NeonState {}
impl ServerDndGrabHandler for NeonState {}

impl DataDeviceHandler for NeonState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl OutputHandler for NeonState {}
