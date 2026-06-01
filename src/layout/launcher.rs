//! 启动器渲染 — 毛玻璃半透明风格
//! 半透明叠加 + 微网格纹理 + 发光边框 + accent 高亮

use crate::config::{parse_color, Config};
use crate::text_render;
use smithay::backend::renderer::Frame;
use super::util::{opaque, rect};

/// 渲染内置启动器
pub fn render_launcher(
    f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32,
    query: &str, filtered: &[(usize, &(String, String))], selected: usize,
) {
    let accent = parse_color(&cfg.colors.focus_border);
    let bar_h = cfg.bar.height;

    // 启动器定位：居中，上半部分
    let lw = ow * 3 / 4;
    let max_items = 12usize;
    let item_h: i32 = 36;
    let header_h: i32 = 48;
    let n = filtered.len().min(max_items);
    let lh = header_h + (n as i32) * item_h + 20;
    let lx = (ow - lw) / 2;
    let ly = bar_h + 24;

    // ── 全屏遮罩（半透明深色，模拟背景暗化但不是纯黑）──
    f.clear(opaque(0.01, 0.01, 0.03), &[rect(0, bar_h, ow, oh - bar_h)]).ok();

    // ── 毛玻璃背景：多层半透明叠加 ──
    // 底层：较深的半透明
    f.clear(opaque(0.06, 0.06, 0.10), &[rect(lx, ly, lw, lh)]).ok();
    // 上层：略亮半透明（模拟模糊后的背景亮度）
    f.clear(opaque(0.02, 0.02, 0.04), &[rect(lx, ly, lw, lh)]).ok();

    // ── 微网格纹理（模拟毛玻璃颗粒感）──
    let grid_step = 24;
    let grid_color = opaque(accent.0 * 0.015, accent.1 * 0.015, accent.2 * 0.015);
    let mut grid_rects: Vec<smithay::utils::Rectangle<i32, smithay::utils::Physical>> = Vec::new();
    for gy in (ly..ly + lh).step_by(grid_step) {
        grid_rects.push(rect(lx, gy, lw, 1));
    }
    for gx in (lx..lx + lw).step_by(grid_step) {
        grid_rects.push(rect(gx, ly, 1, lh));
    }
    if !grid_rects.is_empty() {
        f.clear(grid_color, &grid_rects).ok();
    }

    // ── 发光边框（多层渐变）──
    let glow_layers: [(i32, f32); 5] = [
        (5, 0.03), (4, 0.06), (3, 0.12), (2, 0.25), (1, 0.5),
    ];
    for (expand, brightness) in glow_layers {
        let glow = opaque(accent.0 * brightness, accent.1 * brightness, accent.2 * brightness);
        f.clear(glow, &[rect(lx - expand, ly - expand, lw + 2 * expand, expand)]).ok(); // top
        f.clear(glow, &[rect(lx - expand, ly + lh, lw + 2 * expand, expand)]).ok(); // bottom
        f.clear(glow, &[rect(lx - expand, ly, expand, lh)]).ok(); // left
        f.clear(glow, &[rect(lx + lw, ly, expand, lh)]).ok(); // right
    }

    // 顶部 accent 亮线
    f.clear(opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
        &[rect(lx, ly, lw, 2)]).ok();

    // ── 搜索框 ──
    let search_y = ly + 8;
    let search_h = 32;
    f.clear(opaque(0.04, 0.04, 0.08), &[rect(lx + 8, search_y, lw - 16, search_h)]).ok();
    // 搜索框底部 accent 线
    f.clear(opaque(accent.0 * 0.4, accent.1 * 0.4, accent.2 * 0.4),
        &[rect(lx + 8, search_y + search_h - 2, lw - 16, 2)]).ok();

    // ">" 提示符
    text_render::draw_text(f, ">", lx + 16, search_y + 6, 20.0, accent);

    // 搜索文字
    if query.is_empty() {
        text_render::draw_text(f, "Type to search...", lx + 38, search_y + 7, 18.0,
            (accent.0 * 0.2, accent.1 * 0.2, accent.2 * 0.2));
    } else {
        text_render::draw_text(f, query, lx + 38, search_y + 7, 18.0,
            (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9));
        // 光标
        let cursor_x = lx + 38 + text_render::text_width(query, 18.0);
        f.clear(opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
            &[rect(cursor_x, search_y + 6, 2, 20)]).ok();
    }

    // ── 应用列表 ──
    for (i, (_, (name, _exec))) in filtered.iter().take(max_items).enumerate() {
        let iy = ly + header_h + (i as i32) * item_h;

        if i == selected {
            // 选中项 — accent 背景 + 左侧指示条
            f.clear(opaque(accent.0 * 0.08, accent.1 * 0.08, accent.2 * 0.08),
                &[rect(lx + 4, iy + 2, lw - 8, item_h - 4)]).ok();
            f.clear(opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
                &[rect(lx + 4, iy + 4, 3, item_h - 8)]).ok();
            f.clear(opaque(accent.0 * 0.2, accent.1 * 0.2, accent.2 * 0.2),
                &[rect(lx + 7, iy + 4, 3, item_h - 8)]).ok();
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0,
                (accent.0 * 0.95, accent.1 * 0.95, accent.2 * 0.95));
        } else {
            // 普通项
            f.clear(opaque(0.015, 0.015, 0.03),
                &[rect(lx + 4, iy + 2, lw - 8, item_h - 4)]).ok();
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0,
                (accent.0 * 0.45, accent.1 * 0.45, accent.2 * 0.45));
        }
    }

    // ── 底部信息栏 ──
    f.clear(opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
        &[rect(lx + 8, ly + lh - 18, lw - 16, 1)]).ok();

    let info = format!("{} apps", filtered.len());
    text_render::draw_text(f, &info, lx + 12, ly + lh - 14, 12.0,
        (accent.0 * 0.25, accent.1 * 0.25, accent.2 * 0.25));

    let hint = "↑↓ Navigate  Enter Launch  Esc Close";
    let hw = text_render::text_width(hint, 11.0);
    text_render::draw_text(f, hint, lx + lw - hw - 12, ly + lh - 14, 11.0,
        (accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15));
}
