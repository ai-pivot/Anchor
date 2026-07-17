//! Settings Panel 渲染
//!
//! 由 main.rs 渲染管线条目调用，绘制完整的 Settings overlay。
//! 包含：半透明遮罩 → 面板背景 → 侧栏 → 内容区 → 标题栏 → 底部栏

use smithay::backend::renderer::Frame;
use smithay::utils::{Physical, Point, Rectangle, Size};

use crate::config::{parse_color, Config};
use crate::text_render;

use super::widgets::{
    glow_border, opaque, rect, render_checkbox, render_color_swatch, render_section_header,
    render_slider, render_toggle, rounded_rect,
};
use super::{SettingsEdit, SettingsState, SettingsTab};

// ═══════════════════════════════════════════════════════════════════
// 面板渲染入口 — 由 main.rs 的 Step 7.5 调用
// ═══════════════════════════════════════════════════════════════════

pub fn render_settings_panel(
    f: &mut impl Frame,
    cfg: &Config,
    ow: i32,
    oh: i32,
    settings: &SettingsState,
    frame: u32,
) {
    let progress = settings.progress();
    if progress < 0.01 {
        return;
    }

    let accent = parse_color(&cfg.colors.focus_border);
    let bar_h = if cfg.bar.enabled { cfg.bar.height } else { 0 };

    // 面板尺寸（屏幕的 72% × 78%）
    let panel_w = (ow as f32 * 0.72) as i32;
    let panel_h = (oh as f32 * 0.78) as i32;
    let panel_x = (ow - panel_w) / 2;
    let panel_y = (oh - panel_h) / 2;

    // 动画缩放
    let scale = progress as f32;
    let scaled_w = (panel_w as f32 * (0.92 + 0.08 * scale)) as i32;
    let scaled_h = (panel_h as f32 * (0.92 + 0.08 * scale)) as i32;
    let sx = (ow - scaled_w) / 2;
    let sy = (oh - scaled_h) / 2;
    let alpha = (progress as f32).min(1.0);

    // ── 全屏半透明遮罩 ──
    f.clear(
        smithay::backend::renderer::Color32F::new(0.0, 0.0, 0.0, 0.55 * alpha),
        &[rect(0, 0, ow, oh)],
    )
    .ok();

    // ── 面板背景 ──
    let panel_bg = smithay::backend::renderer::Color32F::new(0.08, 0.08, 0.14, alpha);
    rounded_rect(f, sx, sy, scaled_w, scaled_h, 12, panel_bg);

    // ── 发光边框 ──
    glow_border(f, sx, sy, scaled_w, scaled_h, accent);

    // ── 面板内布局 ──
    let sidebar_w: i32 = 180;
    let title_h: i32 = 52;
    let bottom_h: i32 = 48;
    let pad: i32 = 16;

    // 标题栏
    render_title_bar(f, sx, sy, scaled_w, title_h, accent, alpha);

    // 侧栏
    let sidebar_x = sx + pad;
    let sidebar_y = sy + title_h + pad;
    let sidebar_real_h = scaled_h - title_h - bottom_h - pad * 2;
    render_sidebar(
        f,
        sidebar_x,
        sidebar_y,
        sidebar_w,
        sidebar_real_h,
        settings.tab(),
        accent,
        alpha,
    );

    // 分隔线（侧栏 | 内容区）
    let sep_x = sidebar_x + sidebar_w + pad;
    f.clear(
        smithay::backend::renderer::Color32F::new(
            accent.0 * 0.1,
            accent.1 * 0.1,
            accent.2 * 0.1,
            alpha,
        ),
        &[rect(sep_x, sidebar_y - 8, 1, sidebar_real_h + 8)],
    )
    .ok();

    // 内容区
    let content_x = sep_x + pad;
    let content_y = sidebar_y;
    let content_w = sx + scaled_w - content_x - pad;
    let content_h = scaled_h - title_h - bottom_h - pad * 2;

    // 裁剪（只渲染内容区内的元素）
    // 注意：Frame::clear 不支持裁剪，但我们可以通过不画外部元素来模拟
    render_content(
        f, cfg, content_x, content_y, content_w, content_h, settings, accent, alpha, frame,
    );

    // 底部栏
    render_bottom_bar(
        f,
        sx,
        sy + scaled_h - bottom_h,
        scaled_w,
        bottom_h,
        settings,
        accent,
        alpha,
    );
}

// ═══════════════════════════════════════════════════════════════════
// 标题栏
// ═══════════════════════════════════════════════════════════════════

