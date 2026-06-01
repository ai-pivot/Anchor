//! 启动器渲染（dmenu 风格：搜索框 + 应用列表 + 选中高亮）

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
    let lh = header_h + (n as i32) * item_h + 16;
    let lx = (ow - lw) / 2;
    let ly = bar_h + 20;

    // 半透明暗色背景
    let bg = opaque(0.04, 0.04, 0.08);
    f.clear(bg, &[rect(lx, ly, lw, lh)]).ok();

    // 边框
    let border = opaque(accent.0, accent.1, accent.2);
    f.clear(border, &[rect(lx, ly, lw, 3)]).ok();
    f.clear(border, &[rect(lx, ly + lh - 3, lw, 3)]).ok();
    f.clear(border, &[rect(lx, ly, 3, lh)]).ok();
    f.clear(border, &[rect(lx + lw - 3, ly, 3, lh)]).ok();

    // 搜索框背景
    let search_bg = opaque(0.08, 0.08, 0.14);
    f.clear(search_bg, &[rect(lx + 8, ly + 8, lw - 16, 32)]).ok();

    // ">" 提示符
    text_render::draw_text(f, ">", lx + 14, ly + 14, 20.0, (accent.0, accent.1, accent.2));

    // 搜索文字
    let display_query = if query.is_empty() { "Type to search...".to_string() } else { query.to_string() };
    let text_color = if query.is_empty() { (0.4f32, 0.4, 0.5) } else { (0.9f32, 0.9, 0.95) };
    text_render::draw_text(f, &display_query, lx + 36, ly + 14, 18.0, text_color);

    // 光标
    if !query.is_empty() {
        let cursor_x = lx + 36 + (query.len() as i32) * 10; // 近似
        f.clear(opaque(0.8, 0.8, 0.9), &[rect(cursor_x, ly + 14, 2, 20)]).ok();
    }

    // 应用列表
    for (i, (_, (name, _exec))) in filtered.iter().take(max_items).enumerate() {
        let iy = ly + header_h + (i as i32) * item_h;

        // 选中项高亮
        if i == selected {
            f.clear(opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
                &[rect(lx + 4, iy, lw - 8, item_h)]).ok();
            // 左侧指示条
            f.clear(border, &[rect(lx + 4, iy + 4, 3, item_h - 8)]).ok();
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0, (1.0, 1.0, 1.0));
        } else {
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0, (0.7, 0.7, 0.75));
        }
    }

    // 底部信息
    let info = format!("{} / {} apps", filtered.len(), filtered.len());
    text_render::draw_text(f, &info, lx + 12, ly + lh - 18, 12.0, (0.4, 0.4, 0.5));
}
