//! Windows 11-style drag-to-edge snapping, top-center Snap Layouts drop bar,
//! and Hyprland Auto-Tiling drag-to-swap hit-testing + animated GPU overlay geometry.

use std::time::Instant;

use neon_window_manager::{
    WindowId,
    geometry::{Point, Rect, Size, SnapRegion},
};

/// Active target selected during an interactive window drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragSnapTarget {
    /// Snap dragged window to a Windows 11 region (half, quarter, 65/35, thirds).
    Region(SnapRegion),
    /// Maximize dragged window to fill `work_area`.
    Maximize,
    /// Swap tile slot with another tiled window while Hyprland Auto-Tiling is active.
    SwapTile { target_id: WindowId, tile_rect: Rect },
}

/// One clickable/droppable zone inside a Snap Layouts card.
#[derive(Debug, Clone, Copy)]
pub struct SnapLayoutZone {
    pub region: SnapRegion,
    /// Normalized `(x, y, w, h)` in `[0.0, 1.0]` inside the mini card preview.
    pub norm_rect: (f32, f32, f32, f32),
}

/// One card inside the Windows 11 top-center Snap Layouts drop bar.
#[derive(Debug, Clone, Copy)]
pub struct SnapLayoutCard {
    pub zones: &'static [SnapLayoutZone],
}

/// Five iconic Windows 11 Snap Layouts displayed in the top-center drag bar:
/// 1. `50 | 50` (Left / Right)
/// 2. `65 | 35` (Wide Left / Narrow Right)
/// 3. `33 | 33 | 33` (Three Columns)
/// 4. `50 | 25/25` (Left Half + Right Stacked Quarters)
/// 5. `2x2` (Four Corner Quarters)
pub const SNAP_LAYOUT_CARDS: &[SnapLayoutCard] = &[
    SnapLayoutCard {
        zones: &[
            SnapLayoutZone {
                region: SnapRegion::LeftHalf,
                norm_rect: (0.0, 0.0, 0.48, 1.0),
            },
            SnapLayoutZone {
                region: SnapRegion::RightHalf,
                norm_rect: (0.52, 0.0, 0.48, 1.0),
            },
        ],
    },
    SnapLayoutCard {
        zones: &[
            SnapLayoutZone {
                region: SnapRegion::LeftTwoThirds,
                norm_rect: (0.0, 0.0, 0.63, 1.0),
            },
            SnapLayoutZone {
                region: SnapRegion::RightOneThird,
                norm_rect: (0.67, 0.0, 0.33, 1.0),
            },
        ],
    },
    SnapLayoutCard {
        zones: &[
            SnapLayoutZone {
                region: SnapRegion::LeftThird,
                norm_rect: (0.0, 0.0, 0.305, 1.0),
            },
            SnapLayoutZone {
                region: SnapRegion::CenterThird,
                norm_rect: (0.347, 0.0, 0.306, 1.0),
            },
            SnapLayoutZone {
                region: SnapRegion::RightThird,
                norm_rect: (0.695, 0.0, 0.305, 1.0),
            },
        ],
    },
    SnapLayoutCard {
        zones: &[
            SnapLayoutZone {
                region: SnapRegion::LeftHalf,
                norm_rect: (0.0, 0.0, 0.48, 1.0),
            },
            SnapLayoutZone {
                region: SnapRegion::TopRight,
                norm_rect: (0.52, 0.0, 0.48, 0.47),
            },
            SnapLayoutZone {
                region: SnapRegion::BottomRight,
                norm_rect: (0.52, 0.53, 0.48, 0.47),
            },
        ],
    },
    SnapLayoutCard {
        zones: &[
            SnapLayoutZone {
                region: SnapRegion::TopLeft,
                norm_rect: (0.0, 0.0, 0.48, 0.47),
            },
            SnapLayoutZone {
                region: SnapRegion::TopRight,
                norm_rect: (0.52, 0.0, 0.48, 0.47),
            },
            SnapLayoutZone {
                region: SnapRegion::BottomLeft,
                norm_rect: (0.0, 0.53, 0.48, 0.47),
            },
            SnapLayoutZone {
                region: SnapRegion::BottomRight,
                norm_rect: (0.52, 0.53, 0.48, 0.47),
            },
        ],
    },
];

/// Computed layout geometry of the top-center Windows 11 Snap Layouts bar.
#[derive(Debug, Clone)]
pub struct SnapBarGeometry {
    pub bar_rect: Rect,
    pub cards: Vec<Rect>,
    /// `(card_idx, zone_idx, region, pixel_rect)` for every interactive mini-zone.
    pub zones: Vec<(usize, usize, SnapRegion, Rect)>,
}

