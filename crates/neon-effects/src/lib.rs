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

    /// **Hyprland Workspace Slide** (`cubic-bezier(0.16, 1.0, 0.30, 1.0)`):
    /// Snappy launch with zero edge overshoot so both full desktops (wallpaper, windows,
    /// and dock) pan cleanly side-by-side and decelerate smoothly into exact pixel lock.
    pub const HYPRLAND_WORKSPACE: Self = Self::CubicBezier {
        x1: 0.16,
        y1: 1.0,
        x2: 0.30,
        y2: 1.0,
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
    // Guarded Newton-Raphson with bisection fallback for steep Hyprland Bézier curves.
    let mut lo = 0.0_f64;
    let mut hi = 1.0_f64;
    let mut u = x;
    for _ in 0..12 {
        let bx = cubic_component(u, x1, x2);
        let err = bx - x;
        if err.abs() < 1e-7 {
            break;
        }
        if err < 0.0 {
            lo = u;
        } else {
            hi = u;
        }
        let dx = cubic_derivative(u, x1, x2);
        let next = if dx.abs() > 1e-6 { u - err / dx } else { 0.5 * (lo + hi) };
        u = if next > lo && next < hi {
            next
        } else {
            0.5 * (lo + hi)
        };
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

/// Global effects configuration (supports accessibility reduced motion, blur, speed scaling,
/// and per-action Material 3 Expressive animation toggles).
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
    pub open_animation: bool,
    pub close_animation: bool,
    pub minimize_animation: bool,
    pub maximize_animation: bool,
    pub workspace_animation: bool,
    pub overview_animation: bool,
    pub dock_animation: bool,
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
            open_animation: true,
            close_animation: true,
            minimize_animation: true,
            maximize_animation: true,
            workspace_animation: true,
            overview_animation: true,
            dock_animation: true,
        }
    }
}

/// Active dual-workspace Hyprland-style horizontal slidefade transition.
#[derive(Debug, Clone)]
struct WorkspaceTransition {
    /// `+1` moving to a higher/next workspace (slides right -> left),
    /// `-1` moving to a lower/previous workspace (slides left -> right).
    direction: i32,
    /// Full logical slide travel distance (typically `output_width * 1.03` for a sleek gutter).
    travel_x: f64,
    /// Progress `0.0 -> 1.0` evaluated on [`Easing::HYPRLAND_WORKSPACE`].
    progress: Animation,
}

/// Per-frame Hyprland workspace `slidefade` visual transform for incoming or outgoing windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceSlideSample {
    pub offset_x: f64,
    pub scale: f64,
    pub opacity: f32,
}

impl Default for WorkspaceSlideSample {
    fn default() -> Self {
        Self {
            offset_x: 0.0,
            scale: 1.0,
            opacity: 1.0,
        }
    }
}

/// Coordinates all frame-driven compositor animations (window open, close, minimize, restore,
/// maximize/unmaximize/snap/resize morph, focus highlight, and Hyprland dual-workspace slide).
#[derive(Debug, Default)]
pub struct EffectsManager {
    pub config: EffectsConfig,
    window_anims: HashMap<WindowId, WindowAnimations>,
    workspace_transition: Option<WorkspaceTransition>,
}

impl EffectsManager {
    pub fn new(config: EffectsConfig) -> Self {
        Self {
            config,
            window_anims: HashMap::new(),
            workspace_transition: None,
        }
    }

    pub fn scaled_duration(&self, base_ms: u64) -> Duration {
        if !self.config.enabled || self.config.reduced_motion {
            return Duration::ZERO;
        }
        let speed = self.config.speed_scale.clamp(0.1, 5.0);
        Duration::from_secs_f64((base_ms as f64 / 1000.0) / speed)
    }

    /// Android 16 × Material 3 Expressive Default Spatial open animation (default upward bloom).
    pub fn animate_window_open(&mut self, id: WindowId, now: Duration) {
        self.animate_window_open_from(id, 0.0, 42.0, 0.80, now);
    }

