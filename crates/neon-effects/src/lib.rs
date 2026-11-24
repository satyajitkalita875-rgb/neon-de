//! Time-based, interruptible, frame-driven animation engine for **NEON DE**.
//!
//! Powered by **Material 3 Expressive** spring physics (`Spatial` springs with controlled organic
//! overshoot for position/size/scale morphs, and critically-damped `Effects` springs for crisp
//! zero-overshoot opacity/focus transitions) combined with Hyprland-style window open/close,
//! dock-swoop minimize/restore, and smooth geometry morphing for maximize/unmaximize/snap/resize.

use std::{collections::HashMap, time::Duration};

use neon_window_manager::{WindowId, geometry::Rect};

/// Supported easing functions and physics curves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// CSS-style cubic Bézier curve with control points `(x1, y1)` and `(x2, y2)`.
    CubicBezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    /// Damped harmonic oscillator spring curve (`m = mass`, `k = stiffness`, `c = damping`).
    Spring {
        stiffness: f64,
        damping: f64,
        mass: f64,
    },
}

impl Easing {
    /// NEON DE smooth deceleration curve (crisp start, fluid settle).
    pub const NEON_STANDARD: Self = Self::CubicBezier {
        x1: 0.2,
        y1: 0.0,
        x2: 0.0,
        y2: 1.0,
    };

    /// Crisp spring tuned for window micro-interactions.
    pub const NEON_SPRING: Self = Self::Spring {
        stiffness: 260.0,
        damping: 26.0,
        mass: 1.0,
    };

    /// **Material 3 Expressive — Fast Spatial** (`k = 800, ζ = 0.6`, lively bouncy settle).
    pub const M3_EXPRESSIVE_FAST_SPATIAL: Self = Self::Spring {
        stiffness: 800.0,
        damping: 33.9411,
        mass: 1.0,
    };

    /// **Material 3 Expressive — Default Spatial** (`k = 380, ζ = 0.8`, organic overshoot).
    /// Used for window open, restore, maximize, unmaximize, snap, and resize morphs.
    pub const M3_EXPRESSIVE_DEFAULT_SPATIAL: Self = Self::Spring {
        stiffness: 380.0,
        damping: 31.1897,
        mass: 1.0,
    };

    /// **Material 3 Expressive — Slow Spatial** (`k = 200, ζ = 0.8`, sweeping viewport physics).
    /// Used for multi-desktop workspace switches.
    pub const M3_EXPRESSIVE_SLOW_SPATIAL: Self = Self::Spring {
        stiffness: 200.0,
        damping: 22.6274,
        mass: 1.0,
    };

    /// **Material 3 Expressive — Fast Effects** (`k = 3800, ζ = 1.0`, critically damped, zero bounce).
    pub const M3_EXPRESSIVE_FAST_EFFECTS: Self = Self::Spring {
        stiffness: 3800.0,
        damping: 123.2883,
        mass: 1.0,
    };

    /// **Material 3 Expressive — Default Effects** (`k = 1600, ζ = 1.0`, critically damped, zero bounce).
    /// Used for window opacity and focus ring transitions.
    pub const M3_EXPRESSIVE_DEFAULT_EFFECTS: Self = Self::Spring {
        stiffness: 1600.0,
        damping: 80.0,
        mass: 1.0,
    };

    /// **Material 3 Expressive — Slow Effects** (`k = 800, ζ = 1.0`, critically damped).
    pub const M3_EXPRESSIVE_SLOW_EFFECTS: Self = Self::Spring {
        stiffness: 800.0,
        damping: 56.5685,
        mass: 1.0,
    };

    /// Evaluates the curve at normalized time `t` in `[0.0, 1.0]`.
    pub fn evaluate(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t * t,
            Self::EaseOut => {
                let inv = 1.0 - t;
                1.0 - inv * inv * inv
            }
            Self::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let inv = -2.0 * t + 2.0;
                    1.0 - (inv * inv * inv) / 2.0
                }
            }
            Self::CubicBezier { x1, y1, x2, y2 } => eval_cubic_bezier(t, x1, y1, x2, y2),
            Self::Spring {
                stiffness,
                damping,
                mass,
            } => eval_spring(t, stiffness, damping, mass),
        }
    }
}

