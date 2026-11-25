//! GPU rounded-corner surface wrapper for Smithay's `GlesRenderer`.
//!
//! Wraps a [`WaylandSurfaceRenderElement<GlesRenderer>`] and renders its texture with
//! [`crate::shaders::ROUNDED_WINDOW_FRAGMENT_SHADER`], computing the exact analytical inverse
//! of Smithay's `tex_matrix` so rounded corners align pixel-for-pixel across DRM, winit,
//! offscreen screencopy, HiDPI/fractional scaling, `y_inverted` DMA-BUF surfaces, subsurfaces,
//! viewporter crops, and window animations.

use smithay::{
    backend::renderer::{
        Texture,
        element::{
            Element, Id, Kind, RenderElement, UnderlyingStorage,
            surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture},
        },
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{Buffer as BufferCoord, Physical, Point, Rectangle, Scale, Size, Transform},
};

/// A [`WaylandSurfaceRenderElement`] clipped on the GPU to an anti-aliased rounded window frame.
#[derive(Debug)]
pub struct RoundedSurfaceElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    program: GlesTexProgram,
    /// Outer visible window frame in physical output coordinates (matching `window.geometry()`).
    win_rect: Rectangle<i32, Physical>,
    /// `(top_radius_phys, bottom_radius_phys)` in physical pixels.
    radii: [f32; 2],
    /// Optional ephemeral `Id` used on the single frame a window's corner radius changes
    /// so `OutputDamageTracker` always repaints the updated corner curves.
    override_id: Option<Id>,
}

impl RoundedSurfaceElement {
    pub fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        program: GlesTexProgram,
        win_rect: Rectangle<i32, Physical>,
        top_radius_phys: f32,
        bottom_radius_phys: f32,
        radius_changed: bool,
    ) -> Self {
        Self {
            inner,
            program,
            win_rect,
            radii: [top_radius_phys.max(0.0), bottom_radius_phys.max(0.0)],
            override_id: radius_changed.then(Id::new),
        }
    }
}

/// Evaluates Smithay's `GlesFrame::render_texture_from_to` texture coordinate transform at
/// destination-local pixel `(lx, ly)` (`0..dst.w`, `0..dst.h`).
fn eval_smithay_uv(
    lx: f64,
    ly: f64,
    src: Rectangle<f64, BufferCoord>,
    dst_size: Size<i32, Physical>,
    tex_size: Size<i32, BufferCoord>,
    transform: Transform,
    y_inverted: bool,
) -> (f64, f64) {
    let dst_src_size = transform.transform_size(src.size);
    let sx = dst_src_size.w / dst_size.w as f64;
    let sy = dst_src_size.h / dst_size.h as f64;
    let x1 = lx * sx;
    let y1 = ly * sy;

    // Matches `Transform::matrix()` + `translation` in Smithay's `build_texture_mat`.
    let (x2, y2) = match transform {
        Transform::Normal => (x1, y1),
        Transform::_90 => (y1, dst_src_size.w - x1),
        Transform::_180 => (dst_src_size.w - x1, dst_src_size.h - y1),
        Transform::_270 => (dst_src_size.h - y1, x1),
        Transform::Flipped => (dst_src_size.w - x1, y1),
        Transform::Flipped90 => (y1, x1),
        Transform::Flipped180 => (x1, dst_src_size.h - y1),
        Transform::Flipped270 => (dst_src_size.h - y1, dst_src_size.w - x1),
    };

    let u = (x2 + src.loc.x) / tex_size.w as f64;
    let mut v = (y2 + src.loc.y) / tex_size.h as f64;
    if y_inverted {
        v = -v;
    }
    (u, v)
}

/// Computes `(u_uv0, u_inv_mat)` such that for every fragment with interpolated `v_coords`:
/// `dst_local = vec2(dot(u_inv_mat.xy, v_coords - u_uv0), dot(u_inv_mat.zw, v_coords - u_uv0))`.
pub(crate) fn compute_uv_to_dst(
    src: Rectangle<f64, BufferCoord>,
    dst_size: Size<i32, Physical>,
    tex_size: Size<i32, BufferCoord>,
    transform: Transform,
    y_inverted: bool,
) -> Option<([f32; 2], [f32; 4])> {
    if src.size.w <= 0.0
        || src.size.h <= 0.0
        || dst_size.w <= 0
        || dst_size.h <= 0
        || tex_size.w <= 0
        || tex_size.h <= 0
    {
        return None;
    }

    let dw = dst_size.w as f64;
    let dh = dst_size.h as f64;
    let uv0 = eval_smithay_uv(0.0, 0.0, src, dst_size, tex_size, transform, y_inverted);
    let uv_x = eval_smithay_uv(dw, 0.0, src, dst_size, tex_size, transform, y_inverted);
    let uv_y = eval_smithay_uv(0.0, dh, src, dst_size, tex_size, transform, y_inverted);

    let dux = uv_x.0 - uv0.0;
    let duy = uv_x.1 - uv0.1;
    let dvx = uv_y.0 - uv0.0;
    let dvy = uv_y.1 - uv0.1;

    let det = dux * dvy - duy * dvx;
    if det.abs() < 1e-12 {
        return None;
    }

    let inv_det = 1.0 / det;
    let m00 = (dvy * inv_det * dw) as f32;
    let m01 = (-dvx * inv_det * dw) as f32;
    let m10 = (-duy * inv_det * dh) as f32;
    let m11 = (dux * inv_det * dh) as f32;

    Some(([uv0.0 as f32, uv0.1 as f32], [m00, m01, m10, m11]))
}

