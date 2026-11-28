//! `wl_seat`, `wl_data_device_manager`, and `wl_output` handlers, plus unified Wayland + XWayland
//! [`FocusTarget`] so Smithay's `X11Surface` `KeyboardTarget` (`XSetInputFocus` + `WM_TAKE_FOCUS`)
//! runs reliably for X11 apps such as DaVinci Resolve.

use std::borrow::Cow;

use smithay::{
    backend::input::KeyState,
    desktop::PopupKind,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, CursorImageStatus, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            PointerTarget, RelativeMotionEvent,
        },
        touch::{DownEvent, MotionEvent as TouchMotionEvent, OrientationEvent, ShapeEvent, TouchTarget, UpEvent},
    },
    reexports::wayland_server::{
        Resource, backend::ObjectId,
        protocol::{wl_data_source::WlDataSource, wl_surface::WlSurface},
    },
    utils::{IsAlive, Serial},
    wayland::{
        output::OutputHandler,
        seat::WaylandFocus,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            wlr_data_control::{DataControlHandler, DataControlState},
        },
        tablet_manager::TabletSeatHandler,
    },
    xwayland::X11Surface,
};

use crate::state::NeonState;

/// Unified seat focus target covering both native Wayland surfaces (toplevels, popups, layer-shell,
/// lock screens) and XWayland `X11Surface` windows (driving `XSetInputFocus` + `WM_TAKE_FOCUS`).
#[derive(Debug, Clone, PartialEq)]
pub enum FocusTarget {
    Wayland(WlSurface),
    X11(X11Surface),
}

impl FocusTarget {
    /// Returns the underlying `WlSurface` object ID if currently associated.
    pub fn wl_surface_id(&self) -> Option<ObjectId> {
        self.wl_surface().map(|s| s.id())
    }
}

impl From<WlSurface> for FocusTarget {
    fn from(surface: WlSurface) -> Self {
        Self::Wayland(surface)
    }
}

impl From<X11Surface> for FocusTarget {
    fn from(surface: X11Surface) -> Self {
        Self::X11(surface)
    }
}

impl From<PopupKind> for FocusTarget {
    fn from(popup: PopupKind) -> Self {
        Self::Wayland(popup.wl_surface().clone())
    }
}

impl IsAlive for FocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(s) => s.alive(),
            Self::X11(s) => s.alive(),
        }
    }
}

impl WaylandFocus for FocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(s) => Some(Cow::Borrowed(s)),
            Self::X11(s) => s.wl_surface().map(Cow::Owned),
        }
    }
}

impl KeyboardTarget<NeonState> for FocusTarget {
    fn enter(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(s) => KeyboardTarget::enter(s, seat, data, keys, serial),
            Self::X11(s) => KeyboardTarget::enter(s, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<NeonState>, data: &mut NeonState, serial: Serial) {
        match self {
            Self::Wayland(s) => KeyboardTarget::leave(s, seat, data, serial),
            Self::X11(s) => KeyboardTarget::leave(s, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            Self::Wayland(s) => KeyboardTarget::key(s, seat, data, key, state, serial, time),
            Self::X11(s) => KeyboardTarget::key(s, seat, data, key, state, serial, time),
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(s) => KeyboardTarget::modifiers(s, seat, data, modifiers, serial),
            Self::X11(s) => KeyboardTarget::modifiers(s, seat, data, modifiers, serial),
        }
    }
}

impl PointerTarget<NeonState> for FocusTarget {
    fn enter(&self, seat: &Seat<NeonState>, data: &mut NeonState, event: &MotionEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::enter(s, seat, data, event),
            Self::X11(s) => PointerTarget::enter(s, seat, data, event),
        }
    }

    fn motion(&self, seat: &Seat<NeonState>, data: &mut NeonState, event: &MotionEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::motion(s, seat, data, event),
            Self::X11(s) => PointerTarget::motion(s, seat, data, event),
        }
    }

