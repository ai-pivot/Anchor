//! TOML 配置文件解析
//! 路径: ~/.config/anchor/config.toml 或 ./config.toml

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub colors: Colors,
    #[serde(default)]
    pub bar: Bar,
    #[serde(default)]
    pub wallpaper: Wallpaper,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub keybindings: Keybindings,
    #[serde(default)]
    pub terminal: Terminal,
    #[serde(default)]
    pub launcher: Launcher,
    #[serde(default)]
    pub window_rules: Vec<WindowRule>,
    #[serde(default)]
    pub gpu: Gpu,
    #[serde(default)]
    pub cursor: Cursor,
    #[serde(default)]
    pub outputs: Vec<OutputConfig>,
    #[serde(default)]
    pub scroll: ScrollConfig,
    #[serde(default)]
    pub idle: IdleConfig,
}

/// 窗口规则：根据 app-id 或 title 自动分配工作区/布局
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowRule {
    /// 应用 ID 匹配（子串）
    #[serde(default)]
    pub app_id: String,
    /// 标题匹配（子串）
    #[serde(default)]
    pub title: String,
    /// 分配到工作区 (0-indexed)
    #[serde(default)]
    pub workspace: usize,
    /// 使用的布局
    #[serde(default)]
    pub layout: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Colors {
    #[serde(default = "Colors::default_bg")]
    pub background: String,
    #[serde(default = "Colors::default_focus_border")]
    pub focus_border: String,
    #[serde(default = "Colors::default_unfocus_border")]
    pub unfocus_border: String,
    #[serde(default = "Colors::default_bar_bg")]
    pub bar_background: String,
    #[serde(default = "Colors::default_bar_fg")]
    pub bar_foreground: String,
    #[serde(default = "Colors::default_bar_ws_active")]
    pub bar_workspace_active: String,
    #[serde(default = "Colors::default_bar_ws_inactive")]
    pub bar_workspace_inactive: String,
    #[serde(default = "Colors::default_bar_status")]
    pub bar_status: String,
    #[serde(default = "Colors::default_bar_urgent")]
    pub bar_urgent: String,
    #[serde(default = "Colors::default_bar_sep")]
    pub bar_separator: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bar {
    #[serde(default = "Bar::default_enabled")]
    pub enabled: bool,
    #[serde(default = "Bar::default_height")]
    pub height: i32,
    #[serde(default = "Bar::default_opacity")]
    pub opacity: f32,
    #[serde(default = "Bar::default_sep_width")]
    pub separator_width: i32,
    #[serde(default = "Bar::default_ws_spacing")]
    pub workspace_spacing: i32,
    #[serde(default = "Bar::default_padding_left")]
    pub padding_left: i32,
    #[serde(default = "Bar::default_padding_right")]
    pub padding_right: i32,
    #[serde(default = "Bar::default_gradient_top")]
    pub gradient_top: String,
    #[serde(default = "Bar::default_gradient_bottom")]
    pub gradient_bottom: String,
    #[serde(default = "Bar::default_show_date")]
    pub show_date: bool,
    #[serde(default = "Bar::default_show_cpu")]
    pub show_cpu: bool,
    #[serde(default = "Bar::default_show_memory")]
    pub show_memory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wallpaper {
    /// "color" | "image" | "random" | "gradient"
    #[serde(default = "Wallpaper::default_mode")]
    pub mode: String,
    #[serde(default = "Wallpaper::default_color")]
    pub color: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub directory: String,
    #[serde(default = "Wallpaper::default_gradient_top")]
    pub gradient_top: String,
    #[serde(default = "Wallpaper::default_gradient_bottom")]
    pub gradient_bottom: String,
    /// 切换间隔秒数，0=不切换
    #[serde(default)]
    pub change_interval: u64,
    /// "fill" | "fit" | "stretch" | "center"
    #[serde(default = "Wallpaper::default_scaling")]
    pub scaling: String,
    /// NVIDIA block-linear 参数: 每个 block 的 GOB 高度 (0=禁用转换, 16=典型值)
    #[serde(default = "Wallpaper::default_block_height_gobs")]
    pub block_height_gobs: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default = "Layout::default_border_width")]
    pub border_width: i32,
    #[serde(default = "Layout::default_gap")]
    pub gap: i32,
    #[serde(default = "Layout::default_margin")]
    pub margin: i32,
    /// Header bar 高度（逻辑像素）。
    /// 0 = 不预留 header bar 空间（默认）。
    /// >0 = 在每个窗口顶部预留该高度的区域，客户端可在其中渲染自定义标题栏。
    /// 客户端可通过 anchor-header-bar-v1 协议覆盖此值。
    #[serde(default = "Layout::default_header_bar_height")]
    pub header_bar_height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keybindings {
    #[serde(default = "Keybindings::default_bindings")]
    pub bindings: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Terminal {
    #[serde(default = "Terminal::default_command")]
    pub command: String,
    #[serde(default)]
    pub font: String,
    #[serde(default = "Terminal::default_font_size")]
    pub font_size: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Launcher {
    #[serde(default = "Launcher::default_prompt")]
    pub prompt: String,
    #[serde(default = "Launcher::default_lines")]
    pub lines: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gpu {
    /// 优先使用的 GPU vendor: "auto" | "nvidia" | "amd" | "intel"
    #[serde(default = "Gpu::default_vendor")]
    pub vendor: String,
    /// 手动指定 DRM 设备路径，空字符串=自动检测
    #[serde(default)]
    pub device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    /// X11 cursor theme 名称，空字符串=使用内置光标
    #[serde(default)]
    pub theme: String,
    /// 光标名称 (默认 "left_ptr")
    #[serde(default = "Cursor::default_name")]
    pub name: String,
    /// 光标像素大小 (每个逻辑像素的实际像素数)
    #[serde(default = "Cursor::default_size")]
    pub size: usize,
}

/// 显示器配置：控制输出排列和默认工作区
/// config.toml 中用 [[output]] 数组配置，按定义顺序从左到右排列
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Connector 名称匹配（子串），如 "HDMI-A-1"、"DP-2"
    /// 空字符串=匹配任意未配置的输出（fallback）
    #[serde(default)]
    pub connector: String,
    /// 屏幕位置 x 坐标（像素），用于手动控制排列
    /// 默认自动从左到右排列
    #[serde(default)]
    pub x: i32,
    /// 屏幕位置 y 坐标（像素）
    #[serde(default)]
    pub y: i32,
    /// 默认绑定的工作区索引 (0-8)
    #[serde(default)]
    pub workspace: usize,
    /// 缩放因子（未来扩展，目前固定 1.0）
    #[serde(default = "OutputConfig::default_scale")]
    pub scale: f64,
}

impl OutputConfig {
    fn default_scale() -> f64 {
        1.0
    }
}

/// 无限滚动工作区配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollConfig {
    /// 是否启用无限滚动（弹簧吸附 + 惯性 + 触摸板手势）
    #[serde(default = "ScrollConfig::default_enabled")]
    pub enabled: bool,
    /// 弹簧刚度（k）— 越大吸附越快。默认 300
    #[serde(default = "ScrollConfig::default_stiffness")]
    pub spring_stiffness: f64,
    /// 弹簧阻尼（c）— 越大振荡越少。默认 30
    #[serde(default = "ScrollConfig::default_damping")]
    pub spring_damping: f64,
    /// 惯性摩擦系数（per-frame @60fps）— 0.92 = 保留92%速度每帧
    #[serde(default = "ScrollConfig::default_friction")]
    pub friction: f64,
    /// 触摸板滑动触发阈值（屏幕宽度的比例）
    #[serde(default = "ScrollConfig::default_swipe_threshold")]
    pub swipe_threshold: f64,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            spring_stiffness: 300.0,
            spring_damping: 30.0,
            friction: 0.92,
            swipe_threshold: 0.15,
        }
    }
}

impl ScrollConfig {
    fn default_enabled() -> bool {
        true
    }
    fn default_stiffness() -> f64 {
        300.0
    }
    fn default_damping() -> f64 {
        30.0
    }
    fn default_friction() -> f64 {
        0.92
    }
    fn default_swipe_threshold() -> f64 {
        0.15
    }
}

/// 空闲与电源管理配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdleConfig {
    /// 空闲超时（秒），0=禁用。超时后屏幕 DPMS off
    #[serde(default = "IdleConfig::default_timeout")]
    pub timeout: u64,
    /// 空闲超时后是否自动锁屏
    #[serde(default = "IdleConfig::default_lock_on_idle")]
    pub lock_on_idle: bool,
    /// DPMS 关闭延迟（秒，在 idle timeout 之后）。
    /// 0=与 timeout 同时关闭。设为更大值则先锁屏再息屏。
    #[serde(default = "IdleConfig::default_dpms_delay")]
    pub dpms_delay: u64,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            timeout: IdleConfig::default_timeout(),
            lock_on_idle: IdleConfig::default_lock_on_idle(),
            dpms_delay: IdleConfig::default_dpms_delay(),
        }
    }
}

