//! Modular GPU rendering pipeline for **NEON DE** built on Smithay's OpenGL ES (`GlesRenderer`).
//!
//! Translates [`neon_scene::SceneGraph`] into damage-tracked Smithay render elements with
//! cached custom GLSL elements (analytical soft shadows, rounded accent focus rings, and
//! NEON DE ambient backdrop) so idle frames generate zero spurious damage.

pub mod shaders;

use std::collections::{HashMap, HashSet};

use neon_scene::{BorderStyle, NodeContent, SceneGraph, ShadowStyle};
use neon_window_manager::WindowId;
use smithay::{
    backend::renderer::{
        Color32F,
        damage::{Error as OutputDamageTrackerError, OutputDamageTracker, RenderOutputResult},
        element::{
            AsRenderElements, Kind,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::WaylandSurfaceRenderElement,
        },
        gles::{GlesError, GlesRenderer, GlesTarget, Uniform, element::PixelShaderElement},
    },
    desktop::{Window, layer_map_for_output},
    output::Output,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::shell::wlr_layer::Layer as WlrLayer,
};
use tracing::{info, warn};

use crate::shaders::NeonShaders;

/// Fallback clear color (`#0C0F17`) used if shaders are unavailable.
pub const FALLBACK_CLEAR_COLOR: Color32F = Color32F::new(0.047, 0.059, 0.090, 1.0);