/// Computes the exact pixel layout of the top-center Snap Layouts drop bar inside `work_area`.
pub fn compute_snap_bar_geometry(work_area: Rect, slide_progress: f32) -> SnapBarGeometry {
    let card_w = 78;
    let card_h = 46;
    let card_gap = 10;
    let pad_x = 14;
    let pad_y = 10;
    let n = SNAP_LAYOUT_CARDS.len() as i32;

    let bar_w = pad_x * 2 + n * card_w + (n - 1) * card_gap;
    let bar_h = pad_y * 2 + card_h;
    let bar_x = work_area.loc.x + (work_area.size.w - bar_w) / 2;
    let target_y = work_area.loc.y + 8;
    let hidden_y = work_area.loc.y - bar_h - 12;
    let p = slide_progress.clamp(0.0, 1.0);
    let bar_y = (hidden_y as f32 + (target_y - hidden_y) as f32 * p).round() as i32;

    let bar_rect = Rect::new(Point::new(bar_x, bar_y), Size::new(bar_w, bar_h));
    let mut cards = Vec::with_capacity(SNAP_LAYOUT_CARDS.len());
    let mut zones = Vec::with_capacity(16);

    for (ci, card) in SNAP_LAYOUT_CARDS.iter().enumerate() {
        let cx = bar_x + pad_x + (ci as i32) * (card_w + card_gap);
        let cy = bar_y + pad_y;
        let crect = Rect::new(Point::new(cx, cy), Size::new(card_w, card_h));
        cards.push(crect);

        let inner_pad = 4;
        let ix = cx + inner_pad;
        let iy = cy + inner_pad;
        let iw = (card_w - inner_pad * 2) as f32;
        let ih = (card_h - inner_pad * 2) as f32;
        for (zi, z) in card.zones.iter().enumerate() {
            let (nx, ny, nw, nh) = z.norm_rect;
            let zx = ix + (nx * iw).round() as i32;
            let zy = iy + (ny * ih).round() as i32;
            let zw = (nw * iw).round().max(6.0) as i32;
            let zh = (nh * ih).round().max(6.0) as i32;
            zones.push((ci, zi, z.region, Rect::new(Point::new(zx, zy), Size::new(zw, zh))));
        }
    }

    SnapBarGeometry {
        bar_rect,
        cards,
        zones,
    }
}

/// Smoothly interpolated logical rectangle for Material 3 Expressive snap preview morphing.
#[derive(Debug, Clone)]
struct AnimatedPreviewRect {
    from: Rect,
    to: Rect,
    started: Instant,
    duration_ms: f32,
}

impl AnimatedPreviewRect {
    fn new(from: Rect, to: Rect, duration_ms: f32) -> Self {
        Self {
            from,
            to,
            started: Instant::now(),
            duration_ms: duration_ms.max(1.0),
        }
    }

    fn current(&self) -> (Rect, bool) {
        let t = (self.started.elapsed().as_secs_f32() * 1000.0 / self.duration_ms).clamp(0.0, 1.0);
        // Smooth cubic ease-out (crisp M3 spatial glide without clipping outside work_area)
        let inv = 1.0 - t;
        let e = 1.0 - inv * inv * inv;
        let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * e).round() as i32;
        let rect = Rect::new(
            Point::new(lerp(self.from.loc.x, self.to.loc.x), lerp(self.from.loc.y, self.to.loc.y)),
            Size::new(
                lerp(self.from.size.w, self.to.size.w).max(8),
                lerp(self.from.size.h, self.to.size.h).max(8),
            ),
        );
        (rect, t < 1.0)
    }
}

/// Live drag-to-snap & Snap Layouts bar state tracked inside [`NeonState`].
#[derive(Debug, Default)]
pub struct DragSnapOverlayState {
    pub dragged_window: Option<WindowId>,
    pub target: Option<DragSnapTarget>,
    pub bar_open: bool,
    pub bar_opened_at: Option<Instant>,
    pub hovered_bar_zone: Option<(usize, usize)>,
    preview_anim: Option<AnimatedPreviewRect>,
    preview_fade_started: Option<Instant>,
}

impl DragSnapOverlayState {
    pub fn clear(&mut self) {
        self.dragged_window = None;
        self.target = None;
        self.bar_open = false;
        self.bar_opened_at = None;
        self.hovered_bar_zone = None;
        self.preview_anim = None;
        self.preview_fade_started = None;
    }

    pub fn is_active(&self) -> bool {
        self.target.is_some() || self.bar_open
    }

