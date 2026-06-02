//! 顶栏渲染 — 赛博朋克发光风格（与锁屏一致）
//! 左侧: ANCHOR logo + 工作区指示器
//! 中央: 窗口信息
//! 右侧: CPU/MEM 使用率 + 日期 + 时钟

use super::util::{
    opaque, rect, CLOCK_SIZE, DATE_SIZE, LOGO_SIZE, S1, S2, S3, S4, S6, TITLE_SIZE, WS_SIZE,
};
use crate::config::{parse_color, Config};
use crate::text_render;
use smithay::{backend::renderer::Frame, utils::Size};

/// 渲染 headbar（v29 — 赛博朋克发光风格 + CPU/MEM + 无限滚动指示器）
pub fn render_headbar(
    f: &mut impl Frame,
    cfg: &Config,
    ow: i32,
    _oh: i32,
    n_windows: usize,
    focus_idx: Option<usize>,
    time_secs: u64,
    _window_title: &str,
    active_workspace: usize,
    total_workspaces: usize,
    workspace_window_counts: &[usize],
    cpu_usage: f32,
    mem_usage: f32,
    recording: bool,
    scroll_offset: f64,
) {
    if !cfg.bar.enabled {
        return;
    }
    let h = cfg.bar.height;

    let accent = parse_color(&cfg.colors.focus_border);

    // ── 左侧 accent 竖装饰线 ──
    f.clear(
        opaque(accent.0 * 0.6, accent.1 * 0.6, accent.2 * 0.6),
        &[rect(0, 0, 2, h)],
    ).ok();
    // 发光扩散
    f.clear(
        opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
        &[rect(2, 0, 2, h)],
    ).ok();

    // ── 录制指示器（红色闪烁圆点）──
    if recording {
        let blink = (time_secs % 2) == 0;
        if blink {
            f.clear(
                opaque(0.9, 0.15, 0.15),
                &[rect(ow / 2 + 60, h / 2 - 4, 8, 8)],
            )
            .ok();
        }
        text_render::draw_text(f, "REC", ow / 2 + 72, h / 2 - 8, 13.0, (0.9, 0.3, 0.3));
    }

    // ── 背景 ──
    f.clear(
        opaque(0.03, 0.03, 0.06),
        &[smithay::utils::Rectangle::from_size(Size::new(ow, h))],
    )
    .ok();

    // 底部多层 accent 发光线（赛博朋克风格）
    for (off, br) in [(0i32, 0.8f32), (1, 0.5), (2, 0.25), (3, 0.1), (4, 0.04)] {
        f.clear(
            opaque(accent.0 * br, accent.1 * br, accent.2 * br),
            &[rect(0, h - 5 + off, ow, 1)],
        )
        .ok();
    }
    // 顶部高亮线（与底部呼应，更细更暗）
    f.clear(
        opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
        &[rect(0, 0, ow, 1)],
    ).ok();

    let mut x = S4;

    // ── ANCHOR logo ──
    let logo_w = text_render::text_width("ANCHOR", LOGO_SIZE);
    let logo_y = h / 2 - LOGO_SIZE as i32 / 2 - 2;
    // Logo 背景发光块
    f.clear(
        opaque(accent.0 * 0.08, accent.1 * 0.08, accent.2 * 0.08),
        &[rect(x - S1, S2, logo_w + S2 + S1, h - S4)],
    )
    .ok();
    // 左侧 accent 竖线
    f.clear(
        opaque(accent.0 * 0.6, accent.1 * 0.6, accent.2 * 0.6),
        &[rect(x - S1, S2, 2, h - S4)],
    )
    .ok();
    text_render::draw_text(
        f,
        "ANCHOR",
        x + 2,
        logo_y,
        LOGO_SIZE,
        (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9),
    );
    x += logo_w + S4 + S2;

    // 分隔线（带发光）
    f.clear(
        opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
        &[rect(x, S3, 1, h - S6)],
    )
    .ok();
    x += S3;

    // ── 工作区指示器（赛博朋克方块风格 + 无限滚动视差）──
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
            // 激活工作区 — accent 渐变背景 + 顶部亮线
            // 渐变：顶部亮 → 底部暗（2层）
            f.clear(
                opaque(accent.0 * 0.22, accent.1 * 0.22, accent.2 * 0.22),
                &[rect(x, block_y, block_w, block_h / 2)],
            ).ok();
            f.clear(
                opaque(accent.0 * 0.14, accent.1 * 0.14, accent.2 * 0.14),
                &[rect(x, block_y + block_h / 2, block_w, block_h - block_h / 2)],
            ).ok();
            f.clear(
                opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
                &[rect(x, block_y, block_w, 2)],
            )
            .ok();
            // 底部发光
            f.clear(
                opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
                &[rect(x, block_y + block_h, block_w, 2)],
            )
            .ok();
            text_render::draw_text(f, &num_str, x + ws_pad, text_y, WS_SIZE, accent);
        } else if has_wins {
            // 有窗口 — 暗色填充 + 小指示点
            f.clear(
                opaque(accent.0 * 0.06, accent.1 * 0.06, accent.2 * 0.06),
                &[rect(x, block_y, block_w, block_h)],
            )
            .ok();
            f.clear(
                opaque(accent.0 * 0.4, accent.1 * 0.4, accent.2 * 0.4),
                &[rect(x + block_w / 2 - 2, block_y + block_h + 1, 4, 2)],
            )
            .ok();
            text_render::draw_text(
                f,
                &num_str,
                x + ws_pad,
                text_y,
                WS_SIZE,
                (accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
            );
        } else {
            // 空工作区 — 极暗
            f.clear(
                opaque(0.02, 0.02, 0.04),
                &[rect(x, block_y, block_w, block_h)],
            )
            .ok();
            text_render::draw_text(
                f,
                &num_str,
                x + ws_pad,
                text_y,
                WS_SIZE,
                (accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
            );
        }

        x += block_w + ws_gap;
    }

    // ── 滑动式 ws 位置指示条 ──
    // 用 scroll_offset 驱动的连续位置条形光标
    // 指示条在 ws 方块下方滑动，跟随 scroll_offset 平滑移动
    {
        let indicator_y = h - 8;
        let indicator_h = 3;
        // 计算整个 ws 区域的宽度和起始位置
        let ws_start_x = S4 + logo_w + S4 + S2 + S3;
        let ws_block_pad = 6;
        let ws_gap_total = 3;
        let total_ws_w: i32 = (0..max_show)
            .map(|i| {
                let num_str = format!("{}", i + 1);
                let num_w = text_render::text_width(&num_str, WS_SIZE);
                num_w + ws_block_pad * 2 + if i < max_show - 1 { ws_gap_total } else { 0 }
            })
            .sum();
        // 指示条宽度 = 单个 ws 方块宽度
        let first_block_w = {
            let num_str = format!("{}", 1);
            let num_w = text_render::text_width(&num_str, WS_SIZE);
            num_w + ws_block_pad * 2
        };
        // 计算 scroll_offset 对应的指示条位置
        let frac = scroll_offset - scroll_offset.floor();
        let base_idx = scroll_offset.floor() as i32;
        let offset_in_block: f64 = (0..max_show)
            .map(|i| {
                let num_str = format!("{}", i + 1);
                let num_w = text_render::text_width(&num_str, WS_SIZE);
                num_w + ws_block_pad * 2 + if i < max_show - 1 { ws_gap_total } else { 0 }
            })
            .take(base_idx as usize)
            .sum::<i32>() as f64;
        let current_block_w = {
            let idx = (base_idx as usize).min(max_show - 1);
            let num_str = format!("{}", idx + 1);
            let num_w = text_render::text_width(&num_str, WS_SIZE);
            num_w + ws_block_pad * 2
        };
        let indicator_x = ws_start_x as f64 + offset_in_block + frac * (current_block_w + ws_gap_total) as f64;
        let indicator_w = first_block_w as f64 * (1.0 - frac * 0.3).max(0.7);

        // 发光底座（宽一点，暗一点）
        f.clear(
            opaque(accent.0 * 0.08, accent.1 * 0.08, accent.2 * 0.08),
            &[rect(ws_start_x - 2, indicator_y, total_ws_w + 4, indicator_h)],
        ).ok();
        // 主指示条
        f.clear(
            opaque(accent.0 * 0.8, accent.1 * 0.8, accent.2 * 0.8),
            &[rect(indicator_x as i32, indicator_y, indicator_w as i32, indicator_h)],
        ).ok();
        // 发光层
        f.clear(
            opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
            &[rect(indicator_x as i32 - 2, indicator_y + indicator_h, (indicator_w + 4.0) as i32, 2)],
        ).ok();
    }

    // 分隔线
    f.clear(
        opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
        &[rect(x + S1, S3, 1, h - S6)],
    )
    .ok();
    x += S4;

    // ── 中央窗口信息 ──
    if n_windows > 0 {
        let info = format!("{} / {}", focus_idx.map(|i| i + 1).unwrap_or(0), n_windows);
        let tw = text_render::text_width(&info, TITLE_SIZE);
        let cx = ow / 2 - tw / 2;
        let ty = h / 2 - TITLE_SIZE as i32 / 2 - 1;
        f.clear(
            opaque(accent.0 * 0.04, accent.1 * 0.04, accent.2 * 0.04),
            &[rect(cx - S2, S2, tw + S4, h - S4)],
        )
        .ok();
        // 底部 accent 发光线
        f.clear(
            opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
            &[rect(cx - S2, h - 3, tw + S4, 1)],
        ).ok();
        text_render::draw_text(
            f,
            &info,
            cx,
            ty,
            TITLE_SIZE,
            (accent.0 * 0.4, accent.1 * 0.4, accent.2 * 0.4),
        );
    }

    // ── 右侧区域 ──
    let time_secs_c = time_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_secs_c, &mut tm) };

    let local_h = tm.tm_hour as u8;
    let minutes = tm.tm_min as u8;
    let seconds = tm.tm_sec as u8;

    let mut rx = ow - S4;
    let ty = h / 2 - CLOCK_SIZE as i32 / 2 - 1;

    // ── 时钟 ──
    let time_str = format!("{:02}:{:02}:{:02}", local_h, minutes, seconds);
    let tw = text_render::text_width(&time_str, CLOCK_SIZE);
    // 秒级脉冲：accent 亮度随秒数微妙变化
    let pulse = 0.06 + 0.02 * (seconds as f32 * 0.1047).sin();
    f.clear(
        opaque(accent.0 * pulse, accent.1 * pulse, accent.2 * pulse),
        &[rect(rx - tw - S2, S2, tw + S4, h - S4)],
    )
    .ok();
    // 时钟文字亮度也微弱脉冲
    let text_pulse = 0.85 + 0.05 * (seconds as f32 * 0.1047).sin();
    text_render::draw_text(
        f,
        &time_str,
        rx - tw,
        ty,
        CLOCK_SIZE,
        (accent.0 * text_pulse, accent.1 * text_pulse, accent.2 * text_pulse),
    );
    rx -= tw + S4;

    // 分隔线
    f.clear(
        opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
        &[rect(rx, S3, 1, h - S6)],
    )
    .ok();
    rx -= S3;

    // ── 日期 + 星期几 ──
    if cfg.bar.show_date {
        let month = (tm.tm_mon + 1) as u8;
        let day = tm.tm_mday as u8;
        let weekdays = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
        let weekday = weekdays.get(tm.tm_wday as usize).unwrap_or(&"");
        let date_str = format!("{}-{}-{}", tm.tm_year + 1900, month, day);
        let full_str = format!("{} {}", date_str, weekday);
        let dw = text_render::text_width(&full_str, DATE_SIZE);
        let dy = h / 2 - DATE_SIZE as i32 / 2 - 1;
        // 日期背景发光
        f.clear(
            opaque(accent.0 * 0.03, accent.1 * 0.03, accent.2 * 0.03),
            &[rect(rx - dw - S2, S2, dw + S4, h - S4)],
        ).ok();
        // 底部发光线
        f.clear(
            opaque(accent.0 * 0.12, accent.1 * 0.12, accent.2 * 0.12),
            &[rect(rx - dw - S2, h - 3, dw + S4, 1)],
        ).ok();
        text_render::draw_text(
            f,
            &full_str,
            rx - dw,
            dy,
            DATE_SIZE,
            (accent.0 * 0.35, accent.1 * 0.35, accent.2 * 0.35),
        );
        rx -= dw + S3;
        f.clear(
            opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
            &[rect(rx, S3, 1, h - S6)],
        )
        .ok();
        rx -= S3;
    }

    // ── CPU/MEM 使用率（赛博朋克进度条 + 百分比）──
    let stat_size: f32 = 13.0;
    let bar_w = 48;
    let bar_h = 4;

    if cfg.bar.show_memory {
        let mem_pct = mem_usage * 100.0;
        let mem_str = format!("MEM {:5.1}%", mem_pct);
        let mw = text_render::text_width(&mem_str, stat_size);
        let my = h / 2 - stat_size as i32 / 2 - 6;
        text_render::draw_text(
            f,
            &mem_str,
            rx - mw,
            my,
            stat_size,
            (accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
        );
        // 进度条背景
        let bar_x = rx - mw;
        let bar_y = my + stat_size as i32 + 3;
        f.clear(
            opaque(0.06, 0.06, 0.10),
            &[rect(bar_x, bar_y, bar_w, bar_h)],
        )
        .ok();
        // 进度条填充（颜色随使用率变化：低=accent绿，高=红）
        let fill_w = (bar_w as f32 * mem_usage) as i32;
        let mem_color = if mem_usage < 0.7 {
            (accent.0 * 0.7, accent.1 * 0.7, accent.2 * 0.7)
        } else {
            (0.9, 0.3, 0.3)
        };
        if fill_w > 0 {
            f.clear(
                opaque(mem_color.0, mem_color.1, mem_color.2),
                &[rect(bar_x, bar_y, fill_w, bar_h)],
            )
            .ok();
            // 进度条发光扩散（3层）
            f.clear(
                opaque(mem_color.0 * 0.3, mem_color.1 * 0.3, mem_color.2 * 0.3),
                &[rect(bar_x, bar_y - 1, fill_w, 1)],
            ).ok();
            f.clear(
                opaque(mem_color.0 * 0.12, mem_color.1 * 0.12, mem_color.2 * 0.12),
                &[rect(bar_x, bar_y - 2, fill_w, 1)],
            ).ok();
            // 进度条尾部发光点
            if fill_w > 2 {
                f.clear(
                    opaque(mem_color.0 * 0.5, mem_color.1 * 0.5, mem_color.2 * 0.5),
                    &[rect(bar_x + fill_w - 2, bar_y - 1, 2, bar_h + 2)],
                ).ok();
            }
        }
        rx -= mw.max(bar_w) + S3;
        f.clear(
            opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
            &[rect(rx, S3, 1, h - S6)],
        )
        .ok();
        rx -= S3;
    }

    if cfg.bar.show_cpu {
        let cpu_pct = cpu_usage * 100.0;
        let cpu_str = format!("CPU {:5.1}%", cpu_pct);
        let cw = text_render::text_width(&cpu_str, stat_size);
        let cy = h / 2 - stat_size as i32 / 2 - 6;
        text_render::draw_text(
            f,
            &cpu_str,
            rx - cw,
            cy,
            stat_size,
            (accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
        );
        // 进度条背景
        let bar_x = rx - cw;
        let bar_y = cy + stat_size as i32 + 3;
        f.clear(
            opaque(0.06, 0.06, 0.10),
            &[rect(bar_x, bar_y, bar_w, bar_h)],
        )
        .ok();
        // 进度条填充
        let fill_w = (bar_w as f32 * cpu_usage) as i32;
        let cpu_color = if cpu_usage < 0.7 {
            (accent.0 * 0.7, accent.1 * 0.7, accent.2 * 0.7)
        } else {
            (0.9, 0.3, 0.3)
        };
        if fill_w > 0 {
            f.clear(
                opaque(cpu_color.0, cpu_color.1, cpu_color.2),
                &[rect(bar_x, bar_y, fill_w, bar_h)],
            )
            .ok();
            // 进度条发光扩散（3层）
            f.clear(
                opaque(cpu_color.0 * 0.3, cpu_color.1 * 0.3, cpu_color.2 * 0.3),
                &[rect(bar_x, bar_y - 1, fill_w, 1)],
            ).ok();
            f.clear(
                opaque(cpu_color.0 * 0.12, cpu_color.1 * 0.12, cpu_color.2 * 0.12),
                &[rect(bar_x, bar_y - 2, fill_w, 1)],
            ).ok();
            // 进度条尾部发光点
            if fill_w > 2 {
                f.clear(
                    opaque(cpu_color.0 * 0.5, cpu_color.1 * 0.5, cpu_color.2 * 0.5),
                    &[rect(bar_x + fill_w - 2, bar_y - 1, 2, bar_h + 2)],
                ).ok();
            }
        }
        rx -= cw.max(bar_w) + S3;
    }
}
