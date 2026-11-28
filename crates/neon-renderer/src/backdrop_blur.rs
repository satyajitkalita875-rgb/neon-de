//! Real-time cached GPU Gaussian backdrop blur for shell overlays
//! (`neon-quick-settings`, `neon-workspaces-panel`, and `neon-power-menu`).
//!
//! Renders the live desktop scene below the overlay into an offscreen `GlesTexture`, runs a
//! fast 2-pass separable 9-tap Gaussian blur (half-resolution light frost for Quick Settings,
//! quarter-resolution deep frost for Power Menu), caches the blurred texture across animation
//! frames for zero-stutter 60 FPS open/close/dropdown motion, and emits a
//! [`BlurredBackdropElement`] clipped cleanly to the overlay's exact rounded geometry.

use std::time::{Duration, Instant};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame, Offscreen, Renderer, Texture,
            damage::OutputDamageTracker,
            element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
            gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
            utils::{CommitCounter, DamageSet, OpaqueRegions},
        },
    },
    output::Output,
    utils::{Buffer as BufferCoord, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::{
    FALLBACK_CLEAR_COLOR, NeonRenderElement, rounded::compute_uv_to_dst, shaders::NeonShaders,
};

/// Default cache TTL for animated shell overlays (Quick Settings, Workspaces, Power Menu) so
/// spring open/close animations stay locked at 60 FPS with 0ms blur overhead.
pub const OVERLAY_BLUR_CACHE_TTL: Duration = Duration::from_millis(160);

/// Short cache TTL for always-visible Top Bar & Dock chrome blur so scrolling or dragging windows
/// behind the Top Bar or Dock updates smoothly in real time (idle frames skip rendering completely).
pub const CHROME_BLUR_CACHE_TTL: Duration = Duration::from_millis(32);

/// Reusable offscreen textures for zero-allocation, cached backdrop blur across frames.
#[derive(Debug, Default)]
pub struct BlurTargets {
    full: Option<(Size<i32, BufferCoord>, GlesTexture)>,
    ping: Option<(Size<i32, BufferCoord>, GlesTexture)>,
    pong: Option<(Size<i32, BufferCoord>, GlesTexture)>,
    commit: CommitCounter,
    cached_at: Option<Instant>,
    cached_out_size: Option<Size<i32, Physical>>,
    cached_heavy: Option<bool>,
    cached_strength_q: Option<u32>,
}

impl BlurTargets {
    /// Returns the latest full-resolution upright offscreen desktop snapshot (if available).
    pub fn full_texture(&self) -> Option<(GlesTexture, Size<i32, Physical>)> {
        let (sz, tex) = self.full.as_ref()?;
        let phys = self
            .cached_out_size
            .unwrap_or_else(|| Size::from((sz.w, sz.h)));
        Some((tex.clone(), phys))
    }

    /// Clears the cached snapshot so the next render captures a fresh desktop frame.
    pub fn invalidate(&mut self) {
        self.cached_at = None;
        self.cached_heavy = None;
        self.cached_strength_q = None;
    }

    fn ensure_tex(
        slot: &mut Option<(Size<i32, BufferCoord>, GlesTexture)>,
        renderer: &mut GlesRenderer,
        size: Size<i32, BufferCoord>,
    ) -> Result<(), GlesError> {
        let needs_alloc = slot.as_ref().is_none_or(|(sz, _)| *sz != size);
        if needs_alloc {
            let tex = renderer.create_buffer(Fourcc::Abgr8888, size)?;
            *slot = Some((size, tex));
        }
        Ok(())
    }

    /// Returns a blurred snapshot of `scene_below` `(tex, out_phys, commit, blend_alpha)` driven
    /// continuously by the unified system `blur_strength` (`0.0 ..= 2.0`).
    ///
    /// Returns `None` when `blur_strength <= 0.01` so `0%` system blur performs zero GPU passes.
    #[allow(clippy::too_many_arguments)]
    pub fn render_blurred_scene(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &Output,
        shaders: &NeonShaders,
        scene_below: &[NeonRenderElement],
        heavy: bool,
        blur_strength: f32,
        cache_ttl: Duration,
    ) -> Option<(GlesTexture, Size<i32, Physical>, CommitCounter, f32)> {
        if blur_strength <= 0.01 {
            return None;
        }
        let mode = output.current_mode()?;
        // Use logical-aligned physical dimensions so rotated outputs (`90°`/`270°`) also match
        // element coordinates cleanly, independent of backend swapchain transform (`Normal` on DRM
        // vs `Flipped180` on Winit).
        let out_phys: Size<i32, Physical> = output.current_transform().transform_size(mode.size);
        let out_w = out_phys.w;
        let out_h = out_phys.h;
        if out_w <= 0 || out_h <= 0 {
            return None;
        }
        let s = (blur_strength / 2.0).clamp(0.0, 1.0);
        // Smooth opacity ramp over the first 15% of the slider so 0% -> 100% never pops abruptly.
        let blend_alpha = (s * 6.0).clamp(0.0, 1.0);
        let strength_q = (s * 200.0).round() as u32;

        // Fast zero-cost path during 60 FPS open/close/dropdown animations (immediately bypassed
        // when the user drags the unified System Blur slider).
        if self.cached_out_size == Some(out_phys)
            && self.cached_heavy == Some(heavy)
            && self.cached_strength_q == Some(strength_q)
            && self
                .cached_at
                .is_some_and(|t| t.elapsed() < cache_ttl)
            && let Some((_, ref pong_tex)) = self.pong
        {
            return Some((pong_tex.clone(), out_phys, self.commit, blend_alpha));
        }

        let full_size: Size<i32, BufferCoord> = (out_w, out_h).into();
        let (small_w, small_h, step1, step2_opt) = if heavy {
            // Fullscreen Power Menu: quarter-res with 2 smooth H+V iterations scaled by `s`.
            let s1 = 0.28 + 0.95 * s;
            let s2 = (s > 0.20).then_some(0.24 + 1.05 * s);
            ((out_w / 4).max(64), (out_h / 4).max(64), s1, s2)
        } else {
            // Top Bar, Dock, Quick Settings, Workspaces Sidebar: half-res crisp-to-lush Gaussian
            // blur scaled continuously from light frost (`s=0.1`) to deep glass (`s=1.0`).
            let s1 = 0.25 + 1.55 * s;
            let s2 = (s > 0.30).then_some(0.25 + 1.40 * ((s - 0.30) / 0.70));
            ((out_w / 2).max(128), (out_h / 2).max(128), s1, s2)
        };
        let small_size: Size<i32, BufferCoord> = (small_w, small_h).into();

        Self::ensure_tex(&mut self.full, renderer, full_size).ok()?;
        Self::ensure_tex(&mut self.ping, renderer, small_size).ok()?;
        Self::ensure_tex(&mut self.pong, renderer, small_size).ok()?;

        // 1. Render live scene below the target layer into `full` using `Transform::Normal`.
        //
        // CRITICAL: Always use `Transform::Normal` (NOT `OutputDamageTracker::from_output(output)`):
        // Smithay's `GlesRenderer::render(..., Transform::Normal)` maps screen top (`y = 0`) to
        // FBO row `v = 0.0` and screen bottom (`y = H`) to `v = 1.0`, matching `GlesTexture`
        // (`y_inverted = false`) identically across bare-metal DRM (`Transform::Normal`),
        // nested Winit (`Transform::Flipped180`), and rotated outputs!
        {
            let (_, full_tex) = self.full.as_mut()?;
            let mut tracker = OutputDamageTracker::new(
                out_phys,
                output.current_scale().fractional_scale(),
                Transform::Normal,
            );
            let mut target = renderer.bind(full_tex).ok()?;
            tracker
                .render_output(renderer, &mut target, 0, scene_below, FALLBACK_CLEAR_COLOR)
                .ok()?;
        }

        let full_tex = self.full.as_ref()?.1.clone();
        let small_phys: Size<i32, Physical> = (small_w, small_h).into();
        let small_dst = Rectangle::from_size(small_phys);
        let full_src = Rectangle::from_size(Size::<f64, BufferCoord>::from((
            out_w as f64,
            out_h as f64,
        )));
        let small_src = Rectangle::from_size(Size::<f64, BufferCoord>::from((
            small_w as f64,
            small_h as f64,
        )));

        let full_bounds = [
            0.6 / out_w as f32,
            0.6 / out_h as f32,
            1.0 - 0.6 / out_w as f32,
            1.0 - 0.6 / out_h as f32,
        ];
        let small_bounds = [
            0.6 / small_w as f32,
            0.6 / small_h as f32,
            1.0 - 0.6 / small_w as f32,
            1.0 - 0.6 / small_h as f32,
        ];

        // 2. Pass 1 (H-blur + downsample): full -> ping (upright `Transform::Normal`)
        {
            let (_, ping_tex) = self.ping.as_mut()?;
            let mut target = renderer.bind(ping_tex).ok()?;
            let mut frame = renderer
                .render(&mut target, small_phys, Transform::Normal)
                .ok()?;
            frame.clear(Color32F::TRANSPARENT, &[small_dst]).ok()?;
            frame
                .render_texture_from_to(
                    &full_tex,
                    full_src,
                    small_dst,
                    &[small_dst],
                    &[],
                    Transform::Normal,
                    1.0,
                    Some(&shaders.blur_pass),
                    &[
                        Uniform::new("u_step", [step1 / small_w as f32, 0.0]),
                        Uniform::new("u_uv_bounds", full_bounds),
                    ],
                )
                .ok()?;
            let _ = frame.finish().ok()?;
        }

        // 3. Pass 2 (V-blur): ping -> pong
        let ping_tex_ro = self.ping.as_ref()?.1.clone();
        {
            let (_, pong_tex) = self.pong.as_mut()?;
            let mut target = renderer.bind(pong_tex).ok()?;
            let mut frame = renderer
                .render(&mut target, small_phys, Transform::Normal)
                .ok()?;
            frame.clear(Color32F::TRANSPARENT, &[small_dst]).ok()?;
            frame
                .render_texture_from_to(
                    &ping_tex_ro,
                    small_src,
                    small_dst,
                    &[small_dst],
                    &[],
                    Transform::Normal,
                    1.0,
                    Some(&shaders.blur_pass),
                    &[
                        Uniform::new("u_step", [0.0, step1 / small_h as f32]),
                        Uniform::new("u_uv_bounds", small_bounds),
                    ],
                )
                .ok()?;
            let _ = frame.finish().ok()?;
        }

        // 4. Optional 2nd H+V refinement iteration at medium-to-high System Blur so high-contrast
        //    text/icons dissolve into silky bokeh with zero tap-stride grid ringing.
        if let Some(step2) = step2_opt {
            let pong_tex_ro = self.pong.as_ref()?.1.clone();
            {
                let (_, ping_tex) = self.ping.as_mut()?;
                let mut target = renderer.bind(ping_tex).ok()?;
                let mut frame = renderer
                    .render(&mut target, small_phys, Transform::Normal)
                    .ok()?;
                frame.clear(Color32F::TRANSPARENT, &[small_dst]).ok()?;
                frame
                    .render_texture_from_to(
                        &pong_tex_ro,
                        small_src,
                        small_dst,
                        &[small_dst],
                        &[],
                        Transform::Normal,
                        1.0,
                        Some(&shaders.blur_pass),
                        &[
                            Uniform::new("u_step", [step2 / small_w as f32, 0.0]),
                            Uniform::new("u_uv_bounds", small_bounds),
                        ],
                    )
                    .ok()?;
                let _ = frame.finish().ok()?;
            }
            let ping_tex_ro2 = self.ping.as_ref()?.1.clone();
            {
                let (_, pong_tex) = self.pong.as_mut()?;
                let mut target = renderer.bind(pong_tex).ok()?;
                let mut frame = renderer
                    .render(&mut target, small_phys, Transform::Normal)
                    .ok()?;
                frame.clear(Color32F::TRANSPARENT, &[small_dst]).ok()?;
                frame
                    .render_texture_from_to(
                        &ping_tex_ro2,
                        small_src,
                        small_dst,
                        &[small_dst],
                        &[],
                        Transform::Normal,
                        1.0,
                        Some(&shaders.blur_pass),
                        &[
                            Uniform::new("u_step", [0.0, step2 / small_h as f32]),
                            Uniform::new("u_uv_bounds", small_bounds),
                        ],
                    )
                    .ok()?;
                let _ = frame.finish().ok()?;
            }
        }

        self.cached_at = Some(Instant::now());
        self.cached_out_size = Some(out_phys);
        self.cached_heavy = Some(heavy);
        self.cached_strength_q = Some(strength_q);
        self.commit.increment();
        Some((self.pong.as_ref()?.1.clone(), out_phys, self.commit, blend_alpha))
    }
}

/// Renders a blurred region of the live desktop behind an overlay layer surface with optional
/// anti-aliased rounded corners and animated opacity.
#[derive(Debug)]
pub struct BlurredBackdropElement {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    src_rect: Rectangle<f64, BufferCoord>,
    dst_rect: Rectangle<i32, Physical>,
    full_rect: Rectangle<i32, Physical>,
    corner_radius_phys: f32,
    rounded_program: Option<GlesTexProgram>,
    alpha: f32,
}

impl BlurredBackdropElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        output_phys: Size<i32, Physical>,
        dst_rect: Rectangle<i32, Physical>,
        corner_radius_phys: f32,
        rounded_program: Option<GlesTexProgram>,
        alpha: f32,
    ) -> Option<Self> {
        let vis_x0 = dst_rect.loc.x.max(0);
        let vis_y0 = dst_rect.loc.y.max(0);
        let vis_x1 = (dst_rect.loc.x + dst_rect.size.w).min(output_phys.w);
        let vis_y1 = (dst_rect.loc.y + dst_rect.size.h).min(output_phys.h);
        if vis_x1 <= vis_x0 || vis_y1 <= vis_y0 {
            return None;
        }
        let visible_dst = Rectangle::new(
            (vis_x0, vis_y0).into(),
            (vis_x1 - vis_x0, vis_y1 - vis_y0).into(),
        );

        let tex_sz = texture.size();
        let sx = tex_sz.w as f64 / output_phys.w.max(1) as f64;
        let sy = tex_sz.h as f64 / output_phys.h.max(1) as f64;

        // Inset sampling bounds by half a texel so `GL_LINEAR` on default `GL_REPEAT` offscreen
        // textures never wraps the bottom screen row into the top edge (or vice versa).
        let max_u = (tex_sz.w as f64 - 0.5).max(0.5);
        let max_v = (tex_sz.h as f64 - 0.5).max(0.5);
        let x0 = (visible_dst.loc.x as f64 * sx).clamp(0.5, max_u);
        let y0 = (visible_dst.loc.y as f64 * sy).clamp(0.5, max_v);
        let x1 = ((visible_dst.loc.x + visible_dst.size.w) as f64 * sx).clamp(0.5, max_u);
        let y1 = ((visible_dst.loc.y + visible_dst.size.h) as f64 * sy).clamp(0.5, max_v);

        let src_rect = Rectangle::new(
            (x0, y0).into(),
            ((x1 - x0).max(0.5), (y1 - y0).max(0.5)).into(),
        );

        Some(Self {
            id,
            commit,
            texture,
            src_rect,
            dst_rect: visible_dst,
            full_rect: dst_rect,
            corner_radius_phys: corner_radius_phys.max(0.0),
            rounded_program,
            alpha: alpha.clamp(0.0, 1.0),
        })
    }
}

