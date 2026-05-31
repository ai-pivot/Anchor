//! 布局计算 + 壁纸渲染 + Headbar 渲染 + 窗口装饰 v22
//! 7-segment 几何数字渲染 — 不受 NVIDIA block-linear 影响

use crate::config::{parse_color, Config};
use smithay::{
    backend::renderer::{Frame, Color32F},
    utils::{Physical, Point, Rectangle, Size},
};

#[inline(always)]
fn opaque(r: f32, g: f32, b: f32) -> Color32F { Color32F::new(r, g, b, 1.0) }
#[inline(always)]
fn color_hex(hex: &str) -> Color32F {
    let (r, g, b) = parse_color(hex);
    opaque(r, g, b)
}

const S1: i32 = 4;
const S2: i32 = 8;
const S3: i32 = 12;
const S4: i32 = 16;
const S6: i32 = 24;

// ═══════════════════════════════════════════════════════════
// 7-segment 几何数字渲染（每个笔画是粗实心矩形）
// ═══════════════════════════════════════════════════════════

/// 用实心矩形画一个 7-segment 数字
/// 布局:  A=top, B=top-right, C=bottom-right, D=bottom, E=bottom-left, F=top-left, G=middle
fn draw_7seg(f: &mut impl Frame, digit: u8, x: i32, y: i32, w: i32, h: i32, t: i32, color: Color32F) {
    //        A
    //    ┌───────┐
    //  F │       │ B
    //    │   G   │
    //    ├───────┤
    //  E │       │ C
    //    │       │
    //    └───────┘
    //        D
    let segs: [bool; 7] = match digit {
        0 => [true,  true,  true,  true,  true,  true,  false],
        1 => [false, true,  true,  false, false, false, false],
        2 => [true,  true,  false, true,  true,  false, true],
        3 => [true,  true,  true,  true,  false, false, true],
        4 => [false, true,  true,  false, false, true,  true],
        5 => [true,  false, true,  true,  false, true,  true],
        6 => [true,  false, true,  true,  true,  true,  true],
        7 => [true,  true,  true,  false, false, false, false],
        8 => [true,  true,  true,  true,  true,  true,  true],
        9 => [true,  true,  true,  true,  false, true,  true],
        _ => return,
    };
    let hh = (h - t) / 2; // 中线位置（上半部分高度）
    let inner_w = (w - 2 * t).max(t); // 水平笔画宽度
    let v_h = (hh - t).max(t);       // 垂直笔画高度
    let v_h2 = (h - hh - 2 * t).max(t); // 下半垂直笔画高度

    // A: top horizontal
    if segs[0] { f.clear(color, &[rect(x + t, y, inner_w, t)]).ok(); }
    // B: top-right vertical
    if segs[1] { f.clear(color, &[rect(x + w - t, y + t, t, v_h)]).ok(); }
    // C: bottom-right vertical
    if segs[2] { f.clear(color, &[rect(x + w - t, y + hh + t, t, v_h2)]).ok(); }
    // D: bottom horizontal
    if segs[3] { f.clear(color, &[rect(x + t, y + h - t, inner_w, t)]).ok(); }
    // E: bottom-left vertical
    if segs[4] { f.clear(color, &[rect(x, y + hh + t, t, v_h2)]).ok(); }
    // F: top-left vertical
    if segs[5] { f.clear(color, &[rect(x, y + t, t, v_h)]).ok(); }
    // G: middle horizontal
    if segs[6] { f.clear(color, &[rect(x + t, y + hh, inner_w, t)]).ok(); }
}

/// 画冒号（两个实心方块）
fn draw_colon(f: &mut impl Frame, x: i32, y: i32, h: i32, size: i32, color: Color32F) {
    let gap = h / 3;
    f.clear(color, &[rect(x, y + gap - size / 2, size, size)]).ok();
    f.clear(color, &[rect(x, y + 2 * gap - size / 2, size, size)]).ok();
}

