//! Settings Panel 基础 UI 原语
//!
//! 所有控件用 Frame::clear（实心矩形）+ text_render 绘制。
//! 与 Anchor 的 Launcher / Lock Screen 美学一致。

use smithay::backend::renderer::Frame;
use smithay::utils::{Physical, Point, Rectangle, Size};

use crate::config::parse_color;
use crate::text_render;

// ═══════════════════════════════════════════════════════════════════
// 基础几何辅助
// ═══════════════════════════════════════════════════════════════════

#[inline(always)]
pub fn opaque(r: f32, g: f32, b: f32) -> smithay::backend::renderer::Color32F {
    smithay::backend::renderer::Color32F::new(r, g, b, 1.0)
}

#[inline(always)]
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(Point::new(x, y), Size::new(w.max(0), h.max(0)))
}

/// 磨砂半透明颜色
pub fn glass(brightness: f32) -> smithay::backend::renderer::Color32F {
    opaque(brightness * 0.08, brightness * 0.08, brightness * 0.14)
}

/// 绘制圆角矩形（用 9 个矩形近似：4 角 + 4 边 + 中心）
/// 每个角是一个小方块，边缘比中心略暗，产生柔和的视觉圆角
pub fn rounded_rect(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: smithay::backend::renderer::Color32F,
) {
    let r = radius.min(w / 2).min(h / 2);
    // 中心
    f.clear(color, &[rect(x + r, y + r, w - 2 * r, h - 2 * r)])
        .ok();
    // 上下边
    f.clear(color, &[rect(x + r, y, w - 2 * r, r)]).ok();
    f.clear(color, &[rect(x + r, y + h - r, w - 2 * r, r)])
        .ok();
    // 左右边
    f.clear(color, &[rect(x, y + r, r, h - 2 * r)]).ok();
    f.clear(color, &[rect(x + w - r, y + r, r, h - 2 * r)])
        .ok();
    // 四角（简单的填充方块）
    f.clear(color, &[rect(x, y, r, r)]).ok();
    f.clear(color, &[rect(x + w - r, y, r, r)]).ok();
    f.clear(color, &[rect(x, y + h - r, r, r)]).ok();
    f.clear(color, &[rect(x + w - r, y + h - r, r, r)])
        .ok();
}

/// 绘制发光边框（多层，与 launcher glow 同）+ 实心 accent 内边框
pub fn glow_border(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    accent: (f32, f32, f32),
) {
    let layers: [(i32, f32); 5] = [
        (5, 0.03),
        (4, 0.07),
        (3, 0.14),
        (2, 0.28),
        (1, 0.50),
    ];
    for (expand, brightness) in layers {
        let glow = opaque(
            accent.0 * brightness,
            accent.1 * brightness,
            accent.2 * brightness,
        );
        f.clear(
            glow,
            &[rect(
                x - expand,
                y - expand,
                w + 2 * expand,
                expand,
            )],
        )
        .ok();
        f.clear(
            glow,
            &[rect(
                x - expand,
                y + h,
                w + 2 * expand,
                expand,
            )],
        )
        .ok();
        f.clear(glow, &[rect(x - expand, y, expand, h)]).ok();
        f.clear(glow, &[rect(x + w, y, expand, h)]).ok();
    }
    // 最内层：实心 accent 边框，确保在深色背景上也清晰可见
    let solid = opaque(accent.0 * 0.7, accent.1 * 0.7, accent.2 * 0.7);
    f.clear(solid, &[rect(x - 1, y - 1, w + 2, 2)]).ok();
    f.clear(solid, &[rect(x - 1, y + h - 1, w + 2, 2)]).ok();
    f.clear(solid, &[rect(x - 1, y + 1, 2, h - 2)]).ok();
    f.clear(solid, &[rect(x + w - 1, y + 1, 2, h - 2)]).ok();
}

// ═══════════════════════════════════════════════════════════════════
// Slider 滑条
// ═══════════════════════════════════════════════════════════════════

