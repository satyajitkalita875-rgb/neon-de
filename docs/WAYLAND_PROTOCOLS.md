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
| `zwlr_layer_shell_v1` | Smithay `WlrLayerShellState` | Implemented | Desktop panels, dock, Activities Overview overlay, exclusive-zone work-area reservation, keyboard interactivity (`None` / `OnDemand` / `Exclusive`) |
| `ext_session_lock_v1` | Smithay `SessionLockManagerState` | Implemented | Strict lock-screen rendering & input isolation (`neon-lockscreen` client verified) |
| `wp_fractional_scale_v1` | Smithay `FractionalScaleManagerState` | Implemented | Fractional HiDPI scaling preferred-scale negotiation & live output scale broadcast |
| `wp_viewporter` | Smithay `ViewporterState` | Implemented | Surface cropping and logical scaling |
| `zwp_pointer_gestures_v1` | Smithay `PointerGesturesState` | Implemented | Touchpad swipe, pinch, and hold gestures + 3-finger compositor workspace/overview swipes |
| `wp_presentation` | Smithay `PresentationState` | Implemented | Monotonic presentation clock metadata |
| `xwayland_shell_v1` + XWayland (`X11Wm`) | Smithay `XWaylandShellState` + `X11Wm` | Implemented | Legacy X11 application compatibility (`DISPLAY=:<N>`) managed through `WindowManager` & `Space` |
| `zwp_linux_dmabuf_v1` | Smithay `DmabufState` | Implemented | Zero-copy GPU buffer import (advertised dynamically when EGL renderer exposes DMA-BUF formats) |

## Planned Protocols (Milestones 6–7)

| Protocol | Target Milestone | Purpose |
|---|---|---|
| `zxdg_decoration_manager_v1` | M6 | Server-side / client-side decoration negotiation |
| `zwp_primary_selection_v1` | M6 | Middle-click primary selection |
| `ext_image_copy_capture_v1` / PipeWire portal | M7 | Authorized screen capture & screen sharing |
