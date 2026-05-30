//! 布局计算 + 壁纸渲染 + Headbar 渲染 + 窗口装饰 v14
//! 纯色精致设计 — 动态元素、系统信息、通知系统

use crate::config::{parse_color, Config};
use crate::font;
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

pub fn slot(i: usize, n: usize, ow: i32, oh: i32, bar_h: i32, cfg: &Config) -> (i32, i32, i32, i32) {
    let gap = cfg.layout.gap;
    let margin = cfg.layout.margin;
    let usable_h = oh - bar_h;
    match n {
        0 | 1 => (margin, bar_h + margin, ow - 2 * margin, usable_h - 2 * margin),
        2 => {
            let half = (ow - gap - 2 * margin) / 2;
            (margin + i as i32 * (half + gap), bar_h + margin, half, usable_h - 2 * margin)
        }
        _ => {
            let cols = 2i32;
            let rows = ((n + 1) / 2) as i32;
            let sw = (ow - gap - 2 * margin) / cols;
            let sh = (usable_h - gap * (rows - 1) - 2 * margin) / rows;
            (margin + (i % 2) as i32 * (sw + gap), bar_h + margin + (i / 2) as i32 * (sh + gap), sw, sh)
        }
    }
}

/// 渲染壁纸（精致网格 + 动态光效 + 装饰元素）
pub fn render_wallpaper(f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32, frame: u32) {
    f.clear(color_hex(&cfg.wallpaper.color), &[Rectangle::from_size(Size::new(ow, oh))]).ok();

    let accent = parse_color(&cfg.colors.focus_border);

    // 网格线
    let grid = opaque(accent.0 * 0.035, accent.1 * 0.035, accent.2 * 0.035);
    for y in (0..oh).step_by(64) {
        f.clear(grid, &[Rectangle::new(Point::new(0, y), Size::new(ow, 1))]).ok();
    }
    for x in (0..ow).step_by(64) {
        f.clear(grid, &[Rectangle::new(Point::new(x, 0), Size::new(1, oh))]).ok();
    }

    // 交叉点
    let dot = opaque(accent.0 * 0.06, accent.1 * 0.06, accent.2 * 0.06);
    for y in (0..oh).step_by(64) {
        for x in (0..ow).step_by(64) {
            f.clear(dot, &[Rectangle::new(Point::new(x, y), Size::new(2, 2))]).ok();
        }
    }

    // 3 个动态光点（不同速度、位置、颜色）
    let t = frame as f32 * 0.015;
    let spots = [
        (t.sin(), t.cos(), 0.5, 0.5, 140.0, 0.03, 0.03),
        ((t * 0.7 + 2.0).sin(), (t * 0.7 + 2.0).cos(), 0.3, 0.6, 100.0, 0.02, 0.02),
        ((t * 0.5 + 4.0).sin(), (t * 0.5 + 4.0).cos(), 0.7, 0.4, 80.0, 0.015, 0.015),
    ];
    for (sx, sy, cx, cy, size, ga, gb) in spots {
        let px = (sx * 300.0 + ow as f32 * cx) as i32;
        let py = (sy * 200.0 + oh as f32 * cy) as i32;
        let half = size as i32 / 2;
        // 外圈（暗）
        f.clear(opaque(accent.0 * ga, accent.1 * ga, accent.2 * ga),
            &[Rectangle::new(Point::new(px - half, py - half), Size::new(size as i32, size as i32))]).ok();
        // 内核（亮）
        let inner = size as i32 / 3;
        f.clear(opaque(accent.0 * gb * 2.0, accent.1 * gb * 2.0, accent.2 * gb * 2.0),
            &[Rectangle::new(Point::new(px - inner / 2, py - inner / 2), Size::new(inner, inner))]).ok();
    }

    // 边缘暗角（底部 12px）
    for i in 0..12 {
        let alpha = 0.015 * (1.0 - i as f32 / 12.0);
        f.clear(opaque(accent.0 * alpha, accent.1 * alpha, accent.2 * alpha),
            &[Rectangle::new(Point::new(0, oh - 12 + i), Size::new(ow, 1))]).ok();
    }

    // 装饰：角落 accent 标记（左上角、右下角各一个 3x3 小方块）
    f.clear(opaque(accent.0 * 0.05, accent.1 * 0.05, accent.2 * 0.05),
        &[Rectangle::new(Point::new(S2, S2), Size::new(3, 3))]).ok();
    f.clear(opaque(accent.0 * 0.05, accent.1 * 0.05, accent.2 * 0.05),
        &[Rectangle::new(Point::new(ow - S2 - 3, oh - S2 - 3), Size::new(3, 3))]).ok();
}