/// 画一行 7-segment 数字串（如 "12:34:56"）
/// 返回总宽度
fn draw_7seg_string(f: &mut impl Frame, text: &str, x: i32, y: i32,
    dw: i32, dh: i32, t: i32, gap: i32, color: Color32F) -> i32
{
    let colon_w = t + 2;
    let mut cx = x;
    for ch in text.chars() {
        match ch {
            '0'..='9' => {
                draw_7seg(f, ch as u8 - b'0', cx, y, dw, dh, t, color);
                cx += dw + gap;
            }
            ':' => {
                draw_colon(f, cx, y, dh, t, color);
                cx += colon_w + gap;
            }
            ' ' => { cx += dw / 2; }
            _ => { cx += gap; }
        }
    }
    cx - x - gap // 总宽度（减掉最后一个 gap）
}

/// 计算 7-segment 字符串宽度
fn seg_text_width(text: &str, dw: i32, gap: i32) -> i32 {
    let colon_w = gap + 2;
    let mut w = 0;
    for ch in text.chars() {
        match ch {
            '0'..='9' => w += dw + gap,
            ':' => w += colon_w + gap,
            ' ' => w += dw / 2,
            _ => w += gap,
        }
    }
    (w - gap).max(0)
}

#[inline(always)]
fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}

// ═══════════════════════════════════════════════════════════
// 布局计算
// ═══════════════════════════════════════════════════════════

pub fn slot(i: usize, n: usize, ow: i32, oh: i32, bar_h: i32, cfg: &Config) -> (i32, i32, i32, i32) {
    let gap = cfg.layout.gap;
    let margin = cfg.layout.margin;
    let usable_h = oh - bar_h;

    match n {
        0 => (0, bar_h, 0, 0),
        1 => (margin, bar_h + margin, ow - 2 * margin, usable_h - 2 * margin),
        2 => {
            let total_w = ow - 2 * margin - gap;
            let left_w = (total_w + 1) / 2;
            let right_w = total_w / 2;
            if i == 0 {
                (margin, bar_h + margin, left_w, usable_h - 2 * margin)
            } else {
                (margin + left_w + gap, bar_h + margin, right_w, usable_h - 2 * margin)
            }
        }
        _ => {
            let rows = ((n + 1) / 2) as i32;
            let col_w = (ow - gap - 2 * margin) / 2;
            let total_row_h = usable_h - 2 * margin - gap * (rows - 1);
            let base_h = total_row_h / rows;
            let extra = total_row_h % rows;
            let row = (i / 2) as i32;
            let row_y = bar_h + margin + row * (base_h + gap)
                + if row > 0 { extra.min(row) } else { 0 };
            let row_h = base_h + if row < extra { 1 } else { 0 };
            let col = (i % 2) as i32;
            let x = margin + col * (col_w + gap);
            (x, row_y, col_w, row_h)
        }
    }
}

// ═══════════════════════════════════════════════════════════
// 渲染
// ═══════════════════════════════════════════════════════════

pub fn render_window_bg(f: &mut impl Frame, cfg: &Config, n: usize, ow: i32, oh: i32, bar_h: i32) {
    if n == 0 { return; }
    let bw = cfg.layout.border_width;
    let bg = color_hex(&cfg.colors.background);
    for i in 0..n {
        let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg);
        f.clear(bg, &[rect(x - bw, y - bw, w + 2 * bw, h + 2 * bw)]).ok();
    }
}

pub fn render_wallpaper(f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32, frame: u32) {
    f.clear(color_hex(&cfg.wallpaper.color), &[Rectangle::from_size(Size::new(ow, oh))]).ok();

    let accent = parse_color(&cfg.colors.focus_border);

    let grid = opaque(accent.0 * 0.03, accent.1 * 0.03, accent.2 * 0.03);
    for y in (0..oh).step_by(64) {
        f.clear(grid, &[rect(0, y, ow, 1)]).ok();
    }
    for x in (0..ow).step_by(64) {
        f.clear(grid, &[rect(x, 0, 1, oh)]).ok();
    }

    let dot = opaque(accent.0 * 0.05, accent.1 * 0.05, accent.2 * 0.05);
    for y in (0..oh).step_by(64) {
        for x in (0..ow).step_by(64) {
            f.clear(dot, &[rect(x, y, 2, 2)]).ok();
        }
    }

    let t = frame as f32 * 0.012;
    let spots: [(f32, f32, f32, f32, i32, f32); 3] = [
        (t.sin(), t.cos(), 0.5, 0.5, 160, 0.03),
        ((t * 0.6 + 2.1).sin(), (t * 0.6 + 2.1).cos(), 0.25, 0.65, 120, 0.02),
        ((t * 0.4 + 4.2).sin(), (t * 0.4 + 4.2).cos(), 0.75, 0.35, 90, 0.015),
    ];
    for (sx, sy, cx, cy, size, brightness) in spots {
        let px = (sx * 300.0 + ow as f32 * cx) as i32;
        let py = (sy * 200.0 + oh as f32 * cy) as i32;
        f.clear(opaque(accent.0 * brightness, accent.1 * brightness, accent.2 * brightness),
            &[rect(px - size / 2, py - size / 2, size, size)]).ok();
        let inner = size / 4;
        f.clear(opaque(accent.0 * brightness * 2.5, accent.1 * brightness * 2.5, accent.2 * brightness * 2.5),
            &[rect(px - inner / 2, py - inner / 2, inner, inner)]).ok();
    }
}

