//! Custom OpenGL ES 2.0 (`#version 100`) shaders for **NEON DE**.
//!
//! All shaders output premultiplied RGBA (`vec4(rgb * a, a)`) to match Smithay's
//! `GL_ONE, GL_ONE_MINUS_SRC_ALPHA` blend pipeline.

use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
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

/// Authentic macOS Sequoia server-side window titlebar shader with crisp SDF traffic-light
/// Close (#FF5F57), Minimize (#FEBC2E), and Maximize (#28C840) buttons + hover glyphs (×, −, +).
pub const MACOS_TITLEBAR_FRAGMENT_SHADER: &str = r#"
precision mediump float;

varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

uniform float u_corner_radius;
uniform float u_focused;
uniform float u_light_theme;
uniform float u_hover_controls;
uniform float u_opacity;

float rounded_top_sdf(vec2 pos, vec2 sz, float r) {
    float rad = min(r, min(sz.x * 0.5, sz.y));
    // Only round the top-left and top-right corners; bottom joins flush with client surface
    vec2 p = pos - vec2(sz.x * 0.5, sz.y * 0.5);
    vec2 half_sz = sz * 0.5;
    float corner_r = (pos.y < rad) ? rad : 0.0;
    vec2 q = abs(p) - half_sz + vec2(corner_r);
    return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - corner_r;
}

float sd_segment(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a;
    vec2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-5), 0.0, 1.0);
    return length(pa - ba * h);
}

