// screenshot.rs — 截图功能模块（纯 Rust 实现，无外部依赖）
// 支持：全屏截图、区域选择截图、保存文件 + 复制到剪贴板
// 直接通过 DRM ioctl 读取 framebuffer，用 image crate 编码 PNG

use std::fs::File;
use std::io::Write;
use std::os::unix::io::RawFd;
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

// ── DRM ioctl 命令号（x86_64，从 libdrm 头文件获取） ──────────
const DRM_IOCTL_MODE_GETRESOURCES: u64     = 0xc04064a0;
const DRM_IOCTL_MODE_GETCRTC: u64          = 0xc06864a1;
const DRM_IOCTL_MODE_GETFB: u64            = 0xc01c64ad;
const DRM_IOCTL_PRIME_HANDLE_TO_FD: u64    = 0xc00c642d;
const DRM_IOCTL_MODE_MAP_DUMB: u64         = 0xc01064b3;

fn drm_ioctl(fd: RawFd, cmd: u64, arg: *mut u8) -> i32 {
    unsafe { libc::ioctl(fd, cmd, arg) }
}

/// 从 DRM fd 读取 framebuffer 像素数据
/// 注意：调用方负责 dup fd，本函数不会关闭传入的 fd
/// 返回 (width, height, pitch, pixel_data)
fn drm_read_fb(drm_fd: RawFd) -> Result<(u32, u32, u32, Vec<u8>), String> {
    // dup fd 以避免影响调用方的 DRM 状态
    let fd = unsafe { libc::dup(drm_fd) };
    if fd < 0 {
        return Err(format!("dup DRM fd 失败 (errno={})", unsafe { *libc::__errno_location() }));
    }

    // 1. 获取 DRM resources（CRTC 列表）
    // 必须与内核 struct drm_mode_card_res 精确匹配
    #[repr(C)]
    struct DrmModeCardRes {
        fb_id_ptr: u64,
        crtc_id_ptr: u64,
        connector_id_ptr: u64,
        encoder_id_ptr: u64,
        count_fbs: u32,
        count_crtcs: u32,
        count_connectors: u32,
        count_encoders: u32,
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
    }
    let mut res: DrmModeCardRes = unsafe { std::mem::zeroed() };
    if drm_ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res as *mut _ as *mut u8) < 0 {
        unsafe { libc::close(fd); }
        return Err(format!("DRM_IOCTL_MODE_GETRESOURCES 失败 (errno={})", unsafe { *libc::__errno_location() }));
    }

    // 2. 遍历 CRTC 找第一个有 framebuffer 的
    let mut fb_id: u32 = 0;
    let crtc_count = res.count_crtcs as usize;

    // drm_mode_modeinfo: 4 + 2*5 + 2 + 4*3 + 32 = 4+10+2+12+32 = 60
    // drm_mode_crtc = 8 + 4 + 4 + 4 + 4*2 + 4 + 4 + 60 = 96
    // 但实际 sizeof = 104，可能有 padding
    // 用固定大小的 buffer 来接收 ioctl 数据，避免 struct 布局问题
    let crtc_buf_size = 104; // sizeof(struct drm_mode_crtc) on x86_64
    let mut crtc_buf = vec![0u8; crtc_buf_size];

    // struct drm_mode_crtc 偏移（从内核头文件）：
    // offset 0:  set_connectors_ptr (u64)
    // offset 8:  count_connectors (u32)
    // offset 12: crtc_id (u32)
    // offset 16: fb_id (u32)
    for i in 0..crtc_count {
        let crtc_id = unsafe { *(res.crtc_id_ptr as *const u32).add(i) };
        crtc_buf.fill(0);
        // 写入 crtc_id（offset 12）
        crtc_buf[12..16].copy_from_slice(&crtc_id.to_ne_bytes());

        if drm_ioctl(fd, DRM_IOCTL_MODE_GETCRTC, crtc_buf.as_mut_ptr() as *mut u8) < 0 {
            continue;
        }
        // 读 fb_id（offset 16）
        let this_fb = u32::from_ne_bytes(crtc_buf[16..20].try_into().unwrap());
        if this_fb != 0 {
            fb_id = this_fb;
            break;
        }
    }

    if fb_id == 0 {
        unsafe { libc::close(fd); }
        return Err("没有找到活跃的 CRTC framebuffer".into());
    }

    // 3. 获取 FB 信息
    #[repr(C)]
    struct DrmModeFbCmd {
        fb_id: u32,
        width: u32, height: u32,
        pitch: u32,
        bpp: u32,
        depth: u32,
        handle: u32,
    }
    let mut fb_cmd: DrmModeFbCmd = unsafe { std::mem::zeroed() };
    fb_cmd.fb_id = fb_id;
    if drm_ioctl(fd, DRM_IOCTL_MODE_GETFB, &mut fb_cmd as *mut _ as *mut u8) < 0 {
        unsafe { libc::close(fd); }
        return Err(format!("DRM_IOCTL_MODE_GETFB 失败 (errno={})", unsafe { *libc::__errno_location() }));
    }

    let width = fb_cmd.width;
    let height = fb_cmd.height;
    let pitch = fb_cmd.pitch;
    let handle = fb_cmd.handle;

    tracing::info!("📸 DRM FB: {}x{}, pitch={}, bpp={}, handle={}",
        width, height, pitch, fb_cmd.bpp, handle);

    // 4. 通过 PRIME handle → fd 导出 GEM buffer
    #[repr(C)]
    struct DrmPrimeHandle {
        handle: u32,
        flags: u32,
        fd: i32,
    }
    let mut prime: DrmPrimeHandle = DrmPrimeHandle { handle, flags: 0, fd: -1 };
    let prime_ok = drm_ioctl(fd, DRM_IOCTL_PRIME_HANDLE_TO_FD,
        &mut prime as *mut _ as *mut u8) >= 0;

    let size = pitch as usize * height as usize;
    let mut pixel_data = Vec::new();

    if prime_ok && prime.fd >= 0 {
        // mmap prime fd
        let mapped = unsafe {
            libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, prime.fd, 0)
        };
        if mapped != libc::MAP_FAILED {
            pixel_data.extend_from_slice(unsafe { std::slice::from_raw_parts(mapped as *const u8, size) });
            unsafe { libc::munmap(mapped, size); }
        }
        unsafe { libc::close(prime.fd); }
    }

    if pixel_data.is_empty() {
        // fallback: dumb buffer mmap
        #[repr(C)]
        struct DrmModeMapDumb {
            handle: u32,
            pad: u32,
            offset: u64,
        }
        let mut map_req: DrmModeMapDumb = DrmModeMapDumb { handle, pad: 0, offset: 0 };
        if drm_ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &mut map_req as *mut _ as *mut u8) >= 0 {
            let mapped = unsafe {
                libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, fd, map_req.offset as i64)
            };
            if mapped != libc::MAP_FAILED {
                pixel_data.extend_from_slice(unsafe { std::slice::from_raw_parts(mapped as *const u8, size) });
                unsafe { libc::munmap(mapped, size); }
            }
        }
    }

    unsafe { libc::close(fd); }

    if pixel_data.is_empty() {
        return Err("无法读取 framebuffer 内存（prime mmap 和 dumb mmap 均失败）".into());
    }

    Ok((width, height, pitch, pixel_data))
}

