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

/// Supported Windows 11-style smart window snapping regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapRegion {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftTwoThirds,
    RightOneThird,
    LeftThird,
    CenterThird,
    RightThird,
}

impl SnapRegion {
    /// Parses an IPC / CLI snap region string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "left" | "left_half" => Some(Self::LeftHalf),
            "right" | "right_half" => Some(Self::RightHalf),
            "top" | "top_half" => Some(Self::TopHalf),
            "bottom" | "bottom_half" => Some(Self::BottomHalf),
            "top_left" => Some(Self::TopLeft),
            "top_right" => Some(Self::TopRight),
            "bottom_left" => Some(Self::BottomLeft),
            "bottom_right" => Some(Self::BottomRight),
            "left_two_thirds" | "left_65" => Some(Self::LeftTwoThirds),
            "right_one_third" | "right_35" => Some(Self::RightOneThird),
            "left_third" => Some(Self::LeftThird),
            "center_third" | "middle_third" => Some(Self::CenterThird),
            "right_third" => Some(Self::RightThird),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LeftHalf => "left_half",
            Self::RightHalf => "right_half",
            Self::TopHalf => "top_half",
            Self::BottomHalf => "bottom_half",
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
            Self::LeftTwoThirds => "left_two_thirds",
            Self::RightOneThird => "right_one_third",
            Self::LeftThird => "left_third",
            Self::CenterThird => "center_third",
            Self::RightThird => "right_third",
        }
    }

    /// Computes the exact logical target rectangle inside `work_area` with optional `gap` pixels.
    pub fn target_rect(self, work_area: Rect, gap: i32) -> Rect {
        let gap = gap.max(0);
        let usable_w_2 = (work_area.size.w - gap * 3).max(160);
        let usable_h_2 = (work_area.size.h - gap * 3).max(100);
        let half_w = usable_w_2 / 2;
        let right_w = usable_w_2 - half_w;
        let half_h = usable_h_2 / 2;
        let bottom_h = usable_h_2 - half_h;

        let full_w = (work_area.size.w - gap * 2).max(160);
        let full_h = (work_area.size.h - gap * 2).max(100);
        let left_x = work_area.loc.x + gap;
        let right_x = left_x + half_w + gap;
        let top_y = work_area.loc.y + gap;
        let bottom_y = top_y + half_h + gap;

        // 65 / 35 asymmetric split (Windows 11 Snap Layout 2)
        let two_thirds_w = ((usable_w_2 as f32) * 0.65).round() as i32;
        let one_third_rem_w = (usable_w_2 - two_thirds_w).max(120);
        let asym_right_x = left_x + two_thirds_w + gap;

        // 3-column equal split (Windows 11 Snap Layout 3)
        let usable_w_3 = (work_area.size.w - gap * 4).max(240);
        let third_1 = usable_w_3 / 3;
        let third_2 = usable_w_3 / 3;
        let third_3 = usable_w_3 - third_1 - third_2;
        let col2_x = left_x + third_1 + gap;
        let col3_x = col2_x + third_2 + gap;

        match self {
            Self::LeftHalf => Rect::new(Point::new(left_x, top_y), Size::new(half_w, full_h)),
            Self::RightHalf => Rect::new(Point::new(right_x, top_y), Size::new(right_w, full_h)),
            Self::TopHalf => Rect::new(Point::new(left_x, top_y), Size::new(full_w, half_h)),
            Self::BottomHalf => Rect::new(Point::new(left_x, bottom_y), Size::new(full_w, bottom_h)),
            Self::TopLeft => Rect::new(Point::new(left_x, top_y), Size::new(half_w, half_h)),
            Self::TopRight => Rect::new(Point::new(right_x, top_y), Size::new(right_w, half_h)),
            Self::BottomLeft => {
                Rect::new(Point::new(left_x, bottom_y), Size::new(half_w, bottom_h))
            }
            Self::BottomRight => {
                Rect::new(Point::new(right_x, bottom_y), Size::new(right_w, bottom_h))
            }
            Self::LeftTwoThirds => {
                Rect::new(Point::new(left_x, top_y), Size::new(two_thirds_w, full_h))
            }
            Self::RightOneThird => Rect::new(
                Point::new(asym_right_x, top_y),
                Size::new(one_third_rem_w, full_h),
            ),
            Self::LeftThird => Rect::new(Point::new(left_x, top_y), Size::new(third_1, full_h)),
            Self::CenterThird => Rect::new(Point::new(col2_x, top_y), Size::new(third_2, full_h)),
            Self::RightThird => Rect::new(Point::new(col3_x, top_y), Size::new(third_3, full_h)),
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

        let l3 = SnapRegion::LeftThird.target_rect(area, 0);
        let c3 = SnapRegion::CenterThird.target_rect(area, 0);
        let r3 = SnapRegion::RightThird.target_rect(area, 0);
        assert_eq!(l3.size.w + c3.size.w + r3.size.w, 1280);
        assert_eq!(c3.loc.x, l3.size.w);
        assert_eq!(r3.loc.x, l3.size.w + c3.size.w);

        let l65 = SnapRegion::LeftTwoThirds.target_rect(area, 8);
        let r35 = SnapRegion::RightOneThird.target_rect(area, 8);
        assert_eq!(r35.loc.x, l65.loc.x + l65.size.w + 8);
        assert_eq!(r35.loc.x + r35.size.w + 8, 1280);
    }
}
