//! Interactive floating window move and resize pointer grabs.

use bitflags::bitflags;
use neon_window_manager::tiling::TilingDivider;
use neon_workspaces::WorkspaceId;
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, GestureHoldBeginEvent,
        GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
        GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Size},
};

use crate::{
    state::NeonState,
    wayland::seat::FocusTarget,
    window::{to_wm_point, to_wm_size, window_id},
};

/// Interactive floating & tiled window move grab.
pub struct MoveSurfaceGrab {
    pub start_data: PointerGrabStartData<NeonState>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
    /// When dragging from a maximized/snapped titlebar, defer transitioning back to floating
    /// until pointer motion exceeds a small drag threshold so double-clicking to unmaximize
    /// plays the full smooth Material 3 restore animation cleanly.
    pub pending_unmaximize: bool,
    /// When grabbing a tiled window's titlebar, wait for a small drag deadzone (`>= 6 px`)
    /// before detaching visually so normal clicks & double-clicks never jitter the tile.
    pub pending_tile_drag: bool,
    /// Tracks whether `commit_drag_snap` already ran on button release so aborted grabs
    /// safely snap tiled windows back into their layout slot.
    pub committed: bool,
}

impl PointerGrab<NeonState> for MoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        _focus: Option<(FocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let mut delta = event.location - self.start_data.location;
        if self.pending_tile_drag {
            if delta.x.hypot(delta.y) < 6.0 {
                return;
            }
            self.pending_tile_drag = false;
            data.cursor_status = CursorImageStatus::Named(CursorIcon::Grabbing);
        }
        if self.pending_unmaximize {
            if delta.x.hypot(delta.y) < 6.0 {
                return;
            }
            self.pending_unmaximize = false;
            if let Some(id) = window_id(&self.window) {
                let old_loc = self.initial_window_location;
                let old_size = self.window.geometry().size;
                let work_area = data.effective_work_area_for(id);
                if let Some(update) = data.wm.set_floating(id, work_area) {
                    if let Some(x11) = self.window.x11_surface() {
                        let _ = x11.set_maximized(false);
                        let _ = x11.set_fullscreen(false);
                        if let (Some(loc), Some(sz)) =
                            (update.target_location, update.configure_size)
                        {
                            let _ = x11.configure(Rectangle::new(
                                crate::window::from_wm_point(loc),
                                crate::window::from_wm_size(sz),
                            ));
                        }
                    }
                    data.apply_layout_update(&update);
                    if old_size.w > 0 {
                        let new_w = update
                            .configure_size
                            .map(|s| s.w)
                            .unwrap_or(old_size.w)
                            .max(200);
                        let ratio_x = ((event.location.x - old_loc.x as f64) / old_size.w as f64)
                            .clamp(0.12, 0.88);
                        let new_x = (event.location.x - new_w as f64 * ratio_x).round() as i32;
                        let new_y =
                            (event.location.y - 16.0).round().max(work_area.loc.y as f64) as i32;
                        let adjusted: Point<i32, Logical> = (new_x, new_y).into();
                        data.space.map_element(self.window.clone(), adjusted, true);
                        data.wm.set_floating_location(id, to_wm_point(adjusted));
                        if let Some(x11) = self.window.x11_surface() {
                            let mut g = x11.geometry();
                            g.loc = adjusted;
                            let _ = x11.configure(g);
                        }
                        self.initial_window_location = adjusted;
                    } else if let Some(loc) = data.space.element_location(&self.window) {
                        self.initial_window_location = loc;
                    }
                    self.start_data.location = event.location;
                    delta = (0.0, 0.0).into();
                }
            }
        }

        let new_location = self.initial_window_location.to_f64() + delta;
        let mut rounded: Point<i32, Logical> = new_location.to_i32_round();
        // Prevent dragging a window's top edge above/under the Top Bar when Top Bar auto-hide is off.
        if let Some(id) = window_id(&self.window) {
            let min_y = data.effective_work_area_for(id).loc.y;
            if rounded.y < min_y {
                rounded.y = min_y;
            }
            let is_tiled = data
                .wm
                .get(id)
                .map(|w| w.mode.is_tiled())
                .unwrap_or(false);
            if !is_tiled {
                data.wm.set_floating_location(id, to_wm_point(rounded));
            }
            data.update_drag_snap(id, event.location);
        }
        data.space.map_element(self.window.clone(), rounded, true);
        if let Some(x11) = self.window.x11_surface() {
            let mut geo = x11.geometry();
            geo.loc = rounded;
            let _ = x11.configure(geo);
        }
        data.needs_redraw = true;
    }

    fn relative_motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        focus: Option<(FocusTarget, Point<f64, Logical>)>,
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
            self.committed = true;
            if let Some(id) = window_id(&self.window) {
                if !self.pending_tile_drag {
                    data.commit_drag_snap(id);
                } else {
                    data.drag_snap.clear();
                }
            }
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

    fn unset(&mut self, data: &mut NeonState) {
        if data.drag_snap.is_active() {
            data.drag_snap.clear();
        }
        // If a tiled window grab was cancelled without committing (e.g. Escape/focus change),
        // snap it right back to its tile so it can never remain stranded off-grid.
        if !self.committed
            && !self.pending_tile_drag
            && let Some(id) = window_id(&self.window)
            && let Some(ws_id) = data
                .wm
                .get(id)
                .filter(|w| w.mode.is_tiled())
                .map(|w| w.workspace_id)
        {
            data.relayout_auto_tiling_for_workspace(ws_id);
        }
        data.cursor_status = CursorImageStatus::Named(CursorIcon::Default);
        data.needs_redraw = true;
    }
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
        _focus: Option<(FocusTarget, Point<f64, Logical>)>,
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
            if let Some(id) = window_id(&self.window) {
                let min_y = data.effective_work_area_for(id).loc.y;
                if new_y < min_y {
                    let clamp_delta = min_y - new_y;
                    new_y = min_y;
                    new_h -= clamp_delta;
                }
            }
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
        } else if let Some(x11) = self.window.x11_surface() {
            let target_rect = Rectangle::new((new_x, new_y).into(), clamped_size);
            let _ = x11.configure(target_rect);
            if let Some(id) = window_id(&self.window) {
                let work_area = data.work_area();
                data.wm
                    .update_committed_size(id, to_wm_size(clamped_size), work_area);
                data.wm
                    .set_floating_location(id, to_wm_point((new_x, new_y).into()));
            }
        }
        data.needs_redraw = true;
    }

    fn relative_motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        focus: Option<(FocusTarget, Point<f64, Logical>)>,
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

    fn unset(&mut self, data: &mut NeonState) {
        if data.drag_snap.is_active() {
            data.drag_snap.clear();
        }
        data.cursor_status = CursorImageStatus::Named(CursorIcon::Default);
        data.needs_redraw = true;
    }
}

