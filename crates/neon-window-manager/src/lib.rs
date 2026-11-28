//! Core floating window manager for **NEON DE**.
//!
//! Coordinates:
//! - Monotonic [`WindowId`] assignment and metadata tracking
//! - Exclusive [`WindowMode`] state machine (Floating, Snapped, Maximized, Fullscreen, Minimized)
//! - Parent-child transient/dialog relationships and centered dialog placement
//! - Collision-aware floating placement
//! - Dynamic workspaces via [`neon_workspaces::WorkspaceManager`]
//! - Per-workspace MRU focus history and configurable [`FocusPolicy`]
//! - Configurable [`WindowRulesEngine`]

pub mod geometry;
pub mod rules;
pub mod state_machine;
pub mod tiling;

use std::collections::HashMap;

use neon_workspaces::{WorkspaceId, WorkspaceManager, WorkspacePolicy};
use tracing::debug;

use crate::{
    geometry::{Point, Rect, Size},
    rules::{EvaluatedRules, WindowRule, WindowRulesEngine},
    state_machine::WindowMode,
    tiling::{SplitAxis, TilingDivider, TilingLayout, compute_tiling_layout},
};

/// Monotonic internal window identifier independent of Wayland object IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// Configurable window focus policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPolicy {
    /// GNOME-inspired default: clicking a window focuses and raises it.
    #[default]
    ClickToFocus,
    /// Moving the pointer over a window focuses it without requiring a click.
    FocusFollowsMouse,
}

/// Complete tracked state for a managed NEON DE toplevel window.
#[derive(Debug, Clone)]
pub struct ManagedWindow {
    pub id: WindowId,
    pub workspace_id: WorkspaceId,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub mode: WindowMode,
    /// Last known floating geometry used to restore after leaving Maximized/Fullscreen/Snapped.
    pub floating_geometry: Rect,
    /// Current logical geometry on the output.
    pub current_geometry: Rect,
    pub mapped: bool,
    pub focused: bool,
    pub always_on_top: bool,
    /// When `true`, window stays floating even while global Hyprland Auto-Tiling is active
    /// (e.g. toggled via `Super+V` or automatically set for dialog / utility windows).
    pub float_override: bool,
    /// Tracks whether one-time automatic utility-window float detection has already run so
    /// the user can still explicitly tile utility apps via `Super+V`.
    pub auto_float_checked: bool,
    /// Parent window for transient dialogs / modal sheets.
    pub parent: Option<WindowId>,
    /// Rules already evaluated and applied to this window so we don't re-apply repeatedly.
    pub rules_applied: bool,
    pub matched_rules: Vec<String>,
}

impl ManagedWindow {
    pub fn is_visible_on(&self, active_workspace: WorkspaceId) -> bool {
        self.workspace_id == active_workspace && !self.mode.is_minimized()
    }
}

/// Resulting layout instruction returned to `neon-compositor` after a WM state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLayoutUpdate {
    pub id: WindowId,
    pub mode: WindowMode,
    /// Target logical location to map in `Space` (or `None` when minimized / on another workspace).
    pub target_location: Option<Point>,
    /// Target configure size to send via `xdg_toplevel` (`None` lets client pick floating size).
    pub configure_size: Option<Size>,
    pub visible_on_active_workspace: bool,
}

/// Central window manager coordinating windows, workspaces, rules, placement, and focus.
#[derive(Debug)]
pub struct WindowManager {
    next_id: u64,
    windows: HashMap<WindowId, ManagedWindow>,
    workspaces: WorkspaceManager<WindowId>,
    /// Per-workspace MRU focus stacks (least-recent at front, most-recent at back).
    focus_history: HashMap<WorkspaceId, Vec<WindowId>>,
    /// Per-workspace minimized history (most-recently minimized at back) for quick restore.
    minimized_history: HashMap<WorkspaceId, Vec<WindowId>>,
    /// Per-workspace deterministic tile order for Hyprland Auto-Tiling.
    tile_order: HashMap<WorkspaceId, Vec<WindowId>>,
    /// Per-workspace customizable split ratios (`split_index -> ratio`).
    tiling_split_ratios: HashMap<WorkspaceId, HashMap<usize, f32>>,
    /// Per-workspace split orientation overrides (`Super+J` togglesplit).
    tiling_split_flips: HashMap<WorkspaceId, HashMap<usize, bool>>,
    rules: WindowRulesEngine,
    focus_policy: FocusPolicy,
    snap_gap: i32,
    tiling_gap: i32,
    auto_tiling: bool,
    tiling_layout: TilingLayout,
    cascade_index: i32,
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new(WorkspacePolicy::default())
    }
}

impl WindowManager {
    pub fn new(workspace_policy: WorkspacePolicy) -> Self {
        Self {
            next_id: 0,
            windows: HashMap::new(),
            workspaces: WorkspaceManager::new(workspace_policy),
            focus_history: HashMap::new(),
            minimized_history: HashMap::new(),
            tile_order: HashMap::new(),
            tiling_split_ratios: HashMap::new(),
            tiling_split_flips: HashMap::new(),
            rules: WindowRulesEngine::with_defaults(),
            focus_policy: FocusPolicy::ClickToFocus,
            snap_gap: 0,
            tiling_gap: 6,
            auto_tiling: false,
            tiling_layout: TilingLayout::Dwindle,
            cascade_index: 0,
        }
    }

    pub fn focus_policy(&self) -> FocusPolicy {
        self.focus_policy
    }

    pub fn set_focus_policy(&mut self, policy: FocusPolicy) {
        self.focus_policy = policy;
    }

    pub fn snap_gap(&self) -> i32 {
        self.snap_gap
    }

    pub fn set_snap_gap(&mut self, gap: i32) {
        self.snap_gap = gap.max(0);
    }

    pub fn tiling_gap(&self) -> i32 {
        self.tiling_gap
    }

    pub fn set_tiling_gap(&mut self, gap: i32) {
        self.tiling_gap = gap.clamp(0, 24);
    }

    pub fn is_auto_tiling(&self) -> bool {
        self.auto_tiling
    }

    pub fn tiling_layout(&self) -> TilingLayout {
        self.tiling_layout
    }

    /// Detects utility / dialog applications that should float by default in Auto-Tiling mode.
    fn is_utility_app_id(app_id: Option<&str>) -> bool {
        let Some(aid) = app_id else {
            return false;
        };
        let low = aid.to_ascii_lowercase();
        low.contains("neonde.settings")
            || low == "neon-settings"
            || low.contains("polkit")
            || low.contains("screencast-picker")
            || low.contains("xdg-desktop-portal")
    }

    /// Marks `id` with `float_override = true` once if its `app_id` or parent identifies a
    /// utility/dialog window. Returns `true` if `float_override` changed to `true`.
    pub fn evaluate_auto_float(&mut self, id: WindowId) -> bool {
        let Some(w) = self.windows.get_mut(&id) else {
            return false;
        };
        if w.parent.is_some() {
            w.auto_float_checked = true;
            if !w.float_override {
                w.float_override = true;
                return true;
            }
            return false;
        }
        if w.auto_float_checked || w.app_id.is_none() {
            return false;
        }
        w.auto_float_checked = true;
        if Self::is_utility_app_id(w.app_id.as_deref()) && !w.float_override {
            w.float_override = true;
            return true;
        }
        false
    }

