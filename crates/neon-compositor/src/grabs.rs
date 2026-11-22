//! Interactive floating window move and resize pointer grabs.

use bitflags::bitflags;
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    state::NeonState,
    window::{to_wm_point, window_id},
};

/// Interactive floating window move grab.
pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<NeonState>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<NeonState> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        let rounded: Point<i32, Logical> = new_location.to_i32_round();
        data.space.map_element(self.window.clone(), rounded, true);
        if let Some(id) = window_id(&self.window) {
            data.wm.set_floating_location(id, to_wm_point(rounded));
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        const BTN_LEFT: u32 = 0x110;
        if !handle.current_pressed().contains(&BTN_LEFT) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut NeonState, handle: &mut PointerInnerHandle<'_, NeonState>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<NeonState> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut NeonState) {}
}

bitflags! {
    /// Edge bitflags for interactive floating window resizing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ResizeEdge: u32 {
        const TOP    = 0b0001;
        const BOTTOM = 0b0010;
        const LEFT   = 0b0100;
        const RIGHT  = 0b1000;
        const TOP_LEFT     = Self::TOP.bits() | Self::LEFT.bits();
        const BOTTOM_LEFT  = Self::BOTTOM.bits() | Self::LEFT.bits();
        const TOP_RIGHT    = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    fn from(edge: xdg_toplevel::ResizeEdge) -> Self {
        match edge {
            xdg_toplevel::ResizeEdge::Top => Self::TOP,
            xdg_toplevel::ResizeEdge::Bottom => Self::BOTTOM,
            xdg_toplevel::ResizeEdge::Left => Self::LEFT,
            xdg_toplevel::ResizeEdge::Right => Self::RIGHT,
            xdg_toplevel::ResizeEdge::TopLeft => Self::TOP_LEFT,
            xdg_toplevel::ResizeEdge::BottomLeft => Self::BOTTOM_LEFT,
            xdg_toplevel::ResizeEdge::TopRight => Self::TOP_RIGHT,
            xdg_toplevel::ResizeEdge::BottomRight => Self::BOTTOM_RIGHT,
            _ => Self::BOTTOM_RIGHT,
        }
    }
}

/// Interactive floating window resize grab.
pub struct ResizeSurfaceGrab {
    start_data: PointerGrabStartData<NeonState>,
    window: Window,
    edges: ResizeEdge,
    initial_rect: Rectangle<i32, Logical>,
    last_window_size: Size<i32, Logical>,
}

impl ResizeSurfaceGrab {
    pub fn start(
        start_data: PointerGrabStartData<NeonState>,
        window: Window,
        edges: ResizeEdge,
        initial_rect: Rectangle<i32, Logical>,
    ) -> Self {
        Self {
            start_data,
            window,
            edges,
            initial_rect,
            last_window_size: initial_rect.size,
        }
    }
}

impl PointerGrab<NeonState> for ResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let mut new_w = self.initial_rect.size.w;
        let mut new_h = self.initial_rect.size.h;
        let mut new_x = self.initial_rect.loc.x;
        let mut new_y = self.initial_rect.loc.y;

        if self.edges.contains(ResizeEdge::RIGHT) {
            new_w = (self.initial_rect.size.w as f64 + delta.x) as i32;
        } else if self.edges.contains(ResizeEdge::LEFT) {
            new_w = (self.initial_rect.size.w as f64 - delta.x) as i32;
            new_x = self.initial_rect.loc.x + (self.initial_rect.size.w - new_w.max(160));
        }

        if self.edges.contains(ResizeEdge::BOTTOM) {
            new_h = (self.initial_rect.size.h as f64 + delta.y) as i32;
        } else if self.edges.contains(ResizeEdge::TOP) {
            new_h = (self.initial_rect.size.h as f64 - delta.y) as i32;
            new_y = self.initial_rect.loc.y + (self.initial_rect.size.h - new_h.max(100));
        }

        let clamped_size: Size<i32, Logical> = (new_w.max(160), new_h.max(100)).into();
        self.last_window_size = clamped_size;

        if self.edges.intersects(ResizeEdge::LEFT | ResizeEdge::TOP) {
            data.space
                .map_element(self.window.clone(), (new_x, new_y), true);
            if let Some(id) = window_id(&self.window) {
                data.wm
                    .set_floating_location(id, to_wm_point((new_x, new_y).into()));
            }
        }

        if let Some(toplevel) = self.window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
                state.size = Some(clamped_size);
            });
            toplevel.send_pending_configure();
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        if !handle.current_pressed().contains(&self.start_data.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            if let Some(toplevel) = self.window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.states.unset(xdg_toplevel::State::Resizing);
                    state.size = Some(self.last_window_size);
                });
                toplevel.send_pending_configure();
            }
        }
    }

    fn axis(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut NeonState, handle: &mut PointerInnerHandle<'_, NeonState>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerGrabStartData<NeonState> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut NeonState) {}
}