fn eval_cubic_bezier(x: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let mut u = x;
    for _ in 0..8 {
        let bx = cubic_component(u, x1, x2);
        let dx = cubic_derivative(u, x1, x2);
        let err = bx - x;
        if err.abs() < 1e-6 || dx.abs() < 1e-6 {
            break;
        }
        u = (u - err / dx).clamp(0.0, 1.0);
    }
    cubic_component(u, y1, y2)
}

fn cubic_component(u: f64, p1: f64, p2: f64) -> f64 {
    let inv = 1.0 - u;
    3.0 * inv * inv * u * p1 + 3.0 * inv * u * u * p2 + u * u * u
}

fn cubic_derivative(u: f64, p1: f64, p2: f64) -> f64 {
    let inv = 1.0 - u;
    3.0 * inv * inv * p1 + 6.0 * inv * u * (p2 - p1) + 3.0 * u * u * (1.0 - p2)
}

fn eval_spring(t: f64, stiffness: f64, damping: f64, mass: f64) -> f64 {
    let m = mass.max(0.001);
    let k = stiffness.max(1.0);
    let c = damping.max(0.0);
    let omega0 = (k / m).sqrt();
    let zeta = c / (2.0 * (k * m).sqrt());
    // Compute a natural physical settle horizon (~5 decay constants) so fast springs feel
    // energetic while slower spatial springs retain their graceful expressive overshoot.
    let settle_horizon = (5.2 / (zeta.max(0.35) * omega0)).clamp(0.22, 0.95);
    let tau = t * settle_horizon;

    let displacement = if zeta < 1.0 - 1e-4 {
        let omega_d = omega0 * (1.0 - zeta * zeta).sqrt();
        (-zeta * omega0 * tau).exp()
            * ((omega_d * tau).cos() + (zeta * omega0 / omega_d) * (omega_d * tau).sin())
    } else if (zeta - 1.0).abs() <= 1e-4 {
        (-omega0 * tau).exp() * (1.0 + omega0 * tau)
    } else {
        let r1 = -omega0 * (zeta - (zeta * zeta - 1.0).sqrt());
        let r2 = -omega0 * (zeta + (zeta * zeta - 1.0).sqrt());
        (r2 * (r1 * tau).exp() - r1 * (r2 * tau).exp()) / (r2 - r1)
    };
    let raw = 1.0 - displacement;
    // Smoothly feather the tiny residual tail over the final 8% so arrival at 1.0 never snaps.
    if t > 0.92 {
        let blend = (t - 0.92) / 0.08;
        let smooth = blend * blend * (3.0 - 2.0 * blend);
        raw + (1.0 - raw) * smooth
    } else {
        raw
    }
}

/// A single interruptible, chainable scalar animation segment.
#[derive(Debug, Clone)]
pub struct Animation {
    pub from: f64,
    pub to: f64,
    pub start: Duration,
    pub duration: Duration,
    pub easing: Easing,
    cancelled: bool,
    next: Option<Box<Animation>>,
}

impl Animation {
    pub fn new(from: f64, to: f64, start: Duration, duration: Duration, easing: Easing) -> Self {
        Self {
            from,
            to,
            start,
            duration,
            easing,
            cancelled: false,
            next: None,
        }
    }

    /// Chains a follow-up animation segment that begins automatically when this segment completes.
    pub fn chain(mut self, to: f64, duration: Duration, easing: Easing) -> Self {
        let next_start = self.start + self.duration;
        self.next = Some(Box::new(Self::new(
            self.to, to, next_start, duration, easing,
        )));
        self
    }

    /// Interrupts the animation mid-flight and retargets smoothly from its current value.
    pub fn retarget(&mut self, new_to: f64, now: Duration, duration: Duration, easing: Easing) {
        let current = self.sample(now);
        self.from = current;
        self.to = new_to;
        self.start = now;
        self.duration = duration;
        self.easing = easing;
        self.cancelled = false;
        self.next = None;
    }