/// 渲染水平滑条
pub fn render_slider(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    w: i32,
    value: f32,   // 0.0 .. 1.0
    accent: (f32, f32, f32),
    focused: bool,
) {
    let track_y = y + 18;
    let track_h: i32 = 4;
    let handle_r: i32 = 6;

    // 轨道背景
    f.clear(
        opaque(0.1, 0.1, 0.18),
        &[rect(x, track_y, w, track_h)],
    )
    .ok();

    // 轨道已填充部分
    let fill_w = (w as f32 * value) as i32;
    if fill_w > 0 {
        f.clear(
            opaque(
                accent.0 * 0.6,
                accent.1 * 0.6,
                accent.2 * 0.6,
            ),
            &[rect(x, track_y, fill_w, track_h)],
        )
        .ok();
    }

    // Handle 圆点（用方块模拟）
    let hx = x + fill_w - handle_r;
    f.clear(
        opaque(accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9),
        &[rect(hx, track_y - 4, handle_r * 2, track_h + 8)],
    )
    .ok();

    // 聚焦发光
    if focused {
        // 多层 glow 边框（与 launcher 同款 5 层发光）
        glow_border(f, x - 4, y - 4, w + 8, 30, accent);
        // 左侧 accent 指示条
        f.clear(
            opaque(accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9),
            &[rect(x - 4, y - 4, 3, 32)],
        ).ok();
    }
}

// ═══════════════════════════════════════════════════════════════════
// Toggle 开关
// ═══════════════════════════════════════════════════════════════════

/// 渲染开关 toggle
pub fn render_toggle(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    on: bool,
    accent: (f32, f32, f32),
    focused: bool,
) -> i32 {
    let tw: i32 = 44;
    let th: i32 = 26;
    let r: i32 = th / 2;

    // 背景
    let bg = if on {
        opaque(accent.0 * 0.7, accent.1 * 0.7, accent.2 * 0.7)
    } else {
        opaque(0.12, 0.12, 0.20)
    };
    rounded_rect(f, x, y, tw, th, r, bg);

    // 滑块
    let knob_r = r - 4;
    let knob_x = if on {
        x + tw - knob_r * 2 - 4
    } else {
        x + 4
    };
    rounded_rect(
        f,
        knob_x,
        y + 4,
        knob_r * 2,
        th - 8,
        knob_r,
        opaque(0.95, 0.95, 0.98),
    );

    // 聚焦发光
    if focused {
        glow_border(f, x - 4, y - 4, tw + 8, th + 8, accent);
        // 左侧指示条
        f.clear(
            opaque(accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9),
            &[rect(x - 4, y - 4, 3, th + 8)],
        ).ok();
    }

    tw
}

// ═══════════════════════════════════════════════════════════════════
// Color Swatch 色块
// ═══════════════════════════════════════════════════════════════════

/// 渲染颜色色块
pub fn render_color_swatch(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    size: i32,
    hex: &str,
    label: &str,
    accent: (f32, f32, f32),
    focused: bool,
) {
    let (r, g, b) = parse_color(hex);

    // 色块本体
    f.clear(opaque(r, g, b), &[rect(x, y, size, size)]).ok();

    // 聚焦发光边框
    if focused {
        glow_border(f, x, y, size, size, accent);
        // 底部 accent 条
        f.clear(
            opaque(accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9),
            &[rect(x, y + size + 2, size, 2)],
        ).ok();
    } else {
        // 静态边框
        f.clear(
            opaque(0.15, 0.15, 0.25),
            &[rect(x - 1, y - 1, size + 2, 1)],
        ).ok();
        f.clear(
            opaque(0.15, 0.15, 0.25),
            &[rect(x - 1, y + size, size + 2, 1)],
        ).ok();
        f.clear(
            opaque(0.15, 0.15, 0.25),
            &[rect(x - 1, y, 1, size)],
        ).ok();
        f.clear(
            opaque(0.15, 0.15, 0.25),
            &[rect(x + size, y, 1, size)],
        ).ok();
    }

    // Label below — 聚焦时更亮
    let (lr, lg, lb) = if focused {
        (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9)
    } else {
        (0.5, 0.5, 0.65)
    };
    let lw = text_render::text_width(label, 11.0);
    text_render::draw_text(
        f,
        label,
        x + size / 2 - lw / 2,
        y + size + 6,
        11.0,
        (lr, lg, lb),
    );

    // Hex value below label
    let hw = text_render::text_width(hex, 10.0);
    text_render::draw_text(
        f,
        hex,
        x + size / 2 - hw / 2,
        y + size + 20,
        10.0,
        if focused { (lr * 0.7, lg * 0.7, lb * 0.7) } else { (0.3, 0.3, 0.45) },
    );
}

