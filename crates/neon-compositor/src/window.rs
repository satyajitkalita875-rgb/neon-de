//! NEON DE floating window model, metadata registry, and placement policy.

use std::collections::HashMap;

use smithay::{
    desktop::Window,
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, Size},
};

/// Monotonic internal window identifier independent of Wayland object IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// Tracked metadata and lifecycle state for a managed NEON DE toplevel window.
#[derive(Debug, Clone)]
pub struct WindowRecord {
    pub id: WindowId,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub location: Point<i32, Logical>,
    pub size: Size<i32, Logical>,
    pub mapped: bool,
    pub focused: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    /// Saved floating geometry used to restore a window when leaving maximized/fullscreen state.
    pub saved_floating_geometry: Option<Rectangle<i32, Logical>>,
}

/// Manages NEON DE's window metadata, focus history, and floating placement policy.
#[derive(Debug, Default)]
pub struct WindowRegistry {
    next_id: u64,
    records: HashMap<WindowId, WindowRecord>,
    /// Ordered from least-recently focused (front) to most-recently focused (back).
    focus_history: Vec<WindowId>,
    cascade_index: i32,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a newly created toplevel window and assigns it a stable [`WindowId`].
    pub fn register(&mut self, initial_location: Point<i32, Logical>) -> WindowId {
        self.next_id += 1;
        let id = WindowId(self.next_id);
        self.records.insert(
            id,
            WindowRecord {
                id,
                app_id: None,
                title: None,
                location: initial_location,
                size: (0, 0).into(),
                mapped: false,
                focused: false,
                maximized: false,
                fullscreen: false,
                saved_floating_geometry: None,
            },
        );
        self.focus_history.push(id);
        id
    }

    /// Removes a window record when its toplevel surface is destroyed.
    pub fn remove(&mut self, id: WindowId) -> Option<WindowRecord> {
        self.focus_history.retain(|&entry| entry != id);
        self.records.remove(&id)
    }

    pub fn get(&self, id: WindowId) -> Option<&WindowRecord> {
        self.records.get(&id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowRecord> {
        self.records.get_mut(&id)
    }

    /// Returns the total number of tracked toplevel windows.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Marks the given window as focused and moves it to the top of the MRU focus stack.
    pub fn mark_focused(&mut self, focused_id: Option<WindowId>) {
        for record in self.records.values_mut() {
            record.focused = Some(record.id) == focused_id;
        }
        if let Some(id) = focused_id
            && self.records.contains_key(&id)
        {
            self.focus_history.retain(|&entry| entry != id);
            self.focus_history.push(id);
        }
    }

    /// Returns the currently focused [`WindowId`], if any.
    pub fn focused_id(&self) -> Option<WindowId> {
        self.records
            .values()
            .find(|record| record.focused)
            .map(|record| record.id)
    }

    /// Returns the most-recently focused window ID still present in the registry.
    pub fn most_recent_id(&self) -> Option<WindowId> {
        self.focus_history
            .iter()
            .rev()
            .copied()
            .find(|id| self.records.contains_key(id))
    }

    /// Returns the next window ID for `Alt + Tab` focus cycling.
    pub fn next_cycle_target(&self) -> Option<WindowId> {
        if self.focus_history.len() < 2 {
            return self.most_recent_id();
        }
        // Focus history has the currently active window at the back; cycling picks the
        // least-recently focused or previous window cleanly.
        self.focus_history.iter().rev().nth(1).copied()
    }

    /// Computes a floating cascade placement inside the given output bounds.
    pub fn next_floating_placement(
        &mut self,
        output_size: Size<i32, Logical>,
    ) -> Point<i32, Logical> {
        const MARGIN_X: i32 = 48;
        const MARGIN_Y: i32 = 48;
        const STEP_X: i32 = 36;
        const STEP_Y: i32 = 32;
        const MAX_CASCADE_STEPS: i32 = 8;

        let step = self.cascade_index % MAX_CASCADE_STEPS;
        self.cascade_index = self.cascade_index.wrapping_add(1);

        let max_x = (output_size.w - 320).max(MARGIN_X);
        let max_y = (output_size.h - 240).max(MARGIN_Y);

        let x = (MARGIN_X + step * STEP_X).min(max_x);
        let y = (MARGIN_Y + step * STEP_Y).min(max_y);

        (x, y).into()
    }
}

/// Associates a stable [`WindowId`] with a Smithay [`Window`]'s `UserDataMap`.
pub fn attach_window_id(window: &Window, id: WindowId) {
    window.user_data().insert_if_missing(|| id);
}

/// Retrieves the [`WindowId`] attached to a Smithay [`Window`], if present.
pub fn window_id(window: &Window) -> Option<WindowId> {
    window.user_data().get::<WindowId>().copied()
}

/// Returns true if a Smithay [`Window`] matches the given `WlSurface` object ID.
pub fn window_matches_surface(window: &Window, surface: &WlSurface) -> bool {
    window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().id() == surface.id())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_placement_cascades_and_clamps_within_output() {
        let mut registry = WindowRegistry::new();
        let output: Size<i32, Logical> = (1280, 800).into();

        let p0 = registry.next_floating_placement(output);
        let p1 = registry.next_floating_placement(output);
        let p2 = registry.next_floating_placement(output);

        assert_eq!(p0, (48, 48).into());
        assert_eq!(p1, (84, 80).into());
        assert_eq!(p2, (120, 112).into());

        // Small output clamps safely without underflow or negative coordinates
        let tiny_output: Size<i32, Logical> = (200, 150).into();
        let clamped = registry.next_floating_placement(tiny_output);
        assert_eq!(clamped, (48, 48).into());
    }

    #[test]
    fn focus_history_and_removal_preserve_mru_order() {
        let mut registry = WindowRegistry::new();
        let w1 = registry.register((48, 48).into());
        let w2 = registry.register((84, 80).into());
        let w3 = registry.register((120, 112).into());

        registry.mark_focused(Some(w1));
        assert_eq!(registry.focused_id(), Some(w1));
        assert_eq!(registry.most_recent_id(), Some(w1));

        registry.mark_focused(Some(w3));
        assert_eq!(registry.focused_id(), Some(w3));
        assert_eq!(registry.next_cycle_target(), Some(w1));

        registry.remove(w3);
        assert_eq!(registry.most_recent_id(), Some(w1));
        assert_eq!(registry.len(), 2);

        registry.remove(w1);
        assert_eq!(registry.most_recent_id(), Some(w2));
        assert_eq!(registry.len(), 1);
    }
}
