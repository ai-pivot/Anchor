//! 布局几何计算（核心）
//!
//! - `LayoutPreset`: 4 种布局预设（MasterStack / Columns / Center / Grid）
//! - `SplitDir`: 横向 / 纵向 平铺方向
//! - `slot()`: 给定索引、总数、屏幕尺寸、bar 高度、布局预设和平铺方向，
//!            返回窗口槽位 `(x, y, w, h)`

use super::util::rect;
use crate::config::Config;

/// 布局预设
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutPreset {
    MasterStack,
    Columns,
    Center,
    Grid,
}

/// 平铺方向 — 控制新窗口如何分割空间
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    Horizontal, // 横向分割（新窗口在右边/下边）
    Vertical,   // 纵向分割（新窗口在上边/下边）
}

impl LayoutPreset {
    pub const ALL: [LayoutPreset; 4] = [
        LayoutPreset::MasterStack,
        LayoutPreset::Columns,
        LayoutPreset::Center,
        LayoutPreset::Grid,
    ];
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&x| x == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::MasterStack => "Master-Stack",
            Self::Columns => "Columns",
            Self::Center => "Center",
            Self::Grid => "Grid",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "master-stack" | "masterstack" => Some(Self::MasterStack),
            "columns" | "column" => Some(Self::Columns),
            "center" => Some(Self::Center),
            "grid" => Some(Self::Grid),
            _ => None,
        }
    }
}

impl Default for LayoutPreset {
    fn default() -> Self {
        Self::MasterStack
    }
}

/// 计算窗口槽位（向后兼容版本，header_bar_h = 0）
pub fn slot(
    i: usize,
    n: usize,
    ow: i32,
    oh: i32,
    bar_h: i32,
    cfg: &Config,
    layout: LayoutPreset,
    split: SplitDir,
) -> (i32, i32, i32, i32) {
    slot_with_header_bar(i, n, ow, oh, bar_h, cfg, layout, split, 0)
}

