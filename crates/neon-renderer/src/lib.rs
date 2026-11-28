//! Modular GPU rendering pipeline for **NEON DE** built on Smithay's OpenGL ES (`GlesRenderer`).
//!
//! Translates [`neon_scene::SceneGraph`] into damage-tracked Smithay render elements with
//! cached custom GLSL elements (analytical soft shadows, rounded accent focus rings, and
//! NEON DE ambient backdrop) so idle frames generate zero spurious damage.

pub mod backdrop_blur;
pub mod close_ghost;
pub mod cursor;
pub mod rounded;
pub mod screenshot;
pub mod shaders;

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use neon_scene::{BorderStyle, NodeContent, SceneGraph, SceneLayer, ShadowStyle, TitlebarStyle};
use neon_window_manager::WindowId;
use smithay::{
    backend::renderer::{
        Color32F,
        damage::{Error as OutputDamageTrackerError, OutputDamageTracker, RenderOutputResult},
        element::{
            AsRenderElements, Element, Id, Kind, RenderElement, UnderlyingStorage,
            memory::MemoryRenderBufferRenderElement,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::RescaleRenderElement,
        },
        gles::{
            GlesError, GlesFrame, GlesRenderer, GlesTarget, GlesTexture, Uniform,
            element::PixelShaderElement,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    desktop::{PopupManager, Window, WindowSurface, layer_map_for_output},
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    render_elements,
    utils::{Buffer as BufferCoord, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::{
        compositor::with_states, session_lock::LockSurface, shell::wlr_layer::Layer as WlrLayer,
    },
    xwayland::xwm::WmWindowType,
};
use tracing::{info, warn};

use crate::{
    backdrop_blur::{
        BlurTargets, BlurredBackdropElement, CHROME_BLUR_CACHE_TTL, OVERLAY_BLUR_CACHE_TTL,
    },
    close_ghost::{ClosingGhost, ClosingGhostElement},
    cursor::CursorCache,
    rounded::RoundedSurfaceElement,
    shaders::NeonShaders,
};

pub use crate::close_ghost::PendingCloseGhost;

/// Fallback clear color (`#0C0F17`) used if shaders are unavailable.
pub const FALLBACK_CLEAR_COLOR: Color32F = Color32F::new(0.047, 0.059, 0.090, 1.0);

/// Opaque security curtain color (`#05070D`) used while `ext_session_lock_v1` is locked.
pub const LOCK_CURTAIN_COLOR: Color32F = Color32F::new(0.020, 0.027, 0.051, 1.0);

/// Live blur and overlay motion parameters supplied to [`NeonRenderer::render_scene`].
#[derive(Debug, Clone, Copy)]
pub struct BlurSettings {
    pub enabled: bool,
    pub strength: f32,
    pub corner_radius: f32,
    pub animations_enabled: bool,
    pub speed_scale: f32,
}

impl Default for BlurSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 1.0,
            corner_radius: 12.0,
            animations_enabled: true,
            speed_scale: 1.0,
        }
    }
}

/// Wraps a [`WaylandSurfaceRenderElement`] with a dedicated stable [`Id`] so the same layer
/// surface (Wallpaper or Dock) can be composited at both outgoing and incoming desktop offsets
/// simultaneously during a full-desktop Hyprland workspace slide without `OutputDamageTracker`
/// element ID collisions.
#[derive(Debug)]
pub struct TaggedWaylandElement {
    id: Id,
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
}

impl TaggedWaylandElement {
    fn new(id: Id, inner: WaylandSurfaceRenderElement<GlesRenderer>) -> Self {
        Self { id, inner }
    }
}

impl Element for TaggedWaylandElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, BufferCoord> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for TaggedWaylandElement {
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        self.inner.draw(frame, src, dst, damage, opaque_regions)
    }
}

render_elements! {
    /// Unified render element type composited by [`NeonRenderer`].
    pub NeonRenderElement<=GlesRenderer>;
    Wayland=WaylandSurfaceRenderElement<GlesRenderer>,
    TaggedWayland=TaggedWaylandElement,
    RescaledWayland=RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    RoundedWayland=RoundedSurfaceElement,
    RescaledRoundedWayland=RescaleRenderElement<RoundedSurfaceElement>,
    BlurredBackdrop=BlurredBackdropElement,
    ClosingGhost=ClosingGhostElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Shader=PixelShaderElement,
    Solid=SolidColorRenderElement,
}

#[derive(Debug, Clone)]
struct CachedShadow {
    element: PixelShaderElement,
    area: Rectangle<i32, Logical>,
    rect_size: [f32; 2],
    style: ShadowStyle,
    opacity: f32,
}

#[derive(Debug, Clone)]
struct CachedBorder {
    element: PixelShaderElement,
    area: Rectangle<i32, Logical>,
    style: BorderStyle,
    opacity: f32,
}

#[derive(Debug, Clone)]
struct CachedBackdrop {
    element: PixelShaderElement,
    area: Rectangle<i32, Logical>,
    primary: [f32; 4],
    accent: [f32; 4],
}

#[derive(Debug, Clone)]
struct CachedGlass {
    element: PixelShaderElement,
    area: Rectangle<i32, Logical>,
    output_size: [f32; 2],
    window_origin: [f32; 2],
    corner_radius: f32,
    blur_strength: f32,
    opacity: f32,
}

#[derive(Debug, Clone)]
struct CachedTitlebar {
    element: PixelShaderElement,
    area: Rectangle<i32, Logical>,
    style: TitlebarStyle,
    opacity: f32,
}

#[derive(Debug, Clone, Copy)]
enum OverlayAnim {
    /// Queued via IPC before GTK commits its first painted buffer; promoted to `Opening` on the
    /// exact first committed frame so animations always start cleanly at `t = 0.0` (zero jump).
    PendingOpen { queued: Instant },
    Opening { started: Instant },
    Closing { started: Instant },
}

/// Stateful per-output GPU renderer with compiled custom shaders and cached damage-aware elements.
#[derive(Debug)]
pub struct NeonRenderer {
    shaders: Option<NeonShaders>,
    damage_tracker: OutputDamageTracker,
    shadow_cache: HashMap<WindowId, CachedShadow>,
    border_cache: HashMap<WindowId, CachedBorder>,
    glass_cache: HashMap<WindowId, CachedGlass>,
    titlebar_cache: HashMap<WindowId, CachedTitlebar>,
    rounded_radii_cache: HashMap<WindowId, (i32, i32)>,
    backdrop_cache: Option<CachedBackdrop>,
    cursor_cache: CursorCache,
    chrome_blur_targets: BlurTargets,
    blur_targets: BlurTargets,
    blur_element_ids: HashMap<String, Id>,
    ws_slide_layer_ids: HashMap<String, Vec<Id>>,
    mapped_blur_overlays: HashSet<String>,
    overlay_anims: HashMap<String, OverlayAnim>,
    overlay_motion_enabled: bool,
    overlay_speed_scale: f32,
    blur_anim_commit: CommitCounter,
    last_blur_tex_commit: CommitCounter,
    chrome_blur_commit: CommitCounter,
    last_chrome_blur_tex_commit: CommitCounter,
    closing_ghosts: Vec<ClosingGhost>,
    pub(crate) screencopy_offscreen: Option<(Size<i32, BufferCoord>, GlesTexture)>,
}

impl NeonRenderer {
    /// Creates a [`NeonRenderer`] bound to `output`, compiling NEON DE's custom GLES shaders.
    ///
    /// If shader compilation fails on an unsupported driver, degrades gracefully to solid
    /// composition without crashing the compositor.
    pub fn new(renderer: &mut GlesRenderer, output: &Output) -> Self {
        let shaders = match NeonShaders::compile(renderer) {
            Ok(compiled) => {
                info!(
                    "Compiled NEON DE custom GLES shaders (shadow, border, backdrop, frosted-glass, titlebar, rounded-window, backdrop-blur)"
                );
                Some(compiled)
            }
            Err(err) => {
                warn!(
                    ?err,
                    "Failed to compile custom GLES shaders; falling back to plain composition"
                );
                None
            }
        };

        Self {
            shaders,
            damage_tracker: OutputDamageTracker::from_output(output),
            shadow_cache: HashMap::new(),
            border_cache: HashMap::new(),
            glass_cache: HashMap::new(),
            titlebar_cache: HashMap::new(),
            rounded_radii_cache: HashMap::new(),
            backdrop_cache: None,
            cursor_cache: CursorCache::load_default(),
            chrome_blur_targets: BlurTargets::default(),
            blur_targets: BlurTargets::default(),
            blur_element_ids: HashMap::new(),
            ws_slide_layer_ids: HashMap::new(),
            mapped_blur_overlays: HashSet::new(),
            overlay_anims: HashMap::new(),
            overlay_motion_enabled: true,
            overlay_speed_scale: 1.0,
            blur_anim_commit: CommitCounter::default(),
            last_blur_tex_commit: CommitCounter::default(),
            chrome_blur_commit: CommitCounter::default(),
            last_chrome_blur_tex_commit: CommitCounter::default(),
            closing_ghosts: Vec::new(),
            screencopy_offscreen: None,
        }
    }

    pub fn sync_cursor_config(&mut self, theme: &str, size: u32) {
        self.cursor_cache.set_theme_and_size(theme, size);
    }

