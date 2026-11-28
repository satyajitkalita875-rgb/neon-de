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
uniform float u_buttons_right;
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

    // Traffic-light controls placed on left (macOS: Close, Min, Max) or right (Standard: Min, Max, Close).
    float cy = size.y * 0.5;
    float btn_r = 6.0;
    vec2 c_close = vec2(mix(16.0, size.x - 16.0, u_buttons_right), cy);
    vec2 c_min   = vec2(mix(36.0, size.x - 56.0, u_buttons_right), cy);
    vec2 c_max   = vec2(mix(56.0, size.x - 36.0, u_buttons_right), cy);

    float show_color = max(u_focused, u_hover_controls);
    vec3 inactive_dot = mix(vec3(0.36, 0.37, 0.41), vec3(0.76, 0.77, 0.79), u_light_theme);
    vec3 col_close = mix(inactive_dot, vec3(1.000, 0.373, 0.341), show_color); // #FF5F57
    vec3 col_min   = mix(inactive_dot, vec3(0.996, 0.737, 0.180), show_color); // #FEBC2E
    vec3 col_max   = mix(inactive_dot, vec3(0.157, 0.784, 0.251), show_color); // #28C840

    vec3 rgb = bg;

    // 1. Close button (red) + × glyph on hover
    float d_close = length(pos - c_close) - btn_r;
    float m_close = 1.0 - smoothstep(-0.75, 0.75, d_close);
    if (m_close > 0.001) {
        vec3 bcol = col_close;
        float rim = smoothstep(-1.2, -0.2, d_close);
        bcol *= (1.0 - rim * 0.16);
        if (u_hover_controls > 0.5) {
            vec2 lp = pos - c_close;
            float g1 = sd_segment(lp, vec2(-2.6, -2.6), vec2(2.6, 2.6)) - 0.65;
            float g2 = sd_segment(lp, vec2(-2.6,  2.6), vec2(2.6, -2.6)) - 0.65;
            float gm = 1.0 - smoothstep(-0.65, 0.65, min(g1, g2));
            bcol = mix(bcol, vec3(0.30, 0.03, 0.03), gm * 0.85);
        }
        rgb = mix(rgb, bcol, m_close);
    }

    // 2. Minimize button (yellow) + − glyph on hover
    float d_min = length(pos - c_min) - btn_r;
    float m_min = 1.0 - smoothstep(-0.75, 0.75, d_min);
    if (m_min > 0.001) {
        vec3 bcol = col_min;
        float rim = smoothstep(-1.2, -0.2, d_min);
        bcol *= (1.0 - rim * 0.16);
        if (u_hover_controls > 0.5) {
            vec2 lp = pos - c_min;
            float g = sd_segment(lp, vec2(-3.0, 0.0), vec2(3.0, 0.0)) - 0.68;
            float gm = 1.0 - smoothstep(-0.65, 0.65, g);
            bcol = mix(bcol, vec3(0.38, 0.22, 0.01), gm * 0.85);
        }
        rgb = mix(rgb, bcol, m_min);
    }

    // 3. Maximize button (green) + + glyph on hover
    float d_max = length(pos - c_max) - btn_r;
    float m_max = 1.0 - smoothstep(-0.75, 0.75, d_max);
    if (m_max > 0.001) {
        vec3 bcol = col_max;
        float rim = smoothstep(-1.2, -0.2, d_max);
        bcol *= (1.0 - rim * 0.16);
        if (u_hover_controls > 0.5) {
            vec2 lp = pos - c_max;
            float g1 = sd_segment(lp, vec2(-2.9, 0.0), vec2(2.9, 0.0)) - 0.65;
            float g2 = sd_segment(lp, vec2(0.0, -2.9), vec2(0.0, 2.9)) - 0.65;
            float gm = 1.0 - smoothstep(-0.65, 0.65, min(g1, g2));
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

/// Anti-aliased SDF rounded translucent glass card + crisp rim border shader
/// (used for Windows 11 snap preview highlights, Snap Layouts drop bar, and tiling swap previews).
pub const ROUNDED_RECT_FRAGMENT_SHADER: &str = r#"
precision mediump float;

varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;

uniform float u_corner_radius;
uniform float u_border_width;
uniform vec4 u_fill_color;
uniform vec4 u_border_color;
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
    float outer_mask = 1.0 - smoothstep(-0.75, 0.75, outer);
    if (outer_mask <= 0.001) {
        gl_FragColor = vec4(0.0);
        return;
    }

    float bw = max(u_border_width, 0.0);
    float inner_mask = 1.0;
    if (bw > 0.01) {
        float inner = rounded_box_sdf(
            pos - half_size,
            max(half_size - vec2(bw), vec2(0.5)),
            max(r - bw, 0.0)
        );
        inner_mask = 1.0 - smoothstep(-0.75, 0.75, inner);
    }
    float border_ring = clamp(outer_mask - inner_mask, 0.0, 1.0);

    // Subtle vertical glass sheen inside fill
    float sheen = mix(0.06, 0.0, clamp(v_coords.y, 0.0, 1.0));
    vec3 fill_rgb = clamp(u_fill_color.rgb + vec3(sheen), 0.0, 1.0);
    float fill_a = u_fill_color.a * inner_mask;
    float border_a = u_border_color.a * border_ring;

    // Premultiplied compositing of border over fill
    vec3 premul = fill_rgb * fill_a * (1.0 - border_a) + u_border_color.rgb * border_a;
    float out_a = (fill_a * (1.0 - border_a) + border_a) * u_opacity * alpha;
    gl_FragColor = vec4(premul * u_opacity * alpha, out_a);
}
"#;

/// Apple Liquid Glass shader for floating Mac Dynamic Island capsules & Dock Switcher pills.
/// Combines blurred backdrop Snell-edge refraction, R/G/B chromatic rim dispersion, adaptive
/// luminance-aware frosted clarity + vibrancy boost, sculpted 3D overhead/caustic specular
/// bevels, and a soft ambient Mac elevation shadow around the outer capsule contour.
pub const LIQUID_GLASS_FRAGMENT_SHADER: &str = r#"
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
uniform vec4 u_fwd_mat;
uniform vec4 u_uv_bounds;
uniform vec2 u_dst_offset;
uniform vec2 u_win_size;
uniform vec2 u_radii;
uniform float u_scale;
uniform float u_opacity;
uniform vec4 u_tint;

void main() {
    vec2 d_uv = v_coords - u_uv0;
    vec2 win_pos = u_dst_offset + vec2(dot(u_inv_mat.xy, d_uv), dot(u_inv_mat.zw, d_uv));

    float s = max(u_scale, 1.0);
    vec2 half_size = max(u_win_size * 0.5, vec2(1.0));
    vec2 p = win_pos - half_size;
    float r = (p.y < 0.0) ? u_radii.x : u_radii.y;
    r = clamp(r, 0.0, min(half_size.x, half_size.y));

    vec2 q = abs(p) - half_size + vec2(r);
    float sdf = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;

    // Outward SDF normal (pure GLSL ES 1.00 without derivative extensions)
    vec2 sgn = vec2(p.x < 0.0 ? -1.0 : 1.0, p.y < 0.0 ? -1.0 : 1.0);
    vec2 n_local = (q.x > 0.0 && q.y > 0.0)
        ? normalize(q)
        : ((q.x > q.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0));
    vec2 norm = n_local * sgn;

    // Soft ambient Mac elevation shadow just outside the capsule
    float outer_d = max(sdf, 0.0);
    float shadow_falloff = exp(-outer_d / (3.4 * s)) * (1.0 - smoothstep(4.5 * s, 8.5 * s, outer_d));
    float shadow_dir = mix(0.65, 1.20, clamp(norm.y * 0.5 + 0.5, 0.0, 1.0));
    float shadow_a = 0.28 * shadow_falloff * shadow_dir * u_opacity * alpha;

    float glass_mask = 1.0 - smoothstep(-0.75, 0.75, sdf);
    if (glass_mask <= 0.002) {
        gl_FragColor = vec4(0.0, 0.0, 0.0, shadow_a);
        return;
    }

    float depth = max(-sdf, 0.0) / s;
    float min_half_log = min(half_size.x, half_size.y) / s;
    float bevel_w = clamp(min_half_log * 0.72, 9.0, 24.0);
    float t = clamp(1.0 - depth / bevel_w, 0.0, 1.0);

    // Snell-inspired curved meniscus edge refraction + gentle spherical dome lensing
    float rim_bend = pow(t, 1.70) * (22.0 * s);
    vec2 norm_pos = p / half_size;
    vec2 dome_bend = -norm_pos * (2.6 * s * (1.0 - t * 0.45));
    vec2 refract_px = -norm * rim_bend + dome_bend;

    // Subtle Apple prismatic R/G/B dispersion localized to the high-curvature rim
    float disp_scale = 0.30 * pow(t, 1.30);
    vec2 off_g = vec2(dot(u_fwd_mat.xy, refract_px), dot(u_fwd_mat.zw, refract_px));
    vec2 off_r = off_g * (1.0 + disp_scale);
    vec2 off_b = off_g * (1.0 - disp_scale);

    vec2 lo = u_uv_bounds.xy;
    vec2 hi = u_uv_bounds.zw;
    vec3 bg_r = texture2D(tex, clamp(v_coords + off_r, lo, hi)).rgb;
    vec3 bg_g = texture2D(tex, clamp(v_coords + off_g, lo, hi)).rgb;
    vec3 bg_b = texture2D(tex, clamp(v_coords + off_b, lo, hi)).rgb;
    vec3 refracted = vec3(bg_r.r, bg_g.g, bg_b.b);

    // Jewel-like vibrancy boost so underlying wallpaper/windows stay luminous through glass
    float lum = dot(refracted, vec3(0.2126, 0.7152, 0.0722));
    vec3 vibrant = clamp(mix(vec3(lum), refracted, 1.30), 0.0, 1.0);

    // Adaptive luminance-aware smoky optical glaze for crisp white text legibility
    float scrim_a = mix(0.20, 0.50, smoothstep(0.16, 0.75, lum));
    vec3 glass_tint = mix(vec3(0.15, 0.19, 0.27), vec3(0.05, 0.07, 0.12), smoothstep(0.18, 0.75, lum));
    vec3 body = mix(vibrant, glass_tint, scrim_a);

    // Optional ambient palette tint (e.g. active music artwork sweep)
    if (u_tint.a > 0.001) {
        body = mix(body, u_tint.rgb * 0.85 + vec3(0.10), clamp(u_tint.a * 0.28, 0.0, 0.35));
    }

    // Subtle upper dome illumination + inner Fresnel rim glow
    float vertical_grad = clamp(win_pos.y / max(u_win_size.y, 1.0), 0.0, 1.0);
    body += vec3(0.10, 0.12, 0.16) * pow(1.0 - vertical_grad, 2.1) * (0.45 + 0.55 * t);
    body += vec3(0.14, 0.16, 0.20) * pow(t, 2.8);

    // Sculpted 3D overhead studio specular rim highlight + bottom caustic bounce
    float rim_band = 1.0 - smoothstep(0.45, 2.2, depth);
    float sharp_lip = 1.0 - smoothstep(0.0, 1.2, depth);
    float top_light = max(dot(norm, normalize(vec2(-0.28, -0.96))), 0.0);
    float bottom_bounce = max(dot(norm, normalize(vec2(0.22, 0.97))), 0.0);
    float spec = sharp_lip * (0.20 + 0.54 * pow(top_light, 1.35) + 0.28 * pow(bottom_bounce, 1.7))
               + rim_band  * (0.06 + 0.16 * pow(top_light, 1.8) + 0.10 * pow(bottom_bounce, 2.0));
    body = mix(body, vec3(0.97, 0.99, 1.0), clamp(spec, 0.0, 0.78));

    // Faint dark hairline at the extreme outer boundary for crisp contrast on bright wallpapers
    float edge_contrast = (1.0 - smoothstep(0.0, 0.75, depth)) * 0.16;
    body = mix(body, vec3(0.02, 0.03, 0.05), edge_contrast);

    float g_a = glass_mask * u_opacity * alpha;
    float s_a = (1.0 - glass_mask) * shadow_a;
    vec3 premul = body * g_a;
    float out_a = clamp(g_a + s_a, 0.0, 1.0);

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        premul = vec3(0.0, 0.2, 0.0) + premul * 0.8;
#endif

    gl_FragColor = vec4(premul, out_a);
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
    pub rounded_rect: GlesPixelProgram,
    pub rounded_window: GlesTexProgram,
    pub blur_pass: GlesTexProgram,
    pub liquid_glass: GlesTexProgram,
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
                UniformName::new("u_buttons_right", UniformType::_1f),
                UniformName::new("u_opacity", UniformType::_1f),
            ],
        )?;

        let rounded_rect = renderer.compile_custom_pixel_shader(
            ROUNDED_RECT_FRAGMENT_SHADER,
            &[
                UniformName::new("u_corner_radius", UniformType::_1f),
                UniformName::new("u_border_width", UniformType::_1f),
                UniformName::new("u_fill_color", UniformType::_4f),
                UniformName::new("u_border_color", UniformType::_4f),
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

        let liquid_glass = renderer.compile_custom_texture_shader(
            LIQUID_GLASS_FRAGMENT_SHADER,
            &[
                UniformName::new("u_uv0", UniformType::_2f),
                UniformName::new("u_inv_mat", UniformType::_4f),
                UniformName::new("u_fwd_mat", UniformType::_4f),
                UniformName::new("u_uv_bounds", UniformType::_4f),
                UniformName::new("u_dst_offset", UniformType::_2f),
                UniformName::new("u_win_size", UniformType::_2f),
                UniformName::new("u_radii", UniformType::_2f),
                UniformName::new("u_scale", UniformType::_1f),
                UniformName::new("u_opacity", UniformType::_1f),
                UniformName::new("u_tint", UniformType::_4f),
            ],
        )?;

        Ok(Self {
            shadow,
            border,
            backdrop,
            frosted_glass,
            titlebar,
            rounded_rect,
            rounded_window,
            blur_pass,
            liquid_glass,
        })
    }
}