/// 将 DRM framebuffer 原始像素（XBGR8888 / bgr0）转为 RGBA 的 Vec<u8>
fn bgr0_to_rgba(pixels: &[u8], width: u32, height: u32, pitch: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let row_start = (y * pitch) as usize;
        for x in 0..width {
            let px = row_start + (x * 4) as usize;
            if px + 3 >= pixels.len() { break; }
            // bgr0 = B, G, R, X → RGBA = R, G, B, A
            let b = pixels[px];
            let g = pixels[px + 1];
            let r = pixels[px + 2];
            rgba.push(r);
            rgba.push(g);
            rgba.push(b);
            rgba.push(255);
        }
    }
    rgba
}

/// 将 RGBA 数据编码为 PNG
fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::{ImageBuffer, RgbaImage};
    let img: RgbaImage = ImageBuffer::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "ImageBuffer::from_raw failed".to_string())?;
    let mut png_data = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png_data, image::ImageFormat::Png)
        .map_err(|e| format!("PNG encode failed: {}", e))?;
    Ok(png_data.into_inner())
}

/// 裁剪 RGBA 数据
fn crop_rgba(rgba: &[u8], src_w: u32, src_h: u32, x: i32, y: i32, w: i32, h: i32) -> (Vec<u8>, u32, u32) {
    let x = x.max(0) as u32;
    let y = y.max(0) as u32;
    let w = w.max(1) as u32;
    let h = h.max(1) as u32;
    let w = w.min(src_w - x);
    let h = h.min(src_h - y);
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

/// 执行全屏截图
/// drm_fd: 合成器已有的 DRM 设备 fd（会被 dup，不影响原 fd）
/// 返回 (文件路径, PNG 二进制数据)
pub fn take_full_screenshot(drm_fd: RawFd) -> (String, Option<Vec<u8>>) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::PathBuf::from(format!("{}/Pictures/Screenshots", home));
    let _ = std::fs::create_dir_all(&dir);
    let png_path = dir.join(format!("anchor-{}.png", ts));

    match do_screenshot(drm_fd, &png_path, None) {
        Ok(png_data) => (png_path.display().to_string(), Some(png_data)),
        Err(e) => {
            tracing::warn!("Screenshot failed: {}", e);
            (String::new(), None)
        }
    }
}

