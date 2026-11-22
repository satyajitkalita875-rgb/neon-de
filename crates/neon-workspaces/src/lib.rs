//! GNOME-inspired dynamic workspace management for **NEON DE**.
//!
//! # Dynamic Workspace Invariants
//! 1. At least one workspace always exists.
//! 2. Whenever the trailing (last) workspace becomes non-empty, a fresh empty workspace is
//!    appended automatically so users always have a clean workspace available.
//! 3. Empty intermediate workspaces that are not active are pruned automatically while
//!    preserving the predictable ordering of all remaining workspaces.
//! 4. The active workspace is never pruned while active; if the user leaves an empty
//!    non-trailing workspace, it is pruned once focus moves away.

use std::collections::HashMap;

use tracing::debug;

/// Stable monotonic identifier for a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub u64);

/// Generic opaque window key used by [`WorkspaceManager`].
///
/// Matches `neon_window_manager::WindowId` via `u64` raw IDs or generic type parameter,
/// keeping `neon-workspaces` decoupled from compositor/Wayland types.
#[derive(Debug, Clone)]
pub struct Workspace<W> {
    pub id: WorkspaceId,
    pub name: Option<String>,
    /// Windows assigned to this workspace in insertion / stacking order.
    windows: Vec<W>,
}

impl<W: Copy + PartialEq> Workspace<W> {
    fn new(id: WorkspaceId) -> Self {
        Self {
            id,
            name: None,
            windows: Vec::new(),
        }
    }

    pub fn windows(&self) -> &[W] {
        &self.windows
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn contains(&self, window: W) -> bool {
        self.windows.contains(&window)
    }
}

/// Configuration for workspace lifecycle behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspacePolicy {
    /// Automatically append an empty trailing workspace and prune unused empty workspaces.
    pub dynamic: bool,
    /// Minimum number of workspaces when `dynamic == false` (ignored when dynamic, which uses 1).
    pub static_count: usize,
}

impl Default for WorkspacePolicy {
    fn default() -> Self {
        Self {
            dynamic: true,
            static_count: 4,
        }
    }
}

/// Manages ordered workspaces and window-to-workspace membership.
#[derive(Debug)]
pub struct WorkspaceManager<W: Copy + Eq + std::hash::Hash + std::fmt::Debug> {
    next_id: u64,
    policy: WorkspacePolicy,
    workspaces: Vec<Workspace<W>>,
    active_index: usize,
    window_index: HashMap<W, WorkspaceId>,
}

impl<W: Copy + Eq + std::hash::Hash + std::fmt::Debug> Default for WorkspaceManager<W> {
    fn default() -> Self {
        Self::new(WorkspacePolicy::default())
    }
}

impl<W: Copy + Eq + std::hash::Hash + std::fmt::Debug> WorkspaceManager<W> {
    pub fn new(policy: WorkspacePolicy) -> Self {
        let initial_count = if policy.dynamic {
            1
        } else {
            policy.static_count.max(1)
        };

        let mut manager = Self {
            next_id: 0,
            policy,
            workspaces: Vec::with_capacity(initial_count),
            active_index: 0,
            window_index: HashMap::new(),
        };

        for _ in 0..initial_count {
            manager.push_new_workspace();
        }

        manager
    }

    pub fn policy(&self) -> WorkspacePolicy {
        self.policy
    }

    pub fn workspaces(&self) -> &[Workspace<W>] {
        &self.workspaces
    }

    pub fn len(&self) -> usize {
        self.workspaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
    }

    pub fn active_index(&self) -> usize {
        self.active_index
    }

    pub fn active_workspace(&self) -> &Workspace<W> {
        &self.workspaces[self.active_index]
    }

    pub fn active_workspace_id(&self) -> WorkspaceId {
        self.workspaces[self.active_index].id
    }

