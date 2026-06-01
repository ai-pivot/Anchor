//! 锁屏渲染（5 种背景风格 + dim overlay + 时钟/密码输入框）

use crate::config::{parse_color, Config};
use crate::text_render;
use smithay::{
    backend::renderer::Frame,
    utils::{Physical, Rectangle},
};
use super::util::{opaque, rect};

/// 渲染锁屏覆盖层 — 非焦点屏幕（暗色覆盖 + 同风格背景）
pub fn render_lock_screen_dim(
    f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32, elapsed: f32, style: u8,
) {
    let accent = parse_color(&cfg.colors.focus_border);
    f.clear(opaque(0.02, 0.02, 0.04), &[rect(0, 0, ow, oh)]).ok();

    // 渲染对应风格的暗色版背景
    render_lock_bg(f, accent, ow, oh, elapsed, style, 0.25);

    let lock_str = "LOCKED";
    let lock_w = text_render::text_width(lock_str, 18.0);
    text_render::draw_text(f, lock_str, ow / 2 - lock_w / 2, oh / 2 - 9, 18.0,
        (accent.0 * 0.25, accent.1 * 0.25, accent.2 * 0.25));
}

// ═══════════════════════════════════════════════════════════════
// 5 种锁屏背景风格（每次锁屏随机选择一种）
// 0 = Nebula    — 星云：多层半透明方块模拟星云 + 闪烁星点
// 1 = Wave      — 波纹：多层正弦波 + 水平扫描线
// 2 = Grid      — 赛博网格：透视网格 + 脉冲扫描线
// 3 = Rings     — 同心圆脉冲 + 中心辐射
// 4 = Aurora    — 极光：多层飘动的彩色条带
// ═══════════════════════════════════════════════════════════════

/// 确定性哈希伪随机（用于生成粒子的随机位置，每帧一致）
fn hash_rand(seed: u32, i: u32) -> u32 {
    let mut h = seed.wrapping_add(i).wrapping_mul(0x45d9f3b);
    h = h ^ (h >> 16);
    h = h.wrapping_mul(0x45d9f3b);
    h = h ^ (h >> 16);
    h
}