/// 执行区域截图
/// drm_fd: 合成器已有的 DRM 设备 fd（会被 dup，不影响原 fd）
/// 返回 (文件路径, PNG 二进制数据)
pub fn take_area_screenshot(drm_fd: RawFd, x: i32, y: i32, w: i32, h: i32) -> (String, Option<Vec<u8>>) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::PathBuf::from(format!("{}/Pictures/Screenshots", home));
    let _ = std::fs::create_dir_all(&dir);
    let png_path = dir.join(format!("anchor-{}.png", ts));

    match do_screenshot(drm_fd, &png_path, Some((x, y, w, h))) {
        Ok(png_data) => (png_path.display().to_string(), Some(png_data)),
        Err(e) => {
            tracing::warn!("Area screenshot failed: {}", e);
            (String::new(), None)
        }
    }
}

/// 核心截图逻辑：读 DRM FB → 转 RGBA → 可选裁剪 → 编码 PNG → 保存文件 → 返回 PNG 数据
fn do_screenshot(drm_fd: RawFd, png_path: &Path, area: Option<(i32, i32, i32, i32)>) -> Result<Vec<u8>, String> {
    let (width, height, pitch, raw) = drm_read_fb(drm_fd)?;
    tracing::info!("📸 读取 framebuffer: {}x{}, pitch={}, {} bytes", width, height, pitch, raw.len());

    let rgba = bgr0_to_rgba(&raw, width, height, pitch);

    let (final_rgba, final_w, final_h) = if let Some((x, y, w, h)) = area {
        crop_rgba(&rgba, width, height, x, y, w, h)
    } else {
        (rgba, width, height)
    };

    let png_data = encode_png(&final_rgba, final_w, final_h)?;
    tracing::info!("📸 PNG 编码完成: {}x{}, {} bytes", final_w, final_h, png_data.len());

    // 保存文件
    let mut f = File::create(png_path).map_err(|e| format!("创建文件失败: {}", e))?;
    f.write_all(&png_data).map_err(|e| format!("写入文件失败: {}", e))?;
    tracing::info!("📸 截图已保存: {}", png_path.display());

    Ok(png_data)
}

/// 渲染区域选择 overlay（半透明遮罩 + 选区高亮）
pub fn render_selection_overlay(
    f: &mut impl smithay::backend::renderer::Frame,
    screen_w: i32,
    screen_h: i32,
    rect: (i32, i32, i32, i32),
) {
    let (x, y, w, h) = rect;
    let dim = crate::layout::opaque(0.0, 0.0, 0.0);

    if y > 0 {
        f.clear(dim, &[crate::layout::rect(0, 0, screen_w, y)]).ok();
    }
    if y + h < screen_h {
        f.clear(dim, &[crate::layout::rect(0, y + h, screen_w, screen_h - y - h)]).ok();
    }
    if x > 0 {
        f.clear(dim, &[crate::layout::rect(0, y, x, h)]).ok();
    }
    if x + w < screen_w {
        f.clear(dim, &[crate::layout::rect(x + w, y, screen_w - x - w, h)]).ok();
    }

    let border = crate::layout::opaque(0.48, 0.64, 0.97);
    let bw = 2;
    f.clear(border, &[crate::layout::rect(x, y, w, bw)]).ok();
    f.clear(border, &[crate::layout::rect(x, y + h - bw, w, bw)]).ok();
    f.clear(border, &[crate::layout::rect(x, y, bw, h)]).ok();
    f.clear(border, &[crate::layout::rect(x + w - bw, y, bw, h)]).ok();

    let size_text = format!("{}x{}", w, h);
    crate::text_render::draw_text(f, &size_text, x + w + 6, y - 4, 13.0, (1.0, 1.0, 1.0));
}
