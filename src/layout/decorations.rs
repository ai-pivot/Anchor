//! 窗口背景填充 + 窗口装饰（聚焦/失焦边框 + 窗口编号标签）

use super::geom::{slot, LayoutPreset, SplitDir};
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
    render_window_bg_anim(f, cfg, n, ow, oh, bar_h, layout, split, 0, 0);
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
    render_window_decorations_anim(f, cfg, i, n, focus_idx, ow, oh, bar_h, layout, split, 0, 0);
}

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
) {
    if n == 0 {
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

        // ── 外层发光（4 层递减亮度）──
        for (expand, brightness) in [(4, 0.03f32), (3, 0.06), (2, 0.12), (1, 0.22)] {
            let glow = opaque(accent.0 * brightness, accent.1 * brightness, accent.2 * brightness);
            f.clear(glow, &[rect(x - bw - expand, y - bw - expand, w + 2 * (bw + expand), expand)]).ok();
            f.clear(glow, &[rect(x - bw - expand, y + h + bw, w + 2 * (bw + expand), expand)]).ok();
            f.clear(glow, &[rect(x - bw - expand, y - bw, expand, h + 2 * bw)]).ok();
            f.clear(glow, &[rect(x + w + bw, y - bw, expand, h + 2 * bw)]).ok();
        }

        // ── 主边框 ──
        f.clear(border, &[rect(x - bw, y - bw, w + 2 * bw, bw)]).ok();
        f.clear(border, &[rect(x - bw, y + h, w + 2 * bw, bw)]).ok();
        f.clear(border, &[rect(x - bw, y, bw, h)]).ok();
        f.clear(border, &[rect(x + w, y, bw, h)]).ok();

        // ── 顶部高亮线 ──
        f.clear(bright, &[rect(x - bw, y - bw, w + 2 * bw, 2)]).ok();
        // ── 底部暗线 ──
        f.clear(dark, &[rect(x - bw, y + h + bw - 3, w + 2 * bw, 3)]).ok();
        // ── 左上角发光点 ──
        f.clear(bright, &[rect(x - bw - 1, y - bw - 1, 4, 4)]).ok();
        // ── 右上角发光点 ──
        f.clear(bright, &[rect(x + w + bw - 3, y - bw - 1, 4, 4)]).ok();

        // 窗口编号（暗底 + 亮字）
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
    } else {
        let unfocus = parse_color(&cfg.colors.unfocus_border);
        let border = opaque(unfocus.0, unfocus.1, unfocus.2);
        // 微弱发光
        let glow = opaque(unfocus.0 * 0.15, unfocus.1 * 0.15, unfocus.2 * 0.15);
        f.clear(glow, &[rect(x - 1, y - 1, w + 2, 1)]).ok();
        f.clear(glow, &[rect(x - 1, y + h, w + 2, 1)]).ok();
        f.clear(glow, &[rect(x - 1, y, 1, h)]).ok();
        f.clear(glow, &[rect(x + w, y, 1, h)]).ok();
        // 主边框
        f.clear(border, &[rect(x, y, w, bw)]).ok();
        f.clear(border, &[rect(x, y + h - bw, w, bw)]).ok();
        f.clear(border, &[rect(x, y, bw, h)]).ok();
        f.clear(border, &[rect(x + w - bw, y, bw, h)]).ok();
    }
}