    /// Explicitly marks a window as floating-overridden (e.g., fixed-size dialogs, X11 splashes,
    /// or user-triggered "Float" / "Center" actions during Auto-Tiling).
    pub fn set_float_override(&mut self, id: WindowId, floating: bool) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.float_override = floating;
            w.auto_float_checked = true;
        }
    }

    /// Returns `true` if `id` is eligible to participate in Hyprland Auto-Tiling.
    pub fn is_tile_eligible(&self, id: WindowId) -> bool {
        let Some(w) = self.windows.get(&id) else {
            return false;
        };
        if w.float_override
            || w.parent.is_some()
            || w.mode.is_minimized()
            || w.mode.is_fullscreen()
            || w.mode.is_maximized()
            || w.mode.snapped_region().is_some()
        {
            return false;
        }
        if !w.auto_float_checked && Self::is_utility_app_id(w.app_id.as_deref()) {
            return false;
        }
        true
    }

    /// Enables or disables global Hyprland Auto-Tiling across all workspaces.
    ///
    /// - When turning **ON**: transitions all non-overridden floating, snapped, and maximized
    ///   windows on every workspace into `WindowMode::Tiled` (preserving their `floating_geometry`)
    ///   and computes tiled layouts.
    /// - When turning **OFF**: restores every `WindowMode::Tiled` window cleanly back to
    ///   `WindowMode::Floating` at its saved `floating_geometry` (cascading any windows opened
    ///   during tiling mode so they never stack identically) and clears `Tiled` restore modes on
    ///   minimized windows.
    pub fn set_auto_tiling(
        &mut self,
        enabled: bool,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if self.auto_tiling == enabled {
            return if enabled {
                self.relayout_all_tiles(work_area)
            } else {
                Vec::new()
            };
        }
        self.auto_tiling = enabled;
        if enabled {
            // Convert any currently Snapped or Maximized regular windows so enabling Auto-Tiling
            // immediately tiles the entire workspace as expected.
            let ids: Vec<WindowId> = self.windows.keys().copied().collect();
            for id in ids {
                self.evaluate_auto_float(id);
                if let Some(win) = self.windows.get_mut(&id)
                    && !win.float_override
                    && win.parent.is_none()
                    && (win.mode.is_maximized() || win.mode.snapped_region().is_some())
                {
                    win.mode.set_floating();
                }
            }
            self.relayout_all_tiles(work_area)
        } else {
            // Ensure minimized windows that were minimized from Tiled restore to Floating later.
            for win in self.windows.values_mut() {
                if let WindowMode::Minimized { restore_mode } = &mut win.mode
                    && restore_mode.is_tiled()
                {
                    **restore_mode = WindowMode::Floating;
                }
            }
            let tiled_ids: Vec<WindowId> = self
                .windows
                .values()
                .filter(|w| w.mode.is_tiled())
                .map(|w| w.id)
                .collect();
            let mut updates = Vec::with_capacity(tiled_ids.len());
            for id in tiled_ids {
                let needs_placement = self
                    .windows
                    .get(&id)
                    .is_some_and(|w| w.floating_geometry.size.is_empty());
                if needs_placement {
                    let fallback_size = Size::new(900, 620);
                    let placed = self.compute_floating_placement_excluding(
                        work_area,
                        fallback_size,
                        None,
                        false,
                        Some(id),
                    );
                    if let Some(win) = self.windows.get_mut(&id) {
                        win.floating_geometry = Rect::new(placed, fallback_size);
                    }
                }
                if let Some(win) = self.windows.get_mut(&id) {
                    win.mode.set_floating();
                    win.current_geometry = win.floating_geometry;
                }
                if let Some(up) = self.layout_update_for(id, work_area) {
                    updates.push(up);
                }
            }
            updates
        }
    }

    /// Switches the active Auto-Tiling layout (`Dwindle` ↔ `MasterStack`) and reflows if active.
    pub fn set_tiling_layout(
        &mut self,
        layout: TilingLayout,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if self.tiling_layout != layout {
            self.tiling_layout = layout;
            // Reset per-layout split overrides so switching Dwindle <-> Master-Stack starts crisp.
            self.tiling_split_ratios.clear();
            self.tiling_split_flips.clear();
        }
        if self.auto_tiling {
            self.relayout_all_tiles(work_area)
        } else {
            Vec::new()
        }
    }

    /// Toggles floating override (`Super+V`) on a window during Auto-Tiling mode and reflows its workspace.
    pub fn toggle_window_float(
        &mut self,
        id: WindowId,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        let Some((ws_id, was_non_floating)) = self.windows.get(&id).map(|w| {
            (
                w.workspace_id,
                w.mode.is_tiled() || w.mode.snapped_region().is_some() || w.mode.is_maximized(),
            )
        }) else {
            return Vec::new();
        };

        if !self.auto_tiling {
            // Outside Auto-Tiling, Super+V cleanly floats a snapped/maximized window.
            if was_non_floating && let Some(up) = self.set_floating(id, work_area) {
                return vec![up];
            }
            return Vec::new();
        }

        // Evaluate utility auto-float first so pressing Super+V on a utility app cleanly unfloats it!
        self.evaluate_auto_float(id);

        let becoming_floating = {
            let Some(win) = self.windows.get_mut(&id) else {
                return Vec::new();
            };
            win.float_override = !win.float_override;
            win.auto_float_checked = true;
            win.float_override
        };

        if becoming_floating {
            let needs_size = self
                .windows
                .get(&id)
                .is_some_and(|w| w.floating_geometry.size.is_empty());
            if needs_size {
                let sz = Size::new(860, 580);
                let loc = self.compute_floating_placement_excluding(
                    work_area,
                    sz,
                    None,
                    false,
                    Some(id),
                );
                if let Some(win) = self.windows.get_mut(&id) {
                    win.floating_geometry = Rect::new(loc, sz);
                }
            }
            if let Some(win) = self.windows.get_mut(&id) {
                win.mode.set_floating();
                win.current_geometry = win.floating_geometry;
            }
        } else if let Some(win) = self.windows.get_mut(&id) {
            // Re-entering tiling: clear Snapped/Maximized so `relayout_workspace_tiles` tiles it.
            if win.mode.is_maximized() || win.mode.snapped_region().is_some() {
                win.mode.set_floating();
            }
        }

        let mut updates = Vec::new();
        if becoming_floating && let Some(up) = self.layout_update_for(id, work_area) {
            updates.push(up);
        }
        updates.extend(self.relayout_workspace_tiles(ws_id, work_area));
        updates
    }

    fn ordered_eligible_tiles_for(&mut self, ws_id: WorkspaceId) -> Vec<WindowId> {
        let ws_windows: Vec<WindowId> = self.workspaces.windows_on(ws_id).to_vec();
        for &wid in &ws_windows {
            self.evaluate_auto_float(wid);
        }
        let snapshot_order: Vec<WindowId> = {
            let entry = self.tile_order.entry(ws_id).or_default();
            entry.retain(|id| ws_windows.contains(id));
            for &wid in &ws_windows {
                if !entry.contains(&wid) {
                    entry.push(wid);
                }
            }
            entry.clone()
        };
        snapshot_order
            .into_iter()
            .filter(|&wid| self.is_tile_eligible(wid))
            .collect()
    }

    /// Recomputes and applies tiled rectangles for all eligible windows on `ws_id`.
    pub fn relayout_workspace_tiles(
        &mut self,
        ws_id: WorkspaceId,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if !self.auto_tiling {
            return Vec::new();
        }
        let ordered_eligible = self.ordered_eligible_tiles_for(ws_id);
        let max_splits = ordered_eligible.len().saturating_sub(1);
        if let Some(ratios) = self.tiling_split_ratios.get_mut(&ws_id) {
            ratios.retain(|&idx, _| idx < max_splits);
        }
        if let Some(flips) = self.tiling_split_flips.get_mut(&ws_id) {
            flips.retain(|&idx, _| idx < max_splits);
        }

        let empty_ratios = HashMap::new();
        let empty_flips = HashMap::new();
        let ratios = self.tiling_split_ratios.get(&ws_id).unwrap_or(&empty_ratios);
        let flips = self.tiling_split_flips.get(&ws_id).unwrap_or(&empty_flips);

        let layout_res = compute_tiling_layout(
            self.tiling_layout,
            &ordered_eligible,
            work_area,
            self.tiling_gap,
            ratios,
            flips,
        );
        let active_ws = self.active_workspace_id();
        let mut updates = Vec::with_capacity(layout_res.rects.len());
        for (wid, rect) in layout_res.rects {
            if let Some(win) = self.windows.get_mut(&wid) {
                if win.mode.is_floating() && !win.current_geometry.size.is_empty() {
                    win.floating_geometry = win.current_geometry;
                }
                win.mode.set_tiled();
                win.current_geometry = rect;
                let visible = win.is_visible_on(active_ws);
                updates.push(WindowLayoutUpdate {
                    id: wid,
                    mode: WindowMode::Tiled,
                    target_location: visible.then_some(rect.loc),
                    configure_size: Some(rect.size),
                    visible_on_active_workspace: visible,
                });
            }
        }
        updates
    }

    /// Returns live interactive split dividers for tiled windows on `ws_id`.
    pub fn tiling_dividers_for(&self, ws_id: WorkspaceId, work_area: Rect) -> Vec<TilingDivider> {
        if !self.auto_tiling {
            return Vec::new();
        }
        let ws_windows = self.workspaces.windows_on(ws_id);
        let ordered_eligible: Vec<WindowId> = self
            .tile_order
            .get(&ws_id)
            .map(|order| {
                order
                    .iter()
                    .copied()
                    .filter(|id| ws_windows.contains(id) && self.is_tile_eligible(*id))
                    .collect()
            })
            .unwrap_or_else(|| {
                ws_windows
                    .iter()
                    .copied()
                    .filter(|id| self.is_tile_eligible(*id))
                    .collect()
            });
        if ordered_eligible.len() < 2 {
            return Vec::new();
        }
        let empty_ratios = HashMap::new();
        let empty_flips = HashMap::new();
        let ratios = self.tiling_split_ratios.get(&ws_id).unwrap_or(&empty_ratios);
        let flips = self.tiling_split_flips.get(&ws_id).unwrap_or(&empty_flips);
        compute_tiling_layout(
            self.tiling_layout,
            &ordered_eligible,
            work_area,
            self.tiling_gap,
            ratios,
            flips,
        )
        .dividers
    }

    /// Hit-tests pointer `(px, py)` against the tiled split dividers on `ws_id`.
    pub fn divider_at_point(
        &self,
        ws_id: WorkspaceId,
        px: i32,
        py: i32,
        work_area: Rect,
    ) -> Option<TilingDivider> {
        let hit_pad = (self.tiling_gap / 2 + 3).max(5);
        self.tiling_dividers_for(ws_id, work_area)
            .into_iter()
            .find(|d| d.hit_test(px, py, hit_pad))
    }

    /// Finds the best tiled split divider associated with resizing an edge of tiled window `id`.
    pub fn divider_for_window_edge(
        &self,
        id: WindowId,
        want_vertical: bool,
        toward_positive: bool,
        work_area: Rect,
    ) -> Option<TilingDivider> {
        let win = self.windows.get(&id)?;
        if !self.auto_tiling || !win.mode.is_tiled() {
            return None;
        }
        let target_axis = if want_vertical {
            SplitAxis::VerticalDivider
        } else {
            SplitAxis::HorizontalDivider
        };
        let wr = win.current_geometry;
        let dividers: Vec<TilingDivider> = self
            .tiling_dividers_for(win.workspace_id, work_area)
            .into_iter()
            .filter(|d| d.axis == target_axis)
            .collect();
        if dividers.is_empty() {
            return None;
        }

        let tol = self.tiling_gap.max(4) + 6;
        let touches_edge = |d: &TilingDivider, pos_edge: bool| -> bool {
            match target_axis {
                SplitAxis::VerticalDivider => {
                    let edge_x = if pos_edge { wr.loc.x + wr.size.w } else { wr.loc.x };
                    let div_x = d.divider_rect.loc.x + d.divider_rect.size.w / 2;
                    let y_overlap =
                        d.divider_rect.loc.y < wr.loc.y + wr.size.h && d.divider_rect.loc.y + d.divider_rect.size.h > wr.loc.y;
                    y_overlap && (div_x - edge_x).abs() <= tol
                }
                SplitAxis::HorizontalDivider => {
                    let edge_y = if pos_edge { wr.loc.y + wr.size.h } else { wr.loc.y };
                    let div_y = d.divider_rect.loc.y + d.divider_rect.size.h / 2;
                    let x_overlap =
                        d.divider_rect.loc.x < wr.loc.x + wr.size.w && d.divider_rect.loc.x + d.divider_rect.size.w > wr.loc.x;
                    x_overlap && (div_y - edge_y).abs() <= tol
                }
            }
        };

        // Prefer the divider directly adjacent to the dragged edge, else the opposite edge of this
        // window on the same axis so dragging an outer edge still intuitively resizes the pane.
        dividers
            .iter()
            .copied()
            .find(|d| touches_edge(d, toward_positive))
            .or_else(|| dividers.iter().copied().find(|d| touches_edge(d, !toward_positive)))
            .or_else(|| {
                dividers
                    .into_iter()
                    .find(|d| d.leading_window == id || d.trailing_window == id)
            })
    }

    /// Sets the exact ratio (`[0.20, 0.80]`) of `split_index` on `ws_id` and reflows tiles.
    pub fn set_split_ratio(
        &mut self,
        ws_id: WorkspaceId,
        split_index: usize,
        ratio: f32,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if !self.auto_tiling {
            return Vec::new();
        }
        let clamped = ratio.clamp(0.20, 0.80);
        self.tiling_split_ratios
            .entry(ws_id)
            .or_default()
            .insert(split_index, clamped);
        self.relayout_workspace_tiles(ws_id, work_area)
    }

    /// Adjusts the active split ratio adjacent to `focused_id` by `(dx_delta, dy_delta)` (e.g. `±0.05`).
    pub fn resize_tiled_window(
        &mut self,
        focused_id: WindowId,
        dx_delta: f32,
        dy_delta: f32,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if !self.auto_tiling {
            return Vec::new();
        }
        let Some(ws_id) = self
            .windows
            .get(&focused_id)
            .filter(|w| w.mode.is_tiled())
            .map(|w| w.workspace_id)
        else {
            return Vec::new();
        };

        let mut changed = false;
        if dx_delta.abs() > 1e-4
            && let Some(div) = self.divider_for_window_edge(focused_id, true, true, work_area)
        {
            let cur = div.ratio_bps as f32 / 10_000.0;
            let next = (cur + dx_delta).clamp(0.20, 0.80);
            self.tiling_split_ratios
                .entry(ws_id)
                .or_default()
                .insert(div.split_index, next);
            changed = true;
        }
        if dy_delta.abs() > 1e-4
            && let Some(div) = self.divider_for_window_edge(focused_id, false, true, work_area)
        {
            let cur = div.ratio_bps as f32 / 10_000.0;
            let next = (cur + dy_delta).clamp(0.20, 0.80);
            self.tiling_split_ratios
                .entry(ws_id)
                .or_default()
                .insert(div.split_index, next);
            changed = true;
        }
        if changed {
            self.relayout_workspace_tiles(ws_id, work_area)
        } else {
            Vec::new()
        }
    }

    /// Resets all customized split ratios on `ws_id` back to balanced defaults and reflows.
    pub fn reset_workspace_split_ratios(
        &mut self,
        ws_id: WorkspaceId,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        self.tiling_split_ratios.remove(&ws_id);
        if self.auto_tiling {
            self.relayout_workspace_tiles(ws_id, work_area)
        } else {
            Vec::new()
        }
    }

    /// Toggles the split orientation (`Super+J` Hyprland `togglesplit`) for the split owning `focused_id`.
    pub fn toggle_split_orientation(
        &mut self,
        focused_id: WindowId,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if !self.auto_tiling {
            return Vec::new();
        }
        let Some(ws_id) = self
            .windows
            .get(&focused_id)
            .filter(|w| w.mode.is_tiled())
            .map(|w| w.workspace_id)
        else {
            return Vec::new();
        };
        let ordered = self.ordered_eligible_tiles_for(ws_id);
        if ordered.len() < 2 {
            return Vec::new();
        }
        let split_idx = match self.tiling_layout {
            TilingLayout::MasterStack => 0,
            TilingLayout::Dwindle => {
                let pos = ordered.iter().position(|&id| id == focused_id).unwrap_or(0);
                pos.min(ordered.len() - 2)
            }
        };
        let entry = self
            .tiling_split_flips
            .entry(ws_id)
            .or_default()
            .entry(split_idx)
            .or_insert(false);
        *entry = !*entry;
        self.relayout_workspace_tiles(ws_id, work_area)
    }

    /// Promotes `focused_id` to slot `0` (primary/master pane) on its workspace, or swaps with slot 1
    /// if already in slot `0` (authentic Master-Stack swap-with-master).
    pub fn promote_to_master(
        &mut self,
        focused_id: WindowId,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if !self.auto_tiling {
            return Vec::new();
        }
        let Some(ws_id) = self
            .windows
            .get(&focused_id)
            .filter(|w| w.mode.is_tiled())
            .map(|w| w.workspace_id)
        else {
            return Vec::new();
        };
        let ordered = self.ordered_eligible_tiles_for(ws_id);
        if ordered.len() < 2 {
            return Vec::new();
        }
        let target_id = if ordered[0] == focused_id {
            ordered[1]
        } else {
            ordered[0]
        };
        self.swap_tiled_windows(focused_id, target_id, work_area)
    }

    /// Finds the nearest visible tiled (or floating) neighbor of `focused_id` in direction `(dx, dy)`
    /// for directional keyboard focus navigation (`Super+Arrows` in Auto-Tiling mode).
    pub fn focus_tiled_in_direction(
        &self,
        focused_id: WindowId,
        dx: i32,
        dy: i32,
    ) -> Option<WindowId> {
        let cur = self.windows.get(&focused_id)?;
        let ws_id = cur.workspace_id;
        let cur_rect = cur.current_geometry;
        let cx = cur_rect.loc.x as f64 + cur_rect.size.w as f64 * 0.5;
        let cy = cur_rect.loc.y as f64 + cur_rect.size.h as f64 * 0.5;

        let mut best: Option<(f64, WindowId)> = None;
        for w in self.windows.values() {
            if w.id == focused_id || !w.is_visible_on(ws_id) || !w.mode.is_tiled() {
                continue;
            }
            let tx = w.current_geometry.loc.x as f64 + w.current_geometry.size.w as f64 * 0.5;
            let ty = w.current_geometry.loc.y as f64 + w.current_geometry.size.h as f64 * 0.5;
            let vx = tx - cx;
            let vy = ty - cy;
            let forward = vx * f64::from(dx) + vy * f64::from(dy);
            if forward <= 8.0 {
                continue;
            }
            let perp = (vx * f64::from(dy) - vy * f64::from(dx)).abs();
            let score = forward + perp * 1.6;
            if best.is_none_or(|(bscore, _)| score < bscore) {
                best = Some((score, w.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Reflows tiled layouts across every workspace.
    pub fn relayout_all_tiles(&mut self, work_area: Rect) -> Vec<WindowLayoutUpdate> {
        if !self.auto_tiling {
            return Vec::new();
        }
        let ws_ids: Vec<WorkspaceId> = self
            .workspaces
            .workspaces()
            .iter()
            .map(|w| w.id)
            .collect();
        let mut updates = Vec::new();
        for ws_id in ws_ids {
            updates.extend(self.relayout_workspace_tiles(ws_id, work_area));
        }
        updates
    }

    /// Swaps the tile slots of two tiled windows on the same workspace (for drag-swap & shortcuts).
    pub fn swap_tiled_windows(
        &mut self,
        a: WindowId,
        b: WindowId,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if a == b || !self.auto_tiling {
            return Vec::new();
        }
        let Some(ws_a) = self.windows.get(&a).map(|w| w.workspace_id) else {
            return Vec::new();
        };
        let Some(ws_b) = self.windows.get(&b).map(|w| w.workspace_id) else {
            return Vec::new();
        };
        if ws_a != ws_b {
            return Vec::new();
        }
        let order = self.tile_order.entry(ws_a).or_default();
        if let (Some(ia), Some(ib)) = (
            order.iter().position(|&id| id == a),
            order.iter().position(|&id| id == b),
        ) {
            order.swap(ia, ib);
            return self.relayout_workspace_tiles(ws_a, work_area);
        }
        Vec::new()
    }

    /// Swaps `focused_id` with the nearest tiled neighbor in direction `(dx, dy)` (`-1/0/1`).
    pub fn swap_tiled_in_direction(
        &mut self,
        focused_id: WindowId,
        dx: i32,
        dy: i32,
        work_area: Rect,
    ) -> Vec<WindowLayoutUpdate> {
        if !self.auto_tiling {
            return Vec::new();
        }
        if let Some(target_id) = self.focus_tiled_in_direction(focused_id, dx, dy) {
            return self.swap_tiled_windows(focused_id, target_id, work_area);
        }
        Vec::new()
    }

    /// Returns the tiled window whose tile rect contains `(px, py)` (excluding `exclude_id`),
    /// tolerating thin tiling gaps so drag-to-swap preview never flickers across borders.
    pub fn tiled_window_at_point(
        &self,
        ws_id: WorkspaceId,
        px: i32,
        py: i32,
        exclude_id: WindowId,
    ) -> Option<(WindowId, Rect)> {
        let exact = self.windows.values().find_map(|w| {
            if w.id == exclude_id || w.workspace_id != ws_id || !w.mode.is_tiled() {
                return None;
            }
            let r = w.current_geometry;
            if px >= r.loc.x
                && px < r.loc.x + r.size.w
                && py >= r.loc.y
                && py < r.loc.y + r.size.h
            {
                Some((w.id, r))
            } else {
                None
            }
        });
        if exact.is_some() {
            return exact;
        }
        let pad = (self.tiling_gap / 2 + 2).max(4);
        self.windows.values().find_map(|w| {
            if w.id == exclude_id || w.workspace_id != ws_id || !w.mode.is_tiled() {
                return None;
            }
            let r = w.current_geometry;
            if px >= r.loc.x - pad
                && px < r.loc.x + r.size.w + pad
                && py >= r.loc.y - pad
                && py < r.loc.y + r.size.h + pad
            {
                Some((w.id, r))
            } else {
                None
            }
        })
    }

    pub fn add_rule(&mut self, rule: WindowRule) {
        self.rules.add_rule(rule);
    }

    pub fn workspaces(&self) -> &WorkspaceManager<WindowId> {
        &self.workspaces
    }

    pub fn active_workspace_id(&self) -> WorkspaceId {
        self.workspaces.active_workspace_id()
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn get(&self, id: WindowId) -> Option<&ManagedWindow> {
        self.windows.get(&id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut ManagedWindow> {
        self.windows.get_mut(&id)
    }

    pub fn all_window_ids(&self) -> Vec<WindowId> {
        self.windows.keys().copied().collect()
    }

    /// Returns all currently visible (non-minimized) window IDs on the active workspace.
    pub fn active_visible_windows(&self) -> Vec<WindowId> {
        let active_ws = self.active_workspace_id();
        self.workspaces
            .windows_on(active_ws)
            .iter()
            .copied()
            .filter(|id| {
                self.windows
                    .get(id)
                    .map(|w| !w.mode.is_minimized())
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Returns visible windows on the active workspace ordered from bottom-most (oldest focus)
    /// to top-most (most-recently focused), preserving deterministic stacking order.
    pub fn active_visible_windows_mru_order(&self) -> Vec<WindowId> {
        self.visible_windows_mru_order_for(self.active_workspace_id())
    }

    /// Returns visible (non-minimized) windows on `ws_id` ordered from bottom-most (oldest focus)
    /// to top-most (most-recently focused).
    ///
    /// When Auto-Tiling is active, floating/snapped/maximized windows always stack above tiled
    /// windows so dialogs, Settings, and `Super+V` floated apps are never buried behind tiles,
    /// while `always_on_top` windows always remain on top of everything.
    pub fn visible_windows_mru_order_for(&self, ws_id: WorkspaceId) -> Vec<WindowId> {
        let mut tiled = Vec::new();
        let mut normal = Vec::new();
        let mut pinned_top = Vec::new();
        let auto_tiling = self.auto_tiling;
        let mut push_id = |id: WindowId| {
            let Some(w) = self.windows.get(&id) else {
                return;
            };
            if !w.is_visible_on(ws_id) {
                return;
            }
            if w.always_on_top {
                if !pinned_top.contains(&id) {
                    pinned_top.push(id);
                }
            } else if auto_tiling && w.mode.is_tiled() {
                if !tiled.contains(&id) {
                    tiled.push(id);
                }
            } else if !normal.contains(&id) {
                normal.push(id);
            }
        };
        if let Some(stack) = self.focus_history.get(&ws_id) {
            for &id in stack {
                push_id(id);
            }
        }
        for &id in self.workspaces.windows_on(ws_id) {
            push_id(id);
        }
        tiled.extend(normal);
        tiled.extend(pinned_top);
        tiled
    }

    /// Returns visible non-tiled floating/dialog windows on `ws_id` that should remain stacked
    /// above tiled windows when Auto-Tiling is active (in MRU bottom-to-top order).
    pub fn visible_floating_above_tiled_windows(&self, ws_id: WorkspaceId) -> Vec<WindowId> {
        if !self.auto_tiling {
            return Vec::new();
        }
        self.visible_windows_mru_order_for(ws_id)
            .into_iter()
            .filter(|id| {
                self.windows
                    .get(id)
                    .map(|w| w.mode.is_floating() && !w.always_on_top)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Returns visible `always_on_top` windows on `ws_id` in MRU bottom-to-top order.
    pub fn visible_always_on_top_windows(&self, ws_id: WorkspaceId) -> Vec<WindowId> {
        self.visible_windows_mru_order_for(ws_id)
            .into_iter()
            .filter(|id| {
                self.windows
                    .get(id)
                    .map(|w| w.always_on_top)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Returns visible transient descendant windows of `parent_id` on the active workspace,
    /// ordered bottom-to-top so child dialogs always stack above their parent window.
    pub fn visible_transient_children(&self, parent_id: WindowId) -> Vec<WindowId> {
        let active_ws = self.active_workspace_id();
        let mru = self.active_visible_windows_mru_order();
        let mut result = Vec::new();
        let mut frontier = vec![parent_id];
        let mut cursor = 0;
        while cursor < frontier.len() {
            let cur = frontier[cursor];
            cursor += 1;
            for &wid in &mru {
                if wid != parent_id
                    && !result.contains(&wid)
                    && self.windows.get(&wid).and_then(|w| w.parent) == Some(cur)
                    && self
                        .windows
                        .get(&wid)
                        .map(|w| w.is_visible_on(active_ws))
                        .unwrap_or(false)
                {
                    result.push(wid);
                    frontier.push(wid);
                }
            }
        }
        result
    }

    /// Registers a newly created toplevel window, assigns workspace & intelligent floating location.
    pub fn register_window(&mut self, work_area: Rect, parent: Option<WindowId>) -> ManagedWindow {
        self.next_id += 1;
        let id = WindowId(self.next_id);

        // Transient dialogs inherit their parent's workspace when present.
        let target_ws_idx = parent
            .and_then(|p| self.workspaces.workspace_of_window(p))
            .and_then(|ws| self.workspaces.index_of(ws));
        let workspace_id = self.workspaces.assign_window(id, target_ws_idx);

        let initial_location =
            self.compute_floating_placement(work_area, Size::new(640, 480), parent, false);
        let initial_rect = Rect::new(initial_location, Size::new(0, 0));

        let record = ManagedWindow {
            id,
            workspace_id,
            app_id: None,
            title: None,
            mode: WindowMode::Floating,
            floating_geometry: initial_rect,
            current_geometry: initial_rect,
            mapped: false,
            focused: false,
            always_on_top: false,
            float_override: parent.is_some(),
            auto_float_checked: parent.is_some(),
            parent,
            rules_applied: false,
            matched_rules: Vec::new(),
        };

        self.windows.insert(id, record.clone());
        self.focus_history.entry(workspace_id).or_default().push(id);
        // Insert new window right after currently focused tiled window (authentic Hyprland behavior)
        let focused_on_ws = self
            .focused_id()
            .filter(|fid| self.windows.get(fid).is_some_and(|w| w.workspace_id == workspace_id));
        let order = self.tile_order.entry(workspace_id).or_default();
        if let Some(fid) = focused_on_ws
            && let Some(pos) = order.iter().position(|&w| w == fid)
        {
            order.insert(pos + 1, id);
        } else {
            order.push(id);
        }
        debug!(
            ?id,
            ?workspace_id,
            ?initial_location,
            ?parent,
            "Registered managed window"
        );
        record
    }

    /// Removes a window cleanly from WM state, workspaces, and focus/minimized histories.
    pub fn remove_window(&mut self, id: WindowId) -> Option<ManagedWindow> {
        let removed = self.windows.remove(&id)?;
        self.workspaces.remove_window(id);
        for stack in self.focus_history.values_mut() {
            stack.retain(|&w| w != id);
        }
        for stack in self.minimized_history.values_mut() {
            stack.retain(|&w| w != id);
        }
        for order in self.tile_order.values_mut() {
            order.retain(|&w| w != id);
        }
        // Clear dangling parent links on any remaining child dialogs.
        for window in self.windows.values_mut() {
            if window.parent == Some(id) {
                window.parent = None;
            }
        }
        Some(removed)
    }

    /// Evaluates configured window rules once metadata (`app_id` / `title`) is available.
    ///
    /// Returns a [`WindowLayoutUpdate`] if a matching rule changed the window's mode, size,
    /// position, or workspace assignment.
    pub fn apply_rules_if_needed(
        &mut self,
        id: WindowId,
        work_area: Rect,
    ) -> Option<(EvaluatedRules, WindowLayoutUpdate)> {
        let (app_id, title, already_applied, current_size) = {
            let win = self.windows.get(&id)?;
            (
                win.app_id.clone(),
                win.title.clone(),
                win.rules_applied,
                win.current_geometry.size,
            )
        };
        if already_applied || (app_id.is_none() && title.is_none()) {
            return None;
        }

        let eval = self.rules.evaluate(app_id.as_deref(), title.as_deref());
        if eval.matched_names.is_empty() {
            return None;
        }

        if let Some(ws_idx) = eval.workspace_index
            && let Some(new_ws) = self.workspaces.move_window_to_index(id, ws_idx, false)
            && let Some(win) = self.windows.get_mut(&id)
        {
            let old_ws = win.workspace_id;
            win.workspace_id = new_ws;
            self.transfer_focus_entry(id, old_ws, new_ws);
        }

        let initial_size = eval.initial_size.unwrap_or(if current_size.is_empty() {
            Size::new(640, 480)
        } else {
            current_size
        });

        let centered_origin = if eval.center_on_open {
            Some(work_area.clamp_origin(work_area.center_size(initial_size), initial_size))
        } else {
            None
        };

        if let Some(win) = self.windows.get_mut(&id) {
            win.rules_applied = true;
            win.matched_rules = eval.matched_names.clone();
            if let Some(size) = eval.initial_size {
                win.floating_geometry.size = size;
            }
            if let Some(origin) = centered_origin {
                win.floating_geometry.loc = origin;
                if win.mode.is_floating() {
                    win.current_geometry.loc = origin;
                }
            }
        }

        if let Some(target_mode) = &eval.initial_mode {
            let update = self.apply_mode(id, target_mode.clone(), work_area)?;
            return Some((eval, update));
        }

        let update = self.layout_update_for(id, work_area)?;
        Some((eval, update))
    }

    /// Updates a window's parent relationship and recenters it over the parent if floating and unmapped.
    pub fn set_parent(
        &mut self,
        id: WindowId,
        parent: Option<WindowId>,
        work_area: Rect,
    ) -> Option<Point> {
        let parent_rect = parent.and_then(|pid| self.windows.get(&pid).map(|p| p.current_geometry));
        let win = self.windows.get_mut(&id)?;
        win.parent = parent;
        if parent.is_some() {
            win.float_override = true;
            win.auto_float_checked = true;
            if win.mode.is_tiled() {
                win.mode.set_floating();
            }
        }
        if let Some(prect) = parent_rect
            && win.mode.is_floating()
        {
            let size = if win.current_geometry.size.is_empty() {
                Size::new(480, 320)
            } else {
                win.current_geometry.size
            };
            let centered = work_area.clamp_origin(prect.center_size(size), size);
            win.floating_geometry.loc = centered;
            win.current_geometry.loc = centered;
            return Some(centered);
        }
        None
    }

    /// Computes and applies center-first collision-aware floating placement for `id` using its
    /// known `size` inside `work_area`.
    pub fn place_floating_window(
        &mut self,
        id: WindowId,
        work_area: Rect,
        size: Size,
    ) -> Option<Point> {
        let parent = self.windows.get(&id)?.parent;
        let loc = self.compute_floating_placement_excluding(work_area, size, parent, false, Some(id));
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() {
            win.floating_geometry.loc = loc;
            win.current_geometry.loc = loc;
            Some(loc)
        } else {
            None
        }
    }

    /// Records the window's committed geometry when floating.
    pub fn update_committed_size(
        &mut self,
        id: WindowId,
        size: Size,
        work_area: Rect,
    ) -> Option<Point> {
        let (parent, parent_rect, was_unmapped, is_floating, rules_centered) = {
            let win = self.windows.get(&id)?;
            let prect = win
                .parent
                .and_then(|pid| self.windows.get(&pid).map(|p| p.current_geometry));
            (
                win.parent,
                prect,
                !win.mapped,
                win.mode.is_floating(),
                !win.matched_rules.is_empty(),
            )
        };

        {
            let win = self.windows.get_mut(&id)?;
            // Only let client-committed sizes update `current_geometry` when floating;
            // tiled/snapped/maximized/fullscreen windows keep their authoritative WM slot rect
            // so directional navigation, divider math, and drag-swap hit-testing stay exact.
            if is_floating {
                win.current_geometry.size = size;
                win.floating_geometry.size = size;
            }
        }

        if was_unmapped && !size.is_empty() {
            if let Some(win) = self.windows.get_mut(&id) {
                win.mapped = true;
            }
            if is_floating {
                let target_loc = if let Some(prect) = parent_rect {
                    // Re-center transient dialogs precisely over their parent once real size is known.
                    work_area.clamp_origin(prect.center_size(size), size)
                } else if !rules_centered {
                    // GNOME/KDE center-first placement using the window's actual committed size,
                    // gently cascading only when another visible floating window is already there.
                    self.compute_floating_placement_excluding(
                        work_area,
                        size,
                        parent,
                        false,
                        Some(id),
                    )
                } else {
                    return None;
                };
                if let Some(win) = self.windows.get_mut(&id) {
                    win.floating_geometry.loc = target_loc;
                    win.current_geometry.loc = target_loc;
                }
                return Some(target_loc);
            }
        }
        None
    }

    /// Updates a floating window's location (e.g. during interactive drag).
    pub fn set_floating_location(&mut self, id: WindowId, location: Point) {
        if let Some(win) = self.windows.get_mut(&id) {
            win.current_geometry.loc = location;
            if win.mode.is_floating() {
                win.floating_geometry.loc = location;
            }
        }
    }

    /// Applies an explicit [`WindowMode`] transition and computes the resulting layout update.
    pub fn apply_mode(
        &mut self,
        id: WindowId,
        target_mode: WindowMode,
        work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() && !target_mode.is_floating() {
            // Snapshot current floating geometry before entering maximized/fullscreen/snapped/minimized.
            win.floating_geometry = win.current_geometry;
        }
        win.mode = target_mode;
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
    }

    pub fn set_maximized(
        &mut self,
        id: WindowId,
        maximize: bool,
        work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let auto_tiling = self.auto_tiling;
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() && maximize {
            win.floating_geometry = win.current_geometry;
        }
        if !win.mode.set_maximized(maximize) {
            return self.layout_update_for(id, work_area);
        }
        // When exiting maximize during Auto-Tiling, return regular windows seamlessly back into
        // `WindowMode::Tiled` instead of stranding them in Floating above tiled panes.
        if !maximize && auto_tiling && !win.float_override && win.parent.is_none() {
            win.mode.set_tiled();
        }
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
    }

    pub fn toggle_maximized(
        &mut self,
        id: WindowId,
        work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let next = !self.windows.get(&id)?.mode.is_maximized();
        self.set_maximized(id, next, work_area)
    }

    pub fn set_fullscreen(
        &mut self,
        id: WindowId,
        fullscreen: bool,
        work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let auto_tiling = self.auto_tiling;
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() && fullscreen {
            win.floating_geometry = win.current_geometry;
        }
        if !win.mode.set_fullscreen(fullscreen) {
            return self.layout_update_for(id, work_area);
        }
        if !fullscreen && auto_tiling && !win.float_override && win.parent.is_none() {
            win.mode.set_tiled();
        }
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
    }

    pub fn toggle_fullscreen(
        &mut self,
        id: WindowId,
        work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let next = !self.windows.get(&id)?.mode.is_fullscreen();
        self.set_fullscreen(id, next, work_area)
    }

    pub fn toggle_snap(
        &mut self,
        id: WindowId,
        region: geometry::SnapRegion,
        work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() {
            win.floating_geometry = win.current_geometry;
        }
        win.mode.toggle_snap(region);
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
    }

    /// Explicitly snaps `id` into `region` (used by drag-to-edge / Snap Layouts drops).
    pub fn snap_to_region(
        &mut self,
        id: WindowId,
        region: geometry::SnapRegion,
        work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() {
            win.floating_geometry = win.current_geometry;
        }
        win.mode.set_snapped(region);
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
    }

    pub fn set_floating(&mut self, id: WindowId, work_area: Rect) -> Option<WindowLayoutUpdate> {
        let win = self.windows.get_mut(&id)?;
        if !win.mode.set_floating() {
            return None;
        }
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
    }

    /// Toggles the `always_on_top` stacking flag on `id` and returns its new state.
    pub fn toggle_always_on_top(&mut self, id: WindowId) -> Option<bool> {
        let win = self.windows.get_mut(&id)?;
        win.always_on_top = !win.always_on_top;
        Some(win.always_on_top)
    }

    /// Centers `id` inside `work_area` (restoring to floating if currently snapped/maximized/tiled).
    pub fn center_window(&mut self, id: WindowId, work_area: Rect) -> Option<WindowLayoutUpdate> {
        let auto_tiling = self.auto_tiling;
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_minimized() {
            win.mode.restore();
        }
        if !win.mode.is_floating() {
            win.mode.set_floating();
            if auto_tiling {
                win.float_override = true;
                win.auto_float_checked = true;
            }
            if !win.floating_geometry.size.is_empty() {
                win.current_geometry.size = win.floating_geometry.size;
            }
        }
        let size = if win.current_geometry.size.is_empty() {
            if !win.floating_geometry.size.is_empty() {
                win.floating_geometry.size
            } else {
                Size::new(800, 560)
            }
        } else {
            win.current_geometry.size
        };
        let centered = work_area.clamp_origin(work_area.center_size(size), size);
        win.floating_geometry.loc = centered;
        win.floating_geometry.size = size;
        win.current_geometry.loc = centered;
        win.current_geometry.size = size;
        self.layout_update_for(id, work_area)
    }

    /// Relocates `id` cleanly onto another monitor's `target_work_area`.
    pub fn move_window_to_area(
        &mut self,
        id: WindowId,
        target_work_area: Rect,
    ) -> Option<WindowLayoutUpdate> {
        let is_floating = self.windows.get(&id)?.mode.is_floating();
        if is_floating {
            self.center_window(id, target_work_area)
        } else {
            self.sync_geometry_for_mode(id, target_work_area);
            self.layout_update_for(id, target_work_area)
        }
    }

    pub fn minimize(&mut self, id: WindowId, work_area: Rect) -> Option<WindowLayoutUpdate> {
        let ws_id = {
            let win = self.windows.get_mut(&id)?;
            if win.mode.is_floating() {
                win.floating_geometry = win.current_geometry;
            }
            if !win.mode.minimize() {
                return None;
            }
            win.focused = false;
            win.workspace_id
        };
        let stack = self.minimized_history.entry(ws_id).or_default();
        stack.retain(|&w| w != id);
        stack.push(id);
        self.layout_update_for(id, work_area)
    }

    pub fn restore_window(&mut self, id: WindowId, work_area: Rect) -> Option<WindowLayoutUpdate> {
        let ws_id = {
            let win = self.windows.get_mut(&id)?;
            if !win.mode.restore() {
                return None;
            }
            win.workspace_id
        };
        if let Some(stack) = self.minimized_history.get_mut(&ws_id) {
            stack.retain(|&w| w != id);
        }
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
    }

    /// Restores the most recently minimized window on the active workspace, if any.
    pub fn restore_last_minimized(&mut self, work_area: Rect) -> Option<WindowLayoutUpdate> {
        let active_ws = self.active_workspace_id();
        let candidate = self
            .minimized_history
            .get(&active_ws)
            .and_then(|stack| stack.last().copied())?;
        self.restore_window(candidate, work_area)
    }

    /// Marks the given window focused and updates its workspace's MRU stack.
    pub fn mark_focused(&mut self, focused_id: Option<WindowId>) {
        for win in self.windows.values_mut() {
            win.focused = Some(win.id) == focused_id;
        }
        if let Some(id) = focused_id
            && let Some(ws_id) = self.windows.get(&id).map(|w| w.workspace_id)
        {
            let stack = self.focus_history.entry(ws_id).or_default();
            stack.retain(|&entry| entry != id);
            stack.push(id);
        }
    }

    pub fn focused_id(&self) -> Option<WindowId> {
        self.windows
            .values()
            .find(|win| win.focused)
            .map(|win| win.id)
    }

    /// Returns the most-recently focused visible (non-minimized) window on the active workspace.
    pub fn active_mru_window(&self) -> Option<WindowId> {
        let active_ws = self.active_workspace_id();
        if let Some(stack) = self.focus_history.get(&active_ws)
            && let Some(id) = stack.iter().rev().copied().find(|id| {
                self.windows
                    .get(id)
                    .map(|w| w.is_visible_on(active_ws))
                    .unwrap_or(false)
            })
        {
            return Some(id);
        }
        self.active_visible_windows().into_iter().last()
    }

    /// Returns the next visible window on the active workspace for `Alt + Tab` cycling.
    pub fn next_cycle_target(&self) -> Option<WindowId> {
        let active_ws = self.active_workspace_id();
        let visible_stack: Vec<WindowId> = self
            .focus_history
            .get(&active_ws)
            .map(|stack| {
                stack
                    .iter()
                    .rev()
                    .copied()
                    .filter(|id| {
                        self.windows
                            .get(id)
                            .map(|w| w.is_visible_on(active_ws))
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();

        if visible_stack.len() >= 2 {
            Some(visible_stack[1])
        } else {
            visible_stack.first().copied()
        }
    }

    /// Switches active workspace to `index`, returning `Some(new_workspace_id)` if changed.
    pub fn switch_workspace_index(&mut self, index: usize) -> Option<WorkspaceId> {
        self.workspaces.switch_to_index(index)
    }

    /// Switches active workspace to `index`, automatically creating any missing desktops up to
    /// `MAX_WORKSPACES` (10) so shortcuts like `Super+1..4` and `Super+Numpad` always work.
    pub fn switch_workspace_index_or_create(&mut self, index: usize) -> Option<WorkspaceId> {
        self.workspaces.switch_to_index_or_create(index)
    }

    /// Toggles the pinned persistence state of the workspace at `index`.
    pub fn toggle_pin_workspace(&mut self, index: usize) -> Option<bool> {
        self.workspaces.toggle_pin(index)
    }

    pub fn switch_workspace_next(&mut self) -> Option<WorkspaceId> {
        self.workspaces.switch_next()
    }

    pub fn switch_workspace_prev(&mut self) -> Option<WorkspaceId> {
        self.workspaces.switch_prev()
    }

    /// Adds a new desktop (up to 10 total) and returns its `(WorkspaceId, index)`.
    pub fn add_workspace(&mut self) -> Option<(WorkspaceId, usize)> {
        self.workspaces.add_workspace()
    }

    /// Removes the desktop at `index`, relocating any windows cleanly to an adjacent desktop.
    pub fn remove_workspace(&mut self, index: usize) -> bool {
        let removed_ws_id = self.workspaces.workspaces().get(index).map(|w| w.id);
        if !self.workspaces.remove_workspace(index) {
            return false;
        }
        // Keep per-window `workspace_id` metadata synchronized with WorkspaceManager.
        for (&wid, win) in &mut self.windows {
            if let Some(new_ws) = self.workspaces.workspace_of_window(wid) {
                win.workspace_id = new_ws;
            }
        }
        if let Some(old_id) = removed_ws_id {
            self.focus_history.remove(&old_id);
            self.minimized_history.remove(&old_id);
            self.tile_order.remove(&old_id);
            self.tiling_split_ratios.remove(&old_id);
            self.tiling_split_flips.remove(&old_id);
        }
        true
    }

    /// Moves `id` directly to the workspace at `target_index` without intermediate hops.
    pub fn move_window_to_workspace_index(
        &mut self,
        id: WindowId,
        target_index: usize,
        follow: bool,
    ) -> Option<WorkspaceId> {
        let old_ws = self.windows.get(&id)?.workspace_id;
        let new_ws = self.workspaces.move_window_to_index(id, target_index, follow)?;
        if let Some(win) = self.windows.get_mut(&id) {
            win.workspace_id = new_ws;
        }
        self.transfer_focus_entry(id, old_ws, new_ws);
        Some(new_ws)
    }

    /// Moves `id` to the next workspace (dynamically appending one if needed).
    pub fn move_window_to_next_workspace(
        &mut self,
        id: WindowId,
        follow: bool,
    ) -> Option<WorkspaceId> {
        let old_ws = self.windows.get(&id)?.workspace_id;
        let new_ws = self.workspaces.move_window_next(id, follow)?;
        if let Some(win) = self.windows.get_mut(&id) {
            win.workspace_id = new_ws;
        }
        self.transfer_focus_entry(id, old_ws, new_ws);
        Some(new_ws)
    }

    /// Moves `id` to the previous workspace, if one exists.
    pub fn move_window_to_prev_workspace(
        &mut self,
        id: WindowId,
        follow: bool,
    ) -> Option<WorkspaceId> {
        let old_ws = self.windows.get(&id)?.workspace_id;
        let new_ws = self.workspaces.move_window_prev(id, follow)?;
        if let Some(win) = self.windows.get_mut(&id) {
            win.workspace_id = new_ws;
        }
        self.transfer_focus_entry(id, old_ws, new_ws);
        Some(new_ws)
    }

    fn transfer_focus_entry(&mut self, id: WindowId, old_ws: WorkspaceId, new_ws: WorkspaceId) {
        if let Some(stack) = self.focus_history.get_mut(&old_ws) {
            stack.retain(|&w| w != id);
        }
        let target = self.focus_history.entry(new_ws).or_default();
        target.retain(|&w| w != id);
        target.push(id);

        if let Some(min_stack) = self.minimized_history.get_mut(&old_ws) {
            let was_min = min_stack.contains(&id);
            min_stack.retain(|&w| w != id);
            if was_min {
                self.minimized_history.entry(new_ws).or_default().push(id);
            }
        }
        if let Some(order) = self.tile_order.get_mut(&old_ws) {
            order.retain(|&w| w != id);
        }
        let target_order = self.tile_order.entry(new_ws).or_default();
        if !target_order.contains(&id) {
            target_order.push(id);
        }
    }

    fn sync_geometry_for_mode(&mut self, id: WindowId, work_area: Rect) {
        let snap_gap = self.snap_gap;
        let Some(win) = self.windows.get_mut(&id) else {
            return;
        };

        match &win.mode {
            WindowMode::Floating => {
                win.current_geometry = win.floating_geometry;
            }
            WindowMode::Tiled => {}
            WindowMode::Snapped(region) => {
                win.current_geometry = region.target_rect(work_area, snap_gap);
            }
            WindowMode::Maximized | WindowMode::Fullscreen => {
                win.current_geometry = work_area;
            }
            WindowMode::Minimized { .. } => {}
        }
    }

    fn layout_update_for(&self, id: WindowId, work_area: Rect) -> Option<WindowLayoutUpdate> {
        let win = self.windows.get(&id)?;
        let active_ws = self.active_workspace_id();
        let visible = win.is_visible_on(active_ws);

        let (target_location, configure_size) = match &win.mode {
            WindowMode::Floating => {
                let sz = if win.floating_geometry.size.is_empty() {
                    None
                } else {
                    Some(win.floating_geometry.size)
                };
                (visible.then_some(win.floating_geometry.loc), sz)
            }
            WindowMode::Tiled => {
                let sz = if win.current_geometry.size.is_empty() {
                    None
                } else {
                    Some(win.current_geometry.size)
                };
                (visible.then_some(win.current_geometry.loc), sz)
            }
            WindowMode::Snapped(region) => {
                let rect = region.target_rect(work_area, self.snap_gap);
                (visible.then_some(rect.loc), Some(rect.size))
            }
            WindowMode::Maximized | WindowMode::Fullscreen => {
                (visible.then_some(work_area.loc), Some(work_area.size))
            }
            WindowMode::Minimized { .. } => (None, None),
        };

        Some(WindowLayoutUpdate {
            id,
            mode: win.mode.clone(),
            target_location,
            configure_size,
            visible_on_active_workspace: visible,
        })
    }

    /// Intelligent GNOME/KDE-style center-first floating placement:
    /// 1. Transient dialogs center cleanly over their parent window (clamped to `work_area`).
    /// 2. Standalone floating windows open centered in `work_area`, cascading gently outward
    ///    only when another visible floating window already occupies that origin.
    fn compute_floating_placement(
        &mut self,
        work_area: Rect,
        assumed_size: Size,
        parent: Option<WindowId>,
        center_on_open: bool,
    ) -> Point {
        self.compute_floating_placement_excluding(
            work_area,
            assumed_size,
            parent,
            center_on_open,
            None,
        )
    }

    fn compute_floating_placement_excluding(
        &mut self,
        work_area: Rect,
        assumed_size: Size,
        parent: Option<WindowId>,
        center_on_open: bool,
        exclude_id: Option<WindowId>,
    ) -> Point {
        if let Some(pid) = parent
            && let Some(parent_win) = self.windows.get(&pid)
        {
            let centered = parent_win.current_geometry.center_size(assumed_size);
            return work_area.clamp_origin(centered, assumed_size);
        }

        let center = work_area.clamp_origin(work_area.center_size(assumed_size), assumed_size);
        if center_on_open {
            return center;
        }

        const OFFSETS: [(i32, i32); 9] = [
            (0, 0),
            (32, 28),
            (-32, -28),
            (64, 56),
            (-64, -56),
            (96, 84),
            (-96, -84),
            (128, 112),
            (-128, -112),
        ];

        let active_ws = self.active_workspace_id();
        let occupied: Vec<Point> = self
            .windows
            .values()
            .filter(|w| {
                Some(w.id) != exclude_id
                    && w.mapped
                    && w.is_visible_on(active_ws)
                    && w.mode.is_floating()
            })
            .map(|w| w.current_geometry.loc)
            .collect();

        for (dx, dy) in OFFSETS {
            let candidate = work_area.clamp_origin(
                Point::new(center.x + dx, center.y + dy),
                assumed_size,
            );
            let collides = occupied
                .iter()
                .any(|p| (p.x - candidate.x).abs() < 16 && (p.y - candidate.y).abs() < 16);
            if !collides {
                return candidate;
            }
        }

        let step = self.cascade_index % (OFFSETS.len() as i32);
        self.cascade_index = (self.cascade_index + 1) % (OFFSETS.len() as i32);
        let (dx, dy) = OFFSETS[step as usize];
        work_area.clamp_origin(Point::new(center.x + dx, center.y + dy), assumed_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::SnapRegion;
    use crate::rules::{WindowMatcher, WindowRuleAction};

    #[test]
    fn transient_dialog_centers_on_parent_and_inherits_workspace() {
        let mut wm = WindowManager::default();
        let work_area = Rect::from_size(Size::new(1280, 800));

        let parent = wm.register_window(work_area, None);
        wm.update_committed_size(parent.id, Size::new(800, 600), work_area);

        // Move parent to workspace 1 (dynamic) and follow
        let ws1 = wm.move_window_to_next_workspace(parent.id, true).unwrap();

        let dialog = wm.register_window(work_area, Some(parent.id));
        assert_eq!(dialog.workspace_id, ws1);

        let recentered = wm
            .update_committed_size(dialog.id, Size::new(400, 200), work_area)
            .expect("dialog should recenter once size is committed");
        let p_loc = wm.get(parent.id).unwrap().current_geometry.loc;
        assert_eq!(recentered, Point::new(p_loc.x + 200, p_loc.y + 200));
    }

    #[test]
    fn center_size_handles_larger_child_and_standalone_windows_center_first() {
        let work_area = Rect::new(Point::new(0, 31), Size::new(1920, 969));
        let splash = Rect::new(Point::new(405, 273), Size::new(1110, 490));
        let proj_mgr_size = Size::new(1350, 768);

        // Centering a larger window over a smaller anchor rect must not pin to anchor top-left.
        let over_splash = work_area.clamp_origin(splash.center_size(proj_mgr_size), proj_mgr_size);
        assert_eq!(over_splash.x, 285);

        // First standalone floating window centers cleanly inside work_area once size is committed.
        let mut wm = WindowManager::default();
        let w1 = wm.register_window(work_area, None);
        let loc1 = wm
            .update_committed_size(w1.id, proj_mgr_size, work_area)
            .expect("first standalone window should center on commit");
        assert_eq!(loc1, Point::new(285, 131));

        // Second standalone floating window cascades cleanly from center without colliding.
        let w2 = wm.register_window(work_area, None);
        let loc2 = wm
            .update_committed_size(w2.id, proj_mgr_size, work_area)
            .expect("second standalone window should cascade from center");
        assert_ne!(loc1, loc2);
    }

    #[test]
    fn minimize_and_restore_preserves_prior_snapped_mode_and_focus() {
        let mut wm = WindowManager::default();
        let work_area = Rect::from_size(Size::new(1280, 800));

        let w1 = wm.register_window(work_area, None);
        let w2 = wm.register_window(work_area, None);
        wm.update_committed_size(w1.id, Size::new(500, 400), work_area);
        wm.update_committed_size(w2.id, Size::new(500, 400), work_area);

        wm.mark_focused(Some(w1.id));
        wm.mark_focused(Some(w2.id));

        let snap = wm
            .toggle_snap(w2.id, SnapRegion::RightHalf, work_area)
            .unwrap();
        assert_eq!(snap.target_location, Some(Point::new(640, 0)));
        assert_eq!(snap.configure_size, Some(Size::new(640, 800)));

        let min = wm.minimize(w2.id, work_area).unwrap();
        assert!(!min.visible_on_active_workspace);
        assert_eq!(wm.active_mru_window(), Some(w1.id));

        let restored = wm.restore_last_minimized(work_area).unwrap();
        assert_eq!(restored.id, w2.id);
        assert_eq!(restored.mode, WindowMode::Snapped(SnapRegion::RightHalf));
        assert_eq!(restored.target_location, Some(Point::new(640, 0)));
    }

    #[test]
    fn window_rules_apply_initial_mode_and_workspace() {
        let mut wm = WindowManager::default();
        let work_area = Rect::from_size(Size::new(1280, 800));
        wm.add_rule(WindowRule {
            name: "max-editor".into(),
            matcher: WindowMatcher {
                app_id_exact: Some("org.neonde.editor".into()),
                ..Default::default()
            },
            action: WindowRuleAction {
                initial_mode: Some(WindowMode::Maximized),
                ..Default::default()
            },
        });

        let w = wm.register_window(work_area, None);
        wm.get_mut(w.id).unwrap().app_id = Some("org.neonde.editor".into());

        let (eval, update) = wm
            .apply_rules_if_needed(w.id, work_area)
            .expect("rule should match");
        assert_eq!(eval.matched_names, vec!["max-editor"]);
        assert_eq!(update.mode, WindowMode::Maximized);
        assert_eq!(update.configure_size, Some(Size::new(1280, 800)));
    }

    #[test]
    fn always_on_top_center_and_move_to_monitor_work_cleanly() {
        let mut wm = WindowManager::default();
        let primary = Rect::new(Point::new(0, 31), Size::new(1920, 969));
        let secondary = Rect::new(Point::new(1920, 0), Size::new(1280, 720));
        let w1 = wm.register_window(primary, None);
        let w2 = wm.register_window(primary, None);
        wm.update_committed_size(w1.id, Size::new(800, 500), primary);
        wm.update_committed_size(w2.id, Size::new(700, 450), primary);

        // Pin w1 always-on-top, then focus w2: w1 must stay above w2 in stacking order!
        assert_eq!(wm.toggle_always_on_top(w1.id), Some(true));
        wm.mark_focused(Some(w2.id));
        let order = wm.active_visible_windows_mru_order();
        assert_eq!(order, vec![w2.id, w1.id]);

        // Snap w1 then center it: must restore to floating and center geometrically.
        wm.toggle_snap(w1.id, geometry::SnapRegion::LeftHalf, primary);
        let centered = wm.center_window(w1.id, primary).expect("center update");
        assert!(centered.mode.is_floating());
        assert_eq!(centered.target_location, Some(Point::new(560, 265)));

        // Move w1 onto secondary monitor: centers cleanly inside secondary work area.
        let moved = wm
            .move_window_to_area(w1.id, secondary)
            .expect("move to secondary monitor");
        assert_eq!(moved.target_location, Some(Point::new(1920 + 240, 110)));
    }

    #[test]
    fn direct_move_window_to_workspace_index_moves_cleanly_across_desks() {
        let mut wm = WindowManager::default();
        let area = Rect::new(Point::new(0, 31), Size::new(1280, 769));
        let w1 = wm.register_window(area, None);
        let w2 = wm.register_window(area, None);
        // Registering w1 dynamically adds a trailing empty Desk 2 (index 1), so adding 3 more
        // explicit desks creates indices 2, 3, 4 (Desks 3, 4, 5).
        let (_, idx3) = wm.add_workspace().unwrap();
        let (_, idx4) = wm.add_workspace().unwrap();
        let (ws5_id, idx5) = wm.add_workspace().unwrap();
        assert_eq!((idx3, idx4, idx5), (2, 3, 4));

        // Drag w2 directly from Desk 1 (index 0) to Desk 5 (index 4) without following
        let moved_ws = wm
            .move_window_to_workspace_index(w2.id, idx5, false)
            .expect("should move directly to Desk 5");
        assert_eq!(moved_ws, ws5_id);
        assert_eq!(wm.get(w2.id).unwrap().workspace_id, ws5_id);
        assert_eq!(wm.workspaces().active_index(), 0);
        assert_eq!(wm.get(w1.id).unwrap().workspace_id, wm.active_workspace_id());
    }

    #[test]
    fn hyprland_auto_tiling_tiles_swaps_floats_and_restores_cleanly() {
        let mut wm = WindowManager::default();
        wm.set_tiling_gap(10);
        let area = Rect::new(Point::new(0, 30), Size::new(1280, 720));

        let w1 = wm.register_window(area, None);
        let w2 = wm.register_window(area, None);
        let w3 = wm.register_window(area, None);
        wm.update_committed_size(w1.id, Size::new(700, 500), area);
        wm.update_committed_size(w2.id, Size::new(640, 480), area);
        wm.update_committed_size(w3.id, Size::new(600, 460), area);
        let orig_loc1 = wm.get(w1.id).unwrap().floating_geometry.loc;

        // Enable Auto-Tiling (Dwindle default)
        let ups = wm.set_auto_tiling(true, area);
        assert_eq!(ups.len(), 3);
        assert!(wm.get(w1.id).unwrap().mode.is_tiled());
        assert_eq!(wm.get(w1.id).unwrap().current_geometry.size, Size::new(625, 700));
        assert_eq!(wm.get(w2.id).unwrap().current_geometry.size, Size::new(625, 345));
        assert_eq!(wm.get(w3.id).unwrap().current_geometry.size, Size::new(625, 345));

        // Swap w1 and w2 cleanly
        let swapped = wm.swap_tiled_windows(w1.id, w2.id, area);
        assert_eq!(swapped.len(), 3);
        assert_eq!(wm.get(w2.id).unwrap().current_geometry.loc, Point::new(10, 40));

        // Toggle floating override on w3 -> w1 & w2 reflow to 50/50 split!
        let _ = wm.toggle_window_float(w3.id, area);
        assert!(wm.get(w3.id).unwrap().mode.is_floating());
        assert_eq!(wm.get(w1.id).unwrap().current_geometry.size, Size::new(625, 700));
        assert_eq!(wm.get(w2.id).unwrap().current_geometry.size, Size::new(625, 700));

        // Switch to Master-Stack layout
        let _ = wm.set_tiling_layout(TilingLayout::MasterStack, area);
        assert!(wm.get(w2.id).unwrap().current_geometry.size.w > wm.get(w1.id).unwrap().current_geometry.size.w);

        // Disable Auto-Tiling -> tiled windows return to saved floating geometry cleanly!
        let restored = wm.set_auto_tiling(false, area);
        assert_eq!(restored.len(), 2);
        assert!(wm.get(w1.id).unwrap().mode.is_floating());
        assert_eq!(wm.get(w1.id).unwrap().current_geometry.loc, orig_loc1);
    }

    #[test]
    fn tiling_maximize_unmaximize_resize_focus_and_z_order_invariants() {
        let mut wm = WindowManager::default();
        assert_eq!(wm.tiling_gap(), 6);
        let area = Rect::new(Point::new(0, 0), Size::new(1920, 1032));

        let w1 = wm.register_window(area, None);
        let w2 = wm.register_window(area, None);
        let w3 = wm.register_window(area, None);
        let _ = wm.set_auto_tiling(true, area);
        let ws = wm.active_workspace_id();

        // Client committing an unrelated buffer size must NOT corrupt the WM's tiled rect!
        let expected_w1_size = wm.get(w1.id).unwrap().current_geometry.size;
        wm.update_committed_size(w1.id, Size::new(500, 400), area);
        assert_eq!(wm.get(w1.id).unwrap().current_geometry.size, expected_w1_size);

        // Directional focus: Right from w1 lands on w2 or w3; Down from w2 lands on w3.
        let right = wm.focus_tiled_in_direction(w1.id, 1, 0);
        assert!(right == Some(w2.id) || right == Some(w3.id));
        assert_eq!(wm.focus_tiled_in_direction(w2.id, 0, 1), Some(w3.id));
        assert_eq!(wm.focus_tiled_in_direction(w3.id, 0, -1), Some(w2.id));

        // Maximizing w1 removes it from tiling eligibility so reflow tiles w2 & w3 cleanly side-by-side!
        let _ = wm.set_maximized(w1.id, true, area);
        let reflow = wm.relayout_workspace_tiles(ws, area);
        assert_eq!(reflow.len(), 2);
        assert!(wm.get(w1.id).unwrap().mode.is_maximized());
        assert_eq!(wm.get(w2.id).unwrap().current_geometry.loc.x, 6);

        // Unmaximizing w1 returns it directly to Tiled in its original slot!
        let unmax = wm.set_maximized(w1.id, false, area).unwrap();
        assert!(unmax.mode.is_tiled());
        let reflow2 = wm.relayout_workspace_tiles(ws, area);
        assert_eq!(reflow2.len(), 3);
        assert_eq!(wm.get(w1.id).unwrap().current_geometry.loc.x, 6);

        // Resizing w1 horizontally widens w1 and shrinks w2/w3 while keeping exact 6px gap!
        let before_w = wm.get(w1.id).unwrap().current_geometry.size.w;
        let _ = wm.resize_tiled_window(w1.id, 0.10, 0.0, area);
        let after_w = wm.get(w1.id).unwrap().current_geometry.size.w;
        assert!(after_w > before_w + 150);
        assert_eq!(
            wm.get(w1.id).unwrap().current_geometry.loc.x + after_w + 6,
            wm.get(w2.id).unwrap().current_geometry.loc.x
        );

        // Floating window stays stacked above tiled windows even after focusing a tiled window!
        let _ = wm.toggle_window_float(w3.id, area);
        wm.mark_focused(Some(w1.id));
        let z_order = wm.active_visible_windows_mru_order();
        assert_eq!(z_order.last(), Some(&w3.id));

        // Minimizing a tiled window then disabling Auto-Tiling clears its restore mode to Floating!
        let _ = wm.minimize(w2.id, area);
        let _ = wm.set_auto_tiling(false, area);
        let restored_min = wm.restore_window(w2.id, area).unwrap();
        assert!(restored_min.mode.is_floating());
    }
}
