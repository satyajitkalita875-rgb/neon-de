# NEON DE — Testing Guide

## 1. Test Layers

NEON DE verifies correctness across three layers:

1. **Unit Tests (`cargo test --lib`)**: Deterministic tests for `WindowRegistry`, floating placement, focus stack transitions, and window lifecycle invariants.
2. **Protocol & Client Integration Tests (`cargo test --test '*'`)**: Spins up a real `NeonState` Wayland display server on an isolated socket, connects a real `wayland-client` (`wl_compositor`, `wl_shm`, `xdg_wm_base`, `xdg_surface`, `xdg_toplevel`, `wl_seat`), commits SHM buffers, verifies window mapping, title/app_id propagation, keyboard/pointer focus routing, and clean unmapping/destruction on client disconnect.
3. **Live Nested Session Verification**: Runs `neon-compositor` in a nested `winit` window inside the host desktop, spawns real graphical clients (`weston-terminal`, `weston-simple-shm`, `alacritty`), and validates rendering, keyboard input, pointer focus, interactive move/resize, and shutdown.

## 2. Running Automated Tests

```bash
# Run all unit and integration tests
cargo test --workspace

# Run with verbose tracing logs
RUST_LOG=neon_compositor=debug cargo test --workspace -- --nocapture
```

## 3. Running Live Nested Verification

```bash
./scripts/run-nested.sh
```

Inside the nested NEON DE window:
- The NEON DE Top Panel (Activities, dynamic workspace pills, live clock, active title) and Floating Dock (pinned XDG apps, running window indicators, app grid) launch automatically with `./scripts/run-nested.sh` (`neon-compositor --shell`).
- Press `Super + Space` (or click **Activities** / the Dock app grid button) to open the **Activities Overview** with live search, workspace previews, open window cards, and the XDG Application Grid; press `Esc` to close.
- Type commands into the terminal to verify keyboard focus and XKB translation.
- Click between multiple windows to verify click-to-focus and stacking raise.
- Drag with `Alt + Left Mouse` to move windows, `Alt + Right Mouse` to resize.
- Use `Super + Left/Right` (smart snap), `Super + Up` (maximize), `Super + H` / `Super + Shift + H` (minimize/restore), and `Super + Page_Up/Page_Down` or `Ctrl + Alt + Left/Right` (dynamic workspaces).
- Press `Super + Return` to spawn an additional terminal.
- Press `Alt + Tab` to cycle focus.
- Press `Alt + Q` or `Super + Q` to close the focused window.
- Press `Ctrl + Alt + Escape` (or close the host nested window) to exit cleanly.
