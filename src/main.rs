// Anchor — Wayland tiling compositor v9
// Features: multi-workspace, multi-monitor, wallpaper, config
// Config: ~/.config/anchor/config.toml

mod config;
mod layout;
use layout::LayoutPreset;
mod appctl;
mod auth;
mod block_linear;
mod cursor;
mod lock;
mod notify;
mod record;
mod screenshot;
mod text_render;
mod wallpaper;
mod workspace;
mod xwayland;
use lock::LockState;
mod launcher;
use launcher::LauncherState;
mod scratchpad;
use scratchpad::ScratchpadState;
mod physics;
use physics::{Momentum, Spring};
use workspace::{WindowSlot, Workspace, NUM_WORKSPACES};
mod overview;
use overview::OverviewState;
mod headerbar;
mod ipc;
mod settings;
use headerbar::{
    ensure_header_bar_data, get_header_bar_info, set_client_decoration, set_header_bar_height,
    HeaderBarData,
};
use settings::SettingsState;

/// 预分配的工作区标签，避免渲染热路径中的 format! 分配
const WS_LABELS: [&str; 9] = [
    "WS 1", "WS 2", "WS 3", "WS 4", "WS 5", "WS 6", "WS 7", "WS 8", "WS 9",
];

use std::{
    os::fd::AsRawFd,
    os::unix::io::OwnedFd,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use config::Config;
use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Format, Fourcc, Modifier,
        },
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, GbmBufferedSurface},
        input::{Axis, ButtonState, InputEvent, KeyState, PointerAxisEvent},
        libinput::LibinputInputBackend,
        renderer::{
            element::{
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Element, Kind, RenderElement,
            },
            gles::GlesRenderer,
            utils::{draw_render_elements, on_commit_buffer_handler},
            Bind, Color32F, ExportMem, Frame, ImportDma, ImportMem, Renderer,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
    },
    delegate_compositor, delegate_data_device, delegate_fractional_scale, delegate_idle_inhibit,
    delegate_idle_notify, delegate_input_method_manager, delegate_layer_shell, delegate_output,
    delegate_pointer_constraints, delegate_primary_selection, delegate_relative_pointer,
    delegate_seat, delegate_shm, delegate_text_input_manager, delegate_viewporter,
    delegate_virtual_keyboard_manager, delegate_xdg_activation, delegate_xdg_decoration,
    delegate_xdg_shell,
    desktop::{PopupKind, PopupManager},
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState, XkbConfig},
        pointer::{
            AxisFrame, CursorIcon, CursorImageStatus, MotionEvent, PointerHandle,
            RelativeMotionEvent,
        },
        Seat, SeatHandler, SeatState,
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::{EventLoop, LoopHandle},
        drm::control::{connector, crtc, Device as _},
        wayland_server::{
            protocol::{wl_seat, wl_surface::WlSurface},
            Display, DisplayHandle,
        },
    },
    utils::{
        DeviceFd, IsAlive, Logical, Physical, Point, Rectangle, Size, Transform, SERIAL_COUNTER,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_surface_tree_downward, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction,
        },
        fractional_scale::{FractionalScaleHandler, FractionalScaleManagerState},
        idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState},
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        input_method::{
            InputMethodHandler, InputMethodManagerState, PopupSurface as ImPopupSurface,
        },
        output::OutputManagerState,
        pointer_constraints::{
            with_pointer_constraint, PointerConstraint, PointerConstraintsHandler,
            PointerConstraintsState,
        },
        relative_pointer::RelativePointerManagerState,
        selection::{
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
            SelectionHandler,
        },
        shell::{
            wlr_layer::{
                Anchor, Layer, LayerSurface, LayerSurfaceCachedState, LayerSurfaceData,
                WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::{
                decoration::{XdgDecorationHandler, XdgDecorationState},
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            },
        },
        shm::{ShmHandler, ShmState},
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        virtual_keyboard::VirtualKeyboardManagerState,
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};
use tracing::{error, info, warn};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::wl_buffer,
    protocol::wl_data_source::WlDataSource,
    Client, ListeningSocket, Resource,
};

// ── Workspace (see workspace.rs) ─────────────────────────────

// ── AnchorOutput ────────────────────────────────────────────

struct AnchorOutput {
    output: Output,
    size: Size<i32, Logical>,
    crtc: crtc::Handle,
    connector: connector::Handle,
    buf_surf: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    pending_flip: bool,
    position: (i32, i32),
    /// 此 output 上当前活跃的工作区索引
    active_ws: usize,
    /// Connector 名称（用于配置匹配）
    name: String,
    /// 当前缩放因子
    scale: f64,
    /// DPMS 是否关闭
    dpms_off: bool,
}

// ── App ──────────────────────────────────────────────────

struct Notification {
    text: String,
    created: std::time::Instant,
    duration: std::time::Duration,
}

pub(crate) struct App {
    comp: CompositorState,
    xdg: XdgShellState,
    shm: ShmState,
    seat_state: SeatState<Self>,
    dd: DataDeviceState,
    primary_sel: PrimarySelectionState,
    seat: Seat<Self>,
    deco: XdgDecorationState,
    xdg_activation: XdgActivationState,
    popup_manager: PopupManager,
    osize: Size<i32, Logical>,
    workspaces: Vec<Workspace>,
    active_ws: usize,
    run: bool,
    frame: u32,
    dh: DisplayHandle,
    active: bool,
    dirty: bool,
    kbd: smithay::input::keyboard::KeyboardHandle<Self>,
    pointer: PointerHandle<Self>,
    pointer_pos: (f64, f64),
    /// 上一次指针焦点 surface（用于检测焦点切换并重置光标）
    pointer_focus_surface: Option<WlSurface>,
    cfg: Config,
    cursor_img: cursor::CursorImage,
    /// 当前光标状态：Named(使用命名光标) / Surface(客户端提供的表面) / Hidden(隐藏)
    cursor_status: CursorImageStatus,
    /// 命名光标的缓存（CursorIcon.name() → CursorImage）
    cursor_cache: std::collections::HashMap<String, cursor::CursorImage>,
    window_titles: std::collections::HashMap<usize, String>,
    window_app_ids: std::collections::HashMap<usize, String>,
    vblank_crtcs: std::collections::HashSet<crtc::Handle>,
    wallpaper_cache: wallpaper::WallpaperCache,
    /// Cached GPU texture for image wallpaper
    wallpaper_texture: Option<smithay::backend::renderer::gles::GlesTexture>,
    notifications: Vec<Notification>,
    // Scratchpad (dropdown terminal)
    scratchpad: ScratchpadState,
    // IM popup (fcitx5 candidate box)
    im_popup: Option<ImPopupSurface>,
    /// Pending toplevels — not yet confirmed by app_id.
    /// Chromium tooltip windows have empty app_id and live ~15ms.
    /// Real windows always set a non-empty app_id via app_id_changed.
    pending_tops: Vec<ToplevelSurface>,
    dbus_notifications: Arc<Mutex<notify::NotificationState>>,
    // 内置启动器
    launcher: LauncherState,
    // 工作区切换动画
    ws_anim: WsAnimation,
    // 窗口布局动画（macOS 风格）
    layout_anim: LayoutAnimation,
    /// 上一次 layout_workspace 的 slot 位置（动画起点，在每次 layout 后更新）
    prev_positions: Vec<(crate::workspace::WindowSlot, (i32, i32))>,
    // 屏幕录制
    record_state: record::RecordState,
    // 多显示器尺寸信息（用于鼠标穿越）
    output_sizes: Vec<(i32, i32, i32, i32)>, // (x, y, w, h) per output
    /// 每个 output 当前活跃的工作区索引（独立切换）
    output_active_ws: Vec<usize>,
    /// 鼠标/键盘焦点所在的 output 索引
    focused_output: usize,
    // XWayland (X11 app support)
    xw: xwayland::XWaylandState,
    /// XWayland display number (e.g. 1 for :1), set when XWayland becomes ready
    xdisplay: Option<u32>,
    // 截图状态（区域选择）
    screenshot: screenshot::ScreenshotState,
    /// 待处理的截图请求（在渲染流程中执行）
    pending_screenshot: Option<screenshot::ScreenshotRequest>,
    /// 截图完成的缓存结果（渲染后处理）
    screenshot_result: Option<(String, Option<Vec<u8>>)>,
    /// EventLoop handle（用于 XWM selection 转发等需要注册临时 source 的场景）
    loop_handle: Option<smithay::reexports::calloop::LoopHandle<'static, App>>,
    // 锁屏状态
    lock_state: LockState,
    // X11 辅助窗口（候选框等）出现前保存的原应用焦点
    x11_saved_focus: Option<WlSurface>,
    // CPU/MEM 统计（headbar 显示用）
    cpu_usage: f32, // 0.0 ~ 1.0
    mem_usage: f32, // 0.0 ~ 1.0
    cpu_prev_idle: u64,
    cpu_prev_total: u64,
    // ── 无限滚动工作区（弹簧物理驱动）──
    /// 连续滚动偏移量（0.0 = ws0 居中，1.0 = ws1 居中）
    scroll_offset: f64,
    /// 每个显示器的独立滚动偏移（多显示器各自独立）
    scroll_offsets: Vec<f64>,
    /// 吸附弹簧
    scroll_spring: Spring,
    /// 惯性滚动
    scroll_momentum: Momentum,
    // ── 触摸板手势状态 ──
    gesture_active: bool,
    gesture_dx: f64,
    gesture_dy: f64,
    gesture_fingers: u32,
    /// 上一次渲染帧的时间戳（帧率无关的物理计算）
    last_frame_time: std::time::Instant,
    // ── Overview 状态机（任务面板 + 鸟瞰视图）──
    overview: OverviewState,
    // ── Settings Panel（可视化配置界面）──
    settings: SettingsState,
    /// 窗口打开/关闭动画（ws_idx, start_time, is_open）
    window_anims: Vec<(usize, std::time::Instant, bool)>,
    /// Mission Control 缩略图点击区域 (tx, ty, tw, th, ws_idx, slot)
    expose_thumbs: Vec<(i32, i32, i32, i32, usize, crate::workspace::WindowSlot)>,
    /// 预解析的颜色值（避免每帧 parse_color）
    cached_focus_color: (f32, f32, f32),
    cached_unfocus_color: (f32, f32, f32),
    /// libseat session（用于 TTY 切换）
    session: Option<Arc<std::sync::Mutex<LibSeatSession>>>,
    /// VT 文件描述符（用于 VT mode 设置和切换）
    vt_fd: Option<std::os::unix::io::OwnedFd>,
    // ── Layer Shell (wlr_layer_shell_v1) ──
    layer_shell: WlrLayerShellState,
    // ── Fractional Scale + Viewporter ──
    fractional_scale_mgr: FractionalScaleManagerState,
    viewporter: ViewporterState,
    // ── 空闲 / 电源管理 ──
    idle_notifier: IdleNotifierState<App>,
    idle_inhibit_mgr: IdleInhibitManagerState,
    /// 空闲计时器（calloop timer token）
    idle_timer: Option<smithay::reexports::calloop::RegistrationToken>,
    /// 是否处于空闲状态（用于 DPMS off）
    idle_active: bool,
    /// idle inhibit 计数（>0=有应用阻止空闲）
    idle_inhibit_count: usize,
    // ── DnD 拖放 ──
    /// 当前 DnD 拖拽图标 surface（渲染在光标位置）
    dnd_icon: Option<WlSurface>,
    /// 上一次输入时间（用于空闲检测）
    last_input_time: std::time::Instant,
    ipc: Option<ipc::IpcServer>,
}

/// 工作区切换动画状态
struct WsAnimation {
    /// 动画开始时间
    start: Option<std::time::Instant>,
    /// 旧工作区索引
    from_ws: usize,
    /// 新工作区索引
    to_ws: usize,
    /// 动画时长（ms）
    duration_ms: u64,
    /// 方向: -1=左, 1=右
    direction: i32,
}

/// 窗口布局动画状态（macOS 风格：窗口从旧位置滑到新位置）
struct LayoutAnimation {
    start: Option<std::time::Instant>,
    duration_ms: u64,
    /// 每个窗口身份 -> 旧位置
    old_positions: Vec<(crate::workspace::WindowSlot, (i32, i32))>,
}

impl LayoutAnimation {
    fn new() -> Self {
        Self {
            start: None,
            duration_ms: 350,
            old_positions: Vec::new(),
        }
    }

    fn begin(&mut self, positions: &[(crate::workspace::WindowSlot, (i32, i32))]) {
        self.old_positions = positions.to_vec();
        self.start = Some(std::time::Instant::now());
    }

    fn offset_for(
        &self,
        slot: &crate::workspace::WindowSlot,
        target: (i32, i32),
    ) -> Option<(i32, i32)> {
        let start = self.start?;
        let elapsed = start.elapsed().as_millis() as u64;
        if elapsed >= self.duration_ms {
            return None;
        }
        let old = self
            .old_positions
            .iter()
            .find(|(s, _)| match (s, slot) {
                (crate::workspace::WindowSlot::Wl(a), crate::workspace::WindowSlot::Wl(b)) => {
                    a == b
                }
                (crate::workspace::WindowSlot::X11(a), crate::workspace::WindowSlot::X11(b)) => {
                    a == b
                }
                _ => false,
            })?
            .1;
        let t = elapsed as f32 / self.duration_ms as f32;
        let t_ease = 1.0 - (1.0 - t).powi(3);
        let dx = (old.0 - target.0) as f32 * (1.0 - t_ease);
        let dy = (old.1 - target.1) as f32 * (1.0 - t_ease);
        Some((dx as i32, dy as i32))
    }

    fn is_active(&self) -> bool {
        match self.start {
            Some(s) => {
                let ms: u64 = s.elapsed().as_millis() as u64;
                ms < self.duration_ms
            }
            None => false,
        }
    }
}

impl BufferHandler for App {
    fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {}
}

// ── VT 切换常量（libc 中未定义）──
const VT_ACTIVATE: u64 = 0x5606;
const VT_WAITACTIVE: u64 = 0x5607;
const VT_SETMODE: u64 = 0x5602;
const VT_PROCESS: libc::c_char = 1;
const VT_AUTO: libc::c_char = 0;

#[repr(C)]
struct VtMode {
    mode: libc::c_char,
    waitv: libc::c_char,
    relsig: libc::c_short,
    acqsig: libc::c_short,
    frsig: libc::c_short,
}

impl App {
    fn current_gpu_vendor(&self) -> String {
        let path = std::env::var("TITAN_DRM_DEV").ok();
        let card_name = path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let vendor = std::fs::read_to_string(format!("/sys/class/drm/{}/device/vendor", card_name))
            .unwrap_or_default();
        match vendor.trim() {
            "0x10de" => "NVIDIA",
            "0x1002" => "AMD",
            "0x8086" => "Intel",
            _ => "Unknown",
        }
        .to_string()
    }