    /// Updates drag snap / Snap Layouts bar state for a floating window drag at pointer `(px, py)`.
    pub fn update_floating_drag(
        &mut self,
        dragged_id: WindowId,
        px: f64,
        py: f64,
        work_area: Rect,
        snap_gap: i32,
        enable_snap: bool,
        enable_layouts_bar: bool,
        animations_enabled: bool,
    ) {
        self.dragged_window = Some(dragged_id);
        if !enable_snap {
            self.target = None;
            self.bar_open = false;
            self.hovered_bar_zone = None;
            self.preview_anim = None;
            return;
        }

        let x = px.round() as i32;
        let y = py.round() as i32;
        let left = work_area.loc.x;
        let right = work_area.loc.x + work_area.size.w;
        let top = work_area.loc.y;
        let bottom = work_area.loc.y + work_area.size.h;
        let center_x = left + work_area.size.w / 2;

        // 1. Top-center Windows 11 Snap Layouts drop bar trigger & hover interaction
        if enable_layouts_bar {
            let bar_geo = compute_snap_bar_geometry(work_area, 1.0);
            let near_top_center = y <= top + 14 && (x - center_x).abs() <= 230;
            let inside_keep_zone = self.bar_open
                && y <= bar_geo.bar_rect.loc.y + bar_geo.bar_rect.size.h + 26
                && x >= bar_geo.bar_rect.loc.x - 28
                && x <= bar_geo.bar_rect.loc.x + bar_geo.bar_rect.size.w + 28;

            if near_top_center || inside_keep_zone {
                if !self.bar_open {
                    self.bar_open = true;
                    self.bar_opened_at = Some(Instant::now());
                }
                let mut hit_zone: Option<(usize, usize, SnapRegion)> = None;
                for &(ci, zi, region, zrect) in &bar_geo.zones {
                    if x >= zrect.loc.x - 2
                        && x <= zrect.loc.x + zrect.size.w + 2
                        && y >= zrect.loc.y - 2
                        && y <= zrect.loc.y + zrect.size.h + 2
                    {
                        hit_zone = Some((ci, zi, region));
                        break;
                    }
                }
                if let Some((ci, zi, region)) = hit_zone {
                    self.hovered_bar_zone = Some((ci, zi));
                    let target_rect = region.target_rect(work_area, snap_gap);
                    self.set_target(
                        Some(DragSnapTarget::Region(region)),
                        Some(target_rect),
                        animations_enabled,
                    );
                } else {
                    self.hovered_bar_zone = None;
                    // Staying right at the very top edge outside a layout card previews Maximize
                    if y <= top + 8 {
                        let max_rect = inset_rect(work_area, snap_gap.min(8));
                        self.set_target(
                            Some(DragSnapTarget::Maximize),
                            Some(max_rect),
                            animations_enabled,
                        );
                    } else {
                        self.set_target(None, None, animations_enabled);
                    }
                }
                return;
            } else {
                self.bar_open = false;
                self.bar_opened_at = None;
                self.hovered_bar_zone = None;
            }
        } else {
            self.bar_open = false;
            self.bar_opened_at = None;
            self.hovered_bar_zone = None;
        }

        // 2. Classic Windows edge & corner drag snapping
        let edge_thresh = 16;
        let corner_h = ((work_area.size.h as f32) * 0.20).round() as i32;
        let at_left = x <= left + edge_thresh;
        let at_right = x >= right - edge_thresh;
        let at_top = y <= top + 12;
        let near_top_corner = y <= top + corner_h;
        let near_bot_corner = y >= bottom - corner_h;

        let next_target = if at_left {
            if near_top_corner {
                Some(DragSnapTarget::Region(SnapRegion::TopLeft))
            } else if near_bot_corner {
                Some(DragSnapTarget::Region(SnapRegion::BottomLeft))
            } else {
                Some(DragSnapTarget::Region(SnapRegion::LeftHalf))
            }
        } else if at_right {
            if near_top_corner {
                Some(DragSnapTarget::Region(SnapRegion::TopRight))
            } else if near_bot_corner {
                Some(DragSnapTarget::Region(SnapRegion::BottomRight))
            } else {
                Some(DragSnapTarget::Region(SnapRegion::RightHalf))
            }
        } else if at_top {
            Some(DragSnapTarget::Maximize)
        } else {
            None
        };

        let next_rect = next_target.map(|t| match t {
            DragSnapTarget::Region(r) => r.target_rect(work_area, snap_gap),
            DragSnapTarget::Maximize => inset_rect(work_area, snap_gap.min(8)),
            DragSnapTarget::SwapTile { tile_rect, .. } => tile_rect,
        });
        self.set_target(next_target, next_rect, animations_enabled);
    }

    /// Updates drag-to-swap target while Hyprland Auto-Tiling is active.
    pub fn update_tiling_swap(
        &mut self,
        dragged_id: WindowId,
        target_tile: Option<(WindowId, Rect)>,
        animations_enabled: bool,
    ) {
        self.dragged_window = Some(dragged_id);
        self.bar_open = false;
        self.bar_opened_at = None;
        self.hovered_bar_zone = None;
        if let Some((target_id, tile_rect)) = target_tile {
            self.set_target(
                Some(DragSnapTarget::SwapTile {
                    target_id,
                    tile_rect,
                }),
                Some(tile_rect),
                animations_enabled,
            );
        } else {
            self.set_target(None, None, animations_enabled);
        }
    }