pub fn render_window_decorations(
    f: &mut impl Frame, cfg: &Config,
    i: usize, n: usize, focus_idx: Option<usize>,
    ow: i32, oh: i32, bar_h: i32,
) {
    if n == 0 { return; }
    let bw = cfg.layout.border_width;
    let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg);
    let is_focused = focus_idx == Some(i);

    if is_focused {
        let accent = parse_color(&cfg.colors.focus_border);
        let border = opaque(accent.0, accent.1, accent.2);
        let bright = opaque((accent.0 * 1.6).min(1.0), (accent.1 * 1.6).min(1.0), (accent.2 * 1.6).min(1.0));
        let dark = opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3);

        f.clear(border, &[rect(x - bw, y - bw, w + 2 * bw, bw)]).ok();
        f.clear(border, &[rect(x - bw, y + h, w + 2 * bw, bw)]).ok();
        f.clear(border, &[rect(x - bw, y, bw, h)]).ok();
        f.clear(border, &[rect(x + w, y, bw, h)]).ok();

        f.clear(bright, &[rect(x - bw, y - bw, w + 2 * bw, 2)]).ok();
        f.clear(dark, &[rect(x - bw, y + h + bw - 3, w + 2 * bw, 3)]).ok();

        // 窗口编号标签（7-segment）
        let label_w = 14 + S1;
        f.clear(border, &[rect(x - bw, y - bw, label_w, 18)]).ok();
        draw_7seg(f, (i + 1) as u8, x - bw + 3, y - bw + 2, 10, 14, 3,
            opaque(0.0, 0.0, 0.0));
    } else {
        let unfocus = parse_color(&cfg.colors.unfocus_border);
        let border = opaque(unfocus.0, unfocus.1, unfocus.2);
        f.clear(border, &[rect(x, y, w, bw)]).ok();
        f.clear(border, &[rect(x, y + h - bw, w, bw)]).ok();
        f.clear(border, &[rect(x, y, bw, h)]).ok();
        f.clear(border, &[rect(x + w - bw, y, bw, h)]).ok();
    }
}