/// 计算窗口槽位
///
/// `header_bar_h`: 窗口顶部的 header bar 高度（逻辑像素）
///   - 0 = 不预留空间（默认）
///   - >0 = 窗口内容从 y + header_bar_h 开始，窗口总高度不变
///          客户端在 (x, y) 到 (x+w, y+header_bar_h) 区域内渲染自己的 header bar
pub fn slot_with_header_bar(
    i: usize,
    n: usize,
    ow: i32,
    oh: i32,
    bar_h: i32,
    cfg: &Config,
    layout: LayoutPreset,
    split: SplitDir,
    header_bar_h: i32,
) -> (i32, i32, i32, i32) {
    if n == 0 {
        return (0, bar_h, 0, 0);
    }
    let gap = cfg.layout.gap;
    let margin = cfg.layout.margin;
    let usable_w = ow - 2 * margin;
    let usable_h = oh - bar_h - 2 * margin;

    // 计算 slot 位置（窗口占用的完整区域包括 header bar）
    let (x, y, w, h) = match layout {
        LayoutPreset::MasterStack => match n {
            1 => (margin, bar_h + margin, usable_w, usable_h),
            _ => match split {
                SplitDir::Horizontal => {
                    let master_w = (usable_w - gap) * 2 / 3;
                    if i == 0 {
                        (margin, bar_h + margin, master_w, usable_h)
                    } else {
                        let stack_n = n - 1;
                        let stack_w = usable_w - master_w - gap;
                        let stack_h = (usable_h - gap * (stack_n - 1) as i32) / stack_n as i32;
                        let extra = (usable_h - gap * (stack_n - 1) as i32) % stack_n as i32;
                        let si = i - 1;
                        let sy =
                            bar_h + margin + si as i32 * (stack_h + gap) + extra.min(si as i32);
                        let sh = stack_h + if si < extra as usize { 1 } else { 0 };
                        (margin + master_w + gap, sy, stack_w, sh)
                    }
                }
                SplitDir::Vertical => {
                    let master_h = (usable_h - gap) * 2 / 3;
                    if i == 0 {
                        (margin, bar_h + margin, usable_w, master_h)
                    } else {
                        let stack_n = n - 1;
                        let stack_y = bar_h + margin + master_h + gap;
                        let stack_h = usable_h - master_h - gap;
                        let stack_w = (usable_w - gap * (stack_n - 1) as i32) / stack_n as i32;
                        let extra = (usable_w - gap * (stack_n - 1) as i32) % stack_n as i32;
                        let si = i - 1;
                        let sx = margin + si as i32 * (stack_w + gap) + extra.min(si as i32);
                        let sw = stack_w + if si < extra as usize { 1 } else { 0 };
                        (sx, stack_y, sw, stack_h)
                    }
                }
            },
        },
        LayoutPreset::Columns => match split {
            SplitDir::Horizontal => {
                let col_w = (usable_w - gap * (n as i32 - 1)) / n as i32;
                let extra = (usable_w - gap * (n as i32 - 1)) % n as i32;
                let x = margin + i as i32 * (col_w + gap) + extra.min(i as i32);
                let w = col_w + if i < extra as usize { 1 } else { 0 };
                (x, bar_h + margin, w, usable_h)
            }
            SplitDir::Vertical => {
                let row_h = (usable_h - gap * (n as i32 - 1)) / n as i32;
                let extra = (usable_h - gap * (n as i32 - 1)) % n as i32;
                let y = bar_h + margin + i as i32 * (row_h + gap) + extra.min(i as i32);
                let h = row_h + if i < extra as usize { 1 } else { 0 };
                (margin, y, usable_w, h)
            }
        },
        LayoutPreset::Center => match split {
            SplitDir::Horizontal => {
                let max_w = 1200.min(ow * 7 / 10);
                let cw = (max_w - gap * (n as i32 - 1)) / n as i32;
                let extra = (max_w - gap * (n as i32 - 1)) % n as i32;
                let start_x = ow / 2 - max_w / 2;
                let x = start_x + i as i32 * (cw + gap) + extra.min(i as i32);
                let w = cw + if i < extra as usize { 1 } else { 0 };
                (x, bar_h + margin, w, usable_h)
            }
            SplitDir::Vertical => {
                let max_h = 800.min(oh * 7 / 10);
                let rh = (max_h - gap * (n as i32 - 1)) / n as i32;
                let extra = (max_h - gap * (n as i32 - 1)) % n as i32;
                let total_w = (usable_w).min(n as i32 * (800 / n as i32) + gap * (n as i32 - 1));
                let start_x = ow / 2 - total_w / 2;
                let start_y = (bar_h + oh) / 2 - max_h / 2;
                let cw = (total_w - gap * (n as i32 - 1)) / n as i32;
                let extra_w = (total_w - gap * (n as i32 - 1)) % n as i32;
                let x = start_x + i as i32 * (cw + gap) + extra_w.min(i as i32);
                let w = cw + if i < extra_w as usize { 1 } else { 0 };
                (x, start_y, w, rh + if i < extra as usize { 1 } else { 0 })
            }
        },
        LayoutPreset::Grid => {
            let cols = (n as f32).sqrt().ceil() as i32;
            let rows = (n as i32 + cols - 1) / cols;
            let col = i as i32 % cols;
            let row = i as i32 / cols;
            let items_in_row = if row < rows - 1 {
                cols
            } else {
                n as i32 - row * cols
            };
            let col_w = (usable_w - gap * (cols - 1)) / cols;
            let row_h = (usable_h - gap * (rows - 1)) / rows;
            let row_start = if row == rows - 1 {
                let used_w = items_in_row * col_w + (items_in_row - 1) * gap;
                margin + (usable_w - used_w) / 2
            } else {
                margin
            };
            let x = row_start + col * (col_w + gap);
            let y = bar_h + margin + row * (row_h + gap);
            (x, y, col_w, row_h)
        }
    };

    // 如果有 header bar，窗口内容区高度不变但客户端渲染内容向下偏移
    // slot() 返回的是窗口的完整几何区域（包含 header bar）
    // 客户端通过 configure 获得 (w, h) 的大小
    // header bar 区域在窗口顶部 header_bar_h 像素内
    // 合成器装饰渲染时知道 header_bar_h，不在 header bar 区域绘制自己的标题
    (x, y, w, h)
}
