//! 启动器渲染 — 深色半透明风格
//! 深蓝背景（非纯黑） + 网格纹理 + 发光边框 + accent 高亮

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

    // ── 不清除全屏！壁纸和窗口已渲染好，直接在上面叠加面板 ──
    // 桌面内容透过来就是"毛玻璃"效果（无需真正的模糊）

    // ── 面板背景（比遮罩略亮，明显可见的深蓝）──
    f.clear(opaque(0.10, 0.10, 0.16), &[rect(lx, ly, lw, lh)]).ok();

    // ── 网格纹理（可见的细线网格，增加层次感）──
    let grid_step = 32;
    let grid_color = opaque(accent.0 * 0.04, accent.1 * 0.04, accent.2 * 0.04);
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

    // ── 发光边框（5 层渐变，accent 色）──
    let glow_layers: [(i32, f32); 6] = [
        (6, 0.02), (5, 0.04), (4, 0.08), (3, 0.15), (2, 0.30), (1, 0.55),
    ];
    for (expand, brightness) in glow_layers {
        let glow = opaque(accent.0 * brightness, accent.1 * brightness, accent.2 * brightness);
        f.clear(glow, &[rect(lx - expand, ly - expand, lw + 2 * expand, expand)]).ok();
        f.clear(glow, &[rect(lx - expand, ly + lh, lw + 2 * expand, expand)]).ok();
        f.clear(glow, &[rect(lx - expand, ly, expand, lh)]).ok();
        f.clear(glow, &[rect(lx + lw, ly, expand, lh)]).ok();
    }

    // 顶部 accent 亮线
    f.clear(opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
        &[rect(lx, ly, lw, 2)]).ok();

    // ── 搜索框 ──
    let search_y = ly + 8;
    let search_h = 32;
    f.clear(opaque(0.06, 0.06, 0.11), &[rect(lx + 8, search_y, lw - 16, search_h)]).ok();
    f.clear(opaque(accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
        &[rect(lx + 8, search_y + search_h - 2, lw - 16, 2)]).ok();

    text_render::draw_text(f, ">", lx + 16, search_y + 6, 20.0, accent);

    if query.is_empty() {
        text_render::draw_text(f, "Type to search...", lx + 38, search_y + 7, 18.0,
            (accent.0 * 0.25, accent.1 * 0.25, accent.2 * 0.25));
    } else {
        text_render::draw_text(f, query, lx + 38, search_y + 7, 18.0,
            (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9));
        let cursor_x = lx + 38 + text_render::text_width(query, 18.0);
        f.clear(opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
            &[rect(cursor_x, search_y + 6, 2, 20)]).ok();
    }

    // ── 应用列表 ──
    for (i, (_, (name, _exec))) in filtered.iter().take(max_items).enumerate() {
        let iy = ly + header_h + (i as i32) * item_h;

        if i == selected {
            f.clear(opaque(accent.0 * 0.12, accent.1 * 0.12, accent.2 * 0.12),
                &[rect(lx + 4, iy + 2, lw - 8, item_h - 4)]).ok();
            f.clear(opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
                &[rect(lx + 4, iy + 4, 3, item_h - 8)]).ok();
            f.clear(opaque(accent.0 * 0.25, accent.1 * 0.25, accent.2 * 0.25),
                &[rect(lx + 7, iy + 4, 3, item_h - 8)]).ok();
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0,
                (accent.0 * 0.95, accent.1 * 0.95, accent.2 * 0.95));
        } else {
            f.clear(opaque(0.03, 0.03, 0.06),
                &[rect(lx + 4, iy + 2, lw - 8, item_h - 4)]).ok();
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0,
                (accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5));
        }
    }

    // ── 底部信息栏 ──
    f.clear(opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
        &[rect(lx + 8, ly + lh - 18, lw - 16, 1)]).ok();

    let info = format!("{} apps", filtered.len());
    text_render::draw_text(f, &info, lx + 12, ly + lh - 14, 12.0,
        (accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3));

    let hint = "↑↓ Navigate  Enter Launch  Esc Close";
    let hw = text_render::text_width(hint, 11.0);
    text_render::draw_text(f, hint, lx + lw - hw - 12, ly + lh - 14, 11.0,
        (accent.0 * 0.2, accent.1 * 0.2, accent.2 * 0.2));
}
