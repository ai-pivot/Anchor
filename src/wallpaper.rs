//! 壁纸加载与渲染
//! 支持从 ~/Pictures/ 目录加载图片作为壁纸
//! 使用 32x32 块渲染以平衡性能和质量

use crate::config::Config;
use smithay::{
    backend::renderer::{Color32F, Frame},
    utils::{Point, Rectangle, Size},
};
use std::path::{Path, PathBuf};

#[inline(always)]
fn opaque(r: f32, g: f32, b: f32) -> Color32F {
    Color32F::new(r, g, b, 1.0)
}

/// 壁纸缓存
pub struct WallpaperCache {
    /// 预渲染的像素数据 (RGBA, 4 bytes/pixel)，已缩放到屏幕尺寸
    pub pixels: Option<Vec<u8>>,
    /// 缓存的图片尺寸
    pub size: (usize, usize),
    /// 当前图片路径
    current_path: Option<PathBuf>,
    /// 目录中的所有图片
    directory_images: Vec<PathBuf>,
    /// 当前图片索引
    current_index: usize,
    /// 上次切换时间
    last_change: std::time::Instant,
}

impl WallpaperCache {
    pub fn new() -> Self {
        Self {
            pixels: None,
            size: (0, 0),
            current_path: None,
            directory_images: Vec::new(),
            current_index: 0,
            last_change: std::time::Instant::now(),
        }
    }

    /// 扫描目录中的图片文件
    pub fn scan_directory(&mut self, dir: &str) {
        if dir.is_empty() {
            return;
        }
        let path = Path::new(dir);
        if !path.exists() {
            let home = std::env::var("HOME").unwrap_or_default();
            let alt = Path::new(&home).join("Pictures/wallpapers");
            if alt.exists() {
                self.directory_images = scan_image_files(&alt);
            }
            return;
        }
        self.directory_images = scan_image_files(path);
        if !self.directory_images.is_empty() {
            eprintln!("🖼️  扫描到 {} 张壁纸", self.directory_images.len());
        }
    }

    /// 加载壁纸（首次或切换）
    pub fn load(&mut self, path: &str, screen_w: usize, screen_h: usize) -> bool {
        let target = if !path.is_empty() {
            Some(PathBuf::from(path))
        } else if !self.directory_images.is_empty() {
            // 随机选一张（如果 mode 是 "random"）
            Some(self.directory_images[self.current_index].clone())
        } else {
            None
        };

        let Some(target) = target else { return false };

        if self.current_path.as_ref() == Some(&target) && self.pixels.is_some() {
            return true;
        }

        match load_and_scale(&target, screen_w, screen_h) {
            Ok((pixels, w, h)) => {
                eprintln!("🖼️  壁纸加载: {:?} ({}x{})", target.file_name(), w, h);
                self.pixels = Some(pixels);
                self.size = (w, h);
                self.current_path = Some(target);
                true
            }
            Err(e) => {
                eprintln!("⚠️  壁纸加载失败 {:?}: {}", target, e);
                false
            }
        }
    }

    /// 检查是否需要切换到下一张壁纸
    pub fn check_rotation(&mut self, interval_secs: u64) {
        if interval_secs == 0 || self.directory_images.len() <= 1 {
            return;
        }
        if self.last_change.elapsed().as_secs() >= interval_secs {
            self.current_index = (self.current_index + 1) % self.directory_images.len();
            self.pixels = None;
            self.current_path = None;
            self.last_change = std::time::Instant::now();
            eprintln!("🖼️  切换壁纸 → 索引 {}", self.current_index);
        }
    }

    /// 渲染缓存的壁纸到帧缓冲
    /// 使用 32x32 块采样，大幅减少 draw call
    pub fn render(&self, f: &mut impl Frame, cfg: &Config, ow: i32, oh: i32) -> bool {
        if let Some(ref pixels) = self.pixels {
            let (w, h) = self.size;
            if w == 0 || h == 0 || w != ow as usize || h != oh as usize {
                return false;
            }

            // 先画纯色背景
            let bg = crate::config::parse_color(&cfg.wallpaper.color);
            f.clear(
                opaque(bg.0, bg.1, bg.2),
                &[Rectangle::from_size(Size::new(ow, oh))],
            )
            .ok();

            // 8x8 块渲染（平衡质量和性能）
            let block = 8;
            let cols = (ow as usize + block - 1) / block;
            let rows = (oh as usize + block - 1) / block;

            for by in 0..rows {
                for bx in 0..cols {
                    let dst_x = bx * block;
                    let dst_y = by * block;

                    // 采样块中心像素
                    let cx = dst_x + block / 2;
                    let cy = dst_y + block / 2;
                    if cx >= w || cy >= h {
                        continue;
                    }

                    let idx = (cy * w + cx) * 4;
                    if idx + 3 >= pixels.len() {
                        continue;
                    }

                    let r = pixels[idx] as f32 / 255.0;
                    let g = pixels[idx + 1] as f32 / 255.0;
                    let b = pixels[idx + 2] as f32 / 255.0;

                    let bw = block.min(ow as usize - dst_x) as i32;
                    let bh = block.min(oh as usize - dst_y) as i32;
                    if bw <= 0 || bh <= 0 {
                        continue;
                    }

                    f.clear(
                        opaque(r, g, b),
                        &[Rectangle::new(
                            Point::new(dst_x as i32, dst_y as i32),
                            Size::new(bw, bh),
                        )],
                    )
                    .ok();
                }
            }
            return true;
        }
        false
    }
}

fn scan_image_files(dir: &Path) -> Vec<PathBuf> {
    let exts = ["jpg", "jpeg", "png", "bmp", "webp", "tiff", "gif"];
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| exts.contains(&ext.to_lowercase().as_str()))
                        .unwrap_or(false)
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn load_and_scale(
    path: &Path,
    target_w: usize,
    target_h: usize,
) -> Result<(Vec<u8>, usize, usize), String> {
    let img = image::io::Reader::open(path)
        .map_err(|e| format!("打开失败: {}", e))?
        .decode()
        .map_err(|e| format!("解码失败: {}", e))?;

    eprintln!("🖼️  原始尺寸: {}x{}", img.width(), img.height());

    let rgba = img
        .resize_exact(
            target_w as u32,
            target_h as u32,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();

    let (fw, fh) = (rgba.width() as usize, rgba.height() as usize);
    Ok((rgba.into_raw(), fw, fh))
}
