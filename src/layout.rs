//! 布局计算 + 壁纸渲染 + Headbar 渲染 + 窗口装饰

use crate::config::{parse_color, Config};
use crate::font;
use smithay::{
    backend::renderer::{Frame, Color32F},
    utils::{Physical, Point, Rectangle, Size},
};

/// 计算平铺位置
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

/// 渲染壁纸
pub fn render_wallpaper(f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32, frame: u32) {
    match cfg.wallpaper.mode.as_str() {
        "gradient" => {
            let top = parse_color(&cfg.wallpaper.gradient_top);
            let bot = parse_color(&cfg.wallpaper.gradient_bottom);
            let accent = parse_color(&cfg.colors.focus_border);
            // 多层渐变 + 动态 ambient light（每帧微变）
            let pulse = (frame as f32 * 0.005).sin() * 0.03;
            let step = 4;
            for y in (0..oh).step_by(step) {
                let t = y as f32 / oh as f32;
                // 加一条微弱的 accent 色彩曲线（让背景不单调）
                let curve = (t * std::f32::consts::PI).sin() * 0.04;
                let r = top.0 + (bot.0 - top.0) * t + accent.0 * (curve + pulse);
                let g = top.1 + (bot.1 - top.1) * t + accent.1 * (curve * 0.5);
                let b = top.2 + (bot.2 - top.2) * t + accent.2 * (curve + pulse);
                let rect = Rectangle::<i32, Physical>::new(Point::new(0, y), Size::new(ow, step as i32));
                f.clear(Color32F::new(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), 1.0), &[rect]).ok();
            }
            // 底部柔光（模拟 ambient occlusion）
            for i in 0..20 {
                let alpha = 0.03 * (1.0 - i as f32 / 20.0);
                let y = oh - 20 + i;
                f.clear(Color32F::new(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3, alpha),
                    &[Rectangle::new(Point::new(0, y), Size::new(ow, 1))]).ok();
            }
        }
        "color" | _ => {
            let bg = parse_color(&cfg.wallpaper.color);
            f.clear(Color32F::new(bg.0, bg.1, bg.2, 1.0),
                &[Rectangle::from_size(Size::new(ow, oh))]).ok();
        }
    }
}

/// 渲染窗口边框 + 阴影
pub fn render_window_decorations(
    f: &mut impl Frame, cfg: &Config,
    i: usize, n: usize, focus_idx: Option<usize>,
    ow: i32, oh: i32, bar_h: i32,
) {
    if n == 0 { return; }
    let focus_color = parse_color(&cfg.colors.focus_border);
    let unfocus_color = parse_color(&cfg.colors.unfocus_border);
    let accent = parse_color(&cfg.colors.focus_border);
    let bw = cfg.layout.border_width;

    let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg);
    let is_focused = focus_idx == Some(i);
    let (cr, cg, cb) = if is_focused { focus_color } else { unfocus_color };
    let border_alpha = if is_focused { 1.0 } else { 0.4 };

    // 窗口阴影（多层半透明，模拟高斯模糊）
    if is_focused {
        let shadow_layers = 6;
        for s in 1..=shadow_layers {
            let expand = s as i32 * 2;
            let alpha = 0.04 * (1.0 - s as f32 / shadow_layers as f32);
            let shadow = Color32F::new(0.0, 0.0, 0.0, alpha);
            // 上
            f.clear(shadow, &[Rectangle::new(Point::new(x - expand, y - expand), Size::new(w + 2 * expand, expand))]).ok();
            // 下
            f.clear(shadow, &[Rectangle::new(Point::new(x - expand, y + h), Size::new(w + 2 * expand, expand))]).ok();
            // 左
            f.clear(shadow, &[Rectangle::new(Point::new(x - expand, y), Size::new(expand, h))]).ok();
            // 右
            f.clear(shadow, &[Rectangle::new(Point::new(x + w, y), Size::new(expand, h))]).ok();
        }
    }

    let color = Color32F::new(cr, cg, cb, border_alpha);
    // 边框（4条）
    f.clear(color, &[Rectangle::new(Point::new(x, y), Size::new(w, bw))]).ok();
    f.clear(color, &[Rectangle::new(Point::new(x, y + h - bw), Size::new(w, bw))]).ok();
    f.clear(color, &[Rectangle::new(Point::new(x, y), Size::new(bw, h))]).ok();
    f.clear(color, &[Rectangle::new(Point::new(x + w - bw, y), Size::new(bw, h))]).ok();

    // 焦点窗口顶部高亮线（2px brighter）
    if is_focused {
        let bright = Color32F::new(cr * 1.3, cg * 1.3, cb * 1.3, 0.8);
        f.clear(bright, &[Rectangle::new(Point::new(x, y), Size::new(w, 1))]).ok();
        // 底部微弱 accent 光
        f.clear(Color32F::new(accent.0, accent.1, accent.2, 0.15),
            &[Rectangle::new(Point::new(x, y + h), Size::new(w, 2))]).ok();
    }
}

