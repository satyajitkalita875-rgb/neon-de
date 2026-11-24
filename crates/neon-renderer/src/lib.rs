//! Modular GPU rendering pipeline for **NEON DE** built on Smithay's OpenGL ES (`GlesRenderer`).
//!
//! Translates [`neon_scene::SceneGraph`] into damage-tracked Smithay render elements with
//! cached custom GLSL elements (analytical soft shadows, rounded accent focus rings, and
//! NEON DE ambient backdrop) so idle frames generate zero spurious damage.

pub mod cursor;
pub mod screenshot;
pub mod shaders;

use std::collections::{HashMap, HashSet};

use neon_scene::{BorderStyle, NodeContent, SceneGraph, ShadowStyle, TitlebarStyle};
use neon_window_manager::WindowId;
use smithay::{
    backend::renderer::{
        Color32F,
        damage::{Error as OutputDamageTrackerError, OutputDamageTracker, RenderOutputResult},
        element::{
            AsRenderElements, Kind,
            memory::MemoryRenderBufferRenderElement,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::RescaleRenderElement,
        },
        gles::{
            GlesError, GlesRenderer, GlesTarget, GlesTexture, Uniform, element::PixelShaderElement,
        },
    },
    desktop::{Window, layer_map_for_output},
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    output::Output,
    render_elements,
    utils::{Buffer as BufferCoord, Logical, Physical, Point, Rectangle, Scale, Size},
    wayland::{
        compositor::with_states, session_lock::LockSurface, shell::wlr_layer::Layer as WlrLayer,
    },
};
use tracing::{info, warn};

use crate::{cursor::CursorCache, shaders::NeonShaders};

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
    RescaledWayland=RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
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

/// Stateful per-output GPU renderer with compiled custom shaders and cached damage-aware elements.
#[derive(Debug)]
pub struct NeonRenderer {
    shaders: Option<NeonShaders>,
    damage_tracker: OutputDamageTracker,
    shadow_cache: HashMap<WindowId, CachedShadow>,
    border_cache: HashMap<WindowId, CachedBorder>,
    glass_cache: HashMap<WindowId, CachedGlass>,
    titlebar_cache: HashMap<WindowId, CachedTitlebar>,
    backdrop_cache: Option<CachedBackdrop>,
    cursor_cache: CursorCache,
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
            titlebar_cache: HashMap::new(),
            backdrop_cache: None,
            cursor_cache: CursorCache::load_default(),
            screencopy_offscreen: None,
        }
    }

    pub fn sync_cursor_config(&mut self, theme: &str, size: u32) {
        self.cursor_cache.set_theme_and_size(theme, size);
    }

    pub fn has_shaders(&self) -> bool {
        self.shaders.is_some()
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
        let scale_f = output.current_scale().fractional_scale();
        let scale = Scale::from(scale_f);
        let nodes = scene.collect_front_to_back();

        let mut elements: Vec<NeonRenderElement> =
            Vec::with_capacity(nodes.len() * 3 + override_redirect.len() * 2 + 8);
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

        // Front-most: Overlay and Top layer surfaces (Context Menu, Overview, Top Panel, Dock).
        for (layer, loc) in &upper_layers {
            let phys_loc: Point<i32, Physical> = loc.to_physical_precise_round(scale_f);
            let layer_elements = AsRenderElements::<GlesRenderer>::render_elements::<
                WaylandSurfaceRenderElement<GlesRenderer>,
            >(layer, renderer, phys_loc, scale, 1.0);
            elements.extend(layer_elements.into_iter().map(NeonRenderElement::from));
        }

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

                    if let Some(window) = resolve_window(*window_id) {
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

                        let surface_elements = AsRenderElements::<GlesRenderer>::render_elements::<
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
                            elements.extend(surface_elements.into_iter().map(NeonRenderElement::from));
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
                        && !fullscreen
                        && frame_rect.size.w > 0
                        && frame_rect.size.h > 0
                        && let Some(elem) = self.get_or_update_glass(
                            *window_id,
                            frame_rect,
                            output_logical_size,
                            blur,
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
                        elements.push(NeonRenderElement::from(elem));
                    }
                }
            }
        }

        // Prune cached shader elements for windows that are no longer visible or were closed.
        self.shadow_cache.retain(|id, _| seen_windows.contains(id));
        self.border_cache.retain(|id, _| seen_windows.contains(id));
        self.glass_cache.retain(|id, _| seen_windows.contains(id));
        self.titlebar_cache.retain(|id, _| seen_windows.contains(id));

        elements
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
