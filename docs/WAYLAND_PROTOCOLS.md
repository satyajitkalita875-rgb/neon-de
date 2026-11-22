# NEON DE — Wayland Protocol Compatibility Matrix

> **Rule**: NEON DE never advertises a Wayland protocol global until its handler is implemented and verified.

## Implemented Protocols (Milestone 1)

| Protocol / Global | Provider | Status | Notes |
|---|---|---|---|
| `wl_compositor` | Smithay `CompositorState` | Implemented | Surface creation, commit, damage, frame callbacks |
| `wl_subcompositor` | Smithay `CompositorState` | Implemented | Subsurface tree synchronization |
| `wl_shm` | Smithay `ShmState` | Implemented | Shared-memory client buffers (ARGB8888, XRGB8888, etc.) |
| `wl_seat` | Smithay `SeatState` | Implemented | Keyboard (XKB keymap) and pointer capabilities |
| `wl_output` + `zxdg_output_manager_v1` | Smithay `OutputManagerState` | Implemented | Logical output mode, geometry, scale |
| `wl_data_device_manager` | Smithay `DataDeviceState` | Implemented | Clipboard selection & drag-and-drop scaffolding |
| `xdg_wm_base` (`xdg_surface`, `xdg_toplevel`, `xdg_popup`) | Smithay `XdgShellState` | Implemented | Floating toplevel windows, popups, interactive move/resize, maximize/fullscreen/minimize/tiled state negotiation |
| `zwp_linux_dmabuf_v1` | Smithay `DmabufState` | Implemented | Zero-copy GPU buffer import (advertised dynamically when EGL renderer exposes DMA-BUF formats) |

## Planned Protocols (Milestones 4–7)

| Protocol | Target Milestone | Purpose |
|---|---|---|
| `wp_presentation` | M5 | Frame presentation feedback & timing |
| `wp_viewporter` | M5 | Surface cropping and scaling |
| `wp_fractional_scale_v1` | M5 | Fractional HiDPI scaling |
| `zwlr_layer_shell_v1` | M4 | Desktop panels, dock, overlays, wallpapers |
| `zxdg_decoration_manager_v1` | M2 / M3 | Server-side / client-side decoration negotiation |
| `ext_session_lock_v1` | M5 | Secure lock screen integration |
| `zwp_pointer_gestures_v1` | M5 | Touchpad swipe & pinch gestures |
| `zwp_primary_selection_v1` | M4 | Middle-click primary selection |
| `ext_image_copy_capture_v1` / PipeWire portal | M7 | Authorized screen capture & screen sharing |
| XWayland | M5 | Legacy X11 application compatibility |
