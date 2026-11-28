//! Hyprland-inspired dynamic window tiling algorithms (**Dwindle BSP spiral** and **Master–Stack**).
//!
//! Pure geometry calculation with pixel-exact integer rounding, configurable outer/inner gaps,
//! per-split customizable ratios, per-split orientation flips (`Super+J`), interactive divider
//! hit-targets for mouse resizing, and zero overlap so `neon-window-manager` and `neon-compositor`
//! can reflow workspaces smoothly whenever windows open, close, swap, resize, float, or move.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    WindowId,
    geometry::{Point, Rect, Size},
};

/// Active automatic tiling layout algorithm when Hyprland Auto-Tiling is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TilingLayout {
    /// Hyprland default binary-space-partitioning spiral: splits remaining space cleanly with
    /// aspect-aware alternation so 16:9 displays never collapse into narrow strips.
    #[default]
    Dwindle,
    /// One primary master pane on the left (~56%, resizable) with secondary windows stacked
    /// vertically on the right (each divider resizable).
    MasterStack,
}

impl TilingLayout {
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "master" | "master_stack" | "master-stack" => Self::MasterStack,
            _ => Self::Dwindle,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dwindle => "dwindle",
            Self::MasterStack => "master_stack",
        }
    }
}

/// Orientation of a split divider between two adjacent tiled panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitAxis {
    /// Vertical line dividing a Left pane and a Right pane (drag left/right to resize width).
    VerticalDivider,
    /// Horizontal line dividing a Top pane and a Bottom pane (drag up/down to resize height).
    HorizontalDivider,
}

/// Metadata describing an interactive split divider between tiled sibling panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilingDivider {
    /// Split index inside the active workspace layout (`0` = primary split).
    pub split_index: usize,
    pub axis: SplitAxis,
    /// Full container rectangle being partitioned by this split (used to map pointer → ratio).
    pub container_rect: Rect,
    /// Logical gap strip directly between the two sibling panes.
    pub divider_rect: Rect,
    /// First window on the leading (left / top) side of this split.
    pub leading_window: WindowId,
    /// First window on the trailing (right / bottom) side of this split.
    pub trailing_window: WindowId,
    /// Current split ratio clamped to `[0.20, 0.80]`, stored in basis points (`2000..=8000`) for `Eq`.
    pub ratio_bps: i32,
}

impl TilingDivider {
    /// Computes the exact split ratio `[0.20, 0.80]` corresponding to pointer `(px, py)` inside `container_rect`.
    pub fn ratio_for_point(&self, px: f64, py: f64) -> f32 {
        let raw = match self.axis {
            SplitAxis::VerticalDivider => {
                let w = self.container_rect.size.w.max(1) as f64;
                (px - self.container_rect.loc.x as f64) / w
            }
            SplitAxis::HorizontalDivider => {
                let h = self.container_rect.size.h.max(1) as f64;
                (py - self.container_rect.loc.y as f64) / h
            }
        };
        (raw as f32).clamp(0.20, 0.80)
    }

    /// Returns `true` when pointer `(px, py)` sits within `hit_pad` pixels of this divider line.
    pub fn hit_test(&self, px: i32, py: i32, hit_pad: i32) -> bool {
        let pad = hit_pad.max(3);
        let r = self.divider_rect;
        match self.axis {
            SplitAxis::VerticalDivider => {
                px >= r.loc.x - pad
                    && px <= r.loc.x + r.size.w + pad
                    && py >= r.loc.y
                    && py <= r.loc.y + r.size.h
            }
            SplitAxis::HorizontalDivider => {
                py >= r.loc.y - pad
                    && py <= r.loc.y + r.size.h + pad
                    && px >= r.loc.x
                    && px <= r.loc.x + r.size.w
            }
        }
    }
}

/// Complete layout calculation output: exact per-window tile rects + interactive split dividers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TilingLayoutResult {
    pub rects: Vec<(WindowId, Rect)>,
    pub dividers: Vec<TilingDivider>,
}