impl Element for RoundedSurfaceElement {
    fn id(&self) -> &Id {
        self.override_id.as_ref().unwrap_or_else(|| self.inner.id())
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
        if self.inner.alpha() < 0.999 {
            return OpaqueRegions::default();
        }
        // Trim top and bottom rounded-corner strips (and any client-side margin outside `win_rect`)
        // from reported opaque regions so `OutputDamageTracker` always composites the desktop
        // background and window shadow behind the anti-aliased rounded corners.
        let top_pad = self.radii[0].ceil() as i32 + 2;
        let bot_pad = self.radii[1].ceil() as i32 + 2;
        let safe_h = self.win_rect.size.h - top_pad - bot_pad;
        let safe_w = self.win_rect.size.w - 2;
        if safe_h <= 0 || safe_w <= 0 {
            return OpaqueRegions::default();
        }

        let elem_loc = self.inner.location(scale);
        let safe_rect = Rectangle::new(
            (
                self.win_rect.loc.x - elem_loc.x + 1,
                self.win_rect.loc.y - elem_loc.y + top_pad,
            )
                .into(),
            (safe_w, safe_h).into(),
        );

        self.inner
            .opaque_regions(scale)
            .iter()
            .filter_map(|r| r.intersection(safe_rect))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for RoundedSurfaceElement {
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Disable direct DRM scanout bypass for rounded floating windows so the GPU rounded-corner
        // shader always executes (fullscreen windows do not use `RoundedSurfaceElement`).
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
        match self.inner.texture() {
            WaylandSurfaceTexture::Texture(tex) => {
                let tex_size = tex.size();
                if let Some((uv0, inv_mat)) = compute_uv_to_dst(
                    src,
                    dst.size,
                    tex_size,
                    self.inner.transform(),
                    tex.is_y_inverted(),
                ) {
                    let dst_offset = [
                        (dst.loc.x - self.win_rect.loc.x) as f32,
                        (dst.loc.y - self.win_rect.loc.y) as f32,
                    ];
                    let win_size = [
                        self.win_rect.size.w.max(1) as f32,
                        self.win_rect.size.h.max(1) as f32,
                    ];
                    let uniforms = [
                        Uniform::new("u_uv0", uv0),
                        Uniform::new("u_inv_mat", inv_mat),
                        Uniform::new("u_dst_offset", dst_offset),
                        Uniform::new("u_win_size", win_size),
                        Uniform::new("u_radii", self.radii),
                    ];
                    // Pass empty `opaque_regions` to `render_texture_from_to` so GL blending stays
                    // active on rounded corners even if caller supplies conservative rects.
                    frame.render_texture_from_to(
                        tex,
                        src,
                        dst,
                        damage,
                        &[],
                        self.inner.transform(),
                        self.inner.alpha(),
                        Some(&self.program),
                        &uniforms,
                    )
                } else {
                    self.inner.draw(frame, src, dst, damage, opaque_regions)
                }
            }
            WaylandSurfaceTexture::SolidColor(_) => {
                self.inner.draw(frame, src, dst, damage, opaque_regions)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify_roundtrip(
        src: Rectangle<f64, BufferCoord>,
        dst_size: Size<i32, Physical>,
        tex_size: Size<i32, BufferCoord>,
        transform: Transform,
        y_inverted: bool,
    ) {
        let (uv0, inv) = compute_uv_to_dst(src, dst_size, tex_size, transform, y_inverted)
            .expect("invertible uv transform");
        for &(lx, ly) in &[
            (0.0, 0.0),
            (dst_size.w as f64, 0.0),
            (0.0, dst_size.h as f64),
            (dst_size.w as f64, dst_size.h as f64),
            (dst_size.w as f64 * 0.37, dst_size.h as f64 * 0.63),
        ] {
            let (u, v) = eval_smithay_uv(lx, ly, src, dst_size, tex_size, transform, y_inverted);
            let du = u as f32 - uv0[0];
            let dv = v as f32 - uv0[1];
            let rx = inv[0] * du + inv[1] * dv;
            let ry = inv[2] * du + inv[3] * dv;
            assert!(
                (rx - lx as f32).abs() < 1e-2 && (ry - ly as f32).abs() < 1e-2,
                "roundtrip mismatch for {transform:?} y_inv={y_inverted}: expected ({lx}, {ly}), got ({rx}, {ry})"
            );
        }
    }

    #[test]
    fn uv_inverse_roundtrips_all_transforms_and_crops() {
        let tex_size: Size<i32, BufferCoord> = (1600, 1200).into();
        let src: Rectangle<f64, BufferCoord> = Rectangle::new((40.0, 20.0).into(), (1200.0, 900.0).into());
        let dst_size: Size<i32, Physical> = (800, 600).into();

        for transform in [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ] {
            verify_roundtrip(src, dst_size, tex_size, transform, false);
            verify_roundtrip(src, dst_size, tex_size, transform, true);
        }
    }
}
