//! Task Panel state machine — niri-style horizontal workspace strip.
//!
//! Uses Instant + ease-based animation (same pattern as LayoutAnimation/WsAnimation),
//! guaranteeing deterministic, smooth, zero-jitter animation.

/// Task Panel operating modes.
#[derive(Debug, Clone)]
pub enum OverviewState {
    /// No overlay visible.
    Inactive,
    /// Task Panel — niri-style horizontal strip with all workspaces side by side.
    /// Scroll with left/right, auto-snap on close.
    TaskPanel {
        /// Animation start time
        start: std::time::Instant,
        /// Direction: true = opening, false = closing
        opening: bool,
        /// Duration in ms
        duration_ms: u64,
        /// Continuous scroll position (0.0 = ws0 centered, 1.0 = ws1 centered)
        scroll_offset: f64,
        /// Target scroll position for snap animation
        target_offset: f64,
    },
}

impl Default for OverviewState {
    fn default() -> Self {
        Self::Inactive
    }
}

impl OverviewState {
    /// Whether task panel is active.
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Inactive)
    }

    /// Whether we're in task panel mode (kept for compatibility).
    pub fn is_task_panel(&self) -> bool {
        matches!(self, Self::TaskPanel { .. })
    }

    /// Whether we're in overview mode — always false now.
    #[allow(dead_code)]
    pub fn is_overview(&self) -> bool {
        false
    }

    /// Get current animation progress (0.0 if inactive).
    /// 0.0 = hidden/closed, 1.0 = fully visible.
    pub fn progress(&self) -> f64 {
        match self {
            Self::Inactive => 0.0,
            Self::TaskPanel { start, opening, duration_ms, .. } => {
                let elapsed = start.elapsed().as_millis() as u64;
                let t = (elapsed as f32 / *duration_ms as f32).min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3); // ease_out_cubic
                if *opening { eased as f64 } else { 1.0 - eased as f64 }
            }
        }
    }

    /// Open task panel at the current active workspace.
    pub fn open_task_panel(&mut self, current_ws: usize) {
        *self = Self::TaskPanel {
            start: std::time::Instant::now(),
            opening: true,
            duration_ms: 350,
            scroll_offset: current_ws as f64,
            target_offset: current_ws as f64,
        };
    }

    /// Close task panel.
    pub fn close(&mut self) {
        match self {
            Self::Inactive => return,
            Self::TaskPanel { scroll_offset, .. } => {
                let snap = scroll_offset.round();
                *self = Self::TaskPanel {
                    start: std::time::Instant::now(),
                    opening: false,
                    duration_ms: 300,
                    scroll_offset: *scroll_offset,
                    target_offset: snap,
                };
            }
        }
    }

    /// Get the workspace index that the task panel is currently snapped to.
    pub fn task_panel_ws(&self) -> usize {
        match self {
            Self::TaskPanel { scroll_offset, .. } => {
                scroll_offset.round().max(0.0) as usize
            }
            Self::Inactive => 0,
        }
    }

    /// Scroll task panel left/right by one workspace.
    pub fn task_panel_scroll(&mut self, delta: i32, max_ws: usize) {
        if let Self::TaskPanel { target_offset, .. } = self {
            let new_target = (*target_offset + delta as f64)
                .max(0.0)
                .min((max_ws - 1) as f64);
            *target_offset = new_target;
        }
    }

    /// Update task panel snap animation (spring towards target).
    /// Returns true if animation is still running.
    pub fn update_snap(&mut self, dt: f64) -> bool {
        if let Self::TaskPanel { scroll_offset, target_offset, .. } = self {
            let diff = *target_offset - *scroll_offset;
            if diff.abs() < 0.001 {
                *scroll_offset = *target_offset;
                return false;
            }
            *scroll_offset += diff * (1.0 - (-18.0 * dt).exp());
            true
        } else {
            false
        }
    }

    /// Step animation. Returns true if animation is still running.
    pub fn update_progress(&mut self, _dt: f64) -> bool {
        match self {
            Self::Inactive => false,
            Self::TaskPanel { start, opening, duration_ms, .. } => {
                let elapsed = start.elapsed().as_millis() as u64;
                if elapsed >= *duration_ms {
                    if *opening {
                        false
                    } else {
                        *self = Self::Inactive;
                        false
                    }
                } else {
                    true
                }
            }
        }
    }
}
