//! Logical geometry helpers and smart window snapping calculations.

use serde::{Deserialize, Serialize};

/// 2D point in logical desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// 2D size in logical desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

impl Size {
    pub const fn new(w: i32, h: i32) -> Self {
        Self { w, h }
    }

    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// Axis-aligned rectangle in logical desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub loc: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(loc: Point, size: Size) -> Self {
        Self { loc, size }
    }

    pub const fn from_size(size: Size) -> Self {
        Self {
            loc: Point::new(0, 0),
            size,
        }
    }

    /// Returns the geometric top-left origin that centers a rectangle of `child_size` on `self`.
    ///
    /// Does not clamp inside `self` so centering a larger window over a smaller anchor rect
    /// remains truly centered; callers should clamp the result against the output `work_area`.
    pub fn center_size(self, child_size: Size) -> Point {
        let cx = self.loc.x + (self.size.w - child_size.w) / 2;
        let cy = self.loc.y + (self.size.h - child_size.h) / 2;
        Point::new(cx, cy)
    }

    /// Clamps a window's origin so that it stays reasonably visible inside `self`.
    pub fn clamp_origin(self, origin: Point, child_size: Size) -> Point {
        let max_x = (self.loc.x + self.size.w - child_size.w.max(160)).max(self.loc.x);
        let max_y = (self.loc.y + self.size.h - child_size.h.max(100)).max(self.loc.y);
        Point::new(
            origin.x.clamp(self.loc.x, max_x),
            origin.y.clamp(self.loc.y, max_y),
        )
    }
}

/// Supported smart window snapping / optional tiling regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapRegion {
    LeftHalf,
    RightHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl SnapRegion {
    /// Computes the exact logical target rectangle inside `work_area` with optional `gap` pixels.
    pub fn target_rect(self, work_area: Rect, gap: i32) -> Rect {
        let gap = gap.max(0);
        let usable_w = (work_area.size.w - gap * 3).max(160);
        let usable_h = (work_area.size.h - gap * 3).max(100);
        let half_w = usable_w / 2;
        let right_w = usable_w - half_w;
        let half_h = usable_h / 2;
        let bottom_h = usable_h - half_h;

        let full_h = (work_area.size.h - gap * 2).max(100);
        let left_x = work_area.loc.x + gap;
        let right_x = left_x + half_w + gap;
        let top_y = work_area.loc.y + gap;
        let bottom_y = top_y + half_h + gap;

        match self {
            Self::LeftHalf => Rect::new(Point::new(left_x, top_y), Size::new(half_w, full_h)),
            Self::RightHalf => Rect::new(Point::new(right_x, top_y), Size::new(right_w, full_h)),
            Self::TopLeft => Rect::new(Point::new(left_x, top_y), Size::new(half_w, half_h)),
            Self::TopRight => Rect::new(Point::new(right_x, top_y), Size::new(right_w, half_h)),
            Self::BottomLeft => {
                Rect::new(Point::new(left_x, bottom_y), Size::new(half_w, bottom_h))
            }
            Self::BottomRight => {
                Rect::new(Point::new(right_x, bottom_y), Size::new(right_w, bottom_h))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_regions_partition_work_area_cleanly() {
        let area = Rect::from_size(Size::new(1280, 800));
        let left = SnapRegion::LeftHalf.target_rect(area, 0);
        let right = SnapRegion::RightHalf.target_rect(area, 0);

        assert_eq!(left, Rect::new(Point::new(0, 0), Size::new(640, 800)));
        assert_eq!(right, Rect::new(Point::new(640, 0), Size::new(640, 800)));

        let tl = SnapRegion::TopLeft.target_rect(area, 0);
        let br = SnapRegion::BottomRight.target_rect(area, 0);
        assert_eq!(tl, Rect::new(Point::new(0, 0), Size::new(640, 400)));
        assert_eq!(br, Rect::new(Point::new(640, 400), Size::new(640, 400)));
    }
}
