// screenshot.rs — 截图功能模块
// 支持：全屏截图、区域选择截图、保存文件 + 复制到剪贴板

/// 截图状态：管理区域选择的交互
pub struct ScreenshotState {
    /// 是否正在区域选择模式
    pub selecting: bool,
    /// 选择起点（鼠标按下位置）
    pub start: Option<(f64, f64)>,
    /// 选择终点（鼠标当前位置）
    pub end: Option<(f64, f64)>,
}

impl ScreenshotState {
    pub fn new() -> Self {
        Self {
            selecting: false,
            start: None,
            end: None,
        }
    }

    /// 进入区域选择模式
    pub fn begin_selection(&mut self) {
        self.selecting = true;
        self.start = None;
        self.end = None;
    }

    /// 取消选择模式
    pub fn cancel(&mut self) {
        self.selecting = false;
        self.start = None;
        self.end = None;
    }

    /// 鼠标按下：记录起点
    pub fn on_press(&mut self, x: f64, y: f64) {
        if self.selecting {
            self.start = Some((x, y));
            self.end = Some((x, y));
        }
    }

    /// 鼠标移动：更新终点
    pub fn on_motion(&mut self, x: f64, y: f64) {
        if self.selecting && self.start.is_some() {
            self.end = Some((x, y));
        }
    }

    /// 鼠标释放：完成选择，返回选区 (x, y, w, h)
    /// 如果区域太小（<5px），视为点击取消
    pub fn on_release(&mut self) -> Option<(i32, i32, i32, i32)> {
        if let (Some((sx, sy)), Some((ex, ey))) = (self.start, self.end) {
            let x = sx.min(ex) as i32;
            let y = sy.min(ey) as i32;
            let w = (sx - ex).abs() as i32;
            let h = (sy - ey).abs() as i32;
            self.selecting = false;
            self.start = None;
            self.end = None;
            if w < 5 || h < 5 {
                None // 太小，忽略
            } else {
                Some((x, y, w, h))
            }
        } else {
            self.cancel();
            None
        }
    }

    /// 获取当前选区的矩形 (x, y, w, h)，用于渲染 overlay
    pub fn selection_rect(&self) -> Option<(i32, i32, i32, i32)> {
        if let (Some((sx, sy)), Some((ex, ey))) = (self.start, self.end) {
            let x = sx.min(ex) as i32;
            let y = sy.min(ey) as i32;
            let w = (sx - ex).abs() as i32;
            let h = (sy - ey).abs() as i32;
            if w > 0 && h > 0 {
                Some((x, y, w, h))
            } else {
                None
            }
        } else {
            None
        }
    }
}

/// 执行全屏截图：保存文件 + 复制到剪贴板
/// 返回保存的文件路径
pub fn take_full_screenshot(drm_dev: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::PathBuf::from(format!("{}/Pictures/Screenshots", home));
    let _ = std::fs::create_dir_all(&dir);

    let raw_path = dir.join(format!("anchor-{}.raw", ts));
    let png_path = dir.join(format!("anchor-{}.png", ts));

    // 找到 drm-dump-fb 工具路径
    let exe = std::env::current_exe().unwrap_or_default();
    let project_dir = exe
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    let dump_tool = format!("{}/scripts/drm-dump-fb", project_dir);

    // Step 1: dump 原始帧缓冲区
    let dump_args = format!(
        "timeout 3 {} {} {}",
        dump_tool,
        drm_dev,
        raw_path.display()
    );
    let dump_result = std::process::Command::new("sh")
        .arg("-c")
        .arg(&dump_args)
        .output();

    if let Ok(output) = dump_result {
        if !output.status.success() {
            tracing::warn!("Screenshot dump failed: {}", String::from_utf8_lossy(&output.stderr));
            return String::new();
        }
    }

    // Step 2: 用 ffmpeg 将 raw 转为 PNG
    // DRM 帧缓冲通常是 XBGR8888 格式 (32bpp)
    // 需要知道屏幕尺寸来正确转换
    let convert_args = format!(
        "ffmpeg -y -f rawvideo -pixel_format bgr0 -video_size $(cat /sys/class/drm/card*/modes 2>/dev/null | head -1 | tr 'x' ' ' | awk '{{print $1\"x\"$2}}' | head -1) -i {} -frames:v 1 -q:v 2 {} 2>/dev/null",
        raw_path.display(),
        png_path.display()
    );

    // 尝试常见的分辨率来转换
    let converted = try_convert_raw_to_png(&raw_path, &png_path);

    // 清理 raw 文件
    let _ = std::fs::remove_file(&raw_path);

    if converted {
        // 复制 PNG 到剪贴板
        copy_to_clipboard(&png_path);
        png_path.display().to_string()
    } else {
        // fallback：返回 raw 路径（不做剪贴板复制）
        tracing::warn!("Failed to convert screenshot to PNG");
        raw_path.display().to_string()
    }
}