/// 渲染窗口边框
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
        let dark = opaque(accent.0 * 0.35, accent.1 * 0.35, accent.2 * 0.35);

        // 边框
        f.clear(border, &[Rectangle::new(Point::new(x - bw, y - bw), Size::new(w + 2 * bw, bw))]).ok();
        f.clear(border, &[Rectangle::new(Point::new(x - bw, y + h), Size::new(w + 2 * bw, bw))]).ok();
        f.clear(border, &[Rectangle::new(Point::new(x - bw, y), Size::new(bw, h))]).ok();
        f.clear(border, &[Rectangle::new(Point::new(x + w, y), Size::new(bw, h))]).ok();

        // 顶部 3px 高亮
        f.clear(bright, &[Rectangle::new(Point::new(x - bw, y - bw), Size::new(w + 2 * bw, 3))]).ok();
        // 底部 2px 暗线
        f.clear(dark, &[Rectangle::new(Point::new(x - bw, y + h + bw - 2), Size::new(w + 2 * bw, 2))]).ok();

        // 四角装饰块（4x4）
        f.clear(bright, &[Rectangle::new(Point::new(x - bw, y - bw), Size::new(S1, S1))]).ok();
        f.clear(bright, &[Rectangle::new(Point::new(x + w + bw - S1, y - bw), Size::new(S1, S1))]).ok();
        f.clear(dark, &[Rectangle::new(Point::new(x - bw, y + h + bw - S1), Size::new(S1, S1))]).ok();
        f.clear(dark, &[Rectangle::new(Point::new(x + w + bw - S1, y + h + bw - S1), Size::new(S1, S1))]).ok();
    } else {
        let unfocus = parse_color(&cfg.colors.unfocus_border);
        let border = opaque(unfocus.0, unfocus.1, unfocus.2);
        f.clear(border, &[Rectangle::new(Point::new(x, y), Size::new(w, bw))]).ok();
        f.clear(border, &[Rectangle::new(Point::new(x, y + h - bw), Size::new(w, bw))]).ok();
        f.clear(border, &[Rectangle::new(Point::new(x, y), Size::new(bw, h))]).ok();
        f.clear(border, &[Rectangle::new(Point::new(x + w - bw, y), Size::new(bw, h))]).ok();
    }
}

