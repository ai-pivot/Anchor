//! 通知弹窗渲染（右上角 overlay，淡入淡出，自动换行）
//! 赛博朋克风格：accent 竖条 + 多层发光 + 自适应高度

use crate::text_render;
use smithay::backend::renderer::Frame;
use super::util::{opaque, rect};

/// 自动换行：将文本按 maxWidth 切分为多行
fn wrap_text(text: &str, max_width: i32, font_size: f32) -> Vec<String> {
    if text.is_empty() { return vec![]; }
    let mut lines = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        let test = format!("{}{}", current, ch);
        let w = text_render::text_width(&test, font_size);
        if w > max_width && !current.is_empty() {
            lines.push(current.clone());
            current.clear();
        }
        current.push(ch);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// 渲染通知弹窗（右上角 overlay，多行自适应）
pub fn render_notifications(
    f: &mut impl Frame,
    notifications: &[(String, std::time::Instant, std::time::Duration)],
    ow: i32, bar_h: i32,
    accent: (f32, f32, f32),
) {
    if notifications.is_empty() { return; }
    let now = std::time::Instant::now();
    let pad = 12;
    let line_h = 20;
    let gap = 8;
    let max_w = 360;
    let text_area_w = max_w - 2 * pad - 8; // 8 = 左侧 accent 条宽度
    let font_size = 14.0;
    let margin_right = 16;
    let margin_top = 12;

    let mut y_cursor = bar_h + margin_top;

    for (text, created, duration) in notifications.iter() {
        let elapsed = now.duration_since(*created).as_secs_f32();
        let remaining = duration.as_secs_f32() - elapsed;
        if remaining <= 0.0 { continue; }

        // 淡入淡出
        let alpha = if elapsed < 0.2 {
            elapsed / 0.2
        } else if remaining < 0.5 {
            remaining / 0.5
        } else {
            1.0
        };

        // 自动换行
        let lines = wrap_text(text, text_area_w, font_size);
        if lines.is_empty() { continue; }

        // 计算实际需要的高度
        let content_h = lines.len() as i32 * line_h + pad;
        let actual_w = {
            let max_line_w: i32 = lines.iter()
                .map(|l| text_render::text_width(l, font_size))
                .max()
                .unwrap_or(0);
            (max_line_w + 2 * pad + 8).min(max_w)
        };

        let nx = ow - actual_w - margin_right;
        let ny = y_cursor;

        // ── 背景（自适应高度）──
        let bg_alpha = 0.85 * alpha;
        f.clear(opaque(0.06 * bg_alpha, 0.06 * bg_alpha, 0.10 * bg_alpha),
            &[rect(nx, ny, actual_w, content_h)]).ok();

        // ── 左侧 accent 竖条 + 发光 ──
        let accent_br = alpha * 0.7;
        f.clear(opaque(accent.0 * accent_br, accent.1 * accent_br, accent.2 * accent_br),
            &[rect(nx, ny, 3, content_h)]).ok();
        // 发光
        f.clear(opaque(accent.0 * accent_br * 0.3, accent.1 * accent_br * 0.3, accent.2 * accent_br * 0.3),
            &[rect(nx + 3, ny, 3, content_h)]).ok();

        // ── 顶部 accent 亮线 ──
        f.clear(opaque(accent.0 * alpha * 0.4, accent.1 * alpha * 0.4, accent.2 * alpha * 0.4),
            &[rect(nx, ny, actual_w, 1)]).ok();

        // ── 文字（多行）──
        for (li, line) in lines.iter().enumerate() {
            let lx = nx + 8 + pad;
            let ly = ny + pad / 2 + li as i32 * line_h;
            text_render::draw_text(f, line, lx, ly, font_size,
                (0.9 * alpha, 0.9 * alpha, 0.95 * alpha));
        }

        y_cursor += content_h + gap;
    }
}