/// 执行区域截图：dump 全屏后裁剪区域
pub fn take_area_screenshot(drm_dev: &str, x: i32, y: i32, w: i32, h: i32) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::PathBuf::from(format!("{}/Pictures/Screenshots", home));
    let _ = std::fs::create_dir_all(&dir);

    let raw_path = dir.join(format!("anchor-{}.raw", ts));
    let png_path = dir.join(format!("anchor-{}.png", ts));

    // 找到 drm-dump-fb 工具路径
    let exe = std::env::current_exe().unwrap_or_default();
    let project_dir = exe
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    let dump_tool = format!("{}/scripts/drm-dump-fb", project_dir);

    // Step 1: dump 原始帧缓冲区
    let dump_args = format!(
        "timeout 3 {} {} {}",
        dump_tool,
        drm_dev,
        raw_path.display()
    );
    let dump_result = std::process::Command::new("sh")
        .arg("-c")
        .arg(&dump_args)
        .output();

    if let Ok(output) = dump_result {
        if !output.status.success() {
            tracing::warn!("Screenshot dump failed: {}", String::from_utf8_lossy(&output.stderr));
            return String::new();
        }
    }

    // Step 2: 用 ffmpeg 裁剪并转为 PNG
    let converted = try_convert_raw_to_png_area(&raw_path, &png_path, x, y, w, h);

    // 清理 raw 文件
    let _ = std::fs::remove_file(&raw_path);

    if converted {
        copy_to_clipboard(&png_path);
        png_path.display().to_string()
    } else {
        tracing::warn!("Failed to convert area screenshot to PNG");
        raw_path.display().to_string()
    }
}

/// 尝试将 raw 帧缓冲转为 PNG
/// 自动探测分辨率（遍历常见分辨率）
fn try_convert_raw_to_png(raw_path: &std::path::Path, png_path: &std::path::Path) -> bool {
    // 读取 DRM 分辨率
    let resolutions = get_screen_resolutions();

    for (w, h) in &resolutions {
        let cmd = format!(
            "ffmpeg -y -f rawvideo -pixel_format bgr0 -video_size {}x{} -i '{}' -frames:v 1 -q:v 2 '{}' 2>/dev/null",
            w, h,
            raw_path.display(),
            png_path.display()
        );
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output();

        if let Ok(output) = result {
            if output.status.success() && png_path.exists() {
                return true;
            }
        }
    }
    false
}

/// 尝试将 raw 帧缓冲裁剪区域并转为 PNG
fn try_convert_raw_to_png_area(
    raw_path: &std::path::Path,
    png_path: &std::path::Path,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> bool {
    let resolutions = get_screen_resolutions();

    for (sw, sh) in &resolutions {
        let cmd = format!(
            "ffmpeg -y -f rawvideo -pixel_format bgr0 -video_size {}x{} -i '{}' -vf 'crop={}:{}:{},{}' -frames:v 1 -q:v 2 '{}' 2>/dev/null",
            sw, sh,
            raw_path.display(),
            w, h, x, y,
            png_path.display()
        );
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output();

        if let Ok(output) = result {
            if output.status.success() && png_path.exists() {
                return true;
            }
        }
    }
    false
}

/// 获取屏幕分辨率候选列表
fn get_screen_resolutions() -> Vec<(u32, u32)> {
    let mut res = Vec::new();

    // 从 DRM sysfs 读取当前 mode
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let modes_path = entry.path().join("modes");
            if modes_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&modes_path) {
                    for line in content.lines() {
                        let parts: Vec<&str> = line.trim().split('x').collect();
                        if parts.len() == 2 {
                            if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                                res.push((w, h));
                            }
                        }
                    }
                }
            }
        }
    }

    // 常见分辨率 fallback
    if res.is_empty() {
        res.push((1920, 1080));
        res.push((2560, 1440));
        res.push((3840, 2160));
        res.push((1366, 768));
        res.push((1280, 720));
    }

    res
}

/// 复制图片文件到 Wayland 剪贴板
fn copy_to_clipboard(png_path: &std::path::Path) {
    let result = std::process::Command::new("wl-copy")
        .arg("-t")
        .arg("image/png")
        .arg("--")
        .arg(png_path)
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                tracing::info!("📋 Screenshot copied to clipboard");
            } else {
                tracing::warn!(
                    "wl-copy failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        Err(e) => {
            tracing::warn!("Failed to run wl-copy: {}", e);
        }
    }
}

/// 渲染区域选择 overlay（半透明遮罩 + 选区高亮）
pub fn render_selection_overlay(
    f: &mut impl smithay::backend::renderer::Frame,
    screen_w: i32,
    screen_h: i32,
    rect: (i32, i32, i32, i32),
) {
    let (x, y, w, h) = rect;

    // 半透明遮罩（选区外变暗）
    let dim = crate::layout::opaque(0.0, 0.0, 0.0);
    let alpha = 0.4;

    // 上方遮罩
    if y > 0 {
        f.clear(dim, &[crate::layout::rect(0, 0, screen_w, y)]).ok();
    }
    // 下方遮罩
    if y + h < screen_h {
        f.clear(dim, &[crate::layout::rect(0, y + h, screen_w, screen_h - y - h)]).ok();
    }
    // 左侧遮罩
    if x > 0 {
        f.clear(dim, &[crate::layout::rect(0, y, x, h)]).ok();
    }
    // 右侧遮罩
    if x + w < screen_w {
        f.clear(dim, &[crate::layout::rect(x + w, y, screen_w - x - w, h)]).ok();
    }

    // 选区边框（蓝色）
    let border = crate::layout::opaque(0.48, 0.64, 0.97); // #7aa2f7
    let bw = 2;
    // 顶部边框
    f.clear(border, &[crate::layout::rect(x, y, w, bw)]).ok();
    // 底部边框
    f.clear(border, &[crate::layout::rect(x, y + h - bw, w, bw)]).ok();
    // 左侧边框
    f.clear(border, &[crate::layout::rect(x, y, bw, h)]).ok();
    // 右侧边框
    f.clear(border, &[crate::layout::rect(x + w - bw, y, bw, h)]).ok();

    // 显示尺寸信息
    let size_text = format!("{}x{}", w, h);
    crate::text_render::draw_text(
        f,
        &size_text,
        x + w + 6,
        y - 4,
        13.0,
        (1.0, 1.0, 1.0),
    );
}
