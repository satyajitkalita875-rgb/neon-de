# NEON DE — Rendering Engine & Effects Design

## 1. Design Goals

- GPU-accelerated composition targeting steady 60 FPS at 1080p on Intel CometLake-H integrated graphics and NVIDIA GTX 1650 Mobile.
- Strict separation between compositor event dispatch (`neon-compositor`), retained scene representation (`neon-scene`), frame-driven animations (`neon-effects`), and GPU rendering (`neon-renderer`).
- True damage-tracked partial redraws and zero-swap idle frames when nothing on screen changes.
- Clean OpenGL ES (`GlesRenderer`) path via Smithay with graceful fallback if custom shaders fail on an exotic driver, plus optional `--reduced-motion` and `--no-shadows` modes.

## 2. Implemented Rendering Pipeline (Milestone 3)

```text
Wayland Surfaces (wl_shm + zwp_linux_dmabuf_v1)
  -> Buffer Import & Texture Upload (GlesRenderer / ImportDma)
  -> EffectsManager::tick(now) (crates/neon-effects)
       - Window open fade + upward glide
       - Interruptible focus highlight transition
       - Horizontal workspace slide transition
       - Reduced-motion accessibility mode
  -> Retained SceneGraph sync (crates/neon-scene)
       - Z-ordered SceneLayers: Background -> Windows -> Panels -> Dock -> Overview -> Notifications -> LockScreen -> Cursor
       - Per-window ShadowStyle & BorderStyle (auto-stripped when Maximized/Fullscreen)
       - Logical dirty-region tracking
  -> NeonRenderer::render_scene (crates/neon-renderer)
       - Cached PixelShaderElements (stable Element Ids across idle frames!)
       - Analytical SDF rounded drop-shadow shader (SHADOW_FRAGMENT_SHADER)
       - Anti-aliased SDF rounded focus ring shader (BORDER_FRAGMENT_SHADER)
       - NEON DE ambient obsidian radial-gradient backdrop (BACKDROP_FRAGMENT_SHADER)
       - WaylandSurfaceRenderElement trees (surfaces + subsurfaces + popups)
  -> OutputDamageTracker::render_output (front-to-back damage culling)
  -> Conditional Swapchain Submit (skips swap on zero-damage idle frames)
```

## 3. Shader & Blending Conventions

- All custom pixel shaders target GLSL ES 1.00 (`#version 100` prepended by Smithay) and output **premultiplied RGBA** (`vec4(rgb * a, a)`) to match Smithay's `GL_ONE, GL_ONE_MINUS_SRC_ALPHA` blend pipeline.
- Every cached shader element (`CachedShadow`, `CachedBorder`, `CachedBackdrop`) retains its `Element::id()` and increments its `CommitCounter` **only** when its geometry, style, or opacity actually changes — avoiding spurious full-screen redraws during idle periods.

## 4. Planned Evolution (Milestone 5)

- Multi-pass downsampled Kawase / dual-filter GPU background blur with region caching for translucent panels, dock, and overview surfaces.
- Zero-copy DRM/KMS direct scanout for fullscreen clients on bare-metal outputs.
- `wp_presentation` and `wp_fractional_scale_v1` integration.
