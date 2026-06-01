//! 通知弹窗渲染（右上角 overlay，淡入淡出）

use crate::text_render;
use smithay::backend::renderer::Frame;
use super::util::{opaque, rect};

/// 渲染通知弹窗（右上角 overlay）
pub fn render_notifications(
    f: &mut impl Frame,
    notifications: &[(String, std::time::Instant, std::time::Duration)],
    ow: i32, bar_h: i32,
    accent: (f32, f32, f32),
) {
    if notifications.is_empty() { return; }
    let now = std::time::Instant::now();
    let fg = opaque(0.95, 0.95, 0.98);
    let bg = opaque(0.08, 0.08, 0.12);
    let accent_color = opaque(accent.0, accent.1, accent.2);
    let pad = 12;
    let notif_h = 28;
    let gap = 6;
    let max_w = 360;
    let font_size = 15.0;

    for (idx, (text, created, duration)) in notifications.iter().enumerate() {
        let elapsed = now.duration_since(*created).as_secs_f32();
        let remaining = duration.as_secs_f32() - elapsed;
        if remaining <= 0.0 { continue; }

        // 淡入淡出
        let alpha = if elapsed < 0.2 { elapsed / 0.2 } else if remaining < 0.5 { remaining / 0.5 } else { 1.0 };

        let tw = text_render::text_width(text, font_size).min(max_w - 2 * pad) as i32;
        let nw = tw + 2 * pad;
        let ny = bar_h + 10 + idx as i32 * (notif_h + gap);

        // 背景
        f.clear(bg, &[rect(ow - nw - 12, ny, nw, notif_h)]).ok();
        // 左侧 accent 竖条
        f.clear(accent_color, &[rect(ow - nw - 12, ny, 3, notif_h)]).ok();
        // 文字
        let text_color = (0.95 * alpha, 0.95 * alpha, 0.98 * alpha);
        text_render::draw_text(f, text, ow - nw - 12 + pad, ny + notif_h / 2 - font_size as i32 / 2 - 1, font_size, text_color);
    }
}