impl IdleConfig {
    fn default_timeout() -> u64 {
        300 // 5 分钟
    }
    fn default_lock_on_idle() -> bool {
        true
    }
    fn default_dpms_delay() -> u64 {
        0 // 与 timeout 同时
    }
}

pub fn parse_color(hex: &str) -> (f32, f32, f32) {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return (0.0, 0.0, 0.0);
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f32 / 255.0;
    (r, g, b)
}

pub fn parse_color_alpha(hex: &str, alpha: f32) -> (f32, f32, f32, f32) {
    let (r, g, b) = parse_color(hex);
    (r, g, b, alpha)
}

impl Config {
    pub fn load() -> Self {
        let paths = [
            dirs().join("config.toml"),
            std::path::PathBuf::from("config.toml"),
        ];
        for p in &paths {
            if p.exists() {
                match std::fs::read_to_string(p) {
                    Ok(s) => match toml::from_str(&s) {
                        Ok(c) => {
                            tracing::info!("📋 配置: {}", p.display());
                            return c;
                        }
                        Err(e) => tracing::warn!("⚠️  配置解析错误 {}: {}", p.display(), e),
                    },
                    Err(e) => tracing::warn!("⚠️  配置读取错误 {}: {}", p.display(), e),
                }
            }
        }
        tracing::info!("📋 使用默认配置");
        Self::default()
    }

