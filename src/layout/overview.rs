//! Overview overlay rendering — Task Panel drawer and Bird's Eye View grid.
//!
//! Renders on top of the normal desktop (Step 4.8 in the pipeline).
//! Uses `Frame::clear()` for backgrounds and text_render for labels.
//!
//! **Important**: `Frame::clear()` draws 100% opaque rectangles — there is NO alpha blending.
//! To simulate transparency/fade-in, all colors must be modulated by raw `progress`
//! (NOT eased progress) to ramp from near-background-color to the target color.

use super::util::{
    ease_out_expo, opaque, rect, S2, S3, S4, TITLE_SIZE,
};
use crate::config::Config;
use crate::text_render;
use smithay::backend::renderer::{Color32F, Frame};

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

    // Window thumbnails are now rendered in main.rs Phase 1.5 + Step 4.8
    // using real slot() layout + render_elements_from_surface_tree

    // Bottom shadow (floating panel effect)
    for (si, sb) in [(0i32, 0.12f32), (1, 0.06), (2, 0.03)].iter() {
        f.clear(
            Color32F::new(0.0, 0.0, 0.0, sb * color_alpha),
            &[rect(panel_x - 4, panel_y - 2 - si, panel_w + 8, 2)],
        ).ok();
    }
}