// ═══════════════════════════════════════════════════════════════════
// Button 按钮
// ═══════════════════════════════════════════════════════════════════

pub fn render_button(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    text: &str,
    accent: (f32, f32, f32),
    focused: bool,
    primary: bool,
) -> i32 {
    let tw = text_render::text_width(text, 13.0) + 24;
    let th: i32 = 32;

    // 背景
    let bg = if primary {
        opaque(accent.0 * 0.7, accent.1 * 0.7, accent.2 * 0.7)
    } else {
        opaque(0.08, 0.08, 0.14)
    };
    rounded_rect(f, x, y, tw, th, 6, bg);

    // 文本
    let tcolor = if primary {
        (0.98, 0.98, 0.99)
    } else {
        (accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8)
    };
    text_render::draw_text(
        f,
        text,
        x + 12,
        y + 8,
        13.0,
        tcolor,
    );

    // 聚焦发光
    if focused {
        glow_border(f, x, y, tw, th, accent);
    }

    tw
}

// ═══════════════════════════════════════════════════════════════════
// Checkbox 复选框
// ═══════════════════════════════════════════════════════════════════

pub fn render_checkbox(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    checked: bool,
    label: &str,
    accent: (f32, f32, f32),
    focused: bool,
) -> i32 {
    let box_size: i32 = 18;

    // 聚焦发光边框
    if focused {
        glow_border(f, x - 2, y - 2, box_size + 4, box_size + 4, accent);
    }

    // 边框
    let border_color = if focused {
        opaque(accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9)
    } else {
        opaque(0.2, 0.2, 0.35)
    };
    f.clear(
        border_color,
        &[rect(x, y, box_size, 1)],
    ).ok();
    f.clear(
        border_color,
        &[rect(x, y + box_size - 1, box_size, 1)],
    ).ok();
    f.clear(border_color, &[rect(x, y, 1, box_size)]).ok();
    f.clear(
        border_color,
        &[rect(x + box_size - 1, y, 1, box_size)],
    ).ok();

    // 填充（如果选中）
    if checked {
        f.clear(
            opaque(accent.0 * 0.7, accent.1 * 0.7, accent.2 * 0.7),
            &[rect(x + 3, y + 3, box_size - 6, box_size - 6)],
        ).ok();
    }

    // Label — 聚焦时更亮
    let label_color = if focused {
        (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9)
    } else {
        (0.7, 0.7, 0.85)
    };
    text_render::draw_text(
        f,
        label,
        x + box_size + 8,
        y + 2,
        13.0,
        label_color,
    );

    let lw = text_render::text_width(label, 13.0);
    box_size + 8 + lw
}

// ═══════════════════════════════════════════════════════════════════
// Section header
// ═══════════════════════════════════════════════════════════════════

pub fn render_section_header(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    title: &str,
    accent: (f32, f32, f32),
) -> i32 {
    // 装饰线
    f.clear(
        opaque(accent.0 * 0.4, accent.1 * 0.4, accent.2 * 0.4),
        &[rect(x, y + 10, 3, 14)],
    ).ok();

    text_render::draw_text(
        f,
        title,
        x + 10,
        y + 4,
        14.0,
        (accent.0 * 0.85, accent.1 * 0.85, accent.2 * 0.85),
    );

    // 下划线
    f.clear(
        opaque(0.08, 0.08, 0.16),
        &[rect(x, y + 26, 400, 1)],
    ).ok();

    32 // section height
}
