// screenshot.rs — 截图功能模块
// 支持：全屏截图、区域选择截图、保存文件 + 复制到剪贴板
// 通过 GlesRenderer::copy_framebuffer 在渲染流程中截取像素

use std::fs::File;
use std::io::Write;
use std::path::Path;

/// 截图状态：管理区域选择的交互
pub struct ScreenshotState {
    pub selecting: bool,
    pub start: Option<(f64, f64)>,
    pub end: Option<(f64, f64)>,
}

impl ScreenshotState {
    pub fn new() -> Self {
        Self { selecting: false, start: None, end: None }
    }
    pub fn begin_selection(&mut self) {
        self.selecting = true;
        self.start = None;
        self.end = None;
    }
    pub fn cancel(&mut self) {
        self.selecting = false;
        self.start = None;
        self.end = None;
    }
    pub fn on_press(&mut self, x: f64, y: f64) {
        if self.selecting {
            self.start = Some((x, y));
            self.end = Some((x, y));
        }
    }
    pub fn on_motion(&mut self, x: f64, y: f64) {
        if self.selecting && self.start.is_some() {
            self.end = Some((x, y));
        }
    }
    pub fn on_release(&mut self) -> Option<(i32, i32, i32, i32)> {
        if let (Some((sx, sy)), Some((ex, ey))) = (self.start, self.end) {
            let x = sx.min(ex) as i32;
            let y = sy.min(ey) as i32;
            let w = (sx - ex).abs() as i32;
            let h = (sy - ey).abs() as i32;
            self.selecting = false;
            self.start = None;
            self.end = None;
            if w < 5 || h < 5 { None } else { Some((x, y, w, h)) }
        } else {
            self.cancel();
            None
        }
    }
    pub fn selection_rect(&self) -> Option<(i32, i32, i32, i32)> {
        if let (Some((sx, sy)), Some((ex, ey))) = (self.start, self.end) {
            let x = sx.min(ex) as i32;
            let y = sy.min(ey) as i32;
            let w = (sx - ex).abs() as i32;
            let h = (sy - ey).abs() as i32;
            if w > 0 && h > 0 { Some((x, y, w, h)) } else { None }
        } else {
            None
        }
    }
}

/// 截图请求类型
#[derive(Clone)]
pub enum ScreenshotRequest {
    /// 全屏截图
    Full,
    /// 区域截图 (x, y, w, h)
    Area(i32, i32, i32, i32),
}

/// 将 RGBA 数据编码为 PNG 并保存
/// 返回 (文件路径, PNG 二进制数据)
pub fn save_screenshot(rgba: &[u8], width: u32, height: u32, area: Option<(i32, i32, i32, i32)>) -> (String, Option<Vec<u8>>) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::PathBuf::from(format!("{}/Pictures/Screenshots", home));
    let _ = std::fs::create_dir_all(&dir);
    let png_path = dir.join(format!("anchor-{}.png", ts));

    let (final_rgba, final_w, final_h) = if let Some((x, y, w, h)) = area {
        crop_rgba(rgba, width, height, x, y, w, h)
    } else {
        (rgba.to_vec(), width, height)
    };

    match encode_png(&final_rgba, final_w, final_h) {
        Ok(png_data) => {
            if let Ok(mut f) = File::create(&png_path) {
                let _ = f.write_all(&png_data);
            }
            tracing::info!("📸 截图已保存: {} ({}x{}, {} bytes)", png_path.display(), final_w, final_h, png_data.len());
            (png_path.display().to_string(), Some(png_data))
        }
        Err(e) => {
            tracing::warn!("📸 PNG 编码失败: {}", e);
            (String::new(), None)
        }
    }
}

/// 裁剪 RGBA 数据
fn crop_rgba(rgba: &[u8], src_w: u32, src_h: u32, x: i32, y: i32, w: i32, h: i32) -> (Vec<u8>, u32, u32) {
    let x = x.max(0) as u32;
    let y = y.max(0) as u32;
    let w = w.max(1) as u32;
    let h = h.max(1) as u32;
    let w = w.min(src_w.saturating_sub(x));
    let h = h.min(src_h.saturating_sub(y));
    let mut cropped = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let src_offset = (row * src_w + x) as usize * 4;
        let end = src_offset + w as usize * 4;
        if end <= rgba.len() {
            cropped.extend_from_slice(&rgba[src_offset..end]);
        }
    }
    (cropped, w, h)
}

/// RGBA 数据编码为 PNG
fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::{ImageBuffer, RgbaImage};
    let img: RgbaImage = ImageBuffer::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "ImageBuffer::from_raw failed".to_string())?;
    let mut png_data = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png_data, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encode failed: {}", e))?;
    Ok(png_data.into_inner())
}

/// 渲染区域选择 overlay
pub fn render_selection_overlay(
    f: &mut impl smithay::backend::renderer::Frame,
    screen_w: i32,
    screen_h: i32,
    rect: (i32, i32, i32, i32),
) {
    let (x, y, w, h) = rect;
    let dim = crate::layout::opaque(0.0, 0.0, 0.0);

    if y > 0 { f.clear(dim, &[crate::layout::rect(0, 0, screen_w, y)]).ok(); }
    if y + h < screen_h { f.clear(dim, &[crate::layout::rect(0, y + h, screen_w, screen_h - y - h)]).ok(); }
    if x > 0 { f.clear(dim, &[crate::layout::rect(0, y, x, h)]).ok(); }
    if x + w < screen_w { f.clear(dim, &[crate::layout::rect(x + w, y, screen_w - x - w, h)]).ok(); }

    let border = crate::layout::opaque(0.48, 0.64, 0.97);
    let bw = 2;
    f.clear(border, &[crate::layout::rect(x, y, w, bw)]).ok();
    f.clear(border, &[crate::layout::rect(x, y + h - bw, w, bw)]).ok();
    f.clear(border, &[crate::layout::rect(x, y, bw, h)]).ok();
    f.clear(border, &[crate::layout::rect(x + w - bw, y, bw, h)]).ok();

    let size_text = format!("{}x{}", w, h);
    crate::text_render::draw_text(f, &size_text, x + w + 6, y - 4, 13.0, (1.0, 1.0, 1.0));
}
