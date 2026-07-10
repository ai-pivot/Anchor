//! Anchor 布局模块（拆分版）
//!
//! 将原 `src/layout.rs` 拆分为 7 个职责清晰的子模块，对外仍通过
//! `crate::layout::xxx` 这一路径提供所有公开 API，**调用方无需修改**。
//!
//! 子模块：
//! - `util`         — 工具函数 (`opaque`, `color_hex`, `rect`) + 排版/字号常量
//! - `geom`         — 布局几何：枚举 `LayoutPreset` / `SplitDir` + `slot()`
//! - `wallpaper`    — 壁纸渲染（背景 + 网格 + 光斑）
//! - `decorations`  — 窗口背景 + 装饰边框
//! - `headbar`      — 顶栏（logo + 工作区 + 时钟）
//! - `notifications`— 通知弹窗 overlay
//! - `launcher`     — 启动器（dmenu 风格）
//! - `lock_screen`  — 锁屏 UI（5 种背景风格）

pub mod decorations;
pub mod geom;
pub mod headbar;
pub mod launcher;
pub mod lock_screen;
pub mod notifications;
pub mod overview;
pub mod util;
pub mod wallpaper;

// 重新导出：让 `crate::layout::slot()`、`crate::layout::LayoutPreset` 等
// 写法与原 `layout.rs` 完全一致，调用方（main.rs、workspace.rs 等）零修改。

pub use decorations::*;
pub use geom::*;
pub use headbar::*;
pub use launcher::*;
pub use lock_screen::*;
pub use notifications::*;
pub use overview::*;
pub use util::*;
pub use wallpaper::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    fn test_cfg() -> Config {
        Config::default()
    }

    #[test]
    fn test_slot_one() {
        let cfg = test_cfg();
        let (_x, y, w, h) = slot(
            0,
            1,
            2560,
            1440,
            42,
            &cfg,
            LayoutPreset::MasterStack,
            SplitDir::Horizontal,
        );
        assert!(y >= 42 && w > 0 && h > 0);
    }
    #[test]
    fn test_slot_two() {
        let cfg = test_cfg();
        let a = slot(
            0,
            2,
            2560,
            1440,
            42,
            &cfg,
            LayoutPreset::MasterStack,
            SplitDir::Horizontal,
        );
        let b = slot(
            1,
            2,
            2560,
            1440,
            42,
            &cfg,
            LayoutPreset::MasterStack,
            SplitDir::Horizontal,
        );
        assert!(a.0 + a.2 <= b.0);
    }
    #[test]
    fn test_no_overlap() {
        let cfg = test_cfg();
        for layout in LayoutPreset::ALL {
            for n in 1..=6usize {
                let mut rects: Vec<(i32, i32, i32, i32)> = vec![];
                for i in 0..n {
                    let r = slot(i, n, 2560, 1440, 42, &cfg, layout, SplitDir::Horizontal);
                    for (j, p) in rects.iter().enumerate() {
                        let overlap = r.0 < p.0 + p.2
                            && r.0 + r.2 > p.0
                            && r.1 < p.1 + p.3
                            && r.1 + r.3 > p.1;
                        assert!(!overlap, "{:?} n={n}: {j} overlaps {}", layout, i);
                    }
                    rects.push(r);
                }
            }
        }
    }
}
