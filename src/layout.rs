//! 布局计算 + 壁纸渲染 + Headbar 渲染 + 窗口装饰 v24
//! 多布局预设 + fontdue TTF 字体渲染

use crate::config::{parse_color, Config};
use crate::text_render;
use smithay::{
    backend::renderer::{Frame, Color32F},
    utils::{Physical, Point, Rectangle, Size},
};

/// 布局预设
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutPreset {
    MasterStack,
    Columns,
    Center,
    Grid,
}

impl LayoutPreset {
    pub const ALL: [LayoutPreset; 4] = [
        LayoutPreset::MasterStack, LayoutPreset::Columns,
        LayoutPreset::Center, LayoutPreset::Grid,
    ];
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&x| x == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::MasterStack => "Master-Stack",
            Self::Columns => "Columns",
            Self::Center => "Center",
            Self::Grid => "Grid",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "master-stack" | "masterstack" => Some(Self::MasterStack),
            "columns" | "column" => Some(Self::Columns),
            "center" => Some(Self::Center),
            "grid" => Some(Self::Grid),
            _ => None,
        }
    }
}

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

impl Default for LayoutPreset {
    fn default() -> Self { Self::MasterStack }
}

#[inline(always)]
pub fn opaque(r: f32, g: f32, b: f32) -> Color32F { Color32F::new(r, g, b, 1.0) }
#[inline(always)]
fn color_hex(hex: &str) -> Color32F {
    let (r, g, b) = parse_color(hex);
    opaque(r, g, b)
}
#[inline(always)]
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}

const S1: i32 = 4;
const S2: i32 = 8;
const S3: i32 = 12;
const S4: i32 = 16;
const S6: i32 = 24;

// 字体大小
const LOGO_SIZE: f32 = 20.0;
const WS_SIZE: f32 = 16.0;
const TITLE_SIZE: f32 = 16.0;
const CLOCK_SIZE: f32 = 18.0;
const DATE_SIZE: f32 = 14.0;

// ═══════════════════════════════════════════════════════════
// 布局计算
// ═══════════════════════════════════════════════════════════

pub fn slot(i: usize, n: usize, ow: i32, oh: i32, bar_h: i32, cfg: &Config, layout: LayoutPreset) -> (i32, i32, i32, i32) {
    if n == 0 { return (0, bar_h, 0, 0); }
    let gap = cfg.layout.gap;
    let margin = cfg.layout.margin;
    let usable_w = ow - 2 * margin;
    let usable_h = oh - bar_h - 2 * margin;

    match layout {
        LayoutPreset::MasterStack => {
            match n {
                1 => (margin, bar_h + margin, usable_w, usable_h),
                _ => {
                    let master_w = (usable_w - gap) * 2 / 3;
                    if i == 0 {
                        (margin, bar_h + margin, master_w, usable_h)
                    } else {
                        let stack_n = n - 1;
                        let stack_w = usable_w - master_w - gap;
                        let stack_h = (usable_h - gap * (stack_n - 1) as i32) / stack_n as i32;
                        let extra = (usable_h - gap * (stack_n - 1) as i32) % stack_n as i32;
                        let si = i - 1;
                        let sy = bar_h + margin + si as i32 * (stack_h + gap) + extra.min(si as i32);
                        let sh = stack_h + if si < extra as usize { 1 } else { 0 };
                        (margin + master_w + gap, sy, stack_w, sh)
                    }
                }
            }
        }
        LayoutPreset::Columns => {
            let col_w = (usable_w - gap * (n as i32 - 1)) / n as i32;
            let extra = (usable_w - gap * (n as i32 - 1)) % n as i32;
            let x = margin + i as i32 * (col_w + gap) + extra.min(i as i32);
            let w = col_w + if i < extra as usize { 1 } else { 0 };
            (x, bar_h + margin, w, usable_h)
        }
        LayoutPreset::Center => {
            // 居中：固定宽度 1200px（或屏幕 70%），等高竖排
            let max_w = 1200.min(ow * 7 / 10);
            let cw = (max_w - gap * (n as i32 - 1)) / n as i32;
            let extra = (max_w - gap * (n as i32 - 1)) % n as i32;
            let total_w = max_w;
            let start_x = ow / 2 - total_w / 2;
            let x = start_x + i as i32 * (cw + gap) + extra.min(i as i32);
            let w = cw + if i < extra as usize { 1 } else { 0 };
            (x, bar_h + margin, w, usable_h)
        }
        LayoutPreset::Grid => {
            let cols = (n as f32).sqrt().ceil() as i32;
            let rows = (n as i32 + cols - 1) / cols;
            let col = i as i32 % cols;
            let row = i as i32 / cols;
            let items_in_row = if row < rows - 1 { cols } else { n as i32 - row * cols };
            let col_w = (usable_w - gap * (cols - 1)) / cols;
            let row_h = (usable_h - gap * (rows - 1)) / rows;
            // 最后一行居中
            let row_start = if row == rows - 1 {
                let used_w = items_in_row * col_w + (items_in_row - 1) * gap;
                margin + (usable_w - used_w) / 2
            } else {
                margin
            };
            let x = row_start + col * (col_w + gap);
            let y = bar_h + margin + row * (row_h + gap);
            (x, y, col_w, row_h)
        }
    }
}

