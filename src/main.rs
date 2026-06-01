// Anchor — Wayland tiling compositor v9
// Features: multi-workspace, multi-monitor, wallpaper, config
// Config: ~/.config/anchor/config.toml

mod config;
mod layout;
use layout::LayoutPreset;
mod text_render;
mod block_linear;
mod wallpaper;
mod cursor;
mod notify;
mod xwayland;
mod screenshot;
mod auth;
mod workspace;
use workspace::{Workspace, WindowSlot, NUM_WORKSPACES};
mod lock;
use lock::LockState;
mod launcher;
use launcher::LauncherState;
mod scratchpad;
use scratchpad::ScratchpadState;

use std::{
    os::unix::io::OwnedFd,
    os::fd::AsRawFd,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use config::Config;
use smithay::{
    backend::{
        allocator::{Format, Fourcc, Modifier,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice}},
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, GbmBufferedSurface},
        input::{Axis, ButtonState, InputEvent, KeyState, PointerAxisEvent},
        libinput::LibinputInputBackend,
        renderer::{ImportDma, ImportMem, ExportMem, Bind, Frame, Renderer,
            gles::GlesRenderer,
            element::{RenderElement, surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement}, Kind},
            utils::{draw_render_elements, on_commit_buffer_handler}, Color32F},
        session::{Session, Event as SessionEvent, libseat::LibSeatSession},
    },
    desktop::{PopupManager, PopupKind},    delegate_compositor, delegate_data_device, delegate_input_method_manager,
    delegate_output, delegate_primary_selection, delegate_seat, delegate_shm, delegate_text_input_manager,
    delegate_virtual_keyboard_manager, delegate_xdg_decoration, delegate_xdg_shell,
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState, XkbConfig},
        pointer::{AxisFrame, CursorImageStatus, MotionEvent, PointerHandle}, Seat, SeatHandler, SeatState,
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        drm::control::{connector, crtc, Device as _},
        wayland_server::{Display, DisplayHandle,
            protocol::{wl_seat, wl_surface::WlSurface}},
    },
    utils::{DeviceFd, Logical, Physical, Point, Rectangle, SERIAL_COUNTER,
        Size, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{with_surface_tree_downward, CompositorClientState, CompositorHandler,
            CompositorState, SurfaceAttributes, TraversalAction},
        input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface as ImPopupSurface},
        output::OutputManagerState,
        selection::{
            SelectionHandler,
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler},
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
        },
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            decoration::{XdgDecorationState, XdgDecorationHandler}},
        shm::{ShmHandler, ShmState},
        text_input::TextInputManagerState,
        virtual_keyboard::VirtualKeyboardManagerState,
    },
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{Client, ListeningSocket, Resource,
    backend::{ClientData, ClientId, DisconnectReason}, protocol::wl_buffer};
use tracing::{error, info, warn};

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
}

// ── App ──────────────────────────────────────────────────

struct Notification {
    text: String,
    created: std::time::Instant,
    duration: std::time::Duration,
}

struct App {
    comp: CompositorState, xdg: XdgShellState, shm: ShmState, seat_state: SeatState<Self>,
    dd: DataDeviceState, primary_sel: PrimarySelectionState, seat: Seat<Self>,
    deco: XdgDecorationState,
    popup_manager: PopupManager,
    osize: Size<i32, Logical>,
    workspaces: Vec<Workspace>,
    active_ws: usize,
    run: bool, frame: u32,
    dh: DisplayHandle, active: bool,
    dirty: bool,
    kbd: smithay::input::keyboard::KeyboardHandle<Self>,
    pointer: PointerHandle<Self>,
    pointer_pos: (f64, f64),
    cfg: Config,
    cursor_img: cursor::CursorImage,
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
    /// 毛玻璃模糊纹理缓存
    launcher_blur_tex: Option<smithay::backend::renderer::gles::GlesTexture>,
    /// 毛玻璃纹理尺寸
    launcher_blur_size: (u32, u32),
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
    // CPU/MEM 统计（headbar 显示用）
    cpu_usage: f32,        // 0.0 ~ 1.0
    mem_usage: f32,        // 0.0 ~ 1.0
    cpu_prev_idle: u64,
    cpu_prev_total: u64,
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
        Self { start: None, duration_ms: 350, old_positions: Vec::new() }
    }

    fn begin(&mut self, positions: &[(crate::workspace::WindowSlot, (i32, i32))]) {
        self.old_positions = positions.to_vec();
        self.start = Some(std::time::Instant::now());
    }

    fn offset_for(&self, slot: &crate::workspace::WindowSlot, target: (i32, i32)) -> Option<(i32, i32)> {
        let start = self.start?;
        let elapsed = start.elapsed().as_millis() as u64;
        if elapsed >= self.duration_ms { return None; }
        let old = self.old_positions.iter().find(|(s, _)| {
            match (s, slot) {
                (crate::workspace::WindowSlot::Wl(a), crate::workspace::WindowSlot::Wl(b)) => a == b,
                (crate::workspace::WindowSlot::X11(a), crate::workspace::WindowSlot::X11(b)) => a == b,
                _ => false,
            }
        })?.1;
        let t = elapsed as f32 / self.duration_ms as f32;
        let t_ease = 1.0 - (1.0 - t).powi(3);
        let dx = (old.0 - target.0) as f32 * (1.0 - t_ease);
        let dy = (old.1 - target.1) as f32 * (1.0 - t_ease);
        Some((dx as i32, dy as i32))
    }

    fn is_active(&self) -> bool {
        match self.start { Some(s) => { let ms: u64 = s.elapsed().as_millis() as u64; ms < self.duration_ms }, None => false }
    }
}

impl BufferHandler for App { fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {} }

impl App {
    /// 当前工作区的窗口列表
    fn tops(&self) -> &Vec<ToplevelSurface> { &self.workspaces[self.active_ws].tops }
    fn tops_mut(&mut self) -> &mut Vec<ToplevelSurface> { &mut self.workspaces[self.active_ws].tops }

