//! Window state machine preventing conflicting window states by construction.

use crate::geometry::SnapRegion;

/// Exclusive, non-conflicting window mode state machine.
///
/// By encoding modes as a tagged enum rather than independent boolean flags, impossible
/// combinations (such as simultaneously `Maximized + Fullscreen` or `Minimized + Fullscreen`)
/// are structurally unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WindowMode {
    /// Standard GNOME-inspired floating window.
    #[default]
    Floating,
    /// Automatically tiled by the Hyprland-style tiling layout engine (Dwindle / Master-Stack).
    Tiled,
    /// Snapped / tiled into an explicit Windows 11 snap region.
    Snapped(SnapRegion),
    /// Maximized to fill the output work area.
    Maximized,
    /// Fullscreen covering the entire output.
    Fullscreen,
    /// Minimized (hidden from Space), remembering the exact mode to restore to.
    Minimized { restore_mode: Box<WindowMode> },
}

impl WindowMode {
    pub fn is_floating(&self) -> bool {
        matches!(self, Self::Floating)
    }

    pub fn is_tiled(&self) -> bool {
        matches!(self, Self::Tiled)
    }

    pub fn is_maximized(&self) -> bool {
        matches!(self, Self::Maximized)
    }

    pub fn is_fullscreen(&self) -> bool {
        matches!(self, Self::Fullscreen)
    }

    pub fn is_minimized(&self) -> bool {
        matches!(self, Self::Minimized { .. })
    }

    pub fn snapped_region(&self) -> Option<SnapRegion> {
        match self {
            Self::Snapped(region) => Some(*region),
            _ => None,
        }
    }

    /// Transitions into `Floating`, returning `true` if the mode changed.
    pub fn set_floating(&mut self) -> bool {
        if matches!(self, Self::Floating) {
            return false;
        }
        *self = Self::Floating;
        true
    }

    /// Transitions into `Tiled`, returning `true` if the mode changed.
    pub fn set_tiled(&mut self) -> bool {
        if matches!(self, Self::Tiled) {
            return false;
        }
        *self = Self::Tiled;
        true
    }

    /// Explicitly snaps into `region` (does not toggle back to Floating if already there).
    pub fn set_snapped(&mut self, region: SnapRegion) -> bool {
        if matches!(self, Self::Snapped(cur) if *cur == region) {
            return false;
        }
        *self = Self::Snapped(region);
        true
    }

    /// Transitions into `Maximized` (if `maximize == true`) or back to `Floating`.
    pub fn set_maximized(&mut self, maximize: bool) -> bool {
        if maximize {
            if matches!(self, Self::Maximized) {
                return false;
            }
            *self = Self::Maximized;
            true
        } else if matches!(self, Self::Maximized) {
            *self = Self::Floating;
            true
        } else {
            false
        }
    }

    /// Toggles between `Maximized` and `Floating`.
    pub fn toggle_maximized(&mut self) -> bool {
        let next = !self.is_maximized();
        self.set_maximized(next)
    }

    /// Transitions into `Fullscreen` (if `fullscreen == true`) or restores `Floating`.
    pub fn set_fullscreen(&mut self, fullscreen: bool) -> bool {
        if fullscreen {
            if matches!(self, Self::Fullscreen) {
                return false;
            }
            *self = Self::Fullscreen;
            true
        } else if matches!(self, Self::Fullscreen) {
            *self = Self::Floating;
            true
        } else {
            false
        }
    }

    /// Toggles between `Fullscreen` and `Floating`.
    pub fn toggle_fullscreen(&mut self) -> bool {
        let next = !self.is_fullscreen();
        self.set_fullscreen(next)
    }

    /// Snaps into `region`, or returns to `Floating` if already snapped to that same region.
    pub fn toggle_snap(&mut self, region: SnapRegion) -> bool {
        if matches!(self, Self::Snapped(current) if *current == region) {
            *self = Self::Floating;
        } else {
            *self = Self::Snapped(region);
        }
        true
    }

    /// Minimizes the window, preserving its current non-minimized mode for clean restoration.
    pub fn minimize(&mut self) -> bool {
        if self.is_minimized() {
            return false;
        }
        let previous = std::mem::take(self);
        *self = Self::Minimized {
            restore_mode: Box::new(previous),
        };
        true
    }

    /// Restores a minimized window back to its exact prior mode.
    pub fn restore(&mut self) -> bool {
        match std::mem::take(self) {
            Self::Minimized { restore_mode } => {
                *self = *restore_mode;
                true
            }
            other => {
                *self = other;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_prevents_conflicting_states_and_restores_cleanly() {
        let mut mode = WindowMode::Floating;
        assert!(mode.set_maximized(true));
        assert!(mode.is_maximized());
        assert!(!mode.is_fullscreen());

        // Minimizing a maximized window remembers Maximized
        assert!(mode.minimize());
        assert!(mode.is_minimized());
        assert!(!mode.is_maximized());

        // Double minimize is a no-op
        assert!(!mode.minimize());

        // Restoring returns directly to Maximized
        assert!(mode.restore());
        assert!(mode.is_maximized());

        // Snapping toggles cleanly
        assert!(mode.toggle_snap(SnapRegion::LeftHalf));
        assert_eq!(mode.snapped_region(), Some(SnapRegion::LeftHalf));
        assert!(mode.toggle_snap(SnapRegion::LeftHalf));
        assert!(mode.is_floating());
    }
}
