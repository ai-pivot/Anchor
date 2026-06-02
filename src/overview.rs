//! Overview state machine — Task Panel + Bird's Eye View.
//!
//! Uses Instant + ease-based animation (same pattern as LayoutAnimation/WsAnimation),
//! guaranteeing deterministic, smooth, zero-jitter animation.

/// Overview operating modes.
#[derive(Debug, Clone)]
pub enum OverviewState {
    /// No overlay visible.
    Inactive,
    /// Task Panel — bottom drawer with window thumbnails for current workspace.
    TaskPanel {
        /// Animation start time
        start: std::time::Instant,
        /// Direction: true = opening, false = closing
        opening: bool,
        /// Duration in ms
        duration_ms: u64,
    },
    /// Bird's Eye View — all workspaces as thumbnails in a grid.
    Overview {
        /// Animation start time
        start: std::time::Instant,
        /// Direction: true = opening, false = closing
        opening: bool,
        /// Duration in ms
        duration_ms: u64,
        /// Currently hovered/selected workspace (if any)
        hover_ws: Option<usize>,
    },
}

impl Default for OverviewState {
    fn default() -> Self {
        Self::Inactive
    }
}

impl OverviewState {
    /// Whether any overview overlay is active.
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Inactive)
    }

    /// Get current animation progress (0.0 if inactive).
    /// 0.0 = hidden/closed, 1.0 = fully visible.
    pub fn progress(&self) -> f64 {
        match self {
            Self::Inactive => 0.0,
            Self::TaskPanel { start, opening, duration_ms, .. }
            | Self::Overview { start, opening, duration_ms, .. } => {
                let elapsed = start.elapsed().as_millis() as u64;
                let t = (elapsed as f32 / *duration_ms as f32).min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3); // ease_out_cubic — same as WsAnimation
                if *opening {
                    eased as f64
                } else {
                    1.0 - eased as f64 // reverse for closing
                }
            }
        }
    }

    /// Whether we're in task panel mode.
    pub fn is_task_panel(&self) -> bool {
        matches!(self, Self::TaskPanel { .. })
    }

    /// Whether we're in overview mode.
    pub fn is_overview(&self) -> bool {
        matches!(self, Self::Overview { .. })
    }

    /// Open task panel.
    pub fn open_task_panel(&mut self) {
        *self = Self::TaskPanel {
            start: std::time::Instant::now(),
            opening: true,
            duration_ms: 250, // slightly faster than ws switch for snappy feel
        };
    }

    /// Open overview (bird's eye view).
    pub fn open_overview(&mut self) {
        *self = Self::Overview {
            start: std::time::Instant::now(),
            opening: true,
            duration_ms: 300,
            hover_ws: None, // 首次方向键按下时初始化
        };
    }

    /// Close overview/task panel.
    pub fn close(&mut self) {
        match self {
            Self::Inactive => return,
            Self::TaskPanel { .. } => {
                *self = Self::TaskPanel {
                    start: std::time::Instant::now(),
                    opening: false,
                    duration_ms: 200, // faster close
                };
            }
            Self::Overview { .. } => {
                let hover_ws = self.hover_ws();
                *self = Self::Overview {
                    start: std::time::Instant::now(),
                    opening: false,
                    duration_ms: 250,
                    hover_ws,
                };
            }
        }
    }

    /// Get current hover_ws (for overview mode).
    pub fn hover_ws(&self) -> Option<usize> {
        match self {
            Self::Overview { hover_ws, .. } => *hover_ws,
            _ => None,
        }
    }

    /// Set hover_ws (for overview mode).
    pub fn set_hover_ws(&mut self, ws: Option<usize>) {
        match self {
            Self::Overview { hover_ws, .. } => *hover_ws = ws,
            _ => {}
        }
    }

    /// Step animation. Returns true if animation is still running.
    pub fn update_progress(&mut self, _dt: f64) -> bool {
        match self {
            Self::Inactive => false,
            Self::TaskPanel { start, opening, duration_ms, .. }
            | Self::Overview { start, opening, duration_ms, .. } => {
                let elapsed = start.elapsed().as_millis() as u64;
                if elapsed >= *duration_ms {
                    if *opening {
                        // 打开动画完成 — 保持打开状态，等用户手动关闭
                        false // 动画结束但状态保持活跃
                    } else {
                        // 关闭动画完成 — 切回 Inactive
                        *self = Self::Inactive;
                        false
                    }
                } else {
                    true // 动画进行中
                }
            }
        }
    }
}