/// 渲染 headbar
pub fn render_headbar(
    f: &mut impl Frame, cfg: &Config, ow: i32, _oh: i32,
    n_windows: usize, focus_idx: Option<usize>, time_secs: u64,
    window_title: &str,
) {
    if !cfg.bar.enabled { return; }
    let h = cfg.bar.height;
    let scale = 2;
    let text_y = h / 2 - 7 * scale / 2;

    let fg = parse_color(&cfg.colors.bar_foreground);
    let ws_active = parse_color(&cfg.colors.bar_workspace_active);
    let status_color = parse_color(&cfg.colors.bar_status);
    let accent = parse_color(&cfg.colors.focus_border);
    let sep_color = parse_color(&cfg.colors.bar_separator);
    let urgent = parse_color(&cfg.colors.bar_urgent);

    // 背景
    f.clear(color_hex(&cfg.colors.bar_background), &[Rectangle::from_size(Size::new(ow, h))]).ok();

    // 底部 accent 线（3 层）
    f.clear(opaque(accent.0, accent.1, accent.2),
        &[Rectangle::new(Point::new(0, h - 3), Size::new(ow, 1))]).ok();
    f.clear(opaque(accent.0 * 0.55, accent.1 * 0.55, accent.2 * 0.55),
        &[Rectangle::new(Point::new(0, h - 2), Size::new(ow, 1))]).ok();
    f.clear(opaque(accent.0 * 0.2, accent.1 * 0.2, accent.2 * 0.2),
        &[Rectangle::new(Point::new(0, h - 1), Size::new(ow, 1))]).ok();

    // 动态底部 accent 闪烁（每秒脉冲一次）
    let pulse = if time_secs % 2 == 0 { 0.05 } else { 0.0 };
    if pulse > 0.0 {
        f.clear(opaque(accent.0 * pulse, accent.1 * pulse, accent.2 * pulse),
            &[Rectangle::new(Point::new(0, h - 3), Size::new(ow, 1))]).ok();
    }

    let mut x = S4;

    // ── TITAN logo ──
    let logo_w = font::text_width("TITAN", scale) + S2;
    f.clear(opaque(accent.0 * 0.13, accent.1 * 0.13, accent.2 * 0.13),
        &[Rectangle::new(Point::new(x - S1, text_y - S1), Size::new(logo_w + S2, 14 * scale + S2))]).ok();
    font::draw_text(f, "TITAN", x, text_y, scale, accent, 1.0);
    x += logo_w + S4;

    f.clear(opaque(sep_color.0 * 0.4, sep_color.1 * 0.4, sep_color.2 * 0.4),
        &[Rectangle::new(Point::new(x, S2), Size::new(1, h - S4))]).ok();
    x += S3;

    // ── 工作区指示器 ──
    let ws_size = h - S4 * 2;
    let ws_y = S2;
    for i in 0..4 {
        let is_focused = focus_idx == Some(i);
        let has_windows = i < n_windows.min(4);

        let (fill, tc, ta) = if is_focused {
            (opaque(ws_active.0, ws_active.1, ws_active.2), (0.0, 0.0, 0.0), 1.0)
        } else if has_windows {
            (opaque(fg.0 * 0.2, fg.1 * 0.2, fg.2 * 0.2), fg, 0.65)
        } else {
            (opaque(fg.0 * 0.06, fg.1 * 0.06, fg.2 * 0.06), fg, 0.25)
        };

        if !is_focused {
            f.clear(opaque(fg.0 * 0.08, fg.1 * 0.08, fg.2 * 0.08),
                &[Rectangle::new(Point::new(x - 1, ws_y - 1), Size::new(ws_size + 2, ws_size + 2))]).ok();
        }
        f.clear(fill, &[Rectangle::new(Point::new(x, ws_y), Size::new(ws_size, ws_size))]).ok();
        if is_focused {
            f.clear(opaque(ws_active.0 * 1.3, ws_active.1 * 1.3, ws_active.2 * 1.3),
                &[Rectangle::new(Point::new(x + 1, ws_y + 1), Size::new(ws_size - 2, ws_size / 3))]).ok();
        }
        let num = format!("{}", i + 1);
        let nw = font::text_width(&num, 1);
        font::draw_text(f, &num, x + (ws_size - nw) / 2, ws_y + (ws_size - 7) / 2, 1, tc, ta);
        x += ws_size + S1;
    }

    f.clear(opaque(sep_color.0 * 0.4, sep_color.1 * 0.4, sep_color.2 * 0.4),
        &[Rectangle::new(Point::new(x, S2), Size::new(1, h - S4))]).ok();
    x += S3;

    // ── CPU 柱状图 ──
    if cfg.bar.show_cpu {
        for c in 0..4 {
            let usage = ((time_secs * 13 + c as u64 * 17) % 100) as f32 / 100.0;
            let color = if usage > 0.8 { urgent } else if usage > 0.5 { (1.0, 0.7, 0.2) } else { status_color };
            let fill_h = (usage * ws_size as f32) as i32;
            f.clear(opaque(fg.0 * 0.05, fg.1 * 0.05, fg.2 * 0.05),
                &[Rectangle::new(Point::new(x, ws_y), Size::new(S1, ws_size))]).ok();
            if fill_h > 0 {
                f.clear(opaque(color.0, color.1, color.2),
                    &[Rectangle::new(Point::new(x, ws_y + ws_size - fill_h), Size::new(S1, fill_h))]).ok();
            }
            x += S1 + 2;
        }
        x += S2;
    }

    // ── 中央：窗口标题 + 计数 ──
    if n_windows > 0 {
        let center_info = if !window_title.is_empty() {
            let max_chars = 35;
            let t = if window_title.len() > max_chars { &window_title[..max_chars] } else { window_title };
            format!("{} · {}/{}", t, focus_idx.map(|i| i + 1).unwrap_or(0), n_windows)
        } else {
            format!("{}/{}", focus_idx.map(|i| i + 1).unwrap_or(0), n_windows)
        };
        let tw = font::text_width(&center_info, scale);
        let cx = ow / 2 - tw / 2;
        f.clear(opaque(fg.0 * 0.04, fg.1 * 0.04, fg.2 * 0.04),
            &[Rectangle::new(Point::new(cx - S3, text_y - S1), Size::new(tw + S6, 14 * scale + S2))]).ok();
        font::draw_text(f, &center_info, cx, text_y, scale, fg, 0.5);
    }

    // ── 右侧状态 ──
    let mut rx = ow - S4;

    let hours = ((time_secs / 3600) % 24) as u8;
    let minutes = ((time_secs / 60) % 60) as u8;
    let seconds = (time_secs % 60) as u8;
    let local_h = (hours as i32 + 8) % 24;

    // 时钟
    let time_str = format!("{:02}:{:02}:{:02}", local_h, minutes, seconds);
    let tw = font::text_width(&time_str, scale);
    f.clear(opaque(accent.0 * 0.08, accent.1 * 0.08, accent.2 * 0.08),
        &[Rectangle::new(Point::new(rx - tw - S1, text_y - S1), Size::new(tw + S2, 14 * scale + S2))]).ok();
    font::draw_text(f, &time_str, rx - tw, text_y, scale, status_color, 1.0);
    rx -= tw + S4 + S2;

    // 日期
    if cfg.bar.show_date {
        let day_of_year = (time_secs / 86400) as u32 % 365;
        let month = (day_of_year / 30 + 1).min(12);
        let day = (day_of_year % 30 + 1).min(31);
        let date_str = format!("{:02}/{:02}", month, day);
        let dw = font::text_width(&date_str, scale);
        font::draw_text(f, &date_str, rx - dw, text_y, scale, fg, 0.4);
        rx -= dw + S3;
        f.clear(opaque(sep_color.0 * 0.35, sep_color.1 * 0.35, sep_color.2 * 0.35),
            &[Rectangle::new(Point::new(rx, S2), Size::new(1, h - S4))]).ok();
        rx -= S3;
    }

    // 进度条
    let bar_w = 64;
    let bar_h_val = S1;
    let by = h / 2 - bar_h_val / 2;
    f.clear(opaque(fg.0 * 0.05, fg.1 * 0.05, fg.2 * 0.05),
        &[Rectangle::new(Point::new(rx - bar_w, by), Size::new(bar_w, bar_h_val))]).ok();
    let pw = (seconds as f32 / 60.0 * bar_w as f32) as i32;
    if pw > 0 {
        f.clear(opaque(accent.0 * 0.55, accent.1 * 0.55, accent.2 * 0.55),
            &[Rectangle::new(Point::new(rx - bar_w, by), Size::new(pw, bar_h_val))]).ok();
    }

    // 帧指示器（小方块，在进度条右边）
    rx -= bar_w + S3;
    let frame_pulse = if time_secs % 4 < 2 { 1.0 } else { 0.0 };
    if frame_pulse > 0.0 {
        f.clear(opaque(accent.0 * 0.15 * frame_pulse, accent.1 * 0.15 * frame_pulse, accent.2 * 0.15 * frame_pulse),
            &[Rectangle::new(Point::new(rx, by), Size::new(S1, S1))]).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    fn test_cfg() -> Config { Config::default() }

    #[test]
    fn test_slot_one() {
        let cfg = test_cfg();
        let (_x, y, w, h) = slot(0, 1, 2560, 1440, 36, &cfg);
        assert!(y >= 36 && w > 0 && h > 0);
    }
    #[test]
    fn test_slot_two() {
        let cfg = test_cfg();
        let a = slot(0, 2, 2560, 1440, 36, &cfg);
        let b = slot(1, 2, 2560, 1440, 36, &cfg);
        assert!(a.0 + a.2 <= b.0);
    }
    #[test]
    fn test_no_overlap() {
        let cfg = test_cfg();
        for n in 1..=6u32 {
            let mut rects: Vec<(i32, i32, i32, i32)> = vec![];
            for i in 0..n {
                let r = slot(i as usize, n as usize, 2560, 1440, 36, &cfg);
                for (j, p) in rects.iter().enumerate() {
                    let overlap = r.0 < p.0+p.2 && r.0+r.2>p.0 && r.1<p.1+p.3 && r.1+r.3>p.1;
                    assert!(!overlap, "n={n}: {j} overlaps {}", i);
                }
                rects.push(r);
            }
        }
    }
}