impl Element for BlurredBackdropElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.dst_rect.loc
    }

    fn src(&self) -> Rectangle<f64, BufferCoord> {
        self.src_rect
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst_rect
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.dst_rect.size)])
        }
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.corner_radius_phys <= 0.5 && self.alpha >= 0.999 {
            OpaqueRegions::from_slice(&[Rectangle::from_size(self.dst_rect.size)])
        } else {
            OpaqueRegions::default()
        }
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for BlurredBackdropElement {
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if self.corner_radius_phys > 0.5
            && let Some(ref prog) = self.rounded_program
            && let Some((uv0, inv_mat)) = compute_uv_to_dst(
                src,
                dst.size,
                self.texture.size(),
                Transform::Normal,
                self.texture.is_y_inverted(),
            )
        {
            let dst_offset = [
                (dst.loc.x - self.full_rect.loc.x) as f32,
                (dst.loc.y - self.full_rect.loc.y) as f32,
            ];
            let win_size = [
                self.full_rect.size.w.max(1) as f32,
                self.full_rect.size.h.max(1) as f32,
            ];
            let uniforms = [
                Uniform::new("u_uv0", uv0),
                Uniform::new("u_inv_mat", inv_mat),
                Uniform::new("u_dst_offset", dst_offset),
                Uniform::new("u_win_size", win_size),
                Uniform::new("u_radii", [self.corner_radius_phys, self.corner_radius_phys]),
            ];
            return frame.render_texture_from_to(
                &self.texture,
                src,
                dst,
                damage,
                &[],
                Transform::Normal,
                self.alpha,
                Some(prog),
                &uniforms,
            );
        }

        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            &[],
            Transform::Normal,
            self.alpha,
            None,
            &[],
        )
    }
}

