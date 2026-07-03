//! Anchor 可视化配置界面 — Settings Panel
//!
//! 合成器内覆盖层（与 Launcher / Lock Screen / Overview 同构）。
//! 使用 Frame::clear + text_render 原语绘制，零外部 GUI 依赖。
//!
//! ## 入口
//! - Super + ,  →  打开
//! - Esc         →  关闭

pub mod widgets;
pub mod render;

use crate::config::Config;
use std::time::Instant;

// ═══════════════════════════════════════════════════════════════════
// 配置标签页
// ═══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Colors,
    Layout,
    Bar,
    Wallpaper,
    Keys,
    Input,
    Displays,
    Gpu,
    Rules,
}

impl SettingsTab {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Colors => "Colors",
            Self::Layout => "Layout",
            Self::Bar => "Top Bar",
            Self::Wallpaper => "Wallpaper",
            Self::Keys => "Keybindings",
            Self::Input => "Input",
            Self::Displays => "Displays",
            Self::Gpu => "GPU",
            Self::Rules => "Window Rules",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Colors => "*",
            Self::Layout => "+",
            Self::Bar => "-",
            Self::Wallpaper => "~",
            Self::Keys => "@",
            Self::Input => ":",
            Self::Displays => "=",
            Self::Gpu => ">",
            Self::Rules => "#",
        }
    }

    pub fn all() -> &'static [SettingsTab] {
        &[
            Self::Colors,
            Self::Layout,
            Self::Bar,
            Self::Wallpaper,
            Self::Keys,
            Self::Input,
            Self::Displays,
            Self::Gpu,
            Self::Rules,
        ]
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }

    pub fn prev(self) -> Self {
        let all = Self::all();
        let idx = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(idx + all.len() - 1) % all.len()]
    }
}

// ═══════════════════════════════════════════════════════════════════
// 编辑态：保存配置修改（尚未 Apply）
// ═══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SettingsEdit {
    /// 当前编辑的配置（从 Config 克隆而来，修改在此之上）
    pub cfg: Config,
    /// 当前聚焦的控件索引（每个 tab 独立）
    pub focus_idx: usize,
    /// 是否有未保存的修改
    pub dirty: bool,
    /// Color page: expanded color swatch (None = none expanded)
    pub color_expanded: Option<usize>,
    /// 键盘快捷键录制模式
    pub recording_key: Option<usize>,
}

impl SettingsEdit {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            cfg: cfg.clone(),
            focus_idx: 0,
            dirty: false,
            color_expanded: None,
            recording_key: None,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// 主状态机
// ═══════════════════════════════════════════════════════════════════

pub enum SettingsState {
    /// 未激活
    Inactive,
    /// 已打开
    Active {
        /// 动画开始时间
        start: Instant,
        /// 当前标签页
        active_tab: SettingsTab,
        /// 内容区垂直滚动偏移（像素）
        scroll: f64,
        /// 编辑状态
        edit: SettingsEdit,
    },
    /// 关闭动画进行中（done=true 时等待最后一帧渲染完成后变成 Inactive）
    Closing {
        /// 关闭开始时间
        start: Instant,
        /// 关闭前的活跃标签（用于过渡渲染）
        prev_tab: SettingsTab,
        /// 关闭前的编辑状态
        prev_edit: SettingsEdit,
        /// 动画已完成，等待最后一帧渲染清除
        done: bool,
    },
    /// 保存反馈动画
    Saving {
        /// 保存开始时间
        start: Instant,
        /// 保存前的状态
        prev: Box<SettingsState>,
    },
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::Inactive
    }
}

impl SettingsState {
    /// 打开设置面板
    pub fn open(&mut self, cfg: &Config) {
        *self = Self::Active {
            start: Instant::now(),
            active_tab: SettingsTab::Colors,
            scroll: 0.0,
            edit: SettingsEdit::from_config(cfg),
        };
    }

    /// 开始关闭动画
    pub fn close(&mut self) {
        match self {
            Self::Active {
                active_tab,
                edit,
                ..
            } => {
                *self = Self::Closing {
                    start: Instant::now(),
                    prev_tab: *active_tab,
                    prev_edit: edit.clone(),
                    done: false,
                };
            }
            Self::Saving { prev, .. } => {
                // 如果在保存动画中关闭，直接 inactive
                *self = Self::Inactive;
            }
            _ => *self = Self::Inactive,
        }
    }