    pub fn set_motion_config(&mut self, animations_enabled: bool, speed_scale: f32) {
        self.overlay_motion_enabled = animations_enabled;
        self.overlay_speed_scale = speed_scale.clamp(0.5, 2.5);
    }

    pub fn has_shaders(&self) -> bool {
        self.shaders.is_some()
    }

    /// Triggers a 60 FPS GPU open or close animation for a blurred shell overlay
    /// (`"neon-quick-settings"`, `"neon-workspaces-panel"`, or `"neon-power-menu"`).
    pub fn trigger_overlay_anim(&mut self, namespace: &str, opening: bool) {
        if !self.overlay_motion_enabled {
            if opening {
                self.overlay_anims.remove(namespace);
            } else {
                self.overlay_anims.insert(
                    namespace.to_string(),
                    OverlayAnim::Closing {
                        started: Instant::now(),
                    },
                );
            }
            return;
        }
        let anim = if opening {
            if self.mapped_blur_overlays.contains(namespace) {
                OverlayAnim::Opening {
                    started: Instant::now(),
                }
            } else {
                OverlayAnim::PendingOpen {
                    queued: Instant::now(),
                }
            }
        } else {
            OverlayAnim::Closing {
                started: Instant::now(),
            }
        };
        self.overlay_anims.insert(namespace.to_string(), anim);
    }

    /// Captures offscreen GPU textures for newly closed windows so client-initiated closes
    /// (clicking an app's own `×` button or `Ctrl+Q`) play a smooth pop-out fade animation.
    pub fn ingest_close_ghosts(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        pending: &[PendingCloseGhost],
    ) {
        if pending.is_empty() {
            return;
        }
        let fallback_desktop = self
            .chrome_blur_targets
            .full_texture()
            .or_else(|| self.blur_targets.full_texture());
        for ghost in pending {
            if let Some(captured) =
                ClosingGhost::capture(renderer, output, ghost, fallback_desktop.clone())
            {
                self.closing_ghosts.push(captured);
            }
        }
        // Cap concurrent ghosts to keep VRAM bounded during rapid batch closes.
        if self.closing_ghosts.len() > 8 {
            let drain = self.closing_ghosts.len() - 8;
            self.closing_ghosts.drain(..drain);
        }
    }

    /// Returns `true` while any shell overlay open/close GPU animation or closing-window ghost
    /// is still in progress so the compositor schedules continuous VBlank-locked frames until settled.
    pub fn has_active_overlay_animation(&self) -> bool {
        let motion_on = self.overlay_motion_enabled;
        let speed = self.overlay_speed_scale;
        self.closing_ghosts.iter().any(|g| !g.is_finished())
            || (motion_on
                && self.overlay_anims.iter().any(|(ns, &anim)| {
                    let (_, _, _, _, active) = Self::eval_overlay_anim(ns, Some(anim), true, speed);
                    active
                }))
    }