/// Screen-space Liquid Glass region descriptor consumed by `NeonRenderer` to composite
/// real-time refractive Apple Liquid Glass behind `neon-dynamic-island`.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicIslandGlassRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub top_radius: f32,
    pub bottom_radius: f32,
    pub ear_radius: f32,
    pub alpha: f32,
    pub tint_rgb: [f32; 3],
    pub tint_strength: f32,
}

/// Renders an Apple Liquid Glass Dynamic Island capsule or floating Dock Switcher pill over the
/// blurred desktop scene texture with Snell rim refraction, R/G/B chromatic dispersion, adaptive
/// luminance glazing, 3D specular edge bevels, and a soft ambient Mac elevation shadow.
#[derive(Debug)]
pub struct LiquidGlassElement {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    src_rect: Rectangle<f64, BufferCoord>,
    dst_rect: Rectangle<i32, Physical>,
    shape_rect: Rectangle<i32, Physical>,
    top_radius_phys: f32,
    bottom_radius_phys: f32,
    scale_factor: f32,
    tint_rgba: [f32; 4],
    program: Option<GlesTexProgram>,
    alpha: f32,
}

impl LiquidGlassElement {
    /// Creates a `LiquidGlassElement` for one glass pill (`shape_rect` in physical output coords).
    /// Adds outer padding for a soft Mac ambient elevation shadow while sampling `texture` strictly
    /// within its half-texel safe UV bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        output_phys: Size<i32, Physical>,
        shape_rect: Rectangle<i32, Physical>,
        top_radius_phys: f32,
        bottom_radius_phys: f32,
        scale_factor: f32,
        tint_rgba: [f32; 4],
        program: Option<GlesTexProgram>,
        alpha: f32,
    ) -> Option<Self> {
        if shape_rect.size.w <= 0
            || shape_rect.size.h <= 0
            || output_phys.w <= 0
            || output_phys.h <= 0
            || alpha <= 0.001
        {
            return None;
        }
        let shadow_pad = (9.0 * scale_factor.max(1.0)).ceil() as i32;
        let full_rect = Rectangle::new(
            Point::from((shape_rect.loc.x - shadow_pad, shape_rect.loc.y - shadow_pad)),
            Size::from((
                shape_rect.size.w + shadow_pad * 2,
                shape_rect.size.h + shadow_pad * 2,
            )),
        );
        let screen_rect = Rectangle::from_size(output_phys);
        let visible_dst = full_rect.intersection(screen_rect)?;
        if visible_dst.size.w <= 0 || visible_dst.size.h <= 0 {
            return None;
        }

        let tex_size = texture.size();
        if tex_size.w <= 0 || tex_size.h <= 0 {
            return None;
        }
        let scale_x = tex_size.w as f64 / output_phys.w as f64;
        let scale_y = tex_size.h as f64 / output_phys.h as f64;
        let max_u = (tex_size.w as f64 - 0.5).max(0.5);
        let max_v = (tex_size.h as f64 - 0.5).max(0.5);
        let x0 = (visible_dst.loc.x as f64 * scale_x).clamp(0.5, max_u);
        let y0 = (visible_dst.loc.y as f64 * scale_y).clamp(0.5, max_v);
        let x1 = ((visible_dst.loc.x + visible_dst.size.w) as f64 * scale_x)
            .clamp(x0 + 0.01, max_u + 0.01);
        let y1 = ((visible_dst.loc.y + visible_dst.size.h) as f64 * scale_y)
            .clamp(y0 + 0.01, max_v + 0.01);
        let src_rect = Rectangle::new(Point::from((x0, y0)), Size::from((x1 - x0, y1 - y0)));

        Some(Self {
            id,
            commit,
            texture,
            src_rect,
            dst_rect: visible_dst,
            shape_rect,
            top_radius_phys,
            bottom_radius_phys,
            scale_factor: scale_factor.max(1.0),
            tint_rgba,
            program,
            alpha,
        })
    }
}