    /// 更新关闭动画状态（每帧调用）
    /// 两阶段：动画播放(done=false) → 清除帧(done=true) → Inactive
    pub fn update_close(&mut self) -> bool {
        if let Self::Closing { start, done, .. } = self {
            let over = start.elapsed().as_millis() >= 200;
            if over && *done {
                // 清除帧已过，安全进入 Inactive
                *self = Self::Inactive;
                return false;
            }
            if over && !*done {
                // 动画完成，标记 done 但保持 Closing 一帧
                // 下一帧 progress()=0 → 渲染 early return → 画面自动清除
                *done = true;
                return true;
            }
            return true;
        }
        false
    }

    /// 更新保存动画状态
    pub fn update_saving(&mut self) -> bool {
        if let Self::Saving { start, prev } = self {
            if start.elapsed().as_millis() >= 400 {
                // 保存完成，恢复之前的状态（或进入 closing）
                *self = std::mem::replace(prev.as_mut(), Self::Inactive);
                return false;
            }
            return true;
        }
        false
    }

    /// 面板是否在渲染（含打开/关闭/保存动画）
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Inactive)
    }

    /// 是否需要持续请求渲染（动画进行中）
    pub fn is_animating(&self) -> bool {
        match self {
            Self::Active { start, .. } => {
                start.elapsed().as_millis() < 280
            }
            Self::Closing { done, .. } => {
                // done=true 时仍需一帧释放，下一帧 update_close 会切到 Inactive
                if *done {
                    true
                } else {
                    true
                }
            }
            Self::Saving { .. } => true,
            Self::Inactive => false,
        }
    }

    /// 获取动画进度 0.0 → 1.0（用于面板缩放/透明度）
    pub fn progress(&self) -> f64 {
        match self {
            Self::Inactive => 0.0,
            Self::Active { start, .. } => {
                let t = start.elapsed().as_millis() as f64 / 280.0;
                let t = t.min(1.0);
                1.0 - (1.0 - t).powi(3)
            }
            Self::Closing { start, done, .. } => {
                if *done {
                    return 0.0; // 清除帧：progress=0 → 渲染 early return
                }
                let t = start.elapsed().as_millis() as f64 / 200.0;
                let t = t.min(1.0);
                1.0 - t.powi(3)
            }
            Self::Saving { start, .. } => {
                let t = start.elapsed().as_millis() as f64 / 400.0;
                t.min(1.0)
            }
        }
    }

    /// 获取当前标签页（供渲染使用）
    pub fn tab(&self) -> SettingsTab {
        match self {
            Self::Active { active_tab, .. } => *active_tab,
            Self::Closing { prev_tab, .. } => *prev_tab,
            _ => SettingsTab::Colors,
        }
    }

    /// 获取编辑状态的引用
    pub fn edit(&self) -> Option<&SettingsEdit> {
        match self {
            Self::Active { edit, .. } => Some(edit),
            Self::Closing { prev_edit, .. } => Some(prev_edit),
            _ => None,
        }
    }

    /// 获取编辑状态的可变引用
    pub fn edit_mut(&mut self) -> Option<&mut SettingsEdit> {
        match self {
            Self::Active { edit, .. } => Some(edit),
            _ => None,
        }
    }

    /// 切换标签页
    pub fn switch_tab(&mut self, tab: SettingsTab) {
        if let Self::Active { active_tab, scroll, .. } = self {
            *active_tab = tab;
            *scroll = 0.0;
        }
    }

    /// 应用配置：写入 config.toml
    pub fn apply(&mut self) -> Result<(), String> {
        if let Self::Active { edit, .. } = self {
            let toml_str = toml::to_string_pretty(&edit.cfg)
                .map_err(|e| format!("TOML serialize: {}", e))?;
            let path = crate::config::config_path();
            std::fs::write(&path, &toml_str)
                .map_err(|e| format!("write {}: {}", path.display(), e))?;
            edit.dirty = false;

            // 进入保存反馈动画
            let prev = Box::new(std::mem::replace(self, Self::Inactive));
            *self = Self::Saving {
                start: Instant::now(),
                prev,
            };
            Ok(())
        } else {
            Err("not active".into())
        }
    }

    /// 重置编辑状态（放弃未保存的修改）
    pub fn reset(&mut self, cfg: &Config) {
        if let Self::Active { edit, .. } = self {
            *edit = SettingsEdit::from_config(cfg);
        }
    }

    /// 切换聚焦控件（向上）
    pub fn prev_focus(&mut self) {
        if let Self::Active { active_tab, edit, .. } = self {
            let max = controls_count(*active_tab);
            if max > 0 {
                if edit.focus_idx == 0 {
                    edit.focus_idx = max - 1;
                } else {
                    edit.focus_idx -= 1;
                }
            }
        }
    }

    /// 切换聚焦控件（向下）
    pub fn next_focus(&mut self) {
        if let Self::Active { active_tab, edit, .. } = self {
            let max = controls_count(*active_tab);
            if max > 0 {
                edit.focus_idx = (edit.focus_idx + 1) % max;
            }
        }
    }

    /// 调整当前聚焦控件的值（← →）
    pub fn adjust_focus(&mut self, delta: f64) {
        if let Self::Active {
            active_tab,
            edit,
            ..
        } = self
        {
            let fi = edit.focus_idx;
            match active_tab {
                SettingsTab::Layout => {
                    let val = match fi {
                        0 => &mut edit.cfg.layout.border_width,
                        1 => &mut edit.cfg.layout.gap,
                        2 => &mut edit.cfg.layout.margin,
                        _ => return,
                    };
                    *val = (*val + delta as i32).clamp(0, 64);
                    edit.dirty = true;
                }
                SettingsTab::Bar => match fi {
                    0 => {
                        // Toggle: flip
                        edit.cfg.bar.enabled = !edit.cfg.bar.enabled;
                        edit.dirty = true;
                    }
                    1 => {
                        edit.cfg.bar.height =
                            (edit.cfg.bar.height + delta as i32).clamp(12, 80);
                        edit.dirty = true;
                    }
                    2 => {
                        let v = (edit.cfg.bar.opacity as f64 + delta * 0.05)
                            .clamp(0.1, 1.0);
                        edit.cfg.bar.opacity = v as f32;
                        edit.dirty = true;
                    }
                    3 => {
                        edit.cfg.bar.show_date = !edit.cfg.bar.show_date;
                        edit.dirty = true;
                    }
                    4 => {
                        edit.cfg.bar.show_cpu = !edit.cfg.bar.show_cpu;
                        edit.dirty = true;
                    }
                    5 => {
                        edit.cfg.bar.show_memory = !edit.cfg.bar.show_memory;
                        edit.dirty = true;
                    }
                    _ => {}
                },
                // Colors / Wallpaper: ← → 不做值调整（色块等需要 Enter 展开调色板）
                _ => {}
            }
        }
    }

    /// 激活当前聚焦控件（Enter）
    pub fn activate_focus(&mut self) {
        if let Self::Active {
            active_tab,
            edit,
            ..
        } = self
        {
            let fi = edit.focus_idx;
            match active_tab {
                SettingsTab::Bar => match fi {
                    0 => {
                        edit.cfg.bar.enabled = !edit.cfg.bar.enabled;
                        edit.dirty = true;
                    }
                    3 => {
                        edit.cfg.bar.show_date = !edit.cfg.bar.show_date;
                        edit.dirty = true;
                    }
                    4 => {
                        edit.cfg.bar.show_cpu = !edit.cfg.bar.show_cpu;
                        edit.dirty = true;
                    }
                    5 => {
                        edit.cfg.bar.show_memory = !edit.cfg.bar.show_memory;
                        edit.dirty = true;
                    }
                    _ => {}
                },
                SettingsTab::Wallpaper => match fi {
                    0..=3 => {
                        let modes = ["color", "image", "random", "gradient"];
                        edit.cfg.wallpaper.mode = modes[fi].to_string();
                        edit.dirty = true;
                    }
                    4..=7 => {
                        let scalings = ["fill", "fit", "stretch", "center"];
                        edit.cfg.wallpaper.scaling = scalings[fi - 4].to_string();
                        edit.dirty = true;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

/// 每个标签页的可聚焦控件数量
fn controls_count(tab: SettingsTab) -> usize {
    match tab {
        SettingsTab::Colors => 7,   // 3 core + 4 bar swatches
        SettingsTab::Layout => 3,   // border_width, gap, margin
        SettingsTab::Bar => 6,      // enabled, height, opacity, show_date, show_cpu, show_memory
        SettingsTab::Wallpaper => 8, // 4 mode + 4 scaling radios
        _ => 3, // placeholder for unimplemented tabs
    }
}
