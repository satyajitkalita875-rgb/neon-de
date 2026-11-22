//! Bridge helpers connecting `neon-window-manager` types with Smithay's `Window` and `Logical` geometry.

pub use neon_window_manager::{
    FocusPolicy, ManagedWindow, WindowId, WindowLayoutUpdate, WindowManager,
    geometry::{Point as WmPoint, Rect as WmRect, Size as WmSize, SnapRegion},
    rules::{WindowMatcher, WindowRule, WindowRuleAction},
    state_machine::WindowMode,
};
use smithay::{
    desktop::Window,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, Size},
    wayland::seat::WaylandFocus,
};

/// Associates a stable [`WindowId`] with a Smithay [`Window`]'s `UserDataMap`.
pub fn attach_window_id(window: &Window, id: WindowId) {
    window.user_data().insert_if_missing(|| id);
}

/// Retrieves the [`WindowId`] attached to a Smithay [`Window`], if present.
pub fn window_id(window: &Window) -> Option<WindowId> {
    window.user_data().get::<WindowId>().copied()
}

/// Returns true if a Smithay [`Window`] (Wayland or XWayland) matches the given `WlSurface` object ID.
pub fn window_matches_surface(window: &Window, surface: &WlSurface) -> bool {
    window
        .wl_surface()
        .map(|s| s.id() == surface.id())
        .unwrap_or(false)
}

pub fn to_wm_point(p: Point<i32, Logical>) -> WmPoint {
    WmPoint::new(p.x, p.y)
}

pub fn from_wm_point(p: WmPoint) -> Point<i32, Logical> {
    (p.x, p.y).into()
}

pub fn to_wm_size(s: Size<i32, Logical>) -> WmSize {
    WmSize::new(s.w, s.h)
}

pub fn from_wm_size(s: WmSize) -> Size<i32, Logical> {
    (s.w, s.h).into()
}

pub fn to_wm_rect(r: Rectangle<i32, Logical>) -> WmRect {
    WmRect::new(to_wm_point(r.loc), to_wm_size(r.size))
}