    fn set_target(
        &mut self,
        next_target: Option<DragSnapTarget>,
        next_rect: Option<Rect>,
        animations_enabled: bool,
    ) {
        if self.target == next_target {
            return;
        }
        let prev_rect = self.preview_anim.as_ref().map(|a| a.current().0);
        self.target = next_target;
        if let Some(to) = next_rect {
            let from = prev_rect.unwrap_or_else(|| {
                // Pop subtly outward from 92% centered when appearing fresh
                let dw = (to.size.w as f32 * 0.04).round() as i32;
                let dh = (to.size.h as f32 * 0.04).round() as i32;
                Rect::new(
                    Point::new(to.loc.x + dw, to.loc.y + dh),
                    Size::new((to.size.w - dw * 2).max(16), (to.size.h - dh * 2).max(16)),
                )
            });
            let dur = if animations_enabled { 165.0 } else { 1.0 };
            self.preview_anim = Some(AnimatedPreviewRect::new(from, to, dur));
            if self.preview_fade_started.is_none() {
                self.preview_fade_started = Some(Instant::now());
            }
        } else {
            self.preview_anim = None;
            self.preview_fade_started = None;
        }
    }

    /// Returns `(preview_rect, preview_alpha, is_animating)` for GPU rendering.
    pub fn current_preview(&self, animations_enabled: bool) -> Option<(Rect, f32, bool)> {
        let anim = self.preview_anim.as_ref()?;
        let (rect, rect_animating) = anim.current();
        let fade_t = if !animations_enabled {
            1.0
        } else {
            self.preview_fade_started
                .map(|t0| (t0.elapsed().as_secs_f32() / 0.13).clamp(0.0, 1.0))
                .unwrap_or(1.0)
        };
        Some((rect, fade_t, rect_animating || fade_t < 1.0))
    }

    /// Returns `(bar_geometry, bar_alpha, is_animating)` when the Windows 11 Snap Layouts bar is open.
    pub fn current_bar(&self, work_area: Rect, animations_enabled: bool) -> Option<(SnapBarGeometry, f32, bool)> {
        if !self.bar_open {
            return None;
        }
        let raw_t = if !animations_enabled {
            1.0
        } else {
            self.bar_opened_at
                .map(|t0| (t0.elapsed().as_secs_f32() / 0.18).clamp(0.0, 1.0))
                .unwrap_or(1.0)
        };
        let inv = 1.0 - raw_t;
        let eased = 1.0 - inv * inv * inv;
        let geo = compute_snap_bar_geometry(work_area, eased);
        Some((geo, eased, raw_t < 1.0))
    }
}

fn inset_rect(r: Rect, pad: i32) -> Rect {
    let p = pad.max(0);
    Rect::new(
        Point::new(r.loc.x + p, r.loc.y + p),
        Size::new((r.size.w - p * 2).max(160), (r.size.h - p * 2).max(120)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_corner_and_top_snap_layouts_bar_hit_test_cleanly() {
        let area = Rect::new(Point::new(0, 30), Size::new(1280, 720));
        let mut st = DragSnapOverlayState::default();

        // Left middle edge -> LeftHalf
        st.update_floating_drag(WindowId(1), 4.0, 400.0, area, 8, true, true, false);
        assert_eq!(st.target, Some(DragSnapTarget::Region(SnapRegion::LeftHalf)));
        assert!(!st.bar_open);

        // Top-right corner -> TopRight
        st.update_floating_drag(WindowId(1), 1276.0, 60.0, area, 8, true, true, false);
        assert_eq!(st.target, Some(DragSnapTarget::Region(SnapRegion::TopRight)));

        // Top-center -> opens Windows 11 Snap Layouts bar!
        st.update_floating_drag(WindowId(1), 640.0, 34.0, area, 8, true, true, false);
        assert!(st.bar_open);

        // Hover over card 2 (3-columns) center zone -> CenterThird!
        let bar = compute_snap_bar_geometry(area, 1.0);
        let (_, _, _, center_z) = bar
            .zones
            .iter()
            .copied()
            .find(|&(_, _, r, _)| r == SnapRegion::CenterThird)
            .expect("center third zone");
        st.update_floating_drag(
            WindowId(1),
            (center_z.loc.x + center_z.size.w / 2) as f64,
            (center_z.loc.y + center_z.size.h / 2) as f64,
            area,
            8,
            true,
            true,
            false,
        );
        assert!(st.bar_open);
        assert_eq!(
            st.target,
            Some(DragSnapTarget::Region(SnapRegion::CenterThird))
        );
    }
}