impl Element for LiquidGlassElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
        self.dst_rect.loc
    }

    fn src(&self) -> Rectangle<f64, BufferCoord> {
        self.src_rect
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst_rect
    }

    fn damage_since(
        &self,
        _scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        if commit == Some(self.commit) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.dst_rect.size)])
        }
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for LiquidGlassElement {
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }

    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoord>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let tex_size = self.texture.size();
        if let Some(ref prog) = self.program
            && let Some((uv0, inv_mat, fwd_mat)) = crate::rounded::compute_uv_transform(
                src,
                dst.size,
                tex_size,
                Transform::Normal,
                self.texture.is_y_inverted(),
            )
            && tex_size.w > 0
            && tex_size.h > 0
        {
            let tw = tex_size.w as f32;
            let th = tex_size.h as f32;
            let uv_bounds = [0.5 / tw, 0.5 / th, (tw - 0.5) / tw, (th - 0.5) / th];
            let dst_offset = [
                (dst.loc.x - self.shape_rect.loc.x) as f32,
                (dst.loc.y - self.shape_rect.loc.y) as f32,
            ];
            let win_size = [
                self.shape_rect.size.w.max(1) as f32,
                self.shape_rect.size.h.max(1) as f32,
            ];
            let uniforms = [
                Uniform::new("u_uv0", uv0),
                Uniform::new("u_inv_mat", inv_mat),
                Uniform::new("u_fwd_mat", fwd_mat),
                Uniform::new("u_uv_bounds", uv_bounds),
                Uniform::new("u_dst_offset", dst_offset),
                Uniform::new("u_win_size", win_size),
                Uniform::new("u_radii", [self.top_radius_phys, self.bottom_radius_phys]),
                Uniform::new("u_scale", self.scale_factor),
                Uniform::new("u_opacity", 1.0_f32),
                Uniform::new("u_tint", self.tint_rgba),
            ];
            return frame.render_texture_from_to(
                &self.texture,
                src,
                dst,
                damage,
                &[],
                Transform::Normal,
                self.alpha,
                Some(prog),
                &uniforms,
            );
        }

        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            &[],
            Transform::Normal,
            self.alpha,
            None,
            &[],
        )
    }
}