fn render_title_bar(
    f: &mut impl Frame,
    px: i32,
    py: i32,
    pw: i32,
    h: i32,
    accent: (f32, f32, f32),
    alpha: f32,
) {
    // 标题文字
    let title = "◈ Anchor Settings";
    let tw = text_render::text_width(title, 16.0);
    text_render::draw_text(
        f,
        title,
        px + 20,
        py + 14,
        16.0,
        (
            accent.0 * 0.9 * alpha,
            accent.1 * 0.9 * alpha,
            accent.2 * 0.9 * alpha,
        ),
    );

    // 底部亮线
    f.clear(
        smithay::backend::renderer::Color32F::new(
            accent.0 * 0.15,
            accent.1 * 0.15,
            accent.2 * 0.15,
            alpha,
        ),
        &[rect(px + 16, py + h - 2, pw - 32, 1)],
    )
    .ok();

    // 关闭提示
    let hint = "Esc to close";
    let hw = text_render::text_width(hint, 11.0);
    text_render::draw_text(
        f,
        hint,
        px + pw - hw - 20,
        py + 16,
        11.0,
        (0.25 * alpha, 0.25 * alpha, 0.4 * alpha),
    );
}

// ═══════════════════════════════════════════════════════════════════
// 侧栏
// ═══════════════════════════════════════════════════════════════════

