//! 启动器渲染 — 只渲染 UI 元素（背景/毛玻璃由 main.rs 在调用前完成）
//! 发光边框 + 搜索框 + 应用列表 + 底部信息栏

use super::util::{opaque, rect};
use crate::config::{parse_color, Config};
use crate::text_render;
use smithay::backend::renderer::Frame;

/// 渲染内置启动器（背景已由调用方渲染为毛玻璃纹理）
pub fn render_launcher(
    f: &mut impl Frame,
    cfg: &Config,
    ow: i32,
    oh: i32,
    query: &str,
    filtered: &[(usize, &(String, String))],
    selected: usize,
    frame: u32,
) {
    let accent = parse_color(&cfg.colors.focus_border);
    let bar_h = cfg.bar.height;

    let lw = ow * 3 / 4;
    let max_items = 12usize;
    let item_h: i32 = 36;
    let header_h: i32 = 48;
    let n = filtered.len().min(max_items);
    let lh = header_h + (n as i32) * item_h + 20;
    let lx = (ow - lw) / 2;
    let ly = bar_h + 24;

    // ── 发光边框 ──
    let glow_layers: [(i32, f32); 6] = [
        (6, 0.02),
        (5, 0.04),
        (4, 0.08),
        (3, 0.15),
        (2, 0.30),
        (1, 0.55),
    ];
    for (expand, brightness) in glow_layers {
        let glow = opaque(
            accent.0 * brightness,
            accent.1 * brightness,
            accent.2 * brightness,
        );
        f.clear(
            glow,
            &[rect(lx - expand, ly - expand, lw + 2 * expand, expand)],
        )
        .ok();
        f.clear(glow, &[rect(lx - expand, ly + lh, lw + 2 * expand, expand)])
            .ok();
        f.clear(glow, &[rect(lx - expand, ly, expand, lh)]).ok();
        f.clear(glow, &[rect(lx + lw, ly, expand, lh)]).ok();
    }

    // 顶部 accent 亮线
    f.clear(
        opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
        &[rect(lx, ly, lw, 2)],
    )
    .ok();

    // ── 搜索框 ──
    let search_y = ly + 8;
    let search_h = 32;
    f.clear(
        opaque(0.05, 0.05, 0.09),
        &[rect(lx + 8, search_y, lw - 16, search_h)],
    )
    .ok();
    f.clear(
        opaque(accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
        &[rect(lx + 8, search_y + search_h - 2, lw - 16, 2)],
    )
    .ok();

    text_render::draw_text(f, ">", lx + 16, search_y + 6, 20.0, accent);

    if query.is_empty() {
        // 闪烁光标
        let cursor_alpha = 0.3 + 0.7 * (frame as f32 * 0.06).sin().max(0.0);
        f.clear(
            opaque(
                accent.0 * cursor_alpha,
                accent.1 * cursor_alpha,
                accent.2 * cursor_alpha,
            ),
            &[rect(lx + 38, search_y + 8, 2, search_h - 16)],
        )
        .ok();
        text_render::draw_text(
            f,
            "Type to search...",
            lx + 38,
            search_y + 7,
            18.0,
            (accent.0 * 0.25, accent.1 * 0.25, accent.2 * 0.25),
        );
    } else {
        text_render::draw_text(
            f,
            query,
            lx + 38,
            search_y + 7,
            18.0,
            (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9),
        );
        let cursor_x = lx + 38 + text_render::text_width(query, 18.0);
        f.clear(
            opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
            &[rect(cursor_x, search_y + 6, 2, 20)],
        )
        .ok();
    }

    // ── 应用列表 ──
    for (i, (_, (name, _exec))) in filtered.iter().take(max_items).enumerate() {
        let iy = ly + header_h + (i as i32) * item_h;

        if i == selected {
            f.clear(
                opaque(accent.0 * 0.12, accent.1 * 0.12, accent.2 * 0.12),
                &[rect(lx + 4, iy + 2, lw - 8, item_h - 4)],
            )
            .ok();
            f.clear(
                opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
                &[rect(lx + 4, iy + 4, 3, item_h - 8)],
            )
            .ok();
            text_render::draw_text(
                f,
                name,
                lx + 20,
                iy + 8,
                16.0,
                (accent.0 * 0.95, accent.1 * 0.95, accent.2 * 0.95),
            );
        } else {
            text_render::draw_text(
                f,
                name,
                lx + 20,
                iy + 8,
                16.0,
                (accent.0 * 0.6, accent.1 * 0.6, accent.2 * 0.6),
            );
        }
    }

    // ── 底部信息栏 ──
    f.clear(
        opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
        &[rect(lx + 8, ly + lh - 18, lw - 16, 1)],
    )
    .ok();

    let info = format!("{} apps", filtered.len());
    text_render::draw_text(
        f,
        &info,
        lx + 12,
        ly + lh - 14,
        12.0,
        (accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
    );

    let hint = "↑↓ Navigate  Enter Launch  Esc Close";
    let hw = text_render::text_width(hint, 11.0);
    text_render::draw_text(
        f,
        hint,
        lx + lw - hw - 12,
        ly + lh - 14,
        11.0,
        (accent.0 * 0.2, accent.1 * 0.2, accent.2 * 0.2),
    );
}
