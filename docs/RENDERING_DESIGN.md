# NEON DE — Rendering Engine Design

## 1. Design Goals

- GPU-accelerated composition targeting steady 60 FPS at 1080p on Intel CometLake-H integrated graphics and NVIDIA GTX 1650 Mobile.
- Strict separation between compositor event dispatch and rendering logic.
- Damage-tracked partial redraws to minimize idle GPU/CPU usage.
- Clean abstraction over Smithay's OpenGL ES (`GlesRenderer` / `GlowRenderer`) initially, keeping the door open for alternative renderers later without premature Vulkan complexity.

## 2. Conceptual Rendering Pipeline

```text
Wayland Surfaces (wl_shm / DMA-BUF)
  -> Buffer Import & Texture Upload
  -> Surface Tree Traversal (main surface + subsurfaces + popups)
  -> Retained Scene Graph (Milestone 3: Background -> Windows -> Panels -> Dock -> Overview -> Notifications -> Lockscreen -> Cursor)
  -> Window Transforms & Effects (Milestone 3 & 5: rounded corners, shadows, blur, animations)
  -> Damage Tracking (OutputDamageTracker)
  -> GPU Composition (GlesRenderer / GlowRenderer)
  -> Output Presentation (Winit nested swapchain [M1] / DRM-KMS atomic pageflip [M5])
```

## 3. Milestone 1 Rendering Implementation

In Milestone 1 (`crates/neon-compositor/src/backend/winit.rs`):

- Uses `smithay::backend::winit` with `GlesRenderer` (`GlowRenderer`) to create a nested EGL/GLES window inside the host Wayland/X11 session.
- Uses `smithay::desktop::Space<Window>` + `smithay::backend::renderer::damage::OutputDamageTracker` to render visible client windows, subsurfaces, and popups with damage tracking.
- Draws a NEON DE signature deep obsidian/midnight canvas clear color (`[0.055, 0.067, 0.098, 1.0]`) as the compositor backdrop before client surfaces are composited.
- Dispatches Wayland `wl_surface.frame` callbacks immediately after rendering so clients pace their animations cleanly at the output refresh rate.
- Supports software GL fallback (`LIBGL_ALWAYS_SOFTWARE=1`) for headless or recovery debugging.

## 4. Planned Evolution (Milestones 3 & 5)

- **Milestone 3**: Extract `neon-scene` (retained scene graph with Z-layers, clipping, opacity, transforms) and `neon-renderer` (custom GLSL shaders for rounded corners, analytical shadows, and damage-aware partial redraws). Introduce `neon-effects` for frame-driven spring/cubic-Bézier animations.
- **Milestone 5**: Add downsampled Kawase/dual-filter background blur with region caching and user toggles for reduced motion / low-power mode, plus zero-copy `zwp_linux_dmabuf_v1` + DRM/KMS direct scanout.