// ═══════════════════════════════════════════════════════════
// 渲染
// ═══════════════════════════════════════════════════════════

pub fn render_window_bg(f: &mut impl Frame, cfg: &Config, n: usize, ow: i32, oh: i32, bar_h: i32, layout: LayoutPreset) {
    render_window_bg_anim(f, cfg, n, ow, oh, bar_h, layout, 0);
}

pub fn render_window_bg_anim(f: &mut impl Frame, cfg: &Config, n: usize, ow: i32, oh: i32, bar_h: i32, layout: LayoutPreset, offset_x: i32) {
    if n == 0 { return; }
    let bw = cfg.layout.border_width;
    let bg = color_hex(&cfg.colors.background);
    for i in 0..n {
        let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg, layout);
        f.clear(bg, &[rect(x - bw + offset_x, y - bw, w + 2 * bw, h + 2 * bw)]).ok();
    }
}

pub fn render_wallpaper(f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32, frame: u32) {
    f.clear(color_hex(&cfg.wallpaper.color), &[Rectangle::from_size(Size::new(ow, oh))]).ok();

    let accent = parse_color(&cfg.colors.focus_border);

    let grid = opaque(accent.0 * 0.03, accent.1 * 0.03, accent.2 * 0.03);
    for y in (0..oh).step_by(64) {
        f.clear(grid, &[rect(0, y, ow, 1)]).ok();
    }
    for x in (0..ow).step_by(64) {
        f.clear(grid, &[rect(x, 0, 1, oh)]).ok();
    }

    let dot = opaque(accent.0 * 0.05, accent.1 * 0.05, accent.2 * 0.05);
    for y in (0..oh).step_by(64) {
        for x in (0..ow).step_by(64) {
            f.clear(dot, &[rect(x, y, 2, 2)]).ok();
        }
    }

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

pub fn render_window_decorations(
    f: &mut impl Frame, cfg: &Config,
    i: usize, n: usize, focus_idx: Option<usize>,
    ow: i32, oh: i32, bar_h: i32,
    layout: LayoutPreset,
) {
    render_window_decorations_anim(f, cfg, i, n, focus_idx, ow, oh, bar_h, layout, 0);
}

pub fn render_window_decorations_anim(
    f: &mut impl Frame, cfg: &Config,
    i: usize, n: usize, focus_idx: Option<usize>,
    ow: i32, oh: i32, bar_h: i32,
    layout: LayoutPreset, offset_x: i32,
) {
    if n == 0 { return; }
    let bw = cfg.layout.border_width;
    let (x, y, w, h) = slot(i, n, ow, oh, bar_h, cfg, layout);
    let x = x + offset_x;
    let is_focused = focus_idx == Some(i);

    if is_focused {
        let accent = parse_color(&cfg.colors.focus_border);
        let border = opaque(accent.0, accent.1, accent.2);
        let bright = opaque((accent.0 * 1.6).min(1.0), (accent.1 * 1.6).min(1.0), (accent.2 * 1.6).min(1.0));
        let dark = opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3);

        f.clear(border, &[rect(x - bw, y - bw, w + 2 * bw, bw)]).ok();
        f.clear(border, &[rect(x - bw, y + h, w + 2 * bw, bw)]).ok();
        f.clear(border, &[rect(x - bw, y, bw, h)]).ok();
        f.clear(border, &[rect(x + w, y, bw, h)]).ok();
        f.clear(bright, &[rect(x - bw, y - bw, w + 2 * bw, 2)]).ok();
        f.clear(dark, &[rect(x - bw, y + h + bw - 3, w + 2 * bw, 3)]).ok();

        // 窗口编号（正常文字）
        let label_w = 20;
        f.clear(border, &[rect(x - bw, y - bw, label_w, 18)]).ok();
        text_render::draw_text(f, &format!("{}", i + 1), x - bw + 6, y - bw + 2, 12.0, (0.0, 0.0, 0.0));
    } else {
        let unfocus = parse_color(&cfg.colors.unfocus_border);
        let border = opaque(unfocus.0, unfocus.1, unfocus.2);
        f.clear(border, &[rect(x, y, w, bw)]).ok();
        f.clear(border, &[rect(x, y + h - bw, w, bw)]).ok();
        f.clear(border, &[rect(x, y, bw, h)]).ok();
        f.clear(border, &[rect(x + w - bw, y, bw, h)]).ok();
    }
}

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
    f.clear(color_hex(&cfg.colors.bar_background), &[Rectangle::from_size(Size::new(ow, h))]).ok();

    // 底部 accent 发光线
    for (off, br) in [(0i32, 1.0f32), (1, 0.6), (2, 0.3), (3, 0.12), (4, 0.04)] {
        f.clear(opaque(accent.0 * br, accent.1 * br, accent.2 * br),
            &[rect(0, h - 5 + off, ow, 1)]).ok();
    }

    let mut x = S4;

    // ── TITAN logo（正常文字）──
    let logo_w = text_render::text_width("TITAN", LOGO_SIZE);
    let logo_y = h / 2 - LOGO_SIZE as i32 / 2 - 2;
    f.clear(opaque(accent.0 * 0.12, accent.1 * 0.12, accent.2 * 0.12),
        &[rect(x - S1, S2, logo_w + S2 + S1, h - S4)]).ok();
    f.clear(opaque(accent.0 * 0.5, accent.1 * 0.5, accent.2 * 0.5),
        &[rect(x - S1, S2, 2, h - S4)]).ok();
    text_render::draw_text(f, "TITAN", x + 2, logo_y, LOGO_SIZE, accent);
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

