# NEON DE — Window Manager & Dynamic Workspace Design

## 1. Philosophy

NEON DE uses a **GNOME-inspired floating window management model** as the default workflow, combined with predictable keyboard navigation, intelligent collision-avoiding placement, dynamic workspaces, configurable window rules, and optional smart edge snapping/tiling.

## 2. Extracted Workspace Crates (Milestone 2)

- **`crates/neon-workspaces`** (`WorkspaceManager<W>`, `WorkspaceId`, `WorkspacePolicy`):
  - Maintains an ordered list of workspaces with stable `WorkspaceId(u64)`.
  - Enforces GNOME-style dynamic workspace invariants:
    1. At least one workspace always exists.
    2. Whenever the trailing workspace becomes non-empty, a fresh empty trailing workspace is appended automatically.
    3. Empty non-active intermediate workspaces are pruned automatically without disturbing remaining workspace order.
    4. An active empty workspace stays alive until the user navigates away.
- **`crates/neon-window-manager`** (`WindowManager`, `WindowId`, `ManagedWindow`, `WindowMode`, `SnapRegion`, `FocusPolicy`, `WindowRulesEngine`):
  - **Exclusive State Machine (`WindowMode`)**: Encodes `Floating`, `Snapped(SnapRegion)`, `Maximized`, `Fullscreen`, and `Minimized { restore_mode }` as an exclusive tagged enum so conflicting states (e.g., `Maximized + Fullscreen` or `Minimized + Fullscreen`) are structurally impossible.
  - **Intelligent Floating Placement**:
    - Transient child dialogs inherit their parent's workspace and center cleanly over their parent window (clamped to output bounds).
    - Rule-centered windows center in the output work area.
    - General floating windows use collision-aware cascading inside the work area.
  - **Smart Snapping (`SnapRegion`)**: `LeftHalf`, `RightHalf`, `TopLeft`, `TopRight`, `BottomLeft`, `BottomRight` with configurable gap and proper `xdg_toplevel::State::Tiled*` negotiation.
  - **Focus Management**: Configurable `FocusPolicy` (`ClickToFocus` default; `FocusFollowsMouse` optional) and per-workspace MRU focus stacks so switching workspaces always restores focus to the last active visible window on the destination workspace.
  - **Window Rules Engine**: Ordered matching against `app_id` and `title` supporting initial mode, initial size, centering, initial workspace, and focus-on-map policies.

## 3. Compositor Integration & Shortcuts (`crates/neon-compositor`)

Only non-minimized windows belonging to the active workspace are mapped in Smithay's `Space<Window>`; inactive-workspace and minimized windows remain live in `NeonState::managed_windows` and remap seamlessly when switching workspaces or restoring.

### Keyboard & Pointer Bindings (Milestones 1–2)

| Shortcut | Action |
|---|---|
| `Super + Return` | Spawn terminal inside NEON DE |
| `Alt + Tab` | Cycle focus across visible windows on active workspace |
| `Alt + Q` / `Super + Q` | Graceful close (`xdg_toplevel.close`) on focused window |
| `Alt + Left Drag` | Interactive move (auto-restores floating if maximized/snapped) |
| `Alt + Right Drag` | Interactive resize |
| `Super + Left` / `Super + Right` | Toggle Left Half / Right Half smart snap |
| `Super + Up` | Toggle Maximize |
| `Super + Down` | Restore Floating (or Minimize if already Floating) |
| `Super + F` / `Super + F11` | Toggle Fullscreen |
| `Super + H` | Minimize focused window |
| `Super + Shift + H` | Restore most recently minimized window on active workspace |
| `Super + Page_Down` / `Ctrl + Alt + Right` | Switch to next workspace |
| `Super + Page_Up` / `Ctrl + Alt + Left` | Switch to previous workspace |
| `Super + Shift + Page_Down` / `Ctrl + Alt + Shift + Right` | Move focused window to next workspace (creating one dynamically if needed) |
| `Super + Shift + Page_Up` / `Ctrl + Alt + Shift + Left` | Move focused window to previous workspace |
| `Super + 1..9` | Switch directly to workspace 1–9 |
| `Ctrl + Alt + Escape` | Cleanly exit nested NEON DE session |
