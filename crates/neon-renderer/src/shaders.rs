//! Custom OpenGL ES 2.0 (`#version 100`) shaders for **NEON DE**.
//!
//! All shaders output premultiplied RGBA (`vec4(rgb * a, a)`) to match Smithay's
//! `GL_ONE, GL_ONE_MINUS_SRC_ALPHA` blend pipeline.

use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, UniformName, UniformType,
};

/// Analytical SDF soft drop-shadow shader for floating windows.
pub const SHADOW_FRAGMENT_SHADER: &str = r#"
precision mediump float;

varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

uniform vec2 u_rect_size;
uniform float u_corner_radius;
uniform float u_spread;
uniform vec4 u_color;
uniform float u_opacity;

float rounded_box_sdf(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + vec2(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - radius;
}

void main() {
    vec2 pos = v_coords * size;
    vec2 center = size * 0.5;
    vec2 half_rect = u_rect_size * 0.5;
    float r = min(u_corner_radius, min(half_rect.x, half_rect.y));

    float dist = rounded_box_sdf(pos - center, half_rect, r);
    // Smooth Gaussian-like falloff outside the window box; suppressed cleanly at the edge.
    float sigma = max(u_spread * 0.45, 1.0);
    float falloff = exp(-(max(dist, 0.0) * max(dist, 0.0)) / (2.0 * sigma * sigma));
    // Fade smoothly to zero before reaching the outer quad boundary to prevent hard clipping.
    float edge_fade = 1.0 - smoothstep(u_spread * 0.72, u_spread, max(dist, 0.0));
    float a = u_color.a * falloff * edge_fade * u_opacity * alpha;

    gl_FragColor = vec4(u_color.rgb * a, a);
}
"#;

/// Anti-aliased SDF rounded border / NEON accent focus ring shader.
pub const BORDER_FRAGMENT_SHADER: &str = r#"
precision mediump float;

varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

uniform float u_corner_radius;
uniform float u_border_width;
uniform vec4 u_color;
uniform float u_opacity;

float rounded_box_sdf(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + vec2(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - radius;
}

void main() {
    vec2 pos = v_coords * size;
    vec2 half_size = size * 0.5;
    float r = min(u_corner_radius, min(half_size.x, half_size.y));

    float outer = rounded_box_sdf(pos - half_size, half_size, r);
    float inner = rounded_box_sdf(pos - half_size, max(half_size - vec2(u_border_width), vec2(1.0)), max(r - u_border_width, 0.0));

    float outer_mask = 1.0 - smoothstep(-0.75, 0.75, outer);
    float inner_mask = smoothstep(-0.75, 0.75, inner);
    float ring = clamp(outer_mask * inner_mask, 0.0, 1.0);
    float a = u_color.a * ring * u_opacity * alpha;

    gl_FragColor = vec4(u_color.rgb * a, a);
}
"#;

/// Subtle NEON DE ambient radial-gradient obsidian desktop backdrop shader.
pub const BACKDROP_FRAGMENT_SHADER: &str = r#"
precision mediump float;

varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

uniform vec4 u_primary;
uniform vec4 u_accent;

void main() {
    // Aspect-corrected radial ambient glow anchored gently toward upper-left/center.
    vec2 uv = v_coords;
    float aspect = size.x / max(size.y, 1.0);
    vec2 delta = (uv - vec2(0.32, 0.28)) * vec2(aspect, 1.0);
    float glow = exp(-dot(delta, delta) * 1.45);

    vec2 delta2 = (uv - vec2(0.78, 0.76)) * vec2(aspect, 1.0);
    float glow2 = exp(-dot(delta2, delta2) * 1.8) * 0.45;

    float mix_factor = clamp(glow * 0.65 + glow2, 0.0, 1.0);
    vec3 rgb = mix(u_primary.rgb, u_accent.rgb, mix_factor);
    gl_FragColor = vec4(rgb * alpha, alpha);
}
"#;

/// Subtle frosted-glass translucent backdrop shader rendered behind floating windows
/// when GPU blur/translucency is enabled (analytical 9-tap ambient diffusion + glass highlight).
pub const FROSTED_GLASS_FRAGMENT_SHADER: &str = r#"
precision mediump float;

varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

uniform vec2 u_output_size;
uniform vec2 u_window_origin;
uniform float u_corner_radius;
uniform float u_blur_strength;
uniform float u_opacity;
uniform vec4 u_primary;
uniform vec4 u_accent;

float rounded_box_sdf(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + vec2(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - radius;
}

vec3 sample_ambient(vec2 uv, float aspect) {
    vec2 d1 = (uv - vec2(0.32, 0.28)) * vec2(aspect, 1.0);
    float g1 = exp(-dot(d1, d1) * 1.45);
    vec2 d2 = (uv - vec2(0.78, 0.76)) * vec2(aspect, 1.0);
    float g2 = exp(-dot(d2, d2) * 1.8) * 0.45;
    return mix(u_primary.rgb, u_accent.rgb, clamp(g1 * 0.65 + g2, 0.0, 1.0));
}

void main() {
    vec2 pos = v_coords * size;
    vec2 half_size = size * 0.5;
    float r = min(u_corner_radius, min(half_size.x, half_size.y));
    float dist = rounded_box_sdf(pos - half_size, half_size, r);
    float mask = 1.0 - smoothstep(-0.75, 0.75, dist);

    vec2 out_sz = max(u_output_size, vec2(1.0));
    float aspect = out_sz.x / out_sz.y;
    vec2 global_uv = (u_window_origin + pos) / out_sz;
    vec2 step_uv = (vec2(14.0) * clamp(u_blur_strength, 0.2, 2.0)) / out_sz;

    // 9-tap radial Gaussian diffusion over the ambient desktop field
    vec3 blurred = sample_ambient(global_uv, aspect) * 0.28;
    blurred += sample_ambient(global_uv + vec2( step_uv.x, 0.0), aspect) * 0.11;
    blurred += sample_ambient(global_uv + vec2(-step_uv.x, 0.0), aspect) * 0.11;
    blurred += sample_ambient(global_uv + vec2(0.0,  step_uv.y), aspect) * 0.11;
    blurred += sample_ambient(global_uv + vec2(0.0, -step_uv.y), aspect) * 0.11;
    blurred += sample_ambient(global_uv + vec2( step_uv.x,  step_uv.y) * 0.7, aspect) * 0.07;
    blurred += sample_ambient(global_uv + vec2(-step_uv.x,  step_uv.y) * 0.7, aspect) * 0.07;
    blurred += sample_ambient(global_uv + vec2( step_uv.x, -step_uv.y) * 0.7, aspect) * 0.07;
    blurred += sample_ambient(global_uv + vec2(-step_uv.x, -step_uv.y) * 0.7, aspect) * 0.07;

    // Subtle frosted sheen
    vec3 glass = mix(blurred, vec3(0.14, 0.20, 0.30), 0.16);
    float a = mask * 0.78 * u_opacity * alpha;
    gl_FragColor = vec4(glass * a, a);
}
"#;

/// Compiled NEON DE custom shader suite cached for the lifetime of a `GlesRenderer`.
#[derive(Debug, Clone)]
pub struct NeonShaders {
    pub shadow: GlesPixelProgram,
    pub border: GlesPixelProgram,
    pub backdrop: GlesPixelProgram,
    pub frosted_glass: GlesPixelProgram,
}

impl NeonShaders {
    /// Compiles and links all NEON DE GLSL shaders against the active `GlesRenderer` context.
    pub fn compile(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        let shadow = renderer.compile_custom_pixel_shader(
            SHADOW_FRAGMENT_SHADER,
            &[
                UniformName::new("u_rect_size", UniformType::_2f),
                UniformName::new("u_corner_radius", UniformType::_1f),
                UniformName::new("u_spread", UniformType::_1f),
                UniformName::new("u_color", UniformType::_4f),
                UniformName::new("u_opacity", UniformType::_1f),
            ],
        )?;

        let border = renderer.compile_custom_pixel_shader(
            BORDER_FRAGMENT_SHADER,
            &[
                UniformName::new("u_corner_radius", UniformType::_1f),
                UniformName::new("u_border_width", UniformType::_1f),
                UniformName::new("u_color", UniformType::_4f),
                UniformName::new("u_opacity", UniformType::_1f),
            ],
        )?;

        let backdrop = renderer.compile_custom_pixel_shader(
            BACKDROP_FRAGMENT_SHADER,
            &[
                UniformName::new("u_primary", UniformType::_4f),
                UniformName::new("u_accent", UniformType::_4f),
            ],
        )?;

        let frosted_glass = renderer.compile_custom_pixel_shader(
            FROSTED_GLASS_FRAGMENT_SHADER,
            &[
                UniformName::new("u_output_size", UniformType::_2f),
                UniformName::new("u_window_origin", UniformType::_2f),
                UniformName::new("u_corner_radius", UniformType::_1f),
                UniformName::new("u_blur_strength", UniformType::_1f),
                UniformName::new("u_opacity", UniformType::_1f),
                UniformName::new("u_primary", UniformType::_4f),
                UniformName::new("u_accent", UniformType::_4f),
            ],
        )?;

        Ok(Self {
            shadow,
            border,
            backdrop,
            frosted_glass,
        })
    }
}
