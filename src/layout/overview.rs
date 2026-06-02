//! Overview overlay rendering — Task Panel drawer and Bird's Eye View grid.
//!
//! Renders on top of the normal desktop (Step 4.8 in the pipeline).
//! Uses `Frame::clear()` for backgrounds and text_render for labels.
//!
//! **Important**: `Frame::clear()` draws 100% opaque rectangles — there is NO alpha blending.
//! To simulate transparency/fade-in, all colors must be modulated by raw `progress`
//! (NOT eased progress) to ramp from near-background-color to the target color.

use super::util::{
    ease_out_back, ease_out_expo, opaque, rect, S2, S3, S4, S6, TITLE_SIZE, WS_SIZE,
};
use crate::config::Config;
use crate::text_render;
use smithay::backend::renderer::Frame;

/// Render the Task Panel overlay — a bottom drawer showing current workspace windows.
///
/// `progress`: 0.0 = hidden (off-screen bottom), 1.0 = fully visible
pub fn render_task_panel(
    f: &mut impl Frame,
    cfg: &Config,
    ow: i32,
    oh: i32,
    progress: f64,
    active_ws: usize,
    window_titles: &[String],
    focus_idx: Option<usize>,
    total_windows: usize,
) {
    // Don't render at all when progress is very low — prevents black flash
    // (Frame::clear is always 100% opaque, so even tiny progress = visible dark block)
    if progress < 0.08 {
        return;
    }
    let accent = crate::config::parse_color(&cfg.colors.focus_border);

    // Use raw progress for color modulation (NOT eased), so colors fade from background
    let color_alpha = (progress as f32).min(1.0);
    // Use eased progress only for positioning
    // ease_out_expo: 快速展开无过冲（弹簧本身已有物理过冲，不用 ease_out_back 避免双重过冲）
    let p = ease_out_expo(progress as f32);

    // Panel geometry — bottom 35% of screen
    let panel_h = (oh as f32 * 0.35) as i32;
    let panel_y = oh - (panel_h as f32 * p) as i32;
    let panel_x = S3;
    let panel_w = ow - S3 * 2;

    // Background — dark panel (modulated by raw progress to prevent flash)
    let br = 0.04 * color_alpha;
    let bg = (br, br, (br + 0.04 * color_alpha).min(0.12));
    f.clear(opaque(bg.0, bg.1, bg.2), &[rect(panel_x, panel_y, panel_w, panel_h)])
        .ok();

    // Top accent line
    f.clear(
        opaque(accent.0 * 0.6 * color_alpha, accent.1 * 0.6 * color_alpha, accent.2 * 0.6 * color_alpha),
        &[rect(panel_x, panel_y, panel_w, 1)],
    )
    .ok();

    // Glow below accent line
    for (off, glow_br) in [(1, 0.3f32), (2, 0.12), (3, 0.04)] {
        f.clear(
            opaque(accent.0 * glow_br * color_alpha, accent.1 * glow_br * color_alpha, accent.2 * glow_br * color_alpha),
            &[rect(panel_x, panel_y + off, panel_w, 1)],
        )
        .ok();
    }

    // Title
    let title = format!("Workspace {} — {} windows", active_ws + 1, total_windows);
    text_render::draw_text(
        f,
        &title,
        panel_x + S4,
        panel_y + S2,
        TITLE_SIZE,
        (accent.0 * 0.7 * color_alpha, accent.1 * 0.7 * color_alpha, accent.2 * 0.7 * color_alpha),
    );

    // Window thumbnail grid
    if total_windows > 0 && progress > 0.15 {
        let thumb_w = 180i32;
        let thumb_h = 120i32;
        let thumb_gap = S3;
        let cols = ((panel_w - S4 * 2) / (thumb_w + thumb_gap)).max(1);
        let start_y = panel_y + S4 + 24;

        for i in 0..total_windows as i32 {
            let col = i % cols;
            let row = i / cols;
            let tx = panel_x + S4 + col * (thumb_w + thumb_gap);
            let ty = start_y + row * (thumb_h + thumb_gap + 16);

            if ty + thumb_h > oh {
                break;
            }

            // Staggered appearance
            let stagger = (i as f32 * 0.05).min(0.5);
            let item_p = ((progress as f32 - stagger) / (1.0 - stagger + 0.01)).max(0.0).min(1.0);
            let item_alpha = ease_out_expo(item_p) * color_alpha;

            let is_focused = focus_idx == Some(i as usize);

            // Thumbnail background
            let bg_br = if is_focused { 0.12 } else { 0.06 } * item_alpha;
            f.clear(
                opaque(bg_br, bg_br, (bg_br + 0.04 * item_alpha).min(0.16)),
                &[rect(tx, ty, thumb_w, thumb_h)],
            )
            .ok();

            // Border for focused
            if is_focused {
                f.clear(
                    opaque(accent.0 * 0.8 * item_alpha, accent.1 * 0.8 * item_alpha, accent.2 * 0.8 * item_alpha),
                    &[rect(tx, ty, thumb_w, 2)],
                )
                .ok();
            }

            // Window title below thumbnail
            let title = window_titles.get(i as usize).cloned().unwrap_or_default();
            let truncated = if title.len() > 20 {
                format!("{}...", &title[..17])
            } else {
                title
            };
            text_render::draw_text(
                f,
                &truncated,
                tx + 4,
                ty + thumb_h + 4,
                11.0,
                (
                    accent.0 * 0.5 * item_alpha,
                    accent.1 * 0.5 * item_alpha,
                    accent.2 * 0.5 * item_alpha,
                ),
            );
        }
    }
}