    /// Android 16 Material 3 Expressive container-transform app launch springing directly out of
    /// a clicked Dock, App Drawer, or Desktop icon at relative offset `(from_dx, from_dy)`.
    pub fn animate_window_open_from(
        &mut self,
        id: WindowId,
        from_dx: f64,
        from_dy: f64,
        start_scale: f64,
        now: Duration,
    ) {
        let spatial_dur = if self.config.open_animation {
            self.scaled_duration(430)
        } else {
            Duration::ZERO
        };
        let fade_dur = self.scaled_duration(155);
        let entry = self.window_anims.entry(id).or_default();
        if spatial_dur.is_zero() {
            entry.opacity = None;
            entry.offset_x = None;
            entry.offset_y = None;
            entry.scale_x = None;
            entry.scale_y = None;
            return;
        }
        let s0 = start_scale.clamp(0.18, 0.94);
        entry.opacity = Some(Animation::new(
            0.0,
            1.0,
            now,
            fade_dur,
            Easing::M3_EXPRESSIVE_FAST_EFFECTS,
        ));
        entry.offset_x = if from_dx.abs() > 1.0 {
            Some(Animation::new(
                from_dx,
                0.0,
                now,
                spatial_dur,
                Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
            ))
        } else {
            None
        };
        entry.offset_y = Some(Animation::new(
            from_dy,
            0.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_x = Some(Animation::new(
            s0,
            1.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_y = Some(Animation::new(
            s0,
            1.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
    }

    /// Android 16 × Material 3 Expressive close animation. Returns its duration.
    pub fn animate_window_close(&mut self, id: WindowId, now: Duration) -> Duration {
        self.animate_window_close_toward(id, 0.0, 48.0, now)
    }

    /// Android 16 × Material 3 Expressive return-to-icon close animation gliding smoothly toward
    /// the window's Dock icon by `(delta_x, delta_y)` with expressive scale collapse and late fade.
    pub fn animate_window_close_toward(
        &mut self,
        id: WindowId,
        delta_x: f64,
        delta_y: f64,
        now: Duration,
    ) -> Duration {
        if !self.config.close_animation {
            return Duration::ZERO;
        }
        let dur = self.scaled_duration(290);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            return Duration::ZERO;
        }
        let cur_op = entry.opacity.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_sx = entry.scale_x.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_sy = entry.scale_y.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_ox = entry.offset_x.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_oy = entry.offset_y.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);

        // Smooth M3 Expressive Slow Effects spatial retreat + delayed fade so the return-to-icon
        // glide stays crisp and clearly visible before dissolving.
        let exit_curve = Easing::CubicBezier {
            x1: 0.34,
            y1: 0.88,
            x2: 0.34,
            y2: 1.00,
        };
        let fade_delay = Duration::from_secs_f64(dur.as_secs_f64() * 0.24);
        let fade_dur = dur.saturating_sub(fade_delay);

        entry.opacity = Some(Animation::new(
            cur_op,
            0.0,
            now + fade_delay,
            fade_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
        ));
        entry.scale_x = Some(Animation::new(cur_sx, 0.36, now, dur, exit_curve));
        entry.scale_y = Some(Animation::new(cur_sy, 0.36, now, dur, exit_curve));
        entry.offset_x = Some(Animation::new(cur_ox, cur_ox + delta_x, now, dur, exit_curve));
        entry.offset_y = Some(Animation::new(cur_oy, cur_oy + delta_y, now, dur, exit_curve));
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

    /// Android / macOS dock-swoop minimize animation toward `(target_dx, target_dy)` on the Dock.
    pub fn animate_window_minimize(
        &mut self,
        id: WindowId,
        target_dx: f64,
        target_dy: f64,
        now: Duration,
    ) -> Duration {
        if !self.config.minimize_animation {
            return Duration::ZERO;
        }
        let dur = self.scaled_duration(310);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            return Duration::ZERO;
        }
        let cur_op = entry.opacity.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_ox = entry.offset_x.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_oy = entry.offset_y.as_ref().map(|a| a.sample(now)).unwrap_or(0.0);
        let cur_sx = entry.scale_x.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);
        let cur_sy = entry.scale_y.as_ref().map(|a| a.sample(now)).unwrap_or(1.0);

        let swoop = Easing::CubicBezier {
            x1: 0.34,
            y1: 0.88,
            x2: 0.34,
            y2: 1.00,
        };
        let fade_delay = Duration::from_secs_f64(dur.as_secs_f64() * 0.30);
        let fade_dur = dur.saturating_sub(fade_delay);

        entry.opacity = Some(Animation::new(
            cur_op,
            0.0,
            now + fade_delay,
            fade_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_EFFECTS,
        ));
        entry.offset_x = Some(Animation::new(cur_ox, target_dx, now, dur, swoop));
        entry.offset_y = Some(Animation::new(cur_oy, target_dy, now, dur, swoop));
        entry.scale_x = Some(Animation::new(cur_sx, 0.16, now, dur, swoop));
        entry.scale_y = Some(Animation::new(cur_sy, 0.16, now, dur, swoop));
        dur
    }

    /// Android / macOS × Material 3 Expressive spring restore rising directly out of the Dock icon.
    pub fn animate_window_restore(
        &mut self,
        id: WindowId,
        from_dx: f64,
        from_dy: f64,
        now: Duration,
    ) {
        let spatial_dur = if self.config.minimize_animation {
            self.scaled_duration(420)
        } else {
            Duration::ZERO
        };
        let fade_dur = self.scaled_duration(165);
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
            Easing::M3_EXPRESSIVE_FAST_EFFECTS,
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
            0.20,
            1.0,
            now,
            spatial_dur,
            Easing::M3_EXPRESSIVE_DEFAULT_SPATIAL,
        ));
        entry.scale_y = Some(Animation::new(
            0.20,
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
        if !self.config.maximize_animation {
            return;
        }
        let dur = self.scaled_duration(420);
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

    /// Starts a Hyprland-style full-desktop horizontal slide transition sized to
    /// `output_width` plus a sleek 28px dark gutter separating the outgoing and incoming desktops.
    ///
    /// Returns `true` if the animation started, or `false` if animations / reduced motion
    /// caused the switch to complete immediately.
    pub fn animate_workspace_switch_with_width(
        &mut self,
        direction: i32,
        output_width: f64,
        now: Duration,
    ) -> bool {
        if !self.config.workspace_animation {
            self.workspace_transition = None;
            return false;
        }
        let dur = self.scaled_duration(380);
        if dur.is_zero() {
            self.workspace_transition = None;
            return false;
        }
        let dir = if direction >= 0 { 1 } else { -1 };
        let travel_x = output_width.max(640.0) + 28.0;
        self.workspace_transition = Some(WorkspaceTransition {
            direction: dir,
            travel_x,
            progress: Animation::new(0.0, 1.0, now, dur, Easing::HYPRLAND_WORKSPACE),
        });
        true
    }

    /// Starts a Hyprland-style workspace switch animation using a default 1280px travel width.
    pub fn animate_workspace_switch(&mut self, direction: i32, now: Duration) {
        let _ = self.animate_workspace_switch_with_width(direction, 1280.0, now);
    }

    /// Returns `true` if a Hyprland dual-workspace slide transition is currently in flight.
    pub fn is_workspace_transition_active(&self, now: Duration) -> bool {
        self.workspace_transition
            .as_ref()
            .map(|tr| !tr.progress.is_finished(now))
            .unwrap_or(false)
    }

    /// Samples the active Hyprland full-desktop slide transform for the **incoming** workspace.
    pub fn sample_workspace_incoming(&self, now: Duration) -> WorkspaceSlideSample {
        let Some(tr) = &self.workspace_transition else {
            return WorkspaceSlideSample::default();
        };
        if tr.progress.is_finished(now) {
            return WorkspaceSlideSample::default();
        }
        let p = tr.progress.sample(now).clamp(0.0, 1.0);
        let dir = f64::from(tr.direction);
        // Entire incoming desktop (wallpaper + windows + dock) pans crisply at 1:1 scale &
        // full opacity from `+(W + 28)` (going right) or `-(W + 28)` (going left) to `0.0`.
        WorkspaceSlideSample {
            offset_x: dir * tr.travel_x * (1.0 - p),
            scale: 1.0,
            opacity: 1.0,
        }
    }

    /// Samples the active Hyprland full-desktop slide transform for the **outgoing** workspace.
    ///
    /// Returns `None` once the workspace transition finishes so outgoing windows can be pruned.
    pub fn sample_workspace_outgoing(&self, now: Duration) -> Option<WorkspaceSlideSample> {
        let tr = self.workspace_transition.as_ref()?;
        if tr.progress.is_finished(now) {
            return None;
        }
        let p = tr.progress.sample(now).clamp(0.0, 1.0);
        let dir = f64::from(tr.direction);
        // Entire outgoing desktop (wallpaper + windows + dock) pans simultaneously at 1:1 scale &
        // full opacity from `0.0` to `-(W + 28)` (going right) or `+(W + 28)` (going left),
        // keeping a constant 28px dark gutter between the two desktops on every frame.
        Some(WorkspaceSlideSample {
            offset_x: -dir * tr.travel_x * p,
            scale: 1.0,
            opacity: 1.0,
        })
    }

    /// Removes animation state when a window is destroyed.
    pub fn remove_window(&mut self, id: WindowId) {
        self.window_anims.remove(&id);
    }

    /// Samples current visual state for `id` including any active incoming workspace slide.
    pub fn sample_window(&self, id: WindowId, focused: bool, now: Duration) -> WindowVisualState {
        let ws_in = self.sample_workspace_incoming(now);

        let Some(entry) = self.window_anims.get(&id) else {
            return WindowVisualState {
                opacity: ws_in.opacity,
                offset_x: ws_in.offset_x,
                offset_y: 0.0,
                scale_x: ws_in.scale,
                scale_y: ws_in.scale,
                focus_progress: if focused { 1.0 } else { 0.0 },
            };
        };

        let opacity = (entry
            .opacity
            .as_ref()
            .map(|a| a.sample(now) as f32)
            .unwrap_or(1.0)
            * ws_in.opacity)
            .clamp(0.0, 1.0);
        let offset_x = entry
            .offset_x
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(0.0)
            + ws_in.offset_x;
        let offset_y = entry
            .offset_y
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(0.0);
        let scale_x = (entry
            .scale_x
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(1.0)
            * ws_in.scale)
            .clamp(0.10, 3.5);
        let scale_y = (entry
            .scale_y
            .as_ref()
            .map(|a| a.sample(now))
            .unwrap_or(1.0)
            * ws_in.scale)
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

        if let Some(tr) = &self.workspace_transition {
            if tr.progress.is_finished(now) {
                self.workspace_transition = None;
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
        if self.is_workspace_transition_active(now) {
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
            Easing::HYPRLAND_WORKSPACE,
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
        assert!(fx.sample_workspace_outgoing(Duration::from_millis(10)).is_none());
    }

    #[test]
    fn hyprland_workspace_switch_glides_outgoing_and_incoming_simultaneously() {
        let mut fx = EffectsManager::default();
        let t0 = Duration::from_millis(100);
        assert!(fx.animate_workspace_switch_with_width(1, 1920.0, t0));
        assert!(fx.is_workspace_transition_active(t0));

        // At t0: incoming starts off-right (+1948px), outgoing starts at 0px, constant 28px gutter.
        let in0 = fx.sample_workspace_incoming(t0);
        let out0 = fx.sample_workspace_outgoing(t0).expect("outgoing active at t0");
        assert!((in0.offset_x - 1948.0).abs() < 1e-3);
        assert!(out0.offset_x.abs() < 1e-3);
        assert!((in0.scale - 1.0).abs() < 1e-6 && (in0.opacity - 1.0).abs() < 1e-6);
        assert!((out0.scale - 1.0).abs() < 1e-6 && (out0.opacity - 1.0).abs() < 1e-6);

        // Mid-flight (~60ms into 380ms): both desktops visibly pan side-by-side with constant 1948px spacing.
        let t_mid = t0 + Duration::from_millis(60);
        let in_mid = fx.sample_workspace_incoming(t_mid);
        let out_mid = fx.sample_workspace_outgoing(t_mid).expect("outgoing active mid-flight");
        assert!(in_mid.offset_x > 400.0 && in_mid.offset_x < 1500.0, "in_mid={in_mid:?}");
        assert!(out_mid.offset_x < -400.0 && out_mid.offset_x > -1500.0, "out_mid={out_mid:?}");
        assert!(
            ((in_mid.offset_x - out_mid.offset_x) - 1948.0).abs() < 1e-3,
            "constant desktop gutter must be preserved mid-flight"
        );

        // Settled at t0 + 400ms: incoming locks to (0, 1.0, 1.0), outgoing clears to None.
        let t_end = t0 + Duration::from_millis(400);
        assert!(!fx.tick(t_end));
        assert!(fx.sample_workspace_outgoing(t_end).is_none());
        let in_end = fx.sample_workspace_incoming(t_end);
        assert_eq!(in_end.offset_x, 0.0);
        assert_eq!(in_end.scale, 1.0);
        assert_eq!(in_end.opacity, 1.0);

        // Reverse direction (-1): incoming starts off-left (-travel_x), outgoing glides right (+x).
        assert!(fx.animate_workspace_switch_with_width(-1, 1280.0, t_end));
        let in_rev = fx.sample_workspace_incoming(t_end + Duration::from_millis(60));
        let out_rev = fx
            .sample_workspace_outgoing(t_end + Duration::from_millis(60))
            .expect("reverse outgoing active");
        assert!(in_rev.offset_x < 0.0, "expected negative incoming offset, got {in_rev:?}");
        assert!(out_rev.offset_x > 0.0, "expected positive outgoing offset, got {out_rev:?}");
    }

    #[test]
    fn android_origin_anchored_open_springs_from_icon() {
        let mut fx = EffectsManager::default();
        let id = WindowId(77);
        let t0 = Duration::from_millis(100);
        // Simulate launching from a bottom-left Dock icon relative to window center.
        fx.animate_window_open_from(id, -180.0, 260.0, 0.34, t0);

        let s0 = fx.sample_window(id, true, t0);
        assert!((s0.offset_x - (-180.0)).abs() < 1e-3);
        assert!((s0.offset_y - 260.0).abs() < 1e-3);
        assert!((s0.scale_x - 0.34).abs() < 1e-3);
        assert!((s0.scale_y - 0.34).abs() < 1e-3);

        // Mid-spring: opacity is already crisp while scale/offset glide toward window target.
        let s_mid = fx.sample_window(id, true, t0 + Duration::from_millis(180));
        assert!(s_mid.opacity > 0.95, "opacity should lock in fast: {}", s_mid.opacity);
        assert!(s_mid.scale_x > 0.85, "scale should bloom fast: {}", s_mid.scale_x);
        assert!(s_mid.offset_y < 120.0 && s_mid.offset_y > -20.0);

        // Settled cleanly at end of 430ms Default Spatial spring.
        let s_end = fx.sample_window(id, true, t0 + Duration::from_millis(450));
        assert!((s_end.offset_x).abs() < 1e-6);
        assert!((s_end.offset_y).abs() < 1e-6);
        assert!((s_end.scale_x - 1.0).abs() < 1e-6);
        assert!((s_end.scale_y - 1.0).abs() < 1e-6);
        assert!((s_end.opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn per_action_animation_toggles_disable_individually() {
        let t0 = Duration::from_millis(50);
        let id = WindowId(99);

        let mut fx = EffectsManager::new(EffectsConfig {
            open_animation: false,
            close_animation: false,
            minimize_animation: false,
            maximize_animation: false,
            workspace_animation: false,
            ..Default::default()
        });

        fx.animate_window_open_from(id, -120.0, 200.0, 0.34, t0);
        assert!(!fx.has_active_animations(t0));

        assert!(fx.animate_window_close_toward(id, 0.0, 180.0, t0).is_zero());
        assert!(fx.animate_window_minimize(id, 0.0, 180.0, t0).is_zero());
        fx.animate_window_restore(id, 0.0, 180.0, t0);
        assert!(!fx.has_active_animations(t0));

        fx.animate_window_geometry(
            id,
            Rect::new(
                neon_window_manager::geometry::Point::new(100, 100),
                neon_window_manager::geometry::Size::new(800, 500),
            ),
            Rect::new(
                neon_window_manager::geometry::Point::new(0, 32),
                neon_window_manager::geometry::Size::new(1920, 976),
            ),
            t0,
        );
        assert!(!fx.has_active_animations(t0));

        assert!(!fx.animate_workspace_switch_with_width(1, 1920.0, t0));
        assert!(!fx.is_workspace_transition_active(t0));
    }
}