render_elements! {
    /// Unified render element type composited by [`NeonRenderer`].
    pub NeonRenderElement<=GlesRenderer>;
    Wayland=WaylandSurfaceRenderElement<GlesRenderer>,
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

/// Stateful per-output GPU renderer with compiled custom shaders and cached damage-aware elements.
#[derive(Debug)]
pub struct NeonRenderer {
    shaders: Option<NeonShaders>,
    damage_tracker: OutputDamageTracker,
    shadow_cache: HashMap<WindowId, CachedShadow>,
    border_cache: HashMap<WindowId, CachedBorder>,
    backdrop_cache: Option<CachedBackdrop>,
}

impl NeonRenderer {
    /// Creates a [`NeonRenderer`] bound to `output`, compiling NEON DE's custom GLES shaders.
    ///
    /// If shader compilation fails on an unsupported driver, degrades gracefully to solid
    /// composition without crashing the compositor.
    pub fn new(renderer: &mut GlesRenderer, output: &Output) -> Self {
        let shaders = match NeonShaders::compile(renderer) {
            Ok(compiled) => {
                info!("Compiled NEON DE custom GLES shaders (shadow, border, backdrop)");
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
            backdrop_cache: None,
        }
    }

    pub fn has_shaders(&self) -> bool {
        self.shaders.is_some()
    }

    /// Renders the retained [`SceneGraph`] into `framebuffer` with full `OutputDamageTracker` support.
    pub fn render_scene<'a, F>(
        &'a mut self,
        output: &Output,
        renderer: &mut GlesRenderer,
        framebuffer: &mut GlesTarget<'_>,
        age: usize,
        scene: &SceneGraph,
        mut resolve_window: F,
    ) -> Result<RenderOutputResult<'a>, OutputDamageTrackerError<GlesError>>
    where
        F: FnMut(WindowId) -> Option<&'a Window>,
    {
        let scale_f = output.current_scale().fractional_scale();
        let scale = Scale::from(scale_f);
        let nodes = scene.collect_front_to_back();

        let mut elements: Vec<NeonRenderElement> = Vec::with_capacity(nodes.len() * 3 + 8);
        let mut seen_windows: HashSet<WindowId> = HashSet::new();

        // Collect zwlr_layer_shell_v1 surfaces on this output split into upper (Overlay/Top)
        // and lower (Bottom/Background) strata so they composite in exact protocol Z-order.
        let (upper_layers, lower_layers) = {
            let layer_map = layer_map_for_output(output);
            let mut upper = Vec::new();
            let mut lower = Vec::new();
            for wlr_layer in [WlrLayer::Overlay, WlrLayer::Top] {
                for layer in layer_map.layers_on(wlr_layer).rev() {
                    if let Some(geo) = layer_map.layer_geometry(layer) {
                        upper.push((layer.clone(), geo.loc));
                    }
                }
            }
            for wlr_layer in [WlrLayer::Bottom, WlrLayer::Background] {
                for layer in layer_map.layers_on(wlr_layer).rev() {
                    if let Some(geo) = layer_map.layer_geometry(layer) {
                        lower.push((layer.clone(), geo.loc));
                    }
                }
            }
            (upper, lower)
        };

        // Front-most: Overlay and Top layer surfaces (Overview, Top Panel, Dock).
        for (layer, loc) in &upper_layers {
            let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(scale_f);
            let layer_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(layer, renderer, phys_loc, scale, 1.0);
            elements.extend(layer_elements.into_iter().map(NeonRenderElement::from));
        }

        let mut inserted_lower_layers = false;
        for node in nodes {
            if !inserted_lower_layers && matches!(node.content, NodeContent::Backdrop { .. }) {
                // Insert Bottom and Background layer surfaces just above the compositor backdrop.
                for (layer, loc) in &lower_layers {
                    let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(scale_f);
                    let layer_elements =
                        AsRenderElements::<GlesRenderer>::render_elements::<
                            WaylandSurfaceRenderElement<GlesRenderer>,
                        >(layer, renderer, phys_loc, scale, 1.0);
                    elements.extend(layer_elements.into_iter().map(NeonRenderElement::from));
                }
                inserted_lower_layers = true;
            }

            let eff = node.effective_bounds();
            let eff_rect: Rectangle<i32, Logical> = Rectangle::new(
                (eff.loc.x, eff.loc.y).into(),
                (eff.size.w, eff.size.h).into(),
            );

            match &node.content {
                NodeContent::Window { window_id, .. } => {
                    seen_windows.insert(*window_id);

                    // 1. Client Wayland surface tree (top-most within this window subtree).
                    if let Some(window) = resolve_window(*window_id) {
                        let win_geo = window.geometry();
                        let render_loc: Point<i32, Logical> =
                            (eff.loc.x - win_geo.loc.x, eff.loc.y - win_geo.loc.y).into();
                        let phys_loc: Point<i32, Physical> =
                            render_loc.to_physical_precise_round(scale_f);

                        let surface_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                            WaylandSurfaceRenderElement<GlesRenderer>,
                        >(
                            window, renderer, phys_loc, scale, node.opacity
                        );
                        elements.extend(surface_elements.into_iter().map(NeonRenderElement::from));
                    }

                    // 2. Rounded focus ring / subtle rim border behind the client surface.
                    if let Some(border) = node.border
                        && eff_rect.size.w > 0
                        && eff_rect.size.h > 0
                        && let Some(elem) =
                            self.get_or_update_border(*window_id, eff_rect, border, node.opacity)
                    {
                        elements.push(NeonRenderElement::from(elem));
                    }

                    // 3. Analytical soft drop shadow behind the border.
                    if let Some(shadow) = node.shadow
                        && eff_rect.size.w > 0
                        && eff_rect.size.h > 0
                        && let Some(elem) =
                            self.get_or_update_shadow(*window_id, eff_rect, shadow, node.opacity)
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
                        elements.push(NeonRenderElement::from(elem));
                    }
                }
            }
        }

        // Prune cached shader elements for windows that are no longer visible or were closed.
        self.shadow_cache.retain(|id, _| seen_windows.contains(id));
        self.border_cache.retain(|id, _| seen_windows.contains(id));

        self.damage_tracker.render_output(
            renderer,
            framebuffer,
            age,
            &elements,
            FALLBACK_CLEAR_COLOR,
        )
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
