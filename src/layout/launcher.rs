//! 启动器渲染 — 赛博朋克发光风格（与锁屏/headbar一致）
//! 居中面板 + 发光边框 + accent 高亮 + 选中项动画指示条

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

    // ── 全屏半透明遮罩（突出启动器）──
    f.clear(opaque(0.0, 0.0, 0.02), &[rect(0, bar_h, ow, oh - bar_h)]).ok();

    // ── 面板背景 ──
    f.clear(opaque(0.04, 0.04, 0.08), &[rect(lx, ly, lw, lh)]).ok();

    // ── 发光边框（多层渐变，赛博朋克风格）──
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
    f.clear(opaque(0.06, 0.06, 0.12), &[rect(lx + 8, search_y, lw - 16, search_h)]).ok();
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
            // 选中项 — accent 背景条 + 左侧指示条
            f.clear(opaque(accent.0 * 0.1, accent.1 * 0.1, accent.2 * 0.1),
                &[rect(lx + 4, iy + 2, lw - 8, item_h - 4)]).ok();
            // 左侧 accent 竖条（赛博朋克指示条）
            f.clear(opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
                &[rect(lx + 4, iy + 4, 3, item_h - 8)]).ok();
            // 左侧竖条发光
            f.clear(opaque(accent.0 * 0.2, accent.1 * 0.2, accent.2 * 0.2),
                &[rect(lx + 7, iy + 4, 3, item_h - 8)]).ok();
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0,
                (accent.0 * 0.95, accent.1 * 0.95, accent.2 * 0.95));
        } else {
            // 普通项 — 暗色背景 + 低亮文字
            f.clear(opaque(0.02, 0.02, 0.04),
                &[rect(lx + 4, iy + 2, lw - 8, item_h - 4)]).ok();
            text_render::draw_text(f, name, lx + 20, iy + 8, 16.0,
                (accent.0 * 0.45, accent.1 * 0.45, accent.2 * 0.45));
        }
    }

    // ── 底部信息栏 ──
    // 底部 accent 分隔线
    f.clear(opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
        &[rect(lx + 8, ly + lh - 18, lw - 16, 1)]).ok();

    let info = format!("{} apps", filtered.len());
    let iw = text_render::text_width(&info, 12.0);
    text_render::draw_text(f, &info, lx + 12, ly + lh - 14, 12.0,
        (accent.0 * 0.25, accent.1 * 0.25, accent.2 * 0.25));

    // 右下角快捷键提示
    let hint = "↑↓ Navigate  Enter Launch  Esc Close";
    let hw = text_render::text_width(hint, 11.0);
    text_render::draw_text(f, hint, lx + lw - hw - 12, ly + lh - 14, 11.0,
        (accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15));
}