/// Computes exact logical target rectangles for `window_ids` inside `work_area` using default ratios.
pub fn compute_tiling_rects(
    layout: TilingLayout,
    window_ids: &[WindowId],
    work_area: Rect,
    gap: i32,
) -> Vec<(WindowId, Rect)> {
    let empty_ratios = HashMap::new();
    let empty_flips = HashMap::new();
    compute_tiling_layout(layout, window_ids, work_area, gap, &empty_ratios, &empty_flips).rects
}

/// Computes exact logical target rectangles AND interactive split dividers with per-split ratios
/// and orientation flips.
pub fn compute_tiling_layout(
    layout: TilingLayout,
    window_ids: &[WindowId],
    work_area: Rect,
    gap: i32,
    split_ratios: &HashMap<usize, f32>,
    split_flips: &HashMap<usize, bool>,
) -> TilingLayoutResult {
    if window_ids.is_empty() || work_area.size.is_empty() {
        return TilingLayoutResult::default();
    }
    let gap = gap.clamp(0, 48);
    let inner = Rect::new(
        Point::new(work_area.loc.x + gap, work_area.loc.y + gap),
        Size::new(
            (work_area.size.w - gap * 2).max(160),
            (work_area.size.h - gap * 2).max(120),
        ),
    );

    if window_ids.len() == 1 {
        return TilingLayoutResult {
            rects: vec![(window_ids[0], inner)],
            dividers: Vec::new(),
        };
    }

    match layout {
        TilingLayout::Dwindle => {
            compute_dwindle(window_ids, inner, gap, split_ratios, split_flips)
        }
        TilingLayout::MasterStack => {
            compute_master_stack(window_ids, inner, gap, split_ratios, split_flips)
        }
    }
}

/// Hyprland Dwindle BSP spiral with aspect-aware alternation, customizable per-split ratios,
/// and per-split orientation flips (`Super+J`).
fn compute_dwindle(
    window_ids: &[WindowId],
    initial: Rect,
    gap: i32,
    split_ratios: &HashMap<usize, f32>,
    split_flips: &HashMap<usize, bool>,
) -> TilingLayoutResult {
    let mut rects = Vec::with_capacity(window_ids.len());
    let mut dividers = Vec::with_capacity(window_ids.len().saturating_sub(1));
    let mut rem = initial;
    let last_idx = window_ids.len() - 1;
    let mut prev_vertical_divider: Option<bool> = None;

    for (idx, &id) in window_ids.iter().enumerate() {
        if idx == last_idx {
            rects.push((id, rem));
            break;
        }

        // Aspect-aware smart split: strongly landscape splits left/right, strongly portrait splits
        // top/bottom, and near-square panes (like the right half of a 1920x960 work area!)
        // cleanly alternate from the previous split so 3 windows never become narrow columns.
        let mut vertical_divider = if (rem.size.w as f32) > (rem.size.h as f32) * 1.25 {
            true
        } else if (rem.size.h as f32) > (rem.size.w as f32) * 1.25 {
            false
        } else {
            match prev_vertical_divider {
                Some(prev) => !prev,
                None => rem.size.w >= rem.size.h,
            }
        };
        if split_flips.get(&idx).copied().unwrap_or(false) {
            vertical_divider = !vertical_divider;
        }
        prev_vertical_divider = Some(vertical_divider);

        let ratio = split_ratios
            .get(&idx)
            .copied()
            .unwrap_or(0.50)
            .clamp(0.20, 0.80);
        let next_id = window_ids[idx + 1];

        if vertical_divider {
            let usable_w = (rem.size.w - gap).max(160);
            let min_pane = 80.min(usable_w / 2);
            let left_w = if (ratio - 0.50).abs() < 1e-4 {
                usable_w / 2
            } else {
                ((usable_w as f32) * ratio)
                    .round()
                    .clamp(min_pane as f32, (usable_w - min_pane) as f32) as i32
            };
            let right_w = (usable_w - left_w).max(min_pane);
            let tile = Rect::new(rem.loc, Size::new(left_w, rem.size.h));
            rects.push((id, tile));
            dividers.push(TilingDivider {
                split_index: idx,
                axis: SplitAxis::VerticalDivider,
                container_rect: rem,
                divider_rect: Rect::new(
                    Point::new(rem.loc.x + left_w, rem.loc.y),
                    Size::new(gap.max(1), rem.size.h),
                ),
                leading_window: id,
                trailing_window: next_id,
                ratio_bps: (ratio * 10_000.0).round() as i32,
            });
            rem = Rect::new(
                Point::new(rem.loc.x + left_w + gap, rem.loc.y),
                Size::new(right_w, rem.size.h),
            );
        } else {
            let usable_h = (rem.size.h - gap).max(120);
            let min_pane = 60.min(usable_h / 2);
            let top_h = if (ratio - 0.50).abs() < 1e-4 {
                usable_h / 2
            } else {
                ((usable_h as f32) * ratio)
                    .round()
                    .clamp(min_pane as f32, (usable_h - min_pane) as f32) as i32
            };
            let bot_h = (usable_h - top_h).max(min_pane);
            let tile = Rect::new(rem.loc, Size::new(rem.size.w, top_h));
            rects.push((id, tile));
            dividers.push(TilingDivider {
                split_index: idx,
                axis: SplitAxis::HorizontalDivider,
                container_rect: rem,
                divider_rect: Rect::new(
                    Point::new(rem.loc.x, rem.loc.y + top_h),
                    Size::new(rem.size.w, gap.max(1)),
                ),
                leading_window: id,
                trailing_window: next_id,
                ratio_bps: (ratio * 10_000.0).round() as i32,
            });
            rem = Rect::new(
                Point::new(rem.loc.x, rem.loc.y + top_h + gap),
                Size::new(rem.size.w, bot_h),
            );
        }
    }

    TilingLayoutResult { rects, dividers }
}