/// 渲染 headbar
pub fn render_headbar(f: &mut impl Frame, cfg: &Config, ow: i32, _oh: i32, n_windows: usize, focus_idx: Option<usize>, time_secs: u64) {
    if !cfg.bar.enabled { return; }
    let h = cfg.bar.height;
    let scale = 2;
    let text_y = h / 2 - 7 * scale / 2;

    let fg = parse_color(&cfg.colors.bar_foreground);
    let ws_active = parse_color(&cfg.colors.bar_workspace_active);
    let ws_inactive = parse_color(&cfg.colors.bar_workspace_inactive);
    let status_color = parse_color(&cfg.colors.bar_status);
    let accent = parse_color(&cfg.colors.focus_border);
    let sep_color = parse_color(&cfg.colors.bar_separator);

    // ── 背景（渐变 + 内部深度）──
    let top = parse_color(&cfg.bar.gradient_top);
    let bot = parse_color(&cfg.bar.gradient_bottom);
    let opacity = cfg.bar.opacity;
    for y in 0..h {
        let t = y as f32 / h as f32;
        let r = top.0 + (bot.0 - top.0) * t;
        let g = top.1 + (bot.1 - top.1) * t;
        let b = top.2 + (bot.2 - top.2) * t;
        f.clear(Color32F::new(r, g, b, opacity),
            &[Rectangle::new(Point::new(0, y), Size::new(ow, 1))]).ok();
    }

    // 顶部高亮（模拟顶部光照）
    f.clear(Color32F::new(1.0, 1.0, 1.0, 0.03),
        &[Rectangle::new(Point::new(0, 0), Size::new(ow, 1))]).ok();

    // 底部 accent 发光线（多层，模拟 blur）
    for i in 0..4 {
        let alpha = 0.2 * (1.0 - i as f32 / 4.0);
        f.clear(Color32F::new(accent.0, accent.1, accent.2, alpha),
            &[Rectangle::new(Point::new(0, h - 1 - i), Size::new(ow, 1))]).ok();
    }

    let mut x = cfg.bar.padding_left;

    // ── TITAN logo ──
    font::draw_text(f, "TITAN", x, text_y, scale, accent, 1.0);
    x += font::text_width("TITAN", scale) + cfg.bar.workspace_spacing;

    // 竖分隔线（带渐变，中间亮两端暗）
    draw_vsep(f, x, 6, h - 12, sep_color, cfg.bar.separator_width);
    x += cfg.bar.workspace_spacing * 2;

    // ── 工作区指示器 ──
    let ws_size = h - 16;
    let ws_y = 8;
    for i in 0..4 {
        let is_focused = focus_idx == Some(i);
        let has_windows = i < n_windows.min(4);
        let (cr, cg, cb) = if is_focused { ws_active }
            else if has_windows { (fg.0 * 0.25, fg.1 * 0.25, fg.2 * 0.25) }
            else { (ws_inactive.0 * 0.2, ws_inactive.1 * 0.2, ws_inactive.2 * 0.2) };
        let alpha = if is_focused { 1.0 } else if has_windows { 0.5 } else { 0.15 };

        f.clear(Color32F::new(cr, cg, cb, alpha),
            &[Rectangle::new(Point::new(x, ws_y), Size::new(ws_size, ws_size))]).ok();

        // 焦点工作区底部指示条 + 内部微弱高光
        if is_focused {
            f.clear(Color32F::new(ws_active.0, ws_active.1, ws_active.2, 0.4),
                &[Rectangle::new(Point::new(x + 1, ws_y + 1), Size::new(ws_size - 2, ws_size / 3))]).ok();
            f.clear(Color32F::new(1.0, 1.0, 1.0, 0.1),
                &[Rectangle::new(Point::new(x, ws_y), Size::new(ws_size, 1))]).ok();
        }

        let num = format!("{}", i + 1);
        let nw = font::text_width(&num, 1);
        font::draw_text(f, &num, x + (ws_size - nw) / 2, ws_y + (ws_size - 7) / 2, 1,
            if is_focused { (0.0, 0.0, 0.0) } else { fg }, if is_focused { 0.95 } else { 0.4 });
        x += ws_size + cfg.bar.workspace_spacing;
    }

    draw_vsep(f, x, 8, h - 16, sep_color, cfg.bar.separator_width);
    x += cfg.bar.workspace_spacing;

    // ── CPU 指示器 ──
    if cfg.bar.show_cpu {
        let core_w = 3;
        let core_gap = 1;
        for c in 0..4 {
            let usage = ((time_secs * 13 + c as u64 * 17) % 100) as f32 / 100.0;
            let color = if usage > 0.8 { parse_color(&cfg.colors.bar_urgent) }
                        else if usage > 0.5 { (1.0, 0.7, 0.2) } else { status_color };
            let full_h = ws_size;
            let fill_h = (usage * full_h as f32) as i32;
            // 背景
            f.clear(Color32F::new(ws_inactive.0, ws_inactive.1, ws_inactive.2, 0.15),
                &[Rectangle::new(Point::new(x, ws_y), Size::new(core_w, full_h))]).ok();
            // 填充
            if fill_h > 0 {
                f.clear(Color32F::new(color.0, color.1, color.2, 0.7),
                    &[Rectangle::new(Point::new(x, ws_y + full_h - fill_h), Size::new(core_w, fill_h))]).ok();
            }
            x += core_w + core_gap;
        }
        x += cfg.bar.workspace_spacing;
    }

    // ── 中央窗口信息 ──
    if n_windows > 0 {
        let win_text = format!("WIN {}/{}", focus_idx.map(|i| i + 1).unwrap_or(0), n_windows);
        let tw = font::text_width(&win_text, 2);
        font::draw_text(f, &win_text, ow / 2 - tw / 2, text_y, 2, fg, 0.4);
    }

    // ── 右侧 ──
    let mut rx = ow - cfg.bar.padding_right;

    let hours = ((time_secs / 3600) % 24) as u8;
    let minutes = ((time_secs / 60) % 60) as u8;
    let seconds = (time_secs % 60) as u8;
    let local_h = (hours as i32 + 8) % 24;

    let time_str = format!("{:02}:{:02}:{:02}", local_h, minutes, seconds);
    let tw = font::text_width(&time_str, 2);
    font::draw_text(f, &time_str, rx - tw, text_y, 2, status_color, 1.0);
    rx -= tw + cfg.bar.workspace_spacing * 2;

    // 日期
    if cfg.bar.show_date {
        let day_of_year = (time_secs / 86400) as u32 % 365;
        let month = (day_of_year / 30 + 1).min(12);
        let day = (day_of_year % 30 + 1).min(31);
        let date_str = format!("{:02}/{:02}", month, day);
        let dw = font::text_width(&date_str, 2);
        font::draw_text(f, &date_str, rx - dw, text_y, 2, fg, 0.45);
        rx -= dw + cfg.bar.workspace_spacing;
        draw_vsep(f, rx, 8, h - 16, sep_color, cfg.bar.separator_width);
        rx -= cfg.bar.workspace_spacing;
    }

    // 秒数进度条
    let bar_w = 50;
    let bar_h = 3;
    let by = h / 2 - bar_h / 2;
    f.clear(Color32F::new(ws_inactive.0, ws_inactive.1, ws_inactive.2, 0.15),
        &[Rectangle::new(Point::new(rx - bar_w, by), Size::new(bar_w, bar_h))]).ok();
    let pw = (seconds as f32 / 60.0 * bar_w as f32) as i32;
    if pw > 0 {
        f.clear(Color32F::new(accent.0, accent.1, accent.2, 0.4),
            &[Rectangle::new(Point::new(rx - bar_w, by), Size::new(pw, bar_h))]).ok();
    }
}

/// 渲染竖分隔线（渐变，中间亮两端暗）
fn draw_vsep(f: &mut impl Frame, x: i32, y: i32, height: i32, color: (f32, f32, f32), width: i32) {
    let mid = y + height / 2;
    for dy in 0..height {
        let dist_from_mid = ((y + dy) - mid).abs() as f32 / (height as f32 / 2.0);
        let alpha = 0.4 * (1.0 - dist_from_mid);
        f.clear(Color32F::new(color.0, color.1, color.2, alpha),
            &[Rectangle::new(Point::new(x, y + dy), Size::new(width, 1))]).ok();
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
        let (x, y, w, h) = slot(0, 1, 2560, 1440, 36, &cfg);
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
            let mut rects = vec![];
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