/// 渲染通知弹窗（右上角 overlay）
pub fn render_notifications(
    f: &mut impl Frame,
    notifications: &[(String, std::time::Instant, std::time::Duration)],
    ow: i32, bar_h: i32,
    accent: (f32, f32, f32),
) {
    if notifications.is_empty() { return; }
    let now = std::time::Instant::now();
    let fg = opaque(0.95, 0.95, 0.98);
    let bg = opaque(0.08, 0.08, 0.12);
    let accent_color = opaque(accent.0, accent.1, accent.2);
    let pad = 12;
    let notif_h = 28;
    let gap = 6;
    let max_w = 360;
    let font_size = 15.0;

    for (idx, (text, created, duration)) in notifications.iter().enumerate() {
        let elapsed = now.duration_since(*created).as_secs_f32();
        let remaining = duration.as_secs_f32() - elapsed;
        if remaining <= 0.0 { continue; }

        // 淡入淡出
        let alpha = if elapsed < 0.2 { elapsed / 0.2 } else if remaining < 0.5 { remaining / 0.5 } else { 1.0 };

        let tw = text_render::text_width(text, font_size).min(max_w - 2 * pad) as i32;
        let nw = tw + 2 * pad;
        let ny = bar_h + 10 + idx as i32 * (notif_h + gap);

        // 背景
        f.clear(bg, &[rect(ow - nw - 12, ny, nw, notif_h)]).ok();
        // 左侧 accent 竖条
        f.clear(accent_color, &[rect(ow - nw - 12, ny, 3, notif_h)]).ok();
        // 文字
        let text_color = (0.95 * alpha, 0.95 * alpha, 0.98 * alpha);
        text_render::draw_text(f, text, ow - nw - 12 + pad, ny + notif_h / 2 - font_size as i32 / 2 - 1, font_size, text_color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    fn test_cfg() -> Config { Config::default() }

    #[test]
    fn test_slot_one() {
        let cfg = test_cfg();
        let (_x, y, w, h) = slot(0, 1, 2560, 1440, 42, &cfg, LayoutPreset::MasterStack);
        assert!(y >= 42 && w > 0 && h > 0);
    }
    #[test]
    fn test_slot_two() {
        let cfg = test_cfg();
        let a = slot(0, 2, 2560, 1440, 42, &cfg, LayoutPreset::MasterStack);
        let b = slot(1, 2, 2560, 1440, 42, &cfg, LayoutPreset::MasterStack);
        assert!(a.0 + a.2 <= b.0);
    }
    #[test]
    fn test_no_overlap() {
        let cfg = test_cfg();
        for layout in LayoutPreset::ALL {
            for n in 1..=6usize {
                let mut rects: Vec<(i32,i32,i32,i32)> = vec![];
                for i in 0..n {
                    let r = slot(i, n, 2560, 1440, 42, &cfg, layout);
                    for (j, p) in rects.iter().enumerate() {
                        let overlap = r.0 < p.0+p.2 && r.0+r.2>p.0 && r.1<p.1+p.3 && r.1+r.3>p.1;
                        assert!(!overlap, "{:?} n={n}: {j} overlaps {}", layout, i);
                    }
                    rects.push(r);
                }
            }
        }
    }
}
