//! Physics engine for Anchor — spring-damper system and momentum scrolling.
//!
//! Frame-rate independent physics simulation used by infinite workspace scrolling,
//! task panel animations, overview transitions, and scratchpad/launcher pop-ups.

/// Spring-damper system for smooth, physically-plausible animations.
///
/// Uses the second-order differential equation:
///   x'' = -k·(x - target) - c·x'
///
/// Where `k` is stiffness and `c` is damping. The system is solved analytically
/// per-frame for frame-rate independence.
///
/// # Parameters
/// - `stiffness` (k): Higher = faster snap. Typical: 200-400
/// - `damping` (c): Higher = less oscillation. Critical damping = 2·√k
///
/// # Example
/// ```ignore
/// let mut spring = Spring::new(300.0, 30.0); // ζ ≈ 0.866 (underdamped, slight overshoot)
/// spring.target = 2.0;
/// let x = spring.update(dt); // animate toward 2.0
/// ```
#[derive(Debug, Clone)]
pub struct Spring {
    /// Current position (displacement from equilibrium).
    pub x: f64,
    /// Current velocity.
    pub v: f64,
    /// Target position (equilibrium point).
    pub target: f64,
    /// Spring stiffness (k).
    pub stiffness: f64,
    /// Damping coefficient (c).
    pub damping: f64,
}

impl Spring {
    pub fn new(stiffness: f64, damping: f64) -> Self {
        Self {
            x: 0.0,
            v: 0.0,
            target: 0.0,
            stiffness,
            damping,
        }
    }

    /// Create spring from damping ratio (ζ) and natural frequency (ω₀).
    /// ζ = 1.0 = critically damped (fastest no-overshoot)
    /// ζ < 1.0 = underdamped (slight overshoot, more "bouncy")
    /// ζ > 1.0 = overdamped (slow, no overshoot)
    pub fn from_damping_ratio(omega0: f64, zeta: f64) -> Self {
        Self {
            x: 0.0,
            v: 0.0,
            target: 0.0,
            stiffness: omega0 * omega0,
            damping: 2.0 * zeta * omega0,
        }
    }

    /// Set position and target instantly (no animation).
    pub fn set(&mut self, value: f64) {
        self.x = value;
        self.v = 0.0;
        self.target = value;
    }

    /// Set target only — spring will animate toward it.
    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    /// Apply an instantaneous velocity impulse.
    pub fn impulse(&mut self, velocity: f64) {
        self.v += velocity;
    }

    /// Step the simulation forward by `dt` seconds.
    /// Returns the new position.
    ///
    /// Uses semi-implicit Euler integration (symplectic):
    ///   v += (-k·(x - target) - c·v) · dt
    ///   x += v · dt
    ///
    /// This is stable for typical timesteps and preserves energy well.
    pub fn update(&mut self, dt: f64) -> f64 {
        let force = -self.stiffness * (self.x - self.target) - self.damping * self.v;
        self.v += force * dt;
        self.x += self.v * dt;
        self.x
    }

    /// Whether the spring has settled (velocity and displacement below threshold).
    pub fn is_settled(&self, threshold: f64) -> bool {
        self.v.abs() < threshold && (self.x - self.target).abs() < threshold
    }

    /// Natural frequency ω₀ = √(k/m), assuming unit mass.
    pub fn omega0(&self) -> f64 {
        self.stiffness.sqrt()
    }

    /// Damping ratio ζ = c / (2·√(k·m)), assuming unit mass.
    pub fn zeta(&self) -> f64 {
        if self.stiffness > 0.0 {
            self.damping / (2.0 * self.stiffness.sqrt())
        } else {
            1.0
        }
    }
}

/// Momentum-based scrolling with friction decay.
///
/// Accumulates velocity from discrete impulse events (e.g., touchpad swipe deltas),
/// then decays velocity over time. When velocity drops below threshold,
/// the spring takes over for smooth snap-to-grid.
///
/// # Example
/// ```ignore
/// let mut mom = Momentum::new(0.92);
/// mom.apply_delta(delta_x); // from touchpad swipe
/// let offset = mom.update(dt);
/// ```
#[derive(Debug, Clone)]
pub struct Momentum {
    /// Accumulated velocity.
    pub velocity: f64,
    /// Friction per frame at 60fps. 0.92 = 92% velocity retained each frame.
    pub friction: f64,
}

impl Momentum {
    pub fn new(friction: f64) -> Self {
        Self {
            velocity: 0.0,
            friction,
        }
    }

    /// Reset velocity to zero.
    pub fn reset(&mut self) {
        self.velocity = 0.0;
    }

    /// Apply an instantaneous position delta (e.g., from touchpad swipe).
    /// Converts to velocity for physics simulation.
    pub fn apply_delta(&mut self, delta: f64, dt: f64) {
        if dt > 0.0 {
            self.velocity += delta / dt * 0.1; // scale factor for natural feel
        }
    }

    /// Step the simulation. Returns velocity after decay.
    pub fn update(&mut self, dt: f64) -> f64 {
        // Frame-rate independent friction: friction^(dt * 60)
        let decay = self.friction.powf(dt * 60.0);
        self.velocity *= decay;
        self.velocity
    }

    /// Whether velocity has effectively stopped.
    pub fn is_stopped(&self, threshold: f64) -> bool {
        self.velocity.abs() < threshold
    }
}

/// Smoothly interpolate a value from `current` toward `target` using lerp.
pub fn lerp(current: f64, target: f64, t: f64) -> f64 {
    current + (target - current) * t.clamp(0.0, 1.0)
}

/// Snap a continuous value to the nearest integer (workspace index).
pub fn snap_to_nearest(current: f64) -> f64 {
    current.round()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spring_basic() {
        let mut spring = Spring::new(300.0, 30.0);
        spring.set_target(1.0);
        // After some time steps, should approach 1.0
        for _ in 0..600 {
            spring.update(1.0 / 60.0);
        }
        assert!((spring.x - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_spring_settled() {
        let mut spring = Spring::new(300.0, 30.0);
        spring.set_target(1.0);
        for _ in 0..300 {
            spring.update(1.0 / 60.0);
        }
        assert!(spring.is_settled(0.01));
    }

    #[test]
    fn test_momentum_decay() {
        let mut mom = Momentum::new(0.92);
        mom.velocity = 100.0;
        for _ in 0..120 {
            mom.update(1.0 / 60.0);
        }
        // After 2 seconds, velocity should be very small
        assert!(mom.velocity.abs() < 1.0);
    }

    #[test]
    fn test_snap() {
        assert_eq!(snap_to_nearest(2.3), 2.0);
        assert_eq!(snap_to_nearest(2.7), 3.0);
        assert_eq!(snap_to_nearest(-0.3), 0.0);
    }
}