    fn title_for_surface(&self, surf: &WlSurface) -> String {
        use smithay::wayland::compositor::with_states;
        use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
        with_states(surf, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|d| d.title.clone())
                .unwrap_or_default()
        })
    }

    fn app_id_for_surface(&self, surf: &WlSurface) -> String {
        use smithay::wayland::compositor::with_states;
        use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
        with_states(surf, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|d| d.app_id.clone())
                .unwrap_or_default()
        })
    }

    fn close_focused_window(&mut self) -> Result<(), String> {
        if let Some(window) = crate::appctl::query_focused_window(self) {
            crate::appctl::emit_event(
                self,
                crate::appctl::DesktopEvent::WindowClosed {
                    workspace: window.workspace,
                    title: window.title.clone(),
                    app_id: window.app_id.clone(),
                    kind: window.kind.clone(),
                },
            );
        }
        if let Some(ref surf) = self.workspaces[self.active_ws].focus.clone() {
            let ws = &self.workspaces[self.active_ws];
            if let Some(tl) = ws.tops.iter().find(|tl| tl.wl_surface() == surf) {
                tl.send_close();
                return Ok(());
            }
            if let Some(xs) = ws
                .x11_surfaces
                .iter()
                .find(|xs| xs.wl_surface().as_ref() == Some(surf))
            {
                let _ = xs.close();
                return Ok(());
            }
        }
        Err("no focused window".into())
    }
    /// 当前工作区的窗口列表
    fn tops(&self) -> &Vec<ToplevelSurface> {
        &self.workspaces[self.active_ws].tops
    }
    fn tops_mut(&mut self) -> &mut Vec<ToplevelSurface> {
        &mut self.workspaces[self.active_ws].tops
    }

    fn focus_idx(&self) -> Option<usize> {
        let ws = &self.workspaces[self.active_ws];
        let focus = ws.focus.as_ref()?;
        let order = ws.effective_order();
        for (i, slot) in order.iter().enumerate() {
            let matches = match slot {
                WindowSlot::Wl(idx) => ws.tops.get(*idx).map(|tl| tl.wl_surface() == focus),
                WindowSlot::X11(idx) => ws
                    .x11_surfaces
                    .get(*idx)
                    .and_then(|xs| xs.wl_surface().map(|wl| &wl == focus)),
            };
            if matches == Some(true) {
                return Some(i);
            }
        }
        None
    }

    /// 根据 pointer_pos 全局坐标判断鼠标在哪个 output 上，返回 output 索引
    fn output_at_pointer(&self) -> usize {
        let px = self.pointer_pos.0 as i32;
        let py = self.pointer_pos.1 as i32;
        for (i, (ox, oy, ow, oh)) in self.output_sizes.iter().enumerate() {
            if px >= *ox && px < ox + ow && py >= *oy && py < oy + oh {
                return i;
            }
        }
        0 // fallback 到主输出
    }

    fn clamp_rect_to_bounds(
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        bounds_w: i32,
        bounds_h: i32,
        margin: i32,
    ) -> (i32, i32) {
        let w = w.max(1);
        let h = h.max(1);
        let min_x = margin.min(bounds_w.saturating_sub(1).max(0));
        let min_y = 0;
        let max_x = (bounds_w - w - margin).max(min_x);
        let max_y = (bounds_h - h).max(min_y);
        (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
    }

    fn output_for_rect(&self, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let cx = x + w.max(1) / 2;
        let cy = y + h.max(1) / 2;
        self.output_sizes
            .iter()
            .copied()
            .find(|(ox, oy, ow, oh)| x >= *ox && x < *ox + *ow && y >= *oy && y < *oy + *oh)
            .or_else(|| {
                self.output_sizes.iter().copied().find(|(ox, oy, ow, oh)| {
                    cx >= *ox && cx < *ox + *ow && cy >= *oy && cy < *oy + *oh
                })
            })
            .unwrap_or((0, 0, self.osize.w, self.osize.h))
    }

    fn clamp_global_rect_to_output(
        &self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        margin: i32,
    ) -> (i32, i32) {
        let (ox, oy, ow, oh) = self.output_for_rect(x, y, w, h);
        let (local_x, local_y) = Self::clamp_rect_to_bounds(x - ox, y - oy, w, h, ow, oh, margin);
        (ox + local_x, oy + local_y)
    }

    fn render_bounds_size(
        elems: &[WaylandSurfaceRenderElement<GlesRenderer>],
    ) -> Option<(i32, i32)> {
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for elem in elems {
            let geo = elem.geometry(1.0.into());
            min_x = min_x.min(geo.loc.x);
            min_y = min_y.min(geo.loc.y);
            max_x = max_x.max(geo.loc.x + geo.size.w);
            max_y = max_y.max(geo.loc.y + geo.size.h);
        }
        if min_x <= max_x && min_y <= max_y {
            Some(((max_x - min_x).max(1), (max_y - min_y).max(1)))
        } else {
            None
        }
    }

    /// 获取鼠标所在 output 的活跃工作区索引
    fn active_ws_for_pointer(&self) -> usize {
        // 用当前 active_ws（键盘焦点）— 后续会改为按 output 分配
        self.active_ws
    }

    fn pointer_focus(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        // 找到鼠标所在的 output，将全局坐标转为 output 局部坐标
        let oi = self.output_at_pointer();
        let (ox, oy, ow, oh) =
            self.output_sizes
                .get(oi)
                .copied()
                .unwrap_or((0, 0, self.osize.w, self.osize.h));
        let px = self.pointer_pos.0 - ox as f64;
        let py = self.pointer_pos.1 - oy as f64;

        let bar_h = if self.cfg.bar.enabled {
            self.cfg.bar.height
        } else {
            0
        };
        if py < bar_h as f64 {
            return None;
        }

        // 使用此 output 的 active_ws（不是全局的）
        let out_ws_idx = self
            .output_active_ws
            .get(oi)
            .copied()
            .unwrap_or(self.active_ws);
        let ws = &self.workspaces[out_ws_idx];

        // Fullscreen: 整个 output 区域（除 headbar 外）都属于全屏窗口
        // ── 关键修复：全屏时跳过所有非全屏窗口的 hit-test 和 focus fallback ──
        // 否则当 ws.focus 指向被全屏遮蔽的下方窗口时，鼠标事件会"穿透"到那个窗口
        let order = ws.effective_order();
        if let Some(fi) = ws.fullscreen {
            if let Some(slot) = order.get(fi) {
                match slot {
                    WindowSlot::Wl(idx) => {
                        if let Some(tl) = ws.tops.get(*idx) {
                            // 先检查 toplevel 的 XDG popup（候选词、菜单等）
                            let tl_pos = Point::from((0.0_f64, bar_h as f64));
                            if let Some(r) = self.popup_at_pointer(tl, tl_pos) {
                                return Some(r);
                            }
                            return Some((tl.wl_surface().clone(), tl_pos));
                        }
                    }
                    WindowSlot::X11(idx) => {
                        if let Some(xs) = ws.x11_surfaces.get(*idx) {
                            if let Some(wl) = xs.wl_surface() {
                                return Some((wl, Point::from((0.0, bar_h as f64))));
                            }
                        }
                    }
                }
            }
            // 全屏 slot 已设置但 slot 自身无效（被关闭）→ 仍处于全屏状态，
            // 兜底返回 None 而非 fallback 到 ws.focus（防止穿透到底层窗口）
            return None;
        }

        // 使用 output 局部尺寸做 slot 计算（不是 self.osize）
        let screen_w = ow as f64;
        let screen_h = oh as f64;

        // Check ALL toplevels' XDG popups first (they render on top of windows)
        for (i, slot) in order.iter().enumerate() {
            if let WindowSlot::Wl(idx) = slot {
                if let Some(tl) = ws.tops.get(*idx) {
                    let (x, y, _, _) = layout::slot(
                        i,
                        order.len(),
                        ow,
                        oh,
                        bar_h,
                        &self.cfg,
                        ws.layout,
                        ws.split,
                    );
                    let tl_geo =
                        smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                            states
                                .cached_state
                                .get::<smithay::wayland::shell::xdg::SurfaceCachedState>()
                                .current()
                                .geometry
                        })
                        .unwrap_or_default();
                    let tl_pos = Point::from((
                        x as f64 - tl_geo.loc.x as f64,
                        y as f64 - tl_geo.loc.y as f64,
                    ));
                    if let Some(r) = self.popup_at_pointer(tl, tl_pos) {
                        return Some(r);
                    }
                }
            }
        }

        // Check X11 override-redirect windows BEFORE slot hit-test
        // OR 窗口（右键菜单、输入法候选框等）可能超出 slot 区域，必须优先匹配
        for xs in &self.xw.or_surfaces {
            if let Some(wl) = xs.wl_surface() {
                let geo = xs.geometry();
                let local_x = (geo.loc.x - ox as i32) as f64;
                let local_y = (geo.loc.y - oy as i32) as f64;
                let local_w = geo.size.w as f64;
                let local_h = geo.size.h as f64;
                if px >= local_x
                    && px < local_x + local_w
                    && py >= local_y
                    && py < local_y + local_h
                {
                    return Some((wl, Point::from((local_x, local_y))));
                }
            }
        }

        // Hit-test window slots using unified order
        let n_all = order.len();
        for (i, slot) in order.iter().enumerate() {
            let (x, y, w, h) =
                layout::slot(i, n_all, ow, oh, bar_h, &self.cfg, ws.layout, ws.split);
            if px >= x as f64 && px < (x + w) as f64 && py >= y as f64 && py < (y + h) as f64 {
                match slot {
                    WindowSlot::Wl(idx) => {
                        if let Some(tl) = ws.tops.get(*idx) {
                            let s = tl.wl_surface().clone();
                            // 获取 geometry 偏移（CSD 阴影/边框），渲染位置需减去它
                            let tl_geo = smithay::wayland::compositor::with_states(&s, |states| {
                                states
                                    .cached_state
                                    .get::<smithay::wayland::shell::xdg::SurfaceCachedState>()
                                    .current()
                                    .geometry
                            })
                            .unwrap_or_default();
                            let bx = x as f64 - tl_geo.loc.x as f64;
                            let by = y as f64 - tl_geo.loc.y as f64;
                            let local_pos = Point::from((px - bx, py - by));
                            if let Some((sub, sub_loc)) =
                                smithay::desktop::utils::under_from_surface_tree(
                                    &s,
                                    local_pos,
                                    (0, 0),
                                    smithay::desktop::WindowSurfaceType::ALL,
                                )
                            {
                                let offset =
                                    Point::from((bx + sub_loc.x as f64, by + sub_loc.y as f64));
                                return Some((sub, offset));
                            }
                            return Some((s, Point::from((bx, by))));
                        }
                    }
                    WindowSlot::X11(idx) => {
                        if let Some(s) = ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()) {
                            return Some((s, Point::from((x as f64, y as f64))));
                        }
                    }
                }
            }
        }

        // 鼠标不在任何窗口、popup 或 OR 窗口内 → 无焦点
        None
    }

    /// Check if the pointer is over any XDG popup of the given toplevel.
    /// Returns (popup_wl_surface, popup_global_position) if found.
    /// `local_px, local_py` must be output-local coordinates (not global pointer_pos).
    fn popup_at_pointer_local(
        &self,
        tl: &ToplevelSurface,
        tl_pos: Point<f64, Logical>,
        local_px: f64,
        local_py: f64,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let tl_geo_loc = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
            states
                .cached_state
                .get::<smithay::wayland::shell::xdg::SurfaceCachedState>()
                .current()
                .geometry
                .map(|g| g.loc)
        })
        .unwrap_or_default();

        let popups: Vec<_> = PopupManager::popups_for_surface(tl.wl_surface()).collect();
        if popups.is_empty() {
            return None;
        }
        for (popup, popup_offset) in popups {
            let popup_geo = popup.geometry();
            let offset_x = (tl_geo_loc.x + popup_offset.x - popup_geo.loc.x) as f64;
            let offset_y = (tl_geo_loc.y + popup_offset.y - popup_geo.loc.y) as f64;
            let popup_x = tl_pos.x + offset_x;
            let popup_y = tl_pos.y + offset_y;
            let popup_w = popup_geo.size.w as f64;
            let popup_h = popup_geo.size.h as f64;

            if local_px >= popup_x
                && local_px < popup_x + popup_w
                && local_py >= popup_y
                && local_py < popup_y + popup_h
            {
                return Some((popup.wl_surface().clone(), Point::from((popup_x, popup_y))));
            }
        }
        None
    }

    /// Convenience wrapper using global pointer_pos (kept for backward compatibility).
    /// Converts global pointer_pos to output-local before calling popup_at_pointer_local.
    fn popup_at_pointer(
        &self,
        tl: &ToplevelSurface,
        tl_pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let oi = self.output_at_pointer();
        let (ox, oy, _, _) =
            self.output_sizes
                .get(oi)
                .copied()
                .unwrap_or((0, 0, self.osize.w, self.osize.h));
        let local_px = self.pointer_pos.0 - ox as f64;
        let local_py = self.pointer_pos.1 - oy as f64;
        self.popup_at_pointer_local(tl, tl_pos, local_px, local_py)
    }

    fn fullscreen(&self) -> Option<usize> {
        self.workspaces[self.active_ws].fullscreen
    }
    fn set_fullscreen(&mut self, v: Option<usize>) {
        self.workspaces[self.active_ws].fullscreen = v;
    }

    /// 布局指定工作区的所有窗口
    fn layout_workspace(&mut self, ws_idx: usize) {
        self.workspaces[ws_idx].rebuild_order();
        let order = self.workspaces[ws_idx].effective_order();
        let n = order.len();
        if n == 0 {
            return;
        }
        let bar_h = if self.cfg.bar.enabled {
            self.cfg.bar.height
        } else {
            0
        };

        if let Some(fi) = self.workspaces[ws_idx].fullscreen {
            if fi >= n {
                self.workspaces[ws_idx].fullscreen = None;
            }
        }
        let fullscreen = self.workspaces[ws_idx].fullscreen;

        // 找到该工作区所在的 output，使用该 output 的实际尺寸
        let (out_ox, out_oy, out_w, out_h) = self
            .output_active_ws
            .iter()
            .enumerate()
            .find(|(_, &ws)| ws == ws_idx)
            .and_then(|(oi, _)| self.output_sizes.get(oi).copied())
            .unwrap_or((0, 0, self.osize.w, self.osize.h));

        if let Some(fi) = fullscreen {
            for (i, slot) in order.iter().enumerate() {
                match slot {
                    WindowSlot::Wl(idx) => {
                        if let Some(tl) = self.workspaces[ws_idx].tops.get(*idx) {
                            if i == fi {
                                tl.with_pending_state(|st| {
                                    st.states.set(xdg_toplevel::State::Activated);
                                    st.states.set(xdg_toplevel::State::Fullscreen);
                                    st.size = Some((out_w, out_h - bar_h).into());
                                });
                            } else {
                                tl.with_pending_state(|st| {
                                    st.states.unset(xdg_toplevel::State::Activated);
                                    st.states.unset(xdg_toplevel::State::Fullscreen);
                                    st.size = Some((1, 1).into());
                                });
                            }
                            tl.send_configure();
                        }
                    }
                    WindowSlot::X11(idx) => {
                        if let Some(xs) = self.workspaces[ws_idx].x11_surfaces.get(*idx) {
                            if i == fi {
                                // X11 root window 坐标需要加上 output 偏移
                                let _ = xs.configure(Some(Rectangle::from_loc_and_size(
                                    (out_ox, out_oy + bar_h),
                                    (out_w, out_h - bar_h),
                                )));
                            } else {
                                let _ = xs
                                    .configure(Some(Rectangle::from_loc_and_size((0, 0), (1, 1))));
                            }
                        }
                    }
                }
            }
        } else {
            for (i, slot) in order.iter().enumerate() {
                let (x, y, w, h) = layout::slot(
                    i,
                    n,
                    out_w,
                    out_h,
                    bar_h,
                    &self.cfg,
                    self.workspaces[ws_idx].layout,
                    self.workspaces[ws_idx].split,
                );
                match slot {
                    WindowSlot::Wl(idx) => {
                        if let Some(tl) = self.workspaces[ws_idx].tops.get(*idx) {
                            tl.with_pending_state(|st| {
                                st.states.set(xdg_toplevel::State::Activated);
                                st.states.unset(xdg_toplevel::State::Fullscreen);
                                st.size = Some((w, h).into());
                            });
                            tl.send_configure();
                        }
                    }
                    WindowSlot::X11(idx) => {
                        if let Some(xs) = self.workspaces[ws_idx].x11_surfaces.get(*idx) {
                            // X11 root window 坐标需要加上 output 偏移
                            let _ = xs.configure(Some(Rectangle::from_loc_and_size(
                                (out_ox + x, out_oy + y),
                                (w, h),
                            )));
                        }
                    }
                }
            }
        }

        // 保存每个窗口的身份 + 位置（供下次动画使用）
        if ws_idx == self.active_ws {
            let bar_h = if self.cfg.bar.enabled {
                self.cfg.bar.height
            } else {
                0
            };
            let order = self.workspaces[ws_idx].effective_order();
            self.prev_positions = (0..n)
                .map(|i| {
                    let (x, y, _, _) = layout::slot(
                        i,
                        n,
                        self.osize.w,
                        self.osize.h,
                        bar_h,
                        &self.cfg,
                        self.workspaces[ws_idx].layout,
                        self.workspaces[ws_idx].split,
                    );
                    (order[i].clone(), (x, y))
                })
                .collect();
        }
    }

    fn do_layout(&mut self) {
        self.layout_workspace(self.active_ws);
    }

    /// 在 tops.remove/x11_surfaces.remove 之后重新映射 prev_positions
    /// 因为 remove 导致索引移位: Wl(1) 变成 Wl(0), 但 prev_positions 还映射旧索引
    fn remap_prev_after_remove(&mut self, removed: &WindowSlot) {
        let removed_clone = removed.clone();
        self.prev_positions
            .retain(|(slot, _)| match (slot, &removed_clone) {
                (WindowSlot::Wl(a), WindowSlot::Wl(b)) => a != b,
                (WindowSlot::X11(a), WindowSlot::X11(b)) => a != b,
                _ => true,
            });
        match removed {
            WindowSlot::Wl(removed_idx) => {
                for (slot, _) in &mut self.prev_positions {
                    if let WindowSlot::Wl(ref mut idx) = slot {
                        if *idx > *removed_idx {
                            *idx -= 1;
                        }
                    }
                }
            }
            WindowSlot::X11(removed_idx) => {
                for (slot, _) in &mut self.prev_positions {
                    if let WindowSlot::X11(ref mut idx) = slot {
                        if *idx > *removed_idx {
                            *idx -= 1;
                        }
                    }
                }
            }
        }
    }

    /// 触发布局动画 + 重新布局
    ///
    /// 策略：
    /// - 对比 layout 前后的窗口列表，自动检测 "新增窗口" vs "布局变化"
    /// - 新增窗口场景（窗口数增加，旧窗口不变）：
    ///   已有窗口起始 = 新位置（零偏移，不动画），新窗口从屏幕外滑入
    /// - 布局变化场景（窗口数不变，全屏切换等）：
    ///   所有窗口从旧位置动画到新位置
    /// - 动画进行中时跳过重启动画
    fn do_layout_animated(&mut self) {
        // 如果布局动画正在进行中，只执行布局不重启动画
        let anim_active = self.layout_anim.is_active();
        if anim_active {
            self.layout_workspace(self.active_ws);
            return;
        }

        // 1. 在 layout 之前保存旧位置和窗口数
        let old_snapshot = self.prev_positions.clone();
        let old_n = old_snapshot.len();

        // 2. 执行布局 → prev_positions 更新为新位置
        self.layout_workspace(self.active_ws);
        let new_positions = self.prev_positions.clone();
        let new_n = new_positions.len();

        // 3. 检测是否为 "纯新增窗口" 场景
        //    条件：新窗口数 > 旧窗口数，且旧窗口全部仍存在
        let is_pure_add = new_n > old_n
            && old_snapshot.iter().all(|(old_slot, _)| {
                new_positions
                    .iter()
                    .any(|(new_slot, _)| match (old_slot, new_slot) {
                        (WindowSlot::Wl(a), WindowSlot::Wl(b)) => a == b,
                        (WindowSlot::X11(a), WindowSlot::X11(b)) => a == b,
                        _ => false,
                    })
            });

        // 4. 构建 anim_positions
        // 记录窗口出现事件（用于渐入动画）
        let split = self.workspaces[self.active_ws].split;
        let mut anim_positions = Vec::new();

        for (slot, new_pos) in &new_positions {
            let old_entry = old_snapshot.iter().find(|(s, _)| match (s, slot) {
                (WindowSlot::Wl(a), WindowSlot::Wl(b)) => a == b,
                (WindowSlot::X11(a), WindowSlot::X11(b)) => a == b,
                _ => false,
            });

            if is_pure_add && old_entry.is_some() {
                // 纯新增场景：已有窗口不动画（起始 = 新位置 → 零偏移）
                anim_positions.push((slot.clone(), *new_pos));
            } else if let Some((_, old_pos)) = old_entry {
                // 布局变化场景：已有窗口从旧位置动画到新位置
                anim_positions.push((slot.clone(), *old_pos));
            } else {
                // 新增窗口：从屏幕外滑入
                let fake_pos = match split {
                    layout::SplitDir::Horizontal => (new_pos.0 + self.osize.w + 100, new_pos.1),
                    layout::SplitDir::Vertical => (new_pos.0, new_pos.1 + self.osize.h + 100),
                };
                anim_positions.push((slot.clone(), fake_pos));
            }
        }

        // 5. 启动动画
        if !anim_positions.is_empty() {
            self.layout_anim.begin(&anim_positions);
            self.dirty = true;
        } else {
            // no windows to animate
        }
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notifications.push(Notification {
            text: text.into(),
            created: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(3),
        });
    }

    /// 重载配置文件（不关闭窗口）
    fn reload_config(&mut self) {
        let new_cfg = Config::load();
        let old_cursor_theme = self.cfg.cursor.theme.clone();
        let old_cursor_size = self.cfg.cursor.size;

        // 保留运行时状态（键盘绑定在 config 更新后不重启）
        self.cfg = new_cfg;

        // 预解析颜色缓存
        self.cached_focus_color = config::parse_color(&self.cfg.colors.focus_border);
        self.cached_unfocus_color = config::parse_color(&self.cfg.colors.unfocus_border);

        // 如果光标主题或大小变了，重新加载
        if self.cfg.cursor.theme != old_cursor_theme || self.cfg.cursor.size != old_cursor_size {
            self.cursor_img = cursor::CursorImage::load_from_theme(
                &self.cfg.cursor.theme,
                &self.cfg.cursor.name,
                self.cfg.cursor.size,
            )
            .unwrap_or_else(|| cursor::CursorImage::builtin(self.cfg.cursor.size));
            self.cursor_cache.clear();
        }

        info!("🔄 配置已重载");
        self.notify("配置已重载 ✓");
        self.dirty = true;
    }

    /// 设置 VT 模式：process mode 使内核不拦截 Ctrl+Alt+Fx
    fn set_vt_process_mode(&mut self, process: bool) {
        if let Some(ref vt_fd) = self.vt_fd {
            let mode = VtMode {
                mode: if process { VT_PROCESS } else { VT_AUTO },
                waitv: 0,
                relsig: 0,
                acqsig: 0,
                frsig: 0,
            };
            unsafe {
                libc::ioctl(vt_fd.as_raw_fd(), VT_SETMODE, &mode as *const VtMode);
            }
        }
    }

    /// 切换到指定 TTY
    fn switch_vt(&mut self, vt: i32) {
        // 方法 1: 通过 libseat session
        if let Some(ref session) = self.session {
            if let Ok(mut s) = session.lock() {
                if s.change_vt(vt).is_ok() {
                    info!("🖥️  切换到 TTY {} (libseat)", vt);
                    return;
                }
            }
        }
        // 方法 2: 通过 VT ioctl
        if let Some(ref vt_fd) = self.vt_fd {
            unsafe {
                libc::ioctl(vt_fd.as_raw_fd(), VT_ACTIVATE, vt);
            }
            unsafe {
                libc::ioctl(vt_fd.as_raw_fd(), VT_WAITACTIVE, vt);
            }
            info!("🖥️  切换到 TTY {} (ioctl)", vt);
            return;
        }
        // 方法 3: chvt 命令
        let _ = std::process::Command::new("chvt")
            .arg(vt.to_string())
            .spawn();
        info!("🖥️  切换到 TTY {} (chvt)", vt);
    }

    /// 将 PNG 图片数据设到剪贴板（image/png）
    ///
    /// 设置 Wayland data device selection + 通知 X11 端（xwm.new_selection）。
    /// X11 客户端粘贴时，XwmHandler::send_selection 会直接从 compositor selection
    /// 读取 user_data 写入 fd（不经过 request_data_device_client_selection，
    /// 因为该函数对 compositor-owned selection 返回 ServerSideSelection 错误）。
    fn set_clipboard_png(&mut self, _png_path: String, png_data: Vec<u8>) {
        use smithay::wayland::selection::data_device::set_data_device_selection;
        tracing::info!("📋 设置截图剪贴板: {} bytes", png_data.len());
        let user_data: Arc<[u8]> = Arc::from(png_data);
        let mime_types = vec!["image/png".into()];
        set_data_device_selection::<App>(&self.dh, &self.seat, mime_types.clone(), user_data);
        // 合成器 set_data_device_selection 不会触发 SelectionHandler::new_selection，
        // 需要手动通知 X11 端让 XWM 成为 X11 CLIPBOARD owner
        if let Some(ref mut xwm) = self.xw.xwm {
            if let Err(e) = xwm.new_selection(
                smithay::wayland::selection::SelectionTarget::Clipboard,
                Some(mime_types),
            ) {
                tracing::warn!("📋 XWM new_selection failed: {:?}", e);
            } else {
                tracing::info!("📋 Screenshot copied to Wayland + X11 clipboard");
            }
        } else {
            tracing::info!("📋 Screenshot copied to Wayland clipboard (no XWM)");
        }
    }

    fn drain_notifications(&mut self) {
        let now = std::time::Instant::now();
        self.notifications
            .retain(|n| now.duration_since(n.created) < n.duration);
    }

    /// Read CPU usage from /proc/stat (delta-based)
    fn update_cpu_usage(&mut self) {
        if let Ok(data) = std::fs::read_to_string("/proc/stat") {
            if let Some(line) = data.lines().next() {
                let fields: Vec<u64> = line
                    .split_whitespace()
                    .skip(1) // skip "cpu"
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if fields.len() >= 4 {
                    let idle = fields[3];
                    let total: u64 = fields.iter().sum();
                    let d_idle = idle.saturating_sub(self.cpu_prev_idle);
                    let d_total = total.saturating_sub(self.cpu_prev_total);
                    if d_total > 0 {
                        self.cpu_usage = 1.0 - d_idle as f32 / d_total as f32;
                    }
                    self.cpu_prev_idle = idle;
                    self.cpu_prev_total = total;
                }
            }
        }
    }

    /// Read memory usage from /proc/meminfo
    fn update_mem_usage(&mut self) {
        if let Ok(data) = std::fs::read_to_string("/proc/meminfo") {
            let mut mem_total: u64 = 0;
            let mut mem_available: u64 = 0;
            for line in data.lines() {
                if line.starts_with("MemTotal:") {
                    mem_total = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                } else if line.starts_with("MemAvailable:") {
                    mem_available = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                }
                if mem_total > 0 && mem_available > 0 {
                    break;
                }
            }
            if mem_total > 0 {
                self.mem_usage = 1.0 - mem_available as f32 / mem_total as f32;
            }
        }
    }

    fn toggle_fullscreen(&mut self) {
        let fi = self.focus_idx();
        let fs = self.workspaces[self.active_ws].fullscreen;
        match (fi, fs) {
            (Some(idx), Some(fullscreen_idx)) if idx == fullscreen_idx => {
                info!("🔳 取消全屏 #{}", idx);
                self.workspaces[self.active_ws].fullscreen = None;
            }
            (Some(idx), _) => {
                info!("🔳 全屏 #{}", idx);
                self.workspaces[self.active_ws].fullscreen = Some(idx);
                // 关键修复：全屏时强制把 ws.focus 指向全屏窗口，
                // 避免后续 pointer_focus 的 ws.focus fallback 把鼠标事件转发到被遮蔽的下方窗口
                let focus_surf = {
                    let ws = &self.workspaces[self.active_ws];
                    let order = ws.effective_order();
                    order.get(idx).and_then(|slot| match slot {
                        WindowSlot::Wl(i) => ws.tops.get(*i).map(|tl| tl.wl_surface().clone()),
                        WindowSlot::X11(i) => {
                            ws.x11_surfaces.get(*i).and_then(|xs| xs.wl_surface())
                        }
                    })
                };
                if let Some(surf) = focus_surf {
                    self.workspaces[self.active_ws].focus = Some(surf.clone());
                    let kbd = self.kbd.clone();
                    let serial = SERIAL_COUNTER.next_serial();
                    kbd.set_focus(self, Some(surf), serial);
                }
            }
            _ => return,
        }
        self.do_layout_animated();
        self.dirty = true;
    }

    /// 切换到指定工作区（只替换鼠标所在 output 的工作区）
    /// 支持无限滚动：通过弹簧动画平滑过渡到目标工作区
    fn switch_workspace(&mut self, target: usize) {
        if target >= NUM_WORKSPACES {
            return;
        }

        let out_idx = self.focused_output;

        // 检查目标工作区是否已经在某个 output 上显示
        for (oi, ws) in self.output_active_ws.iter().enumerate() {
            if *ws == target {
                if oi != out_idx {
                    // 目标工作区在另一个屏幕上，移鼠标过去
                    let (ox, oy, ow, oh) = self.output_sizes.get(oi).copied().unwrap_or_default();
                    self.pointer_pos = (ox as f64 + ow as f64 / 2.0, oy as f64 + oh as f64 / 2.0);
                    self.focused_output = oi;
                    crate::appctl::emit_output_focused_changed(self);
                    self.active_ws = target;
                    // 同步 scroll 状态到目标 output
                    self.scroll_offset = self
                        .scroll_offsets
                        .get(oi)
                        .copied()
                        .unwrap_or(target as f64);
                    self.scroll_spring.set(self.scroll_offset);
                    self.scroll_spring.set_target(self.scroll_offset);
                    self.scroll_momentum.reset();
                    // 同步 prev_positions 到新 active_ws（跨屏切换时防止旧 ws 数据污染）
                    self.layout_workspace(target);
                    self.notify(format!("Workspace {} (screen {})", target + 1, oi + 1));
                    self.dirty = true;
                    // 设置焦点
                    let ws_ref = &self.workspaces[target];
                    if let Some(ref surf) = ws_ref.focus {
                        if ws_ref.tops.iter().any(|tl| tl.wl_surface() == surf) {
                            let kbd = self.kbd.clone();
                            let serial = SERIAL_COUNTER.next_serial();
                            kbd.set_focus(self, Some(surf.clone()), serial);
                        }
                    }
                }
                return; // 已在某个屏幕上显示
            }
        }

        // 目标工作区不在任何屏幕上 → 替换当前 focused output 的工作区
        let old_ws = self.output_active_ws[out_idx];
        if target == old_ws {
            return;
        }
        info!(
            "🔀 屏幕 {} 工作区 {} → {}",
            out_idx + 1,
            old_ws + 1,
            target + 1
        );

        // ── 弹簧动画驱动切换 ──
        // 先同步 spring.x 到当前位置，再设 target（零延迟，无跳变）
        self.scroll_spring.x = self.scroll_offset;
        self.scroll_spring.set_target(target as f64);
        self.scroll_momentum.reset();

        // 隐藏旧工作区的窗口（取消激活但不缩小尺寸，保持 buffer 用于缩略图和滚动过渡）
        for tl in &self.workspaces[old_ws].tops {
            tl.with_pending_state(|st| {
                st.states.unset(xdg_toplevel::State::Activated);
                st.states.unset(xdg_toplevel::State::Fullscreen);
            });
            tl.send_configure();
        }

        // 更新 per-output 和全局工作区
        self.output_active_ws[out_idx] = target;
        self.active_ws = target;
        self.notify(format!("Workspace {} (screen {})", target + 1, out_idx + 1));

        // 布局新工作区的窗口
        self.do_layout();

        // 设置焦点
        let ws = &self.workspaces[self.active_ws];
        if let Some(ref surf) = ws.focus {
            if ws.tops.iter().any(|tl| tl.wl_surface() == surf) {
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(surf.clone()), serial);
            }
        }
        self.dirty = true;
    }

    /// 切换到相邻工作区（带弹簧动画，用于方向键和手势结束）
    fn switch_workspace_direction(&mut self, direction: i32) {
        let current = self.active_ws as i32;
        let next = (current + direction).rem_euclid(NUM_WORKSPACES as i32) as usize;
        self.switch_workspace(next);
    }

    /// 将当前焦点窗口移动到目标工作区，然后跟随窗口切换到目标工作区
    /// 支持 Wayland (tops) 和 X11 (x11_surfaces) 窗口
    fn move_window_to_workspace(&mut self, target: usize) {
        if target >= NUM_WORKSPACES {
            return;
        }
        let out_idx = self.focused_output;
        let ws_idx = self.output_active_ws.get(out_idx).copied().unwrap_or(0);
        if target == ws_idx {
            return;
        }
        let fi = match self.focus_idx() {
            Some(i) => i,
            None => return,
        };

        let order = self.workspaces[ws_idx].effective_order();
        let slot = match order.get(fi) {
            Some(s) => s.clone(),
            None => return,
        };

        // 1. 先 clone 窗口和 wl_surface（在 remove 之前）
        let surf = match &slot {
            WindowSlot::Wl(idx) => self.workspaces[ws_idx]
                .tops
                .get(*idx)
                .map(|tl| tl.wl_surface().clone()),
            WindowSlot::X11(idx) => self.workspaces[ws_idx]
                .x11_surfaces
                .get(*idx)
                .and_then(|xs| xs.wl_surface()),
        };
        let surf = match surf {
            Some(s) => s,
            None => return,
        };

        info!(
            "📦 移动窗口 slot {:?} (order #{}) → 工作区 {}",
            slot,
            fi,
            target + 1
        );

        // 2. 从源工作区移除，添加到目标工作区
        match slot {
            WindowSlot::Wl(idx) => {
                let tl = match self.workspaces[ws_idx].tops.get(idx).cloned() {
                    Some(tl) => tl,
                    None => return,
                };
                self.workspaces[ws_idx].tops.remove(idx);
                self.remap_prev_after_remove(&WindowSlot::Wl(idx));
                self.workspaces[ws_idx].rebuild_order();
                self.workspaces[ws_idx].fullscreen = None;
                self.workspaces[ws_idx].focus = self.workspaces[ws_idx]
                    .tops
                    .last()
                    .map(|t| t.wl_surface().clone());

                self.workspaces[target].tops.push(tl);
            }
            WindowSlot::X11(idx) => {
                let xs = match self.workspaces[ws_idx].x11_surfaces.get(idx).cloned() {
                    Some(xs) => xs,
                    None => return,
                };
                self.workspaces[ws_idx].x11_surfaces.remove(idx);
                self.remap_prev_after_remove(&WindowSlot::X11(idx));
                self.workspaces[ws_idx].rebuild_order();
                self.workspaces[ws_idx].fullscreen = None;
                {
                    let src_order = self.workspaces[ws_idx].effective_order();
                    self.workspaces[ws_idx].focus = src_order.last().and_then(|s| match s {
                        WindowSlot::Wl(i) => self.workspaces[ws_idx]
                            .tops
                            .get(*i)
                            .map(|tl| tl.wl_surface().clone()),
                        WindowSlot::X11(i) => self.workspaces[ws_idx]
                            .x11_surfaces
                            .get(*i)
                            .and_then(|x| x.wl_surface()),
                    });
                }

                self.workspaces[target].x11_surfaces.push(xs);
            }
        }
        self.workspaces[target].focus = Some(surf.clone());
        self.workspaces[target].rebuild_order();

        // 4. 布局源工作区
        self.active_ws = ws_idx;
        self.do_layout_animated();

        // 布局目标工作区
        self.layout_workspace(target);

        // 5. 切换到目标工作区
        let target_output = self.output_active_ws.iter().position(|&ws| ws == target);
        if let Some(t_oi) = target_output {
            let (ox, oy, ow, oh) = self.output_sizes.get(t_oi).copied().unwrap_or_default();
            self.pointer_pos = (ox as f64 + ow as f64 / 2.0, oy as f64 + oh as f64 / 2.0);
            self.focused_output = t_oi;
            crate::appctl::emit_output_focused_changed(self);
            self.active_ws = target;
            self.scroll_offset = self
                .scroll_offsets
                .get(t_oi)
                .copied()
                .unwrap_or(target as f64);
            self.scroll_spring.set(self.scroll_offset);
            self.scroll_spring.set_target(self.scroll_offset);
            self.scroll_momentum.reset();
            // 同步 prev_positions 到新 active_ws（跨屏切换时防止旧 ws 数据污染）
            self.layout_workspace(target);
        } else {
            self.switch_workspace(target);
        }

        // 6. 设置焦点
        let kbd = self.kbd.clone();
        let serial = SERIAL_COUNTER.next_serial();
        kbd.set_focus(self, Some(surf), serial);
        self.notify(format!("Moved → WS {}", target + 1));
        self.dirty = true;
    }

    /// 用方向键切换焦点窗口（Super+方向键）
    /// 按屏幕真实几何位置：找到当前焦点窗口在指定方向上最近的邻居
    fn focus_direction(&mut self, direction: Keysym) {
        let order = self.workspaces[self.active_ws].effective_order();
        let n = order.len();
        if n == 0 {
            return;
        }

        let fi = match self.focus_idx() {
            Some(i) => i,
            None => 0,
        };

        let bar_h = if self.cfg.bar.enabled {
            self.cfg.bar.height
        } else {
            0
        };
        let slots: Vec<(i32, i32, i32, i32)> = (0..n)
            .map(|i| {
                layout::slot(
                    i,
                    n,
                    self.osize.w,
                    self.osize.h,
                    bar_h,
                    &self.cfg,
                    self.workspaces[self.active_ws].layout,
                    self.workspaces[self.active_ws].split,
                )
            })
            .collect();

        let (fx, fy, fw, fh) = slots[fi];
        let fcx = fx + fw / 2;
        let fcy = fy + fh / 2;

        let mut best_idx: Option<usize> = None;
        let mut best_dist: i32 = i32::MAX;

        for (i, &(sx, sy, sw, sh)) in slots.iter().enumerate() {
            if i == fi {
                continue;
            }
            let scx = sx + sw / 2;
            let scy = sy + sh / 2;

            let (is_valid, dist) = match direction {
                Keysym::Left => (scx < fcx, (fcx - scx).abs()),
                Keysym::Right => (scx > fcx, (scx - fcx).abs()),
                Keysym::Up => (scy < fcy, (fcy - scy).abs()),
                Keysym::Down => (scy > fcy, (scy - fcy).abs()),
                _ => (false, i32::MAX),
            };

            if is_valid && dist < best_dist {
                best_dist = dist;
                best_idx = Some(i);
            }
        }

        let target = match best_idx {
            Some(t) => t,
            None => return,
        };

        let ws = &self.workspaces[self.active_ws];
        let surf = match &order[target] {
            WindowSlot::Wl(i) => ws.tops.get(*i).map(|tl| tl.wl_surface().clone()),
            WindowSlot::X11(i) => ws.x11_surfaces.get(*i).and_then(|xs| xs.wl_surface()),
        };
        if let Some(surf) = surf {
            info!("🔍 焦点切换 {} → {}", fi + 1, target + 1);
            self.workspaces[self.active_ws].focus = Some(surf.clone());
            let kbd = self.kbd.clone();
            let serial = SERIAL_COUNTER.next_serial();
            kbd.set_focus(self, Some(surf), serial);
            self.dirty = true;
        }
    }

    /// 用方向键交换窗口位置（Super+Shift+方向键）
    /// 按屏幕真实几何位置：找到当前窗口在指定方向上最近的邻居并交换
    fn swap_window(&mut self, direction: Keysym) {
        let fi = match self.focus_idx() {
            Some(i) => i,
            None => return,
        };
        let ws = &mut self.workspaces[self.active_ws];
        ws.rebuild_order();
        let order = ws.effective_order();
        let n = order.len();
        if n <= 1 {
            return;
        }

        let bar_h = if self.cfg.bar.enabled {
            self.cfg.bar.height
        } else {
            0
        };

        // 使用焦点窗口所在的 output 尺寸（不是 self.osize）
        let (ow, oh) = self
            .output_sizes
            .get(self.focused_output)
            .map(|&(_, _, w, h)| (w, h))
            .unwrap_or((self.osize.w, self.osize.h));

        let slots: Vec<(i32, i32, i32, i32)> = (0..n)
            .map(|i| layout::slot(i, n, ow, oh, bar_h, &self.cfg, ws.layout, ws.split))
            .collect();

        let (fx, fy, fw, fh) = slots[fi];

        // 找到指定方向上最合适的邻居
        let mut best_idx: Option<usize> = None;
        let mut best_score: i64 = i64::MAX;

        for (i, &(sx, sy, sw, sh)) in slots.iter().enumerate() {
            if i == fi {
                continue;
            }

            // 计算 y-overlap 分数（用于 Left/Right 判断是否同一行）
            let y_overlap = (fy + fh).min(sy + sh) - fy.max(sy).max(0);
            let y_overlap_ratio = y_overlap as f32 / (fh.min(sh).max(1)) as f32;
            // x-overlap 分数（用于 Up/Down 判断是否同一列）
            let x_overlap = (fx + fw).min(sx + sw) - fx.max(sx).max(0);
            let x_overlap_ratio = x_overlap as f32 / (fw.min(sw).max(1)) as f32;

            let (is_candidate, primary_dist) = match direction {
                // Left: 窗口在焦点左侧。优先同行的（y-overlap 大），其次距离近的
                Keysym::Left => {
                    let right_edge = sx + sw;
                    let left_of = right_edge <= fx + 4; // 4px 容差
                    if left_of {
                        let dist = (fx - right_edge) as i64;
                        // 分数越小越好：距离优先，但同行权重更高
                        let score = dist + ((1.0 - y_overlap_ratio) * 10000.0) as i64;
                        (true, score)
                    } else {
                        (false, 0)
                    }
                }
                // Right: 窗口在焦点右侧
                Keysym::Right => {
                    let left_edge = sx;
                    let right_of = left_edge >= fx + fw - 4;
                    if right_of {
                        let dist = (left_edge - (fx + fw)) as i64;
                        let score = dist + ((1.0 - y_overlap_ratio) * 10000.0) as i64;
                        (true, score)
                    } else {
                        (false, 0)
                    }
                }
                // Up: 窗口在焦点上方。优先同列的（x-overlap 大）
                Keysym::Up => {
                    let bottom_edge = sy + sh;
                    let above = bottom_edge <= fy + 4;
                    if above {
                        let dist = (fy - bottom_edge) as i64;
                        let score = dist + ((1.0 - x_overlap_ratio) * 10000.0) as i64;
                        (true, score)
                    } else {
                        (false, 0)
                    }
                }
                // Down: 窗口在焦点下方
                Keysym::Down => {
                    let top_edge = sy;
                    let below = top_edge >= fy + fh - 4;
                    if below {
                        let dist = (top_edge - (fy + fh)) as i64;
                        let score = dist + ((1.0 - x_overlap_ratio) * 10000.0) as i64;
                        (true, score)
                    } else {
                        (false, 0)
                    }
                }
                _ => (false, 0),
            };

            if is_candidate && primary_dist < best_score {
                best_score = primary_dist;
                best_idx = Some(i);
            }
        }

        let target = match best_idx {
            Some(t) => t,
            None => return,
        };

        ws.window_order.swap(fi, target);
        if let Some(fs) = ws.fullscreen {
            if fs == fi {
                ws.fullscreen = Some(target);
            } else if fs == target {
                ws.fullscreen = Some(fi);
            }
        }
        drop(ws);
        self.do_layout_animated();
        self.dirty = true;
    }

    fn handle_input_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        use smithay::backend::input::{
            Event as _, KeyboardKeyEvent as _, PointerButtonEvent as _, PointerMotionEvent as _,
        };
        // ── 空闲检测：任何输入都重置空闲计时 ──
        let was_idle = !self.idle_active;
        self.last_input_time = std::time::Instant::now();
        if was_idle {
            self.idle_active = true;
            self.dirty = true;
        }
        self.idle_notifier.notify_activity(&self.seat);
        match event {
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let state = event.state();
                let time = (event.time() / 1000) as u32;
                let serial = SERIAL_COUNTER.next_serial();
                let kbd = self.kbd.clone();
                let _ = smithay::input::keyboard::KeyboardHandle::<Self>::input(
                    &kbd,
                    self,
                    keycode,
                    state,
                    serial,
                    time,
                    |data: &mut App,
                     mods: &ModifiersState,
                     keysym: smithay::input::keyboard::KeysymHandle<'_>| {
                        // ── 截图区域选择模式键盘处理 ──
                        if data.screenshot.selecting && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            if sym == Keysym::Escape {
                                data.screenshot.cancel();
                                data.record_state.selecting = false;
                                data.dirty = true;
                                return FilterResult::Intercept(());
                            }
                        }
                        // ── 锁屏模式键盘处理 ──
                        if data.lock_state.locked && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            match sym {
                                Keysym::Escape => {
                                    data.lock_state.clear();
                                    data.lock_state.last_unlock = Some(std::time::Instant::now());
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Return => {
                                    data.lock_state.try_unlock();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::BackSpace => {
                                    data.lock_state.backspace();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                _ => {
                                    // Printable characters → append to password
                                    let ch = match sym.raw() {
                                        32..=126 => Some(sym.raw() as u8 as char),
                                        _ => None,
                                    };
                                    if let Some(c) = ch {
                                        if !mods.logo && !mods.ctrl && !mods.alt {
                                            data.lock_state.push_char(c);
                                            data.dirty = true;
                                            return FilterResult::Intercept(());
                                        }
                                    }
                                }
                            }
                            // Intercept all other keys when locked
                            return FilterResult::Intercept(());
                        }
                        if data.lock_state.locked {
                            return FilterResult::Intercept(());
                        }
                        // ── 启动器模式键盘处理 ──
                        if data.launcher.visible && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            match sym {
                                Keysym::Escape => {
                                    data.launcher.close();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Return => {
                                    data.launcher.select_and_launch(data.xdisplay);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Up => {
                                    data.launcher.select_up();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Down => {
                                    data.launcher.select_down();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::BackSpace => {
                                    data.launcher.backspace();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                _ => {
                                    // 处理可打印字符
                                    let sym = keysym.modified_sym();
                                    let ch = match sym {
                                        k if k.raw() >= 32 && k.raw() < 127 => {
                                            // ASCII 可打印字符
                                            Some(k.raw() as u8 as char)
                                        }
                                        _ => None,
                                    };
                                    if let Some(c) = ch {
                                        if !mods.logo && !mods.ctrl && !mods.alt {
                                            data.launcher.push_char(c);
                                            data.dirty = true;
                                            return FilterResult::Intercept(());
                                        }
                                    }
                                }
                            }
                        }

                        // ── Settings Panel 键盘处理 ──
                        if data.settings.is_active() && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            match sym {
                                Keysym::Escape => {
                                    data.settings.close();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Tab => {
                                    let tab = if mods.shift {
                                        data.settings.tab().prev()
                                    } else {
                                        data.settings.tab().next()
                                    };
                                    data.settings.switch_tab(tab);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Up => {
                                    data.settings.prev_focus();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Down => {
                                    data.settings.next_focus();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Left => {
                                    data.settings.adjust_focus(-1.0);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Right => {
                                    data.settings.adjust_focus(1.0);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Return => {
                                    if mods.ctrl {
                                        // Ctrl+Enter: Apply (保存到 disk)
                                        match data.settings.apply() {
                                            Ok(()) => {
                                                // 重新加载配置到运行时
                                                data.cfg = crate::config::Config::load();
                                                data.cached_focus_color = config::parse_color(
                                                    &data.cfg.colors.focus_border,
                                                );
                                                data.cached_unfocus_color = config::parse_color(
                                                    &data.cfg.colors.unfocus_border,
                                                );
                                                data.notify("✓ Configuration saved");
                                                data.dirty = true;
                                            }
                                            Err(e) => {
                                                data.notify(&format!("✗ Failed: {}", e));
                                            }
                                        }
                                        return FilterResult::Intercept(());
                                    }
                                    // 普通 Enter: 激活当前聚焦控件（toggle/radio）
                                    data.settings.activate_focus();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::r if mods.ctrl => {
                                    // Ctrl+R: 重置编辑
                                    data.settings.reset(&data.cfg);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                _ => {
                                    return FilterResult::Intercept(());
                                }
                            }
                        }

                        // ── Expose 模式：Tab/方向键选窗口 ──
                        if data.overview.is_expose() && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            match sym {
                                Keysym::Left | Keysym::Up => {
                                    data.overview.expose_scroll(-1);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Right | Keysym::Down | Keysym::Tab => {
                                    data.overview.expose_scroll(1);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Return => {
                                    // 全局视图：通过 expose_thumbs 找到选中窗口并 switch_workspace
                                    let sel = data.overview.expose_selected();
                                    if let Some(&(tx, ty, tw, th, target_ws, ref slot)) =
                                        data.expose_thumbs.get(sel)
                                    {
                                        // 在目标 ws 中找到选中窗口的 slot
                                        let sel_in_ws = data
                                            .expose_thumbs
                                            .iter()
                                            .filter(|t| t.4 == target_ws)
                                            .position(|t| t.0 == tx && t.1 == ty)
                                            .unwrap_or(0);

                                        // 提前 clone 需要的数据，避免借用冲突
                                        let action: Option<(Option<WlSurface>, Option<WlSurface>)> = {
                                            let ws = &data.workspaces[target_ws];
                                            let order = ws.effective_order();
                                            if let Some(target_slot) = order.get(sel_in_ws) {
                                                match target_slot {
                                                    WindowSlot::Wl(idx) => {
                                                        ws.tops.get(*idx).map(|tl| {
                                                            (Some(tl.wl_surface().clone()), None)
                                                        })
                                                    }
                                                    WindowSlot::X11(idx) => ws
                                                        .x11_surfaces
                                                        .get(*idx)
                                                        .and_then(|xs| xs.wl_surface())
                                                        .map(|wl| (None, Some(wl.clone()))),
                                                }
                                            } else {
                                                None
                                            }
                                        };

                                        if let Some((wl_surf, x11_surf)) = action {
                                            let focus_surf = wl_surf.or(x11_surf).unwrap();
                                            data.workspaces[target_ws].focus =
                                                Some(focus_surf.clone());
                                            let kbd = data.kbd.clone();
                                            let serial = SERIAL_COUNTER.next_serial();
                                            kbd.set_focus(data, Some(focus_surf), serial);

                                            // 设置 Activated state
                                            let ws = &data.workspaces[target_ws];
                                            let order = ws.effective_order();
                                            if let Some(target_slot) = order.get(sel_in_ws) {
                                                if let WindowSlot::Wl(idx) = target_slot {
                                                    if let Some(tl) = ws.tops.get(*idx) {
                                                        tl.with_pending_state(|st| {
                                                            st.states.set(
                                                                xdg_toplevel::State::Activated,
                                                            );
                                                        });
                                                        tl.send_configure();
                                                    }
                                                }
                                            }
                                        }
                                        data.overview.close();
                                        if target_ws != data.active_ws {
                                            data.switch_workspace(target_ws);
                                        }
                                    } else {
                                        data.overview.close();
                                    }
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                _ => {}
                            }
                        }

                        // ── 非 Super 键的全局处理 ──
                        // Escape 关闭 overview（不需要按住 Super）
                        // ── Overview/Task Panel 模式：Escape 关闭 ──
                        if state == KeyState::Pressed && keysym.modified_sym() == Keysym::Escape {
                            if data.overview.is_active() {
                                data.overview.close();
                                data.dirty = true;
                                return FilterResult::Intercept(());
                            }
                        }

                        // ── Task Panel 模式：left/right 滚动 ws 列表 ──
                        if data.overview.is_task_panel() && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            let n_ws = NUM_WORKSPACES;
                            match sym {
                                Keysym::Left => {
                                    data.overview.task_panel_scroll(-1, n_ws);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Right => {
                                    data.overview.task_panel_scroll(1, n_ws);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Return => {
                                    // 吸附到当前选中的 ws 并关闭
                                    let target = data.overview.task_panel_ws();
                                    data.overview.close();
                                    data.switch_workspace(target);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                _ => {}
                            }
                        }

                        // ── Ctrl+Alt+F1..F7: 切换 TTY ──
                        if state == KeyState::Pressed
                            && mods.ctrl
                            && mods.alt
                            && !mods.logo
                            && !mods.shift
                        {
                            let vt = match keysym.modified_sym() {
                                Keysym::F1 => Some(1),
                                Keysym::F2 => Some(2),
                                Keysym::F3 => Some(3),
                                Keysym::F4 => Some(4),
                                Keysym::F5 => Some(5),
                                Keysym::F6 => Some(6),
                                Keysym::F7 => Some(7),
                                _ => None,
                            };
                            if let Some(vt) = vt {
                                data.switch_vt(vt);
                                return FilterResult::Intercept(());
                            }
                        }

                        if state == KeyState::Pressed && mods.logo {
                            let uid = unsafe { libc::getuid() };
                            match keysym.modified_sym() {
                                Keysym::Return => {
                                    info!("⌨️  启动终端");
                                    let mut cmd =
                                        std::process::Command::new(&data.cfg.terminal.command);
                                    // 继承 anchor 自身的全部环境（含 GPU EGL 变量），
                                    // 再覆盖 Wayland/输入法相关变量
                                    cmd.env_clear();
                                    for (k, v) in std::env::vars() {
                                        cmd.env(k, v);
                                    }
                                    cmd.env("WAYLAND_DISPLAY", "wayland-anchor")
                                        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
                                        .env("XMODIFIERS", "@im=fcitx")
                                        .env("QT_IM_MODULE", "fcitx")
                                        .env("GTK_IM_MODULE", "fcitx");
                                    if let Some(d) = data.xdisplay {
                                        cmd.env("DISPLAY", format!(":{}", d));
                                    }
                                    cmd.spawn().ok();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Escape => {
                                    if mods.shift {
                                        data.run = false;
                                    } else if data
                                        .lock_state
                                        .last_unlock
                                        .map_or(true, |t| t.elapsed().as_millis() > 1000)
                                    {
                                        data.lock_state.lock(data.pointer_pos.0);
                                        data.dirty = true;
                                    }
                                    return FilterResult::Intercept(());
                                }
                                // Super+Shift+R: 区域录制（选区模式）
                                Keysym::R if mods.shift => {
                                    data.screenshot.begin_selection();
                                    data.record_state.selecting = true;
                                    data.notify("Select area to record (drag, Esc to cancel)");
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                // Super+Shift+C: 重载配置（不关闭窗口）
                                // 注意：Shift 下 modified_sym() 返回大写 C
                                Keysym::C if mods.shift => {
                                    data.reload_config();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::q => {
                                    // 关闭焦点窗口，或鼠标位置命中的窗口
                                    let mut closed = false;
                                    if let Some(ref surf) =
                                        data.workspaces[data.active_ws].focus.clone()
                                    {
                                        let ws = &data.workspaces[data.active_ws];
                                        // Try Wayland toplevel first
                                        if let Some(tl) =
                                            ws.tops.iter().find(|tl| tl.wl_surface() == surf)
                                        {
                                            tl.send_close();
                                            closed = true;
                                        }
                                        // Try X11 surface in tiling layout
                                        if let Some(xs) = ws
                                            .x11_surfaces
                                            .iter()
                                            .find(|xs| xs.wl_surface().as_ref() == Some(surf))
                                        {
                                            let _ = xs.close();
                                            closed = true;
                                        }
                                    }

                                    // 焦点窗口关闭失败：通过鼠标位置 hit-test 关闭窗口
                                    // 覆盖：OR 窗口 + tiling 中的 X11 窗口（可能没有 focus）
                                    if !closed {
                                        let px_global = data.pointer_pos.0 as i32;
                                        let py_global = data.pointer_pos.1 as i32;

                                        // 1) 尝试关闭鼠标位置下的 OR 窗口
                                        if let Some(xs) = data.xw.or_surfaces.iter().find(|xs| {
                                            let geo = xs.geometry();
                                            px_global >= geo.loc.x
                                                && px_global < geo.loc.x + geo.size.w
                                                && py_global >= geo.loc.y
                                                && py_global < geo.loc.y + geo.size.h
                                        }) {
                                            tracing::info!(
                                                "🔒 Closing OR window at pointer: class='{}'",
                                                xs.class()
                                            );
                                            let _ = xs.close();
                                            closed = true;
                                        }

                                        // 2) 尝试关闭鼠标位置下的 tiling X11 窗口
                                        if !closed {
                                            let oi = data.output_at_pointer();
                                            let (ox, oy, ow, oh) = data
                                                .output_sizes
                                                .get(oi)
                                                .copied()
                                                .unwrap_or((0, 0, data.osize.w, data.osize.h));
                                            let bar_h = if data.cfg.bar.enabled {
                                                data.cfg.bar.height
                                            } else {
                                                0
                                            };
                                            let local_px = px_global - ox;
                                            let local_py = py_global - oy;
                                            let ws_idx = data
                                                .output_active_ws
                                                .get(oi)
                                                .copied()
                                                .unwrap_or(data.active_ws);
                                            let ws = &data.workspaces[ws_idx];
                                            let order = ws.effective_order();
                                            let n = order.len();
                                            for (i, slot) in order.iter().enumerate() {
                                                let (x, y, w, h) = layout::slot(
                                                    i, n, ow, oh, bar_h, &data.cfg, ws.layout,
                                                    ws.split,
                                                );
                                                if local_px >= x
                                                    && local_px < x + w
                                                    && local_py >= y
                                                    && local_py < y + h
                                                {
                                                    if let WindowSlot::X11(idx) = slot {
                                                        if let Some(xs) = ws.x11_surfaces.get(*idx)
                                                        {
                                                            tracing::info!(
                                                                "🔒 Closing X11 tiling window at pointer: class='{}'",
                                                                xs.class()
                                                            );
                                                            let _ = xs.close();
                                                            closed = true;
                                                        }
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    return FilterResult::Intercept(());
                                }
                                Keysym::d => {
                                    data.launcher.toggle(&data.cfg.terminal.command);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::comma => {
                                    // Super + , → 打开 Settings Panel
                                    if !data.settings.is_active() {
                                        data.settings.open(&data.cfg);
                                    } else {
                                        data.settings.close();
                                    }
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::f => {
                                    data.toggle_fullscreen();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::p => {
                                    // Super+Shift+P: 全屏截图直接保存+剪贴板
                                    // Super+P: 区域选择截图
                                    if mods.shift {
                                        data.pending_screenshot =
                                            Some(screenshot::ScreenshotRequest::Full);
                                        data.dirty = true;
                                    } else {
                                        // 进入区域选择模式
                                        data.screenshot.begin_selection();
                                        data.notify("Select area (drag to select, Esc to cancel)");
                                        data.dirty = true;
                                    }
                                    return FilterResult::Intercept(());
                                }
                                Keysym::r => {
                                    // Super+R: 开始/停止屏幕录制
                                    if data.record_state.recording {
                                        data.record_state.stop();
                                        data.notify("Recording stopped");
                                    } else {
                                        data.record_state
                                            .start(data.osize.w as u32, data.osize.h as u32);
                                        if data.record_state.recording {
                                            data.notify("Recording started (Super+R to stop)");
                                        }
                                    }
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::grave => {
                                    // Scratchpad: 切换下拉终端
                                    let msg = data
                                        .scratchpad
                                        .toggle(&data.cfg.terminal.command, data.xdisplay);
                                    data.notify(msg);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::space => {
                                    let ws = &mut data.workspaces[data.active_ws];
                                    ws.layout = ws.layout.next();
                                    let name = ws.layout.name();
                                    data.notify(format!("Layout: {}", name));
                                    data.do_layout_animated();
                                    return FilterResult::Intercept(());
                                }
                                // Super+A: Mission Control (Expose) — 全局视图
                                Keysym::a => {
                                    if data.overview.is_expose() {
                                        data.overview.close();
                                    } else {
                                        let total: usize = (0..NUM_WORKSPACES)
                                            .map(|i| {
                                                data.workspaces[i].tops.len()
                                                    + data.workspaces[i].x11_surfaces.len()
                                            })
                                            .sum();
                                        if total > 0 {
                                            data.overview.open_expose(total, 0);
                                        }
                                    }
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                // Super+Tab: 切换任务面板（Task Panel）
                                Keysym::Tab => {
                                    if data.overview.is_task_panel() {
                                        data.overview.close();
                                    } else {
                                        data.overview.open_task_panel(data.active_ws);
                                    }
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                // Super+1-9：切换工作区
                                Keysym::_1 => {
                                    data.switch_workspace(0);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_2 => {
                                    data.switch_workspace(1);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_3 => {
                                    data.switch_workspace(2);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_4 => {
                                    data.switch_workspace(3);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_5 => {
                                    data.switch_workspace(4);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_6 => {
                                    data.switch_workspace(5);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_7 => {
                                    data.switch_workspace(6);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_8 => {
                                    data.switch_workspace(7);
                                    return FilterResult::Intercept(());
                                }
                                Keysym::_9 => {
                                    data.switch_workspace(8);
                                    return FilterResult::Intercept(());
                                }
                                // Super+方向键 / Super+Shift+方向键
                                Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => {
                                    if mods.shift {
                                        data.swap_window(keysym.modified_sym());
                                    } else if mods.ctrl {
                                        // Super+Ctrl+Left/Right: 切换到相邻工作区（无限滚动）
                                        match keysym.modified_sym() {
                                            Keysym::Left => {
                                                data.switch_workspace_direction(-1);
                                                return FilterResult::Intercept(());
                                            }
                                            Keysym::Right => {
                                                data.switch_workspace_direction(1);
                                                return FilterResult::Intercept(());
                                            }
                                            _ => {
                                                data.focus_direction(keysym.modified_sym());
                                            }
                                        }
                                    } else {
                                        data.focus_direction(keysym.modified_sym());
                                    }
                                    return FilterResult::Intercept(());
                                }
                                // Super+V: 下一个新窗口纵向分割（类似 sway split v）
                                Keysym::v => {
                                    data.workspaces[data.active_ws].pending_split =
                                        Some(layout::SplitDir::Vertical);
                                    info!("📐 下一个窗口 → 纵向 (Vertical)");
                                    data.notify("Next split: Vertical ↕");
                                    return FilterResult::Intercept(());
                                }
                                // Super+B: 下一个新窗口横向分割（类似 sway split h）
                                Keysym::b => {
                                    data.workspaces[data.active_ws].pending_split =
                                        Some(layout::SplitDir::Horizontal);
                                    info!("📐 下一个窗口 → 横向 (Horizontal)");
                                    data.notify("Next split: Horizontal ↔");
                                    return FilterResult::Intercept(());
                                }
                                _ => {}
                            }
                            // Super+Shift+1-9：移动窗口到工作区（用 raw_syms 避免 Shift 修饰键影响匹配）
                            if mods.shift {
                                if let Some(raw) = keysym.raw_syms().first() {
                                    match *raw {
                                        Keysym::_1 => {
                                            data.move_window_to_workspace(0);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_2 => {
                                            data.move_window_to_workspace(1);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_3 => {
                                            data.move_window_to_workspace(2);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_4 => {
                                            data.move_window_to_workspace(3);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_5 => {
                                            data.move_window_to_workspace(4);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_6 => {
                                            data.move_window_to_workspace(5);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_7 => {
                                            data.move_window_to_workspace(6);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_8 => {
                                            data.move_window_to_workspace(7);
                                            return FilterResult::Intercept(());
                                        }
                                        Keysym::_9 => {
                                            data.move_window_to_workspace(8);
                                            return FilterResult::Intercept(());
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
            // ── 触摸板手势处理 ──
            InputEvent::GestureSwipeBegin { event } => {
                use smithay::backend::input::GestureBeginEvent as _;
                self.gesture_dx = 0.0;
                self.gesture_dy = 0.0;
                self.gesture_fingers = event.fingers();
                self.gesture_active = true;
            }
            InputEvent::GestureSwipeUpdate { event } => {
                use smithay::backend::input::GestureSwipeUpdateEvent as _;
                if !self.gesture_active {
                    return;
                }
                self.gesture_dx += event.delta_x();
                self.gesture_dy += event.delta_y();
                // 3指水平滑动 → 连续滚动
                if self.gesture_fingers == 3 {
                    let delta_normalized = event.delta_x() / {
                        let (_ox, _oy, ow, _oh) = self
                            .output_sizes
                            .get(self.focused_output)
                            .copied()
                            .unwrap_or((0, 0, self.osize.w, self.osize.h));
                        ow as f64
                    };
                    self.scroll_offset += delta_normalized;
                    self.scroll_spring.set_target(self.scroll_offset);
                    self.dirty = true;
                }
            }
            InputEvent::GestureSwipeEnd { event } => {
                use smithay::backend::input::GestureEndEvent as _;
                if !event.cancelled() && self.gesture_fingers == 3 {
                    // 同步 spring.x 到当前位置，然后弹簧吸附到最近工作区
                    self.scroll_spring.x = self.scroll_offset;
                    self.scroll_spring.set_target(self.scroll_offset.round());
                }
                self.gesture_active = false;
                self.gesture_dx = 0.0;
                self.gesture_dy = 0.0;
            }
            InputEvent::PointerMotion { event } => {
                // 捕获相对运动数据（relative_pointer 协议用，游戏视角控制）
                let rel_delta = event.delta();
                let rel_delta_unaccel = event.delta_unaccel();
                let utime = event.time();

                // 检查当前焦点 surface 是否有活跃的指针锁定约束（游戏 pointer lock）
                let focus_now = self.pointer_focus();
                let mut ptr_lock = self.pointer.clone();
                let is_locked = focus_now.as_ref().map_or(false, |(surface, _)| {
                    with_pointer_constraint(surface, &ptr_lock, |constraint| {
                        constraint.map_or(false, |c| {
                            matches!(&*c, PointerConstraint::Locked(_)) && c.is_active()
                        })
                    })
                });

                if is_locked {
                    // 指针锁定模式（游戏）：不移动可见光标，只发送相对运动给客户端。
                    // 游戏通过 zwp_relative_pointer 的 delta 来旋转人物视角。
                    ptr_lock.relative_motion(
                        self,
                        focus_now,
                        &RelativeMotionEvent {
                            delta: rel_delta,
                            delta_unaccel: rel_delta_unaccel,
                            utime,
                        },
                    );
                    ptr_lock.frame(self);
                    self.dirty = true;
                    return;
                }

                // 非锁定模式：若光标被游戏隐藏则恢复
                if matches!(self.cursor_status, CursorImageStatus::Hidden) {
                    self.cursor_status = CursorImageStatus::Named(CursorIcon::Default);
                }

                // ── 正常路径：更新光标位置 ──
                self.pointer_pos.0 += event.delta_x();
                self.pointer_pos.1 += event.delta_y();

                // 多显示器鼠标边界处理：
                // 鼠标可能停留在某个 output 内，也可能在联合空间外（罕见但可能）。
                // 关键原则：
                // 1. 任何 output 内部都不应被 clamp 越界
                // 2. 出界时按方向投影到邻接屏幕（保持鼠标运动方向连续性）
                // 3. 若无邻接屏幕，才用屏幕中心距离的最近屏 clamp（兜底）
                if !self.output_sizes.is_empty() {
                    let px = self.pointer_pos.0 as i32;
                    let py = self.pointer_pos.1 as i32;
                    let in_any = self.output_sizes.iter().any(|(ox, oy, ow, oh)| {
                        px >= *ox && px < ox + ow && py >= *oy && py < oy + oh
                    });
                    if !in_any {
                        // 1) 方向投影：找与鼠标位置相邻的 output
                        //    鼠标在某个 output 的 X 方向之外：找 X 方向上最接近的 output
                        //    优先尝试严格 X 方向（左右跨屏），其次 Y 方向（上下跨屏）
                        let mut new_x = self.pointer_pos.0;
                        let mut new_y = self.pointer_pos.1;
                        // 找出鼠标上方 / 下方 / 左方 / 右方最近的邻接 output
                        // 屏幕布局约定：output 可能是同 Y 范围（左右排），同 X 范围（上下排），或两者都不同
                        // 简化策略：选一个能完整包含鼠标 x 或 y 范围的"水平或垂直邻接屏"
                        let target = self.output_sizes.iter().min_by(
                            |(ox1, oy1, ow1, oh1), (ox2, oy2, ow2, oh2)| {
                                // 距离排序：欧氏距离到 output 中心
                                let c1x = *ox1 as f64 + *ow1 as f64 / 2.0;
                                let c1y = *oy1 as f64 + *oh1 as f64 / 2.0;
                                let c2x = *ox2 as f64 + *ow2 as f64 / 2.0;
                                let c2y = *oy2 as f64 + *oh2 as f64 / 2.0;
                                let d1 = (c1x - self.pointer_pos.0).powi(2)
                                    + (c1y - self.pointer_pos.1).powi(2);
                                let d2 = (c2x - self.pointer_pos.0).powi(2)
                                    + (c2y - self.pointer_pos.1).powi(2);
                                d1.partial_cmp(&d2).unwrap_or(std::cmp::Ordering::Equal)
                            },
                        );
                        if let Some((ox, oy, ow, oh)) = target {
                            // 关键修复：clamp 到 (ox, ox+ow) 而非 (ox, ox+ow-1)
                            // 否则永远到不了真正的右/下边界
                            new_x = self.pointer_pos.0.clamp(*ox as f64, (*ox + *ow) as f64);
                            new_y = self.pointer_pos.1.clamp(*oy as f64, (*oy + *oh) as f64);
                        }
                        self.pointer_pos.0 = new_x;
                        self.pointer_pos.1 = new_y;
                    } else {
                        // 鼠标在某个 output 内：clamp 到该 output 内部（防止指针移到联合空间边界外）
                        // 找到当前所在的 output
                        let current = self.output_sizes.iter().find(|(ox, oy, ow, oh)| {
                            px >= *ox && px < ox + ow && py >= *oy && py < oy + oh
                        });
                        if let Some((ox, oy, ow, oh)) = current {
                            // clamp 允许鼠标到达真实边界
                            self.pointer_pos.0 =
                                self.pointer_pos.0.clamp(*ox as f64, (*ox + *ow) as f64);
                            self.pointer_pos.1 =
                                self.pointer_pos.1.clamp(*oy as f64, (*oy + *oh) as f64);
                        }
                    }
                }

                // 同步 focused_output：鼠标移动到另一个 output 时更新
                let new_focused = self.output_at_pointer();
                if new_focused != self.focused_output {
                    self.focused_output = new_focused;
                    crate::appctl::emit_output_focused_changed(self);
                    // 全局 active_ws 跟踪当前鼠标所在 output 的工作区
                    self.active_ws = self.output_active_ws.get(new_focused).copied().unwrap_or(0);
                    // 关键：同步 scroll_offset 到目标 output 的值，终止弹簧动画
                    // 否则 scroll_offset 还在旧 output 的中间值（如 2.3），
                    // 新 output 的 ws_offset = (new_ws - 2.3) * screen_w → 窗口飞到屏幕外
                    self.scroll_offset = self
                        .scroll_offsets
                        .get(new_focused)
                        .copied()
                        .unwrap_or(self.active_ws as f64);
                    self.scroll_spring.set(self.scroll_offset);
                    self.scroll_spring.set_target(self.scroll_offset);
                    self.scroll_momentum.reset();
                    // 同步 prev_positions 到新 active_ws
                    // 否则 prev_positions 还是旧 output 的窗口数据，
                    // 新建窗口时 do_layout_animated 会把已有窗口误判为新窗口 → 从屏幕外飞入
                    self.layout_workspace(self.active_ws);
                    self.dirty = true;
                }

                // 截图区域选择模式：更新选择终点
                if self.screenshot.selecting {
                    self.screenshot
                        .on_motion(self.pointer_pos.0, self.pointer_pos.1);
                    self.dirty = true;
                    return;
                }

                // 锁屏模式：不转发鼠标事件给客户端，仅更新光标位置
                if self.lock_state.locked {
                    self.dirty = true;
                    return;
                }

                // 转发给客户端
                let serial = SERIAL_COUNTER.next_serial();
                let time = (event.time() / 1000) as u32;
                let focus = self.pointer_focus();

                // 检查指针焦点 surface 是否变化——如果变化，强制重置光标为 Default。
                // X11 应用在窗口边缘设置的 resize 光标可能残留，需要在离开时清除。
                let focus_surface = focus.as_ref().map(|(s, _)| s.clone());
                if focus_surface.as_ref() != self.pointer_focus_surface.as_ref() {
                    self.pointer_focus_surface = focus_surface;
                    // 焦点切换 → 重置光标，新焦点 client 会通过 set_cursor 设置正确光标
                    self.cursor_status = CursorImageStatus::Named(CursorIcon::Default);
                }

                let ptr = self.pointer.clone();
                // 转为 output 局部坐标（pointer_focus 返回的 offset 也是 output 局部的）
                let oi = self.output_at_pointer();
                let (ox, oy, _, _) = self.output_sizes.get(oi).copied().unwrap_or_default();
                ptr.motion(
                    self,
                    focus.clone(),
                    &MotionEvent {
                        location: Point::from((
                            self.pointer_pos.0 - ox as f64,
                            self.pointer_pos.1 - oy as f64,
                        )),
                        serial,
                        time,
                    },
                );
                // 同时发送相对运动（对不使用 relative_pointer 的客户端是 no-op）
                ptr.relative_motion(
                    self,
                    focus,
                    &RelativeMotionEvent {
                        delta: rel_delta,
                        delta_unaccel: rel_delta_unaccel,
                        utime,
                    },
                );
                ptr.frame(self);

                self.dirty = true;
            }
            InputEvent::PointerButton { event } => {
                // 锁屏模式：阻止鼠标按钮
                if self.lock_state.locked {
                    return;
                }
                // ── Settings Panel 模式：拦截所有鼠标事件 ──
                if self.settings.is_active() {
                    return;
                }
                // ── Overview / Task Panel 模式：拦截所有鼠标事件 ──
                if self.overview.is_active() {
                    if event.state() == ButtonState::Released {
                        let px = self.pointer_pos.0 as i32;
                        let py = self.pointer_pos.1 as i32;
                        let (ox, oy, ow, oh) = self
                            .output_sizes
                            .get(self.focused_output)
                            .copied()
                            .unwrap_or_default();
                        let bar_h = if self.cfg.bar.enabled {
                            self.cfg.bar.height as i32
                        } else {
                            0
                        };

                        if self.overview.is_task_panel() {
                            // Task Panel：用真实 slot 缩放布局计算点击区域
                            let panel_h = (oh as f32 * 0.35) as i32;
                            let thumb_scale = (panel_h as f32 - 40.0) / oh as f32;
                            let thumb_ow = (ow as f32 * thumb_scale) as i32;
                            let thumb_ox = (ow - thumb_ow) / 2;
                            let panel_y = oh - panel_h;

                            // 收集点击区域（释放 ws 借用后再操作）
                            let hit_slots: Vec<(WindowSlot, i32, i32, i32, i32)> = {
                                let ws = &self.workspaces[self.active_ws];
                                let order = ws.effective_order();
                                let n = order.len();
                                order
                                    .iter()
                                    .enumerate()
                                    .map(|(i, slot)| {
                                        let (sx, sy, sw, sh) = layout::slot(
                                            i, n, ow, oh, bar_h, &self.cfg, ws.layout, ws.split,
                                        );
                                        (
                                            slot.clone(),
                                            thumb_ox + (sx as f32 * thumb_scale) as i32,
                                            panel_y + 20 + (sy as f32 * thumb_scale) as i32,
                                            (sw as f32 * thumb_scale) as i32,
                                            (sh as f32 * thumb_scale) as i32,
                                        )
                                    })
                                    .collect()
                            };

                            for (slot, tx, ty, tw, th) in &hit_slots {
                                if px >= *tx && px < *tx + *tw && py >= *ty && py < *ty + *th {
                                    match slot {
                                        WindowSlot::Wl(idx) => {
                                            let surf = {
                                                let ws = &self.workspaces[self.active_ws];
                                                ws.tops.get(*idx).map(|tl| tl.wl_surface().clone())
                                            };
                                            if let Some(surf) = surf {
                                                self.workspaces[self.active_ws].focus =
                                                    Some(surf.clone());
                                                let kbd = self.kbd.clone();
                                                let serial = SERIAL_COUNTER.next_serial();
                                                kbd.set_focus(self, Some(surf.clone()), serial);
                                                let ws = &self.workspaces[self.active_ws];
                                                if let Some(tl) = ws.tops.get(*idx) {
                                                    tl.with_pending_state(|st| {
                                                        st.states
                                                            .set(xdg_toplevel::State::Activated);
                                                    });
                                                    tl.send_configure();
                                                }
                                            }
                                        }
                                        WindowSlot::X11(idx) => {
                                            let wl = {
                                                let ws = &self.workspaces[self.active_ws];
                                                ws.x11_surfaces.get(*idx).and_then(|xs| {
                                                    xs.wl_surface().map(|s| s.clone())
                                                })
                                            };
                                            if let Some(wl) = wl {
                                                self.workspaces[self.active_ws].focus =
                                                    Some(wl.clone());
                                                let kbd = self.kbd.clone();
                                                let serial = SERIAL_COUNTER.next_serial();
                                                kbd.set_focus(self, Some(wl.clone()), serial);
                                            }
                                        }
                                    }
                                    self.overview.close();
                                    self.dirty = true;
                                    return;
                                }
                            }
                            // 点击面板空白区域 → 关闭
                            if py >= panel_y {
                                self.overview.close();
                                self.dirty = true;
                            }
                        }
                        // Expose 模式：点击选窗口（全局视图，通过 expose_thumbs 做命中测试）
                        if self.overview.is_expose() {
                            // 先收集命中信息，再执行操作，避免借用冲突
                            let hit: Option<(usize, WindowSlot)> = self
                                .expose_thumbs
                                .iter()
                                .find(|(tx, ty, tw, th, _, _)| {
                                    px >= *tx && px < *tx + *tw && py >= *ty && py < *ty + *th
                                })
                                .map(|&(_, _, _, _, target_ws, ref slot)| {
                                    (target_ws, slot.clone())
                                });
                            if let Some((target_ws, slot)) = hit {
                                let focus_surf: Option<WlSurface> = match &slot {
                                    WindowSlot::Wl(idx) => self.workspaces[target_ws]
                                        .tops
                                        .get(*idx)
                                        .map(|tl| tl.wl_surface().clone()),
                                    WindowSlot::X11(idx) => self.workspaces[target_ws]
                                        .x11_surfaces
                                        .get(*idx)
                                        .and_then(|xs| xs.wl_surface()),
                                };
                                if let Some(surf) = focus_surf {
                                    self.workspaces[target_ws].focus = Some(surf.clone());
                                    let kbd = self.kbd.clone();
                                    let serial = SERIAL_COUNTER.next_serial();
                                    kbd.set_focus(self, Some(surf), serial);
                                }
                                // 设置 Activated state（Wl only）
                                if let WindowSlot::Wl(idx) = &slot {
                                    let ws = &self.workspaces[target_ws];
                                    if let Some(tl) = ws.tops.get(*idx) {
                                        tl.with_pending_state(|st| {
                                            st.states.set(xdg_toplevel::State::Activated);
                                        });
                                        tl.send_configure();
                                    }
                                }
                                self.overview.close();
                                if target_ws != self.active_ws {
                                    self.switch_workspace(target_ws);
                                }
                                self.dirty = true;
                                return;
                            }
                            // 点击空白区域关闭
                            self.overview.close();
                            self.dirty = true;
                            return;
                        }
                    }
                    return; // ← 无论什么鼠标事件都拦截，不穿透到下层窗口
                }
                // 截图/录制区域选择模式：按下记录起点，释放完成
                if self.screenshot.selecting {
                    if event.state() == ButtonState::Pressed {
                        self.screenshot
                            .on_press(self.pointer_pos.0, self.pointer_pos.1);
                        self.dirty = true;
                    } else if event.state() == ButtonState::Released {
                        if let Some((x, y, w, h)) = self.screenshot.on_release() {
                            if self.record_state.selecting {
                                // 区域录制：启动录制
                                self.record_state.selecting = false;
                                let area = record::RecordArea {
                                    x: x as u32,
                                    y: y as u32,
                                    w: w as u32,
                                    h: h as u32,
                                };
                                self.record_state.start_with_area(
                                    self.osize.w as u32,
                                    self.osize.h as u32,
                                    Some(area),
                                );
                                if self.record_state.recording {
                                    self.notify(format!(
                                        "Recording {}x{} area (Super+R to stop)",
                                        w, h
                                    ));
                                }
                            } else {
                                // 区域截图
                                self.pending_screenshot =
                                    Some(screenshot::ScreenshotRequest::Area(x, y, w, h));
                            }
                            self.dirty = true;
                        } else {
                            self.record_state.selecting = false;
                            self.notify("Selection too small, cancelled");
                        }
                        self.dirty = true;
                    }
                    return; // 选区模式中拦截所有鼠标点击
                }

                // 点击聚焦（仅 Press 时）
                if event.state() == ButtonState::Pressed {
                    let oi = self.output_at_pointer();
                    let (ox, oy, ow, oh) = self.output_sizes.get(oi).copied().unwrap_or((
                        0,
                        0,
                        self.osize.w,
                        self.osize.h,
                    ));
                    let px = self.pointer_pos.0 as i32 - ox;
                    let py = self.pointer_pos.1 as i32 - oy;
                    let bar_h = if self.cfg.bar.enabled {
                        self.cfg.bar.height
                    } else {
                        0
                    };

                    // 使用鼠标所在 output 的工作区
                    let ws_idx = self
                        .output_active_ws
                        .get(oi)
                        .copied()
                        .unwrap_or(self.active_ws);
                    self.active_ws = ws_idx;
                    let ws = &self.workspaces[ws_idx];

                    if py >= bar_h {
                        // 全屏模式下：点击确保全屏窗口有焦点
                        if ws.fullscreen.is_some() {
                            let order = ws.effective_order();
                            if let Some(fi) = ws.fullscreen {
                                if let Some(slot) = order.get(fi) {
                                    let surf = match slot {
                                        WindowSlot::Wl(idx) => {
                                            ws.tops.get(*idx).map(|tl| tl.wl_surface().clone())
                                        }
                                        WindowSlot::X11(idx) => {
                                            ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface())
                                        }
                                    };
                                    if let Some(surf) = surf {
                                        self.workspaces[ws_idx].focus = Some(surf.clone());
                                        let kbd = self.kbd.clone();
                                        let serial = SERIAL_COUNTER.next_serial();
                                        kbd.set_focus(self, Some(surf), serial);
                                    }
                                }
                            }
                        } else {
                            let order = ws.effective_order();

                            // Check if click is on an XDG popup — if so, don't steal focus
                            let on_popup = {
                                let mut found = false;
                                for (i, slot) in order.iter().enumerate() {
                                    if let WindowSlot::Wl(idx) = slot {
                                        if let Some(tl) = ws.tops.get(*idx) {
                                            let (x, y, _, _) = layout::slot(
                                                i,
                                                order.len(),
                                                ow,
                                                oh,
                                                bar_h,
                                                &self.cfg,
                                                ws.layout,
                                                ws.split,
                                            );
                                            let tl_pos = Point::from((x as f64, y as f64));
                                            if self.popup_at_pointer(tl, tl_pos).is_some() {
                                                found = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                                found
                            };

                            if !on_popup {
                                let n_all = order.len();
                                for (i, slot) in order.iter().enumerate() {
                                    let (x, y, w, h) = layout::slot(
                                        i, n_all, ow, oh, bar_h, &self.cfg, ws.layout, ws.split,
                                    );
                                    if px >= x && px < x + w && py >= y && py < y + h {
                                        let surf = match slot {
                                            WindowSlot::Wl(idx) => {
                                                ws.tops.get(*idx).map(|tl| tl.wl_surface().clone())
                                            }
                                            WindowSlot::X11(idx) => ws
                                                .x11_surfaces
                                                .get(*idx)
                                                .and_then(|xs| xs.wl_surface()),
                                        };
                                        if let Some(surf) = surf {
                                            self.workspaces[self.active_ws].focus =
                                                Some(surf.clone());
                                            let kbd = self.kbd.clone();
                                            let serial = SERIAL_COUNTER.next_serial();
                                            kbd.set_focus(self, Some(surf), serial);
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                // 点击浮动 OR 窗口（Dialog/菜单等）时设置键盘焦点
                if event.state() == ButtonState::Pressed {
                    let btn_focus = self.pointer_focus();
                    if let Some((surf, _)) = &btn_focus {
                        let kbd_focus = self.kbd.clone();
                        let current_focus = self.workspaces[self.active_ws].focus.as_ref();
                        if current_focus != Some(surf) {
                            // 检查是否是 OR surface
                            let is_or = self
                                .xw
                                .or_surfaces
                                .iter()
                                .any(|xs| xs.wl_surface().as_ref() == Some(surf));
                            if is_or {
                                let focus_serial = SERIAL_COUNTER.next_serial();
                                kbd_focus.set_focus(self, Some(surf.clone()), focus_serial);
                            }
                        }
                    }
                }

                // 转发按钮事件给客户端
                let serial = SERIAL_COUNTER.next_serial();
                let time = (event.time() / 1000) as u32;
                let ptr = self.pointer.clone();
                ptr.button(
                    self,
                    &smithay::input::pointer::ButtonEvent {
                        serial,
                        time,
                        button: event.button_code(),
                        state: event.state(),
                    },
                );
                ptr.frame(self);
            }
            InputEvent::PointerAxis { event } => {
                // 锁屏模式：阻止滚轮
                if self.lock_state.locked {
                    return;
                }
                // ── Settings Panel 模式：拦截滚轮 ──
                if self.settings.is_active() {
                    return;
                }
                let time = (event.time() / 1000) as u32;
                let mut frame = AxisFrame::new(time).source(event.source());
                if let Some(v) = event.amount(Axis::Vertical) {
                    frame = frame.value(Axis::Vertical, v);
                }
                if let Some(v) = event.amount(Axis::Horizontal) {
                    frame = frame.value(Axis::Horizontal, v);
                }
                if let Some(v) = event.amount_v120(Axis::Vertical) {
                    frame = frame.v120(Axis::Vertical, v as i32);
                }
                if let Some(v) = event.amount_v120(Axis::Horizontal) {
                    frame = frame.v120(Axis::Horizontal, v as i32);
                }
                let ptr = self.pointer.clone();
                ptr.axis(self, frame);
                ptr.frame(self);
            }
            _ => {}
        }
    }
}

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg
    }
    fn new_toplevel(&mut self, s: ToplevelSurface) {
        // 拦截 scratchpad 窗口
        if self.scratchpad.intercept_toplevel(s.clone()) {
            // 配置为浮动覆盖层：居中、上方 1/3 高度
            let ow = self.osize.w;
            let oh = self.osize.h;
            let bar_h = if self.cfg.bar.enabled {
                self.cfg.bar.height
            } else {
                0
            };
            let sp_w = ow * 3 / 4; // 75% 宽度
            let sp_h = oh / 3; // 1/3 高度
            let sp_x = (ow - sp_w) / 2;
            let sp_y = bar_h + 8;
            s.with_pending_state(|st| {
                st.size = Some((sp_w, sp_h).into());
                st.states.set(xdg_toplevel::State::Activated);
            });
            s.send_configure();
            info!(
                "🚀 Scratchpad 浮动窗口: {}x{} at ({},{})",
                sp_w, sp_h, sp_x, sp_y
            );
            // 聚焦
            let surf = s.wl_surface().clone();
            let kbd = self.kbd.clone();
            let serial = SERIAL_COUNTER.next_serial();
            kbd.set_focus(self, Some(surf), serial);
            self.dirty = true;
            return;
        }

        self.pending_tops.push(s);
        // 标记打开动画（pending 确认后实际加入 ws 时再触发）
        self.window_anims
            .push((self.active_ws, std::time::Instant::now(), true));
        self.dirty = true;
    }
    fn new_popup(&mut self, popup: PopupSurface, _positioner: PositionerState) {
        info!("🆕 new_popup created");
        let _ = popup.send_configure();
        if let Err(e) = self.popup_manager.track_popup(PopupKind::Xdg(popup)) {
            warn!("⚠️  track_popup: {:?}", e);
        }
    }
    fn grab(
        &mut self,
        popup: PopupSurface,
        _seat: wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
        info!("🆕 grab popup created");
        let _ = popup.send_configure();
        if let Err(e) = self.popup_manager.track_popup(PopupKind::Xdg(popup)) {
            warn!("⚠️  track_popup (grab): {:?}", e);
        }
        self.dirty = true;
    }
    fn reposition_request(
        &mut self,
        popup: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        let _ = popup.send_configure();
    }
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wayland_server::protocol::wl_output::WlOutput>,
    ) {
        // Client (e.g. browser video) requests fullscreen
        let wl_surf = surface.wl_surface().clone();
        for (ws_idx, ws) in self.workspaces.iter_mut().enumerate() {
            let order = ws.effective_order();
            for (i, slot) in order.iter().enumerate() {
                let matched = match slot {
                    WindowSlot::Wl(idx) => ws
                        .tops
                        .get(*idx)
                        .map(|tl| tl.wl_surface() == &wl_surf)
                        .unwrap_or(false),
                    WindowSlot::X11(_) => false,
                };
                if matched {
                    info!("🔳 客户端请求全屏 #{} (工作区 {})", i, ws_idx + 1);
                    ws.fullscreen = Some(i);
                    ws.focus = Some(wl_surf.clone());
                    let kbd = self.kbd.clone();
                    let serial = SERIAL_COUNTER.next_serial();
                    kbd.set_focus(self, Some(wl_surf.clone()), serial);
                    self.do_layout_animated();
                    self.dirty = true;
                    return;
                }
            }
        }
        // Surface not in any workspace — just acknowledge
        surface.send_configure();
    }
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let wl_surf = surface.wl_surface().clone();
        for (ws_idx, ws) in self.workspaces.iter_mut().enumerate() {
            if ws.fullscreen.is_some() {
                for (i, tl) in ws.tops.iter().enumerate() {
                    if tl.wl_surface() == &wl_surf {
                        info!("🔳 客户端取消全屏 #{} (工作区 {})", i, ws_idx + 1);
                        ws.fullscreen = None;
                        self.do_layout_animated();
                        self.dirty = true;
                        return;
                    }
                }
            }
        }
        surface.send_configure();
    }
    /// XDG toplevel 被销毁 — 比 CompositorHandler::destroyed 更可靠
    /// （浏览器等复杂应用内部 wl_surface 可能和 tops 中存储的不一致）
    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl = surface.wl_surface().clone();

        // 检查是否是 scratchpad surface 被销毁
        let is_scratchpad = self
            .scratchpad
            .surface
            .as_ref()
            .map(|s| s.wl_surface() == &wl)
            .unwrap_or(false);
        if is_scratchpad {
            info!("🗑️ Scratchpad surface destroyed");
            self.scratchpad.surface = None;
            self.scratchpad.visible = false;
            // 尝试 kill 子进程（可能已经退出）
            if let Some(ref mut child) = self.scratchpad.process {
                let _ = child.kill();
            }
            self.scratchpad.process = None;
            self.dirty = true;
            // scratchpad surface 不在工作区 tops 中，直接返回
            return;
        }

        // 搜索所有工作区
        for ws_idx in 0..self.workspaces.len() {
            let closed_idx = self.workspaces[ws_idx]
                .tops
                .iter()
                .position(|tl| tl.wl_surface() == &wl);
            if let Some(idx) = closed_idx {
                info!("🗑️ toplevel_destroyed (ws={}, idx={})", ws_idx, idx);
                self.workspaces[ws_idx]
                    .tops
                    .retain(|tl| tl.wl_surface() != &wl);
                self.remap_prev_after_remove(&WindowSlot::Wl(idx));
                // 关闭动画
                self.window_anims
                    .push((ws_idx, std::time::Instant::now(), false));
                self.dirty = true;
                // fullscreen 清理
                if let Some(fi) = self.workspaces[ws_idx].fullscreen {
                    let order = self.workspaces[ws_idx].effective_order();
                    match order.get(fi) {
                        Some(WindowSlot::Wl(i)) if *i == idx => {
                            self.workspaces[ws_idx].fullscreen = None;
                        }
                        _ => {}
                    }
                }
                self.workspaces[ws_idx].rebuild_order();
                // focus 更新
                if self.workspaces[ws_idx].focus.as_ref() == Some(&wl) {
                    let order = self.workspaces[ws_idx].effective_order();
                    self.workspaces[ws_idx].focus = order.last().and_then(|s| match s {
                        WindowSlot::Wl(i) => self.workspaces[ws_idx]
                            .tops
                            .get(*i)
                            .map(|tl| tl.wl_surface().clone()),
                        WindowSlot::X11(i) => self.workspaces[ws_idx]
                            .x11_surfaces
                            .get(*i)
                            .and_then(|xs| xs.wl_surface()),
                    });
                }
                // relayout
                if ws_idx != self.active_ws {
                    self.layout_workspace(ws_idx);
                } else {
                    self.do_layout_animated();
                    self.dirty = true;
                    if let Some(ref s) = self.workspaces[self.active_ws].focus {
                        let kbd = self.kbd.clone();
                        let serial = SERIAL_COUNTER.next_serial();
                        kbd.set_focus(self, Some(s.clone()), serial);
                    }
                }
                return;
            }
        }
        // 也检查 pending_tops
        self.pending_tops.retain(|tl| tl.wl_surface() != &wl);
    }
    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        use smithay::wayland::compositor::with_states;
        use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
        let app_id = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|d| d.app_id.clone())
                .unwrap_or_default()
        });
        info!("🆔 app_id_changed: '{}'", app_id);

        let wl_surf = surface.wl_surface().clone();

        // Check if this is a pending toplevel awaiting confirmation.
        // Non-empty app_id → real window, promote to tiling layout.
        // Empty app_id → tooltip/transient (e.g. Chromium hover), ignore.
        if let Some(pos) = self
            .pending_tops
            .iter()
            .position(|t| t.wl_surface() == &wl_surf)
        {
            self.pending_tops.remove(pos);
            // Filter clipboard helper windows — they create invisible toplevels to own selections
            let is_clipboard = app_id.contains("clipboard")
                || app_id.contains("wl-copy")
                || app_id.contains("wl-paste");
            if !app_id.is_empty() && !is_clipboard {
                info!("✅ pending → tiling (app_id='{}')", app_id);
                // 全屏时打开新窗口 → 立刻退出全屏，让用户看到新窗口
                if self.workspaces[self.active_ws].fullscreen.is_some() {
                    self.workspaces[self.active_ws].fullscreen = None;
                    info!("🪟 全屏退出（本桌面新窗口打开）");
                }
                self.workspaces[self.active_ws].tops.push(surface.clone());
                crate::appctl::emit_event(
                    self,
                    crate::appctl::DesktopEvent::WindowOpened {
                        workspace: self.active_ws,
                        title: self.title_for_surface(surface.wl_surface()),
                        app_id: self.app_id_for_surface(surface.wl_surface()),
                        kind: "wayland".into(),
                    },
                );
                let new_idx = self.workspaces[self.active_ws].tops.len() - 1;
                // 智能插入：根据 split 方向，将新窗口放在焦点窗口旁边
                self.workspaces[self.active_ws].insert_next_to_focus(WindowSlot::Wl(new_idx));
                if let Some(new_split) = self.workspaces[self.active_ws].pending_split.take() {
                    self.workspaces[self.active_ws].split = new_split;
                }
                info!("➕ 窗口 #{} (工作区 {})", new_idx, self.active_ws + 1);
                self.do_layout_animated();
                if let Some(tl) = self.workspaces[self.active_ws].tops.get(new_idx) {
                    let s = tl.wl_surface().clone();
                    self.workspaces[self.active_ws].focus = Some(s.clone());
                    let kbd = self.kbd.clone();
                    let serial = SERIAL_COUNTER.next_serial();
                    kbd.set_focus(self, Some(s), serial);
                }
                self.dirty = true;
            } else {
                info!("🗑️ pending → discarded (app_id='{}')", app_id);
            }
            return;
        }

        // 查找该 surface 在哪个工作区
        let mut found: Option<(usize, usize)> = None;
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for (idx, top) in ws.tops.iter().enumerate() {
                if top.wl_surface() == &wl_surf {
                    found = Some((ws_idx, idx));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        if let Some((ws_idx, idx)) = found {
            self.window_app_ids.insert(idx, app_id.clone());

            // 匹配窗口规则
            for rule in &self.cfg.window_rules {
                if !rule.app_id.is_empty() && app_id.contains(&rule.app_id) {
                    let target_ws = rule.workspace.min(self.workspaces.len() - 1);
                    if target_ws != ws_idx {
                        info!("📐 窗口规则: '{}' → 工作区 {}", app_id, target_ws + 1);
                        if let Some(ref layout_name) = rule.layout {
                            if let Some(l) = crate::layout::LayoutPreset::from_name(layout_name) {
                                self.workspaces[target_ws].layout = l;
                            }
                        }
                        // 移动窗口到目标工作区
                        if self.workspaces[ws_idx].tops.len() > idx {
                            let top = self.workspaces[ws_idx].tops.remove(idx);
                            self.remap_prev_after_remove(&WindowSlot::Wl(idx));
                            self.workspaces[ws_idx].rebuild_order();
                            // 如果目标工作区在全屏，退出全屏让用户看到新窗口
                            if self.workspaces[target_ws].fullscreen.is_some() {
                                self.workspaces[target_ws].fullscreen = None;
                                info!(
                                    "🪟 全屏退出（窗口规则: '{}' → 工作区 {}）",
                                    app_id,
                                    target_ws + 1
                                );
                            }
                            self.workspaces[target_ws].tops.push(top);
                            self.workspaces[target_ws].rebuild_order();
                            self.switch_workspace(target_ws);
                            self.do_layout_animated();
                        }
                    }
                    break;
                }
            }
        }
    }
    fn title_changed(&mut self, surface: ToplevelSurface) {
        use smithay::wayland::compositor::with_states;
        use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
        let title = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|d| d.title.clone())
                .unwrap_or_default()
        });
        let wl_surf = surface.wl_surface().clone();
        for ws in &self.workspaces {
            for (idx, top) in ws.tops.iter().enumerate() {
                if top.wl_surface() == &wl_surf {
                    self.window_titles.insert(idx, title.clone());
                    return;
                }
            }
        }
    }
}

impl SelectionHandler for App {
    type SelectionUserData = Arc<[u8]>;

    /// Wayland 客户端设了新选区 → 通过 Smithay 内建机制代理到 X11
    /// 调 X11Wm::new_selection() 让 XWM 窗口成为 X11 CLIPBOARD owner，
    /// X11 客户端粘贴时 XwmHandler::send_selection 会把 Wayland 数据写入 fd
    fn new_selection(
        &mut self,
        _ty: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(ref mut xwm) = self.xw.xwm {
            if let Some(src) = source {
                let mime_types = src.mime_types();
                if !mime_types.is_empty() {
                    if let Err(e) = xwm.new_selection(
                        smithay::wayland::selection::SelectionTarget::Clipboard,
                        Some(mime_types),
                    ) {
                        tracing::warn!("Wayland→X11: X11Wm::new_selection failed: {:?}", e);
                    } else {
                        tracing::info!("Wayland→X11: XWM is now X11 clipboard owner");
                    }
                }
            } else {
                // 选区被清空 → 清除 X11 选区
                if let Err(e) = xwm.new_selection(
                    smithay::wayland::selection::SelectionTarget::Clipboard,
                    None,
                ) {
                    tracing::warn!("Wayland→X11: clear selection failed: {:?}", e);
                }
            }
        }
    }

    fn send_selection(
        &mut self,
        ty: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: std::os::unix::io::OwnedFd,
        _seat: Seat<Self>,
        user_data: &Self::SelectionUserData,
    ) {
        tracing::info!(
            "📋 send_selection: ty={:?}, mime={}, data_len={}",
            ty,
            mime_type,
            user_data.len()
        );
        // X11 代理选区标记：user_data 以 "X11_PROXY" 开头（10 bytes magic）
        // 这不可能是正常剪贴板内容
        const X11_PROXY_MAGIC: &[u8] = b"X11_PROXY\x00";
        let is_x11_proxy =
            user_data.starts_with(X11_PROXY_MAGIC) && user_data.len() == X11_PROXY_MAGIC.len();

        if is_x11_proxy {
            // X11→Wayland 方向：Wayland 客户端请求粘贴 X11 的数据
            // 通过 X11Wm::send_selection 从 X11 客户端获取数据直接写入 fd
            if let Some(ref mut xwm) = self.xw.xwm {
                if let Some(ref lh) = self.loop_handle {
                    match xwm.send_selection::<App>(ty, mime_type, fd, lh.clone()) {
                        Ok(()) => tracing::info!("X11→Wayland: send_selection forwarded to X11"),
                        Err(e) => tracing::warn!("X11→Wayland: send_selection failed: {:?}", e),
                    }
                } else {
                    tracing::warn!("X11→Wayland: no loop_handle available");
                }
            } else {
                tracing::warn!("X11→Wayland: no XWM available");
            }
        } else {
            // Wayland 本地选区：直接把 user_data 写入 fd
            let buf = user_data.clone();
            std::thread::spawn(move || {
                use std::io::Write;
                if let Err(err) = smithay::reexports::rustix::fs::fcntl_setfl(
                    &fd,
                    smithay::reexports::rustix::fs::OFlags::empty(),
                ) {
                    tracing::warn!("error clearing flags on selection fd: {:?}", err);
                }
                if let Err(err) = std::fs::File::from(fd).write_all(&buf) {
                    tracing::warn!("error writing selection: {:?}", err);
                }
            });
        }
    }
}
impl DataDeviceHandler for App {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.dd
    }
}
impl PrimarySelectionHandler for App {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_sel
    }
}
impl smithay::wayland::output::OutputHandler for App {}

impl ClientDndGrabHandler for App {
    fn started(
        &mut self,
        _source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        _seat: Seat<Self>,
    ) {
        self.dnd_icon = icon;
        self.dirty = true;
    }

    fn dropped(&mut self, _target: Option<WlSurface>, _validated: bool, _seat: Seat<Self>) {
        self.dnd_icon = None;
        self.dirty = true;
    }
}

impl ServerDndGrabHandler for App {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
    fn dropped(&mut self, _seat: Seat<Self>) {
        self.dnd_icon = None;
        self.dirty = true;
    }
}

// ── Layer Shell Handler ──────────────────────────────────────────
impl WlrLayerShellHandler for App {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        _layer: Layer,
        _namespace: String,
    ) {
        // 给 layer surface 发送初始 configure（全屏尺寸）
        let (ow, oh) = self
            .output_sizes
            .first()
            .map(|(_, _, w, h)| (*w, *h))
            .unwrap_or((1920, 1080));
        surface.with_pending_state(|state| {
            state.size = Some((ow, oh).into());
        });
        let _ = surface.send_configure();
        info!("🪟 layer surface: new (namespace={})", _namespace);
        self.dirty = true;
    }

    fn layer_destroyed(&mut self, _surface: LayerSurface) {
        info!("🪟 layer surface: destroyed");
        self.dirty = true;
    }
}

// ── Fractional Scale Handler ─────────────────────────────────────
impl FractionalScaleHandler for App {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        // 发送当前输出缩放因子给客户端
        // 用 focused output 的 scale（存储在 output_sizes 对应的 AnchorOutput 中）
        let scale: f64 = 1.0; // 默认 1.0，渲染管线中按实际 scale 绘制
        use smithay::wayland::fractional_scale::with_fractional_scale;
        smithay::wayland::compositor::with_states(&surface, |states| {
            with_fractional_scale(states, |fs_state| {
                fs_state.set_preferred_scale(scale);
            });
        });
    }
}

// ── Idle Notifier Handler ────────────────────────────────────────
impl IdleNotifierHandler for App {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_notifier
    }
}

// ── Idle Inhibit Handler ─────────────────────────────────────────
impl IdleInhibitHandler for App {
    fn inhibit(&mut self, _surface: WlSurface) {
        self.idle_inhibit_count += 1;
        self.idle_notifier
            .set_is_inhibited(self.idle_inhibit_count > 0);
        info!("🚫 idle inhibit: count={}", self.idle_inhibit_count);
    }

    fn uninhibit(&mut self, _surface: WlSurface) {
        if self.idle_inhibit_count > 0 {
            self.idle_inhibit_count -= 1;
        }
        self.idle_notifier
            .set_is_inhibited(self.idle_inhibit_count > 0);
        info!("✅ idle uninhibit: count={}", self.idle_inhibit_count);
    }
}

impl InputMethodHandler for App {
    fn new_popup(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: new");
        self.im_popup = Some(surface);
        self.dirty = true;
    }
    fn dismiss_popup(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: dismiss");
        if self
            .im_popup
            .as_ref()
            .map_or(false, |p| p.wl_surface() == surface.wl_surface())
        {
            self.im_popup = None;
        }
        self.dirty = true;
    }
    fn popup_repositioned(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: repositioned");
        self.im_popup = Some(surface);
        self.dirty = true;
    }
    fn parent_geometry(&self, _parent: &WlSurface) -> Rectangle<i32, Logical> {
        Rectangle::default()
    }
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.comp
    }
    fn client_compositor_state<'a>(&self, c: &'a Client) -> &'a CompositorClientState {
        if let Some(cs) = c.get_data::<ClientState>() {
            &cs.comp
        } else if let Some(xw) = c.get_data::<smithay::xwayland::XWaylandClientData>() {
            &xw.compositor_state
        } else {
            static FALLBACK: std::sync::OnceLock<CompositorClientState> =
                std::sync::OnceLock::new();
            FALLBACK.get_or_init(CompositorClientState::default)
        }
    }
    fn commit(&mut self, s: &WlSurface) {
        self.dirty = true;
        on_commit_buffer_handler::<Self>(s);
        self.popup_manager.commit(s);
    }
    fn destroyed(&mut self, surface: &WlSurface) {
        // 搜索所有工作区找到被销毁的窗口
        for ws_idx in 0..self.workspaces.len() {
            let before = self.workspaces[ws_idx].tops.len();
            let closed_idx = self.workspaces[ws_idx]
                .tops
                .iter()
                .position(|tl| tl.wl_surface() == surface);
            self.workspaces[ws_idx]
                .tops
                .retain(|tl| tl.wl_surface() != surface);
            if self.workspaces[ws_idx].tops.len() < before {
                // 重新映射 prev_positions（索引移位修复）
                if let Some(removed_idx) = closed_idx {
                    self.remap_prev_after_remove(&WindowSlot::Wl(removed_idx));
                }
                info!("🗑️ 窗口关闭 (工作区 {})", ws_idx + 1);
                // 触发关闭动画（装饰层发光脉冲）
                self.window_anims
                    .push((ws_idx, std::time::Instant::now(), false));
                self.dirty = true;
                // 清理 fullscreen：只在关闭的窗口是全屏窗口时才清除
                if let Some(fi) = self.workspaces[ws_idx].fullscreen {
                    let closed = closed_idx.unwrap_or(usize::MAX);
                    // 在 order 中找到 fullscreen 窗口，检查是否是被关闭的窗口
                    let is_fullscreen_win = {
                        let order = self.workspaces[ws_idx].effective_order();
                        match order.get(fi) {
                            Some(WindowSlot::Wl(idx)) => *idx == closed,
                            _ => false,
                        }
                    };
                    if is_fullscreen_win {
                        self.workspaces[ws_idx].fullscreen = None;
                    }
                }
                // 重建窗口顺序
                self.workspaces[ws_idx].rebuild_order();
                // 更新 focus
                if self.workspaces[ws_idx].focus.as_ref() == Some(surface) {
                    let order = self.workspaces[ws_idx].effective_order();
                    self.workspaces[ws_idx].focus = order.last().and_then(|s| match s {
                        WindowSlot::Wl(idx) => self.workspaces[ws_idx]
                            .tops
                            .get(*idx)
                            .map(|tl| tl.wl_surface().clone()),
                        WindowSlot::X11(idx) => self.workspaces[ws_idx]
                            .x11_surfaces
                            .get(*idx)
                            .and_then(|xs| xs.wl_surface()),
                    });
                }
                // 非当前活动工作区：直接 relayout（多屏可见）
                // 当前活动工作区：交给 do_layout_animated 处理（它会调 layout_workspace，
                //   但先保存旧位置用于动画——如果这里先调 layout_workspace 会覆盖 prev_positions）
                if ws_idx != self.active_ws {
                    self.layout_workspace(ws_idx);
                } else {
                    self.do_layout_animated();
                    self.dirty = true;
                    // 更新键盘焦点
                    if let Some(ref s) = self.workspaces[self.active_ws].focus {
                        let kbd = self.kbd.clone();
                        let serial = SERIAL_COUNTER.next_serial();
                        kbd.set_focus(self, Some(s.clone()), serial);
                    }
                }
                return;
            }
        }
        // Check if destroyed surface is a pending toplevel (tooltip that never got app_id)
        let before = self.pending_tops.len();
        self.pending_tops.retain(|tl| tl.wl_surface() != surface);
        if self.pending_tops.len() < before {
            return;
        }
        // Check if destroyed surface is an X11 window
        for ws_idx in 0..self.workspaces.len() {
            let before = self.workspaces[ws_idx].x11_surfaces.len();
            self.workspaces[ws_idx]
                .x11_surfaces
                .retain(|s| s.wl_surface().as_ref() != Some(surface));
            if self.workspaces[ws_idx].x11_surfaces.len() < before {
                info!("🗑️ X11 窗口 wl_surface 销毁 (工作区 {})", ws_idx + 1);
                self.workspaces[ws_idx].fullscreen = None;
                self.workspaces[ws_idx].rebuild_order();
                // 更新 focus
                let order = self.workspaces[ws_idx].effective_order();
                self.workspaces[ws_idx].focus = order.last().and_then(|s| match s {
                    WindowSlot::Wl(idx) => self.workspaces[ws_idx]
                        .tops
                        .get(*idx)
                        .map(|tl| tl.wl_surface().clone()),
                    WindowSlot::X11(idx) => self.workspaces[ws_idx]
                        .x11_surfaces
                        .get(*idx)
                        .and_then(|xs| xs.wl_surface()),
                });
                // 非当前活动工作区：直接 relayout（多屏可见）
                // 当前活动工作区：交给 do_layout_animated 处理
                if ws_idx != self.active_ws {
                    self.layout_workspace(ws_idx);
                } else {
                    self.do_layout_animated();
                    self.dirty = true;
                    if let Some(ref s) = self.workspaces[self.active_ws].focus {
                        let kbd = self.kbd.clone();
                        let serial = SERIAL_COUNTER.next_serial();
                        kbd.set_focus(self, Some(s.clone()), serial);
                    }
                }
                return;
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}
impl SeatHandler for App {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, seat: &Seat<Self>, surface: Option<&WlSurface>) {
        let dh = self.dh.clone();
        let client = surface.and_then(|s| s.client());

        // Update data device (clipboard) focus — sends selection offer to new focus client
        smithay::wayland::selection::data_device::set_data_device_focus::<App>(
            &dh,
            seat,
            client.clone(),
        );
        // Update primary selection focus
        smithay::wayland::selection::primary_selection::set_primary_focus::<App>(&dh, seat, client);

        // Deactivate all X11 surfaces first (tiled + floating)
        for ws in &self.workspaces {
            for xs in &ws.x11_surfaces {
                let _ = xs.set_activated(false);
            }
        }
        for xs in &self.xw.or_surfaces {
            let _ = xs.set_activated(false);
        }
        // Activate the focused X11 surface — check tiled surfaces
        if let Some(surf) = surface {
            if let Some(event) = crate::appctl::focused_window_event(self) {
                crate::appctl::emit_event(self, event);
            }
            for ws in &self.workspaces {
                for xs in &ws.x11_surfaces {
                    if xs.wl_surface().as_ref() == Some(surf) {
                        let _ = xs.set_activated(true);
                        // X11 surface activated: reset cursor — XWayland will set it via set_cursor
                        self.cursor_image(seat, CursorImageStatus::Named(CursorIcon::Default));
                        return;
                    }
                }
            }
            // Also check floating OR surfaces (Dialog/Utility/etc.)
            for xs in &self.xw.or_surfaces {
                if xs.wl_surface().as_ref() == Some(surf) {
                    let _ = xs.set_activated(true);
                    return;
                }
            }
        } else {
            // Focus left all surfaces (e.g. pointer on desktop) — reset to default cursor
            self.cursor_image(seat, CursorImageStatus::Named(CursorIcon::Default));
        }
    }
    fn cursor_image(&mut self, _: &Seat<Self>, status: CursorImageStatus) {
        // 始终使用主题光标（自嘲熊）。拒绝 XWayland 的 Surface 光标——
        // 它们的尺寸/缩放与合成器不匹配会导致拉伸变形，且 X11 应用
        // 离开窗口边缘后不主动重设光标，导致光标卡住。
        // 拒绝 Surface 后，所有光标都用 Named + 主题渲染，保证一致性。
        let status = match status {
            CursorImageStatus::Surface(_) => CursorImageStatus::Named(CursorIcon::Default),
            other => other,
        };

        // 如果是命名光标，尝试从主题加载对应光标图像并缓存
        if let CursorImageStatus::Named(ref icon) = status {
            let name = icon.name().to_string();
            if !self.cursor_cache.contains_key(&name) {
                let theme = self.cfg.cursor.theme.clone();
                let size = self.cfg.cursor.size;
                let fallback = self.cursor_img.clone();

                let img = cursor::CursorImage::load_from_theme(&theme, &name, size)
                    .or_else(|| {
                        // 尝试 X11 旧名称（如 "hand2"、"xterm" 等）
                        for alt in icon.alt_names() {
                            if let Some(img) =
                                cursor::CursorImage::load_from_theme(&theme, alt, size)
                            {
                                return Some(img);
                            }
                        }
                        None
                    })
                    .unwrap_or(fallback);
                self.cursor_cache.insert(name, img);
            }
        }
        self.cursor_status = status;
        self.dirty = true;
    }
}

#[derive(Default)]
struct ClientState {
    comp: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

fn send_frames(s: &WlSurface, t: u32) {
    with_surface_tree_downward(
        s,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_, st, &()| {
            for cb in st
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                cb.done(t);
            }
        },
        |_, _, &()| true,
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info,smithay=warn")
        .init();
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        let dir = format!("/run/user/{}", unsafe { libc::getuid() });
        std::fs::create_dir_all(&dir).ok();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
    }
    let args: Vec<String> = std::env::args().collect();
    let direct = args.iter().any(|a| a == "--direct");
    let cfg = Config::load();
    // 预解析颜色（cfg 被移动到 App 后无法再读取）
    let pre_focus_color = config::parse_color(&cfg.colors.focus_border);
    let pre_unfocus_color = config::parse_color(&cfg.colors.unfocus_border);
    info!(
        "🚀 Anchor v10 GPU ({})",
        if direct { "direct" } else { "session" }
    );

    // ─── GPU 设备选择 ───
    let gpu_path = if let Ok(p) = std::env::var("TITAN_GPU") {
        std::path::PathBuf::from(p)
    } else if !cfg.gpu.device.is_empty() {
        std::path::PathBuf::from(&cfg.gpu.device)
    } else {
        let mut first_card: Option<std::path::PathBuf> = None;
        let mut preferred: Option<std::path::PathBuf> = None;
        let prefer_vendor = cfg.gpu.vendor.to_lowercase();

        if let Ok(entries) = std::fs::read_dir("/dev/dri") {
            // 收集并排序条目（read_dir 不保证顺序，ext4 按 hash 返回）
            let mut cards: Vec<(String, std::path::PathBuf)> = entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with("card") {
                        Some((name, e.path()))
                    } else {
                        None
                    }
                })
                .collect();
            cards.sort_by(|a, b| {
                let na = a.0.trim_start_matches("card").parse::<u32>().unwrap_or(0);
                let nb = b.0.trim_start_matches("card").parse::<u32>().unwrap_or(0);
                na.cmp(&nb)
            });
            for (name, path) in cards {
                if first_card.is_none() {
                    first_card = Some(path.clone());
                }
                if let Ok(v) =
                    std::fs::read_to_string(format!("/sys/class/drm/{}/device/vendor", name))
                {
                    let vendor = v.trim();
                    let matches = match prefer_vendor.as_str() {
                        "nvidia" => vendor == "0x10de",
                        "amd" => vendor == "0x1002",
                        "intel" => vendor == "0x8086",
                        _ => true,
                    };
                    if matches && preferred.is_none() {
                        preferred = Some(path);
                        if prefer_vendor != "auto" {
                            break;
                        }
                    }
                }
            }
        }
        let result = preferred
            .or(first_card)
            .expect("No DRM device found in /dev/dri");
        info!(
            "🎮 GPU auto-detected (vendor preference: {})",
            prefer_vendor
        );
        result
    };
    info!("🎮 {}", gpu_path.display());

    let gpu_vendor = {
        let card_name = gpu_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let vendor_str =
            std::fs::read_to_string(format!("/sys/class/drm/{}/device/vendor", card_name))
                .unwrap_or_default()
                .trim()
                .to_string();
        match vendor_str.as_str() {
            "0x10de" => "NVIDIA",
            "0x1002" => "AMD",
            "0x8086" => "Intel",
            _ => "Unknown",
        }
        .to_string()
    };
    info!("🔍 GPU vendor: {}", gpu_vendor);
    if let Some(card_name) = gpu_path.file_name() {
        std::env::set_var(
            "TITAN_DRM_DEV",
            format!("/dev/dri/{}", card_name.to_string_lossy()),
        );
    }

    // ─── Session ───
    let (dev_fd, session, notifier) = if direct {
        let fd = Arc::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&gpu_path)?,
        );
        let _ = unsafe { libc::ioctl(fd.as_raw_fd(), 0x4000641eu64 as _) };
        let dup = unsafe { libc::dup(fd.as_raw_fd()) };
        use std::os::unix::io::FromRawFd;
        (
            DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(unsafe {
                std::fs::File::from_raw_fd(dup)
            }))),
            None,
            None,
        )
    } else {
        let (mut session, notifier) = LibSeatSession::new()?;
        use smithay::reexports::rustix::fs::OFlags;
        let fd = session.open(&gpu_path, OFlags::RDWR)?;
        info!("✅ DRM 设备已打开 (via libseat)");
        (
            DrmDeviceFd::new(DeviceFd::from(fd)),
            Some(Arc::new(std::sync::Mutex::new(session))),
            Some(notifier),
        )
    };

    // ─── DRM + GBM 设备 ───
    let (mut device, dn) = DrmDevice::new(dev_fd.clone(), false)?;
    info!("✅ DrmDevice");
    let gbm = GbmDevice::new(dev_fd.clone())?;
    info!("✅ GbmDevice");

    // ─── Wayland 显示 + 座位 + 全局对象（不需要 EGL）───
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();
    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "seat0");
    let kbd = seat.add_keyboard(XkbConfig::default(), 200, 25)?;
    let pointer = seat.add_pointer();
    let _output_manager = OutputManagerState::new();
    info!("✅ wl_output");
    InputMethodManagerState::new::<App, _>(&dh, |_client| true);
    TextInputManagerState::new::<App>(&dh);
    VirtualKeyboardManagerState::new::<App, _>(&dh, |_client| true);
    info!("✅ text-input / input-method / virtual-keyboard");
    info!("✅ dmabuf handler ready");

    // ─── 新协议注册 ───
    let layer_shell_state = WlrLayerShellState::new::<App>(&dh);
    info!("✅ wlr_layer_shell");
    let fractional_scale_mgr = FractionalScaleManagerState::new::<App>(&dh);
    info!("✅ wp_fractional_scale");
    let viewporter = ViewporterState::new::<App>(&dh);
    info!("✅ wp_viewporter");
    let idle_inhibit_mgr = IdleInhibitManagerState::new::<App>(&dh);
    info!("✅ idle_inhibit");

    // ─── 游戏鼠标支持：pointer_constraints（指针锁定）+ relative_pointer（相对运动）───
    // Minecraft 等游戏通过 wp_pointer_constraints 的 LockedPointer 锁定鼠标，
    // 通过 zwp_relative_pointer 获取相对运动来控制视角。
    PointerConstraintsState::new::<App>(&dh);
    info!("✅ wp_pointer_constraints");
    RelativePointerManagerState::new::<App>(&dh);
    info!("✅ zwp_relative_pointer");

    // ─── 提前创建 EventLoop（IdleNotifierState 需要 loop handle）───
    let mut eloop: EventLoop<App> = EventLoop::try_new()?;

    // 加载光标
    let cursor_img = if !cfg.cursor.theme.is_empty() {
        cursor::CursorImage::load_from_theme(&cfg.cursor.theme, &cfg.cursor.name, cfg.cursor.size)
            .unwrap_or_else(|| {
                info!("⚠️  光标主题 '{}' 加载失败，使用内置光标", cfg.cursor.theme);
                cursor::CursorImage::builtin(cfg.cursor.size)
            })
    } else {
        let default_theme = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        cursor::CursorImage::load_from_theme(&default_theme, &cfg.cursor.name, cfg.cursor.size)
            .unwrap_or_else(|| cursor::CursorImage::builtin(cfg.cursor.size))
    };

    // ─── 创建 App（显示相关字段用 dummy 值，显示枚举后更新）───
    let mut state = App {
        comp: CompositorState::new::<App>(&dh),
        xdg: XdgShellState::new::<App>(&dh),
        shm: ShmState::new::<App>(&dh, vec![]),
        seat_state,
        seat,
        dd: DataDeviceState::new::<App>(&dh),
        primary_sel: PrimarySelectionState::new::<App>(&dh),
        deco: XdgDecorationState::new::<App>(&dh),
        xdg_activation: XdgActivationState::new::<App>(&dh),
        popup_manager: PopupManager::default(),
        // 注册 Anchor Header Bar 协议 global
        // 客户端通过此协议声明 header bar 高度
        // 注意：global 注册需要在 event loop 之前
        osize: Size::new(0, 0),
        workspaces: (0..NUM_WORKSPACES).map(|_| Workspace::new()).collect(),
        active_ws: 0,
        run: true,
        frame: 0,
        dh: dh.clone(),
        active: false,
        dirty: true,
        kbd,
        pointer,
        cfg,
        cursor_img,
        cursor_status: CursorImageStatus::Named(CursorIcon::Default),
        cursor_cache: std::collections::HashMap::new(),
        pointer_pos: (0.0, 0.0),
        pointer_focus_surface: None,
        window_titles: std::collections::HashMap::new(),
        window_app_ids: std::collections::HashMap::new(),
        vblank_crtcs: std::collections::HashSet::new(),
        wallpaper_cache: wallpaper::WallpaperCache::new(),
        wallpaper_texture: None,
        notifications: Vec::new(),
        scratchpad: ScratchpadState::new(),
        im_popup: None,
        pending_tops: Vec::new(),
        dbus_notifications: notify::start_notification_daemon(),
        launcher: LauncherState::new(),
        ws_anim: WsAnimation {
            start: None,
            from_ws: 0,
            to_ws: 0,
            duration_ms: 200,
            direction: 0,
        },
        layout_anim: LayoutAnimation::new(),
        prev_positions: Vec::new(),
        window_anims: Vec::new(),
        expose_thumbs: Vec::new(),
        cached_focus_color: pre_focus_color,
        cached_unfocus_color: pre_unfocus_color,
        record_state: record::RecordState::new(),
        output_sizes: vec![],
        output_active_ws: vec![],
        focused_output: 0,
        xw: xwayland::XWaylandState::new::<App>(&dh),
        xdisplay: None,
        screenshot: screenshot::ScreenshotState::new(),
        pending_screenshot: None,
        screenshot_result: None,
        loop_handle: None,
        lock_state: LockState::new(),
        x11_saved_focus: None,
        cpu_usage: 0.0,
        mem_usage: 0.0,
        cpu_prev_idle: 0,
        cpu_prev_total: 0,
        // 无限滚动
        scroll_offset: 0.0,
        scroll_offsets: Vec::new(),
        scroll_spring: Spring::from_damping_ratio(17.3, 0.866),
        scroll_momentum: Momentum::new(0.92),
        gesture_active: false,
        gesture_dx: 0.0,
        gesture_dy: 0.0,
        gesture_fingers: 0,
        last_frame_time: std::time::Instant::now(),
        // Overview 状态机
        overview: OverviewState::default(),
        // Settings Panel 状态机
        settings: SettingsState::default(),
        session: session.clone(),
        vt_fd: std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty0")
            .ok()
            .map(|f| std::os::unix::io::OwnedFd::from(f)),
        layer_shell: layer_shell_state,
        fractional_scale_mgr,
        viewporter,
        idle_notifier: IdleNotifierState::new(&dh, eloop.handle()),
        idle_inhibit_mgr,
        idle_timer: None,
        idle_active: true,
        idle_inhibit_count: 0,
        dnd_icon: None,
        last_input_time: std::time::Instant::now(),
        ipc: ipc::IpcServer::bind_default().ok(),
    };
    let listener = ListeningSocket::bind("wayland-anchor")?;
    std::env::set_var("WAYLAND_DISPLAY", "wayland-anchor");
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        std::env::set_var(
            "XDG_RUNTIME_DIR",
            format!("/run/user/{}", unsafe { libc::getuid() }),
        );
    }
    // ── XDG Portal 支持 ──
    std::env::set_var("XDG_CURRENT_DESKTOP", "anchor");
    std::env::set_var("XDG_SESSION_TYPE", "wayland");

    info!("✅ wayland-anchor (XDG_CURRENT_DESKTOP=anchor)");

    // ─── EventLoop 已在上面创建，这里注册 source ───
    state.loop_handle = Some(eloop.handle());
    let mut clients: Vec<Client> = vec![];
    eloop
        .handle()
        .insert_source(dn, |e, _, state: &mut App| match e {
            DrmEvent::VBlank(crtc) => {
                state.vblank_crtcs.insert(crtc);
            }
            DrmEvent::Error(e) => error!("DRM:{e:?}"),
        })?;
    if let Some(notifier) = notifier {
        eloop
            .handle()
            .insert_source(notifier, |event, _, state: &mut App| match event {
                SessionEvent::ActivateSession => {
                    info!("▶️  会话激活");
                    state.active = true;
                    // 设置 VT 为 process mode，使内核不拦截 Ctrl+Alt+Fx
                    state.set_vt_process_mode(true);
                }
                SessionEvent::PauseSession => {
                    info!("⏸️  会话暂停");
                    state.active = false;
                    // 恢复 VT 为 auto mode
                    state.set_vt_process_mode(false);
                }
            })?;
    }
    if let Some(session) = session.as_ref() {
        state.active = session.lock().unwrap().is_active();
        let t0 = Instant::now();
        while !state.active && t0.elapsed() < Duration::from_secs(10) {
            eloop.dispatch(Some(Duration::from_millis(100)), &mut state)?;
            state.active = session.lock().unwrap().is_active();
        }
        if !state.active {
            return Err("libseat 会话 10s 内未激活".into());
        }
        device.activate(true)?;
        info!("✅ DRM master");
    } else {
        state.active = true;
    }

    // ─── EGL + GLES 渲染器（现在已有 DRM master，NVIDIA 上可以正常初始化）───
    let egl_display = unsafe { smithay::backend::egl::EGLDisplay::new(gbm.clone())? };
    info!("✅ EGLDisplay");
    let egl_context = smithay::backend::egl::EGLContext::new(&egl_display)?;
    info!("✅ EGLContext");
    let render_formats: Vec<Format> = egl_context
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect();
    let mut renderer = unsafe { GlesRenderer::new(egl_context)? };
    info!("✅ GlesRenderer");

    // ─── 多显示器枚举 ───
    let res = device.resource_handles()?;
    let mut anchor_outputs: Vec<AnchorOutput> = Vec::new();
    let mut used_crtcs: std::collections::HashSet<crtc::Handle> = std::collections::HashSet::new();
    let mut output_x_offset: i32 = 0;

    struct ConnectorInfo {
        connector: connector::Handle,
        crtc: crtc::Handle,
        mode: smithay::reexports::drm::control::Mode,
        name: String,
        /// 物理尺寸（mm），用于 DPI/缩放计算
        physical_mm: (u32, u32),
    }
    let mut connector_infos: Vec<ConnectorInfo> = Vec::new();

    for &c in res.connectors() {
        for f in [false, true] {
            if let Ok(info) = device.get_connector(c, f) {
                if info.state() != connector::State::Connected || info.modes().is_empty() {
                    continue;
                }
                let mode = info.modes().first().copied().unwrap();
                let (mw, mh) = mode.size();

                let mut found_crtc = None;
                for &enc in info.encoders() {
                    if let Ok(enc_info) = device.get_encoder(enc) {
                        for possible_crtc in res.filter_crtcs(enc_info.possible_crtcs()) {
                            if !used_crtcs.contains(&possible_crtc) {
                                found_crtc = Some(possible_crtc);
                                break;
                            }
                        }
                    }
                    if found_crtc.is_some() {
                        break;
                    }
                }
                let Some(crtc_h) = found_crtc else { continue };
                used_crtcs.insert(crtc_h);

                let phys_mm = info.size().unwrap_or((0, 0));
                let conn_name = format!("{:?}", c);
                info!(
                    "🖥️  Connector {} (CRTC {:?}): {}x{} ({}x{}mm)",
                    conn_name, crtc_h, mw, mh, phys_mm.0, phys_mm.1
                );

                connector_infos.push(ConnectorInfo {
                    connector: c,
                    crtc: crtc_h,
                    mode,
                    name: conn_name,
                    physical_mm: phys_mm,
                });
                break;
            }
        }
    }
    if connector_infos.is_empty() {
        return Err("无可用显示器".into());
    }

    let fd_clones: Vec<_> = (0..connector_infos.len()).map(|_| dev_fd.clone()).collect();
    let mut output_sizes: Vec<(i32, i32, i32, i32)> = Vec::new();

    for (idx, ci) in connector_infos.iter().enumerate() {
        let (mw, mh) = ci.mode.size();

        let surface = match device.create_surface(ci.crtc, ci.mode, &[ci.connector]) {
            Ok(s) => s,
            Err(e) => {
                warn!("⚠️  Surface 创建失败 {}: {:?}", ci.name, e);
                continue;
            }
        };

        let gbm_dup = GbmDevice::new(fd_clones[idx].clone())?;
        let alloc = GbmAllocator::new(gbm_dup, GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
        let buf_surf = match GbmBufferedSurface::new(
            surface,
            alloc,
            &[
                Fourcc::Argb8888,
                Fourcc::Xrgb8888,
                Fourcc::Abgr8888,
                Fourcc::Xbgr8888,
            ],
            render_formats.iter().copied(),
        ) {
            Ok(bs) => {
                info!("✅ GbmBufferedSurface 创建成功 ({})", ci.name);
                bs
            }
            Err(e) => {
                warn!(
                    "⚠️  GbmBufferedSurface 失败 {}: {:?}, trying SCANOUT only",
                    ci.name, e
                );
                let gbm_dup2 = GbmDevice::new(fd_clones[idx].clone())?;
                let alloc2 = GbmAllocator::new(gbm_dup2, GbmBufferFlags::SCANOUT);
                let surface2 = device.create_surface(ci.crtc, ci.mode, &[ci.connector])?;
                GbmBufferedSurface::new(
                    surface2,
                    alloc2,
                    &[Fourcc::Argb8888, Fourcc::Xrgb8888],
                    render_formats.iter().copied(),
                )?
            }
        };

        let wl_output = Output::new(
            ci.name.clone(),
            PhysicalProperties {
                size: (ci.physical_mm.0 as i32, ci.physical_mm.1 as i32).into(),
                subpixel: Subpixel::Unknown,
                make: gpu_vendor.clone(),
                model: ci.name.clone(),
            },
        );
        let output_mode = Mode {
            size: (mw as i32, mh as i32).into(),
            refresh: ci.mode.vrefresh() as i32 * 1000,
        };
        wl_output.add_mode(output_mode);
        wl_output.set_preferred(output_mode);

        // 匹配配置中的 output 设置（工作区、位置、缩放）
        let output_cfg = state.cfg.outputs.iter().find(|oc| {
            if oc.connector.is_empty() {
                false
            } else {
                ci.name.contains(&oc.connector)
            }
        });

        // ── 计算缩放因子（HiDPI）──
        let output_scale = output_cfg
            .map(|oc| oc.scale)
            .filter(|&s| s > 0.0)
            .unwrap_or_else(|| {
                let (pw_mm, ph_mm) = ci.physical_mm;
                if pw_mm > 0 && ph_mm > 0 {
                    let dpi_x = mw as f64 / (pw_mm as f64 / 25.4);
                    let dpi_y = mh as f64 / (ph_mm as f64 / 25.4);
                    let dpi = (dpi_x + dpi_y) / 2.0;
                    let raw = dpi / 96.0;
                    (raw * 4.0).round() / 4.0
                } else {
                    if mw >= 3400 {
                        2.0
                    } else if mw >= 2400 {
                        1.5
                    } else if mw >= 1900 {
                        1.25
                    } else {
                        1.0
                    }
                }
            })
            .max(1.0);
        info!("🖥️  {} 缩放: {:.2}", ci.name, output_scale);
        wl_output.change_current_state(
            Some(output_mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(output_scale)),
            Some(Point::from((output_x_offset, 0))),
        );
        wl_output.create_global::<App>(&dh);

        let default_ws = output_cfg.map(|oc| oc.workspace).unwrap_or(idx);
        let cfg_x = output_cfg.map(|oc| oc.x).unwrap_or(output_x_offset);
        let cfg_y = output_cfg.map(|oc| oc.y).unwrap_or(0);
        let out_x = if output_cfg.map(|oc| oc.x).unwrap_or(0) != 0
            || output_cfg.map(|oc| oc.y).unwrap_or(0) != 0
        {
            cfg_x // 有显式配置位置
        } else {
            output_x_offset // 自动从左到右排列
        };

        output_sizes.push((out_x, cfg_y, mw as i32, mh as i32));

        anchor_outputs.push(AnchorOutput {
            output: wl_output,
            size: Size::new(mw as i32, mh as i32),
            crtc: ci.crtc,
            connector: ci.connector,
            buf_surf,
            pending_flip: false,
            position: (out_x, cfg_y),
            active_ws: default_ws.min(NUM_WORKSPACES - 1),
            name: ci.name.clone(),
            scale: output_scale,
            dpms_off: false,
        });
        output_x_offset += mw as i32;
    }
    if anchor_outputs.is_empty() {
        return Err("所有输出创建失败".into());
    }
    let primary_size = anchor_outputs[0].size;
    info!("✅ {} 个输出已就绪", anchor_outputs.len());

    // 更新 App 的显示相关字段（之前用 dummy 值创建）
    state.osize = primary_size;
    state.output_sizes = output_sizes;
    // 初始化每个 output 的活跃工作区（从 anchor_outputs 读取）
    state.output_active_ws = anchor_outputs.iter().map(|o| o.active_ws).collect();
    // 初始化每个 output 的独立滚动偏移
    state.scroll_offsets = state.output_active_ws.iter().map(|ws| *ws as f64).collect();
    // 初始全局 active_ws / scroll_offset 跟踪第一个 output
    state.active_ws = state.output_active_ws.first().copied().unwrap_or(0);
    state.scroll_offset = state.active_ws as f64;

    {
        struct SessionInputInterface {
            session: Arc<std::sync::Mutex<LibSeatSession>>,
        }
        impl libinput_crate::LibinputInterface for SessionInputInterface {
            fn open_restricted(
                &mut self,
                path: &std::path::Path,
                flags: i32,
            ) -> Result<std::os::unix::io::OwnedFd, i32> {
                use smithay::backend::session::AsErrno;
                use smithay::reexports::rustix::fs::OFlags;
                self.session
                    .lock()
                    .unwrap()
                    .open(path, OFlags::from_bits_truncate(flags as u32))
                    .map_err(|e| e.as_errno().unwrap_or(libc::EACCES))
            }
            fn close_restricted(&mut self, fd: std::os::unix::io::OwnedFd) {
                let _ = self.session.lock().unwrap().close(fd);
            }
        }
        if let Some(session) = session.clone() {
            let iface = SessionInputInterface { session };
            let mut libinput_ctx = libinput_crate::Libinput::new_with_udev(iface);
            if let Err(e) = libinput_ctx.udev_assign_seat("seat0") {
                warn!("⚠️  libinput: {:?}", e);
            } else {
                info!("✅ libinput (seat0)");
                let backend = LibinputInputBackend::new(libinput_ctx);
                eloop
                    .handle()
                    .insert_source(backend, |event, _, state: &mut App| {
                        state.handle_input_event(event);
                    })?;
            }
        }
    }

    let mut dev_active = state.active;
    let start = Instant::now();

    // 壁纸初始化
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let wp_dir = if state.cfg.wallpaper.directory.is_empty() {
            format!("{}/Pictures/wallpapers", home)
        } else {
            state.cfg.wallpaper.directory.clone()
        };
        state.wallpaper_cache.scan_directory(&wp_dir);
        if state.cfg.wallpaper.mode == "image" || state.cfg.wallpaper.mode == "random" {
            let wp_path = if state.cfg.wallpaper.path.is_empty() {
                String::new()
            } else {
                state.cfg.wallpaper.path.clone()
            };
            state
                .wallpaper_cache
                .load(&wp_path, primary_size.w as usize, primary_size.h as usize);
        }
    }

    std::process::Command::new("fcitx5")
        .arg("-d")
        .env("WAYLAND_DISPLAY", "wayland-anchor")
        .env(
            "XDG_RUNTIME_DIR",
            format!("/run/user/{}", unsafe { libc::getuid() }),
        )
        .env("XMODIFIERS", "@im=fcitx")
        .env("QT_IM_MODULE", "fcitx")
        .env("GTK_IM_MODULE", "fcitx")
        .env("SDL_IM_MODULE", "fcitx")
        .spawn()
        .ok();

    // ── XWayland ──
    {
        let eloop_handle = eloop.handle();
        match xwayland::spawn_xwayland(&dh) {
            Ok((xwayland_src, xw_client)) => {
                eloop
                    .handle()
                    .insert_source(xwayland_src, move |event, _, state: &mut App| {
                        if let smithay::xwayland::XWaylandEvent::Ready { display_number, .. } =
                            event
                        {
                            state.xdisplay = Some(display_number);
                        }
                        xwayland::handle_xwayland_event(
                            event,
                            &eloop_handle,
                            &xw_client,
                            &mut state.xw,
                        );
                    })
                    .ok();
            }
            Err(e) => {
                warn!("⚠️  XWayland spawn failed: {} — X11 apps won't work", e);
            }
        }
    }

    // ── 固定颜色 buffer（用于装饰线等）──
    let bg_color: [f32; 4] = {
        let c = config::parse_color(&state.cfg.colors.background);
        [c.0, c.1, c.2, 1.0]
    };
    let focus_color: [f32; 4] = {
        let c = config::parse_color(&state.cfg.colors.focus_border);
        [c.0, c.1, c.2, 1.0]
    };
    let unfocus_color: [f32; 4] = {
        let c = config::parse_color(&state.cfg.colors.unfocus_border);
        [c.0, c.1, c.2, 1.0]
    };

    while state.run {
        if state.active != dev_active {
            if state.active {
                device.activate(true)?;
                for out in &mut anchor_outputs {
                    out.buf_surf.reset_buffers();
                    out.pending_flip = false;
                }
            } else {
                device.pause();
                for out in &mut anchor_outputs {
                    out.pending_flip = false;
                }
            }
            dev_active = state.active;
        }
        if let Some(mut ipc) = state.ipc.take() {
            ipc.poll(&mut state);
            state.ipc = Some(ipc);
        }
        if !dev_active {
            eloop.dispatch(Some(Duration::from_millis(100)), &mut state)?;
            display.dispatch_clients(&mut state)?;
            display.flush_clients()?;
            continue;
        }

        // VBlank handling: mark frames as submitted
        for out in &mut anchor_outputs {
            if state.vblank_crtcs.remove(&out.crtc) {
                if let Err(e) = out.buf_surf.frame_submitted() {
                    if state.frame > 1 {
                        warn!("VBlank err: {:?}", e);
                    }
                }
                out.pending_flip = false;
            }
        }

        if state.dirty {
            // ── 锁屏 PAM 轮询（必须在渲染之前）──
            // 如果认证刚完成，当前帧立刻渲染桌面而非锁屏
            if state.lock_state.locked {
                let was_locked = state.lock_state.locked;
                state.lock_state.poll_unlock();
                if was_locked && !state.lock_state.locked {
                    appctl::emit_event(
                        &mut state,
                        appctl::DesktopEvent::LockChanged { locked: false },
                    );
                    if let Some(event) = appctl::focused_window_event(&state) {
                        appctl::emit_event(&mut state, event);
                    }
                    // 刚解锁！立即重新布局窗口
                    state.do_layout_animated();
                }
            }

            // Upload wallpaper to GPU texture if needed
            if state.wallpaper_cache.pixels.is_some() && state.wallpaper_texture.is_none() {
                if let Some(ref wp) = state.wallpaper_cache.pixels {
                    let (ww, wh) = state.wallpaper_cache.size;
                    match renderer.import_memory(
                        wp,
                        Fourcc::Abgr8888,
                        Size::new(ww as i32, wh as i32),
                        false,
                    ) {
                        Ok(tex) => {
                            info!("✅ 壁纸纹理上传 GPU ({}x{})", ww, wh);
                            state.wallpaper_texture = Some(tex);
                        }
                        Err(e) => {
                            warn!("⚠️  壁纸纹理上传失败: {:?}, fallback gradient", e);
                            // Don't clear pixels — let CPU render path use them as fallback
                            state.wallpaper_cache.pixels = None;
                        }
                    }
                }
            }

            let before_tops = state.workspaces[state.active_ws].tops.len();
            state.workspaces[state.active_ws]
                .tops
                .retain(|tl| tl.alive());
            // 安全网：如果 tops.retain 移除了死掉的 toplevel（比如浏览器）
            // 触发 relayout 确保剩余窗口正确调整大小
            if state.workspaces[state.active_ws].tops.len() < before_tops {
                state.do_layout_animated();
                state.dirty = true;
            }
            let bar_h = if state.cfg.bar.enabled {
                state.cfg.bar.height
            } else {
                0
            };
            let time_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let ws_anim_active = state.ws_anim.start.is_some();
            let ws_anim_dir = state.ws_anim.direction;
            let ws_anim_duration = state.ws_anim.duration_ms;
            let ws_anim_elapsed = state.ws_anim.start.map(|s| s.elapsed().as_millis() as u64);

            // ── 预计算工作区窗口数（避免 Step 4.8 + Step 5 重复计算）──
            let ws_counts: Vec<usize> = state
                .workspaces
                .iter()
                .map(|w| w.tops.len() + w.x11_surfaces.len())
                .collect();

            // ── 物理引擎更新：弹簧吸附 ──
            {
                let now = std::time::Instant::now();
                let dt = now.duration_since(state.last_frame_time).as_secs_f64();
                state.last_frame_time = now;
                let dt = dt.min(0.1);

                if !state.gesture_active {
                    state.scroll_offset = state.scroll_spring.update(dt);
                    if state.scroll_momentum.is_stopped(0.5)
                        && state.scroll_spring.is_settled(0.001)
                    {
                        let snapped = state.scroll_offset.round();
                        state.scroll_spring.set(snapped);
                        state.scroll_offset = snapped;
                    }
                }

                // 同步回 per-output
                if state.focused_output < state.scroll_offsets.len() {
                    state.scroll_offsets[state.focused_output] = state.scroll_offset;
                }
            }

            // ── Overview 状态更新 ──
            if state.overview.is_active() {
                // 记录关闭前的 Task Panel ws（用于切换后执行）
                let task_panel_target = if state.overview.is_task_panel() {
                    Some(state.overview.task_panel_ws())
                } else {
                    None
                };
                let was_active = true;
                state.overview.update_progress(0.0);
                // 如果 Task Panel 关闭动画完成，切换到选中的 ws
                if was_active && !state.overview.is_active() {
                    if let Some(target) = task_panel_target {
                        if target != state.active_ws {
                            state.switch_workspace(target);
                        }
                    }
                }
                // Task Panel snap 动画
                if state.overview.is_task_panel() {
                    let dt = 1.0 / 60.0;
                    if state.overview.update_snap(dt) {
                        state.dirty = true;
                    }
                }
            }

            // ── Settings Panel 状态更新 ──
            if state.settings.is_active() {
                state.settings.update_close();
                state.settings.update_saving();
            }

            for oi in 0..anchor_outputs.len() {
                let out = &mut anchor_outputs[oi];
                if out.pending_flip {
                    continue;
                }

                // 此 output 的工作区（从 App 的 output_active_ws 读取，确保与 switch_workspace 同步）
                let out_ws_idx = state.output_active_ws.get(oi).copied().unwrap_or(0);
                let out_ws = &state.workspaces[out_ws_idx];
                // 预计算 effective_order（避免每帧重复调用 12+ 次）
                let out_order = out_ws.effective_order();
                let n_windows = out_ws.tops.len();
                let n_x11 = out_ws.x11_surfaces.len();
                let n_total = n_windows + n_x11;
                let fullscreen = out_ws.fullscreen;
                let is_focused_output = oi == state.focused_output;

                // Per-output 的焦点和标题
                let out_ws_focus_idx = {
                    let ws = &state.workspaces[out_ws_idx];
                    let order = &out_order;
                    ws.focus.as_ref().and_then(|surf| {
                        order
                            .iter()
                            .enumerate()
                            .find(|(_, slot)| match slot {
                                WindowSlot::Wl(idx) => ws
                                    .tops
                                    .get(*idx)
                                    .map(|tl| tl.wl_surface() == surf)
                                    .unwrap_or(false),
                                WindowSlot::X11(idx) => ws
                                    .x11_surfaces
                                    .get(*idx)
                                    .and_then(|xs| xs.wl_surface())
                                    .map(|s| &s == surf)
                                    .unwrap_or(false),
                            })
                            .map(|(i, _)| i)
                    })
                };
                // out_window_title 将在 Step 5 中直接通过引用获取，避免 clone

                match out.buf_surf.next_buffer() {
                    Ok((mut dmabuf, _)) => {
                        let ow = out.size.w;
                        let oh = out.size.h;

                        // ═══════════════════════════════════════════════
                        // Phase 1: collect surface elements (before bind)
                        // ═══════════════════════════════════════════════
                        let n_est = out_order.len().max(4);
                        let mut win_elems: Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>> =
                            Vec::with_capacity(n_est);
                        let mut popup_elems: Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>> =
                            Vec::with_capacity(n_est);
                        let mut sp_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            Vec::new();
                        let mut im_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            Vec::new();
                        let mut or_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            Vec::new();
                        let mut im_popup_pos: (i32, i32) = (0, 0);
                        let mut ws_offset: i32 = 0;

                        // Expose 关闭后清理点击区域
                        if !state.overview.is_expose() {
                            state.expose_thumbs.clear();
                        }

                        // ── Task Panel 缩略图 elements ──
                        // 在 Phase 1 收集（需要 renderer），Phase 2 画（需要 f）
                        struct ThumbItem {
                            elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
                            // 缩略图在屏幕上的目标位置和大小
                            tx: i32,
                            ty: i32,
                            tw: i32,
                            th: i32,
                            // draw_render_elements 的缩放因子
                            scale: f64,
                            // 窗口标题（用于标签显示）
                            title: String,
                            // 所在 ws 索引（Mission Control 全局视图用）
                            ws_idx: usize,
                            // 动画起点位置（当前 ws 窗口的原始 slot 位置）
                            from_x: i32,
                            from_y: i32,
                        }
                        let mut task_panel_thumbs: Vec<ThumbItem> = Vec::new();
                        let mut scratchpad_data: Option<(i32, i32, i32, i32)> = None; // (x, y, w, h)
                                                                                      // 预计算 slot 位置缓存（在正常模式分支中填充，Step 3 复用）
                        let mut slot_cache: Vec<(i32, i32, i32, i32)> = Vec::new();

                        // 每个 output 都渲染自己工作区的窗口（不再限制 is_primary）
                        {
                            if let Some(fi) = fullscreen {
                                let fs_order = &out_order;
                                if let Some(fs_slot) = fs_order.get(fi) {
                                    match fs_slot {
                                        WindowSlot::Wl(idx) => {
                                            if let Some(tl) = out_ws.tops.get(*idx) {
                                                let tl_geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                    states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                                                }).unwrap_or_default();
                                                let tl_render_pos = Point::<i32, Physical>::from((
                                                    -tl_geo.loc.x,
                                                    bar_h - tl_geo.loc.y,
                                                ));
                                                win_elems.push(render_elements_from_surface_tree(
                                                    &mut renderer,
                                                    tl.wl_surface(),
                                                    tl_render_pos,
                                                    1.0,
                                                    1.0,
                                                    Kind::Unspecified,
                                                ));
                                                let mut p_elems = Vec::new();
                                                for (popup, popup_offset) in
                                                    PopupManager::popups_for_surface(
                                                        tl.wl_surface(),
                                                    )
                                                {
                                                    let offset = (tl_geo.loc + popup_offset
                                                        - popup.geometry().loc)
                                                        .to_physical_precise_round(1.0);
                                                    let pos = tl_render_pos + offset;
                                                    p_elems.extend(
                                                        render_elements_from_surface_tree(
                                                            &mut renderer,
                                                            popup.wl_surface(),
                                                            pos,
                                                            1.0,
                                                            1.0,
                                                            Kind::Unspecified,
                                                        ),
                                                    );
                                                }
                                                popup_elems.push(p_elems);
                                            }
                                        }
                                        WindowSlot::X11(idx) => {
                                            if let Some(xs) = out_ws.x11_surfaces.get(*idx) {
                                                if let Some(wl) = xs.wl_surface() {
                                                    let render_pos =
                                                        Point::<i32, Physical>::from((0, bar_h));
                                                    win_elems.push(
                                                        render_elements_from_surface_tree(
                                                            &mut renderer,
                                                            &wl,
                                                            render_pos,
                                                            1.0,
                                                            1.0,
                                                            Kind::Unspecified,
                                                        ),
                                                    );
                                                    popup_elems.push(Vec::new());
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                // ── 无限滚动：弹簧驱动的连续 ws_offset ──
                                ws_offset = if is_focused_output {
                                    let offset_normalized = out_ws_idx as f64 - state.scroll_offset;
                                    let clamped = offset_normalized.clamp(-1.5, 1.5);
                                    (clamped * ow as f64) as i32
                                } else {
                                    0
                                };

                                // 渲染当前 ws 的窗口（和之前一样）
                                let order = &out_order;
                                // 预计算所有 slot 位置（避免 Phase 1 + Step 3 重复计算）
                                slot_cache = order
                                    .iter()
                                    .enumerate()
                                    .map(|(i, _)| {
                                        layout::slot(
                                            i,
                                            order.len(),
                                            ow,
                                            oh,
                                            bar_h,
                                            &state.cfg,
                                            state.workspaces[out_ws_idx].layout,
                                            state.workspaces[out_ws_idx].split,
                                        )
                                    })
                                    .collect();
                                for (i, slot) in order.iter().enumerate() {
                                    let (x, y, _w, _h) = slot_cache[i];
                                    let (layout_dx, layout_dy) = if is_focused_output {
                                        state.layout_anim.offset_for(slot, (x, y)).unwrap_or((0, 0))
                                    } else {
                                        (0, 0)
                                    };
                                    match slot {
                                        WindowSlot::Wl(idx) => {
                                            if let Some(tl) = out_ws.tops.get(*idx) {
                                                let tl_geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                    states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                                                }).unwrap_or_default();
                                                let bx = x - tl_geo.loc.x + ws_offset + layout_dx;
                                                let by = y - tl_geo.loc.y + layout_dy;
                                                let tl_render_pos =
                                                    Point::<i32, Physical>::from((bx, by));
                                                win_elems.push(render_elements_from_surface_tree(
                                                    &mut renderer,
                                                    tl.wl_surface(),
                                                    tl_render_pos,
                                                    1.0,
                                                    1.0,
                                                    Kind::Unspecified,
                                                ));
                                                let mut p_elems = Vec::new();
                                                for (popup, popup_offset) in
                                                    PopupManager::popups_for_surface(
                                                        tl.wl_surface(),
                                                    )
                                                {
                                                    let offset = (tl_geo.loc + popup_offset
                                                        - popup.geometry().loc)
                                                        .to_physical_precise_round(1.0);
                                                    let pos = tl_render_pos + offset;
                                                    p_elems.extend(
                                                        render_elements_from_surface_tree(
                                                            &mut renderer,
                                                            popup.wl_surface(),
                                                            pos,
                                                            1.0,
                                                            1.0,
                                                            Kind::Unspecified,
                                                        ),
                                                    );
                                                }
                                                popup_elems.push(p_elems);
                                            }
                                        }
                                        WindowSlot::X11(idx) => {
                                            if let Some(xs) = out_ws.x11_surfaces.get(*idx) {
                                                if let Some(wl) = xs.wl_surface() {
                                                    let render_pos = Point::<i32, Physical>::from(
                                                        (x + ws_offset + layout_dx, y + layout_dy),
                                                    );
                                                    win_elems.push(
                                                        render_elements_from_surface_tree(
                                                            &mut renderer,
                                                            &wl,
                                                            render_pos,
                                                            1.0,
                                                            1.0,
                                                            Kind::Unspecified,
                                                        ),
                                                    );
                                                    popup_elems.push(Vec::new());
                                                }
                                            }
                                        }
                                    }
                                }

                                // ── 渲染相邻 ws 的窗口（niri 风格无限滚动过渡） ──
                                // 只在滚动过渡中（scroll_offset 不是整数）且有相邻 ws 时渲染
                                if is_focused_output {
                                    let frac = state.scroll_offset - state.scroll_offset.round();
                                    if frac.abs() > 0.01 {
                                        // 确定要渲染的相邻 ws
                                        let current_int = state.scroll_offset.round() as i32;
                                        let neighbors = if frac > 0.0 {
                                            // 向右滚动：渲染右边相邻 ws
                                            vec![current_int + 1]
                                        } else {
                                            // 向左滚动：渲染左边相邻 ws
                                            vec![current_int - 1]
                                        };
                                        for neighbor_ws in neighbors {
                                            if neighbor_ws < 0
                                                || neighbor_ws as usize >= NUM_WORKSPACES
                                            {
                                                continue;
                                            }
                                            let nws_idx = neighbor_ws as usize;
                                            // 跳过当前 ws（已经渲染了）
                                            if nws_idx == out_ws_idx {
                                                continue;
                                            }
                                            let nws = &state.workspaces[nws_idx];
                                            let n_order = nws.effective_order();
                                            let n_n = n_order.len();
                                            // 这个相邻 ws 相对于 scroll_offset 的偏移
                                            let n_ws_offset = ((nws_idx as f64
                                                - state.scroll_offset)
                                                * ow as f64)
                                                as i32;

                                            for (i, nslot) in n_order.iter().enumerate() {
                                                let (nx, ny, _, _) = layout::slot(
                                                    i, n_n, ow, oh, bar_h, &state.cfg, nws.layout,
                                                    nws.split,
                                                );
                                                match nslot {
                                                    WindowSlot::Wl(idx) => {
                                                        if let Some(tl) = nws.tops.get(*idx) {
                                                            let tl_geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                                states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                                                            }).unwrap_or_default();
                                                            let bx =
                                                                nx - tl_geo.loc.x + n_ws_offset;
                                                            let by = ny - tl_geo.loc.y;
                                                            win_elems.push(
                                                                render_elements_from_surface_tree(
                                                                    &mut renderer,
                                                                    tl.wl_surface(),
                                                                    Point::<i32, Physical>::from((
                                                                        bx, by,
                                                                    )),
                                                                    1.0,
                                                                    1.0,
                                                                    Kind::Unspecified,
                                                                ),
                                                            );
                                                            popup_elems.push(Vec::new());
                                                        }
                                                    }
                                                    WindowSlot::X11(idx) => {
                                                        if let Some(xs) = nws.x11_surfaces.get(*idx)
                                                        {
                                                            if let Some(wl) = xs.wl_surface() {
                                                                win_elems.push(render_elements_from_surface_tree(
                                                                    &mut renderer,
                                                                    &wl,
                                                                    Point::<i32, Physical>::from((nx + n_ws_offset, ny)),
                                                                    1.0, 1.0, Kind::Unspecified,
                                                                ));
                                                                popup_elems.push(Vec::new());
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Scratchpad surface — collected separately (rendered after background in Step 4)
                            if let Some(ref sp_surf) = state.scratchpad.surface {
                                if sp_surf.alive() && state.scratchpad.visible {
                                    let sp_w = ow * 3 / 4;
                                    let sp_h = oh / 3;
                                    let sp_x = (ow - sp_w) / 2;
                                    let sp_y = bar_h + 8;
                                    sp_elems = render_elements_from_surface_tree(
                                        &mut renderer,
                                        sp_surf.wl_surface(),
                                        (sp_x, sp_y),
                                        1.0,
                                        1.0,
                                        Kind::Unspecified,
                                    );
                                    scratchpad_data = Some((sp_x, sp_y, sp_w, sp_h));
                                }
                            }

                            // IM popup (fcitx5 candidate box) — collected separately (rendered on top of windows)
                            if let Some(ref im_popup) = state.im_popup {
                                if im_popup.alive() {
                                    let mut popup_pos = Point::<i32, Physical>::from((
                                        state.pointer_pos.0 as i32,
                                        state.pointer_pos.1 as i32 + 20,
                                    ));
                                    if let Some(parent) = im_popup.get_parent() {
                                        let popup_loc = im_popup.location();
                                        let im_order = &out_order;
                                        for (i, slot) in im_order.iter().enumerate() {
                                            let matched = match slot {
                                                WindowSlot::Wl(idx) => out_ws
                                                    .tops
                                                    .get(*idx)
                                                    .map(|tl| tl.wl_surface() == &parent.surface)
                                                    .unwrap_or(false),
                                                WindowSlot::X11(idx) => out_ws
                                                    .x11_surfaces
                                                    .get(*idx)
                                                    .and_then(|xs| xs.wl_surface())
                                                    .map(|wl| &wl == &parent.surface)
                                                    .unwrap_or(false),
                                            };
                                            if matched {
                                                // 全屏时父窗口占满 output（除 headbar），基准位置是 (0, bar_h)
                                                // 否则用 layout::slot() 的平铺坐标
                                                let (bx, by) = if fullscreen == Some(i) {
                                                    (0, bar_h)
                                                } else {
                                                    let (x, y, _, _) = layout::slot(
                                                        i,
                                                        im_order.len(),
                                                        ow,
                                                        oh,
                                                        bar_h,
                                                        &state.cfg,
                                                        out_ws.layout,
                                                        out_ws.split,
                                                    );
                                                    match slot {
                                                        WindowSlot::Wl(idx) => {
                                                            if let Some(tl) = out_ws.tops.get(*idx)
                                                            {
                                                                let geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                                    states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                                                                }).unwrap_or_default();
                                                                (x - geo.loc.x, y - geo.loc.y)
                                                            } else {
                                                                (x, y)
                                                            }
                                                        }
                                                        WindowSlot::X11(_) => (x, y),
                                                    }
                                                };
                                                popup_pos = Point::<i32, Physical>::from((
                                                    bx + popup_loc.x,
                                                    by + popup_loc.y,
                                                ));
                                                break;
                                            }
                                        }
                                    }
                                    im_popup_pos = (popup_pos.x, popup_pos.y);
                                    if is_focused_output {
                                        let mut elems = render_elements_from_surface_tree(
                                            &mut renderer,
                                            im_popup.wl_surface(),
                                            popup_pos,
                                            1.0,
                                            1.0,
                                            Kind::Unspecified,
                                        );
                                        let fallback_rect = im_popup.text_input_rectangle();
                                        let (popup_w, popup_h) = App::render_bounds_size(&elems)
                                            .unwrap_or((
                                                fallback_rect.size.w.max(240),
                                                fallback_rect.size.h.max(32),
                                            ));
                                        let (clamped_x, clamped_y) = App::clamp_rect_to_bounds(
                                            popup_pos.x,
                                            popup_pos.y,
                                            popup_w,
                                            popup_h,
                                            ow,
                                            oh,
                                            8,
                                        );
                                        let clamped_pos =
                                            Point::<i32, Physical>::from((clamped_x, clamped_y));
                                        if clamped_pos != popup_pos {
                                            elems = render_elements_from_surface_tree(
                                                &mut renderer,
                                                im_popup.wl_surface(),
                                                clamped_pos,
                                                1.0,
                                                1.0,
                                                Kind::Unspecified,
                                            );
                                        }
                                        im_popup_pos = (clamped_pos.x, clamped_pos.y);
                                        im_elems = elems;
                                    }
                                }
                            }
                        }

                        // Collect X11 override-redirect window elements (input method popups, tooltips)
                        // 每个输出都需要检查 OR 窗口是否在自己的范围内（多显示器支持）
                        for xs in &state.xw.or_surfaces {
                            if let Some(wl) = xs.wl_surface() {
                                let geo = xs.geometry();
                                let (ox, oy, _, _) = state
                                    .output_sizes
                                    .get(oi)
                                    .copied()
                                    .unwrap_or((0, 0, state.osize.w, state.osize.h));
                                let render_pos =
                                    Point::<i32, Physical>::from((geo.loc.x - ox, geo.loc.y - oy));
                                // 检查 OR 窗口是否在此 output 的可见范围内（带 200px 容差）
                                let margin = 200;
                                let visible = render_pos.x + geo.size.w + margin >= 0
                                    && render_pos.x <= ow + margin
                                    && render_pos.y + geo.size.h + margin >= 0
                                    && render_pos.y <= oh + margin;
                                if visible {
                                    or_elems.extend(render_elements_from_surface_tree(
                                        &mut renderer,
                                        &wl,
                                        render_pos,
                                        1.0,
                                        1.0,
                                        Kind::Unspecified,
                                    ));
                                }
                            }
                        }

                        // ═══════════════════════════════════════════════
                        // Phase 1.5: collect thumbnail elements for Task Panel / Overview
                        // ═══════════════════════════════════════════════
                        //
                        // 缩略图渲染原理（已验证）：
                        // draw_render_elements(f, scale, ...) 只缩放 buffer 尺寸，不缩放位置。
                        // 所以 location 直接传目标屏幕位置！scale 只控制渲染尺寸缩小。
                        //
                        if is_focused_output && state.overview.is_active() {
                            let progress = state.overview.progress();
                            if state.overview.is_task_panel() && progress > 0.01 {
                                // ── Task Panel: niri 风格水平条形 ──
                                // scale 从 1.0（正常）→ 0.55（拉远），随 progress 插值
                                let base_scale: f32 = 0.55;
                                let scale: f32 = 1.0 - (1.0 - base_scale) * progress as f32;
                                let scroll_offset = match &state.overview {
                                    OverviewState::TaskPanel { scroll_offset, .. } => {
                                        *scroll_offset
                                    }
                                    _ => state.active_ws as f64,
                                };
                                let scaled_w = (ow as f32 * scale) as i32;
                                let scaled_h = (oh as f32 * scale) as i32;
                                // 每个 ws 的水平间距
                                let ws_spacing = (scaled_w + 40) as f32;
                                // 垂直居中
                                let base_y = (oh - scaled_h) / 2;
                                // 中心偏移：让 scroll_offset 对应的 ws 居中
                                let center_offset = ow as f32 / 2.0 - (scaled_w as f32 / 2.0);

                                for ws_i in 0..NUM_WORKSPACES {
                                    let ws = &state.workspaces[ws_i];
                                    let order = ws.effective_order();
                                    let n = order.len();
                                    if n == 0 {
                                        continue;
                                    }

                                    // 这个 ws 的水平偏移（相对于 scroll_offset）
                                    let ws_screen_x = center_offset
                                        + (ws_i as f32 - scroll_offset as f32) * ws_spacing;
                                    // 只收集可见范围内的 ws（略大于屏幕）
                                    if (ws_screen_x + ws_spacing) < -(scaled_w as f32 * 0.5) {
                                        continue;
                                    }
                                    if ws_screen_x > (ow as f32 + scaled_w as f32 * 0.5) {
                                        continue;
                                    }

                                    let ws_offset_x = ws_screen_x as i32;

                                    for (i, slot) in order.iter().enumerate() {
                                        let (sx, sy, sw, sh) = layout::slot(
                                            i, n, ow, oh, bar_h, &state.cfg, ws.layout, ws.split,
                                        );
                                        let tx = ws_offset_x + (sx as f32 * scale) as i32;
                                        let ty = base_y + (sy as f32 * scale) as i32;

                                        let (gx, gy) = match slot {
                                            WindowSlot::Wl(idx) => ws.tops.get(*idx).map(|tl| {
                                                let g = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                    states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry.unwrap_or_default()
                                                });
                                                (g.loc.x, g.loc.y)
                                            }).unwrap_or((0, 0)),
                                            WindowSlot::X11(_) => (0, 0),
                                        };
                                        let loc_x = tx - (gx as f32 * scale) as i32;
                                        let loc_y = ty - (gy as f32 * scale) as i32;

                                        if let Some(elems) = match slot {
                                            WindowSlot::Wl(idx) => ws.tops.get(*idx).map(|tl| {
                                                render_elements_from_surface_tree(
                                                    &mut renderer,
                                                    tl.wl_surface(),
                                                    Point::<i32, Physical>::from((loc_x, loc_y)),
                                                    1.0,
                                                    1.0,
                                                    Kind::Unspecified,
                                                )
                                            }),
                                            WindowSlot::X11(idx) => ws
                                                .x11_surfaces
                                                .get(*idx)
                                                .and_then(|xs| xs.wl_surface())
                                                .map(|wl| {
                                                    render_elements_from_surface_tree(
                                                        &mut renderer,
                                                        &wl,
                                                        Point::<i32, Physical>::from((
                                                            loc_x, loc_y,
                                                        )),
                                                        1.0,
                                                        1.0,
                                                        Kind::Unspecified,
                                                    )
                                                }),
                                        } {
                                            if !elems.is_empty() {
                                                let title = match slot {
                                                    WindowSlot::Wl(idx) => state
                                                        .window_app_ids
                                                        .get(idx)
                                                        .cloned()
                                                        .unwrap_or_else(|| "Window".to_string()),
                                                    WindowSlot::X11(xidx) => ws
                                                        .x11_surfaces
                                                        .get(*xidx)
                                                        .map(|xs| xs.class())
                                                        .unwrap_or_else(|| "X11".to_string()),
                                                };
                                                task_panel_thumbs.push(ThumbItem {
                                                    elems,
                                                    tx,
                                                    ty,
                                                    tw: (sw as f32 * scale) as i32,
                                                    th: (sh as f32 * scale) as i32,
                                                    scale: scale as f64,
                                                    title,
                                                    ws_idx: ws_i,
                                                    from_x: tx,
                                                    from_y: ty,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            // ── Mission Control / Expose: 全局视图 — 所有工作区窗口散布 ──
                            else if state.overview.is_expose() && progress > 0.01 {
                                // 全局视图：收集所有 ws 的窗口
                                let mut total_windows = 0usize;
                                for ws_i in 0..NUM_WORKSPACES {
                                    let ws = &state.workspaces[ws_i];
                                    let n = ws.tops.len() + ws.x11_surfaces.len();
                                    total_windows += n;
                                }
                                if total_windows > 0 {
                                    // 性能优化：只收集可见范围内的 ws
                                    let margin = 60i32;
                                    let top_margin = 80i32;
                                    let gap = 12i32;
                                    let ws_gap = 40i32;
                                    let grid_w = ow - 2 * margin;
                                    let grid_h = oh - top_margin - margin;

                                    let active_ws_count = (0..NUM_WORKSPACES)
                                        .filter(|&i| {
                                            state.workspaces[i].tops.len()
                                                + state.workspaces[i].x11_surfaces.len()
                                                > 0
                                        })
                                        .count();

                                    let card_w = if active_ws_count > 0 {
                                        (grid_w - (active_ws_count as i32 - 1).max(0) * ws_gap)
                                            / active_ws_count as i32
                                    } else {
                                        grid_w
                                    };
                                    let card_h = grid_h;

                                    let mut card_x_offset = margin;
                                    for ws_i in 0..NUM_WORKSPACES {
                                        let ws = &state.workspaces[ws_i];
                                        let order = ws.effective_order();
                                        let n = order.len();
                                        if n == 0 {
                                            continue;
                                        }

                                        let cols = (n as f32).sqrt().ceil() as usize;
                                        let rows = (n + cols - 1) / cols;
                                        let cell_w =
                                            (card_w - (cols as i32 - 1).max(0) * gap) / cols as i32;
                                        let cell_h = (card_h - 24 - (rows as i32 - 1).max(0) * gap)
                                            / rows as i32;
                                        let thumb_scale = (cell_w as f32 / ow as f32)
                                            .min(cell_h as f32 / oh as f32);
                                        let thumb_w = (ow as f32 * thumb_scale) as i32;
                                        let thumb_h = (oh as f32 * thumb_scale) as i32;

                                        for (i, slot) in order.iter().enumerate() {
                                            let col = i % cols;
                                            let row = i / cols;
                                            let tx = card_x_offset
                                                + col as i32 * (cell_w + gap)
                                                + (cell_w - thumb_w) / 2;
                                            let ty = top_margin
                                                + 24
                                                + row as i32 * (cell_h + gap)
                                                + (cell_h - thumb_h) / 2;

                                            // 性能优化：跳过屏幕外的缩略图
                                            if tx + thumb_w < 0 || tx > ow {
                                                continue;
                                            }

                                            let (gx, gy) = match slot {
                                                WindowSlot::Wl(idx) => ws.tops.get(*idx).map(|tl| {
                                                    let g = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                        states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry.unwrap_or_default()
                                                    });
                                                    (g.loc.x, g.loc.y)
                                                }).unwrap_or((0, 0)),
                                                WindowSlot::X11(_) => (0, 0),
                                            };
                                            let loc_x = tx - (gx as f32 * thumb_scale) as i32;
                                            let loc_y = ty - (gy as f32 * thumb_scale) as i32;

                                            if let Some(elems) = match slot {
                                                WindowSlot::Wl(idx) => {
                                                    ws.tops.get(*idx).map(|tl| {
                                                        render_elements_from_surface_tree(
                                                            &mut renderer,
                                                            tl.wl_surface(),
                                                            Point::<i32, Physical>::from((
                                                                loc_x, loc_y,
                                                            )),
                                                            1.0,
                                                            1.0,
                                                            Kind::Unspecified,
                                                        )
                                                    })
                                                }
                                                WindowSlot::X11(idx) => ws
                                                    .x11_surfaces
                                                    .get(*idx)
                                                    .and_then(|xs| xs.wl_surface())
                                                    .map(|wl| {
                                                        render_elements_from_surface_tree(
                                                            &mut renderer,
                                                            &wl,
                                                            Point::<i32, Physical>::from((
                                                                loc_x, loc_y,
                                                            )),
                                                            1.0,
                                                            1.0,
                                                            Kind::Unspecified,
                                                        )
                                                    }),
                                            } {
                                                if !elems.is_empty() {
                                                    let title = match slot {
                                                        WindowSlot::Wl(idx) => state
                                                            .window_app_ids
                                                            .get(idx)
                                                            .cloned()
                                                            .unwrap_or_else(|| {
                                                                "Window".to_string()
                                                            }),
                                                        WindowSlot::X11(xidx) => ws
                                                            .x11_surfaces
                                                            .get(*xidx)
                                                            .map(|xs| xs.class())
                                                            .unwrap_or_else(|| "X11".to_string()),
                                                    };

                                                    // 动画起点：active_ws 窗口使用原始 slot 位置，其他 ws 窗口从目标位置开始
                                                    let (from_x, from_y) =
                                                        if ws_i == state.active_ws {
                                                            let (sx, sy, _, _) = layout::slot(
                                                                i, n, ow, oh, bar_h, &state.cfg,
                                                                ws.layout, ws.split,
                                                            );
                                                            (sx, sy)
                                                        } else {
                                                            (tx, ty)
                                                        };

                                                    // 存储点击区域信息
                                                    state.expose_thumbs.push((
                                                        tx,
                                                        ty,
                                                        thumb_w,
                                                        thumb_h,
                                                        ws_i,
                                                        slot.clone(),
                                                    ));

                                                    task_panel_thumbs.push(ThumbItem {
                                                        elems,
                                                        tx,
                                                        ty,
                                                        tw: thumb_w,
                                                        th: thumb_h,
                                                        scale: thumb_scale as f64,
                                                        title,
                                                        ws_idx: ws_i,
                                                        from_x,
                                                        from_y,
                                                    });
                                                }
                                            }
                                        }
                                        card_x_offset += card_w + ws_gap;
                                    }
                                }
                            }
                        }

                        // ═══════════════════════════════════════════════
                        // Phase 2: bind + render everything (full control)
                        // ═══════════════════════════════════════════════
                        // ── 收集光标表面元素（必须在 bind 之前，因为需要 &mut renderer）──
                        let mut cursor_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            Vec::new();
                        if let CursorImageStatus::Surface(surface) = &state.cursor_status {
                            let (ox, oy, _, _) =
                                state.output_sizes.get(oi).copied().unwrap_or((0, 0, 0, 0));
                            // 读取客户端设置的光标热点（hotspot），用于正确定位光标
                            let mut hotspot = Point::<i32, Logical>::from((0, 0));
                            let _ = smithay::wayland::compositor::with_states(surface, |states| {
                                if let Some(data) = states
                                    .data_map
                                    .get::<smithay::input::pointer::CursorImageSurfaceData>(
                                ) {
                                    hotspot = data.lock().unwrap().hotspot;
                                }
                            });
                            let cursor_pos = Point::<i32, Physical>::from((
                                state.pointer_pos.0 as i32 - ox - hotspot.x,
                                state.pointer_pos.1 as i32 - oy - hotspot.y,
                            ));
                            cursor_elems = render_elements_from_surface_tree(
                                &mut renderer,
                                surface,
                                cursor_pos,
                                1.0,
                                1.0,
                                Kind::Unspecified,
                            );
                        }

                        // ── 收集 Layer Shell 元素（需要在 bind 之前用 renderer）──
                        let mut layer_bg_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            Vec::new();
                        let mut layer_top_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            Vec::new();
                        for ls in state.layer_shell.layer_surfaces() {
                            if !ls.alive() {
                                continue;
                            }
                            let wl_surf = ls.wl_surface().clone();
                            let (lx, ly, is_top) =
                                smithay::wayland::compositor::with_states(&wl_surf, |states| {
                                    let mut guard =
                                        states.cached_state.get::<LayerSurfaceCachedState>();
                                    let cached = guard.current();
                                    let anchor = cached.anchor;
                                    let margin = cached.margin;
                                    let size = cached.size;
                                    let layer = cached.layer;
                                    let w = if anchor.anchored_horizontally() {
                                        ow - margin.left - margin.right
                                    } else {
                                        size.w.max(1)
                                    };
                                    let h = if anchor.anchored_vertically() {
                                        oh - margin.top - margin.bottom
                                    } else {
                                        size.h.max(1)
                                    };
                                    let x = if anchor.contains(Anchor::LEFT) {
                                        margin.left
                                    } else if anchor.contains(Anchor::RIGHT) {
                                        ow - w - margin.right
                                    } else {
                                        (ow - w) / 2
                                    };
                                    let y = if anchor.contains(Anchor::TOP) {
                                        margin.top
                                    } else if anchor.contains(Anchor::BOTTOM) {
                                        oh - h - margin.bottom
                                    } else {
                                        (oh - h) / 2
                                    };
                                    let is_top = matches!(layer, Layer::Top | Layer::Overlay);
                                    (x, y, is_top)
                                });
                            let loc = Point::<i32, Physical>::from((lx, ly));
                            let elems = render_elements_from_surface_tree(
                                &mut renderer,
                                &wl_surf,
                                loc,
                                1.0,
                                1.0,
                                Kind::Unspecified,
                            );
                            if is_top {
                                layer_top_elems.extend(elems);
                            } else {
                                layer_bg_elems.extend(elems);
                            }
                        }

                        // ── 收集 DnD 图标元素 ──
                        let mut dnd_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            Vec::new();
                        if let Some(icon) = &state.dnd_icon {
                            if icon.alive() {
                                let (ox, oy, _, _) =
                                    state.output_sizes.get(oi).copied().unwrap_or((0, 0, 0, 0));
                                let icon_surf = icon.clone();
                                let icon_pos = Point::<i32, Physical>::from((
                                    state.pointer_pos.0 as i32 - ox,
                                    state.pointer_pos.1 as i32 - oy,
                                ));
                                dnd_elems = render_elements_from_surface_tree(
                                    &mut renderer,
                                    &icon_surf,
                                    icon_pos,
                                    1.0,
                                    1.0,
                                    Kind::Unspecified,
                                );
                            }
                        }

                        let mut target = renderer.bind(&mut dmabuf)?;
                        let sp_size = Size::<i32, Physical>::new(ow, oh);
                        let mut f = renderer.render(&mut target, sp_size, Transform::Normal)?;
                        let dmg = Rectangle::from_size(sp_size);

                        // Step 1: Wallpaper
                        // ── Lock screen: skip all normal rendering ──
                        if state.lock_state.locked {
                            // 计算锁屏激活以来的时间（用于基于时间的动画）
                            let lock_elapsed = state
                                .lock_state
                                .time
                                .map(|t| t.elapsed().as_secs_f32())
                                .unwrap_or(0.0);
                            if is_focused_output {
                                // 焦点屏幕：完整锁屏 UI（时钟 + 密码输入框）
                                layout::render_lock_screen(
                                    &mut f,
                                    &state.cfg,
                                    ow,
                                    oh,
                                    time_secs,
                                    lock_elapsed,
                                    &state.lock_state.input,
                                    state.lock_state.wrong,
                                    state.lock_state.shake,
                                    state.lock_state.style,
                                    state.lock_state.is_authenticating(),
                                );
                            } else {
                                // 其他屏幕：暗色覆盖 + 同风格背景
                                layout::render_lock_screen_dim(
                                    &mut f,
                                    &state.cfg,
                                    ow,
                                    oh,
                                    lock_elapsed,
                                    state.lock_state.style,
                                );
                            }
                            let sync = f.finish()?;
                            drop(target);
                            out.buf_surf.queue_buffer(Some(sync), None, ())?;
                            out.pending_flip = true;
                            continue; // skip all other rendering for this output
                        }
                        // ── 视差偏移：壁纸层移动较慢（×0.3）产生深度感 ──
                        let parallax_wallpaper = if is_focused_output {
                            let fractional =
                                state.scroll_offset - (state.scroll_offset.round() as f64);
                            (fractional * ow as f64 * 0.3) as i32
                        } else {
                            0
                        };

                        if let Some(ref _tex) = state.wallpaper_texture {
                            if let Some(ref wp) = state.wallpaper_cache.pixels {
                                let (ww, wh) = state.wallpaper_cache.size;
                                // clamp 源坐标到纹理边界内
                                let wp_offset_x = (parallax_wallpaper as f64)
                                    .max(0.0)
                                    .min((ww as f64 - ow as f64).max(0.0));
                                let wp_src = Rectangle::<f64, _>::from_loc_and_size(
                                    smithay::utils::Point::from((wp_offset_x, 0.0)),
                                    Size::from((ww as f64, wh as f64)),
                                );
                                let _ = f.render_texture_from_to(
                                    _tex,
                                    wp_src,
                                    Rectangle::from_size((ow, oh).into()),
                                    &[Rectangle::from_size((ow, oh).into())],
                                    &[Rectangle::from_size((ow, oh).into())],
                                    Transform::Normal,
                                    1.0,
                                    None,
                                    &[],
                                );
                            }
                        } else if !state.wallpaper_cache.render(&mut f, &state.cfg, ow, oh) {
                            layout::render_wallpaper(&mut f, &state.cfg, ow, oh, state.frame, {
                                // 复用外层已计算的 time_secs，避免重复 SystemTime::now()
                                ((time_secs / 3600 + 8) % 24) as u8 // UTC+8 → 北京时间
                            });
                        }

                        // Step 2: Window surfaces + XDG popups — render per-window, painter's algorithm
                        // Background windows first, focused window last. Popups rendered on top of their parent window.
                        let fi = out_ws_focus_idx.unwrap_or(0);
                        for (i, elems) in win_elems.iter().enumerate() {
                            let use_scale = 1.0;
                            if i != fi {
                                if !elems.is_empty() {
                                    draw_render_elements(&mut f, use_scale, elems, &[dmg])?;
                                }
                                // XDG popups for this background window
                                if let Some(pe) = popup_elems.get(i) {
                                    if !pe.is_empty() {
                                        draw_render_elements(&mut f, use_scale, pe, &[dmg])?;
                                    }
                                }
                            }
                        }
                        // Focused window rendered last (on top of other windows)
                        {
                            let use_scale = 1.0;
                            if let Some(elems) = win_elems.get(fi) {
                                if !elems.is_empty() {
                                    draw_render_elements(&mut f, use_scale, elems, &[dmg])?;
                                }
                            }
                            // Focused window's XDG popups (on top of focused window)
                            if let Some(pe) = popup_elems.get(fi) {
                                if !pe.is_empty() {
                                    draw_render_elements(&mut f, use_scale, pe, &[dmg])?;
                                }
                            }
                        }

                        // Step 2.5: IM popup (只在焦点屏幕上显示)
                        if is_focused_output && !im_elems.is_empty() {
                            draw_render_elements(&mut f, 1.0, &im_elems, &[dmg])?;
                        }

                        // Step 3: Window decorations — 每个 output 都渲染自己工作区的装饰
                        if fullscreen.is_none() {
                            // 计算窗口打开/关闭动画的发光脉冲强度
                            let anim_glow: f32 = state
                                .window_anims
                                .iter()
                                .filter(|(ws_i, _, _)| *ws_i == out_ws_idx)
                                .map(|(_, time, is_open): &(_, std::time::Instant, bool)| {
                                    let elapsed = time.elapsed().as_millis() as f32;
                                    let duration = if *is_open { 400.0 } else { 250.0 };
                                    let t = (elapsed / duration).min(1.0);
                                    let pulse = if *is_open { t } else { 1.0 - t };
                                    pulse * 0.6
                                })
                                .fold(0.0f32, |a, b| a.max(b));
                            state.window_anims.retain(
                                |(_, t, _): &(usize, std::time::Instant, bool)| {
                                    t.elapsed().as_millis() < 500
                                },
                            );

                            let order = &out_order;
                            for (i, slot) in order.iter().enumerate() {
                                let (x, y, _, _) = slot_cache[i];
                                let (dx, dy) = if is_focused_output {
                                    state
                                        .layout_anim
                                        .offset_for(&order[i], (x, y))
                                        .unwrap_or((0, 0))
                                } else {
                                    (0, 0)
                                };
                                // 获取该窗口的 header bar 信息
                                let (hb_h, is_csd) = match slot {
                                    WindowSlot::Wl(idx) => {
                                        if let Some(tl) = out_ws.tops.get(*idx) {
                                            let (h, csd) = get_header_bar_info(tl);
                                            // 全局配置作为 fallback
                                            let final_h = if h > 0 {
                                                h
                                            } else if !csd {
                                                state.cfg.layout.header_bar_height
                                            } else {
                                                0
                                            };
                                            (final_h, csd)
                                        } else {
                                            (state.cfg.layout.header_bar_height, false)
                                        }
                                    }
                                    WindowSlot::X11(_) => {
                                        (state.cfg.layout.header_bar_height, false)
                                    }
                                };
                                layout::render_window_decorations_anim(
                                    &mut f,
                                    &state.cfg,
                                    i,
                                    n_total,
                                    out_ws_focus_idx,
                                    ow,
                                    oh,
                                    bar_h,
                                    state.workspaces[out_ws_idx].layout,
                                    state.workspaces[out_ws_idx].split,
                                    ws_offset + dx,
                                    dy,
                                    anim_glow,
                                    is_csd,
                                    hb_h,
                                );
                            }
                        }

                        // Step 4: Scratchpad — background FIRST, then surface ON TOP
                        if let Some((sp_x, sp_y, sp_w, sp_h)) = scratchpad_data {
                            let bw = 4;
                            let accent = state.cached_focus_color;
                            let border = layout::opaque(accent.0, accent.1, accent.2);
                            let sp_bg = layout::opaque(0.06, 0.06, 0.10);
                            // Background (opaque, covers windows below)
                            f.clear(
                                sp_bg,
                                &[layout::rect(
                                    sp_x - bw,
                                    sp_y - bw,
                                    sp_w + 2 * bw,
                                    sp_h + 2 * bw,
                                )],
                            )
                            .ok();
                            // Border (accent lines)
                            f.clear(
                                border,
                                &[layout::rect(sp_x - bw, sp_y - bw, sp_w + 2 * bw, bw)],
                            )
                            .ok();
                            f.clear(
                                border,
                                &[layout::rect(sp_x - bw, sp_y + sp_h, sp_w + 2 * bw, bw)],
                            )
                            .ok();
                            f.clear(border, &[layout::rect(sp_x - bw, sp_y, bw, sp_h)])
                                .ok();
                            f.clear(border, &[layout::rect(sp_x + sp_w, sp_y, bw, sp_h)])
                                .ok();
                            // 顶部发光扩散
                            for (off, br) in [(1, 0.3f32), (2, 0.15), (3, 0.06)].iter() {
                                let glow =
                                    layout::opaque(accent.0 * br, accent.1 * br, accent.2 * br);
                                f.clear(
                                    glow,
                                    &[layout::rect(
                                        sp_x - bw - off,
                                        sp_y - bw - off,
                                        sp_w + 2 * (bw + off),
                                        *off,
                                    )],
                                )
                                .ok();
                            }
                            // 底部阴影
                            for (off, br) in [(0i32, 0.10f32), (1, 0.05), (2, 0.02)].iter() {
                                f.clear(
                                    layout::opaque(0.0 * br, 0.0 * br, 0.0 * br),
                                    &[layout::rect(
                                        sp_x - bw - 2,
                                        sp_y + sp_h + bw + off,
                                        sp_w + 2 * bw + 4,
                                        1,
                                    )],
                                )
                                .ok();
                            }
                            f.clear(border, &[layout::rect(sp_x + sp_w, sp_y, bw, sp_h)])
                                .ok();
                            // Scratchpad label
                            crate::text_render::draw_text(
                                &mut f,
                                "SCRATCHPAD",
                                sp_x + 6,
                                sp_y - 22,
                                14.0,
                                accent,
                            );
                            // Scratchpad terminal content — rendered AFTER background
                            draw_render_elements(&mut f, 1.0, &sp_elems, &[dmg])?;
                        }

                        // Step 4.5: X11 override-redirect windows (all outputs, based on position)
                        if !or_elems.is_empty() {
                            draw_render_elements(&mut f, 1.0, &or_elems, &[dmg])?;
                        }

                        // Step 4.8: Overview overlay (Task Panel / Bird's Eye View)
                        if is_focused_output && state.overview.is_active() {
                            let progress = state.overview.progress();
                            let titles: Vec<String> = (0..state.workspaces[state.active_ws]
                                .tops
                                .len()
                                + state.workspaces[state.active_ws].x11_surfaces.len())
                                .map(|i| state.window_titles.get(&i).cloned().unwrap_or_default())
                                .collect();

                            if state.overview.is_task_panel() {
                                // ── Task Panel: niri 风格水平条形 ──
                                // 全屏暗色遮罩（随 progress 加深）
                                let alpha = (progress * 0.85).min(0.85) as f32;
                                f.clear(
                                    Color32F::new(0.02, 0.02, 0.06, alpha),
                                    &[Rectangle::from_size((ow, oh).into())],
                                )
                                .ok();

                                let focus_color = layout::opaque(
                                    state.cached_focus_color.0,
                                    state.cached_focus_color.1,
                                    state.cached_focus_color.2,
                                );
                                let scroll_offset = match &state.overview {
                                    OverviewState::TaskPanel { scroll_offset, .. } => {
                                        *scroll_offset
                                    }
                                    _ => state.active_ws as f64,
                                };
                                // scale 从 1.0→0.55 随 progress 插值（视角拉远动画）
                                let base_scale: f32 = 0.55;
                                let scale: f32 = 1.0 - (1.0 - base_scale) * progress as f32;
                                let scaled_w = (ow as f32 * scale) as i32;
                                let scaled_h = (oh as f32 * scale) as i32;
                                let ws_spacing = (scaled_w + 40) as f32;
                                let base_y = (oh - scaled_h) / 2;
                                let center_offset = ow as f32 / 2.0 - (scaled_w as f32 / 2.0);

                                // 画每个 ws 的背景卡片 + 标签
                                for ws_i in 0..NUM_WORKSPACES {
                                    let ws = &state.workspaces[ws_i];
                                    let n = ws.tops.len() + ws.x11_surfaces.len();
                                    let ws_screen_x = center_offset
                                        + (ws_i as f32 - scroll_offset as f32) * ws_spacing;
                                    if (ws_screen_x + ws_spacing) < -(scaled_w as f32 * 0.5) {
                                        continue;
                                    }
                                    if ws_screen_x > (ow as f32 + scaled_w as f32 * 0.5) {
                                        continue;
                                    }

                                    let card_x = ws_screen_x as i32;
                                    let card_y = base_y;

                                    // ws 背景卡片
                                    let is_selected = (ws_i as f64 - scroll_offset).abs() < 0.5;
                                    let card_bg = if is_selected {
                                        Color32F::new(0.12, 0.14, 0.22, 0.6)
                                    } else {
                                        Color32F::new(0.08, 0.08, 0.14, 0.3)
                                    };
                                    f.clear(
                                        card_bg,
                                        &[Rectangle::from_loc_and_size(
                                            (card_x, card_y),
                                            (scaled_w, scaled_h),
                                        )],
                                    )
                                    .ok();

                                    // ws 边框
                                    let border_br: f32 = if is_selected { 0.5 } else { 0.15 };
                                    let bc = Color32F::new(
                                        focus_color.r() * border_br,
                                        focus_color.g() * border_br,
                                        focus_color.b() * border_br,
                                        1.0,
                                    );
                                    f.clear(bc, &[layout::rect(card_x, card_y, scaled_w, 1)])
                                        .ok();
                                    f.clear(
                                        bc,
                                        &[layout::rect(card_x, card_y + scaled_h - 1, scaled_w, 1)],
                                    )
                                    .ok();
                                    f.clear(bc, &[layout::rect(card_x, card_y, 1, scaled_h)])
                                        .ok();
                                    f.clear(
                                        bc,
                                        &[layout::rect(card_x + scaled_w - 1, card_y, 1, scaled_h)],
                                    )
                                    .ok();

                                    // 选中卡片顶部 accent 亮线
                                    if is_selected {
                                        f.clear(
                                            Color32F::new(
                                                focus_color.r() * 0.7,
                                                focus_color.g() * 0.7,
                                                focus_color.b() * 0.7,
                                                1.0,
                                            ),
                                            &[layout::rect(card_x, card_y, scaled_w, 2)],
                                        )
                                        .ok();
                                    }

                                    // WS 标签
                                    let label_br = if is_selected { 0.8 } else { 0.4 };
                                    crate::text_render::draw_text(
                                        &mut f,
                                        &WS_LABELS[ws_i],
                                        card_x + 8,
                                        card_y - 22,
                                        14.0,
                                        (
                                            focus_color.r() * label_br,
                                            focus_color.g() * label_br,
                                            focus_color.b() * label_br,
                                        ),
                                    );
                                    if n > 0 {
                                        crate::text_render::draw_text(
                                            &mut f,
                                            &format!("{} windows", n),
                                            card_x + 8,
                                            card_y - 10,
                                            10.0,
                                            (
                                                focus_color.r() * label_br * 0.5,
                                                focus_color.g() * label_br * 0.5,
                                                focus_color.b() * label_br * 0.5,
                                            ),
                                        );
                                    }
                                }

                                // 画窗口缩略图
                                for thumb in &task_panel_thumbs {
                                    if !thumb.elems.is_empty() {
                                        let _ = draw_render_elements(
                                            &mut f,
                                            thumb.scale,
                                            &thumb.elems,
                                            &[dmg],
                                        );
                                    }
                                    // 缩略图边框（accent 发光）
                                    let border_br: f32 = 0.2;
                                    f.clear(
                                        Color32F::new(
                                            focus_color.r() * border_br,
                                            focus_color.g() * border_br,
                                            focus_color.b() * border_br,
                                            1.0,
                                        ),
                                        &[layout::rect(thumb.tx, thumb.ty, thumb.tw, 1)],
                                    )
                                    .ok();
                                    f.clear(
                                        Color32F::new(
                                            focus_color.r() * border_br,
                                            focus_color.g() * border_br,
                                            focus_color.b() * border_br,
                                            1.0,
                                        ),
                                        &[layout::rect(
                                            thumb.tx,
                                            thumb.ty + thumb.th - 1,
                                            thumb.tw,
                                            1,
                                        )],
                                    )
                                    .ok();
                                    f.clear(
                                        Color32F::new(
                                            focus_color.r() * border_br,
                                            focus_color.g() * border_br,
                                            focus_color.b() * border_br,
                                            1.0,
                                        ),
                                        &[layout::rect(thumb.tx, thumb.ty, 1, thumb.th)],
                                    )
                                    .ok();
                                    f.clear(
                                        Color32F::new(
                                            focus_color.r() * border_br,
                                            focus_color.g() * border_br,
                                            focus_color.b() * border_br,
                                            1.0,
                                        ),
                                        &[layout::rect(
                                            thumb.tx + thumb.tw - 1,
                                            thumb.ty,
                                            1,
                                            thumb.th,
                                        )],
                                    )
                                    .ok();
                                    // 窗口标题
                                    let display_title = if thumb.title.len() > 12 {
                                        format!("{}…", &thumb.title[..12])
                                    } else {
                                        thumb.title.clone()
                                    };
                                    f.clear(
                                        layout::opaque(0.03, 0.03, 0.06),
                                        &[layout::rect(
                                            thumb.tx - 2,
                                            thumb.ty + thumb.th + 1,
                                            thumb.tw + 4,
                                            14,
                                        )],
                                    )
                                    .ok();
                                    crate::text_render::draw_text(
                                        &mut f,
                                        &display_title,
                                        thumb.tx,
                                        thumb.ty + thumb.th + 3,
                                        10.0,
                                        (
                                            focus_color.r() * 0.35,
                                            focus_color.g() * 0.35,
                                            focus_color.b() * 0.35,
                                        ),
                                    );
                                }
                            } else if state.overview.is_expose() {
                                // ── Mission Control 全局视图 ──
                                // 全屏暗色遮罩
                                let alpha = (progress * 0.85).min(0.85) as f32;
                                f.clear(
                                    Color32F::new(0.02, 0.02, 0.06, alpha),
                                    &[Rectangle::from_size((ow, oh).into())],
                                )
                                .ok();

                                let focus_color = layout::opaque(
                                    state.cached_focus_color.0,
                                    state.cached_focus_color.1,
                                    state.cached_focus_color.2,
                                );
                                let selected = state.overview.expose_selected();

                                // 标题
                                crate::text_render::draw_text(
                                    &mut f,
                                    "Mission Control",
                                    60,
                                    30,
                                    18.0,
                                    (
                                        focus_color.r() * 0.8,
                                        focus_color.g() * 0.8,
                                        focus_color.b() * 0.8,
                                    ),
                                );

                                let margin = 60i32;
                                let top_margin = 80i32;
                                let gap = 12i32;
                                let ws_gap = 40i32;
                                let grid_w = ow - 2 * margin;
                                let grid_h = oh - top_margin - margin;

                                let active_ws_count = (0..NUM_WORKSPACES)
                                    .filter(|&i| {
                                        state.workspaces[i].tops.len()
                                            + state.workspaces[i].x11_surfaces.len()
                                            > 0
                                    })
                                    .count();
                                let card_w = if active_ws_count > 0 {
                                    (grid_w - (active_ws_count as i32 - 1).max(0) * ws_gap)
                                        / active_ws_count as i32
                                } else {
                                    grid_w
                                };

                                // 画每个 ws 的分组
                                let mut card_x_offset = margin;
                                let mut thumb_global_idx = 0usize;
                                for ws_i in 0..NUM_WORKSPACES {
                                    let ws = &state.workspaces[ws_i];
                                    let order = ws.effective_order();
                                    let n = order.len();
                                    if n == 0 {
                                        continue;
                                    }

                                    let is_current = ws_i == state.active_ws;

                                    // WS 卡片背景
                                    let card_bg = if is_current {
                                        Color32F::new(0.08, 0.09, 0.16, 0.4)
                                    } else {
                                        Color32F::new(0.05, 0.05, 0.10, 0.25)
                                    };
                                    f.clear(
                                        card_bg,
                                        &[Rectangle::from_loc_and_size(
                                            (card_x_offset - 4, top_margin),
                                            (card_w + 8, grid_h),
                                        )],
                                    )
                                    .ok();

                                    // 当前 ws 顶部高亮线
                                    if is_current {
                                        f.clear(
                                            Color32F::new(
                                                focus_color.r() * 0.6,
                                                focus_color.g() * 0.6,
                                                focus_color.b() * 0.6,
                                                0.8,
                                            ),
                                            &[layout::rect(
                                                card_x_offset - 4,
                                                top_margin,
                                                card_w + 8,
                                                2,
                                            )],
                                        )
                                        .ok();
                                    }

                                    // WS 标签
                                    let label_br: f32 = if is_current { 0.8 } else { 0.4 };
                                    crate::text_render::draw_text(
                                        &mut f,
                                        &WS_LABELS[ws_i],
                                        card_x_offset,
                                        top_margin + 6,
                                        12.0,
                                        (
                                            focus_color.r() * label_br,
                                            focus_color.g() * label_br,
                                            focus_color.b() * label_br,
                                        ),
                                    );

                                    // 画这个 ws 的缩略图（带动画插值）
                                    for thumb in
                                        task_panel_thumbs.iter().filter(|t| t.ws_idx == ws_i)
                                    {
                                        let is_sel = thumb_global_idx == selected;

                                        // 性能优化：跳过屏幕外的缩略图
                                        if thumb.tx + thumb.tw < 0 || thumb.tx > ow {
                                            thumb_global_idx += 1;
                                            continue;
                                        }

                                        // 动画插值：active_ws 窗口从原位飞到目标位置，其他 ws 窗口从目标位置淡入
                                        let (render_x, render_y) = if ws_i == state.active_ws {
                                            let rx = (thumb.from_x as f32
                                                + (thumb.tx - thumb.from_x) as f32
                                                    * progress as f32)
                                                as i32;
                                            let ry = (thumb.from_y as f32
                                                + (thumb.ty - thumb.from_y) as f32
                                                    * progress as f32)
                                                as i32;
                                            (rx, ry)
                                        } else {
                                            (thumb.tx, thumb.ty)
                                        };

                                        // Shadow
                                        for off in 1..=4 {
                                            let shadow_a = (0.12 - off as f32 * 0.03).max(0.01);
                                            f.clear(
                                                Color32F::new(0.0, 0.0, 0.0, shadow_a),
                                                &[layout::rect(
                                                    render_x - off,
                                                    render_y - off,
                                                    thumb.tw + 2 * off,
                                                    thumb.th + 2 * off,
                                                )],
                                            )
                                            .ok();
                                        }

                                        // Render thumbnail
                                        if !thumb.elems.is_empty() {
                                            let _ = draw_render_elements(
                                                &mut f,
                                                thumb.scale,
                                                &thumb.elems,
                                                &[dmg],
                                            );
                                        }

                                        // Border
                                        let br: f32 = if is_sel { 0.8 } else { 0.15 };
                                        let bc = Color32F::new(
                                            focus_color.r() * br,
                                            focus_color.g() * br,
                                            focus_color.b() * br,
                                            1.0,
                                        );
                                        let bw = if is_sel { 3 } else { 1 };
                                        f.clear(
                                            bc,
                                            &[layout::rect(
                                                render_x - bw,
                                                render_y - bw,
                                                thumb.tw + 2 * bw,
                                                bw,
                                            )],
                                        )
                                        .ok();
                                        f.clear(
                                            bc,
                                            &[layout::rect(
                                                render_x - bw,
                                                render_y + thumb.th,
                                                thumb.tw + 2 * bw,
                                                bw,
                                            )],
                                        )
                                        .ok();
                                        f.clear(
                                            bc,
                                            &[layout::rect(render_x - bw, render_y, bw, thumb.th)],
                                        )
                                        .ok();
                                        f.clear(
                                            bc,
                                            &[layout::rect(
                                                render_x + thumb.tw,
                                                render_y,
                                                bw,
                                                thumb.th,
                                            )],
                                        )
                                        .ok();

                                        // Selected glow
                                        if is_sel {
                                            for off in 1..=3 {
                                                let glow_a = (0.15 - off as f32 * 0.04).max(0.02);
                                                let gc = Color32F::new(
                                                    focus_color.r() * glow_a,
                                                    focus_color.g() * glow_a,
                                                    focus_color.b() * glow_a,
                                                    1.0,
                                                );
                                                f.clear(
                                                    gc,
                                                    &[layout::rect(
                                                        render_x - bw - off,
                                                        render_y - bw - off,
                                                        thumb.tw + 2 * (bw + off),
                                                        off,
                                                    )],
                                                )
                                                .ok();
                                                f.clear(
                                                    gc,
                                                    &[layout::rect(
                                                        render_x - bw - off,
                                                        render_y + thumb.th + bw,
                                                        thumb.tw + 2 * (bw + off),
                                                        off,
                                                    )],
                                                )
                                                .ok();
                                            }
                                        }

                                        // Title
                                        let display_title = if thumb.title.len() > 14 {
                                            format!("{}…", &thumb.title[..14])
                                        } else {
                                            thumb.title.clone()
                                        };
                                        f.clear(
                                            layout::opaque(0.03, 0.03, 0.06),
                                            &[layout::rect(
                                                render_x,
                                                render_y + thumb.th + 2,
                                                thumb.tw,
                                                14,
                                            )],
                                        )
                                        .ok();
                                        let title_br: f32 = if is_sel { 0.9 } else { 0.35 };
                                        crate::text_render::draw_text(
                                            &mut f,
                                            &display_title,
                                            render_x + 3,
                                            render_y + thumb.th + 4,
                                            10.0,
                                            (
                                                focus_color.r() * title_br,
                                                focus_color.g() * title_br,
                                                focus_color.b() * title_br,
                                            ),
                                        );

                                        thumb_global_idx += 1;
                                    }

                                    card_x_offset += card_w + ws_gap;
                                }
                            }
                        }

                        // Step 5: Headbar — 每个 output 显示自己的活跃工作区
                        {
                            // ── 视差偏移：headbar 层移动稍快（×0.1）产生前景感 ──
                            let parallax_headbar = if is_focused_output {
                                let fractional =
                                    state.scroll_offset - (state.scroll_offset.round() as f64);
                                (fractional * ow as f64 * 0.1) as i32
                            } else {
                                0
                            };
                            // 直接获取窗口标题引用，避免提前 clone
                            let out_window_title = state
                                .window_titles
                                .get(&out_ws_focus_idx.unwrap_or(0))
                                .map(|s| s.as_str())
                                .unwrap_or("");
                            layout::render_headbar(
                                &mut f,
                                &state.cfg,
                                ow,
                                oh,
                                n_total,
                                out_ws_focus_idx,
                                time_secs,
                                out_window_title,
                                out_ws_idx,
                                NUM_WORKSPACES,
                                &ws_counts,
                                state.cpu_usage,
                                state.mem_usage,
                                state.record_state.recording,
                                state.scroll_offset,
                            );
                        }

                        // Step 5.5: Layer Shell surfaces (draw pre-collected elements)
                        {
                            let scale_f = if ow > 0 { out.scale } else { 1.0 };
                            if !layer_bg_elems.is_empty() {
                                let _ =
                                    draw_render_elements(&mut f, scale_f, &layer_bg_elems, &[dmg]);
                            }
                            if !layer_top_elems.is_empty() {
                                let _ =
                                    draw_render_elements(&mut f, scale_f, &layer_top_elems, &[dmg]);
                            }
                        }

                        // Step 6: Notifications — 只在鼠标所在的 output 上显示
                        if is_focused_output && !state.notifications.is_empty() {
                            let accent = state.cached_focus_color;
                            let notif_data: Vec<(String, std::time::Instant, std::time::Duration)> =
                                state
                                    .notifications
                                    .iter()
                                    .map(|n| (n.text.clone(), n.created, n.duration))
                                    .collect();
                            layout::render_notifications(
                                &mut f,
                                &notif_data,
                                ow,
                                state.cfg.bar.height,
                                accent,
                            );
                        }

                        // Step 7: Launcher — 面板外透明（桌面可见）+ 面板本体深色背景
                        if is_focused_output && state.launcher.visible {
                            // 面板本体: 深色纯色背景（文字可读）
                            let filtered = state.launcher.filtered();
                            let lw = ow * 3 / 4;
                            let max_items = 12usize;
                            let item_h: i32 = 36;
                            let header_h: i32 = 48;
                            let n_items = filtered.len().min(max_items);
                            let lh = header_h + (n_items as i32) * item_h + 20;
                            let lx = (ow - lw) / 2;
                            let ly = bar_h + 24;
                            f.clear(
                                layout::opaque(0.10, 0.10, 0.16),
                                &[layout::rect(lx, ly, lw, lh)],
                            )
                            .ok();

                            // 渲染 launcher UI 元素
                            layout::render_launcher(
                                &mut f,
                                &state.cfg,
                                ow,
                                oh,
                                &state.launcher.query,
                                &filtered,
                                state.launcher.selected,
                                state.frame,
                            );
                        }

                        // Step 7.5: Settings Panel — 可视化配置界面
                        if is_focused_output && state.settings.is_active() {
                            settings::render::render_settings_panel(
                                &mut f,
                                &state.cfg,
                                ow,
                                oh,
                                &state.settings,
                                state.frame,
                            );
                        }

                        // Step 8: Cursor — 只在鼠标所在的 output 上渲染（坐标需要转换）
                        if is_focused_output {
                            match &state.cursor_status {
                                CursorImageStatus::Hidden => {
                                    // 光标隐藏（例如：视频全屏、游戏）
                                }
                                CursorImageStatus::Surface(_) => {
                                    // 客户端提供了自定义光标表面，渲染它
                                    if !cursor_elems.is_empty() {
                                        let _ = draw_render_elements(
                                            &mut f,
                                            1.0,
                                            &cursor_elems,
                                            &[dmg],
                                        );
                                    } else {
                                        // 回退：无元素时使用默认光标
                                        let (ox, _oy, _, _) = state
                                            .output_sizes
                                            .get(oi)
                                            .copied()
                                            .unwrap_or((0, 0, 0, 0));
                                        let cx = state.pointer_pos.0 as i32
                                            - ox
                                            - state.cursor_img.hotspot_x as i32;
                                        let cy = state.pointer_pos.1 as i32
                                            - _oy
                                            - state.cursor_img.hotspot_y as i32;
                                        state.cursor_img.render_batched(&mut f, cx, cy);
                                    }
                                }
                                CursorImageStatus::Named(icon) => {
                                    // 使用命名光标，优先从缓存取，否则用默认
                                    let img = state
                                        .cursor_cache
                                        .get(icon.name())
                                        .unwrap_or(&state.cursor_img);
                                    let (ox, _oy, _, _) =
                                        state.output_sizes.get(oi).copied().unwrap_or((0, 0, 0, 0));
                                    let cx = state.pointer_pos.0 as i32 - ox - img.hotspot_x as i32;
                                    let cy =
                                        state.pointer_pos.1 as i32 - _oy - img.hotspot_y as i32;
                                    img.render_batched(&mut f, cx, cy);
                                }
                            }
                        }

                        // Step 8.5: DnD drag icon — draw pre-collected elements at cursor
                        if !dnd_elems.is_empty() {
                            let _ = draw_render_elements(&mut f, 1.0, &dnd_elems, &[dmg]);
                        }

                        // Step 9: Screenshot area selection overlay
                        // 坐标需要从全局转为 output-local
                        {
                            let (out_ox, out_oy, _, _) = state
                                .output_sizes
                                .get(oi)
                                .copied()
                                .unwrap_or((0, 0, ow, oh));
                            if state.screenshot.selecting {
                                if let Some((rx, ry, rw, rh)) = state.screenshot.selection_rect() {
                                    // 全局坐标转为 output-local
                                    let local_rect = (rx - out_ox, ry - out_oy, rw, rh);
                                    screenshot::render_selection_overlay(
                                        &mut f, ow, oh, local_rect,
                                    );
                                }
                            }
                        }

                        let sync = f.finish()?;
                        // drop f 释放对 target 的借用

                        // Step 10: 执行待处理的截图请求（finish 后 framebuffer 完整，target 仍可用）
                        // 截图截鼠标所在的 output
                        {
                            let (out_ox, out_oy, _, _) = state
                                .output_sizes
                                .get(oi)
                                .copied()
                                .unwrap_or((0, 0, ow, oh));
                            let px = state.pointer_pos.0 as i32;
                            let py = state.pointer_pos.1 as i32;
                            let pointer_on_this_output = px >= out_ox
                                && px < out_ox + ow
                                && py >= out_oy
                                && py < out_oy + oh;
                            if pointer_on_this_output {
                                if let Some(req) = state.pending_screenshot.take() {
                                    let area = match &req {
                                        screenshot::ScreenshotRequest::Area(x, y, w, h) => {
                                            // 全局坐标转为 output-local
                                            Some((*x - out_ox, *y - out_oy, *w, *h))
                                        }
                                        screenshot::ScreenshotRequest::Full => None,
                                    };
                                    use smithay::backend::allocator::Fourcc;
                                    use smithay::backend::renderer::Renderer;
                                    let region = Rectangle::from_size((ow, oh).into());
                                    match renderer.copy_framebuffer(
                                        &target,
                                        region,
                                        Fourcc::Abgr8888,
                                    ) {
                                        Ok(mapping) => match renderer.map_texture(&mapping) {
                                            Ok(pixels) => {
                                                let w = ow as u32;
                                                let h = oh as u32;
                                                let row_len = w as usize * 4;
                                                let mut rgba = Vec::with_capacity(pixels.len());
                                                for row in 0..h as usize {
                                                    let start = row * row_len;
                                                    let end = start + row_len;
                                                    if end <= pixels.len() {
                                                        rgba.extend_from_slice(&pixels[start..end]);
                                                    }
                                                }
                                                let result =
                                                    screenshot::save_screenshot(&rgba, w, h, area);
                                                state.screenshot_result = Some(result);
                                            }
                                            Err(e) => {
                                                tracing::warn!("📸 map_texture failed: {:?}", e);
                                                state.screenshot_result =
                                                    Some((String::new(), None));
                                            }
                                        },
                                        Err(e) => {
                                            tracing::warn!("📸 copy_framebuffer failed: {:?}", e);
                                            state.screenshot_result = Some((String::new(), None));
                                        }
                                    }
                                }
                            }
                        }

                        // 屏幕录制帧捕获（每 6 帧采样一次 ≈ 10fps，降低 GPU→CPU 压力）
                        // 录屏也只录鼠标所在的 output
                        {
                            let (out_ox, out_oy, _, _) = state
                                .output_sizes
                                .get(oi)
                                .copied()
                                .unwrap_or((0, 0, ow, oh));
                            let px = state.pointer_pos.0 as i32;
                            let py = state.pointer_pos.1 as i32;
                            let pointer_on_this_output = px >= out_ox
                                && px < out_ox + ow
                                && py >= out_oy
                                && py < out_oy + oh;
                            if state.record_state.recording
                                && pointer_on_this_output
                                && state.frame % 3 == 0
                            {
                                use smithay::backend::allocator::Fourcc;
                                use smithay::backend::renderer::Renderer;
                                let region = Rectangle::from_size((ow, oh).into());
                                match renderer.copy_framebuffer(&target, region, Fourcc::Abgr8888) {
                                    Ok(mapping) => match renderer.map_texture(&mapping) {
                                        Ok(pixels) => {
                                            state
                                                .record_state
                                                .write_frame(&pixels, ow as u32, oh as u32);
                                        }
                                        Err(_) => {}
                                    },
                                    Err(_) => {}
                                }
                            }
                        }

                        drop(target);

                        out.buf_surf.queue_buffer(Some(sync), None, ())?;
                        out.pending_flip = true;
                    }
                    Err(e) => {
                        if state.frame == 0 {
                            error!("❌ {e:?}");
                        }
                    }
                }
            }
            state.dirty = false;

            // 处理截图结果
            if let Some((path, png_data)) = state.screenshot_result.take() {
                if path.is_empty() {
                    appctl::emit_event(
                        &mut state,
                        appctl::DesktopEvent::ScreenshotFailed {
                            reason: "screenshot failed".into(),
                        },
                    );
                    state.notify("Screenshot failed".to_string());
                } else if let Some(png) = png_data {
                    state.set_clipboard_png(path.clone(), png);
                    appctl::emit_event(
                        &mut state,
                        appctl::DesktopEvent::ScreenshotCompleted { path: path.clone() },
                    );
                    state.notify(format!("Saved: {} (copied to clipboard)", path));
                } else {
                    appctl::emit_event(
                        &mut state,
                        appctl::DesktopEvent::ScreenshotCompleted { path: path.clone() },
                    );
                    state.notify(format!("Saved: {} (clipboard failed)", path));
                }
                state.dirty = true;
            }

            // 发送 frame callback（合并 toplevel + popup 遍历）
            let now = start.elapsed().as_millis() as u32;
            for s in state.xdg.toplevel_surfaces() {
                send_frames(s.wl_surface(), now);
                // XDG popup surfaces (browser menus, context menus, etc.)
                for (popup, _) in PopupManager::popups_for_surface(s.wl_surface()) {
                    send_frames(popup.wl_surface(), now);
                }
            }
            // IM popup surface (fcitx5 candidate box) — needs frame callback to commit buffer
            if let Some(ref im_popup) = state.im_popup {
                if im_popup.alive() {
                    send_frames(im_popup.wl_surface(), now);
                }
            }
            // Scratchpad surface — needs frame callback to commit buffer
            if let Some(ref sp_surf) = state.scratchpad.surface {
                if sp_surf.alive() {
                    send_frames(sp_surf.wl_surface(), now);
                }
            }
            // X11 surfaces — 遍历所有工作区（多显示器各自不同 ws，不能只用 active_ws）
            for ws in &state.workspaces {
                for xs in &ws.x11_surfaces {
                    if let Some(wl) = xs.wl_surface() {
                        send_frames(&wl, now);
                    }
                }
            }
            // X11 OR surfaces (input method popups, tooltips)
            for xs in &state.xw.or_surfaces {
                if let Some(wl) = xs.wl_surface() {
                    send_frames(&wl, now);
                }
            }
            // Handle X11 layout changes
            // (X11 layout is now triggered directly in map/unmap/destroy handlers)
            // 动画进行中时持续请求渲染
            if state
                .ws_anim
                .start
                .map(|s| (s.elapsed().as_millis() as u64) < state.ws_anim.duration_ms)
                .unwrap_or(false)
            {
                state.dirty = true;
            }
            // 布局动画进行中时持续请求渲染
            if state.layout_anim.is_active() {
                state.dirty = true;
            }
            // 窗口打开/关闭发光动画进行中时持续渲染
            if !state.window_anims.is_empty() {
                state.dirty = true;
            }
            // Overview 动画进行中时持续请求渲染（与 ws_anim/layout_anim 同模式）
            if state.overview.is_active() {
                state.dirty = true;
            }
            // Settings Panel 动画进行中时持续请求渲染
            if state.settings.is_active() {
                state.dirty = true;
            }
            // 弹簧滚动动画进行中时持续请求渲染（与 ws_anim 同模式）
            if !state.scroll_spring.is_settled(0.001) {
                state.dirty = true;
            }
            // 锁屏动画需要持续重绘（frame 驱动动画，必须保证 dirty 始终为 true）
            if state.lock_state.locked {
                state.dirty = true;
            }
            state.frame += 1;
            // Drain D-Bus notifications into toast system
            {
                let dbus_notifs: Vec<_> = state
                    .dbus_notifications
                    .lock()
                    .map(|mut s| s.pending.drain(..).collect())
                    .unwrap_or_default();
                for n in dbus_notifs {
                    let text = if n.body.is_empty() {
                        format!("[{}] {}", n.app_name, n.summary)
                    } else {
                        format!("[{}] {} {}", n.app_name, n.summary, n.body)
                    };
                    state.notify(text);
                }
            }
            state.drain_notifications();
            if state.frame % 60 == 0 {
                state.popup_manager.cleanup();
            }
            if state.frame == 1 {
                info!("✅ 第一帧渲染！");
            }
            if state.frame % 600 == 0 {
                info!("📊 {} 帧", state.frame);
            }
        }

        eloop.dispatch(Some(Duration::from_millis(16)), &mut state)?;
        // 时钟每秒更新（bar enabled 时）
        if state.frame % 60 == 0 && state.cfg.bar.enabled {
            state.dirty = true;
        }
        // CPU/MEM 状态每 5 秒更新
        if state.frame % 300 == 0 {
            state.update_cpu_usage();
            state.update_mem_usage();
            state.dirty = true;
        }

        // ── 空闲检测：检查是否超时 → 触发锁屏/DPMS ──
        if state.frame % 60 == 0
            && state.idle_active
            && state.idle_inhibit_count == 0
            && state.cfg.idle.timeout > 0
            && !state.lock_state.locked
        {
            if state.last_input_time.elapsed() >= Duration::from_secs(state.cfg.idle.timeout) {
                state.idle_active = false;
                if state.cfg.idle.lock_on_idle {
                    state.lock_state.lock(state.pointer_pos.0);
                    info!("💤 空闲锁屏 ({}s)", state.cfg.idle.timeout);
                }
            }
        }

        if let Ok(Some(stream)) = listener.accept() {
            clients.push(
                display
                    .handle()
                    .insert_client(stream, Arc::new(ClientState::default()))?,
            );
        }
        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }

    info!("👋");
    Ok(())
}

delegate_xdg_shell!(App);
delegate_xdg_decoration!(App);
delegate_xdg_activation!(App);
delegate_compositor!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_data_device!(App);
delegate_primary_selection!(App);
delegate_output!(App);
delegate_input_method_manager!(App);
delegate_text_input_manager!(App);
delegate_virtual_keyboard_manager!(App);
delegate_layer_shell!(App);
delegate_fractional_scale!(App);
delegate_viewporter!(App);
delegate_idle_notify!(App);
delegate_idle_inhibit!(App);
smithay::delegate_xwayland_shell!(App);

// ── Pointer Constraints + Relative Pointer（游戏鼠标支持）─────────
// Minecraft 等游戏通过 wp_pointer_constraints 的 LockedPointer 锁定鼠标隐藏光标，
// 通过 zwp_relative_pointer 获取相对运动来旋转视角。
delegate_pointer_constraints!(App);
delegate_relative_pointer!(App);

impl PointerConstraintsHandler for App {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        // Anchor 不拒绝任何指针约束请求——游戏需要 pointer lock 才能控制视角。
        // 约束存储在 surface 的 data_map 中（由 PointerConstraintsState 管理），
        // 激活后会向客户端发送 locked/confined 事件。
        with_pointer_constraint(surface, pointer, |constraint| {
            if let Some(constraint) = constraint {
                constraint.activate();
            }
        });
        // 隐藏系统光标——游戏会自己绘制准星，系统光标会干扰
        self.cursor_status = CursorImageStatus::Hidden;
        self.dirty = true;
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        _location: Point<f64, Logical>,
    ) {
        // Anchor 不使用 cursor position hint（游戏自己管理准星位置）
    }
}

// ── XWayland Handlers ──────────────────────────────────────

impl smithay::wayland::xwayland_shell::XWaylandShellHandler for App {
    fn xwayland_shell_state(
        &mut self,
    ) -> &mut smithay::wayland::xwayland_shell::XWaylandShellState {
        &mut self.xw.shell
    }

    fn surface_associated(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        wl_surface: WlSurface,
        surface: smithay::xwayland::X11Surface,
    ) {
        tracing::info!(
            "🔗 XWayland surface associated: class='{}' title='{}'",
            surface.class(),
            surface.title()
        );

        // wl_surface 现在就绪了。如果这个 X11 窗口在 tiling 布局中但之前
        // 没有被 focus（因为 map_window_request 时 wl_surface 还是 None），
        // 现在补上 focus 和 configure。
        let wid = surface.window_id();
        let mut found_and_focused = false;
        for ws_idx in 0..self.workspaces.len() {
            let ws = &self.workspaces[ws_idx];
            if ws.x11_surfaces.iter().any(|xs| xs.window_id() == wid) {
                // 找到了这个窗口所在的 workspace
                // 设置 focus
                self.workspaces[ws_idx].focus = Some(wl_surface.clone());
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(wl_surface.clone()), serial);

                // 重新 layout（configure 尺寸给 X11 客户端）
                if ws_idx == self.active_ws {
                    self.do_layout_animated();
                } else {
                    self.layout_workspace(ws_idx);
                }

                tracing::info!("🔗 surface_associated: focused + layout for ws={}", ws_idx);
                found_and_focused = true;
                break;
            }
        }

        if !found_and_focused {
            // 窗口可能在 or_surfaces 中（OR 窗口），不需要 tiling layout
            tracing::info!("🔗 surface_associated: window not in tiling (OR window?)");
        }

        self.dirty = true;
    }
}

impl smithay::xwayland::XwmHandler for App {
    fn xwm_state(&mut self, _xwm: smithay::xwayland::xwm::XwmId) -> &mut smithay::xwayland::X11Wm {
        self.xw
            .xwm
            .as_mut()
            .expect("XwmHandler called but X11Wm not ready")
    }

    fn new_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        tracing::info!(
            "🆕 X11 new_window: class='{}' title='{}'",
            window.class(),
            window.title()
        );
    }

    fn new_override_redirect_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        use smithay::xwayland::xwm::WmWindowType;
        tracing::info!(
            "🆕 X11 OR window: class='{}' title='{}' type={:?} transient={:?}",
            window.class(),
            window.title(),
            window.window_type(),
            window.is_transient_for()
        );

        // 检查是否为辅助窗口（tooltip、popup 等）
        // 某些应用（如飞书打开 MPV）可能意外以 OR 模式创建普通窗口
        let is_aux = matches!(
            window.window_type(),
            Some(WmWindowType::Tooltip)
                | Some(WmWindowType::PopupMenu)
                | Some(WmWindowType::DropdownMenu)
                | Some(WmWindowType::Notification)
        );

        // 非 auxiliary OR 窗口（如 MPV 视频窗口、Dialog 等）也需要能关闭和交互
        // 记录警告以便调试，但仍然加入 or_surfaces 以避免丢失窗口
        if !is_aux {
            tracing::warn!(
                "⚠️  Non-aux X11 OR window: class='{}' type={:?} — may need manual close",
                window.class(),
                window.window_type()
            );
        }

        self.xw.on_new_or_window(window);
        self.dirty = true;
    }

    fn map_window_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        use smithay::xwayland::xwm::WmWindowType;

        // is_aux: tooltip/popup/notification → 浮动，不抢焦点
        // is_floating: Utility/Menu/Splash 和无父 Dialog → 浮动，但需要焦点和交互
        // 带 transient_for 的 Dialog（如 XWayland 文件选择器）跟随父窗口进入 tiling，
        // 否则会固定走全局浮动层，看起来总是在屏幕 1 上悬浮。
        let is_aux = matches!(
            window.window_type(),
            Some(WmWindowType::Tooltip)
                | Some(WmWindowType::PopupMenu)
                | Some(WmWindowType::DropdownMenu)
                | Some(WmWindowType::Notification)
        );
        let has_transient = window.is_transient_for().is_some();
        let is_transient_dialog = has_transient
            && matches!(
                window.window_type(),
                Some(WmWindowType::Dialog)
                    | Some(WmWindowType::Menu)
                    | Some(WmWindowType::Utility)
                    | Some(WmWindowType::Splash)
            );
        let is_floating = !is_aux
            && !is_transient_dialog
            && matches!(
                window.window_type(),
                Some(WmWindowType::Dialog)
                    | Some(WmWindowType::Utility)
                    | Some(WmWindowType::Menu)
                    | Some(WmWindowType::Splash)
            );

        tracing::info!(
            "🗺️  X11 map_request: class='{}' title='{}' type={:?} transient={:?} aux={} transient_dialog={} floating={} has_wl_surface={}",
            window.class(),
            window.title(),
            window.window_type(),
            window.is_transient_for(),
            is_aux,
            is_transient_dialog,
            is_floating,
            window.wl_surface().is_some()
        );

        if let Err(e) = window.set_mapped(true) {
            tracing::warn!("⚠️  X11 set_mapped failed: {:?}", e);
            return;
        }

        let wid = window.window_id();

        if is_aux {
            // 辅助窗口（tooltip、IM候选框等）：浮动，不抢焦点
            tracing::info!("📌 X11 aux → floating overlay (no focus)");
            if !self.xw.or_surfaces.iter().any(|s| s.window_id() == wid) {
                self.xw.or_surfaces.push(window);
            }
        } else if is_floating {
            // Dialog/Utility/Menu：浮动，但需要焦点和交互
            // 不进入 tiling 避免被强制缩放（缩放会破坏内部菜单坐标系）
            tracing::info!(
                "🪟 X11 floating dialog → overlay (with focus): class='{}'",
                window.class()
            );
            if !self.xw.or_surfaces.iter().any(|s| s.window_id() == wid) {
                // 不要在 map_request 阶段立即确认 (0,0) 初始几何；
                // 等 configure_request / wl_surface 就绪后再接受客户端位置，避免二级弹窗固定到屏幕1左上角。
                self.xw.or_surfaces.push(window.clone());
            }
            // 设置键盘焦点
            if let Some(wl) = window.wl_surface() {
                self.x11_saved_focus = self.workspaces[self.active_ws].focus.clone();
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(wl), serial);
            }
        } else {
            // 普通窗口：跟随 transient_for 父窗口所在的 workspace
            let target_ws = if has_transient {
                let parent = window.is_transient_for().unwrap();
                let mut found_ws = None;
                'search: for ws_i in 0..self.workspaces.len() {
                    for xs in &self.workspaces[ws_i].x11_surfaces {
                        if xs.window_id() == parent {
                            found_ws = Some(ws_i);
                            break 'search;
                        }
                    }
                }
                // 也搜索 or_surfaces（父窗口可能是 Utility 类型的浮动窗）
                if found_ws.is_none() {
                    if let Some(parent_xs) =
                        self.xw.or_surfaces.iter().find(|s| s.window_id() == parent)
                    {
                        let geo = parent_xs.geometry();
                        // 根据父窗口的 X11 坐标推断所在 output/ws
                        for (oi, &ws_i) in self.output_active_ws.iter().enumerate() {
                            if let Some(&(ox, oy, ow, oh)) = self.output_sizes.get(oi) {
                                if geo.loc.x >= ox && geo.loc.x < ox + ow as i32 {
                                    found_ws = Some(ws_i);
                                    break;
                                }
                            }
                        }
                    }
                }
                tracing::info!(
                    "🔗 transient_for search: class='{}' parent_id={} found_ws={:?}",
                    window.class(),
                    parent,
                    found_ws
                );
                found_ws.unwrap_or(self.active_ws)
            } else {
                self.active_ws
            };

            let ws = &mut self.workspaces[target_ws];
            let is_new = !ws.x11_surfaces.iter().any(|s| s.window_id() == wid);
            if is_new {
                // 全屏时打开新窗口 → 立刻退出全屏
                if ws.fullscreen.is_some() {
                    ws.fullscreen = None;
                    info!("🪟 全屏退出（本桌面新 X11 窗口打开）");
                }
                ws.x11_surfaces.push(window.clone());
                ws.rebuild_order();
            }

            if let Some(wl) = window.wl_surface() {
                self.x11_saved_focus = self.workspaces[target_ws].focus.clone();
                self.workspaces[target_ws].focus = Some(wl.clone());
                crate::appctl::emit_event(
                    self,
                    crate::appctl::DesktopEvent::WindowOpened {
                        workspace: target_ws,
                        title: window.title(),
                        app_id: window.class(),
                        kind: "x11".into(),
                    },
                );
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(wl), serial);
            }
            if target_ws == self.active_ws {
                self.do_layout_animated();
            } else {
                self.layout_workspace(target_ws);
            }
        }
        self.dirty = true;
    }

    fn mapped_override_redirect_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        tracing::info!("🗺️  X11 OR mapped: class='{}'", window.class());
        // Re-add in case it was removed by unmapped_window (fcitx5 reuses the same X11 window)
        let wid = window.window_id();
        if !self.xw.or_surfaces.iter().any(|s| s.window_id() == wid) {
            self.xw.or_surfaces.push(window);
        }
        self.dirty = true;
    }

    fn unmapped_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        tracing::info!("🗑️  X11 unmapped: class='{}'", window.class());
        let wid = window.window_id();

        // 检查这个窗口是否在平铺布局中（x11_surfaces 里）
        let was_in_layout = self
            .workspaces
            .iter()
            .any(|ws| ws.x11_surfaces.iter().any(|s| s.window_id() == wid));

        for ws in &mut self.workspaces {
            ws.x11_surfaces.retain(|s| s.window_id() != wid);
            ws.rebuild_order();
        }
        self.xw.or_surfaces.retain(|s| s.window_id() != wid);

        if was_in_layout {
            // 平铺窗口关闭：需要重新布局和 refocus
            self.do_layout_animated();
            let order = self.workspaces[self.active_ws].effective_order();
            if let Some((_, slot)) = order.iter().enumerate().last() {
                let surf = match slot {
                    WindowSlot::Wl(idx) => self.workspaces[self.active_ws]
                        .tops
                        .get(*idx)
                        .map(|tl| tl.wl_surface().clone()),
                    WindowSlot::X11(idx) => self.workspaces[self.active_ws]
                        .x11_surfaces
                        .get(*idx)
                        .and_then(|xs| xs.wl_surface()),
                };
                if let Some(surf) = surf {
                    self.workspaces[self.active_ws].focus = Some(surf.clone());
                    let kbd = self.kbd.clone();
                    let serial = SERIAL_COUNTER.next_serial();
                    kbd.set_focus(self, Some(surf), serial);
                }
            }
        } else {
            // 辅助窗口（候选框、tooltip 等）消失：恢复到辅助窗口出现前的原应用焦点
            // 不做 layout 避免所有 X11 窗口收到不必要的 ConfigureWindow
            if let Some(saved) = self.x11_saved_focus.take() {
                self.workspaces[self.active_ws].focus = Some(saved.clone());
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(saved), serial);
            }
        }
        self.dirty = true;
    }

    fn destroyed_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        tracing::info!("💥 X11 destroyed: class='{}'", window.class());
        let wid = window.window_id();

        // 检查这个窗口是否在平铺布局中
        let was_in_layout = self
            .workspaces
            .iter()
            .any(|ws| ws.x11_surfaces.iter().any(|s| s.window_id() == wid));

        if was_in_layout {
            // 重新映射 prev_positions（X11 窗口索引移位）
            for ws_idx in 0..self.workspaces.len() {
                if let Some(removed_idx) = self.workspaces[ws_idx]
                    .x11_surfaces
                    .iter()
                    .position(|s| s.window_id() == wid)
                {
                    self.remap_prev_after_remove(&WindowSlot::X11(removed_idx));
                }
            }
            for ws in &mut self.workspaces {
                ws.x11_surfaces.retain(|s| s.window_id() != wid);
                ws.rebuild_order();
            }
            self.xw.or_surfaces.retain(|s| s.window_id() != wid);
            self.do_layout_animated();
            // Refocus
            let order = self.workspaces[self.active_ws].effective_order();
            if let Some((_, slot)) = order.iter().enumerate().last() {
                let surf = match slot {
                    WindowSlot::Wl(idx) => self.workspaces[self.active_ws]
                        .tops
                        .get(*idx)
                        .map(|tl| tl.wl_surface().clone()),
                    WindowSlot::X11(idx) => self.workspaces[self.active_ws]
                        .x11_surfaces
                        .get(*idx)
                        .and_then(|xs| xs.wl_surface()),
                };
                if let Some(surf) = surf {
                    self.workspaces[self.active_ws].focus = Some(surf.clone());
                    let kbd = self.kbd.clone();
                    let serial = SERIAL_COUNTER.next_serial();
                    kbd.set_focus(self, Some(surf), serial);
                }
            }
        } else {
            // 辅助窗口销毁：恢复到辅助窗口出现前的原应用焦点
            self.xw.or_surfaces.retain(|s| s.window_id() != wid);
            if let Some(saved) = self.x11_saved_focus.take() {
                self.workspaces[self.active_ws].focus = Some(saved.clone());
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(saved), serial);
            }
        }
        self.dirty = true;
    }

    fn configure_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<smithay::xwayland::xwm::Reorder>,
    ) {
        self.xw.on_configure_request(&window, x, y, w, h);

        // Clamp OR 窗口（输入法候选框等）到屏幕可视区域内
        let is_or = self
            .xw
            .or_surfaces
            .iter()
            .any(|s| s.window_id() == window.window_id());
        if is_or && window.wl_surface().is_some() {
            let geo = window.geometry();
            let new_x = x.unwrap_or(geo.loc.x);
            let new_y = y.unwrap_or(geo.loc.y);
            let new_w = w.map(|v| v as i32).unwrap_or(geo.size.w);
            let new_h = h.map(|v| v as i32).unwrap_or(geo.size.h);

            let margin: i32 = 8;
            let (clamp_x, clamp_y) = self.clamp_global_rect_to_output(
                new_x,
                new_y,
                new_w.max(100),
                new_h.max(20),
                margin,
            );

            if clamp_x != new_x || clamp_y != new_y {
                let _ = window.configure(Some(Rectangle::from_loc_and_size(
                    (clamp_x, clamp_y),
                    (new_w, new_h),
                )));
            }
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<smithay::xwayland::xwm::X11Window>,
    ) {
    }

    fn resize_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        _button: u32,
        _resize_edge: smithay::xwayland::xwm::ResizeEdge,
    ) {
        self.xw.ack_with_current_geometry(&window);
    }

    fn move_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        _button: u32,
    ) {
        self.xw.ack_with_current_geometry(&window);
    }

    fn send_selection(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        selection: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: std::os::unix::io::OwnedFd,
    ) {
        // X11 客户端请求 Wayland 选区数据
        // 两种情况：
        //   1) Wayland 客户端拥有选区 → request_data_device_client_selection 转发请求
        //   2) 合成器自身拥有选区（如截图剪贴板）→ request_data_device_client_selection
        //      返回 ServerSideSelection 错误，需要手动从 seat user_data 读取并写入 fd
        use smithay::wayland::selection::data_device::{
            current_data_device_selection_userdata, request_data_device_client_selection,
        };

        if selection == smithay::wayland::selection::SelectionTarget::Clipboard {
            // 先检查合成器是否拥有选区（截图等 compositor-provided selection）
            if let Some(user_data) = current_data_device_selection_userdata::<App>(&self.seat) {
                tracing::info!("📋 XwmHandler::send_selection: compositor owns clipboard, writing {} bytes to fd", user_data.len());
                let buf: Arc<[u8]> = user_data.clone();
                std::thread::spawn(move || {
                    use std::io::Write;
                    if let Err(err) = smithay::reexports::rustix::fs::fcntl_setfl(
                        &fd,
                        smithay::reexports::rustix::fs::OFlags::empty(),
                    ) {
                        tracing::warn!("error clearing flags on selection fd: {:?}", err);
                    }
                    if let Err(err) = std::fs::File::from(fd).write_all(&buf) {
                        tracing::warn!("error writing compositor selection to X11 fd: {:?}", err);
                    }
                });
                return;
            }
        }

        // Wayland 客户端拥有选区 → 请求客户端发送数据
        match request_data_device_client_selection::<App>(&self.seat, mime_type, fd) {
            Ok(()) => tracing::info!("📋 XwmHandler::send_selection: forwarded to Wayland client"),
            Err(e) => tracing::warn!("📋 XwmHandler::send_selection: request failed: {:?}", e),
        }
    }

    fn maximize_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        self.xw.ack_with_current_geometry(&window);
    }

    fn unmaximize_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        self.xw.ack_with_current_geometry(&window);
    }

    fn fullscreen_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        let _ = window.set_fullscreen(true);
        let wid = window.window_id();
        for ws_idx in 0..self.workspaces.len() {
            let ws = &self.workspaces[ws_idx];
            let order = ws.effective_order();
            for (i, slot) in order.iter().enumerate() {
                // 在内层作用域完成所有不可变借用，结束后再可变借用 self.workspaces
                let (is_match, focus_surf) = {
                    let m = match slot {
                        WindowSlot::Wl(_) => false,
                        WindowSlot::X11(idx) => ws
                            .x11_surfaces
                            .get(*idx)
                            .map(|s| s.window_id() == wid)
                            .unwrap_or(false),
                    };
                    let focus = if m {
                        order.get(i).and_then(|s2| match s2 {
                            WindowSlot::X11(idx) => {
                                ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface())
                            }
                            _ => None,
                        })
                    } else {
                        None
                    };
                    (m, focus)
                };
                if is_match {
                    self.workspaces[ws_idx].fullscreen = Some(i);
                    if ws_idx != self.active_ws {
                        self.active_ws = ws_idx;
                    }
                    // 同步 ws.focus 到全屏 X11 窗口（防穿透）
                    if let Some(wl) = focus_surf {
                        self.workspaces[ws_idx].focus = Some(wl);
                    }
                    self.do_layout_animated();
                    self.dirty = true;
                    return;
                }
            }
        }
    }

    fn unfullscreen_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        let _ = window.set_fullscreen(false);
        let wid = window.window_id();
        for ws_idx in 0..self.workspaces.len() {
            let ws = &self.workspaces[ws_idx];
            if let Some(fi) = ws.fullscreen {
                let order = ws.effective_order();
                let matches = match order.get(fi) {
                    Some(WindowSlot::X11(idx)) => ws
                        .x11_surfaces
                        .get(*idx)
                        .map(|s| s.window_id() == wid)
                        .unwrap_or(false),
                    _ => false,
                };
                if matches {
                    self.workspaces[ws_idx].fullscreen = None;
                    self.do_layout_animated();
                    self.dirty = true;
                    return;
                }
            }
        }
    }

    fn minimize_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
    ) {
    }
    fn unminimize_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
    ) {
    }

    fn allow_selection_access(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _selection: smithay::wayland::selection::SelectionTarget,
    ) -> bool {
        // Wayland->X11: allow X11 clients to access Wayland selection
        true
    }

    fn new_selection(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        selection: smithay::wayland::selection::SelectionTarget,
        mime_types: Vec<String>,
    ) {
        // X11→Wayland: X11 客户端设了新选区 → 在 Wayland 端注册为 compositor 选区
        // 使用 set_data_device_selection 让 Anchor 成为 Wayland data device selection owner
        // Wayland 客户端粘贴时 SelectionHandler::send_selection 会被调，通过 X11Wm::send_selection 获取 X11 数据
        if selection == smithay::wayland::selection::SelectionTarget::Clipboard
            && !mime_types.is_empty()
        {
            use smithay::wayland::selection::data_device::set_data_device_selection;
            // 使用 magic bytes 标记这是 X11 代理选区
            // SelectionHandler::send_selection 检测到这个标记会用 X11Wm::send_selection 获取实际数据
            let user_data: Arc<[u8]> = Arc::from(&b"X11_PROXY\x00"[..]);
            set_data_device_selection::<App>(&self.dh, &self.seat, mime_types, user_data);
            tracing::info!("X11→Wayland: registered X11 selection as compositor selection");
        }
    }

    fn cleared_selection(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _selection: smithay::wayland::selection::SelectionTarget,
    ) {
    }

    fn property_notify(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        property: smithay::xwayland::xwm::WmWindowProperty,
    ) {
        match property {
            smithay::xwayland::xwm::WmWindowProperty::Title => {
                tracing::info!("📝 X11 title: '{}'", window.title());
            }
            smithay::xwayland::xwm::WmWindowProperty::Class => {
                tracing::info!("🆔 X11 class: '{}'", window.class());
            }
            _ => {}
        }
    }

    fn disconnected(&mut self, _xwm: smithay::xwayland::xwm::XwmId) {
        tracing::warn!("⚠️  X11 WM disconnected");
        self.xw.xwm = None;
    }
}

// ── XDG Decoration Handler ──────────────────────────────────
// 支持 SSD（服务端装饰）和 CSD（客户端装饰）混合模式。
// 默认使用 SSD，但当客户端请求 CSD 时允许切换。
// 客户端使用 CSD 时将自己在 toplevel 中渲染 header bar。
impl XdgDecorationHandler for App {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        // 默认使用 ServerSide 装饰
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        // 初始化 header bar 数据
        ensure_header_bar_data(&toplevel);
        toplevel.send_configure();
    }
    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        ensure_header_bar_data(&toplevel);

        match mode {
            Mode::ClientSide => {
                // 允许客户端使用 CSD（自行渲染 header bar）
                toplevel.with_pending_state(|state| {
                    state.decoration_mode = Some(Mode::ClientSide);
                });
                set_client_decoration(&toplevel);
            }
            Mode::ServerSide => {
                // 客户端请求 SSD
                toplevel.with_pending_state(|state| {
                    state.decoration_mode = Some(Mode::ServerSide);
                });
            }
            _ => {
                // 未知模式，默认 SSD
                toplevel.with_pending_state(|state| {
                    state.decoration_mode = Some(Mode::ServerSide);
                });
            }
        }
        toplevel.send_configure();
        self.do_layout();
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        // 客户端取消装饰模式设置 → 回退到 SSD
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }
}

// ── XDG Activation Handler ──────────────────────────────────
// 实现 xdg-activation-v1 协议，支持跨应用焦点激活
// （例如：浏览器点击链接 → 打开/激活对应应用窗口）

impl XdgActivationHandler for App {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        let elapsed = token_data.timestamp.elapsed().as_secs();
        if elapsed > 30 {
            info!(
                "⏰ XDG activation token expired ({}s old), ignoring",
                elapsed
            );
            return;
        }

        let app_id_hint = token_data.app_id.clone().unwrap_or_default();
        info!(
            "🎯 XDG activation request: app_id={:?}, token_age={}s",
            app_id_hint, elapsed,
        );
        self.xdg_activation.remove_token(&token);

        // 第一阶段：查找目标 surface 所在的工作区
        let wl_surf = surface;
        let mut target: Option<(usize, WlSurface)> = None;

        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            // Wayland toplevels
            for tl in &ws.tops {
                if tl.wl_surface() == &wl_surf {
                    target = Some((ws_idx, wl_surf.clone()));
                    break;
                }
            }
            if target.is_some() {
                break;
            }
            // X11 surfaces
            for xs in &ws.x11_surfaces {
                if xs.wl_surface().as_ref() == Some(&wl_surf) {
                    target = Some((ws_idx, wl_surf.clone()));
                    break;
                }
            }
            if target.is_some() {
                break;
            }
        }

        // 第二阶段：执行焦点切换
        if let Some((ws_idx, surf)) = target {
            if ws_idx != self.active_ws {
                self.switch_workspace(ws_idx);
            }
            self.workspaces[ws_idx].focus = Some(surf.clone());
            let serial = SERIAL_COUNTER.next_serial();
            let kbd = self.kbd.clone();
            let _ = kbd.set_focus(self, Some(surf), serial);
            self.dirty = true;
            info!("🎯 XDG activation: activated toplevel in ws {}", ws_idx);
            return;
        }

        // 检查 pending_tops
        for tl in &self.pending_tops {
            if tl.wl_surface() == &wl_surf {
                info!("🎯 Activation target found in pending_tops, will auto-focus when app_id arrives");
                return;
            }
        }
        info!("🎯 XDG activation: target surface not found in workspaces");
    }
}

// ── Anchor Header Bar Protocol Handler ──────────────────────
// 实现 anchor-header-bar-v1 协议的 Wayland Dispatch

use headerbar::{anchor_header_bar_manager_v1, anchor_header_bar_v1};
use wayland_server::backend::ObjectId;
use wayland_server::Dispatch;
use wayland_server::Resource as _Resource;

// ── GlobalDispatch: anchor_header_bar_manager_v1 ──
// 必须实现 GlobalDispatch 才能创建 global

impl wayland_server::GlobalDispatch<anchor_header_bar_manager_v1::AnchorHeaderBarManagerV1, ()>
    for App
{
    fn bind(
        _state: &mut Self,
        _dh: &smithay::reexports::wayland_server::DisplayHandle,
        _client: &wayland_server::Client,
        resource: wayland_server::New<anchor_header_bar_manager_v1::AnchorHeaderBarManagerV1>,
        _global_data: &(),
        _data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        let _manager = _data_init.init(resource, ());
        tracing::info!("🏷️ Header bar protocol: client bound manager");
    }
}

// ── Dispatch: anchor_header_bar_manager_v1 ──

impl Dispatch<anchor_header_bar_manager_v1::AnchorHeaderBarManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _client: &wayland_server::Client,
        _resource: &anchor_header_bar_manager_v1::AnchorHeaderBarManagerV1,
        request: <anchor_header_bar_manager_v1::AnchorHeaderBarManagerV1 as wayland_server::Resource>::Request,
        _data: &(),
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        match request {
            anchor_header_bar_manager_v1::Request::GetHeaderBar { id, toplevel } => {
                let toplevel_id = toplevel.id();
                let header_bar = data_init.init(id, toplevel_id.clone());
                // 发送初始 configured 事件（高度为 0，表示尚未设置）
                header_bar.configured(0);
                tracing::info!(
                    "🏷️ Header bar protocol: client requested header bar for toplevel {:?}",
                    toplevel_id
                );
            }
            anchor_header_bar_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

// ── Dispatch: anchor_header_bar_v1 ──

impl Dispatch<anchor_header_bar_v1::AnchorHeaderBarV1, ObjectId> for App {
    fn request(
        state: &mut Self,
        _client: &wayland_server::Client,
        resource: &anchor_header_bar_v1::AnchorHeaderBarV1,
        request: <anchor_header_bar_v1::AnchorHeaderBarV1 as wayland_server::Resource>::Request,
        toplevel_id: &ObjectId,
        _dhandle: &smithay::reexports::wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        match request {
            anchor_header_bar_v1::Request::SetHeight { height } => {
                if height < 0 {
                    resource.post_error(
                        anchor_header_bar_v1::Error::InvalidHeight as u32,
                        "Header bar height must be non-negative",
                    );
                    return;
                }
                // 查找对应的 ToplevelSurface
                let found = state
                    .workspaces
                    .iter()
                    .flat_map(|ws| ws.tops.iter())
                    .chain(state.pending_tops.iter())
                    .find(|tl| tl.wl_surface().id() == *toplevel_id)
                    .cloned();
                if let Some(tl) = found {
                    ensure_header_bar_data(&tl);
                    let actual_height = if height > 0 { height } else { 0 };
                    set_header_bar_height(&tl, actual_height);
                    resource.configured(actual_height);
                    tracing::info!(
                        "🏷️ Header bar height set to {} for toplevel {:?}",
                        actual_height,
                        toplevel_id
                    );
                    state.do_layout();
                    state.dirty = true;
                } else {
                    tracing::warn!("🏷️ Header bar: toplevel {:?} not found", toplevel_id);
                }
            }
            anchor_header_bar_v1::Request::Destroy => {}
            _ => {}
        }
    }
}