    /// Cancels the animation immediately, freezing it at its target/settled state.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.next = None;
    }

    /// Samples the animated scalar value at timestamp `now`.
    pub fn sample(&self, now: Duration) -> f64 {
        if self.cancelled {
            return self.to;
        }
        if now <= self.start {
            return self.from;
        }
        if self.duration.is_zero() {
            if let Some(next) = &self.next {
                return next.sample(now);
            }
            return self.to;
        }
        let elapsed = now.saturating_sub(self.start);
        if elapsed >= self.duration {
            if let Some(next) = &self.next {
                return next.sample(now);
            }
            return self.to;
        }
        let t = elapsed.as_secs_f64() / self.duration.as_secs_f64();
        let eased = self.easing.evaluate(t);
        self.from + (self.to - self.from) * eased
    }

    /// Returns `true` when this animation (and any chained segments) has settled.
    pub fn is_finished(&self, now: Duration) -> bool {
        if self.cancelled {
            return true;
        }
        let segment_done = now >= self.start + self.duration;
        if !segment_done {
            return false;
        }
        match &self.next {
            Some(next) => next.is_finished(now),
            None => true,
        }
    }
}

/// Snapshot of animated visual properties for a single managed window on the current frame.
#[derive(Debug, Clone, Copy)]
pub struct WindowVisualState {
    pub opacity: f32,
    pub offset_x: f64,
    pub offset_y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub focus_progress: f32,
}

impl Default for WindowVisualState {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            focus_progress: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct WindowAnimations {
    opacity: Option<Animation>,
    offset_x: Option<Animation>,
    offset_y: Option<Animation>,
    scale_x: Option<Animation>,
    scale_y: Option<Animation>,
    focus_progress: Option<Animation>,
    last_focus_value: f32,
}

/// Global effects configuration (supports accessibility reduced motion, blur, and speed scaling).
#[derive(Debug, Clone, Copy)]
pub struct EffectsConfig {
    pub enabled: bool,
    pub reduced_motion: bool,
    pub speed_scale: f64,
    pub shadows_enabled: bool,
    pub borders_enabled: bool,
    pub blur_enabled: bool,
    pub blur_strength: f32,
    pub inactive_opacity: f32,
    pub corner_radius: f32,
}

impl Default for EffectsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reduced_motion: false,
            speed_scale: 1.0,
            shadows_enabled: true,
            borders_enabled: true,
            blur_enabled: true,
            blur_strength: 1.0,
            inactive_opacity: 0.96,
            corner_radius: 12.0,
        }
    }
}

/// Coordinates all frame-driven compositor animations (window open, close, minimize, restore,
/// maximize/unmaximize/snap/resize morph, focus highlight, and workspace slide).
#[derive(Debug, Default)]
pub struct EffectsManager {
    pub config: EffectsConfig,
    window_anims: HashMap<WindowId, WindowAnimations>,
    workspace_offset_x: Option<Animation>,
}

impl EffectsManager {
    pub fn new(config: EffectsConfig) -> Self {
        Self {
            config,
            window_anims: HashMap::new(),
            workspace_offset_x: None,
        }
    }

    pub fn scaled_duration(&self, base_ms: u64) -> Duration {
        if !self.config.enabled || self.config.reduced_motion {
            return Duration::ZERO;
        }
        let speed = self.config.speed_scale.clamp(0.1, 5.0);
        Duration::from_secs_f64((base_ms as f64 / 1000.0) / speed)
    }

