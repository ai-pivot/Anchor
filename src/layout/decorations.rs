//! 窗口背景填充 + 窗口装饰（聚焦/失焦边框 + 窗口编号标签）
//!
//! 支持 header bar 模式：
//! - 当窗口有 header bar（通过协议或全局配置声明）时，
//!   合成器在边框顶部预留 header bar 区域，不绘制自己的标题/按钮
//! - CSD 窗口完全不绘制装饰

use super::geom::{slot, slot_with_header_bar, LayoutPreset, SplitDir};
use super::util::{color_hex, opaque, rect};
use crate::config::{parse_color, Config};
use crate::text_render;
use smithay::backend::renderer::Frame;

pub fn render_window_bg(
    f: &mut impl Frame,
    cfg: &Config,
    n: usize,
    ow: i32,
    oh: i32,
    bar_h: i32,
    layout: LayoutPreset,
    split: SplitDir,
) {
    render_window_bg_anim(f, cfg, n, ow, oh, bar_h, layout, split, 0, 0, 0.0);
}

pub fn render_window_bg_anim(
    f: &mut impl Frame,
    cfg: &Config,
    n: usize,
    ow: i32,
    oh: i32,
    bar_h: i32,
    layout: LayoutPreset,
    split: SplitDir,
    offset_x: i32,
    offset_y: i32,
    anim_glow: f32,  // 窗口打开/关闭时的额外发光脉冲强度
) {
    if n == 0 {
        return;
    }
    let bw = cfg.layout.border_width;
    let bg = color_hex(&cfg.colors.background);
    for i in 0..n {
        let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg, layout, split);
        f.clear(
            bg,
            &[rect(
                x - bw + offset_x,
                y - bw + offset_y,
                w + 2 * bw,
                h + 2 * bw,
            )],
        )
        .ok();
    }
}

pub fn render_window_decorations(
    f: &mut impl Frame,
    cfg: &Config,
    i: usize,
    n: usize,
    focus_idx: Option<usize>,
    ow: i32,
    oh: i32,
    bar_h: i32,
    layout: LayoutPreset,
    split: SplitDir,
) {
    render_window_decorations_anim(f, cfg, i, n, focus_idx, ow, oh, bar_h, layout, split, 0, 0, 0.0, false, 0);
}