    pub fn index_of(&self, id: WorkspaceId) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.id == id)
    }

    pub fn get(&self, id: WorkspaceId) -> Option<&Workspace<W>> {
        self.workspaces.iter().find(|ws| ws.id == id)
    }

    pub fn workspace_of_window(&self, window: W) -> Option<WorkspaceId> {
        self.window_index.get(&window).copied()
    }

    pub fn is_window_on_active(&self, window: W) -> bool {
        self.workspace_of_window(window) == Some(self.active_workspace_id())
    }

    pub fn windows_on(&self, id: WorkspaceId) -> &[W] {
        self.get(id).map(Workspace::windows).unwrap_or(&[])
    }

    pub fn active_windows(&self) -> &[W] {
        self.active_workspace().windows()
    }

    /// Assigns a new window to the active workspace (or specified workspace index if valid).
    pub fn assign_window(&mut self, window: W, target_index: Option<usize>) -> WorkspaceId {
        // Remove any stale assignment first.
        if self.window_index.contains_key(&window) {
            self.remove_window(window);
        }

        let idx = target_index
            .filter(|&i| i < self.workspaces.len())
            .unwrap_or(self.active_index);

        let ws_id = self.workspaces[idx].id;
        self.workspaces[idx].windows.push(window);
        self.window_index.insert(window, ws_id);

        debug!(?window, ?ws_id, index = idx, "Assigned window to workspace");
        self.reconcile_dynamic_workspaces();
        ws_id
    }

    /// Removes a window from whichever workspace currently owns it.
    pub fn remove_window(&mut self, window: W) -> Option<WorkspaceId> {
        let ws_id = self.window_index.remove(&window)?;
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.windows.retain(|&w| w != window);
        }
        debug!(?window, ?ws_id, "Removed window from workspace");
        self.reconcile_dynamic_workspaces();
        Some(ws_id)
    }

    /// Switches active workspace to the given zero-based index.
    ///
    /// Returns `Some(new_id)` if the active workspace actually changed.
    pub fn switch_to_index(&mut self, target_index: usize) -> Option<WorkspaceId> {
        if target_index >= self.workspaces.len() || target_index == self.active_index {
            return None;
        }
        let target_id = self.workspaces[target_index].id;
        self.active_index = target_index;
        self.reconcile_dynamic_workspaces();
        // Re-resolve active_index in case pruning shifted indices.
        if let Some(new_idx) = self.index_of(target_id) {
            self.active_index = new_idx;
        }
        let active_id = self.active_workspace_id();
        debug!(?active_id, index = self.active_index, "Switched workspace");
        Some(active_id)
    }

    /// Switches to the workspace with the given [`WorkspaceId`].
    pub fn switch_to_id(&mut self, id: WorkspaceId) -> Option<WorkspaceId> {
        let idx = self.index_of(id)?;
        self.switch_to_index(idx)
    }

    /// Switches to the next workspace to the right, if one exists.
    pub fn switch_next(&mut self) -> Option<WorkspaceId> {
        let next = self.active_index.checked_add(1)?;
        self.switch_to_index(next)
    }

    /// Switches to the previous workspace to the left, if one exists.
    pub fn switch_prev(&mut self) -> Option<WorkspaceId> {
        let prev = self.active_index.checked_sub(1)?;
        self.switch_to_index(prev)
    }

    /// Moves a window to the workspace at `target_index`, optionally following focus there.
    pub fn move_window_to_index(
        &mut self,
        window: W,
        target_index: usize,
        follow: bool,
    ) -> Option<WorkspaceId> {
        if target_index >= self.workspaces.len() {
            return None;
        }
        let target_id = self.workspaces[target_index].id;
        if self.workspace_of_window(window) == Some(target_id) {
            return None;
        }

        if let Some(old_id) = self.window_index.get(&window).copied()
            && let Some(old_ws) = self.workspaces.iter_mut().find(|ws| ws.id == old_id)
        {
            old_ws.windows.retain(|&w| w != window);
        }

        self.workspaces[target_index].windows.push(window);
        self.window_index.insert(window, target_id);

        if follow {
            self.active_index = target_index;
        }

        self.reconcile_dynamic_workspaces();
        if follow && let Some(new_idx) = self.index_of(target_id) {
            self.active_index = new_idx;
        }

        debug!(
            ?window,
            ?target_id,
            follow,
            "Moved window to target workspace"
        );
        Some(target_id)
    }

    /// Moves a window to the next workspace (creating one if dynamic and needed).
    pub fn move_window_next(&mut self, window: W, follow: bool) -> Option<WorkspaceId> {
        let current_idx = self
            .workspace_of_window(window)
            .and_then(|id| self.index_of(id))
            .unwrap_or(self.active_index);

        let target_idx = current_idx + 1;
        if target_idx >= self.workspaces.len() {
            if !self.policy.dynamic {
                return None;
            }
            self.push_new_workspace();
        }
        self.move_window_to_index(window, target_idx, follow)
    }

    /// Moves a window to the previous workspace if one exists.
    pub fn move_window_prev(&mut self, window: W, follow: bool) -> Option<WorkspaceId> {
        let current_idx = self
            .workspace_of_window(window)
            .and_then(|id| self.index_of(id))
            .unwrap_or(self.active_index);

        let target_idx = current_idx.checked_sub(1)?;
        self.move_window_to_index(window, target_idx, follow)
    }

    fn push_new_workspace(&mut self) -> WorkspaceId {
        self.next_id += 1;
        let id = WorkspaceId(self.next_id);
        self.workspaces.push(Workspace::new(id));
        id
    }

    /// Enforces GNOME-style dynamic workspace invariants without dropping the active workspace.
    fn reconcile_dynamic_workspaces(&mut self) {
        if !self.policy.dynamic {
            return;
        }

        let active_id = self.workspaces[self.active_index].id;

        // 1. Always ensure the trailing workspace is empty so the user can move into a new one.
        if self
            .workspaces
            .last()
            .map(|ws| !ws.is_empty())
            .unwrap_or(true)
        {
            self.push_new_workspace();
        }

        // 2. Remove empty workspaces that are neither the active workspace nor the single trailing
        //    empty workspace. Also collapse multiple consecutive trailing empty workspaces (unless active).
        let mut idx = 0;
        while idx + 1 < self.workspaces.len() {
            let is_active = self.workspaces[idx].id == active_id;
            let is_empty = self.workspaces[idx].is_empty();
            if is_empty && !is_active {
                let removed = self.workspaces.remove(idx);
                debug!(workspace = ?removed.id, "Pruned empty dynamic workspace");
                continue;
            }
            idx += 1;
        }

        // 3. Recompute active_index cleanly.
        self.active_index = self
            .index_of(active_id)
            .unwrap_or_else(|| self.workspaces.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_workspace_creates_trailing_empty_and_prunes_unused() {
        let mut mgr: WorkspaceManager<u64> = WorkspaceManager::default();
        assert_eq!(mgr.len(), 1);
        assert_eq!(mgr.active_index(), 0);

        // Assigning window 1 to workspace 0 automatically appends empty workspace 1.
        let ws0 = mgr.assign_window(1, None);
        assert_eq!(mgr.len(), 2);
        assert_eq!(mgr.active_workspace_id(), ws0);
        assert!(mgr.workspaces()[1].is_empty());

        // Switching to workspace 1 and adding window 2 appends workspace 2.
        let ws1 = mgr.switch_next().expect("workspace 1 exists");
        assert_eq!(mgr.active_index(), 1);
        mgr.assign_window(2, None);
        assert_eq!(mgr.len(), 3);
        assert_eq!(mgr.workspace_of_window(2), Some(ws1));
        assert!(mgr.workspaces()[2].is_empty());

        // Removing window 1 from non-active workspace 0 prunes workspace 0 cleanly!
        mgr.remove_window(1);
        assert_eq!(mgr.len(), 2);
        assert_eq!(mgr.active_workspace_id(), ws1);
        assert_eq!(mgr.active_index(), 0);
        assert_eq!(mgr.active_windows(), &[2]);
    }

    #[test]
    fn active_empty_workspace_stays_until_user_leaves_it() {
        let mut mgr: WorkspaceManager<u64> = WorkspaceManager::default();
        mgr.assign_window(10, None);
        mgr.switch_next();
        mgr.assign_window(20, None);
        // Workspaces: [10], [20] (active), []
        assert_eq!(mgr.len(), 3);

        // Remove window 20 while workspace 1 is active -> workspace 1 remains active (not yanked away)
        mgr.remove_window(20);
        assert_eq!(mgr.active_index(), 1);
        assert!(mgr.active_windows().is_empty());

        // Switch back to workspace 0 -> now empty intermediate workspace gets pruned!
        mgr.switch_prev();
        assert_eq!(mgr.active_index(), 0);
        assert_eq!(mgr.len(), 2);
        assert_eq!(mgr.workspaces()[0].windows(), &[10]);
        assert!(mgr.workspaces()[1].is_empty());
    }

    #[test]
    fn moving_window_across_workspaces_updates_membership_and_follow() {
        let mut mgr: WorkspaceManager<u64> = WorkspaceManager::default();
        let ws0 = mgr.assign_window(1, None);
        mgr.assign_window(2, None);
        assert_eq!(mgr.len(), 2);

        // Move window 2 to next workspace and follow it
        let ws1 = mgr.move_window_next(2, true).expect("moved to ws1");
        assert_ne!(ws0, ws1);
        assert_eq!(mgr.active_workspace_id(), ws1);
        assert_eq!(mgr.workspace_of_window(1), Some(ws0));
        assert_eq!(mgr.workspace_of_window(2), Some(ws1));
        assert_eq!(mgr.len(), 3); // [1], [2], []

        // Move window 2 back to previous without following
        let back = mgr.move_window_prev(2, false).expect("moved back to ws0");
        assert_eq!(back, ws0);
        assert_eq!(mgr.workspace_of_window(2), Some(ws0));
        assert_eq!(mgr.active_workspace_id(), ws1);
    }
}