/// 渲染 headbar（v22 — 7-segment 几何数字）
pub fn render_headbar(
    f: &mut impl Frame, cfg: &Config, ow: i32, _oh: i32,
    n_windows: usize, focus_idx: Option<usize>, time_secs: u64,
    _window_title: &str,
    active_workspace: usize, total_workspaces: usize,
    workspace_window_counts: &[usize],
) {
    if !cfg.bar.enabled { return; }
    let h = cfg.bar.height; // 42px

    let fg = parse_color(&cfg.colors.bar_foreground);
    let ws_active = parse_color(&cfg.colors.bar_workspace_active);
    let status_color = parse_color(&cfg.colors.bar_status);
    let accent = parse_color(&cfg.colors.focus_border);
    let sep_color = parse_color(&cfg.colors.bar_separator);

    // 7-segment 参数（workspace 和时钟共用）
    let dw = 10;  // 数字宽
    let dh = 18;  // 数字高
    let dt = 3;   // 笔画粗
    let dg = 3;   // 数字间距

    // ── 背景条 ──
    f.clear(color_hex(&cfg.colors.bar_background), &[Rectangle::from_size(Size::new(ow, h))]).ok();

    // 底部 accent 发光线
    for (off, br) in [(0i32, 1.0f32), (1, 0.6), (2, 0.3), (3, 0.12), (4, 0.04)] {
        f.clear(opaque(accent.0 * br, accent.1 * br, accent.2 * br),
            &[rect(0, h - 5 + off, ow, 1)]).ok();
    }

    let mut x = S4;

    // ── TITAN logo（用粗实心矩形拼字母）──
    let lw = 4;   // logo 笔画粗
    let lh = 20;  // logo 字母高
    let ly = h / 2 - lh / 2;
    let lg = 2;   // 字母间距
    let logo_color = opaque(accent.0, accent.1, accent.2);
    // T: 竖线 + 横线
    f.clear(logo_color, &[rect(x + 4, ly, lw, lh)]).ok();
    f.clear(logo_color, &[rect(x, ly, 12, lw)]).ok();
    x += 14 + lg;
    // I: 竖线
    f.clear(logo_color, &[rect(x + 2, ly, lw, lh)]).ok();
    x += 8 + lg;
    // T: 竖线 + 横线
    f.clear(logo_color, &[rect(x + 4, ly, lw, lh)]).ok();
    f.clear(logo_color, &[rect(x, ly, 12, lw)]).ok();
    x += 14 + lg;
    // A: 左竖 + 右竖 + 横 + 中横
    f.clear(logo_color, &[rect(x, ly, lw, lh)]).ok();
    f.clear(logo_color, &[rect(x + 8, ly, lw, lh)]).ok();
    f.clear(logo_color, &[rect(x, ly, 12, lw)]).ok();
    f.clear(logo_color, &[rect(x, ly + lh / 2 - 1, 12, lw)]).ok();
    x += 14 + lg;
    // N: 左竖 + 右竖 + 斜线（用粗矩形近似）
    f.clear(logo_color, &[rect(x, ly, lw, lh)]).ok();
    f.clear(logo_color, &[rect(x + 8, ly, lw, lh)]).ok();
    f.clear(logo_color, &[rect(x + 2, ly + 2, lw, lw)]).ok();
    f.clear(logo_color, &[rect(x + 4, ly + 6, lw, lw)]).ok();
    f.clear(logo_color, &[rect(x + 6, ly + 10, lw, lw)]).ok();
    x += 14 + S4;

    // 分隔线
    f.clear(opaque(sep_color.0 * 0.3, sep_color.1 * 0.3, sep_color.2 * 0.3),
        &[rect(x, S3, 1, h - S6)]).ok();
    x += S3;

    // ── 工作区指示器（7-segment 数字 + 彩色方块背景）──
    let ws_pad = 5;
    let ws_block_h = dh + ws_pad * 2; // 28px
    let ws_block_y = h / 2 - ws_block_h / 2;
    let ws_gap = 3;
    let max_show = total_workspaces.min(9);

    for i in 0..max_show {
        let is_active = i == active_workspace;
        let ws_wins = workspace_window_counts.get(i).copied().unwrap_or(0);
        let has_wins = ws_wins > 0;
        let digit = (i + 1) as u8;
        let block_w = dw + ws_pad * 2;
        let num_x = x + ws_pad;
        let num_y = ws_block_y + ws_pad;

        if is_active {
            // 活跃工作区：accent 填充 + 暗色数字
            f.clear(opaque(ws_active.0, ws_active.1, ws_active.2),
                &[rect(x, ws_block_y, block_w, ws_block_h)]).ok();
            // 顶部高亮
            f.clear(opaque(ws_active.0 * 1.4, ws_active.1 * 1.4, ws_active.2 * 1.4),
                &[rect(x, ws_block_y, block_w, 2)]).ok();
            draw_7seg(f, digit, num_x, num_y, dw, dh, dt, opaque(0.05, 0.05, 0.08));
        } else if has_wins {
            // 有窗口：暗色方块 + dim 数字
            f.clear(opaque(fg.0 * 0.12, fg.1 * 0.12, fg.2 * 0.12),
                &[rect(x, ws_block_y, block_w, ws_block_h)]).ok();
            // 底部指示点
            f.clear(opaque(fg.0 * 0.5, fg.1 * 0.5, fg.2 * 0.5),
                &[rect(x + block_w / 2 - 2, ws_block_y + ws_block_h + 1, 4, 2)]).ok();
            draw_7seg(f, digit, num_x, num_y, dw, dh, dt, opaque(fg.0 * 0.7, fg.1 * 0.7, fg.2 * 0.7));
        } else {
            // 空工作区：极暗方块 + 很暗数字
            f.clear(opaque(fg.0 * 0.04, fg.1 * 0.04, fg.2 * 0.04),
                &[rect(x, ws_block_y, block_w, ws_block_h)]).ok();
            draw_7seg(f, digit, num_x, num_y, dw, dh, dt, opaque(fg.0 * 0.2, fg.1 * 0.2, fg.2 * 0.2));
        }

        x += block_w + ws_gap;
    }

    // 分隔线
    f.clear(opaque(sep_color.0 * 0.3, sep_color.1 * 0.3, sep_color.2 * 0.3),
        &[rect(x + S1, S3, 1, h - S6)]).ok();
    x += S4;

    // ── 中央窗口计数 ──
    if n_windows > 0 {
        let count_str = format!("{}:{}", focus_idx.map(|i| i + 1).unwrap_or(0), n_windows);
        let cw = seg_text_width(&count_str, dw, dg);
        let cx = ow / 2 - cw / 2;
        let cy = h / 2 - dh / 2;
        f.clear(opaque(fg.0 * 0.04, fg.1 * 0.04, fg.2 * 0.04),
            &[rect(cx - S2, S2, cw + S4, h - S4)]).ok();
        draw_7seg_string(f, &count_str, cx, cy, dw, dh, dt, dg, opaque(fg.0 * 0.5, fg.1 * 0.5, fg.2 * 0.5));
    }

    // ── 右侧时钟（7-segment，使用 libc localtime）──
    let mut rx = ow - S4;
    let time_secs_c = time_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_secs_c, &mut tm) };
    let local_h = tm.tm_hour as u8;
    let minutes = tm.tm_min as u8;
    let seconds = tm.tm_sec as u8;

    let ty = h / 2 - dh / 2;
    let clock_color = opaque(status_color.0, status_color.1, status_color.2);

    // 日期（从 localtime 获取）
    if cfg.bar.show_date {
        let month = (tm.tm_mon + 1) as u8;
        let day = tm.tm_mday as u8;
        let date_str = format!("{:02}:{:02}", month, day);
        let dw2 = seg_text_width(&date_str, dw, dg);
        draw_7seg_string(f, &date_str, rx - dw2, ty, dw, dh, dt, dg,
            opaque(fg.0 * 0.4, fg.1 * 0.4, fg.2 * 0.4));
        rx -= dw2 + S3;
        f.clear(opaque(sep_color.0 * 0.3, sep_color.1 * 0.3, sep_color.2 * 0.3),
            &[rect(rx, S3, 1, h - S6)]).ok();
        rx -= S3;
    }

    // 时钟 HH:MM:SS（7-segment）
    let time_str = format!("{:02}:{:02}:{:02}", local_h, minutes, seconds);
    let tw = seg_text_width(&time_str, dw, dg);
    // 时钟背景
    f.clear(opaque(accent.0 * 0.08, accent.1 * 0.08, accent.2 * 0.08),
        &[rect(rx - tw - S2, S2, tw + S4, h - S4)]).ok();
    draw_7seg_string(f, &time_str, rx - tw, ty, dw, dh, dt, dg, clock_color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    fn test_cfg() -> Config { Config::default() }

    #[test]
    fn test_slot_one() {
        let cfg = test_cfg();
        let (_x, y, w, h) = slot(0, 1, 2560, 1440, 42, &cfg);
        assert!(y >= 42 && w > 0 && h > 0);
    }
    #[test]
    fn test_slot_two() {
        let cfg = test_cfg();
        let a = slot(0, 2, 2560, 1440, 42, &cfg);
        let b = slot(1, 2, 2560, 1440, 42, &cfg);
        assert!(a.0 + a.2 <= b.0);
    }
    #[test]
    fn test_no_overlap() {
        let cfg = test_cfg();
        for n in 1..=6usize {
            let mut rects: Vec<(i32,i32,i32,i32)> = vec![];
            for i in 0..n {
                let r = slot(i, n, 2560, 1440, 42, &cfg);
                for (j, p) in rects.iter().enumerate() {
                    let overlap = r.0 < p.0+p.2 && r.0+r.2>p.0 && r.1<p.1+p.3 && r.1+r.3>p.1;
                    assert!(!overlap, "n={n}: {j} overlaps {}", i);
                }
                rects.push(r);
            }
        }
    }
}
