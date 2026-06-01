//! 顶栏渲染（logo + 工作区指示器 + 窗口信息 + 日期/时钟）

use crate::config::{parse_color, Config};
use crate::text_render;
use smithay::{
    backend::renderer::Frame,
    utils::Size,
};
use super::util::{color_hex, opaque, rect, S1, S2, S3, S4, S6, LOGO_SIZE, WS_SIZE, TITLE_SIZE, CLOCK_SIZE, DATE_SIZE};

/// 渲染 headbar（v23 — fontdue 正常文字）
pub fn render_headbar(
    f: &mut impl Frame, cfg: &Config, ow: i32, _oh: i32,
    n_windows: usize, focus_idx: Option<usize>, time_secs: u64,
    _window_title: &str,
    active_workspace: usize, total_workspaces: usize,
    workspace_window_counts: &[usize],
) {
    if !cfg.bar.enabled { return; }
    let h = cfg.bar.height;

    let fg = parse_color(&cfg.colors.bar_foreground);
    let ws_active = parse_color(&cfg.colors.bar_workspace_active);
    let status_color = parse_color(&cfg.colors.bar_status);
    let accent = parse_color(&cfg.colors.focus_border);
    let sep_color = parse_color(&cfg.colors.bar_separator);

    // ── 背景 ──
    f.clear(color_hex(&cfg.colors.bar_background), &[smithay::utils::Rectangle::from_size(Size::new(ow, h))]).ok();

    // 底部 accent 发光线
    for (off, br) in [(0i32, 1.0f32), (1, 0.6), (2, 0.3), (3, 0.12), (4, 0.04)] {
        f.clear(opaque(accent.0 * br, accent.1 * br, accent.2 * br),
            &[rect(0, h - 5 + off, ow, 1)]).ok();
    }

    let mut x = S4;

    // ── ANCHOR logo（正常文字）──
    let logo_w = text_render::text_width("ANCHOR", LOGO_SIZE);
    let logo_y = h / 2 - LOGO_SIZE as i32 / 2 - 2;
    f.clear(opaque(accent.0 * 0.12, accent.1 * 0.12, accent.2 * 0.12),
        &[rect(x - S1, S2, logo_w + S2 + S1, h - S4)]).ok();
    f.clear(opaque(accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
        &[rect(x - S1, S2, 2, h - S4)]).ok();
    text_render::draw_text(f, "ANCHOR", x + 2, logo_y, LOGO_SIZE, accent);
    x += logo_w + S4 + S2;

    // 分隔线
    f.clear(opaque(sep_color.0 * 0.3, sep_color.1 * 0.3, sep_color.2 * 0.3),
        &[rect(x, S3, 1, h - S6)]).ok();
    x += S3;

    // ── 工作区指示器 ──
    let ws_pad = 6;
    let ws_gap = 3;
    let max_show = total_workspaces.min(9);

    for i in 0..max_show {
        let is_active = i == active_workspace;
        let ws_wins = workspace_window_counts.get(i).copied().unwrap_or(0);
        let has_wins = ws_wins > 0;
        let num_str = format!("{}", i + 1);
        let num_w = text_render::text_width(&num_str, WS_SIZE);
        let block_w = num_w + ws_pad * 2;
        let block_h = WS_SIZE as i32 + ws_pad * 2;
        let block_y = h / 2 - block_h / 2;
        let text_y = block_y + ws_pad;

        if is_active {
            f.clear(opaque(ws_active.0, ws_active.1, ws_active.2),
                &[rect(x, block_y, block_w, block_h)]).ok();
            f.clear(opaque(ws_active.0 * 1.4, ws_active.1 * 1.4, ws_active.2 * 1.4),
                &[rect(x, block_y, block_w, 2)]).ok();
            text_render::draw_text(f, &num_str, x + ws_pad, text_y, WS_SIZE, (0.02, 0.02, 0.05));
        } else if has_wins {
            f.clear(opaque(fg.0 * 0.12, fg.1 * 0.12, fg.2 * 0.12),
                &[rect(x, block_y, block_w, block_h)]).ok();
            f.clear(opaque(fg.0 * 0.5, fg.1 * 0.5, fg.2 * 0.5),
                &[rect(x + block_w / 2 - 2, block_y + block_h + 1, 4, 2)]).ok();
            text_render::draw_text(f, &num_str, x + ws_pad, text_y, WS_SIZE,
                (fg.0 * 0.7, fg.1 * 0.7, fg.2 * 0.7));
        } else {
            f.clear(opaque(fg.0 * 0.04, fg.1 * 0.04, fg.2 * 0.04),
                &[rect(x, block_y, block_w, block_h)]).ok();
            text_render::draw_text(f, &num_str, x + ws_pad, text_y, WS_SIZE,
                (fg.0 * 0.2, fg.1 * 0.2, fg.2 * 0.2));
        }

        x += block_w + ws_gap;
    }

    // 分隔线
    f.clear(opaque(sep_color.0 * 0.3, sep_color.1 * 0.3, sep_color.2 * 0.3),
        &[rect(x + S1, S3, 1, h - S6)]).ok();
    x += S4;

    // ── 中央窗口信息 ──
    if n_windows > 0 {
        let info = format!("{} / {}", focus_idx.map(|i| i + 1).unwrap_or(0), n_windows);
        let tw = text_render::text_width(&info, TITLE_SIZE);
        let cx = ow / 2 - tw / 2;
        let ty = h / 2 - TITLE_SIZE as i32 / 2 - 1;
        f.clear(opaque(fg.0 * 0.04, fg.1 * 0.04, fg.2 * 0.04),
            &[rect(cx - S2, S2, tw + S4, h - S4)]).ok();
        text_render::draw_text(f, &info, cx, ty, TITLE_SIZE, (fg.0 * 0.5, fg.1 * 0.5, fg.2 * 0.5));
    }

    // ── 右侧：日期 + 时钟 ──
    let time_secs_c = time_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_secs_c, &mut tm) };

    let local_h = tm.tm_hour as u8;
    let minutes = tm.tm_min as u8;
    let seconds = tm.tm_sec as u8;

    let mut rx = ow - S4;
    let ty = h / 2 - CLOCK_SIZE as i32 / 2 - 1;

    // 日期
    if cfg.bar.show_date {
        let month = (tm.tm_mon + 1) as u8;
        let day = tm.tm_mday as u8;
        let date_str = format!("{}-{}-{}", tm.tm_year + 1900, month, day);
        let dw = text_render::text_width(&date_str, DATE_SIZE);
        let dy = h / 2 - DATE_SIZE as i32 / 2 - 1;
        text_render::draw_text(f, &date_str, rx - dw, dy, DATE_SIZE, (fg.0 * 0.4, fg.1 * 0.4, fg.2 * 0.4));
        rx -= dw + S3;
        f.clear(opaque(sep_color.0 * 0.3, sep_color.1 * 0.3, sep_color.2 * 0.3),
            &[rect(rx, S3, 1, h - S6)]).ok();
        rx -= S3;
    }

    // 时钟
    let time_str = format!("{:02}:{:02}:{:02}", local_h, minutes, seconds);
    let tw = text_render::text_width(&time_str, CLOCK_SIZE);
    f.clear(opaque(accent.0 * 0.08, accent.1 * 0.08, accent.2 * 0.08),
        &[rect(rx - tw - S2, S2, tw + S4, h - S4)]).ok();
    text_render::draw_text(f, &time_str, rx - tw, ty, CLOCK_SIZE, status_color);
}
