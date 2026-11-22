# NEON DE — Window Manager Design

## 1. Philosophy

NEON DE uses a **GNOME-inspired floating window management model** as the default workflow, paired with predictable keyboard navigation, smart window placement, and optional snapping/tiling (Milestone 2).

## 2. Window Identity & Model

Every managed toplevel window is tracked in `WindowRegistry` (`crates/neon-compositor/src/window.rs`) with a monotonic `WindowId(u64)` independent of Wayland object IDs, storing:

- `WindowId` — stable internal identifier
- Smithay `Window` handle (backing `ToplevelSurface` / `WlSurface`)
- `app_id` & `title` (synced from `xdg_toplevel`)
- Logical `position` and `size`
- Focus / active state (`xdg_toplevel::State::Activated`)
- Lifecycle state (`Mapped` / `Unmapped`, plus planned `Minimized`, `Maximized`, `Fullscreen`, `WorkspaceId`, `OutputId`, parent-child transient links in Milestone 2)

## 3. Milestone 1 Implemented Behaviour

- **Floating placement**: Cascade placement inside the active output geometry, clamped to stay visible on screen.
- **Click-to-focus & raise**: Clicking any window activates it, raises it to the top of the stacking order, and transfers keyboard focus to its `WlSurface`.
- **Keyboard focus lifecycle**: Newly mapped toplevels automatically receive focus; when a focused window closes, focus transfers cleanly to the next top-most mapped window (or clears if none remain).
- **Interactive move & resize**: Supports both client-initiated (`xdg_toplevel.move` / `xdg_toplevel.resize`) and compositor-initiated (`Alt + Left Drag` to move, `Alt + Right Drag` to resize) pointer grabs.
- **Compositor keybindings (Milestone 1)**:
  - `Alt + Q` or `Super + Q`: Request graceful close (`xdg_toplevel.send_close()`) on the focused window.
  - `Alt + Tab`: Cycle focus to the next mapped window.
  - `Super + Return`: Spawn a terminal (`weston-terminal` / `alacritty` / `kitty`) connected to the NEON DE Wayland socket.
  - `Ctrl + Alt + Escape`: Cleanly exit the nested compositor session.

## 4. Planned Evolution (Milestone 2)

- Extract `crates/neon-window-manager` and `crates/neon-workspaces`.
- Explicit window state machine preventing conflicting states (e.g., minimized + fullscreen).
- Dynamic workspace lifecycle (automatic empty trailing workspace, automatic cleanup of unused workspaces, predictable ordering).
- Smart snapping (edge half/quarter tiling), window rules by `app_id`/title, transient dialog centering and modal parent tracking.