    /// Hyprland `popin` × Material 3 Expressive Default Spatial open animation.
    pub fn animate_window_open(&mut self, id: WindowId, now: Duration) {
        let spatial_dur = self.scaled_duration(360);
        let fade_dur = self.scaled_duration(200);
        let entry = self.window_anims.entry(id).or_default();
        if spatial_dur.is_zero() {
            entry.opacity = None;
            entry.offset_x = None;
            entry.offset_y = None;
            entry.scale_x = None;
            entry.scale_y = None;
            return;
        }
        entry.opacity = Some(Animation::new(
            0.0,
            1.0,
            now,
            fade_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
        ));
        entry.offset_x = None;
        entry.offset_y = Some(Animation::new(
            24.0,
            0.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_x = Some(Animation::new(
            0.86,
            1.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_y = Some(Animation::new(
            0.86,
            1.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
    }

    /// Hyprland `popout` × Material 3 Expressive close animation. Returns its duration.
    pub fn animate_window_close(&mut self, id: WindowId, now: Duration) -> Duration {
        let dur = self.scaled_duration(185);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            return Duration::ZERO;
        }
        let cur_op = entry.opacity.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_sx = entry.scale_x.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_sy = entry.scale_y.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_oy = entry.offset_y.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);

        entry.opacity = Some(Animation::new(
            cur_op,
            0.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_FAST_EFFECTS,
        ));
        entry.scale_x = Some(Animation::new(
            cur_sx,
            0.84,
            now,
            dur,
            Easing::M3_EXPRESSIVE_FAST_EFFECTS,
        ));
        entry.scale_y = Some(Animation::new(
            cur_sy,
            0.84,
            now,
            dur,
            Easing::M3_EXPRESSIVE_FAST_EFFECTS,
        ));
        entry.offset_y = Some(Animation::new(
            cur_oy,
            cur_oy + 20.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_FAST_EFFECTS,
        ));
        dur
    }

    /// Smoothly restores a window that declined closure (e.g., prompted to save unsaved changes).
    pub fn animate_window_cancel_close(&mut self, id: WindowId, now: Duration) {
        let dur = self.scaled_duration(260);
        let fade_dur = self.scaled_duration(180);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            entry.opacity = None;
            entry.offset_x = None;
            entry.offset_y = None;
            entry.scale_x = None;
            entry.scale_y = None;
            return;
        }
        let cur_op = entry.opacity.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_sx = entry.scale_x.as_ref().map(|a| a.sample(now)).unwrap_or(0.86);
        let cur_sy = entry.scale_y.as_ref().map(|a| a.sample(now)).unwrap_or(0.86);
        let cur_oy = entry.offset_y.as_ref().map(|a| a.sample(now)).unwrap_or(16.0);

        entry.opacity = Some(Animation::new(
            cur_op,
            1.0,
            now,
            fade_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
        ));
        entry.scale_x = Some(Animation::new(
            cur_sx,
            1.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_FAST_SPATIAL,
        ));
        entry.scale_y = Some(Animation::new(
            cur_sy,
            1.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_FAST_SPATIAL,
        ));
        entry.offset_y = Some(Animation::new(
            cur_oy,
            0.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_FAST_SPATIAL,
        ));
    }

    /// macOS dock-swoop minimize animation toward `(target_dx, target_dy)` below the window.
    pub fn animate_window_minimize(
        &mut self,
        id: WindowId,
        target_dx: f64,
        target_dy: f64,
        now: Duration,
    ) -> Duration {
        let dur = self.scaled_duration(240);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            return Duration::ZERO;
        }
        let cur_op = entry.opacity.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_ox = entry.offset_x.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_oy = entry.offset_y.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_sx = entry.scale_x.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_sy = entry.scale_y.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);

        entry.opacity = Some(Animation::new(
            cur_op,
            0.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
        ));
        entry.offset_x = Some(Animation::new(cur_ox, target_dx, now, dur, Easing::NEON_STANDARD));
        entry.offset_y = Some(Animation::new(cur_oy, target_dy, now, dur, Easing::NEON_STANDARD));
        entry.scale_x = Some(Animation::new(cur_sx, 0.22, now, dur, Easing::NEON_STANDARD));
        entry.scale_y = Some(Animation::new(cur_sy, 0.22, now, dur, Easing::NEON_STANDARD));
        dur
    }

    /// macOS × Material 3 Expressive spring restore animation rising back up from the Dock.
    pub fn animate_window_restore(
        &mut self,
        id: WindowId,
        from_dx: f64,
        from_dy: f64,
        now: Duration,
    ) {
        let spatial_dur = self.scaled_duration(380);
        let fade_dur = self.scaled_duration(200);
        let entry = self.window_anims.entry(id).or_default();
        if spatial_dur.is_zero() {
            entry.opacity = None;
            entry.offset_x = None;
            entry.offset_y = None;
            entry.scale_x = None;
            entry.scale_y = None;
            return;
        }
        entry.opacity = Some(Animation::new(
            0.0,
            1.0,
            now,
            fade_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
        ));
        entry.offset_x = Some(Animation::new(
            from_dx,
            0.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.offset_y = Some(Animation::new(
            from_dy,
            0.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_x = Some(Animation::new(
            0.28,
            1.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_y = Some(Animation::new(
            0.28,
            1.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
    }

    /// Hyprland × Material 3 Expressive spatial morph between `old_rect` and `new_rect`
    /// (maximize, unmaximize, snap, fullscreen, or programmatic resize).
    pub fn animate_window_geometry(
        &mut self,
        id: WindowId,
        old_rect: Rect,
        new_rect: Rect,
        now: Duration,
    ) {
        let dur = self.scaled_duration(380);
        if dur.is_zero() || old_rect.size.w <= 0 || old_rect.size.h <= 0 || new_rect.size.w <= 0 || new_rect.size.h <= 0 {
            return;
        }
        let dx_edge = (old_rect.loc.x - new_rect.loc.x).abs();
        let dy_edge = (old_rect.loc.y - new_rect.loc.y).abs();
        let dw = (old_rect.size.w - new_rect.size.w).abs();
        let dh = (old_rect.size.h - new_rect.size.h).abs();
        if dx_edge < 6 && dy_edge < 6 && dw < 6 && dh < 6 {
            return;
        }

        let entry = self.window_anims.entry(id).or_default();
        // Account for any in-flight transform so rapid maximize/unmaximize clicks retarget seamlessly.
        let cur_ox = entry.offset_x.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_oy = entry.offset_y.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_sx = entry.scale_x.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_sy = entry.scale_y.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);

        let visual_old_w = (old_rect.size.w as f64) * cur_sx;
        let visual_old_h = (old_rect.size.h as f64) * cur_sy;
        let old_cx = old_rect.loc.x as f64 + cur_ox + old_rect.size.w as f64 * 0.5;
        let old_cy = old_rect.loc.y as f64 + cur_oy + old_rect.size.h as f64 * 0.5;

        let new_cx = new_rect.loc.x as f64 + new_rect.size.w as f64 * 0.5;
        let new_cy = new_rect.loc.y as f64 + new_rect.size.h as f64 * 0.5;

        let start_sx = (visual_old_w / new_rect.size.w as f64).clamp(0.22, 3.2);
        let start_sy = (visual_old_h / new_rect.size.h as f64).clamp(0.22, 3.2);
        let start_ox = old_cx - new_cx;
        let start_oy = old_cy - new_cy;

        entry.offset_x = Some(Animation::new(
            start_ox,
            0.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.offset_y = Some(Animation::new(
            start_oy,
            0.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_x = Some(Animation::new(
            start_sx,
            1.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_y = Some(Animation::new(
            start_sy,
            1.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
    }

    /// Smoothly transitions focus highlight (`0.0` unfocused <-> `1.0` focused).
    pub fn animate_focus(&mut self, id: WindowId, focused: bool, now: Duration) {
        let target = if focused { 1.0 } else { 0.0 };
        let dur = self.scaled_duration(180);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            entry.focus_progress = None;
            entry.last_focus_value = target as f32;
            return;
        }
        if let Some(anim) = entry.focus_progress.as_mut() {
            anim.retarget(target, now, dur, Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS);
        } else {
            entry.focus_progress = Some(Animation::new(
                f64::from(entry.last_focus_value),
                target,
                now,
                dur,
                Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
            ));
        }
    }

    /// Starts a Material 3 Expressive Slow Spatial horizontal workspace transition slide.
    pub fn animate_workspace_switch(&mut self, direction: i32, now: Duration) {
        let dur = self.scaled_duration(380);
        if dur.is_zero() {
            self.workspace_offset_x = None;
            return;
        }
        let start_offset = if direction >= 0 { 320.0 } else { -320.0 };
        self.workspace_offset_x = Some(Animation::new(
            start_offset,
            0.0,
            now,
            dur,
            Easing::M3_EXPRESSIVE_SLOW_SPATIAL,
        ));
    }

    /// Removes animation state when a window is destroyed.
    pub fn remove_window(&mut self, id: WindowId) {
        self.window_anims.remove(&id);
    }

    /// Samples current visual state for `id` including any active workspace slide offset.
    pub fn sample_window(&self, id: WindowId, focused: bool, now: Duration) -> WindowVisualState {
        let ws_dx = self
            .workspace_offset_x
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(0.0);

        let Some(entry) = self.window_anims.get(&id) else {
            return WindowVisualState {
                opacity: 1.0,
                offset_x: ws_dx,
                offset_y: 0.0,
                scale_x: 1.0,
                scale_y: 1.0,
                focus_progress: if focused { 1.0 } else { 0.0 },
            };
        };

        let opacity = entry
            .opacity
            .as_ref()
            .map(|a| a.sample(now) as f32)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let offset_x = entry
            .offset_x
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(0.0)
            + ws_dx;
        let offset_y = entry
            .offset_y
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(0.0);
        let scale_x = entry
            .scale_x
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(1.0)
            .clamp(0.10, 3.5);
        let scale_y = entry
            .scale_y
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(1.0)
            .clamp(0.10, 3.5);
        let focus_progress = entry
            .focus_progress
            .as_ref()
            .map(|a| a.sample(now) as f32)
            .unwrap_or(if focused { 1.0 } else { 0.0 })
            .clamp(0.0, 1.0);

        WindowVisualState {
            opacity,
            offset_x,
            offset_y,
            scale_x,
            scale_y,
            focus_progress,
        }
    }

    /// Prunes finished animations and returns `true` if any animation is still active.
    pub fn tick(&mut self, now: Duration) -> bool {
        let mut active = false;

        if let Some(ws) = &self.workspace_offset_x {
            if ws.is_finished(now) {
                self.workspace_offset_x = None;
            } else {
                active = true;
            }
        }

        for entry in self.window_anims.values_mut() {
            for slot in [
                &mut entry.opacity,
                &mut entry.offset_x,
                &mut entry.offset_y,
                &mut entry.scale_x,
                &mut entry.scale_y,
            ] {
                if let Some(a) = slot {
                    if a.is_finished(now) {
                        *slot = None;
                    } else {
                        active = true;
                    }
                }
            }
            if let Some(a) = &entry.focus_progress {
                let val = a.sample(now) as f32;
                entry.last_focus_value = val;
                if a.is_finished(now) {
                    entry.focus_progress = None;
                } else {
                    active = true;
                }
            }
        }

        active
    }

    /// Returns `true` if any animation is currently in progress at `now`.
    pub fn has_active_animations(&self, now: Duration) -> bool {
        if self
            .workspace_offset_x
            .as_ref()
            .map(|a| !a.is_finished(now))
            .unwrap_or(false)
        {
            return true;
        }
        self.window_anims.values().any(|entry| {
            [
                &entry.opacity,
                &entry.offset_x,
                &entry.offset_y,
                &entry.scale_x,
                &entry.scale_y,
                &entry.focus_progress,
            ]
            .into_iter()
            .any(|slot| slot.as_ref().map(|a| !a.is_finished(now)).unwrap_or(false))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neon_window_manager::geometry::{Point, Size};

    #[test]
    fn easings_satisfy_endpoints_and_monotonic_progress() {
        let curves = [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::NEON_STANDARD,
            Easing::NEON_SPRING,
            Easing::M3_EXPRESSIVE_FAST_SPATIAL,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
            Easing::M3_EXPRESSIVE_SLOW_SPATIAL,
            Easing::M3_EXPRESSIVE_FAST_EFFECTS,
            Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
            Easing::M3_EXPRESSIVE_SLOW_EFFECTS,
        ];
        for curve in curves {
            assert_eq!(curve.evaluate(0.0), 0.0);
            assert_eq!(curve.evaluate(1.0), 1.0);
            let mid = curve.evaluate(0.5);
            assert!(mid > 0.0 && mid < 1.30, "mid={mid} for {curve:?}");
        }
        // Critically damped M3 Effects springs must never overshoot 1.0.
        for step in 1..100 {
            let t = f64::from(step) / 100.0;
            let val = Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS.evaluate(t);
            assert!(val <= 1.0001, "M3 Effects spring overshot at t={t}: {val}");
        }
    }

    #[test]
    fn animation_retarget_is_continuous_without_jump() {
        let t0 = Duration::from_millis(0);
        let mut anim = Animation::new(0.0, 100.0, t0, Duration::from_millis(200), Easing::Linear);

        let t_mid = Duration::from_millis(100);
        let mid_before = anim.sample(t_mid);
        assert!((mid_before - 50.0).abs() < 1e-6);

        anim.retarget(0.0, t_mid, Duration::from_millis(100), Easing::Linear);
        let mid_after = anim.sample(t_mid);
        assert!((mid_after - 50.0).abs() < 1e-6, "retarget must not jump");
        assert!((anim.sample(Duration::from_millis(150)) - 25.0).abs() < 1e-6);
        assert!(anim.is_finished(Duration::from_millis(200)));
    }

    #[test]
    fn geometry_morph_animates_scale_and_center_offset_smoothly() {
        let mut fx = EffectsManager::default();
        let id = WindowId(7);
        let old_r = Rect::new(Point::new(200, 150), Size::new(800, 500));
        let new_r = Rect::new(Point::new(0, 30), Size::new(1920, 976));
        let t0 = Duration::from_millis(100);

        fx.animate_window_geometry(id, old_r, new_r, t0);
        let s0 = fx.sample_window(id, true, t0);
        assert!((s0.scale_x - (800.0 / 1920.0)).abs() < 0.01);
        assert!((s0.scale_y - (500.0 / 976.0)).abs() < 0.01);

        let s_end = fx.sample_window(id, true, t0 + Duration::from_millis(500));
        assert!((s_end.scale_x - 1.0).abs() < 1e-6);
        assert!((s_end.scale_y - 1.0).abs() < 1e-6);
        assert!(s_end.offset_x.abs() < 1e-6);
        assert!(s_end.offset_y.abs() < 1e-6);
    }

    #[test]
    fn animation_chaining_and_cancellation() {
        let mut anim = Animation::new(
            0.0,
            1.0,
            Duration::from_millis(0),
            Duration::from_millis(100),
            Easing::Linear,
        )
        .chain(0.5, Duration::from_millis(100), Easing::Linear);

        assert!((anim.sample(Duration::from_millis(100)) - 1.0).abs() < 1e-6);
        assert!(!anim.is_finished(Duration::from_millis(150)));
        assert!((anim.sample(Duration::from_millis(150)) - 0.75).abs() < 1e-6);
        assert!(anim.is_finished(Duration::from_millis(200)));

        anim.cancel();
        assert!(anim.is_finished(Duration::from_millis(10)));
    }

    #[test]
    fn reduced_motion_completes_immediately() {
        let mut fx = EffectsManager::new(EffectsConfig {
            reduced_motion: true,
            ..Default::default()
        });
        let id = WindowId(1);
        fx.animate_window_open(id, Duration::from_millis(10));
        fx.animate_workspace_switch(1, Duration::from_millis(10));
        assert!(!fx.has_active_animations(Duration::from_millis(10)));

        let vis = fx.sample_window(id, true, Duration::from_millis(10));
        assert_eq!(vis.opacity, 1.0);
        assert_eq!(vis.offset_x, 0.0);
        assert_eq!(vis.offset_y, 0.0);
        assert_eq!(vis.scale_x, 1.0);
        assert_eq!(vis.scale_y, 1.0);
    }
}
