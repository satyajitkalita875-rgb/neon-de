//! GPU-captured closing-window ghost animations for client-initiated window closes.
//!
//! When a Wayland or XWayland application closes itself directly (e.g. clicking its own
//! CSD `×` button or pressing `Ctrl+Q`/`Ctrl+W`), the surface is destroyed immediately
//! without a compositor pre-close grace window. Capturing a one-shot offscreen `GlesTexture`
//! snapshot of the departing window lets `NeonRenderer` play a smooth 60 FPS shrink/fade
//! close animation toward the Dock even after the client has disconnected.

use std::time::{Duration, Instant};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame, Offscreen, Renderer, Texture,
            damage::OutputDamageTracker,
            element::{
                AsRenderElements, Element, Id, Kind, RenderElement, UnderlyingStorage,
                surface::WaylandSurfaceRenderElement,
            },
            gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
            utils::{CommitCounter, DamageSet, OpaqueRegions},
        },
    },
    desktop::Window,
    output::Output,
    utils::{Buffer as BufferCoord, Logical, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::rounded::compute_uv_to_dst;

/// Pending client-initiated window close queued by `NeonState` for GPU texture capture on the
/// next render frame before the window's texture is released.
#[derive(Debug, Clone)]
pub struct PendingCloseGhost {
    pub window: Window,
    pub rect: Rectangle<i32, Logical>,
    pub corner_radius: f32,
    pub target_offset: (f64, f64),
    pub duration: Duration,
}

/// Active GPU-captured closing window animation owned by [`crate::NeonRenderer`].
#[derive(Debug)]
pub(crate) struct ClosingGhost {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    base_rect_phys: Rectangle<i32, Physical>,
    corner_radius_phys: f32,
    target_offset_phys: (f64, f64),
    started: Instant,
    duration: Duration,
}

impl ClosingGhost {
    /// Captures a one-shot offscreen snapshot of `pending.window` (falling back to cropping
    /// `fallback_desktop` if the client already detached its buffer before closing).
    pub(crate) fn capture(
        renderer: &mut GlesRenderer,
        output: &Output,
        pending: &PendingCloseGhost,
        fallback_desktop: Option<(GlesTexture, Size<i32, Physical>)>,
    ) -> Option<Self> {
        if pending.duration.is_zero() || pending.rect.size.w <= 12 || pending.rect.size.h <= 12 {
            return None;
        }
        let scale_f = output.current_scale().fractional_scale();
        let output_origin = output.current_location();
        let local_rect = Rectangle::new(
            (
                pending.rect.loc.x - output_origin.x,
                pending.rect.loc.y - output_origin.y,
            )
                .into(),
            pending.rect.size,
        );
        let base_rect_phys: Rectangle<i32, Physical> = local_rect.to_physical_precise_round(scale_f);
        let out_phys = output
            .current_mode()
            .map(|m| output.current_transform().transform_size(m.size))
            .unwrap_or_else(|| Size::from((4096, 4096)));
        let out_rect = Rectangle::from_size(out_phys);
        if base_rect_phys.size.w <= 12
            || base_rect_phys.size.h <= 12
            || base_rect_phys.intersection(out_rect).is_none()
        {
            return None;
        }

        let tex_w = base_rect_phys.size.w.clamp(16, 2560);
        let tex_h = base_rect_phys.size.h.clamp(16, 1600);
        let tex_size: Size<i32, BufferCoord> = (tex_w, tex_h).into();
        let tex_phys: Size<i32, Physical> = (tex_w, tex_h).into();
        let mut tex = renderer.create_buffer(Fourcc::Abgr8888, tex_size).ok()?;

        // Primary path: render the closing window's live surface tree directly into `tex`.
        let win_geo = pending.window.geometry();
        let render_loc: Point<i32, Logical> = (-win_geo.loc.x, -win_geo.loc.y).into();
        let phys_loc: Point<i32, Physical> = render_loc.to_physical_precise_round(scale_f);
        let surface_elems = AsRenderElements::<GlesRenderer>::render_elements::<
            WaylandSurfaceRenderElement<GlesRenderer>,
        >(&pending.window, renderer, phys_loc, Scale::from(scale_f), 1.0);

        let captured = if !surface_elems.is_empty() {
            let mut tracker = OutputDamageTracker::new(tex_phys, scale_f, Transform::Normal);
            if let Ok(mut target) = renderer.bind(&mut tex) {
                tracker
                    .render_output(
                        renderer,
                        &mut target,
                        0,
                        &surface_elems,
                        Color32F::TRANSPARENT,
                    )
                    .is_ok()
            } else {
                false
            }
        } else if let Some((full_tex, full_phys)) = fallback_desktop {
            // Fallback path: client detached its buffer before destroy; crop its exact rect from
            // the last upright full-desktop offscreen snapshot captured right before this frame.
            let sx = full_tex.size().w as f64 / full_phys.w.max(1) as f64;
            let sy = full_tex.size().h as f64 / full_phys.h.max(1) as f64;
            let x0 = (base_rect_phys.loc.x as f64 * sx).clamp(0.0, full_tex.size().w as f64);
            let y0 = (base_rect_phys.loc.y as f64 * sy).clamp(0.0, full_tex.size().h as f64);
            let x1 = ((base_rect_phys.loc.x + base_rect_phys.size.w) as f64 * sx)
                .clamp(0.0, full_tex.size().w as f64);
            let y1 = ((base_rect_phys.loc.y + base_rect_phys.size.h) as f64 * sy)
                .clamp(0.0, full_tex.size().h as f64);
            let cw = x1 - x0;
            let ch = y1 - y0;
            if cw >= 8.0 && ch >= 8.0 {
                let src = Rectangle::new((x0, y0).into(), (cw, ch).into());
                let dst = Rectangle::from_size(tex_phys);
                if let Ok(mut target) = renderer.bind(&mut tex)
                    && let Ok(mut frame) = renderer.render(&mut target, tex_phys, Transform::Normal)
                {
                    let _ = frame.clear(Color32F::TRANSPARENT, &[dst]);
                    let ok = frame
                        .render_texture_from_to(
                            &full_tex,
                            src,
                            dst,
                            &[dst],
                            &[],
                            Transform::Normal,
                            1.0,
                            None,
                            &[],
                        )
                        .is_ok();
                    let _ = frame.finish();
                    ok
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if !captured {
            return None;
        }

        Some(Self {
            id: Id::new(),
            commit: CommitCounter::default(),
            texture: tex,
            base_rect_phys,
            corner_radius_phys: (pending.corner_radius as f64 * scale_f) as f32,
            target_offset_phys: (
                pending.target_offset.0 * scale_f,
                pending.target_offset.1 * scale_f,
            ),
            started: Instant::now(),
            duration: pending.duration,
        })
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.duration.is_zero() || self.started.elapsed() >= self.duration
    }

    pub(crate) fn build_element(
        &mut self,
        rounded_program: Option<GlesTexProgram>,
    ) -> Option<ClosingGhostElement> {
        let elapsed = self.started.elapsed();
        if self.duration.is_zero() || elapsed >= self.duration {
            return None;
        }
        let t = (elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0);
        // Android 16 Material 3 Expressive return-to-icon collapse:
        // brisk smooth deceleration toward the Dock icon + container-transform corner rounding +
        // delayed opacity dissolve so the closing window visibly zips back into its Dock icon.
        let ease = 1.0 - (1.0 - t).powi(3);
        let scale = 1.0 - 0.64 * ease;
        let alpha_t = ((t - 0.24) / 0.76).clamp(0.0, 1.0);
        let alpha = (1.0 - alpha_t * alpha_t * (3.0 - 2.0 * alpha_t)).clamp(0.0, 1.0) as f32;
        if alpha <= 0.005 {
            return None;
        }

        let ox = self.target_offset_phys.0 * ease;
        let oy = self.target_offset_phys.1 * ease;
        let cx = self.base_rect_phys.loc.x as f64 + self.base_rect_phys.size.w as f64 * 0.5 + ox;
        let cy = self.base_rect_phys.loc.y as f64 + self.base_rect_phys.size.h as f64 * 0.5 + oy;
        let w = (self.base_rect_phys.size.w as f64 * scale).round().max(2.0) as i32;
        let h = (self.base_rect_phys.size.h as f64 * scale).round().max(2.0) as i32;
        let dst_rect = Rectangle::new(
            (
                (cx - w as f64 * 0.5).round() as i32,
                (cy - h as f64 * 0.5).round() as i32,
            )
                .into(),
            (w, h).into(),
        );

        // Morph corners smoothly toward a squircle/pill as the app returns to its Dock icon.
        let max_r = (w.min(h) as f64) * 0.42;
        let morphed_radius =
            (self.corner_radius_phys as f64 * (1.0 + 2.4 * ease)).min(max_r) as f32;

        let tex_sz = self.texture.size();
        let src_rect = Rectangle::from_size(Size::<f64, BufferCoord>::from((
            tex_sz.w as f64,
            tex_sz.h as f64,
        )));

        self.commit.increment();
        Some(ClosingGhostElement {
            id: self.id.clone(),
            commit: self.commit,
            texture: self.texture.clone(),
            src_rect,
            dst_rect,
            corner_radius_phys: morphed_radius,
            rounded_program,
            alpha,
        })
    }
}

/// Render element drawing a captured closing-window texture with smooth scale, fade, and
/// anti-aliased rounded corners.
#[derive(Debug)]
pub struct ClosingGhostElement {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    src_rect: Rectangle<f64, BufferCoord>,
    dst_rect: Rectangle<i32, Physical>,
    corner_radius_phys: f32,
    rounded_program: Option<GlesTexProgram>,
    alpha: f32,
}

impl Element for ClosingGhostElement {
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

impl RenderElement<GlesRenderer> for ClosingGhostElement {
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
                (dst.loc.x - self.dst_rect.loc.x) as f32,
                (dst.loc.y - self.dst_rect.loc.y) as f32,
            ];
            let win_size = [
                self.dst_rect.size.w.max(1) as f32,
                self.dst_rect.size.h.max(1) as f32,
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