/// Render the Bird's Eye View — all workspaces as thumbnails in a 3×3 grid.
///
/// `progress`: 0.0 = normal view, 1.0 = full overview
pub fn render_overview(
    f: &mut impl Frame,
    cfg: &Config,
    ow: i32,
    oh: i32,
    progress: f64,
    active_ws: usize,
    workspace_window_counts: &[usize],
    hover_ws: Option<usize>,
) {
    // Don't render at very low progress — prevents full-screen black flash
    if progress < 0.08 {
        return;
    }
    let accent = crate::config::parse_color(&cfg.colors.focus_border);

    // Raw progress for color modulation (prevents flash at low progress)
    let color_alpha = (progress as f32).min(1.0);
    // Eased progress for grid item appearance
    let p = ease_out_expo(progress as f32);

    // Full-screen overlay (modulated by raw progress)
    let ov_br = 0.06 * color_alpha;
    f.clear(opaque(ov_br, ov_br, (ov_br + 0.04 * color_alpha).min(0.10)), &[rect(0, 0, ow, oh)])
        .ok();

    // 3×3 grid of workspace thumbnails
    let total = workspace_window_counts.len().min(9);
    let cols = 3;
    let grid_gap = S4;
    let grid_margin_x = S6 * 2;
    let grid_margin_top = S6 * 3;
    let grid_w = ow - grid_margin_x * 2;
    let grid_h = oh - grid_margin_top - S6;
    let cell_w = (grid_w - (cols - 1) * grid_gap) / cols;
    let cell_h = (grid_h - 2 * grid_gap) / 3;

    // Title
    text_render::draw_text(
        f,
        "OVERVIEW",
        ow / 2 - text_render::text_width("OVERVIEW", WS_SIZE) / 2,
        S4,
        WS_SIZE,
        (accent.0 * 0.6 * color_alpha, accent.1 * 0.6 * color_alpha, accent.2 * 0.6 * color_alpha),
    );

    for i in 0..total as i32 {
        let col = i % cols;
        let row = i / cols;
        let cx = grid_margin_x + col * (cell_w + grid_gap);
        let cy = grid_margin_top + row * (cell_h + grid_gap);

        let is_active = i as usize == active_ws;
        let is_hovered = hover_ws == Some(i as usize);

        // Staggered appearance
        let stagger = ((col as f32 + row as f32) * 0.06).min(0.4);
        let item_p = ((p - stagger) / (1.0 - stagger + 0.01)).max(0.0).min(1.0);
        let item_p = ease_out_expo(item_p);

        if item_p < 0.01 {
            continue; // skip invisible items
        }

        // Workspace thumbnail background
        let bg_br = (if is_active { 0.10 } else if is_hovered { 0.08 } else { 0.04 }) * item_p * color_alpha;
        f.clear(
            opaque(bg_br, bg_br, (bg_br + 0.02 * item_p * color_alpha).min(0.12)),
            &[rect(cx, cy, cell_w, cell_h)],
        )
        .ok();

        // Active indicator — accent border
        let border_br = if is_active { 0.8 } else if is_hovered { 0.5 } else { 0.0 } * item_p * color_alpha;
        if border_br > 0.01 {
            f.clear(
                opaque(accent.0 * border_br, accent.1 * border_br, accent.2 * border_br),
                &[rect(cx, cy, cell_w, 2)],
            )
            .ok();
            if is_active {
                f.clear(
                    opaque(accent.0 * 0.15 * item_p * color_alpha, accent.1 * 0.15 * item_p * color_alpha, accent.2 * 0.15 * item_p * color_alpha),
                    &[rect(cx, cy + cell_h - 2, cell_w, 2)],
                )
                .ok();
            }
        }

        // Workspace number label
        let label = format!("WS {}", i + 1);
        text_render::draw_text(
            f,
            &label,
            cx + S2,
            cy + S2,
            12.0,
            (
                accent.0 * 0.6 * item_p * color_alpha,
                accent.1 * 0.6 * item_p * color_alpha,
                accent.2 * 0.6 * item_p * color_alpha,
            ),
        );

        // Window count
        let n = workspace_window_counts.get(i as usize).copied().unwrap_or(0);
        if n > 0 {
            let count = format!("{} windows", n);
            text_render::draw_text(
                f,
                &count,
                cx + S2,
                cy + cell_h - S4,
                10.0,
                (
                    accent.0 * 0.35 * item_p * color_alpha,
                    accent.1 * 0.35 * item_p * color_alpha,
                    accent.2 * 0.35 * item_p * color_alpha,
                ),
            );
        }
    }
}