void main() {
    vec2 pos = v_coords * size;
    float box_d = rounded_top_sdf(pos, size, u_corner_radius);
    float mask = 1.0 - smoothstep(-0.75, 0.75, box_d);
    if (mask <= 0.001) {
        gl_FragColor = vec4(0.0);
        return;
    }

    // macOS dark vs light titlebar surface gradient
    float t = clamp(pos.y / max(size.y, 1.0), 0.0, 1.0);
    vec3 dark_top = mix(vec3(0.145, 0.149, 0.173), vec3(0.180, 0.184, 0.212), u_focused);
    vec3 dark_bot = mix(vec3(0.125, 0.129, 0.153), vec3(0.155, 0.159, 0.186), u_focused);
    vec3 light_top = mix(vec3(0.910, 0.914, 0.925), vec3(0.955, 0.958, 0.968), u_focused);
    vec3 light_bot = mix(vec3(0.875, 0.879, 0.892), vec3(0.922, 0.926, 0.938), u_focused);

    vec3 bg = mix(mix(dark_top, dark_bot, t), mix(light_top, light_bot, t), u_light_theme);

    // Subtle 1px bottom hairline divider
    if (pos.y > size.y - 1.1) {
        vec3 sep = mix(vec3(1.0, 1.0, 1.0), vec3(0.0, 0.0, 0.0), u_light_theme);
        bg = mix(bg, sep, 0.10);
    }

    // macOS traffic-light buttons at top-left: Close (16, 14), Minimize (36, 14), Maximize (56, 14)
    float cy = size.y * 0.5;
    float btn_r = 6.0;
    vec2 c_close = vec2(16.0, cy);
    vec2 c_min   = vec2(36.0, cy);
    vec2 c_max   = vec2(56.0, cy);

    float show_color = max(u_focused, u_hover_controls);
    vec3 inactive_dot = mix(vec3(0.36, 0.37, 0.41), vec3(0.76, 0.77, 0.79), u_light_theme);
    vec3 col_close = mix(inactive_dot, vec3(1.000, 0.373, 0.341), show_color); // #FF5F57
    vec3 col_min   = mix(inactive_dot, vec3(0.996, 0.737, 0.180), show_color); // #FEBC2E
    vec3 col_max   = mix(inactive_dot, vec3(0.157, 0.784, 0.251), show_color); // #28C840

    vec3 rgb = bg;

    // 1. Close button (red) + × glyph on hover
    float d_close = length(pos - c_close) - btn_r;
    float m_close = 1.0 - smoothstep(-0.65, 0.65, d_close);
    if (m_close > 0.001) {
        vec3 bcol = col_close;
        float rim = smoothstep(-1.2, -0.2, d_close);
        bcol *= (1.0 - rim * 0.16);
        if (u_hover_controls > 0.5) {
            vec2 lp = pos - c_close;
            float g1 = sd_segment(lp, vec2(-2.6, -2.6), vec2(2.6, 2.6)) - 0.65;
            float g2 = sd_segment(lp, vec2(-2.6,  2.6), vec2(2.6, -2.6)) - 0.65;
            float gm = 1.0 - smoothstep(-0.45, 0.45, min(g1, g2));
            bcol = mix(bcol, vec3(0.30, 0.03, 0.03), gm * 0.85);
        }
        rgb = mix(rgb, bcol, m_close);
    }

    // 2. Minimize button (yellow) + − glyph on hover
    float d_min = length(pos - c_min) - btn_r;
    float m_min = 1.0 - smoothstep(-0.65, 0.65, d_min);
    if (m_min > 0.001) {
        vec3 bcol = col_min;
        float rim = smoothstep(-1.2, -0.2, d_min);
        bcol *= (1.0 - rim * 0.16);
        if (u_hover_controls > 0.5) {
            vec2 lp = pos - c_min;
            float g = sd_segment(lp, vec2(-3.0, 0.0), vec2(3.0, 0.0)) - 0.68;
            float gm = 1.0 - smoothstep(-0.45, 0.45, g);
            bcol = mix(bcol, vec3(0.38, 0.22, 0.01), gm * 0.85);
        }
        rgb = mix(rgb, bcol, m_min);
    }

    // 3. Maximize button (green) + + glyph on hover
    float d_max = length(pos - c_max) - btn_r;
    float m_max = 1.0 - smoothstep(-0.65, 0.65, d_max);
    if (m_max > 0.001) {
        vec3 bcol = col_max;
        float rim = smoothstep(-1.2, -0.2, d_max);
        bcol *= (1.0 - rim * 0.16);
        if (u_hover_controls > 0.5) {
            vec2 lp = pos - c_max;
            float g1 = sd_segment(lp, vec2(-2.9, 0.0), vec2(2.9, 0.0)) - 0.65;
            float g2 = sd_segment(lp, vec2(0.0, -2.9), vec2(0.0, 2.9)) - 0.65;
            float gm = 1.0 - smoothstep(-0.45, 0.45, min(g1, g2));
            bcol = mix(bcol, vec3(0.03, 0.28, 0.06), gm * 0.85);
        }
        rgb = mix(rgb, bcol, m_max);
    }

    float a = mask * u_opacity * alpha;
    gl_FragColor = vec4(rgb * a, a);
}
"#;

/// Custom GLES texture shader that clips floating client window surfaces cleanly to
/// anti-aliased rounded corners (supporting `NO_ALPHA`, `EXTERNAL` DMA-BUF, `DEBUG_FLAGS`,
/// arbitrary buffer transforms, viewporter crops, subsurfaces, and CSD shadow margins).
pub const ROUNDED_WINDOW_FRAGMENT_SHADER: &str = r#"
#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 u_uv0;
uniform vec4 u_inv_mat;
uniform vec2 u_dst_offset;
uniform vec2 u_win_size;
uniform vec2 u_radii;

