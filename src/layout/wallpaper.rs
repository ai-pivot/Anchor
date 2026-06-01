//! 壁纸渲染（背景填充 + 网格线 + 闪烁点 + 动态光斑）

use crate::config::{parse_color, Config};
use smithay::{
    backend::renderer::Frame,
    utils::{Physical, Rectangle, Size},
};
use super::util::{color_hex, opaque, rect};

pub fn render_wallpaper(f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32, frame: u32) {
    f.clear(color_hex(&cfg.wallpaper.color), &[Rectangle::from_size(Size::new(ow, oh))]).ok();

    let accent = parse_color(&cfg.colors.focus_border);

    // Batch grid lines: one draw call for all horizontal, one for all vertical
    let grid = opaque(accent.0 * 0.03, accent.1 * 0.03, accent.2 * 0.03);
    let h_lines: Vec<Rectangle<i32, Physical>> = (0..oh).step_by(64)
        .map(|y| rect(0, y, ow, 1)).collect();
    let v_lines: Vec<Rectangle<i32, Physical>> = (0..ow).step_by(64)
        .map(|x| rect(x, 0, 1, oh)).collect();
    if !h_lines.is_empty() { f.clear(grid, &h_lines).ok(); }
    if !v_lines.is_empty() { f.clear(grid, &v_lines).ok(); }

    // Batch all dots into a single draw call
    let dot = opaque(accent.0 * 0.05, accent.1 * 0.05, accent.2 * 0.05);
    let dots: Vec<Rectangle<i32, Physical>> = (0..oh).step_by(64)
        .flat_map(|y| (0..ow).step_by(64).map(move |x| rect(x, y, 2, 2)))
        .collect();
    if !dots.is_empty() { f.clear(dot, &dots).ok(); }

    // Animated glow spots (6 calls — fine)
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