/// 渲染锁屏背景（可缩放亮度用于非焦点屏幕）
/// `elapsed` = 锁屏激活以来的秒数（基于 Instant，与帧率无关）
fn render_lock_bg(
    f: &mut impl Frame, accent: (f32, f32, f32), ow: i32, oh: i32,
    elapsed: f32, style: u8, brightness_scale: f32,
) {
    // 基于 elapsed 的动画时间，使用整数帧计数保持 hash_rand 确定性
    // 速度系数 0.7：旧代码 t = frame * 0.012，60fps 时 t 每秒增加 0.72
    let t = elapsed * 0.7;
    let frame = (elapsed * 60.0) as u32; // 假设 60fps 基准的虚拟帧计数（用于确定性随机）

    match style {
        // ── Style 0: Nebula 星云 ──
        0 => {
            // 多层星云光斑
            let clouds: [(f32, f32, f32, i32, f32); 6] = [
                (t.sin(), t.cos(), 0.5, 260, 0.22),
                ((t * 0.7 + 1.5).sin(), (t * 0.7 + 1.5).cos(), 0.3, 220, 0.18),
                ((t * 0.5 + 3.0).sin(), (t * 0.5 + 3.0).cos(), 0.7, 240, 0.15),
                ((t * 0.3 + 4.5).sin(), (t * 0.3 + 4.5).cos(), 0.6, 200, 0.12),
                ((t * 0.9 + 2.0).sin(), (t * 0.9 + 2.0).cos(), 0.4, 180, 0.10),
                ((t * 0.4 + 5.5).sin(), (t * 0.4 + 5.5).cos(), 0.8, 160, 0.08),
            ];
            for (sx, sy, cx, size, brightness) in clouds {
                let b = brightness * brightness_scale;
                let px = (sx * 250.0 + ow as f32 * cx) as i32;
                let py = (sy * 180.0 + oh as f32 * 0.5) as i32;
                f.clear(opaque(accent.0 * b, accent.1 * b, accent.2 * b),
                    &[rect(px - size / 2, py - size / 2, size, size)]).ok();
                // 内核
                let inner = size / 3;
                f.clear(opaque(accent.0 * b * 2.5, accent.1 * b * 2.5, accent.2 * b * 2.5),
                    &[rect(px - inner / 2, py - inner / 2, inner, inner)]).ok();
            }
            // 闪烁星点
            let seed = frame / 3; // 每3帧变一次
            let mut star_rects: Vec<Rectangle<i32, Physical>> = Vec::new();
            for i in 0..40u32 {
                let h = hash_rand(seed, i);
                let sx = (h & 0xFFFF) as i32 * ow / 65536;
                let sy = ((h >> 16) & 0xFFFF) as i32 * oh / 65536;
                let twinkle = ((frame + i) % 60) as f32 / 60.0;
                if twinkle > 0.4 {
                    star_rects.push(rect(sx, sy, 2, 2));
                }
            }
            if !star_rects.is_empty() {
                let b = 0.6 * brightness_scale;
                f.clear(opaque(accent.0 * b + 0.35 * brightness_scale, accent.1 * b + 0.35 * brightness_scale, accent.2 * b + 0.4 * brightness_scale), &star_rects).ok();
            }
        }

        // ── Style 1: Wave 波纹 ──
        1 => {
            // 多层水平正弦波
            let layers: [(f32, f32, f32, f32); 5] = [
                (0.015, 0.3, 0.008, 0.25),   // freq, amp, speed, brightness
                (0.020, 0.2, 0.012, 0.20),
                (0.010, 0.4, 0.006, 0.16),
                (0.025, 0.15, 0.015, 0.22),
                (0.018, 0.25, 0.010, 0.18),
            ];
            for (freq, amp, speed, brightness) in layers {
                let b = brightness * brightness_scale;
                let base_y = oh as f32 * 0.5;
                let mut wave_rects: Vec<Rectangle<i32, Physical>> = Vec::new();
                for x in (0..ow).step_by(3) {
                    let y = base_y + (x as f32 * freq + t * speed * 1000.0).sin() * oh as f32 * amp;
                    let yo = y as i32;
                    wave_rects.push(rect(x, yo, 3, 4));
                }
                if !wave_rects.is_empty() {
                    f.clear(opaque(accent.0 * b, accent.1 * b, accent.2 * b), &wave_rects).ok();
                }
            }
            // 扫描线（基于时间的平滑移动）
            let scan_y = (elapsed * 84.0) as i32 % oh;
            let b = 0.30 * brightness_scale;
            f.clear(opaque(accent.0 * b, accent.1 * b, accent.2 * b),
                &[rect(0, scan_y, ow, 3)]).ok();
            // 扫描线光晕
            f.clear(opaque(accent.0 * b * 0.3, accent.1 * b * 0.3, accent.2 * b * 0.3),
                &[rect(0, scan_y - 6, ow, 6)]).ok();
            f.clear(opaque(accent.0 * b * 0.3, accent.1 * b * 0.3, accent.2 * b * 0.3),
                &[rect(0, scan_y + 3, ow, 6)]).ok();
        }

        // ── Style 2: Grid 赛博网格 ──
        2 => {
            let grid_size = 48i32;
            let b1 = 0.18 * brightness_scale;
            let b2 = 0.30 * brightness_scale;
            // 竖线
            let mut v_lines: Vec<Rectangle<i32, Physical>> = Vec::new();
            for x in (0..ow).step_by(grid_size as usize) {
                v_lines.push(rect(x, 0, 1, oh));
            }
            if !v_lines.is_empty() {
                f.clear(opaque(accent.0 * b1, accent.1 * b1, accent.2 * b1), &v_lines).ok();
            }
            // 横线
            let mut h_lines: Vec<Rectangle<i32, Physical>> = Vec::new();
            for y in (0..oh).step_by(grid_size as usize) {
                h_lines.push(rect(0, y, ow, 1));
            }
            if !h_lines.is_empty() {
                f.clear(opaque(accent.0 * b1, accent.1 * b1, accent.2 * b1), &h_lines).ok();
            }
            // 交叉点高亮
            let mut dots: Vec<Rectangle<i32, Physical>> = Vec::new();
            for x in (0..ow).step_by(grid_size as usize) {
                for y in (0..oh).step_by(grid_size as usize) {
                    dots.push(rect(x - 1, y - 1, 3, 3));
                }
            }
            if !dots.is_empty() {
                f.clear(opaque(accent.0 * b2, accent.1 * b2, accent.2 * b2), &dots).ok();
            }
            // 垂直扫描线（来回，基于时间）
            let scan_raw = (elapsed * 126.0) as i32 % (ow * 2);
            let scan_x = if scan_raw > ow { ow * 2 - scan_raw } else { scan_raw };
            let sb = 0.40 * brightness_scale;
            // 扫描线本体
            f.clear(opaque(accent.0 * sb, accent.1 * sb, accent.2 * sb),
                &[rect(scan_x, 0, 4, oh)]).ok();
            // 扫描线光晕
            for (dx, glow_b) in [(-12i32, 0.06f32), (-6, 0.12), (6, 0.12), (12, 0.06)] {
                let gx = scan_x + dx;
                if gx >= 0 && gx < ow {
                    f.clear(opaque(accent.0 * glow_b * brightness_scale, accent.1 * glow_b * brightness_scale, accent.2 * glow_b * brightness_scale),
                        &[rect(gx, 0, 2, oh)]).ok();
                }
            }
        }

        // ── Style 3: Rings 同心圆脉冲 ──
        3 => {
            let cx = ow / 2;
            let cy = oh / 2;
            let n_rings = 8usize;
            let max_radius = (ow.max(oh) as f32 * 0.7) as i32;
            for i in 0..n_rings {
                let phase = (t * 0.5 + i as f32 * 0.8) % (n_rings as f32 * 0.8);
                let radius = (phase / (n_rings as f32 * 0.8) * max_radius as f32) as i32;
                if radius < 4 { continue; }
                let fade = 1.0 - phase / (n_rings as f32 * 0.8);
                let b = fade * 0.28 * brightness_scale;
                let thickness = 2 + (fade * 4.0) as i32;
                let r = radius;
                let mut arc: Vec<Rectangle<i32, Physical>> = Vec::new();
                for angle_step in 0..90 {
                    let angle = angle_step as f32 * std::f32::consts::PI * 2.0 / 90.0;
                    let px = cx + (r as f32 * angle.cos()) as i32;
                    let py = cy + (r as f32 * angle.sin()) as i32;
                    arc.push(rect(px - thickness / 2, py - thickness / 2, thickness, thickness));
                }
                if !arc.is_empty() {
                    f.clear(opaque(accent.0 * b, accent.1 * b, accent.2 * b), &arc).ok();
                }
            }
            // 中心发光点（脉冲呼吸）
            let pulse = 0.5 + 0.5 * (t * 2.0).sin();
            let b = pulse * 0.5 * brightness_scale;
            f.clear(opaque(accent.0 * b + 0.15 * brightness_scale, accent.1 * b + 0.15 * brightness_scale, accent.2 * b + 0.18 * brightness_scale),
                &[rect(cx - 6, cy - 6, 12, 12)]).ok();
            // 中心光晕
            f.clear(opaque(accent.0 * b * 0.3, accent.1 * b * 0.3, accent.2 * b * 0.3),
                &[rect(cx - 20, cy - 20, 40, 40)]).ok();
        }

        // ── Style 4: Aurora 极光 ──
        _ => {
            // 多条飘动的彩色条带
            let bands: [(f32, f32, f32, f32); 5] = [
                (0.3, 0.004, 0.01, 0.22),
                (0.4, 0.006, 0.015, 0.18),
                (0.5, 0.003, 0.008, 0.25),
                (0.6, 0.005, 0.012, 0.16),
                (0.7, 0.007, 0.009, 0.20),
            ];
            for (base_y_ratio, freq, speed, brightness) in bands {
                let b = brightness * brightness_scale;
                let base_y = oh as f32 * base_y_ratio;
                let mut band_rects: Vec<Rectangle<i32, Physical>> = Vec::new();
                for x in (0..ow).step_by(4) {
                    let wave1 = (x as f32 * freq + t * speed * 800.0).sin();
                    let wave2 = (x as f32 * freq * 1.5 + t * speed * 600.0 + 2.0).sin() * 0.5;
                    let y = base_y + (wave1 + wave2) * oh as f32 * 0.10;
                    let thickness = (6.0 + 3.0 * wave1.abs()) as i32;
                    band_rects.push(rect(x, y as i32, 4, thickness));
                }
                if !band_rects.is_empty() {
                    // 极光用偏绿/青色调
                    let ar = accent.0 * 0.4 + 0.10;
                    let ag = accent.1 * 0.4 + 0.25;
                    let ab = accent.2 * 0.4 + 0.15;
                    f.clear(opaque(ar * b, ag * b, ab * b), &band_rects).ok();
                }
            }
            // 底部渐变
            let grad_h = oh / 4;
            for i in 0..6 {
                let fade = (i as f32 / 6.0) * 0.08 * brightness_scale;
                f.clear(opaque(accent.0 * fade, accent.1 * fade, accent.2 * fade),
                    &[rect(0, oh - grad_h + i * grad_h / 5, ow, grad_h / 5)]).ok();
            }
        }
    }
}

