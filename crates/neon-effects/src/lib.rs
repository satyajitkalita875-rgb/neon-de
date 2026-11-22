//! Time-based, interruptible, frame-driven animation engine for **NEON DE**.
//!
//! Designed to never block the compositor event loop: every animation is a pure function of
//! monotonic timestamp (`Duration`), supports seamless mid-flight retargeting, cancellation,
//! chaining, cubic Bézier curves, spring physics, and accessibility reduced-motion fallback.

use std::{collections::HashMap, time::Duration};

use neon_window_manager::WindowId;

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
    /// Damped harmonic oscillator spring curve.
    Spring {
        stiffness: f64,
        damping: f64,
        mass: f64,
    },
}

impl Easing {
    /// NEON DE signature smooth deceleration curve (crisp start, fluid settle).
    pub const NEON_STANDARD: Self = Self::CubicBezier {
        x1: 0.2,
        y1: 0.0,
        x2: 0.0,
        y2: 1.0,
    };

    /// Crisp spring tuned for window open/focus micro-interactions.
    pub const NEON_SPRING: Self = Self::Spring {
        stiffness: 260.0,
        damping: 26.0,
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
    // Solve B_x(u) = x via Newton-Raphson with safe clamping.
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
    // Map normalized t in [0, 1] across a physical settle horizon of 0.8s.
    let tau = t * 0.8;
    let omega0 = (k / m).sqrt();
    let zeta = c / (2.0 * (k * m).sqrt());

    let displacement = if zeta < 1.0 {
        let omega_d = omega0 * (1.0 - zeta * zeta).sqrt();
        (-zeta * omega0 * tau).exp()
            * ((omega_d * tau).cos() + (zeta * omega0 / omega_d) * (omega_d * tau).sin())
    } else if (zeta - 1.0).abs() < 1e-4 {
        (-omega0 * tau).exp() * (1.0 + omega0 * tau)
    } else {
        let r1 = -omega0 * (zeta - (zeta * zeta - 1.0).sqrt());
        let r2 = -omega0 * (zeta + (zeta * zeta - 1.0).sqrt());
        (r2 * (r1 * tau).exp() - r1 * (r2 * tau).exp()) / (r2 - r1)
    };
    1.0 - displacement
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
    pub focus_progress: f32,
}

impl Default for WindowVisualState {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            focus_progress: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct WindowAnimations {
    opacity: Option<Animation>,
    offset_x: Option<Animation>,
    offset_y: Option<Animation>,
    focus_progress: Option<Animation>,
    last_focus_value: f32,
}

/// Global effects configuration (supports accessibility reduced motion and speed scaling).
#[derive(Debug, Clone, Copy)]
pub struct EffectsConfig {
    pub enabled: bool,
    pub reduced_motion: bool,
    pub speed_scale: f64,
    pub shadows_enabled: bool,
    pub borders_enabled: bool,
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
            corner_radius: 12.0,
        }
    }
}

/// Coordinates all frame-driven compositor animations (window open, focus, restore, workspace slide).
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

    fn scaled_duration(&self, base_ms: u64) -> Duration {
        if !self.config.enabled || self.config.reduced_motion {
            return Duration::ZERO;
        }
        let speed = self.config.speed_scale.clamp(0.1, 5.0);
        Duration::from_secs_f64((base_ms as f64 / 1000.0) / speed)
    }

    /// Starts a smooth fade-in + subtle upward glide when a window maps or restores.
    pub fn animate_window_open(&mut self, id: WindowId, now: Duration) {
        let dur = self.scaled_duration(180);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            entry.opacity = None;
            entry.offset_y = None;
            return;
        }
        entry.opacity = Some(Animation::new(0.0, 1.0, now, dur, Easing::EaseOut));
        entry.offset_y = Some(Animation::new(16.0, 0.0, now, dur, Easing::NEON_STANDARD));
    }

    /// Smoothly transitions focus highlight (`0.0` unfocused <-> `1.0` focused).
    pub fn animate_focus(&mut self, id: WindowId, focused: bool, now: Duration) {
        let target = if focused { 1.0 } else { 0.0 };
        let dur = self.scaled_duration(140);
        let entry = self.window_anims.entry(id).or_default();
        if dur.is_zero() {
            entry.focus_progress = None;
            entry.last_focus_value = target as f32;
            return;
        }
        if let Some(anim) = entry.focus_progress.as_mut() {
            anim.retarget(target, now, dur, Easing::EaseOut);
        } else {
            entry.focus_progress = Some(Animation::new(
                f64::from(entry.last_focus_value),
                target,
                now,
                dur,
                Easing::EaseOut,
            ));
        }
    }

    /// Starts a horizontal workspace transition slide (`direction`: `+1` moving right, `-1` moving left).
    pub fn animate_workspace_switch(&mut self, direction: i32, now: Duration) {
        let dur = self.scaled_duration(220);
        if dur.is_zero() {
            self.workspace_offset_x = None;
            return;
        }
        let start_offset = if direction >= 0 { 180.0 } else { -180.0 };
        self.workspace_offset_x = Some(Animation::new(
            start_offset,
            0.0,
            now,
            dur,
            Easing::NEON_STANDARD,
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
            if let Some(a) = &entry.opacity {
                if a.is_finished(now) {
                    entry.opacity = None;
                } else {
                    active = true;
                }
            }
            if let Some(a) = &entry.offset_x {
                if a.is_finished(now) {
                    entry.offset_x = None;
                } else {
                    active = true;
                }
            }
            if let Some(a) = &entry.offset_y {
                if a.is_finished(now) {
                    entry.offset_y = None;
                } else {
                    active = true;
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
            entry
                .opacity
                .as_ref()
                .map(|a| !a.is_finished(now))
                .unwrap_or(false)
                || entry
                    .offset_x
                    .as_ref()
                    .map(|a| !a.is_finished(now))
                    .unwrap_or(false)
                || entry
                    .offset_y
                    .as_ref()
                    .map(|a| !a.is_finished(now))
                    .unwrap_or(false)
                || entry
                    .focus_progress
                    .as_ref()
                    .map(|a| !a.is_finished(now))
                    .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easings_satisfy_endpoints_and_monotonic_progress() {
        let curves = [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::NEON_STANDARD,
            Easing::NEON_SPRING,
        ];
        for curve in curves {
            assert_eq!(curve.evaluate(0.0), 0.0);
            assert_eq!(curve.evaluate(1.0), 1.0);
            let mid = curve.evaluate(0.5);
            assert!(mid > 0.0 && mid < 1.25, "mid={mid} for {curve:?}");
        }
    }

    #[test]
    fn animation_retarget_is_continuous_without_jump() {
        let t0 = Duration::from_millis(0);
        let mut anim = Animation::new(0.0, 100.0, t0, Duration::from_millis(200), Easing::Linear);

        let t_mid = Duration::from_millis(100);
        let mid_before = anim.sample(t_mid);
        assert!((mid_before - 50.0).abs() < 1e-6);

        // Retarget backwards to 0.0 at t=100ms
        anim.retarget(0.0, t_mid, Duration::from_millis(100), Easing::Linear);
        let mid_after = anim.sample(t_mid);
        assert!((mid_after - 50.0).abs() < 1e-6, "retarget must not jump");
        assert!((anim.sample(Duration::from_millis(150)) - 25.0).abs() < 1e-6);
        assert!(anim.is_finished(Duration::from_millis(200)));
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
    }
}
