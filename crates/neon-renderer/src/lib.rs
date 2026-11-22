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
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
        },
        gles::{GlesError, GlesRenderer, GlesTarget, Uniform, element::PixelShaderElement},
    },
    desktop::{Window, layer_map_for_output},
    output::Output,
    render_elements,
    utils::{Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::session_lock::LockSurface,
    wayland::shell::wlr_layer::Layer as WlrLayer,
};
use tracing::{info, warn};

use crate::shaders::NeonShaders;

/// Fallback clear color (`#0C0F17`) used if shaders are unavailable.
pub const FALLBACK_CLEAR_COLOR: Color32F = Color32F::new(0.047, 0.059, 0.090, 1.0);

/// Opaque security curtain color (`#05070D`) used while `ext_session_lock_v1` is locked.
pub const LOCK_CURTAIN_COLOR: Color32F = Color32F::new(0.020, 0.027, 0.051, 1.0);

/// Live blur parameters supplied to [`NeonRenderer::render_scene`].
#[derive(Debug, Clone, Copy)]
pub struct BlurSettings {
    pub enabled: bool,
    pub strength: f32,
    pub corner_radius: f32,
}

impl Default for BlurSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 1.0,
            corner_radius: 12.0,
        }
    }
}

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

/// Stateful per-output GPU renderer with compiled custom shaders and cached damage-aware elements.
#[derive(Debug)]
pub struct NeonRenderer {
    shaders: Option<NeonShaders>,
    damage_tracker: OutputDamageTracker,
    shadow_cache: HashMap<WindowId, CachedShadow>,
    border_cache: HashMap<WindowId, CachedBorder>,
    glass_cache: HashMap<WindowId, CachedGlass>,
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
                info!(
                    "Compiled NEON DE custom GLES shaders (shadow, border, backdrop, frosted-glass)"
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
            backdrop_cache: None,
        }
    }

    pub fn has_shaders(&self) -> bool {
        self.shaders.is_some()
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

        self.damage_tracker
            .render_output(renderer, framebuffer, age, &elements, LOCK_CURTAIN_COLOR)
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
                NodeContent::Window {
                    window_id,
                    fullscreen,
                    ..
                } => {
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

                    // 2. Frosted-glass blur plate behind translucent non-fullscreen windows.
                    if blur.enabled
                        && !fullscreen
                        && eff_rect.size.w > 0
                        && eff_rect.size.h > 0
                        && let Some(elem) = self.get_or_update_glass(
                            *window_id,
                            eff_rect,
                            output_logical_size,
                            blur,
                            node.opacity,
                        )
                    {
                        elements.push(NeonRenderElement::from(elem));
                    }

                    // 3. Rounded focus ring / subtle rim border behind the client surface.
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
        self.glass_cache.retain(|id, _| seen_windows.contains(id));

        self.damage_tracker.render_output(
            renderer,
            framebuffer,
            age,
            &elements,
            FALLBACK_CLEAR_COLOR,
        )
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
