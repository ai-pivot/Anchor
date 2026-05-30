//! 平铺布局计算 + Headbar 渲染

use crate::config::{parse_color, Config};
use smithay::{
    backend::renderer::{Frame, Color32F},
    utils::{Logical, Physical, Point, Rectangle, Size},
};

/// 计算第 i 个窗口在 n 个窗口中的平铺位置和大小
pub fn slot(i: usize, n: usize, ow: i32, oh: i32, bar_h: i32, cfg: &Config) -> (i32, i32, i32, i32) {
    let gap = cfg.layout.gap;
    let margin = cfg.layout.margin;
    let usable_h = oh - bar_h; // 减去 headbar 高度

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
            let col = (i % 2) as i32;
            let row = (i / 2) as i32;
            let x = margin + col * (sw + gap);
            let y = bar_h + margin + row * (sh + gap);
            (x, y, sw, sh)
        }
    }
}

/// 渲染 headbar（在窗口内容之后调用，确保在最顶层）
pub fn render_headbar(f: &mut impl Frame, cfg: &Config, ow: i32, _oh: i32, n_windows: usize, focus_idx: Option<usize>, time_secs: u64) {
    if !cfg.bar.enabled { return; }
    let h = cfg.bar.height;

    let bar_bg = parse_color(&cfg.colors.bar_background);
    let fg = parse_color(&cfg.colors.bar_foreground);
    let ws_active = parse_color(&cfg.colors.bar_workspace_active);
    let ws_inactive = parse_color(&cfg.colors.bar_workspace_inactive);
    let status_color = parse_color(&cfg.colors.bar_status);
    let accent = parse_color(&cfg.colors.focus_border);

    // ── 背景（半透明，让窗口内容微透出） ──
    let bar_rect = Rectangle::<i32, Physical>::new(Point::new(0, 0), Size::new(ow, h));
    f.clear(Color32F::new(bar_bg.0, bar_bg.1, bar_bg.2, 0.95), &[bar_rect]).ok();

    // 底部分隔线（accent 色）
    f.clear(Color32F::new(accent.0, accent.1, accent.2, 0.6),
        &[Rectangle::new(Point::new(0, h - 1), Size::new(ow, 1))]).ok();

    // ── 左侧：TITAN 文字 logo ──
    let scale = 2;
    let text_y = h / 2 - 7 * scale / 2;
    let mut x = cfg.bar.padding_left;
    crate::font::draw_text(f, "TITAN", x, text_y, scale, accent, 0.9);
    x += crate::font::text_width("TITAN", scale) + cfg.bar.workspace_spacing * 2;

    // 竖分隔线
    f.clear(Color32F::new(fg.0, fg.1, fg.2, 0.15),
        &[Rectangle::new(Point::new(x, 6), Size::new(1, h - 12))]).ok();
    x += cfg.bar.workspace_spacing;

    // ── 工作区指示器 ──
    let ws_size = h - 14;
    let ws_y = 7;
    for i in 0..4 {
        let is_focused = focus_idx == Some(i);
        let has_windows = i < n_windows.min(4);
        let (cr, cg, cb) = if is_focused { ws_active } else if has_windows { (fg.0 * 0.4, fg.1 * 0.4, fg.2 * 0.4) } else { (ws_inactive.0 * 0.4, ws_inactive.1 * 0.4, ws_inactive.2 * 0.4) };
        let alpha = if is_focused { 1.0 } else if has_windows { 0.7 } else { 0.3 };
        f.clear(Color32F::new(cr, cg, cb, alpha),
            &[Rectangle::new(Point::new(x, ws_y), Size::new(ws_size, ws_size))]).ok();
        // 工作区编号
        let num = &format!("{}", i + 1);
        let nw = crate::font::text_width(num, 1);
        crate::font::draw_text(f, num, x + (ws_size - nw) / 2, ws_y + (ws_size - 7) / 2, 1,
            if is_focused { (0.0, 0.0, 0.0) } else { (1.0, 1.0, 1.0) }, 0.8);
        x += ws_size + cfg.bar.workspace_spacing;
    }

    // ── 中央：窗口数量 ──
    if n_windows > 0 {
        let win_text = format!("{}:{}", n_windows, focus_idx.map(|i| i + 1).unwrap_or(0));
        let tw = crate::font::text_width(&win_text, 2);
        crate::font::draw_text(f, &win_text, ow / 2 - tw / 2, text_y, 2, fg, 0.6);
    }

    // ── 右侧：系统时间 ──
    let hours = ((time_secs / 3600) % 24) as u8;
    let minutes = ((time_secs / 60) % 60) as u8;
    let seconds = (time_secs % 60) as u8;
    // 使用本地时间偏移（CST = UTC+8）
    let (h_disp, m_disp) = {
        let local_h = (hours as i32 + 8) % 24;
        (local_h as u8, minutes)
    };
    let time_str = format!("{:02}:{:02}:{:02}", h_disp, m_disp, seconds);
    let tscale = 2;
    let tw = crate::font::text_width(&time_str, tscale);
    let tx = ow - cfg.bar.padding_right - tw;
    crate::font::draw_text(f, &time_str, tx, text_y, tscale, status_color, 1.0);

    // 时间前面的秒数进度条
    let bar_w = 80;
    let bar_h = 3;
    let bx = tx - bar_w - cfg.bar.workspace_spacing;
    let by = h / 2 - bar_h / 2;
    f.clear(Color32F::new(ws_inactive.0, ws_inactive.1, ws_inactive.2, 0.3),
        &[Rectangle::new(Point::new(bx, by), Size::new(bar_w, bar_h))]).ok();
    let pw = (seconds as f32 / 60.0 * bar_w as f32) as i32;
    if pw > 0 {
        f.clear(Color32F::new(status_color.0, status_color.1, status_color.2, 0.7),
            &[Rectangle::new(Point::new(bx, by), Size::new(pw, bar_h))]).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn test_cfg() -> Config { Config::default() }

    #[test]
    fn test_slot_one_window_fullscreen() {
        let cfg = test_cfg();
        let (x, y, w, h) = slot(0, 1, 2560, 1440, 32, &cfg);
        assert_eq!(x, 0);
        assert_eq!(y, 32); // bar height
        assert_eq!(w, 2560);
        assert_eq!(h, 1440 - 32);
    }

    #[test]
    fn test_slot_two_windows_side_by_side() {
        let cfg = test_cfg();
        let a = slot(0, 2, 2560, 1440, 32, &cfg);
        let b = slot(1, 2, 2560, 1440, 32, &cfg);
        assert!(a.0 + a.2 <= b.0, "left and right should not overlap");
        assert!(a.1 >= 32, "starts below bar");
        assert!(b.1 >= 32, "starts below bar");
    }

    #[test]
    fn test_slot_no_overlap() {
        let cfg = test_cfg();
        for n in 1..=6u32 {
            let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
            for i in 0..n {
                let r = slot(i as usize, n as usize, 2560, 1440, 32, &cfg);
                for (j, prev) in rects.iter().enumerate() {
                    let overlap = r.0 < prev.0 + prev.2
                        && r.0 + r.2 > prev.0
                        && r.1 < prev.1 + prev.3
                        && r.1 + r.3 > prev.1;
                    assert!(!overlap, "n={n}: window {j} {prev:?} overlaps window {} {r:?}", i);
                }
                rects.push(r);
            }
        }
    }
}