/// 渲染锁屏覆盖层（全屏暗色覆盖 + 居中密码输入框 + 大时钟）
pub fn render_lock_screen(
    f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32,
    time_secs: u64, elapsed: f32,
    password: &str, wrong: bool, shake: Option<std::time::Instant>,
    style: u8,
) {
    let accent = parse_color(&cfg.colors.focus_border);

    // ── 全屏暗色覆盖 ──
    f.clear(opaque(0.03, 0.03, 0.06), &[rect(0, 0, ow, oh)]).ok();

    // ── 背景动画效果 ──
    render_lock_bg(f, accent, ow, oh, elapsed, style, 1.0);

    // ── Shake 偏移（密码错误时抖动） ──
    let mut shake_x = 0i32;
    if let Some(shake_start) = shake {
        let elapsed = shake_start.elapsed().as_secs_f32();
        if elapsed < 0.4 {
            // Damped sine wave shake
            let damping = 1.0 - elapsed / 0.4;
            shake_x = (12.0 * damping * (elapsed * 30.0).sin()) as i32;
        }
    }

    let cx = ow / 2 + shake_x;

    // ── 大时钟 ──
    let time_secs_c = time_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&time_secs_c, &mut tm) };
    let hours = tm.tm_hour as u8;
    let minutes = tm.tm_min as u8;

    let clock_str = format!("{:02}:{:02}", hours, minutes);
    let clock_size = 72.0;
    let clock_w = text_render::text_width(&clock_str, clock_size);
    let clock_y = oh / 2 - 160;
    text_render::draw_text(f, &clock_str, cx - clock_w / 2, clock_y, clock_size,
        (accent.0 * 0.9, accent.1 * 0.9, accent.2 * 0.9));

    // ── 日期 ──
    let month = (tm.tm_mon + 1) as u8;
    let day = tm.tm_mday as u8;
    let weekday = match tm.tm_wday {
        0 => "Sunday", 1 => "Monday", 2 => "Tuesday", 3 => "Wednesday",
        4 => "Thursday", 5 => "Friday", 6 => "Saturday", _ => "",
    };
    let date_str = format!("{}, {}-{:02}-{:02}", weekday, tm.tm_year + 1900, month, day);
    let date_w = text_render::text_width(&date_str, 20.0);
    text_render::draw_text(f, &date_str, cx - date_w / 2, clock_y + 82, 20.0,
        (0.4, 0.4, 0.5));

    // ── 用户名 ──
    let username = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let user_w = text_render::text_width(&username, 24.0);
    text_render::draw_text(f, &username, cx - user_w / 2, oh / 2 - 50, 24.0,
        (0.6, 0.6, 0.65));

    // ── 锁图标（用方块手绘） ──
    {
        let lx = cx;
        let ly = oh / 2 - 20;
        let accent_c = opaque(accent.0, accent.1, accent.2);
        // 锁身（方形）
        f.clear(opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
            &[rect(lx - 12, ly, 24, 20)]).ok();
        f.clear(accent_c, &[rect(lx - 12, ly, 24, 2)]).ok();
        f.clear(accent_c, &[rect(lx - 12, ly + 18, 24, 2)]).ok();
        f.clear(accent_c, &[rect(lx - 12, ly, 2, 20)]).ok();
        f.clear(accent_c, &[rect(lx + 10, ly, 2, 20)]).ok();
        // 锁环（拱形）
        f.clear(accent_c, &[rect(lx - 8, ly - 12, 2, 14)]).ok();
        f.clear(accent_c, &[rect(lx + 6, ly - 12, 2, 14)]).ok();
        f.clear(accent_c, &[rect(lx - 8, ly - 14, 16, 2)]).ok();
        // 锁孔
        f.clear(accent_c, &[rect(lx - 1, ly + 6, 2, 2)]).ok();
        f.clear(accent_c, &[rect(lx - 1, ly + 8, 2, 5)]).ok();
    }

    // ── 密码输入框 ──
    let box_w = 360.min(ow * 3 / 5);
    let box_h = 44;
    let box_x = cx - box_w / 2;
    let box_y = oh / 2 + 24;

    // 输入框背景
    let input_bg = if wrong {
        opaque(0.12, 0.04, 0.04)
    } else {
        opaque(0.06, 0.06, 0.10)
    };
    f.clear(input_bg, &[rect(box_x, box_y, box_w, box_h)]).ok();

    // 发光边框效果（多层渐变）
    let glow_layers: [(i32, f32); 5] = [
        (6, 0.04), (4, 0.08), (3, 0.15), (2, 0.3), (1, 0.6),
    ];
    for (expand, brightness) in glow_layers {
        let glow = if wrong {
            opaque(0.6 * brightness, 0.1 * brightness, 0.1 * brightness)
        } else {
            opaque(accent.0 * brightness, accent.1 * brightness, accent.2 * brightness)
        };
        f.clear(glow, &[rect(box_x - expand, box_y - expand, box_w + 2 * expand, expand)]).ok(); // top
        f.clear(glow, &[rect(box_x - expand, box_y + box_h, box_w + 2 * expand, expand)]).ok(); // bottom
        f.clear(glow, &[rect(box_x - expand, box_y, expand, box_h)]).ok(); // left
        f.clear(glow, &[rect(box_x + box_w, box_y, expand, box_h)]).ok(); // right
    }

    // ── 密码圆点 ──
    let dot_radius = 4;
    let dot_gap = 16;
    let max_dots = (box_w as i32 - 40) / dot_gap;
    let n_dots = password.len().min(max_dots as usize);
    let dots_width = n_dots as i32 * dot_gap;
    let dots_start = cx - dots_width / 2 + dot_gap / 2;

    let dot_color = if wrong {
        opaque(0.9, 0.3, 0.3)
    } else {
        opaque(accent.0 * 0.9 + 0.1, accent.1 * 0.9 + 0.1, accent.2 * 0.9 + 0.1)
    };

    for i in 0..n_dots {
        let dx = dots_start + i as i32 * dot_gap - dot_radius;
        let dy = box_y + box_h / 2 - dot_radius;
        f.clear(dot_color, &[rect(dx, dy, dot_radius * 2, dot_radius * 2)]).ok();
    }

    // ── 闪烁光标（基于时间的 ~1Hz 闪烁） ──
    let cursor_visible = (elapsed * 1.0).sin() > 0.0;
    if cursor_visible {
        let cursor_x = dots_start + n_dots as i32 * dot_gap + 4;
        f.clear(dot_color, &[rect(cursor_x, box_y + 10, 2, box_h - 20)]).ok();
    }

    // ── 提示文字 ──
    if wrong {
        let hint = "Authentication failed";
        let hw = text_render::text_width(hint, 14.0);
        text_render::draw_text(f, hint, cx - hw / 2, box_y + box_h + 14, 14.0,
            (0.9, 0.3, 0.3));
    } else if password.is_empty() {
        let hint = "Enter password to unlock";
        let hw = text_render::text_width(hint, 14.0);
        text_render::draw_text(f, hint, cx - hw / 2, box_y + box_h + 14, 14.0,
            (0.3, 0.3, 0.4));
    }

    // ── 底部装饰线 ──
    let line_w = 120;
    let line_y = oh / 2 + 120;
    f.clear(opaque(accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15),
        &[rect(cx - line_w, line_y, line_w * 2, 1)]).ok();
    // 两端小方块
    f.clear(opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
        &[rect(cx - line_w - 2, line_y - 2, 4, 4)]).ok();
    f.clear(opaque(accent.0 * 0.3, accent.1 * 0.3, accent.2 * 0.3),
        &[rect(cx + line_w - 2, line_y - 2, 4, 4)]).ok();

    // ── 风格标签（右下角小字） ──
    let style_names = ["NEBULA", "WAVE", "CYBER", "RINGS", "AURORA"];
    let style_label = style_names.get(style as usize).unwrap_or(&"UNKNOWN");
    let label_w = text_render::text_width(style_label, 11.0);
    text_render::draw_text(f, style_label, ow - label_w - 16, oh - 20, 11.0,
        (accent.0 * 0.15, accent.1 * 0.15, accent.2 * 0.15));
}