    fn relative_motion(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &RelativeMotionEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::relative_motion(s, seat, data, event),
            Self::X11(s) => PointerTarget::relative_motion(s, seat, data, event),
        }
    }

    fn button(&self, seat: &Seat<NeonState>, data: &mut NeonState, event: &ButtonEvent) {
        match self {
            Self::Wayland(s) => PointerTarget::button(s, seat, data, event),
            Self::X11(s) => PointerTarget::button(s, seat, data, event),
        }
    }

    fn axis(&self, seat: &Seat<NeonState>, data: &mut NeonState, frame: AxisFrame) {
        match self {
            Self::Wayland(s) => PointerTarget::axis(s, seat, data, frame),
            Self::X11(s) => PointerTarget::axis(s, seat, data, frame),
        }
    }

    fn frame(&self, seat: &Seat<NeonState>, data: &mut NeonState) {
        match self {
            Self::Wayland(s) => PointerTarget::frame(s, seat, data),
            Self::X11(s) => PointerTarget::frame(s, seat, data),
        }
    }

    fn gesture_swipe_begin(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GestureSwipeBeginEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_swipe_begin(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_swipe_begin(s, seat, data, event),
        }
    }

    fn gesture_swipe_update(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GestureSwipeUpdateEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_swipe_update(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_swipe_update(s, seat, data, event),
        }
    }

    fn gesture_swipe_end(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GestureSwipeEndEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_swipe_end(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_swipe_end(s, seat, data, event),
        }
    }

    fn gesture_pinch_begin(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GesturePinchBeginEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_pinch_begin(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_pinch_begin(s, seat, data, event),
        }
    }

    fn gesture_pinch_update(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GesturePinchUpdateEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_pinch_update(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_pinch_update(s, seat, data, event),
        }
    }

    fn gesture_pinch_end(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GesturePinchEndEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_pinch_end(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_pinch_end(s, seat, data, event),
        }
    }

    fn gesture_hold_begin(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GestureHoldBeginEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_hold_begin(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_hold_begin(s, seat, data, event),
        }
    }

    fn gesture_hold_end(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &GestureHoldEndEvent,
    ) {
        match self {
            Self::Wayland(s) => PointerTarget::gesture_hold_end(s, seat, data, event),
            Self::X11(s) => PointerTarget::gesture_hold_end(s, seat, data, event),
        }
    }

    fn leave(&self, seat: &Seat<NeonState>, data: &mut NeonState, serial: Serial, time: u32) {
        match self {
            Self::Wayland(s) => PointerTarget::leave(s, seat, data, serial, time),
            Self::X11(s) => PointerTarget::leave(s, seat, data, serial, time),
        }
    }
}

impl TouchTarget<NeonState> for FocusTarget {
    fn down(&self, seat: &Seat<NeonState>, data: &mut NeonState, event: &DownEvent, seq: Serial) {
        match self {
            Self::Wayland(s) => TouchTarget::down(s, seat, data, event, seq),
            Self::X11(s) => TouchTarget::down(s, seat, data, event, seq),
        }
    }

    fn up(&self, seat: &Seat<NeonState>, data: &mut NeonState, event: &UpEvent, seq: Serial) {
        match self {
            Self::Wayland(s) => TouchTarget::up(s, seat, data, event, seq),
            Self::X11(s) => TouchTarget::up(s, seat, data, event, seq),
        }
    }

    fn motion(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &TouchMotionEvent,
        seq: Serial,
    ) {
        match self {
            Self::Wayland(s) => TouchTarget::motion(s, seat, data, event, seq),
            Self::X11(s) => TouchTarget::motion(s, seat, data, event, seq),
        }
    }

    fn frame(&self, seat: &Seat<NeonState>, data: &mut NeonState, seq: Serial) {
        match self {
            Self::Wayland(s) => TouchTarget::frame(s, seat, data, seq),
            Self::X11(s) => TouchTarget::frame(s, seat, data, seq),
        }
    }

    fn cancel(&self, seat: &Seat<NeonState>, data: &mut NeonState, seq: Serial) {
        match self {
            Self::Wayland(s) => TouchTarget::cancel(s, seat, data, seq),
            Self::X11(s) => TouchTarget::cancel(s, seat, data, seq),
        }
    }

    fn shape(&self, seat: &Seat<NeonState>, data: &mut NeonState, event: &ShapeEvent, seq: Serial) {
        match self {
            Self::Wayland(s) => TouchTarget::shape(s, seat, data, event, seq),
            Self::X11(s) => TouchTarget::shape(s, seat, data, event, seq),
        }
    }

    fn orientation(
        &self,
        seat: &Seat<NeonState>,
        data: &mut NeonState,
        event: &OrientationEvent,
        seq: Serial,
    ) {
        match self {
            Self::Wayland(s) => TouchTarget::orientation(s, seat, data, event, seq),
            Self::X11(s) => TouchTarget::orientation(s, seat, data, event, seq),
        }
    }
}

impl SeatHandler for NeonState {
    type KeyboardFocus = FocusTarget;
    type PointerFocus = FocusTarget;
    type TouchFocus = FocusTarget;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&FocusTarget>) {
        let client = focused
            .and_then(|target| target.wl_surface())
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
        self.needs_redraw = true;
    }
}

impl TabletSeatHandler for NeonState {}

impl SelectionHandler for NeonState {
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for NeonState {
    fn started(
        &mut self,
        _source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        _seat: Seat<Self>,
    ) {
        self.dnd_icon = icon;
        self.pending_drag_state = Some(true);
        self.needs_redraw = true;
    }

    fn dropped(&mut self, _target: Option<WlSurface>, _validated: bool, _seat: Seat<Self>) {
        self.dnd_icon = None;
        self.pending_drag_state = Some(false);
        self.needs_redraw = true;
    }
}
impl ServerDndGrabHandler for NeonState {}

impl DataDeviceHandler for NeonState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl DataControlHandler for NeonState {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}

impl OutputHandler for NeonState {}
