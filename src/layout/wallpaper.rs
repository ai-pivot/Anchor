//! 壁纸渲染（背景填充 + 网格线 + 闪烁点 + 动态光斑）

use super::util::{color_hex, opaque, rect};
use crate::config::{parse_color, Config};
use smithay::{
    backend::renderer::Frame,
    utils::{Physical, Rectangle, Size},
};

pub fn render_wallpaper(f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32, frame: u32, hour: u8) {
    f.clear(
        color_hex(&cfg.wallpaper.color),
        &[Rectangle::from_size(Size::new(ow, oh))],
    )
    .ok();

    let accent = parse_color(&cfg.colors.focus_border);

    // ── 时间相关的色调叠加 ──
    // 0-6: 深夜蓝调, 6-12: 暖橙, 12-18: 正常, 18-24: 冷蓝
    let (warm_r, warm_g, warm_b) = if hour < 6 {
        (0.0, 0.0, 0.005)
    } else if hour < 12 {
        (0.006, 0.003, 0.0)
    } else if hour < 18 {
        (0.0, 0.0, 0.0)
    } else {
        (0.0, 0.0, 0.004)
    };
    if warm_r > 0.0 || warm_g > 0.0 || warm_b > 0.0 {
        f.clear(
            opaque(warm_r, warm_g, warm_b),
            &[Rectangle::from_size(Size::new(ow, oh))],
        ).ok();
    }

    // Batch grid lines: one draw call for all horizontal, one for all vertical
    let grid = opaque(accent.0 * 0.03, accent.1 * 0.03, accent.2 * 0.03);
    let h_lines: Vec<Rectangle<i32, Physical>> =
        (0..oh).step_by(48).map(|y| rect(0, y, ow, 1)).collect();
    let v_lines: Vec<Rectangle<i32, Physical>> =
        (0..ow).step_by(48).map(|x| rect(x, 0, 1, oh)).collect();
    if !h_lines.is_empty() {
        f.clear(grid, &h_lines).ok();
    }
    if !v_lines.is_empty() {
        f.clear(grid, &v_lines).ok();
    }

    // Batch all dots into a single draw call
    let dot = opaque(accent.0 * 0.05, accent.1 * 0.05, accent.2 * 0.05);
    let dots: Vec<Rectangle<i32, Physical>> = (0..oh)
        .step_by(48)
        .flat_map(|y| (0..ow).step_by(48).map(move |x| rect(x, y, 2, 2)))
        .collect();
    if !dots.is_empty() {
        f.clear(dot, &dots).ok();
    }

    // 全局呼吸脉冲（微妙的全屏亮度变化）
    let breathe = 0.008 + 0.004 * (frame as f32 * 0.008).sin();
    f.clear(
        opaque(accent.0 * breathe, accent.1 * breathe, accent.2 * breathe),
        &[rect(0, 0, ow, oh)],
    ).ok();

    // Animated glow spots (6 calls — fine)
    let t = frame as f32 * 0.012;
    let spots: [(f32, f32, f32, f32, i32, f32); 5] = [
        (t.sin(), t.cos(), 0.5, 0.5, 180, 0.04),
        (
            (t * 0.6 + 2.1).sin(),
            (t * 0.6 + 2.1).cos(),
            0.25,
            0.65,
            140, 0.03,
        ),
        (
            (t * 0.4 + 4.2).sin(),
            (t * 0.4 + 4.2).cos(),
            0.75,
            0.35,
            100, 0.025,
        ),
        // 新增：更多光斑，营造更动感的效果
        (
            (t * 0.8 + 1.0).cos(),
            (t * 0.5 + 3.0).sin(),
            0.6,
            0.7,
            80, 0.02,
        ),
        (
            (t * 0.3 + 5.0).sin(),
            (t * 0.7 + 1.5).cos(),
            0.15,
            0.4,
            110, 0.025,
        ),
    ];
    for (sx, sy, cx, cy, size, brightness) in spots {
        let px = (sx * 300.0 + ow as f32 * cx) as i32;
        let py = (sy * 200.0 + oh as f32 * cy) as i32;
        f.clear(
            opaque(
                accent.0 * brightness,
                accent.1 * brightness,
                accent.2 * brightness,
            ),
            &[rect(px - size / 2, py - size / 2, size, size)],
        )
        .ok();
        let inner = size / 4;
        f.clear(
            opaque(
                accent.0 * brightness * 2.5,
                accent.1 * brightness * 2.5,
                accent.2 * brightness * 2.5,
            ),
            &[rect(px - inner / 2, py - inner / 2, inner, inner)],
        )
        .ok();
    }
}