    fn focus_idx(&self) -> Option<usize> {
        let ws = &self.workspaces[self.active_ws];
        let focus = ws.focus.as_ref()?;
        let order = ws.effective_order();
        for (i, slot) in order.iter().enumerate() {
            let matches = match slot {
                WindowSlot::Wl(idx) => ws.tops.get(*idx).map(|tl| tl.wl_surface() == focus),
                WindowSlot::X11(idx) => ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface().map(|wl| &wl == focus)),
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

    /// 获取鼠标所在 output 的活跃工作区索引
    fn active_ws_for_pointer(&self) -> usize {
        // 用当前 active_ws（键盘焦点）— 后续会改为按 output 分配
        self.active_ws
    }

    fn pointer_focus(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        // 找到鼠标所在的 output，将全局坐标转为 output 局部坐标
        let oi = self.output_at_pointer();
        let (ox, oy, ow, oh) = self.output_sizes.get(oi).copied().unwrap_or((0, 0, self.osize.w, self.osize.h));
        let px = self.pointer_pos.0 - ox as f64;
        let py = self.pointer_pos.1 - oy as f64;

        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        if py < bar_h as f64 { return None; }

        // 使用此 output 的 active_ws（不是全局的）
        let out_ws_idx = self.output_active_ws.get(oi).copied().unwrap_or(self.active_ws);
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
                            if let Some(r) = self.popup_at_pointer(tl, tl_pos) { return Some(r); }
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
                    let (x, y, _, _) = layout::slot(i, order.len(), ow, oh, bar_h, &self.cfg, ws.layout, ws.split);
                    let tl_geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                        states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                    }).unwrap_or_default();
                    let tl_pos = Point::from((x as f64 - tl_geo.loc.x as f64, y as f64 - tl_geo.loc.y as f64));
                    if let Some(r) = self.popup_at_pointer(tl, tl_pos) {
                        return Some(r);
                    }
                }
            }
        }

        // Hit-test window slots using unified order
        let n_all = order.len();
        for (i, slot) in order.iter().enumerate() {
            let (x, y, w, h) = layout::slot(i, n_all, ow, oh, bar_h, &self.cfg, ws.layout, ws.split);
            if px >= x as f64 && px < (x + w) as f64 && py >= y as f64 && py < (y + h) as f64 {
                match slot {
                    WindowSlot::Wl(idx) => {
                        if let Some(tl) = ws.tops.get(*idx) {
                            let s = tl.wl_surface().clone();
                            // 获取 geometry 偏移（CSD 阴影/边框），渲染位置需减去它
                            let tl_geo = smithay::wayland::compositor::with_states(&s, |states| {
                                states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                            }).unwrap_or_default();
                            let bx = x as f64 - tl_geo.loc.x as f64;
                            let by = y as f64 - tl_geo.loc.y as f64;
                            let local_pos = Point::from((px - bx, py - by));
                            if let Some((sub, sub_loc)) = smithay::desktop::utils::under_from_surface_tree(
                                &s,
                                local_pos,
                                (0, 0),
                                smithay::desktop::WindowSurfaceType::ALL,
                            ) {
                                let offset = Point::from((bx + sub_loc.x as f64, by + sub_loc.y as f64));
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

        // Last resort: last window in order
        if let Some((i, slot)) = order.iter().enumerate().last() {
            let (x, y, _w, _h) = layout::slot(i, order.len(), self.osize.w, self.osize.h, bar_h, &self.cfg, ws.layout, ws.split);
            match slot {
                WindowSlot::Wl(idx) => {
                    if let Some(tl) = ws.tops.get(*idx) {
                        let s = tl.wl_surface().clone();
                        let geo = smithay::wayland::compositor::with_states(&s, |states| {
                            states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                        }).unwrap_or_default();
                        return Some((s, Point::from((x as f64 - geo.loc.x as f64, y as f64 - geo.loc.y as f64))));
                    }
                }
                WindowSlot::X11(idx) => {
                    if let Some(s) = ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()) {
                        return Some((s, Point::from((x as f64, y as f64))));
                    }
                }
            }
        }

        None
    }

    /// Check if the pointer is over any XDG popup of the given toplevel.
    /// Returns (popup_wl_surface, popup_global_position) if found.
    fn popup_at_pointer(&self, tl: &ToplevelSurface, tl_pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let px = self.pointer_pos.0;
        let py = self.pointer_pos.1;

        let tl_geo_loc = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
            states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                .map(|g| g.loc)
        }).unwrap_or_default();

        let popups: Vec<_> = PopupManager::popups_for_surface(tl.wl_surface()).collect();
        if popups.is_empty() { return None; }
        for (popup, popup_offset) in popups {
            let popup_geo = popup.geometry();
            let offset_x = (tl_geo_loc.x + popup_offset.x - popup_geo.loc.x) as f64;
            let offset_y = (tl_geo_loc.y + popup_offset.y - popup_geo.loc.y) as f64;
            let popup_x = tl_pos.x + offset_x;
            let popup_y = tl_pos.y + offset_y;
            let popup_w = popup_geo.size.w as f64;
            let popup_h = popup_geo.size.h as f64;

            if px >= popup_x && px < popup_x + popup_w && py >= popup_y && py < popup_y + popup_h {
                return Some((popup.wl_surface().clone(), Point::from((popup_x, popup_y))));
            }
        }
        None
    }

    fn fullscreen(&self) -> Option<usize> { self.workspaces[self.active_ws].fullscreen }
    fn set_fullscreen(&mut self, v: Option<usize>) { self.workspaces[self.active_ws].fullscreen = v; }

    /// 布局指定工作区的所有窗口
    fn layout_workspace(&mut self, ws_idx: usize) {
        self.workspaces[ws_idx].rebuild_order();
        let order = self.workspaces[ws_idx].effective_order();
        let n = order.len();
        if n == 0 { return; }
        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };

        if let Some(fi) = self.workspaces[ws_idx].fullscreen {
            if fi >= n { self.workspaces[ws_idx].fullscreen = None; }
        }
        let fullscreen = self.workspaces[ws_idx].fullscreen;
        let osize_w = self.osize.w;
        let osize_h = self.osize.h;

        if let Some(fi) = fullscreen {
            for (i, slot) in order.iter().enumerate() {
                match slot {
                    WindowSlot::Wl(idx) => {
                        if let Some(tl) = self.workspaces[ws_idx].tops.get(*idx) {
                            if i == fi {
                                tl.with_pending_state(|st| {
                                    st.states.set(xdg_toplevel::State::Activated);
                                    st.states.set(xdg_toplevel::State::Fullscreen);
                                    st.size = Some((osize_w, osize_h - bar_h).into());
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
                                let _ = xs.configure(Some(Rectangle::from_loc_and_size((0, bar_h), (osize_w, osize_h - bar_h))));
                            } else {
                                let _ = xs.configure(Some(Rectangle::from_loc_and_size((0, 0), (1, 1))));
                            }
                        }
                    }
                }
            }
        } else {
            for (i, slot) in order.iter().enumerate() {
                let (x, y, w, h) = layout::slot(i, n, osize_w, osize_h, bar_h, &self.cfg, self.workspaces[ws_idx].layout, self.workspaces[ws_idx].split);
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
                            let _ = xs.configure(Some(Rectangle::from_loc_and_size((x, y), (w, h))));
                        }
                    }
                }
            }
        }

        // 保存每个窗口的身份 + 位置（供下次动画使用）
        if ws_idx == self.active_ws {
            let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
            let order = self.workspaces[ws_idx].effective_order();
            self.prev_positions = (0..n).map(|i| {
                let (x, y, _, _) = layout::slot(i, n, self.osize.w, self.osize.h, bar_h, &self.cfg, self.workspaces[ws_idx].layout, self.workspaces[ws_idx].split);
                (order[i].clone(), (x, y))
            }).collect();
        }
    }

    fn do_layout(&mut self) {
        self.layout_workspace(self.active_ws);
    }

    /// 在 tops.remove/x11_surfaces.remove 之后重新映射 prev_positions
    /// 因为 remove 导致索引移位: Wl(1) 变成 Wl(0), 但 prev_positions 还映射旧索引
    fn remap_prev_after_remove(&mut self, removed: &WindowSlot) {
        let removed_clone = removed.clone();
        self.prev_positions.retain(|(slot, _)| {
            match (slot, &removed_clone) {
                (WindowSlot::Wl(a), WindowSlot::Wl(b)) => a != b,
                (WindowSlot::X11(a), WindowSlot::X11(b)) => a != b,
                _ => true,
            }
        });
        match removed {
            WindowSlot::Wl(removed_idx) => {
                for (slot, _) in &mut self.prev_positions {
                    if let WindowSlot::Wl(ref mut idx) = slot {
                        if *idx > *removed_idx { *idx -= 1; }
                    }
                }
            }
            WindowSlot::X11(removed_idx) => {
                for (slot, _) in &mut self.prev_positions {
                    if let WindowSlot::X11(ref mut idx) = slot {
                        if *idx > *removed_idx { *idx -= 1; }
                    }
                }
            }
        }
    }

    /// 触发布局动画 + 重新布局
    fn do_layout_animated(&mut self) {
        // 保存动画起点（每个窗口的身份 + 位置）
        let old_positions = self.prev_positions.clone();

        // 执行布局
        self.layout_workspace(self.active_ws);

        // 为新窗口（在 prev_positions 中没有记录的）填充假的旧位置
        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        let split = self.workspaces[self.active_ws].split;
        let new_positions = self.prev_positions.clone(); // layout_workspace 刚更新
        let mut anim_positions = old_positions;

        // 为新增的窗口添加假旧位置（从 split 方向滑入）
        for (slot, new_pos) in &new_positions {
            let already_tracked = anim_positions.iter().any(|(s, _)| {
                match (s, slot) {
                    (WindowSlot::Wl(a), WindowSlot::Wl(b)) => a == b,
                    (WindowSlot::X11(a), WindowSlot::X11(b)) => a == b,
                    _ => false,
                }
            });
            if !already_tracked {
                let fake_pos = match split {
                    layout::SplitDir::Horizontal => (new_pos.0 + self.osize.w + 100, new_pos.1),
                    layout::SplitDir::Vertical => (new_pos.0, new_pos.1 + self.osize.h + 100),
                };
                anim_positions.push((slot.clone(), fake_pos));
            }
        }

        // 启动动画
        if !anim_positions.is_empty() {
            self.layout_anim.begin(&anim_positions);
            self.dirty = true;
        }
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notifications.push(Notification {
            text: text.into(),
            created: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(3),
        });
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
        set_data_device_selection::<App>(
            &self.dh,
            &self.seat,
            mime_types.clone(),
            user_data,
        );
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
        self.notifications.retain(|n| now.duration_since(n.created) < n.duration);
    }

    /// Read CPU usage from /proc/stat (delta-based)
    fn update_cpu_usage(&mut self) {
        if let Ok(data) = std::fs::read_to_string("/proc/stat") {
            if let Some(line) = data.lines().next() {
                let fields: Vec<u64> = line.split_whitespace()
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
                    mem_total = line.split_whitespace()
                        .nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                } else if line.starts_with("MemAvailable:") {
                    mem_available = line.split_whitespace()
                        .nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                }
                if mem_total > 0 && mem_available > 0 { break; }
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
                        WindowSlot::X11(i) => ws.x11_surfaces.get(*i).and_then(|xs| xs.wl_surface()),
                    })
                };
                if let Some(surf) = focus_surf {
                    self.workspaces[self.active_ws].focus = Some(surf);
                }
            }
            _ => return,
        }
        self.do_layout_animated();
        self.dirty = true;
    }

    /// 切换到指定工作区（只替换鼠标所在 output 的工作区）
    fn switch_workspace(&mut self, target: usize) {
        if target >= NUM_WORKSPACES { return; }

        let out_idx = self.focused_output;

        // 检查目标工作区是否已经在某个 output 上显示
        // 如果是，把鼠标移到那个 output 即可
        for (oi, ws) in self.output_active_ws.iter().enumerate() {
            if *ws == target {
                if oi != out_idx {
                    // 目标工作区在另一个屏幕上，移鼠标过去
                    let (ox, oy, ow, oh) = self.output_sizes.get(oi).copied().unwrap_or_default();
                    self.pointer_pos = (ox as f64 + ow as f64 / 2.0, oy as f64 + oh as f64 / 2.0);
                    self.focused_output = oi;
                    self.active_ws = target;
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
        if target == old_ws { return; }
        info!("🔀 屏幕 {} 工作区 {} → {}", out_idx + 1, old_ws + 1, target + 1);

        // 触发切换动画
        let dir = if target > old_ws { 1 } else { -1 };
        self.ws_anim = WsAnimation {
            start: Some(std::time::Instant::now()),
            from_ws: old_ws,
            to_ws: target,
            duration_ms: 200,
            direction: dir,
        };

        // 隐藏旧工作区的窗口（最小化到 1x1）
        for tl in &self.workspaces[old_ws].tops {
            tl.with_pending_state(|st| {
                st.states.unset(xdg_toplevel::State::Activated);
                st.states.unset(xdg_toplevel::State::Fullscreen);
                st.size = Some((1, 1).into());
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

    /// 将当前焦点窗口移动到目标工作区，然后跟随窗口切换到目标工作区
    /// 支持 Wayland (tops) 和 X11 (x11_surfaces) 窗口
    fn move_window_to_workspace(&mut self, target: usize) {
        if target >= NUM_WORKSPACES { return; }
        let out_idx = self.focused_output;
        let ws_idx = self.output_active_ws.get(out_idx).copied().unwrap_or(0);
        if target == ws_idx { return; }
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
            WindowSlot::Wl(idx) => self.workspaces[ws_idx].tops.get(*idx).map(|tl| tl.wl_surface().clone()),
            WindowSlot::X11(idx) => self.workspaces[ws_idx].x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()),
        };
        let surf = match surf {
            Some(s) => s,
            None => return,
        };

        info!("📦 移动窗口 slot {:?} (order #{}) → 工作区 {}", slot, fi, target + 1);

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
                self.workspaces[ws_idx].focus = self.workspaces[ws_idx].tops.last().map(|t| t.wl_surface().clone());

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
                        WindowSlot::Wl(i) => self.workspaces[ws_idx].tops.get(*i).map(|tl| tl.wl_surface().clone()),
                        WindowSlot::X11(i) => self.workspaces[ws_idx].x11_surfaces.get(*i).and_then(|x| x.wl_surface()),
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
            self.active_ws = target;
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
        if n == 0 { return; }

        let fi = match self.focus_idx() {
            Some(i) => i,
            None => 0,
        };

        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        let slots: Vec<(i32, i32, i32, i32)> = (0..n)
            .map(|i| layout::slot(i, n, self.osize.w, self.osize.h, bar_h, &self.cfg, self.workspaces[self.active_ws].layout, self.workspaces[self.active_ws].split))
            .collect();

        let (fx, fy, fw, fh) = slots[fi];
        let fcx = fx + fw / 2;
        let fcy = fy + fh / 2;

        let mut best_idx: Option<usize> = None;
        let mut best_dist: i32 = i32::MAX;

        for (i, &(sx, sy, sw, sh)) in slots.iter().enumerate() {
            if i == fi { continue; }
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
    /// 按屏幕真实几何位置：找到当前窗口在指定方向上最近的邻居
    fn swap_window(&mut self, direction: Keysym) {
        let fi = match self.focus_idx() {
            Some(i) => i,
            None => return,
        };
        let ws = &mut self.workspaces[self.active_ws];
        ws.rebuild_order();
        let order = ws.effective_order();
        let n = order.len();
        if n <= 1 { return; }

        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        // 计算所有窗口的 slot 位置
        let slots: Vec<(i32, i32, i32, i32)> = (0..n)
            .map(|i| layout::slot(i, n, self.osize.w, self.osize.h, bar_h, &self.cfg, ws.layout, ws.split))
            .collect();

        let (fx, fy, fw, fh) = slots[fi];
        // 焦点窗口中心
        let fcx = fx + fw / 2;
        let fcy = fy + fh / 2;

        // 找到在指定方向上最近的窗口
        let mut best_idx: Option<usize> = None;
        let mut best_dist: i32 = i32::MAX;

        for (i, &(sx, sy, sw, sh)) in slots.iter().enumerate() {
            if i == fi { continue; }
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

        info!("🔄 交换窗口 {} ↔ {} (方向感知)", fi + 1, target + 1);
        ws.window_order.swap(fi, target);

        if let Some(fs) = ws.fullscreen {
            if fs == fi { ws.fullscreen = Some(target); }
            else if fs == target { ws.fullscreen = Some(fi); }
        }

        drop(ws);
        self.do_layout_animated();
        self.dirty = true;
    }

    fn handle_input_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        use smithay::backend::input::{KeyboardKeyEvent as _, PointerButtonEvent as _, PointerMotionEvent as _, Event as _};
        match event {
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let state = event.state();
                let time = (event.time() / 1000) as u32;
                let serial = SERIAL_COUNTER.next_serial();
                let kbd = self.kbd.clone();
                let _ = smithay::input::keyboard::KeyboardHandle::<Self>::input(
                    &kbd, self, keycode, state, serial, time,
                    |data: &mut App, mods: &ModifiersState, keysym: smithay::input::keyboard::KeysymHandle<'_>| {
                        // ── 截图区域选择模式键盘处理 ──
                        if data.screenshot.selecting && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            if sym == Keysym::Escape {
                                data.screenshot.cancel();
                                data.dirty = true;
                                return FilterResult::Intercept(());
                            }
                        }
                        // ── 锁屏模式键盘处理 ──
                        if data.lock_state.locked && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            match sym {
                                Keysym::Escape => { data.lock_state.clear(); data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::Return => {
                                    data.lock_state.try_unlock();
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::BackSpace => { data.lock_state.backspace(); data.dirty = true; return FilterResult::Intercept(()); }
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
                        if data.lock_state.locked { return FilterResult::Intercept(()); }
                        // ── 启动器模式键盘处理 ──
                        if data.launcher.visible && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            match sym {
                                Keysym::Escape => { data.launcher.close(); data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::Return => { data.launcher.select_and_launch(data.xdisplay); data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::Up => { data.launcher.select_up(); data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::Down => { data.launcher.select_down(); data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::BackSpace => { data.launcher.backspace(); data.dirty = true; return FilterResult::Intercept(()); }
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
                        
                        if state == KeyState::Pressed && mods.logo {
                            let uid = unsafe { libc::getuid() };
                            match keysym.modified_sym() {
                                Keysym::Return => {
                                    info!("⌨️  启动终端");
                                    let mut cmd = std::process::Command::new(&data.cfg.terminal.command);
                                    cmd.env("WAYLAND_DISPLAY", "wayland-anchor")
                                        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
                                        .env("XMODIFIERS", "@im=fcitx").env("QT_IM_MODULE", "fcitx").env("GTK_IM_MODULE", "fcitx");
                                    if let Some(d) = data.xdisplay {
                                        cmd.env("DISPLAY", format!(":{}", d));
                                    }
                                    cmd.spawn().ok();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Escape => {
                                    if mods.shift {
                                        data.run = false;
                                    } else {
                                        data.lock_state.lock(data.pointer_pos.0);
                                        data.dirty = true;
                                    }
                                    return FilterResult::Intercept(());
                                }
                                // Super+Shift+R: reload config & restart
                                Keysym::r => {
                                    info!("🔄 Reloading Anchor...");
                                    std::process::Command::new("kill")
                                        .arg("-SIGUSR1")
                                        .arg(std::process::id().to_string())
                                        .spawn().ok();
                                    // Just mark dirty to force re-render with fresh state
                                    data.cfg = Config::load();
                                    data.wallpaper_cache = wallpaper::WallpaperCache::new();
                                    data.wallpaper_texture = None;
                                    {
                                        let home = std::env::var("HOME").unwrap_or_default();
                                        let wp_dir = if data.cfg.wallpaper.directory.is_empty() {
                                            format!("{}/Pictures/wallpapers", home)
                                        } else {
                                            data.cfg.wallpaper.directory.clone()
                                        };
                                        data.wallpaper_cache.scan_directory(&wp_dir);
                                        if data.cfg.wallpaper.mode == "image" || data.cfg.wallpaper.mode == "random" {
                                            let wp_path = if data.cfg.wallpaper.path.is_empty() { String::new() } else { data.cfg.wallpaper.path.clone() };
                                            data.wallpaper_cache.load(&wp_path, data.osize.w as usize, data.osize.h as usize);
                                        }
                                    }
                                    data.cursor_img = if !data.cfg.cursor.theme.is_empty() {
                                        cursor::CursorImage::load_from_theme(&data.cfg.cursor.theme, &data.cfg.cursor.name, data.cfg.cursor.size)
                                            .unwrap_or_else(|| cursor::CursorImage::builtin(data.cfg.cursor.size))
                                    } else {
                                        let default_theme = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
                                        cursor::CursorImage::load_from_theme(&default_theme, &data.cfg.cursor.name, data.cfg.cursor.size)
                                            .unwrap_or_else(|| cursor::CursorImage::builtin(data.cfg.cursor.size))
                                    };
                                    data.notify("Config reloaded ✓");
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::q => {
                                    if let Some(ref surf) = data.workspaces[data.active_ws].focus.clone() {
                                        let ws = &data.workspaces[data.active_ws];
                                        // Try Wayland toplevel first
                                        if let Some(tl) = ws.tops.iter().find(|tl| tl.wl_surface() == surf) {
                                            tl.send_close();
                                        }
                                        // Try X11 surface
                                        if let Some(xs) = ws.x11_surfaces.iter().find(|xs| xs.wl_surface().as_ref() == Some(surf)) {
                                            let _ = xs.close();
                                        }
                                    }
                                    return FilterResult::Intercept(());
                                }
                                Keysym::d => {
                                    data.launcher.toggle(&data.cfg.terminal.command);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::f => { data.toggle_fullscreen(); return FilterResult::Intercept(()); }
                                Keysym::p => {
                                    // Super+Shift+P: 全屏截图直接保存+剪贴板
                                    // Super+P: 区域选择截图
                                    if mods.shift {
                                        data.pending_screenshot = Some(screenshot::ScreenshotRequest::Full);
                                        data.dirty = true;
                                    } else {
                                        // 进入区域选择模式
                                        data.screenshot.begin_selection();
                                        data.notify("Select area (drag to select, Esc to cancel)");
                                        data.dirty = true;
                                    }
                                    return FilterResult::Intercept(());
                                }
                                Keysym::grave => {
                                    // Scratchpad: 切换下拉终端
                                    let msg = data.scratchpad.toggle(&data.cfg.terminal.command, data.xdisplay);
                                    data.notify(msg);
                                    data.dirty = true;
                                    return FilterResult::Intercept(());
                                }
                                Keysym::space => {
                                    let ws = &mut data.workspaces[data.active_ws];
                                    ws.layout = ws.layout.next();
                                    let name = ws.layout.name();
                                    info!("🔄 布局切换 → {}", name);
                                    data.notify(format!("Layout: {}", name));
                                    data.do_layout_animated();
                                    return FilterResult::Intercept(());
                                }
                                // Super+1-9：切换工作区
                                Keysym::_1 => { data.switch_workspace(0); return FilterResult::Intercept(()); }
                                Keysym::_2 => { data.switch_workspace(1); return FilterResult::Intercept(()); }
                                Keysym::_3 => { data.switch_workspace(2); return FilterResult::Intercept(()); }
                                Keysym::_4 => { data.switch_workspace(3); return FilterResult::Intercept(()); }
                                Keysym::_5 => { data.switch_workspace(4); return FilterResult::Intercept(()); }
                                Keysym::_6 => { data.switch_workspace(5); return FilterResult::Intercept(()); }
                                Keysym::_7 => { data.switch_workspace(6); return FilterResult::Intercept(()); }
                                Keysym::_8 => { data.switch_workspace(7); return FilterResult::Intercept(()); }
                                Keysym::_9 => { data.switch_workspace(8); return FilterResult::Intercept(()); }
                                // Super+方向键 / Super+Shift+方向键
                                Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => {
                                    if mods.shift {
                                        data.swap_window(keysym.modified_sym());
                                    } else {
                                        data.focus_direction(keysym.modified_sym());
                                    }
                                    return FilterResult::Intercept(());
                                }
                                // Super+V: 下一个新窗口纵向分割（类似 sway split v）
                                Keysym::v => {
                                    data.workspaces[data.active_ws].pending_split = Some(layout::SplitDir::Vertical);
                                    info!("📐 下一个窗口 → 纵向 (Vertical)");
                                    data.notify("Next split: Vertical ↕");
                                    return FilterResult::Intercept(());
                                }
                                // Super+B: 下一个新窗口横向分割（类似 sway split h）
                                Keysym::b => {
                                    data.workspaces[data.active_ws].pending_split = Some(layout::SplitDir::Horizontal);
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
                                        Keysym::_1 => { data.move_window_to_workspace(0); return FilterResult::Intercept(()); }
                                        Keysym::_2 => { data.move_window_to_workspace(1); return FilterResult::Intercept(()); }
                                        Keysym::_3 => { data.move_window_to_workspace(2); return FilterResult::Intercept(()); }
                                        Keysym::_4 => { data.move_window_to_workspace(3); return FilterResult::Intercept(()); }
                                        Keysym::_5 => { data.move_window_to_workspace(4); return FilterResult::Intercept(()); }
                                        Keysym::_6 => { data.move_window_to_workspace(5); return FilterResult::Intercept(()); }
                                        Keysym::_7 => { data.move_window_to_workspace(6); return FilterResult::Intercept(()); }
                                        Keysym::_8 => { data.move_window_to_workspace(7); return FilterResult::Intercept(()); }
                                        Keysym::_9 => { data.move_window_to_workspace(8); return FilterResult::Intercept(()); }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::PointerMotion { event } => {
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
                        let target = self.output_sizes.iter().min_by(|(ox1, oy1, ow1, oh1), (ox2, oy2, ow2, oh2)| {
                            // 距离排序：欧氏距离到 output 中心
                            let c1x = *ox1 as f64 + *ow1 as f64 / 2.0;
                            let c1y = *oy1 as f64 + *oh1 as f64 / 2.0;
                            let c2x = *ox2 as f64 + *ow2 as f64 / 2.0;
                            let c2y = *oy2 as f64 + *oh2 as f64 / 2.0;
                            let d1 = (c1x - self.pointer_pos.0).powi(2) + (c1y - self.pointer_pos.1).powi(2);
                            let d2 = (c2x - self.pointer_pos.0).powi(2) + (c2y - self.pointer_pos.1).powi(2);
                            d1.partial_cmp(&d2).unwrap_or(std::cmp::Ordering::Equal)
                        });
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
                            self.pointer_pos.0 = self.pointer_pos.0.clamp(*ox as f64, (*ox + *ow) as f64);
                            self.pointer_pos.1 = self.pointer_pos.1.clamp(*oy as f64, (*oy + *oh) as f64);
                        }
                    }
                }

                // 同步 focused_output：鼠标移动到另一个 output 时更新
                let new_focused = self.output_at_pointer();
                if new_focused != self.focused_output {
                    self.focused_output = new_focused;
                    // 全局 active_ws 跟踪当前鼠标所在 output 的工作区
                    self.active_ws = self.output_active_ws.get(new_focused).copied().unwrap_or(0);
                    self.dirty = true;
                }

                // 截图区域选择模式：更新选择终点
                if self.screenshot.selecting {
                    self.screenshot.on_motion(self.pointer_pos.0, self.pointer_pos.1);
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
                let ptr = self.pointer.clone();
                // 转为 output 局部坐标（pointer_focus 返回的 offset 也是 output 局部的）
                let oi = self.output_at_pointer();
                let (ox, oy, _, _) = self.output_sizes.get(oi).copied().unwrap_or_default();
                ptr.motion(self, focus, &MotionEvent {
                    location: Point::from((self.pointer_pos.0 - ox as f64, self.pointer_pos.1 - oy as f64)),
                    serial,
                    time,
                });
                ptr.frame(self);

                self.dirty = true;
            }
            InputEvent::PointerButton { event } => {
                // 锁屏模式：阻止鼠标按钮
                if self.lock_state.locked { return; }
                // 截图区域选择模式：按下记录起点，释放完成截图
                if self.screenshot.selecting {
                    if event.state() == ButtonState::Pressed {
                        self.screenshot.on_press(self.pointer_pos.0, self.pointer_pos.1);
                        self.dirty = true;
                    } else if event.state() == ButtonState::Released {
                        if let Some((x, y, w, h)) = self.screenshot.on_release() {
                            self.pending_screenshot = Some(screenshot::ScreenshotRequest::Area(x, y, w, h));
                            self.dirty = true;
                        } else {
                            self.notify("Selection too small, cancelled");
                        }
                        self.dirty = true;
                    }
                    return; // 截图模式中拦截所有鼠标点击
                }

                // 点击聚焦（仅 Press 时）
                if event.state() == ButtonState::Pressed {
                    let oi = self.output_at_pointer();
                    let (ox, oy, ow, oh) = self.output_sizes.get(oi).copied().unwrap_or((0, 0, self.osize.w, self.osize.h));
                    let px = self.pointer_pos.0 as i32 - ox;
                    let py = self.pointer_pos.1 as i32 - oy;
                    let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
                    
                    // 使用鼠标所在 output 的工作区
                    let ws_idx = self.output_active_ws.get(oi).copied().unwrap_or(self.active_ws);
                    self.active_ws = ws_idx;
                    let ws = &self.workspaces[ws_idx];

                    if py >= bar_h {
                        // 全屏模式下：点击不切换焦点，只确保全屏窗口有焦点
                        if ws.fullscreen.is_some() {
                            // 全屏时整个区域都属于全屏窗口，不切换焦点
                        } else {
                            let order = ws.effective_order();

                            // Check if click is on an XDG popup — if so, don't steal focus
                            let on_popup = {
                                let mut found = false;
                                for (i, slot) in order.iter().enumerate() {
                                    if let WindowSlot::Wl(idx) = slot {
                                        if let Some(tl) = ws.tops.get(*idx) {
                                            let (x, y, _, _) = layout::slot(i, order.len(), ow, oh, bar_h, &self.cfg, ws.layout, ws.split);
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
                                    let (x, y, w, h) = layout::slot(i, n_all, ow, oh, bar_h, &self.cfg, ws.layout, ws.split);
                                    if px >= x && px < x + w && py >= y && py < y + h {
                                        let surf = match slot {
                                            WindowSlot::Wl(idx) => ws.tops.get(*idx).map(|tl| tl.wl_surface().clone()),
                                            WindowSlot::X11(idx) => ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()),
                                        };
                                        if let Some(surf) = surf {
                                            self.workspaces[self.active_ws].focus = Some(surf.clone());
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

                // 转发按钮事件给客户端
                let serial = SERIAL_COUNTER.next_serial();
                let time = (event.time() / 1000) as u32;
                let ptr = self.pointer.clone();
                ptr.button(self, &smithay::input::pointer::ButtonEvent {
                    serial,
                    time,
                    button: event.button_code(),
                    state: event.state(),
                });
                ptr.frame(self);
            }
            InputEvent::PointerAxis { event } => {
                // 锁屏模式：阻止滚轮
                if self.lock_state.locked { return; }
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
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg }
    fn new_toplevel(&mut self, s: ToplevelSurface) {
        // 拦截 scratchpad 窗口
        if self.scratchpad.intercept_toplevel(s.clone()) {
            // 配置为浮动覆盖层：居中、上方 1/3 高度
            let ow = self.osize.w;
            let oh = self.osize.h;
            let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
            let sp_w = ow * 3 / 4; // 75% 宽度
            let sp_h = oh / 3;     // 1/3 高度
            let sp_x = (ow - sp_w) / 2;
            let sp_y = bar_h + 8;
            s.with_pending_state(|st| {
                st.size = Some((sp_w, sp_h).into());
                st.states.set(xdg_toplevel::State::Activated);
            });
            s.send_configure();
            info!("🚀 Scratchpad 浮动窗口: {}x{} at ({},{})", sp_w, sp_h, sp_x, sp_y);
            // 聚焦
            let surf = s.wl_surface().clone();
            let kbd = self.kbd.clone();
            let serial = SERIAL_COUNTER.next_serial();
            kbd.set_focus(self, Some(surf), serial);
            self.dirty = true;
            return;
        }
        
        self.pending_tops.push(s);
    }
    fn new_popup(&mut self, popup: PopupSurface, _positioner: PositionerState) {
        info!("🆕 new_popup created");
        let _ = popup.send_configure();
        if let Err(e) = self.popup_manager.track_popup(PopupKind::Xdg(popup)) {
            warn!("⚠️  track_popup: {:?}", e);
        }
    }
    fn grab(&mut self, popup: PopupSurface, _seat: wl_seat::WlSeat, _serial: smithay::utils::Serial) {
        info!("🆕 grab popup created");
        let _ = popup.send_configure();
        if let Err(e) = self.popup_manager.track_popup(PopupKind::Xdg(popup)) {
            warn!("⚠️  track_popup (grab): {:?}", e);
        }
        self.dirty = true;
    }
    fn reposition_request(&mut self, popup: PopupSurface, _positioner: PositionerState, _token: u32) {
        let _ = popup.send_configure();
    }
    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<wayland_server::protocol::wl_output::WlOutput>) {
        // Client (e.g. browser video) requests fullscreen
        let wl_surf = surface.wl_surface().clone();
        for (ws_idx, ws) in self.workspaces.iter_mut().enumerate() {
            for (i, tl) in ws.tops.iter().enumerate() {
                if tl.wl_surface() == &wl_surf {
                    info!("🔳 客户端请求全屏 #{} (工作区 {})", i, ws_idx + 1);
                    ws.fullscreen = Some(i);
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
    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        use smithay::wayland::compositor::with_states;
        use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
        let app_id = with_states(surface.wl_surface(), |states| {
            states.data_map.get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok())
                .and_then(|d| d.app_id.clone())
                .unwrap_or_default()
        });
        info!("🆔 app_id_changed: '{}'", app_id);
        
        let wl_surf = surface.wl_surface().clone();

        // Check if this is a pending toplevel awaiting confirmation.
        // Non-empty app_id → real window, promote to tiling layout.
        // Empty app_id → tooltip/transient (e.g. Chromium hover), ignore.
        if let Some(pos) = self.pending_tops.iter().position(|t| t.wl_surface() == &wl_surf) {
            self.pending_tops.remove(pos);
            // Filter clipboard helper windows — they create invisible toplevels to own selections
            let is_clipboard = app_id.contains("clipboard") || app_id.contains("wl-copy") || app_id.contains("wl-paste");
            if !app_id.is_empty() && !is_clipboard {
                info!("✅ pending → tiling (app_id='{}')", app_id);
                self.workspaces[self.active_ws].tops.push(surface.clone());
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
            if found.is_some() { break; }
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
            states.data_map.get::<XdgToplevelSurfaceData>()
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
        tracing::info!("📋 send_selection: ty={:?}, mime={}, data_len={}", ty, mime_type, user_data.len());
        // X11 代理选区标记：user_data 以 "X11_PROXY" 开头（10 bytes magic）
        // 这不可能是正常剪贴板内容
        const X11_PROXY_MAGIC: &[u8] = b"X11_PROXY\x00";
        let is_x11_proxy = user_data.starts_with(X11_PROXY_MAGIC) && user_data.len() == X11_PROXY_MAGIC.len();

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
                if let Err(err) = smithay::reexports::rustix::fs::fcntl_setfl(&fd, smithay::reexports::rustix::fs::OFlags::empty()) {
                    tracing::warn!("error clearing flags on selection fd: {:?}", err);
                }
                if let Err(err) = std::fs::File::from(fd).write_all(&buf) {
                    tracing::warn!("error writing selection: {:?}", err);
                }
            });
        }
    }
}
impl DataDeviceHandler for App { fn data_device_state(&self) -> &DataDeviceState { &self.dd } }
impl PrimarySelectionHandler for App {
    fn primary_selection_state(&self) -> &PrimarySelectionState { &self.primary_sel }
}
impl smithay::wayland::output::OutputHandler for App {}
impl ClientDndGrabHandler for App {}
impl ServerDndGrabHandler for App { fn send(&mut self, _: String, _: OwnedFd, _: Seat<Self>) {} }

impl InputMethodHandler for App {
    fn new_popup(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: new");
        self.im_popup = Some(surface);
        self.dirty = true;
    }
    fn dismiss_popup(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: dismiss");
        if self.im_popup.as_ref().map_or(false, |p| p.wl_surface() == surface.wl_surface()) {
            self.im_popup = None;
        }
        self.dirty = true;
    }
    fn popup_repositioned(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: repositioned");
        self.im_popup = Some(surface);
        self.dirty = true;
    }
    fn parent_geometry(&self, _parent: &WlSurface) -> Rectangle<i32, Logical> { Rectangle::default() }
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState { &mut self.comp }
    fn client_compositor_state<'a>(&self, c: &'a Client) -> &'a CompositorClientState {
        if let Some(cs) = c.get_data::<ClientState>() {
            &cs.comp
        } else if let Some(xw) = c.get_data::<smithay::xwayland::XWaylandClientData>() {
            &xw.compositor_state
        } else {
            static FALLBACK: std::sync::OnceLock<CompositorClientState> = std::sync::OnceLock::new();
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
            let closed_idx = self.workspaces[ws_idx].tops.iter().position(|tl| tl.wl_surface() == surface);
            self.workspaces[ws_idx].tops.retain(|tl| tl.wl_surface() != surface);
            if self.workspaces[ws_idx].tops.len() < before {
                // 重新映射 prev_positions（索引移位修复）
                if let Some(removed_idx) = closed_idx {
                    self.remap_prev_after_remove(&WindowSlot::Wl(removed_idx));
                }
                info!("🗑️ 窗口关闭 (工作区 {})", ws_idx + 1);
                // 清理 fullscreen（fullscreen 存的是 effective_order 索引）
                self.workspaces[ws_idx].fullscreen = None;
                // 重建窗口顺序
                self.workspaces[ws_idx].rebuild_order();
                // 更新 focus
                if self.workspaces[ws_idx].focus.as_ref() == Some(surface) {
                    let order = self.workspaces[ws_idx].effective_order();
                    self.workspaces[ws_idx].focus = order.last().and_then(|s| match s {
                        WindowSlot::Wl(idx) => self.workspaces[ws_idx].tops.get(*idx).map(|tl| tl.wl_surface().clone()),
                        WindowSlot::X11(idx) => self.workspaces[ws_idx].x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()),
                    });
                }
                if ws_idx == self.active_ws {
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
            self.workspaces[ws_idx].x11_surfaces.retain(|s| s.wl_surface().as_ref() != Some(surface));
            if self.workspaces[ws_idx].x11_surfaces.len() < before {
                info!("🗑️ X11 窗口 wl_surface 销毁 (工作区 {})", ws_idx + 1);
                self.workspaces[ws_idx].fullscreen = None;
                self.workspaces[ws_idx].rebuild_order();
                // 更新 focus
                let order = self.workspaces[ws_idx].effective_order();
                self.workspaces[ws_idx].focus = order.last().and_then(|s| match s {
                    WindowSlot::Wl(idx) => self.workspaces[ws_idx].tops.get(*idx).map(|tl| tl.wl_surface().clone()),
                    WindowSlot::X11(idx) => self.workspaces[ws_idx].x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()),
                });
                if ws_idx == self.active_ws {
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

impl ShmHandler for App { fn shm_state(&self) -> &ShmState { &self.shm } }
impl SeatHandler for App {
    type KeyboardFocus = WlSurface; type PointerFocus = WlSurface; type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> { &mut self.seat_state }
    fn focus_changed(&mut self, seat: &Seat<Self>, surface: Option<&WlSurface>) {
        let dh = self.dh.clone();
        let client = surface.and_then(|s| s.client());

        // Update data device (clipboard) focus — sends selection offer to new focus client
        smithay::wayland::selection::data_device::set_data_device_focus::<App>(
            &dh, seat, client.clone(),
        );
        // Update primary selection focus
        smithay::wayland::selection::primary_selection::set_primary_focus::<App>(
            &dh, seat, client,
        );

        // Deactivate all X11 surfaces first
        for ws in &self.workspaces {
            for xs in &ws.x11_surfaces {
                let _ = xs.set_activated(false);
            }
        }
        // Activate the focused X11 surface
        if let Some(surf) = surface {
            for ws in &self.workspaces {
                for xs in &ws.x11_surfaces {
                    if xs.wl_surface().as_ref() == Some(surf) {
                        let _ = xs.set_activated(true);
                        return;
                    }
                }
            }
        }
    }
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {}
}

#[derive(Default)] struct ClientState { comp: CompositorClientState }
impl ClientData for ClientState { fn initialized(&self, _: ClientId) {} fn disconnected(&self, _: ClientId, _: DisconnectReason) {} }

fn send_frames(s: &WlSurface, t: u32) {
    with_surface_tree_downward(s, (), |_,_,&()| TraversalAction::DoChildren(()),
        |_,st,&()| { for cb in st.cached_state.get::<SurfaceAttributes>().current().frame_callbacks.drain(..) { cb.done(t); } },
        |_,_,&()| true);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info,smithay=warn").init();
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        let dir = format!("/run/user/{}", unsafe { libc::getuid() });
        std::fs::create_dir_all(&dir).ok();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
    }
    let args: Vec<String> = std::env::args().collect();
    let direct = args.iter().any(|a| a == "--direct");
    let cfg = Config::load();
    info!("🚀 Anchor v10 GPU ({})", if direct { "direct" } else { "session" });

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
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("card") {
                    if first_card.is_none() { first_card = Some(e.path().clone()); }
                    if let Ok(v) = std::fs::read_to_string(format!("/sys/class/drm/{}/device/vendor", name)) {
                        let vendor = v.trim();
                        let matches = match prefer_vendor.as_str() {
                            "nvidia" => vendor == "0x10de",
                            "amd" => vendor == "0x1002",
                            "intel" => vendor == "0x8086",
                            _ => true,
                        };
                        if matches && preferred.is_none() {
                            preferred = Some(e.path());
                            if prefer_vendor != "auto" { break; }
                        }
                    }
                }
            }
        }
        let result = preferred.or(first_card).expect("No DRM device found in /dev/dri");
        info!("🎮 GPU auto-detected (vendor preference: {})", prefer_vendor);
        result
    };
    info!("🎮 {}", gpu_path.display());
    
    let gpu_vendor = {
        let card_name = gpu_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let vendor_str = std::fs::read_to_string(format!("/sys/class/drm/{}/device/vendor", card_name))
            .unwrap_or_default().trim().to_string();
        match vendor_str.as_str() {
            "0x10de" => "NVIDIA",
            "0x1002" => "AMD",
            "0x8086" => "Intel",
            _ => "Unknown",
        }.to_string()
    };
    info!("🔍 GPU vendor: {}", gpu_vendor);
    if let Some(card_name) = gpu_path.file_name() {
        std::env::set_var("TITAN_DRM_DEV", format!("/dev/dri/{}", card_name.to_string_lossy()));
    }

    // ─── Session ───
    let (dev_fd, session, notifier) = if direct {
        let fd = Arc::new(std::fs::OpenOptions::new().read(true).write(true).open(&gpu_path)?);
        let _ = unsafe { libc::ioctl(fd.as_raw_fd(), 0x4000641eu64 as _) };
        let dup = unsafe { libc::dup(fd.as_raw_fd()) };
        use std::os::unix::io::FromRawFd;
        (DrmDeviceFd::new(DeviceFd::from(OwnedFd::from(unsafe { std::fs::File::from_raw_fd(dup) }))), None, None)
    } else {
        let (mut session, notifier) = LibSeatSession::new()?;
        use smithay::reexports::rustix::fs::OFlags;
        let fd = session.open(&gpu_path, OFlags::RDWR)?;
        info!("✅ DRM 设备已打开 (via libseat)");
        (DrmDeviceFd::new(DeviceFd::from(fd)), Some(Arc::new(std::sync::Mutex::new(session))), Some(notifier))
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
        comp: CompositorState::new::<App>(&dh), xdg: XdgShellState::new::<App>(&dh),
        shm: ShmState::new::<App>(&dh, vec![]), seat_state, seat,
        dd: DataDeviceState::new::<App>(&dh),
        primary_sel: PrimarySelectionState::new::<App>(&dh),
        deco: XdgDecorationState::new::<App>(&dh),
        popup_manager: PopupManager::default(),
        osize: Size::new(0, 0),
        workspaces: (0..NUM_WORKSPACES).map(|_| Workspace::new()).collect(),
        active_ws: 0,
        run: true, frame: 0,
        dh: dh.clone(), active: false,
        dirty: true,
        kbd, pointer, cfg,
        cursor_img,
        pointer_pos: (0.0, 0.0),
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
        ws_anim: WsAnimation { start: None, from_ws: 0, to_ws: 0, duration_ms: 200, direction: 0 },
        layout_anim: LayoutAnimation::new(),
        prev_positions: Vec::new(),
        launcher_blur_tex: None,
        launcher_blur_size: (1, 1),
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
        cpu_usage: 0.0,
        mem_usage: 0.0,
        cpu_prev_idle: 0,
        cpu_prev_total: 0,
    };
    let listener = ListeningSocket::bind("wayland-anchor")?;
    std::env::set_var("WAYLAND_DISPLAY", "wayland-anchor");
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }));
    }
    info!("✅ wayland-anchor");

    // ─── EventLoop + 等 DRM master 就绪（必须在 EGL 之前！）───
    // NVIDIA 上 EGL 初始化需要 DRM master，GDM 切换 session 时需要 dispatch 才能收到 ActivateSession
    let mut eloop: EventLoop<App> = EventLoop::try_new()?;
    state.loop_handle = Some(eloop.handle());
    let mut clients: Vec<Client> = vec![];
    eloop.handle().insert_source(dn, |e,_,state: &mut App| match e {
        DrmEvent::VBlank(crtc) => { state.vblank_crtcs.insert(crtc); }
        DrmEvent::Error(e) => error!("DRM:{e:?}"),
    })?;
    if let Some(notifier) = notifier {
        eloop.handle().insert_source(notifier, |event, _, state: &mut App| match event {
            SessionEvent::ActivateSession => { info!("▶️  会话激活"); state.active = true; }
            SessionEvent::PauseSession => { info!("⏸️  会话暂停"); state.active = false; }
        })?;
    }
    if let Some(session) = session.as_ref() {
        state.active = session.lock().unwrap().is_active();
        let t0 = Instant::now();
        while !state.active && t0.elapsed() < Duration::from_secs(10) {
            eloop.dispatch(Some(Duration::from_millis(100)), &mut state)?;
            state.active = session.lock().unwrap().is_active();
        }
        if !state.active { return Err("libseat 会话 10s 内未激活".into()); }
        device.activate(true)?;
        info!("✅ DRM master");
    } else { state.active = true; }

    // ─── EGL + GLES 渲染器（现在已有 DRM master，NVIDIA 上可以正常初始化）───
    let egl_display = unsafe { smithay::backend::egl::EGLDisplay::new(gbm.clone())? };
    info!("✅ EGLDisplay");
    let egl_context = smithay::backend::egl::EGLContext::new(&egl_display)?;
    info!("✅ EGLContext");
    let render_formats: Vec<Format> = egl_context.dmabuf_render_formats().iter().copied().collect();
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
    }
    let mut connector_infos: Vec<ConnectorInfo> = Vec::new();

    for &c in res.connectors() {
        for f in [false, true] {
            if let Ok(info) = device.get_connector(c, f) {
                if info.state() != connector::State::Connected || info.modes().is_empty() { continue; }
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
                    if found_crtc.is_some() { break; }
                }
                let Some(crtc_h) = found_crtc else { continue };
                used_crtcs.insert(crtc_h);

                let conn_name = format!("{:?}", c);
                info!("🖥️  Connector {} (CRTC {:?}): {}x{}", conn_name, crtc_h, mw, mh);

                connector_infos.push(ConnectorInfo {
                    connector: c, crtc: crtc_h, mode, name: conn_name,
                });
                break;
            }
        }
    }
    if connector_infos.is_empty() { return Err("无可用显示器".into()); }

    let fd_clones: Vec<_> = (0..connector_infos.len())
        .map(|_| dev_fd.clone())
        .collect();
    let mut output_sizes: Vec<(i32, i32, i32, i32)> = Vec::new();

    for (idx, ci) in connector_infos.iter().enumerate() {
        let (mw, mh) = ci.mode.size();

        let surface = match device.create_surface(ci.crtc, ci.mode, &[ci.connector]) {
            Ok(s) => s,
            Err(e) => { warn!("⚠️  Surface 创建失败 {}: {:?}", ci.name, e); continue; }
        };

        let gbm_dup = GbmDevice::new(fd_clones[idx].clone())?;
        let alloc = GbmAllocator::new(gbm_dup, GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
        let buf_surf = match GbmBufferedSurface::new(surface, alloc,
            &[Fourcc::Argb8888, Fourcc::Xrgb8888, Fourcc::Abgr8888, Fourcc::Xbgr8888], render_formats.iter().copied()) {
            Ok(bs) => {
                info!("✅ GbmBufferedSurface 创建成功 ({})", ci.name);
                bs
            }
            Err(e) => {
                warn!("⚠️  GbmBufferedSurface 失败 {}: {:?}, trying SCANOUT only", ci.name, e);
                let gbm_dup2 = GbmDevice::new(fd_clones[idx].clone())?;
                let alloc2 = GbmAllocator::new(gbm_dup2, GbmBufferFlags::SCANOUT);
                let surface2 = device.create_surface(ci.crtc, ci.mode, &[ci.connector])?;
                GbmBufferedSurface::new(surface2, alloc2,
                    &[Fourcc::Argb8888, Fourcc::Xrgb8888], render_formats.iter().copied())?
            }
        };
        
        let wl_output = Output::new(ci.name.clone(), PhysicalProperties {
            size: (mw as i32 / 10, mh as i32 / 10).into(),
            subpixel: Subpixel::Unknown, make: gpu_vendor.clone(), model: ci.name.clone(),
        });
        let output_mode = Mode { size: (mw as i32, mh as i32).into(), refresh: ci.mode.vrefresh() as i32 * 1000 };
        wl_output.add_mode(output_mode);
        wl_output.set_preferred(output_mode);
        wl_output.change_current_state(
            Some(output_mode), Some(Transform::Normal),
            Some(Scale::Integer(1)), Some(Point::from((output_x_offset, 0)))
        );
        wl_output.create_global::<App>(&dh);

        // 匹配配置中的 output 设置（工作区、位置）
        let output_cfg = state.cfg.outputs.iter().find(|oc| {
            if oc.connector.is_empty() { false } else { ci.name.contains(&oc.connector) }
        });
        let default_ws = output_cfg.map(|oc| oc.workspace).unwrap_or(idx);
        let cfg_x = output_cfg.map(|oc| oc.x).unwrap_or(output_x_offset);
        let cfg_y = output_cfg.map(|oc| oc.y).unwrap_or(0);
        let out_x = if output_cfg.map(|oc| oc.x).unwrap_or(0) != 0 || output_cfg.map(|oc| oc.y).unwrap_or(0) != 0 {
            cfg_x  // 有显式配置位置
        } else {
            output_x_offset  // 自动从左到右排列
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
        });
        output_x_offset += mw as i32;
    }
    if anchor_outputs.is_empty() { return Err("所有输出创建失败".into()); }
    let primary_size = anchor_outputs[0].size;
    info!("✅ {} 个输出已就绪", anchor_outputs.len());

    // 更新 App 的显示相关字段（之前用 dummy 值创建）
    state.osize = primary_size;
    state.output_sizes = output_sizes;
    // 初始化每个 output 的活跃工作区（从 anchor_outputs 读取）
    state.output_active_ws = anchor_outputs.iter().map(|o| o.active_ws).collect();
    // 初始全局 active_ws 跟踪第一个 output 的工作区
    state.active_ws = state.output_active_ws.first().copied().unwrap_or(0);

    {
        struct SessionInputInterface { session: Arc<std::sync::Mutex<LibSeatSession>> }
        impl libinput_crate::LibinputInterface for SessionInputInterface {
    fn open_restricted(&mut self, path: &std::path::Path, flags: i32) -> Result<std::os::unix::io::OwnedFd, i32> {
                use smithay::reexports::rustix::fs::OFlags;
                use smithay::backend::session::AsErrno;
                self.session.lock().unwrap().open(path, OFlags::from_bits_truncate(flags as u32)).map_err(|e| e.as_errno().unwrap_or(libc::EACCES))
            }
            fn close_restricted(&mut self, fd: std::os::unix::io::OwnedFd) { let _ = self.session.lock().unwrap().close(fd); }
        }
        if let Some(session) = session.clone() {
            let iface = SessionInputInterface { session };
            let mut libinput_ctx = libinput_crate::Libinput::new_with_udev(iface);
            if let Err(e) = libinput_ctx.udev_assign_seat("seat0") { warn!("⚠️  libinput: {:?}", e); }
            else {
                info!("✅ libinput (seat0)");
                let backend = LibinputInputBackend::new(libinput_ctx);
                eloop.handle().insert_source(backend, |event, _, state: &mut App| { state.handle_input_event(event); })?;
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
            let wp_path = if state.cfg.wallpaper.path.is_empty() { String::new() } else { state.cfg.wallpaper.path.clone() };
            state.wallpaper_cache.load(&wp_path, primary_size.w as usize, primary_size.h as usize);
        }
    }

    std::process::Command::new("fcitx5")
        .arg("-d")
        .env("WAYLAND_DISPLAY", "wayland-anchor")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }))
        .env("XMODIFIERS", "@im=fcitx")
        .env("QT_IM_MODULE", "fcitx")
        .env("GTK_IM_MODULE", "fcitx")
        .env("SDL_IM_MODULE", "fcitx")
        .spawn().ok();

    // ── XWayland ──
    {
        let eloop_handle = eloop.handle();
        match xwayland::spawn_xwayland(&dh) {
            Ok((xwayland_src, xw_client)) => {
                eloop.handle().insert_source(xwayland_src, move |event, _, state: &mut App| {
                    if let smithay::xwayland::XWaylandEvent::Ready { display_number, .. } = event {
                        state.xdisplay = Some(display_number);
                    }
                    xwayland::handle_xwayland_event(
                        event, &eloop_handle, &xw_client, &mut state.xw,
                    );
                }).ok();
            }
            Err(e) => {
                warn!("⚠️  XWayland spawn failed: {} — X11 apps won't work", e);
            }
        }
    }

    info!("🔄 GPU 渲染中...");

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
                for out in &mut anchor_outputs { out.pending_flip = false; }
            }
            dev_active = state.active;
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
                    if state.frame > 1 { warn!("VBlank err: {:?}", e); }
                }
                out.pending_flip = false;
            }
        }

        if state.dirty {
            // ── 锁屏 PAM 轮询（必须在渲染之前）──
            // 如果认证刚完成，当前帧立刻渲染桌面而非锁屏
            if state.lock_state.locked {
                state.lock_state.poll_unlock();
                if !state.lock_state.locked {
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

            state.workspaces[state.active_ws].tops.retain(|tl| tl.alive());
            let bar_h = if state.cfg.bar.enabled { state.cfg.bar.height } else { 0 };
            let time_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();

            let ws_anim_active = state.ws_anim.start.is_some();
            let ws_anim_dir = state.ws_anim.direction;
            let ws_anim_duration = state.ws_anim.duration_ms;
            let ws_anim_elapsed = state.ws_anim.start.map(|s| s.elapsed().as_millis() as u64);

            for oi in 0..anchor_outputs.len() {
                let out = &mut anchor_outputs[oi];
                if out.pending_flip { continue; }

                // 此 output 的工作区（从 App 的 output_active_ws 读取，确保与 switch_workspace 同步）
                let out_ws_idx = state.output_active_ws.get(oi).copied().unwrap_or(0);
                let out_ws = &state.workspaces[out_ws_idx];
                let n_windows = out_ws.tops.len();
                let n_x11 = out_ws.x11_surfaces.len();
                let n_total = n_windows + n_x11;
                let fullscreen = out_ws.fullscreen;
                let is_focused_output = oi == state.focused_output;

                // Per-output 的焦点和标题
                let out_ws_focus_idx = {
                    let ws = &state.workspaces[out_ws_idx];
                    let order = ws.effective_order();
                    ws.focus.as_ref().and_then(|surf| {
                        order.iter().enumerate().find(|(_, slot)| match slot {
                            WindowSlot::Wl(idx) => ws.tops.get(*idx).map(|tl| tl.wl_surface() == surf).unwrap_or(false),
                            WindowSlot::X11(idx) => ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()).map(|s| &s == surf).unwrap_or(false),
                        }).map(|(i, _)| i)
                    })
                };
                let out_window_title = state.window_titles.get(&out_ws_focus_idx.unwrap_or(0))
                    .cloned().unwrap_or_default();

                match out.buf_surf.next_buffer() {
                    Ok((mut dmabuf, _)) => {
                        let ow = out.size.w;
                        let oh = out.size.h;

                        // ═══════════════════════════════════════════════
                        // Phase 1: collect surface elements (before bind)
                        // ═══════════════════════════════════════════════
                        let mut win_elems: Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>> = Vec::new();
                        let mut popup_elems: Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>> = Vec::new();
                        let mut sp_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
                        let mut im_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
                        let mut or_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
                        let mut im_popup_pos: (i32, i32) = (0, 0);
                        let mut ws_offset: i32 = 0;
                        let mut scratchpad_data: Option<(i32, i32, i32, i32)> = None; // (x, y, w, h)

                        // 每个 output 都渲染自己工作区的窗口（不再限制 is_primary）
                        {
                            if let Some(fi) = fullscreen {
                                let fs_order = out_ws.effective_order();
                                if let Some(fs_slot) = fs_order.get(fi) {
                                    match fs_slot {
                                        WindowSlot::Wl(idx) => {
                                            if let Some(tl) = out_ws.tops.get(*idx) {
                                                let tl_geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                    states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                                                }).unwrap_or_default();
                                                let tl_render_pos = Point::<i32, Physical>::from((-tl_geo.loc.x, bar_h - tl_geo.loc.y));
                                                win_elems.push(
                                                    render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), tl_render_pos, 1.0, 1.0, Kind::Unspecified)
                                                );
                                                let mut p_elems = Vec::new();
                                                for (popup, popup_offset) in PopupManager::popups_for_surface(tl.wl_surface()) {
                                                    let offset = (tl_geo.loc + popup_offset - popup.geometry().loc)
                                                        .to_physical_precise_round(1.0);
                                                    let pos = tl_render_pos + offset;
                                                    p_elems.extend(
                                                        render_elements_from_surface_tree(&mut renderer, popup.wl_surface(), pos, 1.0, 1.0, Kind::Unspecified)
                                                    );
                                                }
                                                popup_elems.push(p_elems);
                                            }
                                        }
                                        WindowSlot::X11(idx) => {
                                            if let Some(xs) = out_ws.x11_surfaces.get(*idx) {
                                                if let Some(wl) = xs.wl_surface() {
                                                    let render_pos = Point::<i32, Physical>::from((0, bar_h));
                                                    win_elems.push(
                                                        render_elements_from_surface_tree(&mut renderer, &wl, render_pos, 1.0, 1.0, Kind::Unspecified)
                                                    );
                                                    popup_elems.push(Vec::new());
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                // 工作区切换动画只对鼠标所在的 output 生效
                                ws_offset = if is_focused_output && ws_anim_active {
                                    if let Some(elapsed) = ws_anim_elapsed {
                                        if elapsed < ws_anim_duration {
                                            let t = elapsed as f32 / ws_anim_duration as f32;
                                            let t_ease = 1.0 - (1.0 - t).powi(3);
                                            (ws_anim_dir as f32 * ow as f32 * (1.0 - t_ease)) as i32
                                        } else { 0 }
                                    } else { 0 }
                                } else { 0 };

                                // Unified window rendering using effective_order
                                let order = out_ws.effective_order();
                                for (i, slot) in order.iter().enumerate() {
                                    let (x, y, _w, _h) = layout::slot(i, n_total, ow, oh, bar_h, &state.cfg, state.workspaces[out_ws_idx].layout, state.workspaces[out_ws_idx].split);
                                    // 布局动画偏移（macOS 风格：从旧位置滑到新位置）
                                    let (layout_dx, layout_dy) = state.layout_anim.offset_for(slot, (x, y)).unwrap_or((0, 0));
                                    match slot {
                                        WindowSlot::Wl(idx) => {
                                            if let Some(tl) = out_ws.tops.get(*idx) {
                                                let tl_geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                    states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                                                }).unwrap_or_default();
                                                // 减去 geometry.loc 偏移（CSD 阴影/边框），使内容区精确对齐 slot
                                                let bx = x - tl_geo.loc.x + ws_offset + layout_dx;
                                                let by = y - tl_geo.loc.y + layout_dy;
                                                let tl_render_pos = Point::<i32, Physical>::from((bx, by));
                                                win_elems.push(
                                                    render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), tl_render_pos, 1.0, 1.0, Kind::Unspecified)
                                                );
                                                let mut p_elems = Vec::new();
                                                for (popup, popup_offset) in PopupManager::popups_for_surface(tl.wl_surface()) {
                                                    let offset = (tl_geo.loc + popup_offset - popup.geometry().loc)
                                                        .to_physical_precise_round(1.0);
                                                    let pos = tl_render_pos + offset;
                                                    p_elems.extend(
                                                        render_elements_from_surface_tree(&mut renderer, popup.wl_surface(), pos, 1.0, 1.0, Kind::Unspecified)
                                                    );
                                                }
                                                popup_elems.push(p_elems);
                                            }
                                        }
                                        WindowSlot::X11(idx) => {
                                            if let Some(xs) = out_ws.x11_surfaces.get(*idx) {
                                                if let Some(wl) = xs.wl_surface() {
                                                    let render_pos = Point::<i32, Physical>::from((x + ws_offset + layout_dx, y + layout_dy));
                                                    win_elems.push(
                                                        render_elements_from_surface_tree(&mut renderer, &wl, render_pos, 1.0, 1.0, Kind::Unspecified)
                                                    );
                                                    popup_elems.push(Vec::new());
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
                                    sp_elems = render_elements_from_surface_tree(&mut renderer, sp_surf.wl_surface(), (sp_x, sp_y), 1.0, 1.0, Kind::Unspecified);
                                    scratchpad_data = Some((sp_x, sp_y, sp_w, sp_h));
                                }
                            }

                            // IM popup (fcitx5 candidate box) — collected separately (rendered on top of windows)
                            if let Some(ref im_popup) = state.im_popup {
                                if im_popup.alive() {
                                    let mut popup_pos = (state.pointer_pos.0 as i32, state.pointer_pos.1 as i32 + 20);
                                    if let Some(parent) = im_popup.get_parent() {
                                        let popup_loc = im_popup.location();
                                        let im_order = out_ws.effective_order();
                                        for (i, slot) in im_order.iter().enumerate() {
                                            let matched = match slot {
                                                WindowSlot::Wl(idx) => out_ws.tops.get(*idx)
                                                    .map(|tl| tl.wl_surface() == &parent.surface)
                                                    .unwrap_or(false),
                                                WindowSlot::X11(idx) => out_ws.x11_surfaces.get(*idx)
                                                    .and_then(|xs| xs.wl_surface())
                                                    .map(|wl| &wl == &parent.surface)
                                                    .unwrap_or(false),
                                            };
                                            if matched {
                                                let (x, y, _, _) = layout::slot(i, im_order.len(), ow, oh, bar_h, &state.cfg, out_ws.layout, out_ws.split);
                                                let (bx, by) = match slot {
                                                    WindowSlot::Wl(idx) => {
                                                        if let Some(tl) = out_ws.tops.get(*idx) {
                                                            let geo = smithay::wayland::compositor::with_states(tl.wl_surface(), |states| {
                                                                states.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>().current().geometry
                                                            }).unwrap_or_default();
                                                            (x - geo.loc.x, y - geo.loc.y)
                                                        } else { (x, y) }
                                                    }
                                                    WindowSlot::X11(_) => (x, y),
                                                };
                                                popup_pos = (bx + popup_loc.x, by + popup_loc.y);
                                                break;
                                            }
                                        }
                                    }
                                    im_popup_pos = popup_pos;
                                    if is_focused_output {
                                        im_elems = render_elements_from_surface_tree(&mut renderer, im_popup.wl_surface(), popup_pos, 1.0, 1.0, Kind::Unspecified);
                                    }
                                }
                            }
                        }

                        // Collect X11 override-redirect window elements (input method popups, tooltips)
                        for xs in &state.xw.or_surfaces {
                            if let Some(wl) = xs.wl_surface() {
                                let geo = xs.geometry();
                                // 关键修复：X11 OR 窗口的 geometry() 是 X11 root window 绝对坐标。
                                // 渲染管线使用 output 局部坐标，所以必须减去当前 output 的 (ox, oy) 偏移。
                                // 否则在多屏场景下，OR 窗口会渲染到错误的 output 上（甚至屏幕外）。
                                // 全屏应用占据整个 output 时，这一转换也保证 OR popup 出现在正确位置。
                                let (ox, oy, _, _) = state.output_sizes.get(oi).copied().unwrap_or((0, 0, state.osize.w, state.osize.h));
                                let render_pos = Point::<i32, Physical>::from((geo.loc.x - ox, geo.loc.y - oy));
                                if is_focused_output {
                                    or_elems.extend(
                                        render_elements_from_surface_tree(&mut renderer, &wl, render_pos, 1.0, 1.0, Kind::Unspecified)
                                    );
                                }
                            }
                        }

                        // ═══════════════════════════════════════════════
                        // Phase 2: bind + render everything (full control)
                        // ═══════════════════════════════════════════════
                        let mut target = renderer.bind(&mut dmabuf)?;
                        let sp_size = Size::<i32, Physical>::new(ow, oh);
                        let mut f = renderer.render(&mut target, sp_size, Transform::Normal)?;
                        let dmg = Rectangle::from_size(sp_size);

                        // Step 1: Wallpaper
                        // ── Lock screen: skip all normal rendering ──
                        if state.lock_state.locked {
                            // 计算锁屏激活以来的时间（用于基于时间的动画）
                            let lock_elapsed = state.lock_state.time
                                .map(|t| t.elapsed().as_secs_f32())
                                .unwrap_or(0.0);
                            if is_focused_output {
                                // 焦点屏幕：完整锁屏 UI（时钟 + 密码输入框）
                                layout::render_lock_screen(
                                    &mut f, &state.cfg, ow, oh,
                                    time_secs, lock_elapsed,
                                    &state.lock_state.input, state.lock_state.wrong, state.lock_state.shake,
                                    state.lock_state.style, state.lock_state.is_authenticating(),
                                );
                            } else {
                                // 其他屏幕：暗色覆盖 + 同风格背景
                                layout::render_lock_screen_dim(&mut f, &state.cfg, ow, oh, lock_elapsed, state.lock_state.style);
                            }
                            let sync = f.finish()?;
                            drop(target);
                            out.buf_surf.queue_buffer(Some(sync), None, ())?;
                            out.pending_flip = true;
                            continue; // skip all other rendering for this output
                        }
                        if let Some(ref _tex) = state.wallpaper_texture {
                            if let Some(ref wp) = state.wallpaper_cache.pixels {
                                let (ww, wh) = state.wallpaper_cache.size;
                                // Direct GPU texture blit — no element system, no flicker
                                let _ = f.render_texture_from_to(
                                    _tex,
                                    Rectangle::from_size(Size::from((ww as f64, wh as f64))),
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
                            layout::render_wallpaper(&mut f, &state.cfg, ow, oh, state.frame);
                        }

                        // Step 2: Window surfaces + XDG popups — render per-window, painter's algorithm
                        // Background windows first, focused window last. Popups rendered on top of their parent window.
                        let fi = out_ws_focus_idx.unwrap_or(0);
                        for (i, elems) in win_elems.iter().enumerate() {
                            if i != fi {
                                if !elems.is_empty() {
                                    draw_render_elements(&mut f, 1.0, elems, &[dmg])?;
                                }
                                // XDG popups for this background window
                                if let Some(pe) = popup_elems.get(i) {
                                    if !pe.is_empty() {
                                        draw_render_elements(&mut f, 1.0, pe, &[dmg])?;
                                    }
                                }
                            }
                        }
                        // Focused window rendered last (on top of other windows)
                        if let Some(elems) = win_elems.get(fi) {
                            if !elems.is_empty() {
                                draw_render_elements(&mut f, 1.0, elems, &[dmg])?;
                            }
                        }
                        // Focused window's XDG popups (on top of focused window)
                        if let Some(pe) = popup_elems.get(fi) {
                            if !pe.is_empty() {
                                draw_render_elements(&mut f, 1.0, pe, &[dmg])?;
                            }
                        }

                        // Step 2.5: IM popup (只在焦点屏幕上显示)
                        if is_focused_output && !im_elems.is_empty() {
                            draw_render_elements(&mut f, 1.0, &im_elems, &[dmg])?;
                        }

                        // Step 3: Window decorations — 每个 output 都渲染自己工作区的装饰
                        if fullscreen.is_none() {
                            let order = out_ws.effective_order();
                            for (i, _) in order.iter().enumerate() {
                                let (x, y, _, _) = layout::slot(i, n_total, ow, oh, bar_h, &state.cfg, state.workspaces[out_ws_idx].layout, state.workspaces[out_ws_idx].split);
                                let (dx, dy) = state.layout_anim.offset_for(&order[i], (x, y)).unwrap_or((0, 0));
                                layout::render_window_decorations_anim(
                                    &mut f, &state.cfg, i, n_total, out_ws_focus_idx,
                                    ow, oh, bar_h, state.workspaces[out_ws_idx].layout, state.workspaces[out_ws_idx].split,
                                    ws_offset + dx, dy
                                );
                            }
                        }

                        // Step 4: Scratchpad — background FIRST, then surface ON TOP
                        if let Some((sp_x, sp_y, sp_w, sp_h)) = scratchpad_data {
                            let bw = 4;
                            let accent = crate::config::parse_color(&state.cfg.colors.focus_border);
                            let border = layout::opaque(accent.0, accent.1, accent.2);
                            let sp_bg = layout::opaque(0.06, 0.06, 0.10);
                            // Background (opaque, covers windows below)
                            f.clear(sp_bg, &[layout::rect(sp_x - bw, sp_y - bw, sp_w + 2 * bw, sp_h + 2 * bw)]).ok();
                            // Border (top accent line)
                            f.clear(border, &[layout::rect(sp_x - bw, sp_y - bw, sp_w + 2 * bw, bw)]).ok();
                            f.clear(border, &[layout::rect(sp_x - bw, sp_y + sp_h, sp_w + 2 * bw, bw)]).ok();
                            f.clear(border, &[layout::rect(sp_x - bw, sp_y, bw, sp_h)]).ok();
                            f.clear(border, &[layout::rect(sp_x + sp_w, sp_y, bw, sp_h)]).ok();
                            // Scratchpad label
                            crate::text_render::draw_text(&mut f, "SCRATCHPAD", sp_x + 6, sp_y - 22, 14.0, accent);
                            // Scratchpad terminal content — rendered AFTER background
                            draw_render_elements(&mut f, 1.0, &sp_elems, &[dmg])?;
                        }

                        // Step 4.5: X11 override-redirect windows (只在焦点屏幕)
                        // Must render on top of window content but below headbar
                        if is_focused_output && !or_elems.is_empty() {
                            draw_render_elements(&mut f, 1.0, &or_elems, &[dmg])?;
                        }

                        // Step 5: Headbar — 每个 output 显示自己的活跃工作区
                        {
                            let ws_counts: Vec<usize> = state.workspaces.iter().map(|w| w.tops.len() + w.x11_surfaces.len()).collect();
                            layout::render_headbar(&mut f, &state.cfg, ow, oh, n_windows, out_ws_focus_idx, time_secs, &out_window_title, out_ws_idx, NUM_WORKSPACES, &ws_counts, state.cpu_usage, state.mem_usage);
                        }

                        // Step 6: Notifications — 只在鼠标所在的 output 上显示
                        if is_focused_output && !state.notifications.is_empty() {
                            let accent = crate::config::parse_color(&state.cfg.colors.focus_border);
                            let notif_data: Vec<(String, std::time::Instant, std::time::Duration)> = state.notifications.iter()
                                .map(|n| (n.text.clone(), n.created, n.duration)).collect();
                            layout::render_notifications(&mut f, &notif_data, ow, state.cfg.bar.height, accent);
                        }

                        // Step 7: Launcher — 只在鼠标所在的 output 上显示
                        if is_focused_output && state.launcher.visible {
                            let filtered = state.launcher.filtered();
                            let lw = ow * 3 / 4;
                            let max_items = 12usize;
                            let item_h: i32 = 36;
                            let header_h: i32 = 48;
                            let n_items = filtered.len().min(max_items);
                            let lh = header_h + (n_items as i32) * item_h + 20;
                            let lx = (ow - lw) / 2;
                            let ly = bar_h + 24;

                            // 毛玻璃背景（使用上一帧缓存的模糊纹理）
                            if let Some(ref blur_tex) = state.launcher_blur_tex {
                                let _ = f.render_texture_from_to(
                                    blur_tex,
                                    Rectangle::from_size(Size::from((state.launcher_blur_size.0 as f64, state.launcher_blur_size.1 as f64))),
                                    Rectangle::from_loc_and_size((lx, ly), (lw, lh)),
                                    &[Rectangle::from_loc_and_size((lx, ly), (lw, lh))],
                                    &[Rectangle::from_loc_and_size((lx, ly), (lw, lh))],
                                    Transform::Normal,
                                    1.0,
                                    None,
                                    &[],
                                );
                            } else {
                                f.clear(layout::opaque(0.08, 0.08, 0.14),
                                    &[layout::rect(lx, ly, lw, lh)]).ok();
                            }
                            // 渲染 launcher UI 元素
                            layout::render_launcher(&mut f, &state.cfg, ow, oh, &state.launcher.query, &filtered, state.launcher.selected);
                        }

                        // Step 8: Cursor — 只在鼠标所在的 output 上渲染（坐标需要转换）
                        if is_focused_output {
                            let (ox, _oy, _ow, _oh) = state.output_sizes.get(oi).copied().unwrap_or((0, 0, 0, 0));
                            let cx = state.pointer_pos.0 as i32 - ox - state.cursor_img.hotspot_x as i32;
                            let cy = state.pointer_pos.1 as i32 - _oy - state.cursor_img.hotspot_y as i32;
                            state.cursor_img.render_batched(&mut f, cx, cy);
                        }

                        // Step 9: Screenshot area selection overlay
                        if is_focused_output && state.screenshot.selecting {
                            if let Some(rect) = state.screenshot.selection_rect() {
                                screenshot::render_selection_overlay(&mut f, ow, oh, rect);
                            }
                        }

                        let sync = f.finish()?;
                        // drop f 释放对 target 的借用

                        // Step 9.5: 毛玻璃模糊纹理更新（launcher 可见时，每 10 帧更新一次）
                        if is_focused_output && state.launcher.visible && state.frame % 10 == 0 {
                            let lw = ow * 3 / 4;
                            let max_items = 12usize;
                            let item_h: i32 = 36;
                            let header_h: i32 = 48;
                            let n_items = state.launcher.filtered().len().min(max_items);
                            let lh = header_h + (n_items as i32) * item_h + 20;
                            let lx = (ow - lw) / 2;
                            let ly = bar_h + 24;
                            let blur_scale = 8u32;
                            let small_w = (lw as u32 / blur_scale).max(1);
                            let small_h = (lh as u32 / blur_scale).max(1);
                            let region = Rectangle::from_loc_and_size((lx, ly), (lw, lh));
                            if let Ok(mapping) = renderer.copy_framebuffer(&target, region, Fourcc::Abgr8888) {
                                if let Ok(pixels) = renderer.map_texture(&mapping) {
                                    let mut blurred = vec![0u8; (small_w * small_h * 4) as usize];
                                    for sy in 0..small_h {
                                        for sx in 0..small_w {
                                            let src_x = (sx * blur_scale) as usize;
                                            let src_y = (sy * blur_scale) as usize;
                                            let mut r = 0u32; let mut g = 0u32; let mut b = 0u32; let mut count = 0u32;
                                            for dy in 0..blur_scale {
                                                for dx in 0..blur_scale {
                                                    let px = (src_x + dx as usize).min((lw as usize).saturating_sub(1));
                                                    let py = (src_y + dy as usize).min((lh as usize).saturating_sub(1));
                                                    let idx = (py * lw as usize + px) * 4;
                                                    if idx + 3 < pixels.len() {
                                                        r += pixels[idx] as u32;
                                                        g += pixels[idx+1] as u32;
                                                        b += pixels[idx+2] as u32;
                                                        count += 1;
                                                    }
                                                }
                                            }
                                            if count > 0 {
                                                let di = ((sy * small_w + sx) * 4) as usize;
                                                blurred[di]   = (r / count * 7 / 10) as u8;
                                                blurred[di+1] = (g / count * 7 / 10) as u8;
                                                blurred[di+2] = (b / count * 7 / 10) as u8;
                                                blurred[di+3] = 255;
                                            }
                                        }
                                    }
                                    if let Ok(tex) = renderer.import_memory(
                                        &blurred,
                                        Fourcc::Abgr8888,
                                        Size::new(small_w as i32, small_h as i32),
                                        false,
                                    ) {
                                        state.launcher_blur_tex = Some(tex);
                                        state.launcher_blur_size = (small_w, small_h);
                                    }
                                }
                            }
                        }

                        // Step 10: 执行待处理的截图请求（finish 后 framebuffer 完整，target 仍可用）
                        if is_focused_output {
                            if let Some(req) = state.pending_screenshot.take() {
                                let area = match &req {
                                    screenshot::ScreenshotRequest::Area(x, y, w, h) => Some((*x, *y, *w, *h)),
                                    screenshot::ScreenshotRequest::Full => None,
                                };
                                use smithay::backend::allocator::Fourcc;
                                use smithay::backend::renderer::Renderer;
                                let region = Rectangle::from_size((ow, oh).into());
                                // 关键：使用 Abgr8888 而非 Xrgb8888。
                                // Abgr8888 在 little-endian 内存中 = R,G,B,A 字节序（RGBA），
                                // 避免 XRGB 格式的 BGR/RGB 字节序歧义。
                                // 同时 GlesRenderer 会自动做 Y-flip（bottom-up → top-down），
                                // 所以这里不再做行反转。
                                match renderer.copy_framebuffer(&target, region, Fourcc::Abgr8888) {
                                    Ok(mapping) => {
                                        match renderer.map_texture(&mapping) {
                                            Ok(pixels) => {
                                                let w = ow as u32;
                                                let h = oh as u32;
                                                let row_len = w as usize * 4;
                                                // Abgr8888 little-endian = R,G,B,A 像素序
                                                let mut rgba = Vec::with_capacity(pixels.len());
                                                for row in 0..h as usize {
                                                    let start = row * row_len;
                                                    let end = start + row_len;
                                                    if end <= pixels.len() {
                                                        // 字节序: R,G,B,A → R,G,B,A (无需翻转)
                                                        rgba.extend_from_slice(&pixels[start..end]);
                                                    }
                                                }
                                                let result = screenshot::save_screenshot(&rgba, w, h, area);
                                                state.screenshot_result = Some(result);
                                            }
                                            Err(e) => {
                                                tracing::warn!("📸 map_texture failed: {:?}", e);
                                                state.screenshot_result = Some((String::new(), None));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("📸 copy_framebuffer failed: {:?}", e);
                                        state.screenshot_result = Some((String::new(), None));
                                    }
                                }
                            }
                        }

                        drop(target);

                        out.buf_surf.queue_buffer(Some(sync), None, ())?;
                        out.pending_flip = true;
                    }
                    Err(e) => {
                        if state.frame == 0 { error!("❌ {e:?}"); }
                    }
                }
            }
            state.dirty = false;

            // 处理截图结果
            if let Some((path, png_data)) = state.screenshot_result.take() {
                if path.is_empty() {
                    state.notify("Screenshot failed".to_string());
                } else if let Some(png) = png_data {
                    state.set_clipboard_png(path.clone(), png);
                    state.notify(format!("Saved: {} (copied to clipboard)", path));
                } else {
                    state.notify(format!("Saved: {} (clipboard failed)", path));
                }
                state.dirty = true;
            }

            // 发送 frame callback
            let now = start.elapsed().as_millis() as u32;
            for s in state.xdg.toplevel_surfaces() { send_frames(s.wl_surface(), now); }
            // XDG popup surfaces (browser menus, context menus, etc.)
            for s in state.xdg.toplevel_surfaces() {
                for (popup, _) in PopupManager::popups_for_surface(s.wl_surface()) {
                    send_frames(popup.wl_surface(), now);
                }
            }
            // IM popup surface (fcitx5 candidate box) — needs frame callback to commit buffer
            if let Some(ref im_popup) = state.im_popup {
                if im_popup.alive() { send_frames(im_popup.wl_surface(), now); }
            }
            // Scratchpad surface — needs frame callback to commit buffer
            if let Some(ref sp_surf) = state.scratchpad.surface {
                if sp_surf.alive() { send_frames(sp_surf.wl_surface(), now); }
            }
            // X11 surfaces — need frame callbacks to update
            for xs in &state.workspaces[state.active_ws].x11_surfaces {
                if let Some(wl) = xs.wl_surface() {
                    send_frames(&wl, now);
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
            if state.ws_anim.start.map(|s| (s.elapsed().as_millis() as u64) < state.ws_anim.duration_ms).unwrap_or(false) {
                state.dirty = true;
            }
            // 布局动画进行中时持续请求渲染
            if state.layout_anim.is_active() {
                state.dirty = true;
            }
            // 锁屏动画需要持续重绘（frame 驱动动画，必须保证 dirty 始终为 true）
            if state.lock_state.locked {
                state.dirty = true;
            }
            state.frame += 1;
            // Drain D-Bus notifications into toast system
            {
                let dbus_notifs: Vec<_> = state.dbus_notifications.lock()
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
            if state.frame % 60 == 0 { state.popup_manager.cleanup(); }
            if state.frame == 1 { info!("✅ 第一帧渲染！"); }
            if state.frame % 600 == 0 { info!("📊 {} 帧", state.frame); }
        }

        eloop.dispatch(Some(Duration::from_millis(16)), &mut state)?;
        // 时钟每秒更新（bar enabled 时）
        if state.frame % 60 == 0 && state.cfg.bar.enabled { state.dirty = true; }
        // CPU/MEM 状态每 5 秒更新
        if state.frame % 300 == 0 {
            state.update_cpu_usage();
            state.update_mem_usage();
            state.dirty = true;
        }

        if let Ok(Some(stream)) = listener.accept() {
            clients.push(display.handle().insert_client(stream, Arc::new(ClientState::default()))?);
        }
        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }

    info!("👋"); Ok(())
}

delegate_xdg_shell!(App);
delegate_xdg_decoration!(App);
delegate_compositor!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_data_device!(App);
delegate_primary_selection!(App);
delegate_output!(App);
delegate_input_method_manager!(App);
delegate_text_input_manager!(App);
delegate_virtual_keyboard_manager!(App);
smithay::delegate_xwayland_shell!(App);

// ── XWayland Handlers ──────────────────────────────────────

impl smithay::wayland::xwayland_shell::XWaylandShellHandler for App {
    fn xwayland_shell_state(&mut self) -> &mut smithay::wayland::xwayland_shell::XWaylandShellState {
        &mut self.xw.shell
    }

    fn surface_associated(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _wl_surface: WlSurface,
        _surface: smithay::xwayland::X11Surface,
    ) {
        tracing::info!("🔗 XWayland surface associated (wl_surface ready)");
        self.dirty = true;
    }
}

impl smithay::xwayland::XwmHandler for App {
    fn xwm_state(&mut self, _xwm: smithay::xwayland::xwm::XwmId) -> &mut smithay::xwayland::X11Wm {
        self.xw.xwm.as_mut().expect("XwmHandler called but X11Wm not ready")
    }

    fn new_window(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        tracing::info!("🆕 X11 new_window: class='{}' title='{}'", window.class(), window.title());
    }

    fn new_override_redirect_window(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        self.xw.on_new_or_window(window);
        self.dirty = true;
    }

    fn map_window_request(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        tracing::info!("🗺️  X11 map_request: class='{}' title='{}'", window.class(), window.title());

        if let Err(e) = window.set_mapped(true) {
            tracing::warn!("⚠️  X11 set_mapped failed: {:?}", e);
            return;
        }

        let wid = window.window_id();
        let ws = &mut self.workspaces[self.active_ws];
        let is_new = !ws.x11_surfaces.iter().any(|s| s.window_id() == wid);
        if is_new {
            ws.x11_surfaces.push(window.clone());
            ws.rebuild_order();
        }

        // Focus the new X11 window
        if let Some(wl) = window.wl_surface() {
            self.workspaces[self.active_ws].focus = Some(wl.clone());
            let kbd = self.kbd.clone();
            let serial = SERIAL_COUNTER.next_serial();
            kbd.set_focus(self, Some(wl), serial);
        }
        self.do_layout_animated();
        self.dirty = true;
    }

    fn mapped_override_redirect_window(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        tracing::info!("🗺️  X11 OR mapped: class='{}'", window.class());
        // Re-add in case it was removed by unmapped_window (fcitx5 reuses the same X11 window)
        let wid = window.window_id();
        if !self.xw.or_surfaces.iter().any(|s| s.window_id() == wid) {
            self.xw.or_surfaces.push(window);
        }
        self.dirty = true;
    }

    fn unmapped_window(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        tracing::info!("🗑️  X11 unmapped: class='{}'", window.class());
        let wid = window.window_id();
        for ws in &mut self.workspaces {
            ws.x11_surfaces.retain(|s| s.window_id() != wid);
            ws.rebuild_order();
        }
        self.xw.or_surfaces.retain(|s| s.window_id() != wid);
        self.do_layout_animated();
        // Refocus — find the last window in effective_order
        let order = self.workspaces[self.active_ws].effective_order();
        if let Some((_, slot)) = order.iter().enumerate().last() {
            let surf = match slot {
                WindowSlot::Wl(idx) => self.workspaces[self.active_ws].tops.get(*idx).map(|tl| tl.wl_surface().clone()),
                WindowSlot::X11(idx) => self.workspaces[self.active_ws].x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()),
            };
            if let Some(surf) = surf {
                self.workspaces[self.active_ws].focus = Some(surf.clone());
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(surf), serial);
            }
        }
        self.dirty = true;
    }

    fn destroyed_window(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        tracing::info!("💥 X11 destroyed: class='{}'", window.class());
        let wid = window.window_id();
        // 重新映射 prev_positions（X11 窗口索引移位）
        for ws_idx in 0..self.workspaces.len() {
            if let Some(removed_idx) = self.workspaces[ws_idx].x11_surfaces.iter().position(|s| s.window_id() == wid) {
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
                WindowSlot::Wl(idx) => self.workspaces[self.active_ws].tops.get(*idx).map(|tl| tl.wl_surface().clone()),
                WindowSlot::X11(idx) => self.workspaces[self.active_ws].x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()),
            };
            if let Some(surf) = surf {
                self.workspaces[self.active_ws].focus = Some(surf.clone());
                let kbd = self.kbd.clone();
                let serial = SERIAL_COUNTER.next_serial();
                kbd.set_focus(self, Some(surf), serial);
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
        use smithay::wayland::selection::data_device::{request_data_device_client_selection, current_data_device_selection_userdata};

        if selection == smithay::wayland::selection::SelectionTarget::Clipboard {
            // 先检查合成器是否拥有选区（截图等 compositor-provided selection）
            if let Some(user_data) = current_data_device_selection_userdata::<App>(&self.seat) {
                tracing::info!("📋 XwmHandler::send_selection: compositor owns clipboard, writing {} bytes to fd", user_data.len());
                let buf: Arc<[u8]> = user_data.clone();
                std::thread::spawn(move || {
                    use std::io::Write;
                    if let Err(err) = smithay::reexports::rustix::fs::fcntl_setfl(&fd, smithay::reexports::rustix::fs::OFlags::empty()) {
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
        match request_data_device_client_selection::<App>(
            &self.seat,
            mime_type,
            fd,
        ) {
            Ok(()) => tracing::info!("📋 XwmHandler::send_selection: forwarded to Wayland client"),
            Err(e) => tracing::warn!("📋 XwmHandler::send_selection: request failed: {:?}", e),
        }
    }

    fn maximize_request(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        self.xw.ack_with_current_geometry(&window);
    }

    fn unmaximize_request(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        self.xw.ack_with_current_geometry(&window);
    }

    fn fullscreen_request(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
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
                        WindowSlot::X11(idx) => ws.x11_surfaces.get(*idx).map(|s| s.window_id() == wid).unwrap_or(false),
                    };
                    let focus = if m {
                        order.get(i).and_then(|s2| match s2 {
                            WindowSlot::X11(idx) => ws.x11_surfaces.get(*idx).and_then(|xs| xs.wl_surface()),
                            _ => None,
                        })
                    } else { None };
                    (m, focus)
                };
                if is_match {
                    self.workspaces[ws_idx].fullscreen = Some(i);
                    if ws_idx != self.active_ws { self.active_ws = ws_idx; }
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

    fn unfullscreen_request(&mut self, _xwm: smithay::xwayland::xwm::XwmId, window: smithay::xwayland::X11Surface) {
        let _ = window.set_fullscreen(false);
        let wid = window.window_id();
        for ws_idx in 0..self.workspaces.len() {
            let ws = &self.workspaces[ws_idx];
            if let Some(fi) = ws.fullscreen {
                let order = ws.effective_order();
                let matches = match order.get(fi) {
                    Some(WindowSlot::X11(idx)) => ws.x11_surfaces.get(*idx).map(|s| s.window_id() == wid).unwrap_or(false),
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

    fn minimize_request(&mut self, _xwm: smithay::xwayland::xwm::XwmId, _window: smithay::xwayland::X11Surface) {}
    fn unminimize_request(&mut self, _xwm: smithay::xwayland::xwm::XwmId, _window: smithay::xwayland::X11Surface) {}

    fn allow_selection_access(&mut self, _xwm: smithay::xwayland::xwm::XwmId, _selection: smithay::wayland::selection::SelectionTarget) -> bool {
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
        if selection == smithay::wayland::selection::SelectionTarget::Clipboard && !mime_types.is_empty() {
            use smithay::wayland::selection::data_device::set_data_device_selection;
            // 使用 magic bytes 标记这是 X11 代理选区
            // SelectionHandler::send_selection 检测到这个标记会用 X11Wm::send_selection 获取实际数据
            let user_data: Arc<[u8]> = Arc::from(&b"X11_PROXY\x00"[..]);
            set_data_device_selection::<App>(
                &self.dh,
                &self.seat,
                mime_types,
                user_data,
            );
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
// Tell clients to use server-side decorations (no CSD titlebar)
impl XdgDecorationHandler for App {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
        toplevel.send_configure();
    }
}