/// 渲染窗口装饰
///
/// `is_csd`: 窗口是否使用客户端装饰（CSD）
/// `header_bar_h`: 窗口的 header bar 高度（0 = 无 header bar）
pub fn render_window_decorations_anim(
    f: &mut impl Frame,
    cfg: &Config,
    i: usize,
    n: usize,
    focus_idx: Option<usize>,
    ow: i32,
    oh: i32,
    bar_h: i32,
    layout: LayoutPreset,
    split: SplitDir,
    offset_x: i32,
    offset_y: i32,
    anim_glow: f32,  // 窗口打开/关闭时的额外发光脉冲强度
    is_csd: bool,    // 客户端是否自己绘制装饰
    header_bar_h: i32, // header bar 高度
) {
    if n == 0 {
        return;
    }

    // CSD 窗口：只渲染最少的边框高亮，不画标题/按钮/发光
    if is_csd {
        render_csd_decorations(f, cfg, i, n, focus_idx, ow, oh, bar_h, layout, split, offset_x, offset_y);
        return;
    }

    let bw = cfg.layout.border_width;
    let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg, layout, split);
    let x = x + offset_x;
    let y = y + offset_y;
    let is_focused = focus_idx == Some(i);

    if is_focused {
        let accent = parse_color(&cfg.colors.focus_border);
        let border = opaque(accent.0, accent.1, accent.2);
        let bright = opaque(
            (accent.0 * 1.6).min(1.0),
            (accent.1 * 1.6).min(1.0),
            (accent.2 * 1.6).min(1.0),
        );
        let dark = opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3);

        // ── 外层发光（4 层递减亮度）── 动画时增强发光
        let glow_boost = 1.0 + anim_glow * 3.0;
        for (expand, brightness) in [(4, 0.03f32), (3, 0.06), (2, 0.12), (1, 0.22)] {
            let glow = opaque(accent.0 * brightness * glow_boost, accent.1 * brightness * glow_boost, accent.2 * brightness * glow_boost);
            f.clear(glow, &[rect(x - bw - expand, y - bw - expand, w + 2 * (bw + expand), expand)]).ok();
            f.clear(glow, &[rect(x - bw - expand, y + h + bw, w + 2 * (bw + expand), expand)]).ok();
            f.clear(glow, &[rect(x - bw - expand, y - bw, expand, h + 2 * bw)]).ok();
            f.clear(glow, &[rect(x + w + bw, y - bw, expand, h + 2 * bw)]).ok();
        }

        // ── 主边框 ──
        f.clear(border, &[rect(x - bw, y - bw, w + 2 * bw, bw)]).ok();
        f.clear(border, &[rect(x - bw, y + h, w + 2 * bw, bw)]).ok();
        // ── 左右边框（从上到下渐变：顶部亮 → 底部暗）──
        let grad_steps = 4;
        let seg_h = h / grad_steps;
        for g in 0..grad_steps {
            let t = g as f32 / (grad_steps - 1).max(1) as f32;
            let br = 1.0 - t * 0.4;
            let grad_color = opaque(accent.0 * br, accent.1 * br, accent.2 * br);
            let sy = y + g * seg_h;
            let sh = if g == grad_steps - 1 { h - g * seg_h } else { seg_h };
            f.clear(grad_color, &[rect(x - bw, sy, bw, sh)]).ok();
            f.clear(grad_color, &[rect(x + w, sy, bw, sh)]).ok();
        }

        // ── 顶部高亮线 ──
        f.clear(bright, &[rect(x - bw, y - bw, w + 2 * bw, 2)]).ok();
        // ── 底部暗线 ──
        f.clear(dark, &[rect(x - bw, y + h + bw - 3, w + 2 * bw, 3)]).ok();
        // ── 左上角发光点 ──
        f.clear(bright, &[rect(x - bw - 1, y - bw - 1, 4, 4)]).ok();
        // ── 右上角发光点 ──
        f.clear(bright, &[rect(x + w + bw - 3, y - bw - 1, 4, 4)]).ok();

        // ── Header bar 分隔线（如果窗口有 header bar）──
        if header_bar_h > 0 {
            // 在 header bar 底部画一条分隔线
            let sep_y = y + header_bar_h;
            f.clear(dark, &[rect(x, sep_y, w, 1)]).ok();
        }

        // ── 窗口控制按钮（macOS 红绿灯风格）──
        // 如果有 header bar，按钮放在 header bar 区域内（客户端自己画）
        // 如果没有 header bar，按钮放在合成器的边框中
        if header_bar_h == 0 {
            let btn_y = y - bw + 2;
            let btn_r = 4;
            let btn_gap = 14;
            // 关闭按钮（红色）
            f.clear(
                opaque(0.8, 0.2, 0.2),
                &[rect(x + w - 12 - bw, btn_y, btn_r, btn_r)],
            ).ok();
            // 最小化按钮（黄色）
            f.clear(
                opaque(0.7, 0.6, 0.15),
                &[rect(x + w - 12 - bw - btn_gap, btn_y, btn_r, btn_r)],
            ).ok();
            // 全屏按钮（绿色）
            f.clear(
                opaque(0.2, 0.7, 0.3),
                &[rect(x + w - 12 - bw - btn_gap * 2, btn_y, btn_r, btn_r)],
            ).ok();
        }

        // 窗口编号（暗底 + 亮字）— 只在没有 header bar 时显示
        if header_bar_h == 0 {
            let label_w = 22;
            f.clear(dark, &[rect(x + 4, y + 4, label_w, 18)]).ok();
            text_render::draw_text(
                f,
                &format!("{}", i + 1),
                x + 8,
                y + 6,
                12.0,
                (accent.0 * 1.2, accent.1 * 1.2, accent.2 * 1.2),
            );
        }
    } else {
        let unfocus = parse_color(&cfg.colors.unfocus_border);
        let border = opaque(unfocus.0, unfocus.1, unfocus.2);
        // 微弱发光 — 动画时增强
        let boost = 1.0 + anim_glow * 2.5;
        let glow = opaque(unfocus.0 * 0.15 * boost, unfocus.1 * 0.15 * boost, unfocus.2 * 0.15 * boost);
        f.clear(glow, &[rect(x - 1, y - 1, w + 2, 1)]).ok();
        f.clear(glow, &[rect(x - 1, y + h, w + 2, 1)]).ok();
        f.clear(glow, &[rect(x - 1, y, 1, h)]).ok();
        f.clear(glow, &[rect(x + w, y, 1, h)]).ok();
        // 主边框
        f.clear(border, &[rect(x, y, w, bw)]).ok();
        f.clear(border, &[rect(x, y + h - bw, w, bw)]).ok();
        f.clear(border, &[rect(x, y, bw, h)]).ok();
        f.clear(border, &[rect(x + w - bw, y, bw, h)]).ok();

        // Header bar 分隔线
        if header_bar_h > 0 {
            let sep_y = y + header_bar_h;
            let dark = opaque(unfocus.0 * 0.2, unfocus.1 * 0.2, unfocus.2 * 0.2);
            f.clear(dark, &[rect(x, sep_y, w, 1)]).ok();
        }
    }
}

/// CSD 窗口的装饰渲染 — 只渲染最小的边框高亮
fn render_csd_decorations(
    f: &mut impl Frame,
    cfg: &Config,
    i: usize,
    n: usize,
    focus_idx: Option<usize>,
    ow: i32,
    oh: i32,
    bar_h: i32,
    layout: LayoutPreset,
    split: SplitDir,
    offset_x: i32,
    offset_y: i32,
) {
    let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg, layout, split);
    let x = x + offset_x;
    let y = y + offset_y;
    let is_focused = focus_idx == Some(i);

    if is_focused {
        // 焦点窗口：微弱的边框高亮线
        let accent = parse_color(&cfg.colors.focus_border);
        let color = opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3);
        f.clear(color, &[rect(x - 1, y - 1, w + 2, 1)]).ok();
        f.clear(color, &[rect(x - 1, y + h, w + 2, 1)]).ok();
        f.clear(color, &[rect(x - 1, y, 1, h)]).ok();
        f.clear(color, &[rect(x + w, y, 1, h)]).ok();
    }
    // CSD 非焦点窗口：不画任何装饰
}