fn render_sidebar(
    f: &mut impl Frame,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    active_tab: SettingsTab,
    accent: (f32, f32, f32),
    alpha: f32,
) {
    let item_h: i32 = 36;
    let all_tabs = SettingsTab::all();

    for (i, tab) in all_tabs.iter().enumerate() {
        let iy = y + (i as i32) * (item_h + 2);

        if *tab == active_tab {
            // 激活项：鲜明 accent 背景 + 厚指示条
            f.clear(
                smithay::backend::renderer::Color32F::new(
                    accent.0 * 0.18,
                    accent.1 * 0.18,
                    accent.2 * 0.18,
                    alpha,
                ),
                &[rect(x + 4, iy + 2, w - 8, item_h - 4)],
            )
            .ok();
            // 左侧粗指示条
            f.clear(
                smithay::backend::renderer::Color32F::new(
                    accent.0 * 0.95,
                    accent.1 * 0.95,
                    accent.2 * 0.95,
                    alpha,
                ),
                &[rect(x, iy + 4, 4, item_h - 8)],
            )
            .ok();

            let label = format!("{}  {}", tab.icon(), tab.name());
            text_render::draw_text(
                f,
                &label,
                x + 16,
                iy + 8,
                14.0,
                (1.0 * alpha, 1.0 * alpha, 1.0 * alpha),
            );
        } else {
            let label = format!("  {}", tab.name());
            text_render::draw_text(
                f,
                &label,
                x + 16,
                iy + 8,
                13.0,
                (0.30 * alpha, 0.30 * alpha, 0.45 * alpha),
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// 底部状态栏
// ═══════════════════════════════════════════════════════════════════

fn render_bottom_bar(
    f: &mut impl Frame,
    px: i32,
    py: i32,
    pw: i32,
    h: i32,
    settings: &SettingsState,
    accent: (f32, f32, f32),
    alpha: f32,
) {
    // 顶部亮线
    f.clear(
        smithay::backend::renderer::Color32F::new(
            accent.0 * 0.15,
            accent.1 * 0.15,
            accent.2 * 0.15,
            alpha,
        ),
        &[rect(px + 16, py, pw - 32, 1)],
    )
    .ok();

    // 保存反馈动画
    if matches!(settings, SettingsState::Saving { .. }) {
        let t = settings.progress() as f32;
        let flash = 1.0 - (t * 3.0).min(1.0); // 快速闪烁
        let green = flash * alpha;
        text_render::draw_text(
            f,
            "✓ Saved",
            px + 20,
            py + 16,
            14.0,
            (0.2 * green, 0.9 * green, 0.3 * green),
        );
        return;
    }

    // 状态信息
    if let Some(edit) = settings.edit() {
        if edit.dirty {
            // 未保存的修改指示
            let dot_color = smithay::backend::renderer::Color32F::new(
                accent.0 * 0.8,
                accent.1 * 0.8,
                accent.2 * 0.8,
                alpha,
            );
            f.clear(dot_color, &[rect(px + 16, py + 14, 8, 8)]).ok();
            text_render::draw_text(
                f,
                "Unsaved changes",
                px + 32,
                py + 14,
                12.0,
                (
                    accent.0 * 0.6 * alpha,
                    accent.1 * 0.6 * alpha,
                    accent.2 * 0.6 * alpha,
                ),
            );
        }
    }

    // 当前聚焦控件名
    if let Some(edit) = settings.edit() {
        let focus_name = focus_label(settings.tab(), edit.focus_idx);
        if !focus_name.is_empty() {
            text_render::draw_text(
                f,
                &format!("\u{2190}\u{2192} {}", focus_name), // ←→ label
                px + 200,
                py + 14,
                12.0,
                (
                    accent.0 * 0.7 * alpha,
                    accent.1 * 0.7 * alpha,
                    accent.2 * 0.7 * alpha,
                ),
            );
        }
    }

    // 操作提示
    let hint = "Enter:Toggle \u{2190}\u{2192}:Adjust  Ctrl+Enter:Apply  Esc:Close";
    let hw = text_render::text_width(hint, 11.0);
    text_render::draw_text(
        f,
        hint,
        px + pw - hw - 20,
        py + 14,
        11.0,
        (0.2 * alpha, 0.2 * alpha, 0.35 * alpha),
    );
}

// ═══════════════════════════════════════════════════════════════════
// 内容区路由
// ═══════════════════════════════════════════════════════════════════

fn render_content(
    f: &mut impl Frame,
    cfg: &Config,
    cx: i32,
    cy: i32,
    cw: i32,
    ch: i32,
    settings: &SettingsState,
    accent: (f32, f32, f32),
    alpha: f32,
    frame: u32,
) {
    let tab = settings.tab();
    let edit = match settings.edit() {
        Some(e) => e,
        None => return,
    };

    match tab {
        SettingsTab::Colors => {
            render_page_colors(f, cfg, cx, cy, cw, ch, edit, accent, alpha, frame)
        }
        SettingsTab::Layout => render_page_layout(f, cfg, cx, cy, cw, ch, edit, accent, alpha),
        SettingsTab::Bar => render_page_bar(f, cfg, cx, cy, cw, ch, edit, accent, alpha),
        SettingsTab::Wallpaper => {
            render_page_wallpaper(f, cfg, cx, cy, cw, ch, edit, accent, alpha)
        }
        _ => render_page_placeholder(f, cx, cy, cw, ch, tab, accent, alpha),
    }
}

// ═══════════════════════════════════════════════════════════════════
// Colors 外观页
// ═══════════════════════════════════════════════════════════════════

fn render_page_colors(
    f: &mut impl Frame,
    cfg: &Config,
    cx: i32,
    cy: i32,
    _cw: i32,
    _ch: i32,
    edit: &SettingsEdit,
    accent: (f32, f32, f32),
    alpha: f32,
    _frame: u32,
) {
    let mut iy = cy;
    let col_gap: i32 = 8;

    // ── 核心色板 ──
    iy += render_section_header(f, cx, iy, "Core Palette", accent);

    let swatch_size: i32 = 48;
    let swatches = [
        ("Background", &edit.cfg.colors.background),
        ("Focus Border", &edit.cfg.colors.focus_border),
        ("Unfocus Border", &edit.cfg.colors.unfocus_border),
    ];
    for (i, (label, hex)) in swatches.iter().enumerate() {
        let sx = cx + (i as i32) * (swatch_size + col_gap + 50);
        render_color_swatch(
            f,
            sx,
            iy + 8,
            swatch_size,
            hex,
            label,
            accent,
            i == edit.focus_idx,
        );
    }
    iy += swatch_size + 40;

    // ── 顶栏色板 ──
    iy += render_section_header(f, cx, iy, "Bar Palette", accent);

    let bar_colors = [
        ("Bar BG", &edit.cfg.colors.bar_background),
        ("Bar FG", &edit.cfg.colors.bar_foreground),
        ("WS Active", &edit.cfg.colors.bar_workspace_active),
        ("WS Inactive", &edit.cfg.colors.bar_workspace_inactive),
    ];
    for (i, (label, hex)) in bar_colors.iter().enumerate() {
        let row = i / 3;
        let col = i % 3;
        let sx = cx + (col as i32) * (swatch_size + col_gap + 50);
        render_color_swatch(
            f,
            sx,
            iy + 8 + row as i32 * (swatch_size + 30),
            swatch_size,
            hex,
            label,
            accent,
            i + 3 == edit.focus_idx,
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// Layout 布局页
// ═══════════════════════════════════════════════════════════════════

fn render_page_layout(
    f: &mut impl Frame,
    cfg: &Config,
    cx: i32,
    cy: i32,
    _cw: i32,
    _ch: i32,
    edit: &SettingsEdit,
    accent: (f32, f32, f32),
    _alpha: f32,
) {
    let mut iy = cy;

    iy += render_section_header(f, cx, iy, "Spacing", accent);

    // Border Width
    text_render::draw_text(f, "Border Width", cx, iy + 6, 13.0, (0.7, 0.7, 0.85));
    render_slider(
        f,
        cx + 140,
        iy,
        200,
        edit.cfg.layout.border_width as f32 / 32.0,
        accent,
        edit.focus_idx == 0,
    );
    let val_text = format!("{} px", edit.cfg.layout.border_width);
    text_render::draw_text(f, &val_text, cx + 350, iy + 6, 12.0, (0.5, 0.5, 0.65));
    iy += 36;

    // Gap
    text_render::draw_text(f, "Gap", cx, iy + 6, 13.0, (0.7, 0.7, 0.85));
    render_slider(
        f,
        cx + 140,
        iy,
        200,
        edit.cfg.layout.gap as f32 / 48.0,
        accent,
        edit.focus_idx == 1,
    );
    let val_text2 = format!("{} px", edit.cfg.layout.gap);
    text_render::draw_text(f, &val_text2, cx + 350, iy + 6, 12.0, (0.5, 0.5, 0.65));
    iy += 36;

    // Margin
    text_render::draw_text(f, "Margin", cx, iy + 6, 13.0, (0.7, 0.7, 0.85));
    render_slider(
        f,
        cx + 140,
        iy,
        200,
        edit.cfg.layout.margin as f32 / 48.0,
        accent,
        edit.focus_idx == 2,
    );
    let val_text3 = format!("{} px", edit.cfg.layout.margin);
    text_render::draw_text(f, &val_text3, cx + 350, iy + 6, 12.0, (0.5, 0.5, 0.65));
}

// ═══════════════════════════════════════════════════════════════════
// Bar 顶栏页
// ═══════════════════════════════════════════════════════════════════

fn render_page_bar(
    f: &mut impl Frame,
    cfg: &Config,
    cx: i32,
    cy: i32,
    _cw: i32,
    _ch: i32,
    edit: &SettingsEdit,
    accent: (f32, f32, f32),
    _alpha: f32,
) {
    let mut iy = cy;

    iy += render_section_header(f, cx, iy, "Top Bar", accent);

    // Enabled toggle
    text_render::draw_text(f, "Enabled", cx, iy + 10, 13.0, (0.7, 0.7, 0.85));
    render_toggle(
        f,
        cx + 140,
        iy + 2,
        edit.cfg.bar.enabled,
        accent,
        edit.focus_idx == 0,
    );
    iy += 36;

    // Height
    text_render::draw_text(f, "Height", cx, iy + 6, 13.0, (0.7, 0.7, 0.85));
    render_slider(
        f,
        cx + 140,
        iy,
        200,
        edit.cfg.bar.height as f32 / 80.0,
        accent,
        edit.focus_idx == 1,
    );
    let val_text = format!("{} px", edit.cfg.bar.height);
    text_render::draw_text(f, &val_text, cx + 350, iy + 6, 12.0, (0.5, 0.5, 0.65));
    iy += 36;

    // Opacity
    text_render::draw_text(f, "Opacity", cx, iy + 6, 13.0, (0.7, 0.7, 0.85));
    render_slider(
        f,
        cx + 140,
        iy,
        200,
        edit.cfg.bar.opacity,
        accent,
        edit.focus_idx == 2,
    );
    let val_text2 = format!("{:.0}%", edit.cfg.bar.opacity * 100.0);
    text_render::draw_text(f, &val_text2, cx + 350, iy + 6, 12.0, (0.5, 0.5, 0.65));
    iy += 36;

    // ── 显示元素 ──
    iy += render_section_header(f, cx, iy, "Display", accent);

    render_checkbox(
        f,
        cx,
        iy + 8,
        edit.cfg.bar.show_date,
        "Date & Time",
        accent,
        edit.focus_idx == 3,
    );
    iy += 30;
    render_checkbox(
        f,
        cx,
        iy + 8,
        edit.cfg.bar.show_cpu,
        "CPU Usage",
        accent,
        edit.focus_idx == 4,
    );
    iy += 30;
    render_checkbox(
        f,
        cx,
        iy + 8,
        edit.cfg.bar.show_memory,
        "Memory Usage",
        accent,
        edit.focus_idx == 5,
    );
}

// ═══════════════════════════════════════════════════════════════════
// Wallpaper 壁纸页
// ═══════════════════════════════════════════════════════════════════

fn render_page_wallpaper(
    f: &mut impl Frame,
    cfg: &Config,
    cx: i32,
    cy: i32,
    _cw: i32,
    _ch: i32,
    edit: &SettingsEdit,
    accent: (f32, f32, f32),
    _alpha: f32,
) {
    let mut iy = cy;

    iy += render_section_header(f, cx, iy, "Wallpaper Mode", accent);

    let modes = ["color", "image", "random", "gradient"];
    for (i, mode) in modes.iter().enumerate() {
        let active = edit.cfg.wallpaper.mode == *mode;
        let focused = edit.focus_idx == i;
        let col = i % 4;
        let mx = cx + 10 + col as i32 * 100;
        let my = iy + 8;

        // Radio button — glow when focused
        if focused {
            glow_border(f, mx - 1, my + 3, 12, 12, accent);
        }
        let inner = if active {
            opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8)
        } else {
            opaque(0.15, 0.15, 0.25)
        };
        f.clear(inner, &[rect(mx, my + 4, 10, 10)]).ok();
        let border_alpha = if focused { 0.5 } else { 0.2 };
        f.clear(
            opaque(border_alpha, border_alpha, 0.35),
            &[rect(mx - 1, my + 3, 12, 12)],
        )
        .ok();

        let display = match *mode {
            "color" => "Solid",
            "image" => "Image",
            "random" => "Random",
            "gradient" => "Gradient",
            _ => mode,
        };
        let label_color = if focused {
            (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9)
        } else {
            (0.7, 0.7, 0.85)
        };
        text_render::draw_text(f, display, mx + 18, my + 2, 12.0, label_color);
    }
    iy += 40;

    // Scaling
    iy += render_section_header(f, cx, iy, "Scaling", accent);

    let scalings = ["fill", "fit", "stretch", "center"];
    for (i, sc) in scalings.iter().enumerate() {
        let active = edit.cfg.wallpaper.scaling == *sc;
        let focused = edit.focus_idx == i + 4;
        let col = i % 4;
        let mx = cx + 10 + col as i32 * 90;
        // Radio button — glow when focused
        if focused {
            glow_border(f, mx - 1, iy + 11, 12, 12, accent);
        }
        let inner = if active {
            opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8)
        } else {
            opaque(0.15, 0.15, 0.25)
        };
        f.clear(inner, &[rect(mx, iy + 12, 10, 10)]).ok();
        let border_alpha = if focused { 0.5 } else { 0.2 };
        f.clear(
            opaque(border_alpha, border_alpha, 0.35),
            &[rect(mx - 1, iy + 11, 12, 12)],
        )
        .ok();

        let label_color = if focused {
            (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9)
        } else {
            (0.7, 0.7, 0.85)
        };
        text_render::draw_text(f, sc, mx + 18, iy + 10, 12.0, label_color);
    }
}

// ═══════════════════════════════════════════════════════════════════
// 占位页（未实现但基础设施就绪的标签页）
// ═══════════════════════════════════════════════════════════════════

fn render_page_placeholder(
    f: &mut impl Frame,
    cx: i32,
    cy: i32,
    _cw: i32,
    _ch: i32,
    tab: SettingsTab,
    accent: (f32, f32, f32),
    _alpha: f32,
) {
    let title = format!("{} settings will be available soon.", tab.name());
    text_render::draw_text(
        f,
        &title,
        cx + 20,
        cy + 40,
        14.0,
        (accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
    );

    let hint = "← Use sidebar to switch tabs";
    text_render::draw_text(f, hint, cx + 20, cy + 70, 12.0, (0.25, 0.25, 0.4));
}

// ═══════════════════════════════════════════════════════════════════
// 聚焦控件名称映射
// ═══════════════════════════════════════════════════════════════════

fn focus_label(tab: SettingsTab, idx: usize) -> &'static str {
    match tab {
        SettingsTab::Colors => match idx {
            0 => "Background",
            1 => "Focus Border",
            2 => "Unfocus Border",
            3 => "Bar BG",
            4 => "Bar FG",
            5 => "WS Active",
            6 => "WS Inactive",
            _ => "",
        },
        SettingsTab::Layout => match idx {
            0 => "Border Width",
            1 => "Gap",
            2 => "Margin",
            _ => "",
        },
        SettingsTab::Bar => match idx {
            0 => "Enabled",
            1 => "Height",
            2 => "Opacity",
            3 => "Show Date",
            4 => "Show CPU",
            5 => "Show Memory",
            _ => "",
        },
        SettingsTab::Wallpaper => match idx {
            0 => "Solid",
            1 => "Image",
            2 => "Random",
            3 => "Gradient",
            4 => "Fill",
            5 => "Fit",
            6 => "Stretch",
            7 => "Center",
            _ => "",
        },
        _ => "",
    }
}