    /// Evaluates `(offset_x_logical, offset_y_logical, scale, alpha, is_animating)` for a shell overlay.
    fn eval_overlay_anim(
        namespace: &str,
        anim: Option<OverlayAnim>,
        motion_enabled: bool,
        speed_scale: f32,
    ) -> (f32, f32, f32, f32, bool) {
        let Some(anim) = anim else {
            return (0.0, 0.0, 1.0, 1.0, false);
        };
        if !motion_enabled {
            return match anim {
                OverlayAnim::Closing { .. } => (0.0, 0.0, 1.0, 0.0, false),
                _ => (0.0, 0.0, 1.0, 1.0, false),
            };
        }
        let speed = speed_scale.clamp(0.5, 2.5);
        if let OverlayAnim::PendingOpen { queued } = anim {
            // Keep scheduling frames until GTK attaches its first real buffer (capped at 600ms).
            return (0.0, 0.0, 1.0, 0.0, queued.elapsed().as_millis() < 600);
        }
        // Helper: official Material 3 Expressive damped spatial spring (ζ < 1 organic overshoot,
        // feathered smoothly over the final 10% so arrival at 1.0 never snaps).
        let eval_m3_spring = |t: f32, zeta: f32, omega0: f32| -> f32 {
            let settle = (5.0 / (zeta * omega0)).clamp(0.22, 0.72);
            let tau = t * settle;
            let wd = omega0 * (1.0 - zeta * zeta).sqrt();
            let raw =
                1.0 - (-zeta * omega0 * tau).exp() * ((wd * tau).cos() + (zeta * omega0 / wd) * (wd * tau).sin());
            let tail = ((t - 0.88) / 0.12).clamp(0.0, 1.0);
            let st = tail * tail * (3.0 - 2.0 * tail);
            raw * (1.0 - st) + st
        };

        match namespace {
            "neon-quick-settings" | "neon-notification-center" => match anim {
                OverlayAnim::Opening { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.36;
                    if elapsed >= DUR {
                        return (0.0, 0.0, 1.0, 1.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    // M3 Expressive Default Spatial (k=380, ζ=0.72): lively top-anchored pop-in.
                    let spring = eval_m3_spring(t, 0.72, 19.5);
                    let scale = (0.90 + 0.10 * spring).clamp(0.88, 1.024);
                    let offset_y = -20.0 * (1.0 - spring);
                    // M3 Expressive Fast Effects opacity (zero overshoot).
                    let at = (elapsed / 0.135).clamp(0.0, 1.0);
                    let alpha = 1.0 - (1.0 - at).powi(3);
                    (0.0, offset_y, scale, alpha, true)
                }
                OverlayAnim::Closing { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.22;
                    if elapsed >= DUR {
                        return (0.0, -18.0, 0.91, 0.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = 1.0 - (1.0 - t).powi(3);
                    let scale = 1.0 - 0.09 * p;
                    let offset_y = -18.0 * p;
                    // Slight delay before alpha hits 0 so the tuck-up motion stays clearly visible.
                    let at = ((t - 0.16) / 0.84).clamp(0.0, 1.0);
                    let alpha = (1.0 - at * at * (3.0 - 2.0 * at)).clamp(0.0, 1.0);
                    (0.0, offset_y, scale, alpha, true)
                }
                OverlayAnim::PendingOpen { .. } => unreachable!(),
            },
            "neon-workspaces-panel" => match anim {
                OverlayAnim::Opening { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.24;
                    if elapsed >= DUR {
                        return (0.0, 0.0, 1.0, 1.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = 1.0 - (1.0 - t).powi(4);
                    let offset_x = -248.0 * (1.0 - p);
                    let at = (elapsed / 0.11).clamp(0.0, 1.0);
                    let alpha = 1.0 - (1.0 - at).powi(3);
                    (offset_x, 0.0, 1.0, alpha, true)
                }
                OverlayAnim::Closing { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.175;
                    if elapsed >= DUR {
                        return (-248.0, 0.0, 1.0, 0.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = t * t * (3.0 - 2.0 * t);
                    let offset_x = -248.0 * p;
                    let alpha = (1.0 - p).clamp(0.0, 1.0);
                    (offset_x, 0.0, 1.0, alpha, true)
                }
                OverlayAnim::PendingOpen { .. } => unreachable!(),
            },
            "neon-screencast-picker" | "neon-screenshot-bar" => match anim {
                OverlayAnim::Opening { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.34;
                    if elapsed >= DUR {
                        return (0.0, 0.0, 1.0, 1.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let spring = eval_m3_spring(t, 0.74, 19.5);
                    let scale = (0.90 + 0.10 * spring).clamp(0.88, 1.022);
                    let offset_y = 20.0 * (1.0 - spring);
                    let at = (elapsed / 0.13).clamp(0.0, 1.0);
                    let alpha = 1.0 - (1.0 - at).powi(3);
                    (0.0, offset_y, scale, alpha, true)
                }
                OverlayAnim::Closing { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.21;
                    if elapsed >= DUR {
                        return (0.0, 16.0, 0.91, 0.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = 1.0 - (1.0 - t).powi(3);
                    let at = ((t - 0.15) / 0.85).clamp(0.0, 1.0);
                    let alpha = (1.0 - at * at * (3.0 - 2.0 * at)).clamp(0.0, 1.0);
                    (0.0, 16.0 * p, 1.0 - 0.09 * p, alpha, true)
                }
                OverlayAnim::PendingOpen { .. } => unreachable!(),
            },
            "neon-context-menu" | "neon-context-submenu" => match anim {
                OverlayAnim::Opening { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.22;
                    if elapsed >= DUR {
                        return (0.0, 0.0, 1.0, 1.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    // M3 Expressive Fast Spatial (k=800, ζ=0.68): snappy crisp spring pop.
                    let spring = eval_m3_spring(t, 0.68, 28.0);
                    let scale = (0.90 + 0.10 * spring).clamp(0.88, 1.02);
                    let offset_y = -7.0 * (1.0 - spring);
                    let at = (elapsed / 0.10).clamp(0.0, 1.0);
                    let alpha = 1.0 - (1.0 - at).powi(3);
                    (0.0, offset_y, scale, alpha, true)
                }
                OverlayAnim::Closing { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.145;
                    if elapsed >= DUR {
                        return (0.0, -6.0, 0.92, 0.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = 1.0 - (1.0 - t).powi(3);
                    let at = ((t - 0.12) / 0.88).clamp(0.0, 1.0);
                    let alpha = (1.0 - at * at * (3.0 - 2.0 * at)).clamp(0.0, 1.0);
                    (0.0, -6.0 * p, 1.0 - 0.08 * p, alpha, true)
                }
                OverlayAnim::PendingOpen { .. } => unreachable!(),
            },
            "neon-dock-preview" => match anim {
                OverlayAnim::Opening { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.18;
                    if elapsed >= DUR {
                        return (0.0, 0.0, 1.0, 1.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = 1.0 - (1.0 - t).powi(3);
                    let scale = 0.96 + 0.04 * p;
                    let offset_y = -8.0 * (1.0 - p);
                    (0.0, offset_y, scale, p, true)
                }
                OverlayAnim::Closing { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.12;
                    if elapsed >= DUR {
                        return (0.0, -6.0, 0.96, 0.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = 1.0 - (1.0 - t).powi(2);
                    (0.0, -6.0 * p, 1.0 - 0.04 * p, 1.0 - p, true)
                }
                OverlayAnim::PendingOpen { .. } => unreachable!(),
            },
            "neon-power-menu" => match anim {
                OverlayAnim::Opening { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.30;
                    if elapsed >= DUR {
                        return (0.0, 0.0, 1.0, 1.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let spring = eval_m3_spring(t, 0.78, 19.0);
                    let scale = (0.95 + 0.05 * spring).clamp(0.94, 1.012);
                    let at = (elapsed / 0.17).clamp(0.0, 1.0);
                    let alpha = 1.0 - (1.0 - at).powi(3);
                    (0.0, 0.0, scale, alpha, true)
                }
                OverlayAnim::Closing { started } => {
                    let elapsed = started.elapsed().as_secs_f32() * speed;
                    const DUR: f32 = 0.21;
                    if elapsed >= DUR {
                        return (0.0, 0.0, 0.96, 0.0, false);
                    }
                    let t = (elapsed / DUR).clamp(0.0, 1.0);
                    let p = 1.0 - (1.0 - t).powi(3);
                    let at = ((t - 0.14) / 0.86).clamp(0.0, 1.0);
                    let alpha = (1.0 - at * at * (3.0 - 2.0 * at)).clamp(0.0, 1.0);
                    (0.0, 0.0, 1.0 - 0.04 * p, alpha, true)
                }
                OverlayAnim::PendingOpen { .. } => unreachable!(),
            },
            _ => (0.0, 0.0, 1.0, 1.0, false),
        }
    }

    /// Collects render elements for a locked session (`ext_session_lock_v1`).
    pub fn collect_locked_elements(
        &self,
        output: &Output,
        renderer: &mut GlesRenderer,
        lock_surface: Option<&LockSurface>,
    ) -> Vec<NeonRenderElement> {
        let scale_f = output.current_scale().fractional_scale();
        let scale = Scale::from(scale_f);
        let mut elements: Vec<NeonRenderElement> = Vec::new();

        if let Some(surface) = lock_surface {
            let lock_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    surface.wl_surface(),
                    (0, 0),
                    scale,
                    1.0,
                    Kind::Unspecified,
                );
            elements.extend(lock_elements.into_iter().map(NeonRenderElement::from));
        }
        elements
    }

    /// Renders ONLY the active `ext_session_lock_v1` lock surface over an opaque security curtain.
    ///
    /// Guarantees that zero regular desktop windows, panels, dock, or overview surfaces are drawn
    /// while the session is locked.
    pub fn render_locked<'a>(
        &'a mut self,
        output: &Output,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
        age: usize,
        lock_surface: Option<&LockSurface>,
    ) -> Result<RenderOutputResult<'a>, OutputDamageTrackerError<GlesError>> {
        let elements = self.collect_locked_elements(output, renderer, lock_surface);

        self.damage_tracker
            .render_output(renderer, framebuffer, age, &elements, LOCK_CURTAIN_COLOR)
    }

    /// Renders a real anti-aliased XCursor (or client-provided `wl_surface` cursor) for
    /// bare-metal DRM/KMS outputs.
    pub fn cursor_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        pointer_loc: Point<f64, Logical>,
        output_origin: Point<i32, Logical>,
        scale_f: f64,
        cursor_status: &CursorImageStatus,
    ) -> Vec<NeonRenderElement> {
        let local_x = pointer_loc.x - output_origin.x as f64;
        let local_y = pointer_loc.y - output_origin.y as f64;

        match cursor_status {
            CursorImageStatus::Hidden => Vec::new(),
            CursorImageStatus::Surface(wl_surface) => {
                let hotspot = with_states(wl_surface, |states| {
                    states
                        .data_map
                        .get::<std::sync::Mutex<CursorImageAttributes>>()
                        .and_then(|m| m.lock().ok())
                        .map(|attrs| attrs.hotspot)
                        .unwrap_or_else(|| (0, 0).into())
                });
                let render_loc: Point<i32, Logical> = (
                    local_x.round() as i32 - hotspot.x,
                    local_y.round() as i32 - hotspot.y,
                )
                    .into();
                let phys_loc: Point<i32, Physical> = render_loc.to_physical_precise_round(scale_f);
                let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        wl_surface,
                        phys_loc,
                        Scale::from(scale_f),
                        1.0,
                        Kind::Cursor,
                    );
                if !elems.is_empty() {
                    return elems.into_iter().map(NeonRenderElement::from).collect();
                }
                self.render_named_cursor(renderer, CursorIcon::Default, local_x, local_y, scale_f)
            }
            CursorImageStatus::Named(icon) => {
                self.render_named_cursor(renderer, *icon, local_x, local_y, scale_f)
            }
        }
    }

    /// Renders an active Wayland client Drag-and-Drop icon surface (`wl_data_device.start_drag`)
    /// directly beneath the pointer cursor.
    pub fn dnd_icon_elements(
        &self,
        renderer: &mut GlesRenderer,
        pointer_loc: Point<f64, Logical>,
        output_origin: Point<i32, Logical>,
        scale_f: f64,
        dnd_icon: Option<&WlSurface>,
    ) -> Vec<NeonRenderElement> {
        let Some(wl_surface) = dnd_icon else {
            return Vec::new();
        };
        let render_loc: Point<i32, Logical> = (
            (pointer_loc.x - output_origin.x as f64).round() as i32 + 6,
            (pointer_loc.y - output_origin.y as f64).round() as i32 + 6,
        )
            .into();
        let phys_loc: Point<i32, Physical> = render_loc.to_physical_precise_round(scale_f);
        let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            render_elements_from_surface_tree(
                renderer,
                wl_surface,
                phys_loc,
                Scale::from(scale_f),
                0.94,
                Kind::Unspecified,
            );
        elems.into_iter().map(NeonRenderElement::from).collect()
    }

    fn render_named_cursor(
        &mut self,
        renderer: &mut GlesRenderer,
        icon: CursorIcon,
        local_x: f64,
        local_y: f64,
        scale_f: f64,
    ) -> Vec<NeonRenderElement> {
        let loaded = self.cursor_cache.get_named_scaled(icon, scale_f);
        let phys_loc: Point<f64, Physical> = Point::<f64, Logical>::from((
            local_x - loaded.hotspot.0,
            local_y - loaded.hotspot.1,
        ))
        .to_physical(scale_f);

        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            phys_loc,
            &loaded.buffer,
            None,
            None,
            loaded.logical_size,
            Kind::Cursor,
        ) {
            Ok(elem) => vec![NeonRenderElement::from(elem)],
            Err(_) => Vec::new(),
        }
    }

    /// Collects all front-to-back [`NeonRenderElement`]s for the current [`SceneGraph`] on `output`.
    pub fn collect_scene_elements<'a, F>(
        &mut self,
        output: &Output,
        renderer: &mut GlesRenderer,
        scene: &SceneGraph,
        blur: BlurSettings,
        resolve_window: F,
    ) -> Vec<NeonRenderElement>
    where
        F: FnMut(WindowId) -> Option<&'a Window>,
    {
        self.collect_scene_elements_with_popups(output, renderer, scene, blur, &[], resolve_window)
    }

    /// Collects all front-to-back [`NeonRenderElement`]s including XWayland override-redirect
    /// popup windows (such as right-click context menus and dropdowns) composited directly above
    /// regular windows and below `Overlay`/`Top` shell layers.
    pub fn collect_scene_elements_with_popups<'a, F>(
        &mut self,
        output: &Output,
        renderer: &mut GlesRenderer,
        scene: &SceneGraph,
        blur: BlurSettings,
        override_redirect: &[(&Window, Point<i32, Logical>)],
        mut resolve_window: F,
    ) -> Vec<NeonRenderElement>
    where
        F: FnMut(WindowId) -> Option<&'a Window>,
    {
        self.overlay_motion_enabled = blur.animations_enabled;
        self.overlay_speed_scale = blur.speed_scale.clamp(0.5, 2.5);
        let scale_f = output.current_scale().fractional_scale();
        let scale = Scale::from(scale_f);
        let nodes = scene.collect_front_to_back();

        let mut elements: Vec<NeonRenderElement> =
            Vec::with_capacity(nodes.len() * 3 + override_redirect.len() * 2 + 8);
        let mut front_scene_elements: Vec<NeonRenderElement> = Vec::new();
        let mut seen_windows: HashSet<WindowId> = HashSet::new();

        // Ignore departing outgoing-workspace slide nodes (`z_index < 0`) so switching from a
        // fullscreen video workspace to an empty/normal workspace immediately shows Top Bar & Dock.
        let has_fullscreen_window = nodes.iter().any(|n| {
            n.z_index >= 0
                && matches!(
                    n.content,
                    NodeContent::Window {
                        fullscreen: true,
                        ..
                    }
                )
        });

        // Collect zwlr_layer_shell_v1 surfaces on this output split into:
        // - `front_overlays`: topmost non-blur `Layer::Overlay` surfaces (toasts, screenshot UI, dialogs)
        // - `blur_overlays`: `Layer::Overlay` popups that receive a real-time GPU Gaussian backdrop
        //   blur (`neon-context-menu`, `neon-quick-settings`, `neon-power-menu`, `neon-screencast-picker`)
        // - `backdrop_overlays`: transparent click-catcher backdrops stacked directly below blur overlays
        // - `chrome_overlays`: elevated Top Bar & Dock (`Layer::Overlay` during Launchpad / Mission
        //   Control) — stacked ABOVE fullscreen `Layer::Top` backdrops and ALWAYS BELOW popups/menus
        // - `top_layers`: `Layer::Top` chrome (panel, dock, sensors, overview, mission control)
        // - `lower_layers`: `Layer::Bottom` / `Layer::Background` (wallpaper)
        let (
            front_overlays,
            blur_overlays,
            backdrop_overlays,
            chrome_overlays,
            top_layers,
            lower_layers,
            has_overview_layer,
        ) = {
            let layer_map = layer_map_for_output(output);
            let mut front = Vec::new();
            let mut blur_ov = Vec::new();
            let mut backdrop_ov = Vec::new();
            let mut chrome_ov = Vec::new();
            let mut top = Vec::new();
            let mut lower = Vec::new();
            let has_overview_layer = layer_map.layers_on(WlrLayer::Top).any(|l| {
                matches!(l.namespace(), "neon-overview" | "neon-workspaces-panel")
            });

            for layer in layer_map.layers_on(WlrLayer::Overlay).rev() {
                if let Some(geo) = layer_map.layer_geometry(layer) {
                    match layer.namespace() {
                        "neon-context-submenu" => {
                            blur_ov.insert(0, (layer.clone(), geo, 13.0_f32, 22i32));
                        }
                        "neon-context-menu" => {
                            let idx = usize::from(
                                blur_ov
                                    .first()
                                    .is_some_and(|(l, _, _, _)| l.namespace() == "neon-context-submenu"),
                            );
                            blur_ov.insert(idx, (layer.clone(), geo, 13.0_f32, 22i32));
                        }
                        "neon-quick-settings" | "neon-notification-center" => {
                            blur_ov.push((layer.clone(), geo, 24.0_f32, 0i32));
                        }
                        "neon-dock-preview" => {
                            blur_ov.push((layer.clone(), geo, 14.0_f32, 0i32));
                        }
                        "neon-workspaces-panel" => {
                            blur_ov.push((layer.clone(), geo, 16.0_f32, 0i32));
                        }
                        "neon-screencast-picker" => {
                            blur_ov.push((layer.clone(), geo, 22.0_f32, 0i32));
                        }
                        "neon-screenshot-bar" => {
                            blur_ov.push((layer.clone(), geo, 26.0_f32, 18i32));
                        }
                        "neon-power-menu" => {
                            blur_ov.push((layer.clone(), geo, 0.0_f32, 0i32));
                        }
                        "neon-quick-settings-backdrop"
                        | "neon-notification-center-backdrop"
                        | "neon-workspaces-backdrop"
                        | "neon-screencast-picker-backdrop"
                        | "neon-context-menu-backdrop"
                        | "neon-screenshot-ui" => {
                            backdrop_ov.push((layer.clone(), geo.loc));
                        }
                        "neon-panel" | "neon-dock" => {
                            chrome_ov.push((layer.clone(), geo.loc));
                        }
                        _ => {
                            front.push((layer.clone(), geo.loc));
                        }
                    }
                }
            }
            for layer in layer_map.layers_on(WlrLayer::Top).rev() {
                if let Some(geo) = layer_map.layer_geometry(layer)
                    && (!has_fullscreen_window
                        || has_overview_layer
                        || matches!(
                            layer.namespace(),
                            "neon-overview" | "neon-workspaces-panel"
                        ))
                {
                    top.push((layer.clone(), geo));
                }
            }
            for wlr_layer in [WlrLayer::Bottom, WlrLayer::Background] {
                for layer in layer_map.layers_on(wlr_layer).rev() {
                    if let Some(geo) = layer_map.layer_geometry(layer) {
                        lower.push((layer.clone(), geo.loc));
                    }
                }
            }
            (
                front,
                blur_ov,
                backdrop_ov,
                chrome_ov,
                top,
                lower,
                has_overview_layer,
            )
        };

        let ws_slide = scene.workspace_slide();
        let active_blur_strength = if blur.enabled && blur.strength > 0.01 {
            blur.strength
        } else {
            0.0
        };

        // XWayland override-redirect popups (context menus, dropdowns, tooltips) above windows.
        for &(or_win, loc) in override_redirect.iter().rev() {
            let win_geo = or_win.geometry();
            let render_loc: Point<i32, Logical> = (loc.x - win_geo.loc.x, loc.y - win_geo.loc.y).into();
            let phys_loc: Point<i32, Physical> = render_loc.to_physical_precise_round(scale_f);
            let popup_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(or_win, renderer, phys_loc, scale, 1.0);
            elements.extend(popup_elements.into_iter().map(NeonRenderElement::from));
        }

        // Emit active GPU closing-window ghosts above regular windows (and below Top Bar & Dock).
        self.closing_ghosts.retain(|g| !g.is_finished());
        let ghost_rounded_prog = self.shaders.as_ref().map(|s| s.rounded_window.clone());
        for ghost in self.closing_ghosts.iter_mut().rev() {
            if let Some(elem) = ghost.build_element(ghost_rounded_prog.clone()) {
                elements.push(NeonRenderElement::from(elem));
            }
        }

        let output_logical_size = output
            .current_mode()
            .map(|m| {
                let sz = m.size.to_f64().to_logical(scale_f);
                [sz.w as f32, sz.h as f32]
            })
            .unwrap_or([1280.0, 800.0]);

        let mut inserted_lower_layers = false;
        for node in nodes {
            if !inserted_lower_layers && matches!(node.content, NodeContent::Backdrop { .. }) {
                for (layer, loc) in &lower_layers {
                    if let Some(slide) = ws_slide {
                        // Full-desktop Hyprland workspace slide: pan incoming and outgoing
                        // wallpaper layers together with windows and dock across the 28px gutter.
                        let in_loc: Point<i32, Logical> =
                            (loc.x + slide.incoming_dx.round() as i32, loc.y).into();
                        let in_phys: Point<i32, Physical> =
                            in_loc.to_physical_precise_round(scale_f);
                        let in_elems = AsRenderElements::<GlesRenderer>::render_elements::<
                            WaylandSurfaceRenderElement<GlesRenderer>,
                        >(layer, renderer, in_phys, scale, 1.0);
                        elements.extend(in_elems.into_iter().map(NeonRenderElement::from));

                        let out_loc: Point<i32, Logical> =
                            (loc.x + slide.outgoing_dx.round() as i32, loc.y).into();
                        let out_phys: Point<i32, Physical> =
                            out_loc.to_physical_precise_round(scale_f);
                        let out_elems = AsRenderElements::<GlesRenderer>::render_elements::<
                            WaylandSurfaceRenderElement<GlesRenderer>,
                        >(layer, renderer, out_phys, scale, 1.0);
                        let ids = self
                            .ws_slide_layer_ids
                            .entry(format!("out:{}", layer.namespace()))
                            .or_default();
                        while ids.len() < out_elems.len() {
                            ids.push(Id::new());
                        }
                        for (idx, elem) in out_elems.into_iter().enumerate() {
                            elements.push(NeonRenderElement::from(TaggedWaylandElement::new(
                                ids[idx].clone(),
                                elem,
                            )));
                        }
                    } else {
                        let phys_loc: Point<i32, Physical> =
                            loc.to_physical_precise_round(scale_f);
                        let layer_elements =
                            AsRenderElements::<GlesRenderer>::render_elements::<
                                WaylandSurfaceRenderElement<GlesRenderer>,
                            >(layer, renderer, phys_loc, scale, 1.0);
                        elements.extend(layer_elements.into_iter().map(NeonRenderElement::from));
                    }
                }
                inserted_lower_layers = true;
            }

            let eff = node.effective_bounds();
            let eff_rect: Rectangle<i32, Logical> = Rectangle::new(
                (eff.loc.x, eff.loc.y).into(),
                (eff.size.w, eff.size.h).into(),
            );

            match &node.content {
                NodeContent::Window {
                    window_id,
                    fullscreen,
                    ..
                } => {
                    seen_windows.insert(*window_id);

                    let mut skip_glass_backdrop = *fullscreen;
                    if let Some(window) = resolve_window(*window_id) {
                        if let Some(x11) = window.x11_surface()
                            && (x11.is_decorated()
                                || x11.is_popup()
                                || matches!(
                                    x11.window_type(),
                                    Some(
                                        WmWindowType::Splash
                                            | WmWindowType::Notification
                                            | WmWindowType::Tooltip
                                            | WmWindowType::PopupMenu
                                            | WmWindowType::DropdownMenu
                                            | WmWindowType::Menu
                                            | WmWindowType::Toolbar
                                    )
                                ))
                        {
                            // Frameless / CSD / splash X11 windows (such as DaVinci Resolve)
                            // manage their own chrome and transparency; avoid drawing a rectangular
                            // frosted-glass box behind them.
                            skip_glass_backdrop = true;
                        }
                        let win_geo = window.geometry();
                        let (sx, sy) = node.scale;
                        let is_scaled = (sx - 1.0).abs() > 0.001 || (sy - 1.0).abs() > 0.001;
                        let render_loc: Point<i32, Logical> = if is_scaled {
                            (
                                node.bounds.loc.x + node.offset.0.round() as i32 - win_geo.loc.x,
                                node.bounds.loc.y + node.offset.1.round() as i32 - win_geo.loc.y,
                            )
                                .into()
                        } else {
                            (eff.loc.x - win_geo.loc.x, eff.loc.y - win_geo.loc.y).into()
                        };
                        let phys_loc: Point<i32, Physical> =
                            render_loc.to_physical_precise_round(scale_f);

                        let rounded_program = if !*fullscreen
                            && node.corner_radius > 0.5
                            && eff_rect.size.w > 0
                            && eff_rect.size.h > 0
                        {
                            self.shaders.as_ref().map(|s| s.rounded_window.clone())
                        } else {
                            None
                        };

                        if let Some(program) = rounded_program {
                            let win_phys: Rectangle<i32, Physical> =
                                eff_rect.to_physical_precise_round(scale_f);
                            let scale_min = sx.min(sy).clamp(0.1, 4.0);
                            let bot_r_phys = (node.corner_radius as f64 * scale_f * scale_min) as f32;
                            let top_r_phys = if node.titlebar.is_some() { 0.0 } else { bot_r_phys };
                            let quant = (
                                (top_r_phys * 10.0).round() as i32,
                                (bot_r_phys * 10.0).round() as i32,
                            );
                            let radius_changed = self
                                .rounded_radii_cache
                                .insert(*window_id, quant)
                                .is_some_and(|prev| prev != quant);

                            // Emit XDG popups (menus, popovers, tooltips) unclipped above the window.
                            let mut main_surface_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                                Vec::new();
                            match window.underlying_surface() {
                                WindowSurface::Wayland(s) => {
                                    let surface = s.wl_surface();
                                    let popup_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                                        PopupManager::popups_for_surface(surface)
                                            .flat_map(|(popup, popup_offset)| {
                                                let offset = (win_geo.loc + popup_offset
                                                    - popup.geometry().loc)
                                                    .to_physical_precise_round(scale_f);
                                                render_elements_from_surface_tree(
                                                    renderer,
                                                    popup.wl_surface(),
                                                    phys_loc + offset,
                                                    scale,
                                                    node.opacity,
                                                    Kind::Unspecified,
                                                )
                                            })
                                            .collect();
                                    if is_scaled {
                                        let (cx, cy) = node.effective_center();
                                        let center_phys: Point<i32, Physical> =
                                            Point::<f64, Logical>::from((cx, cy))
                                                .to_physical_precise_round(scale_f);
                                        let elem_scale = Scale::from((sx, sy));
                                        elements.extend(popup_elems.into_iter().map(|elem| {
                                            NeonRenderElement::from(
                                                RescaleRenderElement::from_element(
                                                    elem,
                                                    center_phys,
                                                    elem_scale,
                                                ),
                                            )
                                        }));
                                    } else {
                                        elements.extend(
                                            popup_elems.into_iter().map(NeonRenderElement::from),
                                        );
                                    }

                                    main_surface_elements.extend(
                                        render_elements_from_surface_tree(
                                            renderer,
                                            surface,
                                            phys_loc,
                                            scale,
                                            node.opacity,
                                            Kind::Unspecified,
                                        ),
                                    );
                                }
                                WindowSurface::X11(s) => {
                                    main_surface_elements.extend(
                                        AsRenderElements::<GlesRenderer>::render_elements::<
                                            WaylandSurfaceRenderElement<GlesRenderer>,
                                        >(
                                            s, renderer, phys_loc, scale, node.opacity
                                        ),
                                    );
                                }
                            }

                            let rounded_iter = main_surface_elements.into_iter().map(|elem| {
                                RoundedSurfaceElement::new(
                                    elem,
                                    program.clone(),
                                    win_phys,
                                    top_r_phys,
                                    bot_r_phys,
                                    radius_changed,
                                )
                            });
                            if is_scaled {
                                let (cx, cy) = node.effective_center();
                                let center_phys: Point<i32, Physical> =
                                    Point::<f64, Logical>::from((cx, cy))
                                        .to_physical_precise_round(scale_f);
                                let elem_scale = Scale::from((sx, sy));
                                elements.extend(rounded_iter.map(|elem| {
                                    NeonRenderElement::from(RescaleRenderElement::from_element(
                                        elem,
                                        center_phys,
                                        elem_scale,
                                    ))
                                }));
                            } else {
                                elements.extend(rounded_iter.map(NeonRenderElement::from));
                            }
                        } else {
                            self.rounded_radii_cache.remove(window_id);
                            let surface_elements =
                                AsRenderElements::<GlesRenderer>::render_elements::<
                                    WaylandSurfaceRenderElement<GlesRenderer>,
                                >(
                                    window, renderer, phys_loc, scale, node.opacity
                                );
                            if is_scaled {
                                let (cx, cy) = node.effective_center();
                                let center_phys: Point<i32, Physical> =
                                    Point::<f64, Logical>::from((cx, cy))
                                        .to_physical_precise_round(scale_f);
                                let elem_scale = Scale::from((sx, sy));
                                elements.extend(surface_elements.into_iter().map(|elem| {
                                    NeonRenderElement::from(RescaleRenderElement::from_element(
                                        elem,
                                        center_phys,
                                        elem_scale,
                                    ))
                                }));
                            } else {
                                elements.extend(
                                    surface_elements.into_iter().map(NeonRenderElement::from),
                                );
                            }
                        }
                    }

                    let scaled_titlebar = node.titlebar.map(|mut tb| {
                        let sy = node.scale.1;
                        if (sy - 1.0).abs() > 0.001 {
                            tb.height = (tb.height as f64 * sy).round().max(1.0) as i32;
                        }
                        tb
                    });

                    if let Some(titlebar) = scaled_titlebar
                        && !fullscreen
                        && eff_rect.size.w > 0
                        && eff_rect.size.h > 0
                        && let Some(elem) = self.get_or_update_titlebar(
                            *window_id,
                            eff_rect,
                            titlebar,
                            node.opacity,
                        )
                    {
                        elements.push(NeonRenderElement::from(elem));
                    }

                    let frame_rect = if let Some(tb) = scaled_titlebar
                        && !fullscreen
                    {
                        Rectangle::new(
                            (eff_rect.loc.x, eff_rect.loc.y - tb.height).into(),
                            (eff_rect.size.w, eff_rect.size.h + tb.height).into(),
                        )
                    } else {
                        eff_rect
                    };

                    if blur.enabled
                        && !skip_glass_backdrop
                        && frame_rect.size.w > 0
                        && frame_rect.size.h > 0
                        && let Some(elem) = self.get_or_update_glass(
                            *window_id,
                            frame_rect,
                            output_logical_size,
                            BlurSettings {
                                corner_radius: node.corner_radius,
                                ..blur
                            },
                            node.opacity,
                        )
                    {
                        elements.push(NeonRenderElement::from(elem));
                    }

                    if let Some(border) = node.border
                        && frame_rect.size.w > 0
                        && frame_rect.size.h > 0
                        && let Some(elem) =
                            self.get_or_update_border(*window_id, frame_rect, border, node.opacity)
                    {
                        elements.push(NeonRenderElement::from(elem));
                    }

                    if let Some(shadow) = node.shadow
                        && frame_rect.size.w > 0
                        && frame_rect.size.h > 0
                        && let Some(elem) =
                            self.get_or_update_shadow(*window_id, frame_rect, shadow, node.opacity)
                    {
                        elements.push(NeonRenderElement::from(elem));
                    }
                }
                NodeContent::Backdrop {
                    primary_color,
                    accent_color,
                } => {
                    if eff_rect.size.w > 0
                        && eff_rect.size.h > 0
                        && let Some(elem) =
                            self.get_or_update_backdrop(eff_rect, *primary_color, *accent_color)
                    {
                        elements.push(NeonRenderElement::from(elem));
                    }
                }
                NodeContent::SolidRect { color } => {
                    if eff_rect.size.w > 0 && eff_rect.size.h > 0 {
                        let buf = SolidColorBuffer::new(
                            eff_rect.size,
                            Color32F::new(color[0], color[1], color[2], color[3] * node.opacity),
                        );
                        let elem = SolidColorRenderElement::from_buffer(
                            &buf,
                            eff_rect.loc.to_physical_precise_round(scale_f),
                            scale_f,
                            node.opacity,
                            Kind::Unspecified,
                        );
                        if node.layer >= SceneLayer::Overview {
                            front_scene_elements.push(NeonRenderElement::from(elem));
                        } else {
                            elements.push(NeonRenderElement::from(elem));
                        }
                    }
                }
                NodeContent::RoundedRect {
                    fill_color,
                    border_color,
                    border_width,
                } => {
                    if eff_rect.size.w > 0 && eff_rect.size.h > 0 {
                        if let Some(program) = self.shaders.as_ref().map(|s| s.rounded_rect.clone())
                        {
                            let uniforms = vec![
                                Uniform::new("u_corner_radius", node.corner_radius),
                                Uniform::new("u_border_width", *border_width),
                                Uniform::new("u_fill_color", *fill_color),
                                Uniform::new("u_border_color", *border_color),
                                Uniform::new("u_opacity", node.opacity),
                            ];
                            let elem = PixelShaderElement::new(
                                program,
                                eff_rect,
                                None,
                                1.0,
                                uniforms,
                                Kind::Unspecified,
                            );
                            if node.layer >= SceneLayer::Overview {
                                front_scene_elements.push(NeonRenderElement::from(elem));
                            } else {
                                elements.push(NeonRenderElement::from(elem));
                            }
                        } else {
                            let buf = SolidColorBuffer::new(
                                eff_rect.size,
                                Color32F::new(
                                    fill_color[0],
                                    fill_color[1],
                                    fill_color[2],
                                    fill_color[3] * node.opacity,
                                ),
                            );
                            let elem = SolidColorRenderElement::from_buffer(
                                &buf,
                                eff_rect.loc.to_physical_precise_round(scale_f),
                                scale_f,
                                node.opacity,
                                Kind::Unspecified,
                            );
                            if node.layer >= SceneLayer::Overview {
                                front_scene_elements.push(NeonRenderElement::from(elem));
                            } else {
                                elements.push(NeonRenderElement::from(elem));
                            }
                        }
                    }
                }
            }
        }

        // Prune cached shader elements for windows that are no longer visible or were closed.
        self.shadow_cache.retain(|id, _| seen_windows.contains(id));
        self.border_cache.retain(|id, _| seen_windows.contains(id));
        self.glass_cache.retain(|id, _| seen_windows.contains(id));
        self.titlebar_cache.retain(|id, _| seen_windows.contains(id));
        self.rounded_radii_cache
            .retain(|id, _| seen_windows.contains(id));

        // Prepend `Layer::Top` chrome (Top Bar, Dock, App Drawer) above windows & wallpaper,
        // applying unified GPU backdrop blur behind Top Bar and Dock from `active_blur_strength`.
        let has_blur_chrome = !has_overview_layer
            && top_layers
                .iter()
                .any(|(l, _)| matches!(l.namespace(), "neon-panel" | "neon-dock"));
        let chrome_blur = if active_blur_strength > 0.01
            && has_blur_chrome
            && ws_slide.is_none()
            && let Some(shaders) = self.shaders.clone()
        {
            self.chrome_blur_targets
                .render_blurred_scene(
                    renderer,
                    output,
                    &shaders,
                    &elements,
                    false,
                    active_blur_strength,
                    CHROME_BLUR_CACHE_TTL,
                )
                .map(|(tex, out_phys, commit, blend_alpha)| {
                    if commit != self.last_chrome_blur_tex_commit {
                        self.last_chrome_blur_tex_commit = commit;
                        self.chrome_blur_commit.increment();
                    }
                    (
                        tex,
                        out_phys,
                        self.chrome_blur_commit,
                        shaders.rounded_window,
                        blend_alpha,
                    )
                })
        } else {
            if ws_slide.is_some() || active_blur_strength <= 0.01 {
                self.chrome_blur_targets.invalidate();
            }
            None
        };

        let mut scene_below_overlay: Vec<NeonRenderElement> =
            Vec::with_capacity(elements.len() + top_layers.len() * 3 + 4);
        for (layer, geo) in &top_layers {
            let ns = layer.namespace();
            if ns == "neon-dock"
                && let Some(slide) = ws_slide
            {
                // Full-desktop Hyprland workspace slide: Dock glides out with the departing
                // desktop while the incoming desktop's Dock enters smoothly from the opposite side.
                let in_loc: Point<i32, Logical> =
                    (geo.loc.x + slide.incoming_dx.round() as i32, geo.loc.y).into();
                let in_phys: Point<i32, Physical> = in_loc.to_physical_precise_round(scale_f);
                let in_elems = AsRenderElements::<GlesRenderer>::render_elements::<
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(layer, renderer, in_phys, scale, 1.0);
                scene_below_overlay.extend(in_elems.into_iter().map(NeonRenderElement::from));

                let out_loc: Point<i32, Logical> =
                    (geo.loc.x + slide.outgoing_dx.round() as i32, geo.loc.y).into();
                let out_phys: Point<i32, Physical> = out_loc.to_physical_precise_round(scale_f);
                let out_elems = AsRenderElements::<GlesRenderer>::render_elements::<
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(layer, renderer, out_phys, scale, 1.0);
                let ids = self
                    .ws_slide_layer_ids
                    .entry("out:neon-dock".to_string())
                    .or_default();
                while ids.len() < out_elems.len() {
                    ids.push(Id::new());
                }
                for (idx, elem) in out_elems.into_iter().enumerate() {
                    scene_below_overlay.push(NeonRenderElement::from(TaggedWaylandElement::new(
                        ids[idx].clone(),
                        elem,
                    )));
                }
                continue;
            }

            let phys_loc: Point<i32, Physical> = geo.loc.to_physical_precise_round(scale_f);
            let layer_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(layer, renderer, phys_loc, scale, 1.0);
            let has_committed = !layer_elements.is_empty();
            scene_below_overlay.extend(layer_elements.into_iter().map(NeonRenderElement::from));

            if has_committed
                && geo.size.w > 0
                && geo.size.h > 0
                && let Some((ref blur_tex, out_phys, commit, ref rounded_prog, blend_alpha)) =
                    chrome_blur
            {
                let blur_rect_and_radius: Option<(Rectangle<i32, Logical>, f32)> = match ns {
                    "neon-panel" => Some((*geo, 0.0)),
                    "neon-dock" => {
                        let out_w_logical = ((out_phys.w as f64) / scale_f).round() as i32;
                        if geo.size.w >= out_w_logical - 8 && geo.loc.x <= 4 {
                            // Full-width Combined Bottom Taskbar renders edge-to-edge without outer padding.
                            Some((*geo, 0.0))
                        } else if geo.size.h > geo.size.w {
                            // Vertical Left/Right dock wrapper uses `padding: 16px 7px;`
                            let pw = geo.size.w - 14;
                            let ph = geo.size.h - 32;
                            if pw > 16 && ph > 16 {
                                let rad = if pw <= 42 { 15.0 } else { 20.0 };
                                Some((
                                    Rectangle::new(
                                        (geo.loc.x + 7, geo.loc.y + 16).into(),
                                        (pw, ph).into(),
                                    ),
                                    rad,
                                ))
                            } else {
                                None
                            }
                        } else {
                            // Horizontal Bottom dock wrapper uses `padding: 16px 24px 5px 24px;`
                            // (16px top headroom for macOS wave magnification without clipping).
                            let pw = geo.size.w - 48;
                            let ph = geo.size.h - 21;
                            if pw > 16 && ph > 16 {
                                let rad = if ph <= 42 { 15.0 } else { 20.0 };
                                Some((
                                    Rectangle::new(
                                        (geo.loc.x + 24, geo.loc.y + 16).into(),
                                        (pw, ph).into(),
                                    ),
                                    rad,
                                ))
                            } else {
                                None
                            }
                        }
                    }
                    _ => None,
                };
                if let Some((blur_rect, radius_logical)) = blur_rect_and_radius {
                    let dst_phys: Rectangle<i32, Physical> =
                        blur_rect.to_physical_precise_round(scale_f);
                    let elem_id = self
                        .blur_element_ids
                        .entry(format!("chrome:{ns}"))
                        .or_insert_with(Id::new)
                        .clone();
                    let radius_phys = radius_logical * scale_f as f32;
                    if let Some(blur_elem) = BlurredBackdropElement::new(
                        elem_id,
                        commit,
                        blur_tex.clone(),
                        out_phys,
                        dst_phys,
                        radius_phys,
                        (radius_phys > 0.5).then(|| rounded_prog.clone()),
                        blend_alpha,
                    ) {
                        scene_below_overlay.push(NeonRenderElement::from(blur_elem));
                    }
                }
            }
        }
        scene_below_overlay.extend(elements);
        // Prepend elevated Top Bar & Dock (`chrome_overlays`) cleanly above `Layer::Top`
        // fullscreen backdrops (`neon-overview`, `neon-workspaces-panel`), while keeping them
        // strictly BELOW popups (`neon-context-menu`, `neon-quick-settings`, `neon-power-menu`,
        // `neon-screenshot-ui`, `neon-notifications`) and their click-catcher backdrops.
        let elements = if chrome_overlays.is_empty() {
            scene_below_overlay
        } else {
            let mut with_chrome =
                Vec::with_capacity(scene_below_overlay.len() + chrome_overlays.len() * 2);
            for (layer, loc) in &chrome_overlays {
                let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(scale_f);
                let layer_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(layer, renderer, phys_loc, scale, 1.0);
                with_chrome.extend(layer_elements.into_iter().map(NeonRenderElement::from));
            }
            with_chrome.extend(scene_below_overlay);
            with_chrome
        };

        // Fast path when no `Layer::Overlay` surfaces are mapped (0 extra allocations / GPU passes).
        if blur_overlays.is_empty() {
            self.blur_targets.invalidate();
            self.mapped_blur_overlays.clear();
            // Keep any fresh IPC PendingOpen / Opening trigger until GTK commits its first buffer.
            self.overlay_anims.retain(|_, anim| {
                matches!(
                    anim,
                    OverlayAnim::PendingOpen { queued } if queued.elapsed().as_millis() < 600
                ) || matches!(
                    anim,
                    OverlayAnim::Opening { started } if started.elapsed().as_millis() < 150
                )
            });
        }
        if front_overlays.is_empty()
            && blur_overlays.is_empty()
            && backdrop_overlays.is_empty()
            && front_scene_elements.is_empty()
        {
            return elements;
        }

        // Pre-compute cached lightweight GPU Gaussian backdrop blur when Quick Settings,
        // Workspaces Sidebar, or Power Menu is open and unified System Blur is active (> 0%).
        let blurred_backdrop = if !blur_overlays.is_empty()
            && active_blur_strength > 0.01
            && let Some(shaders) = self.shaders.clone()
        {
            let heavy = blur_overlays
                .iter()
                .any(|(l, _, _, _)| l.namespace() == "neon-power-menu");
            self.blur_targets
                .render_blurred_scene(
                    renderer,
                    output,
                    &shaders,
                    &elements,
                    heavy,
                    active_blur_strength,
                    OVERLAY_BLUR_CACHE_TTL,
                )
                .map(|(tex, out_phys, commit, blend_alpha)| {
                    (tex, out_phys, commit, shaders.rounded_window, blend_alpha)
                })
        } else {
            if active_blur_strength <= 0.01 {
                self.blur_targets.invalidate();
            }
            None
        };

        let mut final_elements: Vec<NeonRenderElement> =
            Vec::with_capacity(elements.len() + front_overlays.len() * 2 + blur_overlays.len() * 3 + 4);

        for (layer, loc) in &front_overlays {
            let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(scale_f);
            let layer_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(layer, renderer, phys_loc, scale, 1.0);
            final_elements.extend(layer_elements.into_iter().map(NeonRenderElement::from));
        }
        let front_overlay_elem_count = final_elements.len();

        let out_h_logical = output
            .current_mode()
            .map(|m| ((f64::from(m.size.h)) / scale_f).round() as i32)
            .unwrap_or(1080);

        let mut seen_blur_ns: HashSet<String> = HashSet::new();
        for (layer, geo, corner_radius_logical, blur_inset_logical) in &blur_overlays {
            let ns = layer.namespace();
            seen_blur_ns.insert(ns.to_string());

            let base_phys: Rectangle<i32, Physical> = geo.to_physical_precise_round(scale_f);
            // Probe whether GTK has attached a real painted buffer yet before starting the
            // opening clock so frame 0 of every overlay slide always begins cleanly at `t = 0.0`.
            let probe_has_buffer = geo.size.w > 0
                && geo.size.h > 0
                && !AsRenderElements::<GlesRenderer>::render_elements::<
                    WaylandSurfaceRenderElement<GlesRenderer>,
                >(layer, renderer, base_phys.loc, scale, 1.0)
                .is_empty();

            if probe_has_buffer {
                let first_commit = self.mapped_blur_overlays.insert(ns.to_string());
                if first_commit
                    || matches!(
                        self.overlay_anims.get(ns),
                        Some(OverlayAnim::PendingOpen { .. })
                    )
                {
                    self.overlay_anims.insert(
                        ns.to_string(),
                        OverlayAnim::Opening {
                            started: Instant::now(),
                        },
                    );
                }
            } else {
                continue;
            }

            let (offset_x, raw_offset_y, overlay_scale, alpha, is_animating) =
                Self::eval_overlay_anim(
                    ns,
                    self.overlay_anims.get(ns).copied(),
                    self.overlay_motion_enabled,
                    self.overlay_speed_scale,
                );
            if alpha <= 0.005 {
                continue;
            }

            let bottom_anchored = matches!(
                ns,
                "neon-quick-settings" | "neon-notification-center" | "neon-dock-preview"
            ) && geo.loc.y + geo.size.h / 2 > out_h_logical / 2;
            let offset_y = if bottom_anchored {
                -raw_offset_y
            } else {
                raw_offset_y
            };

            let dx_phys = (offset_x as f64 * scale_f).round() as i32;
            let dy_phys = (offset_y as f64 * scale_f).round() as i32;
            let shifted_loc: Point<i32, Physical> =
                (base_phys.loc.x + dx_phys, base_phys.loc.y + dy_phys).into();
            let anchor_phys: Point<i32, Physical> = if bottom_anchored {
                (
                    shifted_loc.x + base_phys.size.w / 2,
                    shifted_loc.y + base_phys.size.h,
                )
                    .into()
            } else {
                (shifted_loc.x + base_phys.size.w / 2, shifted_loc.y).into()
            };
            let is_scaled = (overlay_scale - 1.0).abs() > 0.001;

            let layer_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(layer, renderer, shifted_loc, scale, alpha);
            let has_committed = !layer_elements.is_empty();
            if is_scaled {
                let elem_scale = Scale::from((overlay_scale as f64, overlay_scale as f64));
                final_elements.extend(layer_elements.into_iter().map(|elem| {
                    NeonRenderElement::from(RescaleRenderElement::from_element(
                        elem,
                        anchor_phys,
                        elem_scale,
                    ))
                }));
            } else {
                final_elements.extend(layer_elements.into_iter().map(NeonRenderElement::from));
            }

            let inset = *blur_inset_logical;
            let blur_w_logical = geo.size.w - inset * 2;
            let blur_h_logical = geo.size.h - inset * 2;
            if has_committed
                && blur_w_logical > 8
                && blur_h_logical > 8
                && let Some((ref blur_tex, out_phys, commit, ref rounded_prog, blend_alpha)) =
                    blurred_backdrop
            {
                if is_animating || commit != self.last_blur_tex_commit {
                    self.last_blur_tex_commit = commit;
                    self.blur_anim_commit.increment();
                }
                let inset_phys = (inset as f64 * scale_f).round() as i32;
                let inner_loc: Point<i32, Physical> =
                    (shifted_loc.x + inset_phys, shifted_loc.y + inset_phys).into();
                let inner_size: Size<i32, Physical> = (
                    (base_phys.size.w - inset_phys * 2).max(1),
                    (base_phys.size.h - inset_phys * 2).max(1),
                )
                    .into();
                let dst_phys: Rectangle<i32, Physical> = if is_scaled {
                    let sw = ((inner_size.w as f64) * overlay_scale as f64)
                        .round()
                        .max(1.0) as i32;
                    let sh = ((inner_size.h as f64) * overlay_scale as f64)
                        .round()
                        .max(1.0) as i32;
                    let scaled_inset_y = (inset_phys as f64 * overlay_scale as f64).round() as i32;
                    let dst_y = if bottom_anchored {
                        anchor_phys.y - scaled_inset_y - sh
                    } else {
                        anchor_phys.y + scaled_inset_y
                    };
                    Rectangle::new((anchor_phys.x - sw / 2, dst_y).into(), (sw, sh).into())
                } else {
                    Rectangle::new(inner_loc, inner_size)
                };
                let elem_id = self
                    .blur_element_ids
                    .entry(ns.to_string())
                    .or_insert_with(Id::new)
                    .clone();
                let radius_phys = *corner_radius_logical * scale_f as f32 * overlay_scale;
                if let Some(blur_elem) = BlurredBackdropElement::new(
                    elem_id,
                    self.blur_anim_commit,
                    blur_tex.clone(),
                    out_phys,
                    dst_phys,
                    radius_phys,
                    (radius_phys > 0.5).then(|| rounded_prog.clone()),
                    alpha * blend_alpha,
                ) {
                    final_elements.push(NeonRenderElement::from(blur_elem));
                }
            }
        }
        self.mapped_blur_overlays
            .retain(|ns| seen_blur_ns.contains(ns));
        self.overlay_anims.retain(|ns, anim| {
            seen_blur_ns.contains(ns)
                || matches!(
                    anim,
                    OverlayAnim::PendingOpen { queued } if queued.elapsed().as_millis() < 600
                )
                || matches!(
                    anim,
                    OverlayAnim::Opening { started } if started.elapsed().as_millis() < 150
                )
        });

        for (layer, loc) in &backdrop_overlays {
            let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(scale_f);
            let layer_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(layer, renderer, phys_loc, scale, 1.0);
            final_elements.extend(layer_elements.into_iter().map(NeonRenderElement::from));
        }

        if !front_scene_elements.is_empty() {
            // Place topmost scene overlays (e.g. Windows 11 Snap Layouts drop bar) right above
            // Top Bar & Dock while remaining cleanly under cursor & front alerts.
            let insert_at = front_overlay_elem_count.min(final_elements.len());
            final_elements.splice(insert_at..insert_at, front_scene_elements);
        }

        final_elements.extend(elements);
        final_elements
    }

    /// Renders the retained [`SceneGraph`] into `framebuffer` with full `OutputDamageTracker` support.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene<'a, F>(
        &'a mut self,
        output: &Output,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
        age: usize,
        scene: &SceneGraph,
        blur: BlurSettings,
        resolve_window: F,
    ) -> Result<RenderOutputResult<'a>, OutputDamageTrackerError<GlesError>>
    where
        F: FnMut(WindowId) -> Option<&'a Window>,
    {
        self.render_scene_with_popups(output, renderer, framebuffer, age, scene, blur, &[], resolve_window)
    }

    /// Renders the retained [`SceneGraph`] including XWayland override-redirect popup windows.
    #[allow(clippy::too_many_arguments)]
    pub fn render_scene_with_popups<'a, F>(
        &'a mut self,
        output: &Output,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
        age: usize,
        scene: &SceneGraph,
        blur: BlurSettings,
        override_redirect: &[(&Window, Point<i32, Logical>)],
        resolve_window: F,
    ) -> Result<RenderOutputResult<'a>, OutputDamageTrackerError<GlesError>>
    where
        F: FnMut(WindowId) -> Option<&'a Window>,
    {
        let elements = self.collect_scene_elements_with_popups(
            output,
            renderer,
            scene,
            blur,
            override_redirect,
            resolve_window,
        );

        self.damage_tracker.render_output(
            renderer,
            framebuffer,
            age,
            &elements,
            FALLBACK_CLEAR_COLOR,
        )
    }

    fn get_or_update_titlebar(
        &mut self,
        id: WindowId,
        window_rect: Rectangle<i32, Logical>,
        style: TitlebarStyle,
        opacity: f32,
    ) -> Option<PixelShaderElement> {
        let program = self.shaders.as_ref()?.titlebar.clone();
        let area = Rectangle::new(
            (window_rect.loc.x, window_rect.loc.y - style.height).into(),
            (window_rect.size.w, style.height).into(),
        );

        let make_uniforms = || {
            vec![
                Uniform::new("u_corner_radius", style.corner_radius),
                Uniform::new("u_focused", if style.focused { 1.0_f32 } else { 0.0_f32 }),
                Uniform::new(
                    "u_light_theme",
                    if style.light_theme { 1.0_f32 } else { 0.0_f32 },
                ),
                Uniform::new(
                    "u_hover_controls",
                    if style.hover_controls { 1.0_f32 } else { 0.0_f32 },
                ),
                Uniform::new(
                    "u_buttons_right",
                    if style.buttons_right { 1.0_f32 } else { 0.0_f32 },
                ),
                Uniform::new("u_opacity", opacity),
            ]
        };

        if let Some(cached) = self.titlebar_cache.get_mut(&id) {
            if cached.area != area {
                cached.element.resize(area, None);
                cached.area = area;
            }
            if cached.style != style || (cached.opacity - opacity).abs() > 1e-4 {
                cached.element.update_uniforms(make_uniforms());
                cached.style = style;
                cached.opacity = opacity;
            }
            return Some(cached.element.clone());
        }

        let element =
            PixelShaderElement::new(program, area, None, 1.0, make_uniforms(), Kind::Unspecified);
        self.titlebar_cache.insert(
            id,
            CachedTitlebar {
                element: element.clone(),
                area,
                style,
                opacity,
            },
        );
        Some(element)
    }

    fn get_or_update_glass(
        &mut self,
        id: WindowId,
        window_rect: Rectangle<i32, Logical>,
        output_size: [f32; 2],
        blur: BlurSettings,
        opacity: f32,
    ) -> Option<PixelShaderElement> {
        let program = self.shaders.as_ref()?.frosted_glass.clone();
        let window_origin = [window_rect.loc.x as f32, window_rect.loc.y as f32];
        let primary = [0.047_f32, 0.059, 0.090, 1.0];
        let accent = [0.090_f32, 0.145, 0.245, 1.0];

        let make_uniforms = || {
            vec![
                Uniform::new("u_output_size", output_size),
                Uniform::new("u_window_origin", window_origin),
                Uniform::new("u_corner_radius", blur.corner_radius),
                Uniform::new("u_blur_strength", blur.strength),
                Uniform::new("u_opacity", opacity),
                Uniform::new("u_primary", primary),
                Uniform::new("u_accent", accent),
            ]
        };

        if let Some(cached) = self.glass_cache.get_mut(&id) {
            if cached.area != window_rect {
                cached.element.resize(window_rect, None);
                cached.area = window_rect;
            }
            if cached.output_size != output_size
                || cached.window_origin != window_origin
                || (cached.corner_radius - blur.corner_radius).abs() > 1e-4
                || (cached.blur_strength - blur.strength).abs() > 1e-4
                || (cached.opacity - opacity).abs() > 1e-4
            {
                cached.element.update_uniforms(make_uniforms());
                cached.output_size = output_size;
                cached.window_origin = window_origin;
                cached.corner_radius = blur.corner_radius;
                cached.blur_strength = blur.strength;
                cached.opacity = opacity;
            }
            return Some(cached.element.clone());
        }

        let element = PixelShaderElement::new(
            program,
            window_rect,
            None,
            1.0,
            make_uniforms(),
            Kind::Unspecified,
        );
        self.glass_cache.insert(
            id,
            CachedGlass {
                element: element.clone(),
                area: window_rect,
                output_size,
                window_origin,
                corner_radius: blur.corner_radius,
                blur_strength: blur.strength,
                opacity,
            },
        );
        Some(element)
    }

    fn get_or_update_shadow(
        &mut self,
        id: WindowId,
        window_rect: Rectangle<i32, Logical>,
        style: ShadowStyle,
        opacity: f32,
    ) -> Option<PixelShaderElement> {
        let program = self.shaders.as_ref()?.shadow.clone();
        let pad = style.spread.ceil() as i32;
        let offset_y = style.offset_y.round() as i32;
        let area = Rectangle::new(
            (window_rect.loc.x - pad, window_rect.loc.y - pad + offset_y).into(),
            (window_rect.size.w + pad * 2, window_rect.size.h + pad * 2).into(),
        );
        let rect_size = [window_rect.size.w as f32, window_rect.size.h as f32];

        let make_uniforms = || {
            vec![
                Uniform::new("u_rect_size", rect_size),
                Uniform::new("u_corner_radius", style.corner_radius),
                Uniform::new("u_spread", style.spread),
                Uniform::new("u_color", style.color),
                Uniform::new("u_opacity", opacity),
            ]
        };

        if let Some(cached) = self.shadow_cache.get_mut(&id) {
            if cached.area != area {
                cached.element.resize(area, None);
                cached.area = area;
            }
            if cached.rect_size != rect_size
                || cached.style != style
                || (cached.opacity - opacity).abs() > 1e-4
            {
                cached.element.update_uniforms(make_uniforms());
                cached.rect_size = rect_size;
                cached.style = style;
                cached.opacity = opacity;
            }
            return Some(cached.element.clone());
        }

        let element =
            PixelShaderElement::new(program, area, None, 1.0, make_uniforms(), Kind::Unspecified);
        self.shadow_cache.insert(
            id,
            CachedShadow {
                element: element.clone(),
                area,
                rect_size,
                style,
                opacity,
            },
        );
        Some(element)
    }

    fn get_or_update_border(
        &mut self,
        id: WindowId,
        window_rect: Rectangle<i32, Logical>,
        style: BorderStyle,
        opacity: f32,
    ) -> Option<PixelShaderElement> {
        let program = self.shaders.as_ref()?.border.clone();
        let pad = style.width.ceil() as i32;
        let area = Rectangle::new(
            (window_rect.loc.x - pad, window_rect.loc.y - pad).into(),
            (window_rect.size.w + pad * 2, window_rect.size.h + pad * 2).into(),
        );

        let make_uniforms = || {
            vec![
                Uniform::new("u_corner_radius", style.corner_radius),
                Uniform::new("u_border_width", style.width),
                Uniform::new("u_color", style.color),
                Uniform::new("u_opacity", opacity),
            ]
        };

        if let Some(cached) = self.border_cache.get_mut(&id) {
            if cached.area != area {
                cached.element.resize(area, None);
                cached.area = area;
            }
            if cached.style != style || (cached.opacity - opacity).abs() > 1e-4 {
                cached.element.update_uniforms(make_uniforms());
                cached.style = style;
                cached.opacity = opacity;
            }
            return Some(cached.element.clone());
        }

        let element =
            PixelShaderElement::new(program, area, None, 1.0, make_uniforms(), Kind::Unspecified);
        self.border_cache.insert(
            id,
            CachedBorder {
                element: element.clone(),
                area,
                style,
                opacity,
            },
        );
        Some(element)
    }

    fn get_or_update_backdrop(
        &mut self,
        area: Rectangle<i32, Logical>,
        primary: [f32; 4],
        accent: [f32; 4],
    ) -> Option<PixelShaderElement> {
        let program = self.shaders.as_ref()?.backdrop.clone();
        let opaque = Some(vec![Rectangle::from_size(Size::from((
            area.size.w,
            area.size.h,
        )))]);

        let make_uniforms = || {
            vec![
                Uniform::new("u_primary", primary),
                Uniform::new("u_accent", accent),
            ]
        };

        if let Some(cached) = self.backdrop_cache.as_mut() {
            if cached.area != area {
                cached.element.resize(area, opaque);
                cached.area = area;
            }
            if cached.primary != primary || cached.accent != accent {
                cached.element.update_uniforms(make_uniforms());
                cached.primary = primary;
                cached.accent = accent;
            }
            return Some(cached.element.clone());
        }

        let element = PixelShaderElement::new(
            program,
            area,
            opaque,
            1.0,
            make_uniforms(),
            Kind::Unspecified,
        );
        self.backdrop_cache = Some(CachedBackdrop {
            element: element.clone(),
            area,
            primary,
            accent,
        });
        Some(element)
    }
}