    /// 获取壁纸目录中的所有图片文件
    pub fn wallpaper_files(&self) -> Vec<std::path::PathBuf> {
        if self.wallpaper.directory.is_empty() {
            return vec![];
        }
        let exts = ["png", "jpg", "jpeg", "bmp", "webp"];
        let mut files = vec![];
        if let Ok(entries) = std::fs::read_dir(&self.wallpaper.directory) {
            for e in entries.flatten() {
                let p = e.path();
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if exts.contains(&ext.to_lowercase().as_str()) {
                        files.push(p);
                    }
                }
            }
        }
        files.sort();
        files
    }
}

fn dirs() -> std::path::PathBuf {
    let uid = unsafe { libc::getuid() };
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let base = xdg.unwrap_or_else(|| format!("{}/.config", home));
    Path::new(&base).join("anchor").to_path_buf()
}

/// 返回配置文件路径 (~/.config/anchor/config.toml)
pub fn config_path() -> std::path::PathBuf {
    dirs().join("config.toml")
}

// ── Defaults ────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            colors: Colors::default(),
            bar: Bar::default(),
            wallpaper: Wallpaper::default(),
            layout: Layout::default(),
            keybindings: Keybindings::default(),
            terminal: Terminal::default(),
            launcher: Launcher::default(),
            window_rules: Vec::new(),
            gpu: Gpu::default(),
            cursor: Cursor::default(),
            scroll: ScrollConfig::default(),
            idle: IdleConfig::default(),
            outputs: Vec::new(),
        }
    }
}
impl Default for Colors {
    fn default() -> Self {
        Self {
            background: Colors::default_bg(),
            focus_border: Colors::default_focus_border(),
            unfocus_border: Colors::default_unfocus_border(),
            bar_background: Colors::default_bar_bg(),
            bar_foreground: Colors::default_bar_fg(),
            bar_workspace_active: Colors::default_bar_ws_active(),
            bar_workspace_inactive: Colors::default_bar_ws_inactive(),
            bar_status: Colors::default_bar_status(),
            bar_urgent: Colors::default_bar_urgent(),
            bar_separator: Colors::default_bar_sep(),
        }
    }
}
impl Default for Bar {
    fn default() -> Self {
        Self {
            enabled: true,
            height: 36,
            opacity: 0.92,
            separator_width: 1,
            workspace_spacing: 6,
            padding_left: 16,
            padding_right: 16,
            gradient_top: Bar::default_gradient_top(),
            gradient_bottom: Bar::default_gradient_bottom(),
            show_date: true,
            show_cpu: true,
            show_memory: true,
        }
    }
}
impl Default for Wallpaper {
    fn default() -> Self {
        Self {
            mode: Wallpaper::default_mode(),
            color: Wallpaper::default_color(),
            path: String::new(),
            directory: String::new(),
            gradient_top: Wallpaper::default_gradient_top(),
            gradient_bottom: Wallpaper::default_gradient_bottom(),
            change_interval: 0,
            scaling: Wallpaper::default_scaling(),
            block_height_gobs: Wallpaper::default_block_height_gobs(),
        }
    }
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            border_width: 2,
            gap: 6,
            margin: 0,
            header_bar_height: 0,
        }
    }
}
impl Default for Keybindings {
    fn default() -> Self {
        Self {
            bindings: Keybindings::default_bindings(),
        }
    }
}
impl Default for Terminal {
    fn default() -> Self {
        Self {
            command: "foot".into(),
            font: "monospace".into(),
            font_size: 12,
        }
    }
}
impl Default for Launcher {
    fn default() -> Self {
        Self {
            prompt: "Launch".into(),
            lines: 10,
        }
    }
}
impl Default for Gpu {
    fn default() -> Self {
        Self {
            vendor: Gpu::default_vendor(),
            device: String::new(),
        }
    }
}
impl Default for Cursor {
    fn default() -> Self {
        Self {
            theme: String::new(),
            name: Cursor::default_name(),
            size: Cursor::default_size(),
        }
    }
}