/// Master–Stack layout: `window_ids[0]` occupies the primary master pane (`split_ratios[0]`,
/// default `0.56`, flipped right when `split_flips[0]` is true), and `window_ids[1..]` divide
/// the stack column vertically with customizable per-divider ratios (`split_ratios[1..]`).
fn compute_master_stack(
    window_ids: &[WindowId],
    inner: Rect,
    gap: i32,
    split_ratios: &HashMap<usize, f32>,
    split_flips: &HashMap<usize, bool>,
) -> TilingLayoutResult {
    let mut rects = Vec::with_capacity(window_ids.len());
    let mut dividers = Vec::with_capacity(window_ids.len().saturating_sub(1));

    let master_ratio = split_ratios
        .get(&0)
        .copied()
        .unwrap_or(0.56)
        .clamp(0.25, 0.78);
    let master_on_right = split_flips.get(&0).copied().unwrap_or(false);

    let usable_w = (inner.size.w - gap).max(200);
    let master_w = ((usable_w as f32) * master_ratio)
        .round()
        .clamp(120.0, (usable_w - 120).max(120) as f32) as i32;
    let stack_w = (usable_w - master_w).max(120);

    let (master_x, stack_x, divider_x) = if master_on_right {
        (
            inner.loc.x + stack_w + gap,
            inner.loc.x,
            inner.loc.x + stack_w,
        )
    } else {
        (
            inner.loc.x,
            inner.loc.x + master_w + gap,
            inner.loc.x + master_w,
        )
    };

    rects.push((
        window_ids[0],
        Rect::new(Point::new(master_x, inner.loc.y), Size::new(master_w, inner.size.h)),
    ));

    let stack = &window_ids[1..];
    dividers.push(TilingDivider {
        split_index: 0,
        axis: SplitAxis::VerticalDivider,
        container_rect: inner,
        divider_rect: Rect::new(
            Point::new(divider_x, inner.loc.y),
            Size::new(gap.max(1), inner.size.h),
        ),
        leading_window: if master_on_right {
            stack[0]
        } else {
            window_ids[0]
        },
        trailing_window: if master_on_right {
            window_ids[0]
        } else {
            stack[0]
        },
        ratio_bps: (master_ratio * 10_000.0).round() as i32,
    });

    let n = stack.len();
    let has_custom_stack = (0..n.saturating_sub(1)).any(|i| split_ratios.contains_key(&(1 + i)));
    if !has_custom_stack {
        let n_i32 = n as i32;
        let total_gaps = gap * (n_i32 - 1);
        let usable_h = (inner.size.h - total_gaps).max(n_i32 * 60);
        let base_h = usable_h / n_i32;
        let extra = usable_h - base_h * n_i32;
        let mut cur_y = inner.loc.y;

        for (i, &id) in stack.iter().enumerate() {
            let h = base_h + if (i as i32) < extra { 1 } else { 0 };
            rects.push((id, Rect::new(Point::new(stack_x, cur_y), Size::new(stack_w, h))));
            if i + 1 < n {
                let rem_h = (inner.loc.y + inner.size.h - cur_y).max(120);
                let default_r = 1.0 / (n - i) as f32;
                dividers.push(TilingDivider {
                    split_index: 1 + i,
                    axis: SplitAxis::HorizontalDivider,
                    container_rect: Rect::new(Point::new(stack_x, cur_y), Size::new(stack_w, rem_h)),
                    divider_rect: Rect::new(
                        Point::new(stack_x, cur_y + h),
                        Size::new(stack_w, gap.max(1)),
                    ),
                    leading_window: id,
                    trailing_window: stack[i + 1],
                    ratio_bps: (default_r * 10_000.0).round() as i32,
                });
            }
            cur_y += h + gap;
        }
    } else {
        let mut rem_y = inner.loc.y;
        let mut rem_h = inner.size.h;
        for (i, &id) in stack.iter().enumerate() {
            if i + 1 == n {
                rects.push((
                    id,
                    Rect::new(Point::new(stack_x, rem_y), Size::new(stack_w, rem_h.max(60))),
                ));
                break;
            }
            let default_r = 1.0 / (n - i) as f32;
            let ratio = split_ratios
                .get(&(1 + i))
                .copied()
                .unwrap_or(default_r)
                .clamp(0.18, 0.82);
            let remaining_after = (n - i - 1) as i32;
            let usable_h = (rem_h - gap).max(60 * (remaining_after + 1));
            let min_this = 60.min(usable_h / 2);
            let max_this = (usable_h - 60 * remaining_after).max(min_this);
            let h = ((usable_h as f32) * ratio)
                .round()
                .clamp(min_this as f32, max_this as f32) as i32;
            rects.push((id, Rect::new(Point::new(stack_x, rem_y), Size::new(stack_w, h))));
            dividers.push(TilingDivider {
                split_index: 1 + i,
                axis: SplitAxis::HorizontalDivider,
                container_rect: Rect::new(Point::new(stack_x, rem_y), Size::new(stack_w, rem_h)),
                divider_rect: Rect::new(
                    Point::new(stack_x, rem_y + h),
                    Size::new(stack_w, gap.max(1)),
                ),
                leading_window: id,
                trailing_window: stack[i + 1],
                ratio_bps: (ratio * 10_000.0).round() as i32,
            });
            rem_y += h + gap;
            rem_h = (rem_h - h - gap).max(60);
        }
    }

    TilingLayoutResult { rects, dividers }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwindle_tiles_one_two_three_and_four_windows_cleanly_with_gaps() {
        let area = Rect::new(Point::new(0, 30), Size::new(1280, 720));
        let w1 = WindowId(1);
        let w2 = WindowId(2);
        let w3 = WindowId(3);
        let w4 = WindowId(4);

        let one = compute_tiling_rects(TilingLayout::Dwindle, &[w1], area, 10);
        assert_eq!(one, vec![(w1, Rect::new(Point::new(10, 40), Size::new(1260, 700)))]);

        let two = compute_tiling_rects(TilingLayout::Dwindle, &[w1, w2], area, 10);
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].1, Rect::new(Point::new(10, 40), Size::new(625, 700)));
        assert_eq!(two[1].1, Rect::new(Point::new(645, 40), Size::new(625, 700)));

        // Third window splits the right pane vertically (Top-Right / Bottom-Right)
        let three = compute_tiling_rects(TilingLayout::Dwindle, &[w1, w2, w3], area, 10);
        assert_eq!(three[0].1, Rect::new(Point::new(10, 40), Size::new(625, 700)));
        assert_eq!(three[1].1, Rect::new(Point::new(645, 40), Size::new(625, 345)));
        assert_eq!(three[2].1, Rect::new(Point::new(645, 395), Size::new(625, 345)));

        // Fourth window splits bottom-right horizontally (625x345 -> w > h*1.25!)
        let four = compute_tiling_rects(TilingLayout::Dwindle, &[w1, w2, w3, w4], area, 10);
        assert_eq!(four[2].1.loc.y, 395);
        assert_eq!(four[3].1.loc.y, 395);
        assert_eq!(four[2].1.size.w + 10 + four[3].1.size.w, 625);
    }

    #[test]
    fn dwindle_alternates_cleanly_on_1080p_with_panel_and_dock() {
        // 1920x960 work area: after first left/right split, right pane is ~951x948 (w > h by 3px!).
        // Smart aspect alternation must still split Top-Right / Bottom-Right instead of narrow strips.
        let area = Rect::new(Point::new(0, 30), Size::new(1920, 960));
        let ids = [WindowId(1), WindowId(2), WindowId(3)];
        let res = compute_tiling_layout(
            TilingLayout::Dwindle,
            &ids,
            area,
            6,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(res.rects.len(), 3);
        assert_eq!(res.dividers.len(), 2);
        assert_eq!(res.dividers[0].axis, SplitAxis::VerticalDivider);
        assert_eq!(res.dividers[1].axis, SplitAxis::HorizontalDivider);
        // Right two tiles share identical x & width and stack cleanly with a 6px gap!
        assert_eq!(res.rects[1].1.loc.x, res.rects[2].1.loc.x);
        assert_eq!(res.rects[1].1.size.w, res.rects[2].1.size.w);
        assert_eq!(
            res.rects[1].1.loc.y + res.rects[1].1.size.h + 6,
            res.rects[2].1.loc.y
        );
    }

    #[test]
    fn custom_split_ratios_and_flips_work_for_dwindle_and_master_stack() {
        let area = Rect::new(Point::new(0, 0), Size::new(1920, 1032));
        let ids = [WindowId(1), WindowId(2)];

        let mut ratios = HashMap::new();
        ratios.insert(0, 0.65);
        let flips = HashMap::new();

        let res = compute_tiling_layout(TilingLayout::Dwindle, &ids, area, 6, &ratios, &flips);
        assert!(res.rects[0].1.size.w > res.rects[1].1.size.w + 400);
        assert_eq!(
            res.rects[0].1.loc.x + res.rects[0].1.size.w + 6,
            res.rects[1].1.loc.x
        );

        let mut flips_on = HashMap::new();
        flips_on.insert(0, true);
        let flipped =
            compute_tiling_layout(TilingLayout::Dwindle, &ids, area, 6, &HashMap::new(), &flips_on);
        // Flipped from left/right into top/bottom!
        assert_eq!(flipped.dividers[0].axis, SplitAxis::HorizontalDivider);
        assert_eq!(flipped.rects[0].1.loc.x, flipped.rects[1].1.loc.x);
        assert!(flipped.rects[1].1.loc.y > flipped.rects[0].1.loc.y);
    }

    #[test]
    fn master_stack_allocates_primary_left_and_equal_right_stack() {
        let area = Rect::new(Point::new(0, 0), Size::new(1280, 800));
        let ids = [WindowId(1), WindowId(2), WindowId(3), WindowId(4)];
        let rects = compute_tiling_rects(TilingLayout::MasterStack, &ids, area, 8);
        assert_eq!(rects.len(), 4);
        assert_eq!(rects[0].1.loc, Point::new(8, 8));
        assert_eq!(rects[0].1.size.h, 784);
        // Right stack windows sit flush to right margin without gaps or overlap
        let stack_x = rects[0].1.loc.x + rects[0].1.size.w + 8;
        assert_eq!(rects[1].1.loc.x, stack_x);
        assert_eq!(rects[2].1.loc.x, stack_x);
        assert_eq!(rects[3].1.loc.x, stack_x);
        assert_eq!(rects[3].1.loc.y + rects[3].1.size.h + 8, 800);
        assert_eq!(rects[1].1.loc.x + rects[1].1.size.w + 8, 1280);
    }
}