void main() {
    vec4 color = texture2D(tex, v_coords);

#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif

    vec2 d_uv = v_coords - u_uv0;
    vec2 win_pos = u_dst_offset + vec2(dot(u_inv_mat.xy, d_uv), dot(u_inv_mat.zw, d_uv));

    vec2 half_size = u_win_size * 0.5;
    float r = (win_pos.y < half_size.y) ? u_radii.x : u_radii.y;
    r = min(r, min(half_size.x, half_size.y));

    vec2 rel = abs(win_pos - half_size);
    float mask = 1.0;

    if (rel.x > half_size.x || rel.y > half_size.y) {
        float d_box = length(max(rel - half_size, vec2(0.0)));
        mask = 1.0 - smoothstep(0.0, 1.0, d_box);
    }

    if (r > 0.5) {
        vec2 corner_vec = rel - (half_size - vec2(r));
        if (corner_vec.x > 0.0 && corner_vec.y > 0.0) {
            float d_corner = length(corner_vec) - r;
            mask = min(mask, 1.0 - smoothstep(-0.75, 0.75, d_corner));
        }
    }

    color *= mask;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// 9-tap hardware-bilinear separable Gaussian blur texture shader used for real-time frosted
/// backdrops behind Quick Settings and the fullscreen Power Menu.
pub const BLUR_PASS_FRAGMENT_SHADER: &str = r#"
#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

uniform vec2 u_step;
uniform vec4 u_uv_bounds;

void main() {
    vec2 lo = u_uv_bounds.xy;
    vec2 hi = u_uv_bounds.zw;
    vec4 sum = texture2D(tex, clamp(v_coords, lo, hi)) * 0.16;
    sum += texture2D(tex, clamp(v_coords + u_step * 1.4, lo, hi)) * 0.15;
    sum += texture2D(tex, clamp(v_coords - u_step * 1.4, lo, hi)) * 0.15;
    sum += texture2D(tex, clamp(v_coords + u_step * 3.2, lo, hi)) * 0.12;
    sum += texture2D(tex, clamp(v_coords - u_step * 3.2, lo, hi)) * 0.12;
    sum += texture2D(tex, clamp(v_coords + u_step * 5.1, lo, hi)) * 0.09;
    sum += texture2D(tex, clamp(v_coords - u_step * 5.1, lo, hi)) * 0.09;
    sum += texture2D(tex, clamp(v_coords + u_step * 7.0, lo, hi)) * 0.06;
    sum += texture2D(tex, clamp(v_coords - u_step * 7.0, lo, hi)) * 0.06;

#if defined(NO_ALPHA)
    vec4 color = vec4(sum.rgb, 1.0) * alpha;
#else
    vec4 color = sum * alpha;
#endif

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
"#;

/// Compiled NEON DE custom shader suite cached for the lifetime of a `GlesRenderer`.
#[derive(Debug, Clone)]
pub struct NeonShaders {
    pub shadow: GlesPixelProgram,
    pub border: GlesPixelProgram,
    pub backdrop: GlesPixelProgram,
    pub frosted_glass: GlesPixelProgram,
    pub titlebar: GlesPixelProgram,
    pub rounded_window: GlesTexProgram,
    pub blur_pass: GlesTexProgram,
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

        let titlebar = renderer.compile_custom_pixel_shader(
            MACOS_TITLEBAR_FRAGMENT_SHADER,
            &[
                UniformName::new("u_corner_radius", UniformType::_1f),
                UniformName::new("u_focused", UniformType::_1f),
                UniformName::new("u_light_theme", UniformType::_1f),
                UniformName::new("u_hover_controls", UniformType::_1f),
                UniformName::new("u_opacity", UniformType::_1f),
            ],
        )?;

        let rounded_window = renderer.compile_custom_texture_shader(
            ROUNDED_WINDOW_FRAGMENT_SHADER,
            &[
                UniformName::new("u_uv0", UniformType::_2f),
                UniformName::new("u_inv_mat", UniformType::_4f),
                UniformName::new("u_dst_offset", UniformType::_2f),
                UniformName::new("u_win_size", UniformType::_2f),
                UniformName::new("u_radii", UniformType::_2f),
            ],
        )?;

        let blur_pass = renderer.compile_custom_texture_shader(
            BLUR_PASS_FRAGMENT_SHADER,
            &[
                UniformName::new("u_step", UniformType::_2f),
                UniformName::new("u_uv_bounds", UniformType::_4f),
            ],
        )?;

        Ok(Self {
            shadow,
            border,
            backdrop,
            frosted_glass,
            titlebar,
            rounded_window,
            blur_pass,
        })
    }
}