impl Colors {
    fn default_bg() -> String {
        "#0f0f1a".into()
    }
    fn default_focus_border() -> String {
        "#7aa2f7".into()
    }
    fn default_unfocus_border() -> String {
        "#3b3d57".into()
    }
    fn default_bar_bg() -> String {
        "#0d0d16".into()
    }
    fn default_bar_fg() -> String {
        "#c0caf5".into()
    }
    fn default_bar_ws_active() -> String {
        "#7aa2f7".into()
    }
    fn default_bar_ws_inactive() -> String {
        "#3b3d57".into()
    }
    fn default_bar_status() -> String {
        "#9ece6a".into()
    }
    fn default_bar_urgent() -> String {
        "#f7768e".into()
    }
    fn default_bar_sep() -> String {
        "#414868".into()
    }
}
impl Bar {
    fn default_gradient_top() -> String {
        "#16161e".into()
    }
    fn default_gradient_bottom() -> String {
        "#0d0d16".into()
    }
}
impl Wallpaper {
    fn default_mode() -> String {
        "gradient".into()
    }
    fn default_color() -> String {
        "#0f0f1a".into()
    }
    fn default_gradient_top() -> String {
        "#1a1a3e".into()
    }
    fn default_gradient_bottom() -> String {
        "#0f0f1a".into()
    }
    fn default_scaling() -> String {
        "fill".into()
    }
    fn default_block_height_gobs() -> usize {
        16
    }
}
impl Keybindings {
    fn default_bindings() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("super+return".into(), "terminal".into());
        m.insert("super+q".into(), "close".into());
        m.insert("super+d".into(), "launcher".into());
        m.insert("super+f".into(), "fullscreen".into());
        m.insert("super+shift+escape".into(), "quit".into());
        m.insert("super+w".into(), "wallpaper_next".into());
        m
    }
}

// ── Serde default functions (required by #[serde(default = "...")]) ──
impl Bar {
    fn default_enabled() -> bool {
        true
    }
    fn default_height() -> i32 {
        36
    }
    fn default_opacity() -> f32 {
        0.92
    }
    fn default_sep_width() -> i32 {
        1
    }
    fn default_ws_spacing() -> i32 {
        6
    }
    fn default_padding_left() -> i32 {
        16
    }
    fn default_padding_right() -> i32 {
        16
    }
    fn default_show_date() -> bool {
        true
    }
    fn default_show_cpu() -> bool {
        true
    }
    fn default_show_memory() -> bool {
        true
    }
}
impl Layout {
    fn default_border_width() -> i32 {
        2
    }
    fn default_gap() -> i32 {
        6
    }
    fn default_margin() -> i32 {
        0
    }
    fn default_header_bar_height() -> i32 {
        0
    }
}
impl Terminal {
    fn default_command() -> String {
        "foot".into()
    }
    fn default_font_size() -> i32 {
        12
    }
}
impl Launcher {
    fn default_prompt() -> String {
        "Launch".into()
    }
    fn default_lines() -> i32 {
        10
    }
}
impl Gpu {
    fn default_vendor() -> String {
        "auto".into()
    }
}
impl Cursor {
    fn default_name() -> String {
        "left_ptr".into()
    }
    fn default_size() -> usize {
        3
    }
}