/// Live 60 FPS interactive split-divider resize grab for Hyprland Auto-Tiling.
pub struct TileSplitResizeGrab {
    start_data: PointerGrabStartData<NeonState>,
    workspace_id: WorkspaceId,
    divider: TilingDivider,
    last_ratio_bps: i32,
}

impl TileSplitResizeGrab {
    pub fn start(
        start_data: PointerGrabStartData<NeonState>,
        workspace_id: WorkspaceId,
        divider: TilingDivider,
    ) -> Self {
        Self {
            start_data,
            workspace_id,
            last_ratio_bps: divider.ratio_bps,
            divider,
        }
    }
}

impl PointerGrab<NeonState> for TileSplitResizeGrab {
    fn motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        _focus: Option<(FocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let ratio = self
            .divider
            .ratio_for_point(event.location.x, event.location.y);
        let bps = (ratio * 10_000.0).round() as i32;
        if (bps - self.last_ratio_bps).abs() < 15 {
            return;
        }
        self.last_ratio_bps = bps;
        let work_area = data.work_area();
        let updates =
            data.wm
                .set_split_ratio(self.workspace_id, self.divider.split_index, ratio, work_area);
        data.apply_layout_updates_batch(updates, false);
    }

    fn relative_motion(
        &mut self,
        data: &mut NeonState,
        handle: &mut PointerInnerHandle<'_, NeonState>,
        focus: Option<(FocusTarget, Point<f64, Logical>)>,
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

    fn unset(&mut self, data: &mut NeonState) {
        data.hovering_tile_divider = false;
        data.cursor_status = CursorImageStatus::Named(CursorIcon::Default);
        data.needs_redraw = true;
    }
}
