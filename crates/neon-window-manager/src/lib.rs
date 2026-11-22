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

use std::collections::HashMap;

use neon_workspaces::{WorkspaceId, WorkspaceManager, WorkspacePolicy};
use tracing::debug;

use crate::{
    geometry::{Point, Rect, Size},
    rules::{EvaluatedRules, WindowRule, WindowRulesEngine},
    state_machine::WindowMode,
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
    rules: WindowRulesEngine,
    focus_policy: FocusPolicy,
    snap_gap: i32,
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
            rules: WindowRulesEngine::with_defaults(),
            focus_policy: FocusPolicy::ClickToFocus,
            snap_gap: 0,
            cascade_index: 0,
        }
    }

    pub fn focus_policy(&self) -> FocusPolicy {
        self.focus_policy
    }

    pub fn set_focus_policy(&mut self, policy: FocusPolicy) {
        self.focus_policy = policy;
    }

    pub fn set_snap_gap(&mut self, gap: i32) {
        self.snap_gap = gap.max(0);
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
            parent,
            rules_applied: false,
            matched_rules: Vec::new(),
        };

        self.windows.insert(id, record.clone());
        self.focus_history.entry(workspace_id).or_default().push(id);
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
            Some(work_area.center_size(initial_size))
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

    /// Records the window's committed geometry when floating.
    pub fn update_committed_size(
        &mut self,
        id: WindowId,
        size: Size,
        work_area: Rect,
    ) -> Option<Point> {
        let parent_rect = self
            .windows
            .get(&id)
            .and_then(|w| w.parent)
            .and_then(|pid| self.windows.get(&pid).map(|p| p.current_geometry));

        let win = self.windows.get_mut(&id)?;
        let was_unmapped = !win.mapped;
        win.current_geometry.size = size;
        if win.mode.is_floating() {
            win.floating_geometry.size = size;
        }
        if was_unmapped && !size.is_empty() {
            win.mapped = true;
            // Re-center dialogs precisely once their real size is committed.
            if let Some(prect) = parent_rect
                && win.mode.is_floating()
            {
                let centered = work_area.clamp_origin(prect.center_size(size), size);
                win.floating_geometry.loc = centered;
                win.current_geometry.loc = centered;
                return Some(centered);
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
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() && maximize {
            win.floating_geometry = win.current_geometry;
        }
        if !win.mode.set_maximized(maximize) {
            return self.layout_update_for(id, work_area);
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
        let win = self.windows.get_mut(&id)?;
        if win.mode.is_floating() && fullscreen {
            win.floating_geometry = win.current_geometry;
        }
        if !win.mode.set_fullscreen(fullscreen) {
            return self.layout_update_for(id, work_area);
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

    pub fn set_floating(&mut self, id: WindowId, work_area: Rect) -> Option<WindowLayoutUpdate> {
        let win = self.windows.get_mut(&id)?;
        if !win.mode.set_floating() {
            return None;
        }
        self.sync_geometry_for_mode(id, work_area);
        self.layout_update_for(id, work_area)
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

    pub fn switch_workspace_next(&mut self) -> Option<WorkspaceId> {
        self.workspaces.switch_next()
    }

    pub fn switch_workspace_prev(&mut self) -> Option<WorkspaceId> {
        self.workspaces.switch_prev()
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

    /// Intelligent floating placement:
    /// 1. Transient dialogs center cleanly over their parent window.
    /// 2. Rule-centered windows center inside the output work area.
    /// 3. General floating windows cascade with overlap avoidance inside the work area.
    fn compute_floating_placement(
        &mut self,
        work_area: Rect,
        assumed_size: Size,
        parent: Option<WindowId>,
        center_on_open: bool,
    ) -> Point {
        if let Some(pid) = parent
            && let Some(parent_win) = self.windows.get(&pid)
        {
            let centered = parent_win.current_geometry.center_size(assumed_size);
            return work_area.clamp_origin(centered, assumed_size);
        }

        if center_on_open {
            return work_area.center_size(assumed_size);
        }

        const MARGIN_X: i32 = 48;
        const MARGIN_Y: i32 = 48;
        const STEP_X: i32 = 36;
        const STEP_Y: i32 = 32;
        const MAX_STEPS: i32 = 8;

        let active_ws = self.active_workspace_id();
        let occupied: Vec<Point> = self
            .windows
            .values()
            .filter(|w| w.is_visible_on(active_ws) && w.mode.is_floating())
            .map(|w| w.current_geometry.loc)
            .collect();

        for offset in 0..MAX_STEPS {
            let step = (self.cascade_index + offset) % MAX_STEPS;
            let candidate = work_area.clamp_origin(
                Point::new(
                    work_area.loc.x + MARGIN_X + step * STEP_X,
                    work_area.loc.y + MARGIN_Y + step * STEP_Y,
                ),
                assumed_size,
            );
            let collides = occupied
                .iter()
                .any(|p| (p.x - candidate.x).abs() < 16 && (p.y - candidate.y).abs() < 16);
            if !collides {
                self.cascade_index = (step + 1) % MAX_STEPS;
                return candidate;
            }
        }

        let step = self.cascade_index % MAX_STEPS;
        self.cascade_index = (self.cascade_index + 1) % MAX_STEPS;
        work_area.clamp_origin(
            Point::new(
                work_area.loc.x + MARGIN_X + step * STEP_X,
                work_area.loc.y + MARGIN_Y + step * STEP_Y,
            ),
            assumed_size,
        )
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
}
