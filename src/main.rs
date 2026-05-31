// Anchor — Wayland tiling compositor v9
// Features: multi-workspace, multi-monitor, wallpaper, config
// Config: ~/.config/anchor/config.toml

mod config;
mod layout;
use layout::LayoutPreset;
mod font;
mod text_render;
mod block_linear;
mod wallpaper;
mod cursor;

use std::{
    os::unix::io::OwnedFd,
    os::fd::AsRawFd,
    sync::Arc,
    time::{Duration, Instant},
};

use config::Config;
use smithay::{
    backend::{
        allocator::{Format, Fourcc, Modifier,
            dmabuf::{Dmabuf, DmabufAllocator},
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice}},
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, GbmBufferedSurface},
        egl::{EGLContext, EGLDisplay},
        input::{Axis, ButtonState, InputEvent, KeyState, PointerAxisEvent},
        libinput::LibinputInputBackend,
        renderer::{Bind, Frame, Renderer,
            gles::{GlesError, GlesRenderer},
            element::{surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement}, Kind},
            utils::{draw_render_elements, on_commit_buffer_handler}, Color32F},
        session::{Session, Event as SessionEvent, libseat::LibSeatSession},
    },
    delegate_compositor, delegate_data_device, delegate_input_method_manager,
    delegate_output, delegate_seat, delegate_shm, delegate_text_input_manager,
    delegate_virtual_keyboard_manager, delegate_xdg_shell,
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
                ServerDndGrabHandler}},
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        shm::{ShmHandler, ShmState},
        text_input::TextInputManagerState,
        virtual_keyboard::VirtualKeyboardManagerState,
    },
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{Client, ListeningSocket,
    backend::{ClientData, ClientId, DisconnectReason}, protocol::wl_buffer};
use tracing::{error, info, warn};

// ── Workspace ──────────────────────────────────────────────

const NUM_WORKSPACES: usize = 9;

struct Workspace {
    tops: Vec<ToplevelSurface>,
    focus: Option<WlSurface>,
    fullscreen: Option<usize>,
    layout: LayoutPreset,
    split: layout::SplitDir,
    /// 下一个新窗口使用的分割方向（设一次消费一次）
    pending_split: Option<layout::SplitDir>,
}

impl Workspace {
    fn new() -> Self {
        Self { tops: Vec::new(), focus: None, fullscreen: None, layout: LayoutPreset::default(), split: layout::SplitDir::Horizontal, pending_split: None }
    }
}

// ── AnchorOutput ────────────────────────────────────────────

struct AnchorOutput {
    output: Output,
    size: Size<i32, Logical>,
    crtc: crtc::Handle,
    connector: connector::Handle,
    buf_surf: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    pending_flip: bool,
    position: (i32, i32),
}

// ── App ──────────────────────────────────────────────────

struct Notification {
    text: String,
    created: std::time::Instant,
    duration: std::time::Duration,
}

struct App {
    comp: CompositorState, xdg: XdgShellState, shm: ShmState, seat_state: SeatState<Self>,
    dd: DataDeviceState, seat: Seat<Self>,
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
    notifications: Vec<Notification>,
    scratchpad: Option<std::process::Child>,
    scratchpad_visible: bool,
    scratchpad_surface: Option<ToplevelSurface>,
    scratchpad_pending: bool,
    // 内置启动器
    launcher_visible: bool,
    launcher_query: String,
    launcher_apps: Vec<(String, String)>, // (name, exec)
    launcher_selected: usize,
    // 工作区切换动画
    ws_anim: WsAnimation,
    // 多显示器尺寸信息（用于鼠标穿越）
    output_sizes: Vec<(i32, i32, i32, i32)>, // (x, y, w, h) per output
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

impl BufferHandler for App { fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {} }

impl App {
    /// 当前工作区的窗口列表
    fn tops(&self) -> &Vec<ToplevelSurface> { &self.workspaces[self.active_ws].tops }
    fn tops_mut(&mut self) -> &mut Vec<ToplevelSurface> { &mut self.workspaces[self.active_ws].tops }

    fn focus_idx(&self) -> Option<usize> {
        let ws = &self.workspaces[self.active_ws];
        ws.focus.as_ref().and_then(|s| ws.tops.iter().position(|tl| tl.wl_surface() == s))
    }

    /// Find the surface under the pointer.
    /// Returns (WlSurface, slot_offset_in_global_space).
    /// Smithay computes surface-local coords as: event.location - slot_offset
    fn pointer_focus(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        let px = self.pointer_pos.0 as i32;
        let py = self.pointer_pos.1 as i32;
        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        if py < bar_h { return None; }
        let ws = &self.workspaces[self.active_ws];

        // Fullscreen: entire area below bar
        if let Some(fi) = ws.fullscreen {
            if let Some(tl) = ws.tops.get(fi) {
                return Some((tl.wl_surface().clone(), Point::from((0.0, bar_h as f64))));
            }
        }

        // Normal: hit-test each window slot
        for (i, tl) in ws.tops.iter().enumerate() {
            let (x, y, w, h) = layout::slot(i, ws.tops.len(), self.osize.w, self.osize.h, bar_h, &self.cfg, ws.layout, ws.split);
            if px >= x && px < x + w && py >= y && py < y + h {
                return Some((tl.wl_surface().clone(), Point::from((x as f64, y as f64))));
            }
        }

        // Popup hit-test: if click is outside all slots, check popup surfaces
        // Popups can extend beyond their parent's slot boundaries
        for popup in self.xdg.popup_surfaces() {
            // Use compositor's with_states to get the popup's geometry
            let popup_surf = popup.wl_surface();
            if let Some(parent) = popup.get_parent_surface() {
                // Find the parent's slot position
                for (i, tl) in ws.tops.iter().enumerate() {
                    if tl.wl_surface() == &parent {
                        let (x, y, _w, _h) = layout::slot(i, ws.tops.len(), self.osize.w, self.osize.h, bar_h, &self.cfg, ws.layout, ws.split);
                        // The popup is a child of this toplevel
                        // Return the parent surface — Smithay will route to the popup via subsurface tree
                        return Some((parent, Point::from((x as f64, y as f64))));
                    }
                }
            }
        }

        None
    }

    fn fullscreen(&self) -> Option<usize> { self.workspaces[self.active_ws].fullscreen }
    fn set_fullscreen(&mut self, v: Option<usize>) { self.workspaces[self.active_ws].fullscreen = v; }

    fn do_layout(&mut self) {
        let ws_idx = self.active_ws;
        let n = self.workspaces[ws_idx].tops.len();
        if n == 0 { return; }
        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };

        // 修正 fullscreen
        if let Some(fi) = self.workspaces[ws_idx].fullscreen {
            if fi >= n { self.workspaces[ws_idx].fullscreen = None; }
        }
        let fullscreen = self.workspaces[ws_idx].fullscreen;

        let osize_w = self.osize.w;
        let osize_h = self.osize.h;
        let gap = self.cfg.layout.gap;
        let margin = self.cfg.layout.margin;

        if let Some(fi) = fullscreen {
            for (i, tl) in self.workspaces[ws_idx].tops.iter().enumerate() {
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
        } else {
            for (i, tl) in self.workspaces[ws_idx].tops.iter().enumerate() {
                let (_x, _y, w, h) = layout::slot(i, n, osize_w, osize_h, bar_h, &self.cfg, self.workspaces[self.active_ws].layout, self.workspaces[self.active_ws].split);
                tl.with_pending_state(|st| {
                    st.states.set(xdg_toplevel::State::Activated);
                    st.states.unset(xdg_toplevel::State::Fullscreen);
                    st.states.set(xdg_toplevel::State::TiledLeft);
                    st.states.set(xdg_toplevel::State::TiledRight);
                    st.states.set(xdg_toplevel::State::TiledTop);
                    st.states.set(xdg_toplevel::State::TiledBottom);
                    st.size = Some((w, h).into());
                });
                info!("📐 layout 窗口 #{}: {}x{}", i, w, h);
                tl.send_configure();
            }
        }
    }

    fn notify(&mut self, text: impl Into<String>) {
        self.notifications.push(Notification {
            text: text.into(),
            created: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(3),
        });
    }

    fn load_apps(terminal_cmd: &str) -> Vec<(String, String)> {
        let mut apps = Vec::new();
        let dirs = [
            "/usr/share/applications".to_string(),
            format!("{}/.local/share/applications", std::env::var("HOME").unwrap_or_default()),
        ];
        for dir in &dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.path().to_str() {
                        if name.ends_with(".desktop") {
                            if let Ok(content) = std::fs::read_to_string(name) {
                                let mut app_name = String::new();
                                let mut app_exec = String::new();
                                let mut is_terminal = false;
                                let mut no_display = false;
                                for line in content.lines() {
                                    if line.starts_with("Name=") && app_name.is_empty() {
                                        app_name = line[5..].to_string();
                                    }
                                    if line.starts_with("Exec=") && app_exec.is_empty() {
                                        let exec = &line[5..];
                                        // 移除 %参数占位符
                                        app_exec = exec.split_whitespace().next().unwrap_or(exec).to_string();
                                    }
                                    if line.starts_with("Terminal=true") { is_terminal = true; }
                                    if line.starts_with("NoDisplay=true") { no_display = true; }
                                }
                                if !app_name.is_empty() && !app_exec.is_empty() && !no_display {
                                    if is_terminal {
                                        app_exec = format!("{} {}", terminal_cmd, app_exec);
                                    }
                                    apps.push((app_name, app_exec));
                                }
                            }
                        }
                    }
                }
            }
        }
        apps.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        apps.dedup_by(|a, b| a.0 == b.0);
        apps
    }

    fn toggle_launcher(&mut self) {
        if self.launcher_visible {
            self.launcher_visible = false;
            self.launcher_query.clear();
            self.launcher_apps.clear();
        } else {
            let all_apps = Self::load_apps(&self.cfg.terminal.command);
            self.launcher_apps = all_apps;
            self.launcher_query.clear();
            self.launcher_selected = 0;
            self.launcher_visible = true;
        }
        self.dirty = true;
    }

    fn launcher_filter(&self) -> Vec<(usize, &(String, String))> {
        let q = self.launcher_query.to_lowercase();
        self.launcher_apps.iter().enumerate()
            .filter(|(_, (name, _))| name.to_lowercase().contains(&q))
            .collect()
    }

    fn launcher_select(&mut self) {
        let filtered = self.launcher_filter();
        if let Some((_, (_, exec))) = filtered.get(self.launcher_selected) {
            let exec_cmd = exec.clone();
            info!("🚀 启动器: {}", exec_cmd);
            std::process::Command::new("sh")
                .arg("-c")
                .arg(&exec_cmd)
                .env("WAYLAND_DISPLAY", "wayland-anchor")
                .env("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }))
                .spawn().ok();
        }
        self.launcher_visible = false;
        self.launcher_query.clear();
        self.dirty = true;
    }

    fn toggle_scratchpad(&mut self) {
        if self.scratchpad_visible {
            // 隐藏：杀掉 scratchpad 进程 + 清除 surface
            if let Some(ref mut child) = self.scratchpad {
                let _ = child.kill();
                let _ = child.wait();
            }
            self.scratchpad = None;
            self.scratchpad_surface = None;
            self.scratchpad_visible = false;
            self.notify("Scratchpad hidden");
        } else {
            // 显示：启动终端（使用用户配置的终端）
            self.scratchpad_pending = true;
            let uid = unsafe { libc::getuid() };
            match std::process::Command::new(&self.cfg.terminal.command)
                .env("WAYLAND_DISPLAY", "wayland-anchor")
                .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
                .spawn()
            {
                Ok(child) => {
                    self.scratchpad = Some(child);
                    self.scratchpad_visible = true;
                    self.notify(&format!("Scratchpad ({})", self.cfg.terminal.command));
                }
                Err(e) => {
                    self.scratchpad_pending = false;
                    self.notify(&format!("Failed to launch {}: {}", self.cfg.terminal.command, e));
                }
            }
        }
        self.dirty = true;
    }

    fn drain_notifications(&mut self) {
        let now = std::time::Instant::now();
        self.notifications.retain(|n| now.duration_since(n.created) < n.duration);
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
            }
            _ => return,
        }
        self.do_layout();
        self.dirty = true;
    }

    /// 切换到指定工作区
    fn switch_workspace(&mut self, target: usize) {
        if target >= NUM_WORKSPACES || target == self.active_ws { return; }
        info!("🔀 工作区 {} → {}", self.active_ws + 1, target + 1);
        
        // 触发切换动画
        let dir = if target > self.active_ws { 1 } else { -1 };
        self.ws_anim = WsAnimation {
            start: Some(std::time::Instant::now()),
            from_ws: self.active_ws,
            to_ws: target,
            duration_ms: 200,
            direction: dir,
        };

        // 隐藏当前工作区的窗口（最小化到 1x1）
        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        let _bar_h = bar_h;
        for tl in &self.workspaces[self.active_ws].tops {
            tl.with_pending_state(|st| {
                st.states.unset(xdg_toplevel::State::Activated);
                st.states.unset(xdg_toplevel::State::Fullscreen);
                st.size = Some((1, 1).into());
            });
            tl.send_configure();
        }

        self.active_ws = target;
        self.notify(format!("Workspace {}", target + 1));

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

    /// 将当前焦点窗口移动到目标工作区
    fn move_window_to_workspace(&mut self, target: usize) {
        if target >= NUM_WORKSPACES || target == self.active_ws { return; }
        let fi = match self.focus_idx() {
            Some(i) => i,
            None => return,
        };

        let tl = match self.workspaces[self.active_ws].tops.get(fi) {
            Some(tl) => tl.clone(),
            None => return,
        };
        let surf = tl.wl_surface().clone();

        info!("📦 移动窗口 #{} → 工作区 {}", fi + 1, target + 1);
        self.notify(format!("Moved → WS {}", target + 1));

        // 从当前工作区移除
        self.workspaces[self.active_ws].tops.remove(fi);
        // 修正 fullscreen
        if let Some(fs) = self.workspaces[self.active_ws].fullscreen {
            if fs == fi { self.workspaces[self.active_ws].fullscreen = None; }
            else if fs > fi { self.workspaces[self.active_ws].fullscreen = Some(fs - 1); }
        }
        if self.workspaces[self.active_ws].focus.as_ref() == Some(&surf) {
            self.workspaces[self.active_ws].focus = self.workspaces[self.active_ws].tops.last()
                .map(|t| t.wl_surface().clone());
        }

        // 隐藏移动的窗口
        tl.with_pending_state(|st| {
            st.states.unset(xdg_toplevel::State::Activated);
            st.size = Some((1, 1).into());
        });
        tl.send_configure();

        // 添加到目标工作区
        self.workspaces[target].tops.push(tl);

        // 重新布局当前工作区
        self.do_layout();
        // 更新焦点
        if let Some(ref s) = self.workspaces[self.active_ws].focus {
            let kbd = self.kbd.clone();
            let serial = SERIAL_COUNTER.next_serial();
            kbd.set_focus(self, Some(s.clone()), serial);
        }
        self.dirty = true;
    }

    /// 用方向键交换窗口位置
    fn swap_window(&mut self, direction: Keysym) {
        let fi = match self.focus_idx() {
            Some(i) => i,
            None => return,
        };
        let n = self.workspaces[self.active_ws].tops.len();
        if n <= 1 { return; }

        let target = match direction {
            Keysym::Left | Keysym::Up => if fi > 0 { fi - 1 } else { return },
            Keysym::Right | Keysym::Down => if fi + 1 < n { fi + 1 } else { return },
            _ => return,
        };

        info!("🔄 交换窗口 {} ↔ {}", fi + 1, target + 1);
        let ws = &mut self.workspaces[self.active_ws];
        ws.tops.swap(fi, target);

        // 修正 fullscreen
        if let Some(fs) = ws.fullscreen {
            if fs == fi { ws.fullscreen = Some(target); }
            else if fs == target { ws.fullscreen = Some(fi); }
        }

        drop(ws);
        self.do_layout();
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
                        // ── 启动器模式键盘处理 ──
                        if data.launcher_visible && state == KeyState::Pressed {
                            let sym = keysym.modified_sym();
                            match sym {
                                Keysym::Escape => { data.launcher_visible = false; data.launcher_query.clear(); data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::Return => { data.launcher_select(); return FilterResult::Intercept(()); }
                                Keysym::Up => { if data.launcher_selected > 0 { data.launcher_selected -= 1; } data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::Down => { let max = data.launcher_filter().len().saturating_sub(1); if data.launcher_selected < max { data.launcher_selected += 1; } data.dirty = true; return FilterResult::Intercept(()); }
                                Keysym::BackSpace => { data.launcher_query.pop(); data.launcher_selected = 0; data.dirty = true; return FilterResult::Intercept(()); }
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
                                            data.launcher_query.push(c);
                                            data.launcher_selected = 0;
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
                                    std::process::Command::new(&data.cfg.terminal.command)
                                        .env("WAYLAND_DISPLAY", "wayland-anchor")
                                        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
                                        .env("XMODIFIERS", "@im=fcitx").env("QT_IM_MODULE", "fcitx").env("GTK_IM_MODULE", "fcitx")
                                        .env("ELECTRON_OZONE_PLATFORM_HINT", "wayland")
                                        .spawn().ok();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Escape if mods.shift => { data.run = false; return FilterResult::Intercept(()); }
                                Keysym::q => {
                                    if let Some(ref surf) = data.workspaces[data.active_ws].focus.clone() {
                                        let ws = &data.workspaces[data.active_ws];
                                        if let Some(tl) = ws.tops.iter().find(|tl| tl.wl_surface() == surf) { tl.send_close(); }
                                    }
                                    return FilterResult::Intercept(());
                                }
                                Keysym::d => {
                                    data.toggle_launcher();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::f => { data.toggle_fullscreen(); return FilterResult::Intercept(()); }
                                Keysym::p => {
                                    // 内置截图
                                    let ts = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                                    let dir = std::path::PathBuf::from(format!("{}/Pictures/Screenshots", home));
                                    let _ = std::fs::create_dir_all(&dir);
                                    let path = dir.join(format!("anchor-{}.raw", ts));
                                    let exe = std::env::current_exe().unwrap_or_default();
                                    let project_dir = exe.parent().and_then(|p| p.parent())
                                        .map(|p| p.display().to_string())
                                        .unwrap_or_else(|| ".".into());
                                    let dump_tool = format!("{}/scripts/drm-dump-fb", project_dir);
                                    // 截图使用的 DRM 设备
                                    let drm_dev = std::env::var("TITAN_DRM_DEV")
                                        .unwrap_or_else(|_| "/dev/dri/card0".into());
                                    let args = format!("timeout 3 {} {} {}", dump_tool, drm_dev, path.display());
                                    std::process::Command::new("sh").arg("-c").arg(&args).spawn().ok();
                                    data.notify("Screenshot saved");
                                    return FilterResult::Intercept(());
                                }
                                Keysym::grave => {
                                    // Scratchpad: 切换下拉终端
                                    data.toggle_scratchpad();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::space => {
                                    let ws = &mut data.workspaces[data.active_ws];
                                    ws.layout = ws.layout.next();
                                    let name = ws.layout.name();
                                    info!("🔄 布局切换 → {}", name);
                                    data.notify(format!("Layout: {}", name));
                                    data.do_layout();
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
                                // Super+方向键：交换窗口
                                Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => {
                                    data.swap_window(keysym.modified_sym());
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
                            // Super+Shift+1-9：移动窗口到工作区
                            if mods.shift {
                                match keysym.modified_sym() {
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
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::PointerMotion { event } => {
                self.pointer_pos.0 += event.delta_x();
                self.pointer_pos.1 += event.delta_y();

                // 跨显示器鼠标穿越
                let screen_w = self.osize.w as f64;
                let screen_h = self.osize.h as f64;
                if self.pointer_pos.0 < 0.0 {
                    if self.output_sizes.len() > 1 {
                        self.pointer_pos.0 = 0.0;
                    } else {
                        self.pointer_pos.0 = 0.0;
                    }
                }
                if self.pointer_pos.0 > screen_w {
                    if self.output_sizes.len() > 1 {
                        self.pointer_pos.0 = screen_w - 1.0;
                    } else {
                        self.pointer_pos.0 = screen_w - 1.0;
                    }
                }
                self.pointer_pos.1 = self.pointer_pos.1.clamp(0.0, screen_h - 1.0);

                // 转发给客户端
                let serial = SERIAL_COUNTER.next_serial();
                let time = (event.time() / 1000) as u32;
                let focus = self.pointer_focus();
                let ptr = self.pointer.clone();
                ptr.motion(self, focus, &MotionEvent {
                    location: Point::from((self.pointer_pos.0, self.pointer_pos.1)),
                    serial,
                    time,
                });
                ptr.frame(self);

                self.dirty = true;
            }
            InputEvent::PointerButton { event } => {
                // 点击聚焦（仅 Press 时）
                if event.state() == ButtonState::Pressed {
                    let px = self.pointer_pos.0 as i32;
                    let py = self.pointer_pos.1 as i32;
                    let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
                    if py >= bar_h {
                        let ws = &self.workspaces[self.active_ws];
                        for (i, tl) in ws.tops.iter().enumerate() {
                            let (x, y, w, h) = layout::slot(i, ws.tops.len(), self.osize.w, self.osize.h, bar_h, &self.cfg, ws.layout, ws.split);
                            if px >= x && px < x + w && py >= y && py < y + h {
                                let surf = tl.wl_surface().clone();
                                self.workspaces[self.active_ws].focus = Some(surf.clone());
                                let kbd = self.kbd.clone();
                                let serial = SERIAL_COUNTER.next_serial();
                                kbd.set_focus(self, Some(surf), serial);
                                break;
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
        if self.scratchpad_pending {
            self.scratchpad_pending = false;
            self.scratchpad_surface = Some(s.clone());
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
        
        self.workspaces[self.active_ws].tops.push(s);
        // 消费 pending_split：如果用户按了 Super+Shift+V/B，下一个窗口用新方向
        if let Some(new_split) = self.workspaces[self.active_ws].pending_split.take() {
            self.workspaces[self.active_ws].split = new_split;
            info!("📐 应用 pending split: {:?}", new_split);
        }
        let ws = &self.workspaces[self.active_ws];
        let idx = ws.tops.len() - 1;
        info!("➕ 窗口 #{} (工作区 {})", idx, self.active_ws + 1);
        drop(ws);
        self.do_layout();
        if let Some(tl) = self.workspaces[self.active_ws].tops.get(idx) {
            let surf = tl.wl_surface().clone();
            self.workspaces[self.active_ws].focus = Some(surf.clone());
            let kbd = self.kbd.clone();
            let serial = SERIAL_COUNTER.next_serial();
            kbd.set_focus(self, Some(surf), serial);
        }
    }
    fn new_popup(&mut self, popup: PopupSurface, _positioner: PositionerState) {
        let _ = popup.send_configure();
    }
    fn grab(&mut self, popup: PopupSurface, _seat: wl_seat::WlSeat, _serial: smithay::utils::Serial) {
        let _ = popup.send_configure();
    }
    fn reposition_request(&mut self, popup: PopupSurface, _positioner: PositionerState, _token: u32) {
        let _ = popup.send_configure();
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
                            self.workspaces[target_ws].tops.push(top);
                            self.switch_workspace(target_ws);
                            self.do_layout();
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

impl SelectionHandler for App { type SelectionUserData = (); }
impl DataDeviceHandler for App { fn data_device_state(&self) -> &DataDeviceState { &self.dd } }
impl smithay::wayland::output::OutputHandler for App {}
impl ClientDndGrabHandler for App {}
impl ServerDndGrabHandler for App { fn send(&mut self, _: String, _: OwnedFd, _: Seat<Self>) {} }

impl InputMethodHandler for App {
    fn new_popup(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: new");
        let _ = surface;
    }
    fn dismiss_popup(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: dismiss");
        let _ = surface;
    }
    fn popup_repositioned(&mut self, surface: ImPopupSurface) {
        info!("🔤 IM popup: repositioned");
        let _ = surface;
    }
    fn parent_geometry(&self, _parent: &WlSurface) -> Rectangle<i32, Logical> { Rectangle::default() }
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState { &mut self.comp }
    fn client_compositor_state<'a>(&self, c: &'a Client) -> &'a CompositorClientState { &c.get_data::<ClientState>().unwrap().comp }
    fn commit(&mut self, s: &WlSurface) {
        self.dirty = true;
        on_commit_buffer_handler::<Self>(s);
    }
    fn destroyed(&mut self, surface: &WlSurface) {
        // 搜索所有工作区找到被销毁的窗口
        for ws_idx in 0..self.workspaces.len() {
            let before = self.workspaces[ws_idx].tops.len();
            let closed_idx = self.workspaces[ws_idx].tops.iter().position(|tl| tl.wl_surface() == surface);
            self.workspaces[ws_idx].tops.retain(|tl| tl.wl_surface() != surface);
            if self.workspaces[ws_idx].tops.len() < before {
                info!("🗑️ 窗口关闭 (工作区 {})", ws_idx + 1);
                if let Some(ci) = closed_idx {
                    if let Some(fs) = self.workspaces[ws_idx].fullscreen {
                        if fs == ci { self.workspaces[ws_idx].fullscreen = None; }
                        else if fs > ci { self.workspaces[ws_idx].fullscreen = Some(fs - 1); }
                    }
                }
                if self.workspaces[ws_idx].focus.as_ref() == Some(surface) {
                    self.workspaces[ws_idx].focus = self.workspaces[ws_idx].tops.last()
                        .map(|t| t.wl_surface().clone());
                }
                if ws_idx == self.active_ws {
                    self.do_layout();
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
    }
}

impl ShmHandler for App { fn shm_state(&self) -> &ShmState { &self.shm } }
impl SeatHandler for App {
    type KeyboardFocus = WlSurface; type PointerFocus = WlSurface; type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> { &mut self.seat_state }
    fn focus_changed(&mut self, _: &Seat<Self>, surface: Option<&WlSurface>) {
        if surface.is_some() { info!("⌨️  键盘焦点改变"); }
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
    info!("🚀 Anchor v9 ({})", if direct { "direct" } else { "session" });

    // ─── GPU 设备选择 ───
    // 优先级: TITAN_GPU 环境变量 > config.toml [gpu].device > 自动检测
    let gpu_path = if let Ok(p) = std::env::var("TITAN_GPU") {
        std::path::PathBuf::from(p)
    } else if !cfg.gpu.device.is_empty() {
        std::path::PathBuf::from(&cfg.gpu.device)
    } else {
        // 自动检测：优先按 config 中的 vendor 找，找不到就用第一个 card
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
                            _ => true, // auto: 第一个就是最好的
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
    
    // 检测 GPU vendor 名称
    let gpu_vendor = {
        let card_name = gpu_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let vendor_str = std::fs::read_to_string(format!("/sys/class/drm/{}/device/vendor", card_name))
            .unwrap_or_default()
            .trim()
            .to_string();
        match vendor_str.as_str() {
            "0x10de" => "NVIDIA",
            "0x1002" => "AMD",
            "0x8086" => "Intel",
            _ => "Unknown",
        }.to_string()
    };
    info!("🔍 GPU vendor: {}", gpu_vendor);
    // 保存 DRM 设备路径到环境变量（截图等子进程使用）
    if let Some(card_name) = gpu_path.file_name() {
        std::env::set_var("TITAN_DRM_DEV", format!("/dev/dri/{}", card_name.to_string_lossy()));
    }

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

    let (mut device, dn) = DrmDevice::new(dev_fd.clone(), false)?;
    info!("✅ DrmDevice");
    let dev_fd_copy = dev_fd.clone();
    let gbm = GbmDevice::new(dev_fd)?;

    // 创建 GPU 渲染器 (GLES via EGL)
    // EGLDisplay takes ownership of the GbmDevice, so we need to create a second GBM device
    // for buffer allocation (GbmAllocator)
    let gbm_alloc = GbmDevice::new(dev_fd_copy.clone())?;
    let egl_display = unsafe { EGLDisplay::new(gbm)? };
    let egl_context = EGLContext::new(&egl_display)?;
    let mut renderer = unsafe { GlesRenderer::new(egl_context)? };
    info!("✅ GlesRenderer (GPU)");

    let res = device.resource_handles()?;
    // 广播支持的 dmabuf 格式（GPU 渲染器可以处理）
    let dmabuf_formats: Vec<Format> = [Fourcc::Argb8888, Fourcc::Xrgb8888].iter()
        .flat_map(|&c| [Format{code:c,modifier:Modifier::Linear}, Format{code:c,modifier:Modifier::Invalid}]).collect();

    // ── 多显示器枚举 ──
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

    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();
    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "seat0");
    let kbd = seat.add_keyboard(XkbConfig::default(), 200, 25)?;
    let pointer = seat.add_pointer();

    let _output_manager = OutputManagerState::new();
    info!("✅ wl_output");

    let fd_clones: Vec<_> = (0..connector_infos.len())
        .map(|_| dev_fd_copy.clone())
        .collect();

    let mut output_sizes: Vec<(i32, i32, i32, i32)> = Vec::new();

    for (idx, ci) in connector_infos.iter().enumerate() {
        let (mw, mh) = ci.mode.size();

        let surface = match device.create_surface(ci.crtc, ci.mode, &[ci.connector]) {
            Ok(s) => s,
            Err(e) => { warn!("⚠️  Surface 创建失败 {}: {:?}", ci.name, e); continue; }
        };

        let gbm_dup = GbmDevice::new(fd_clones[idx].clone())?;
        let alloc = GbmAllocator::new(gbm_dup, GbmBufferFlags::SCANOUT);
        let buf_surf = match GbmBufferedSurface::new(surface, alloc,
            &[Fourcc::Argb8888, Fourcc::Xrgb8888], dmabuf_formats.clone().into_iter()) {
            Ok(bs) => bs,
            Err(e) => { warn!("⚠️  BufferSurface 创建失败 {}: {:?}", ci.name, e); continue; }
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

        output_sizes.push((output_x_offset, 0, mw as i32, mh as i32));

        anchor_outputs.push(AnchorOutput {
            output: wl_output,
            size: Size::new(mw as i32, mh as i32),
            crtc: ci.crtc,
            connector: ci.connector,
            buf_surf,
            pending_flip: false,
            position: (output_x_offset, 0),
        });
        output_x_offset += mw as i32;
    }
    if anchor_outputs.is_empty() { return Err("所有输出创建失败".into()); }
    let primary_size = anchor_outputs[0].size;
    info!("✅ {} 个输出已就绪", anchor_outputs.len());

    InputMethodManagerState::new::<App, _>(&dh, |_client| true);
    TextInputManagerState::new::<App>(&dh);
    VirtualKeyboardManagerState::new::<App, _>(&dh, |_client| true);
    info!("✅ text-input / input-method / virtual-keyboard");

    info!("✅ dmabuf (GPU renderer, formats: {})", dmabuf_formats.len());

    // 加载光标
    let cursor_img = if !cfg.cursor.theme.is_empty() {
        cursor::CursorImage::load_from_theme(&cfg.cursor.theme, &cfg.cursor.name, cfg.cursor.size)
            .unwrap_or_else(|| {
                info!("⚠️  光标主题 '{}' 加载失败，使用内置光标", cfg.cursor.theme);
                cursor::CursorImage::builtin(cfg.cursor.size)
            })
    } else {
        // 尝试读取系统默认主题
        let default_theme = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        cursor::CursorImage::load_from_theme(&default_theme, &cfg.cursor.name, cfg.cursor.size)
            .unwrap_or_else(|| cursor::CursorImage::builtin(cfg.cursor.size))
    };

    let mut state = App {
        comp: CompositorState::new::<App>(&dh), xdg: XdgShellState::new::<App>(&dh),
        shm: ShmState::new::<App>(&dh, vec![]), seat_state, seat,
        dd: DataDeviceState::new::<App>(&dh),
        osize: primary_size,
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
        notifications: Vec::new(),
        scratchpad: None,
        scratchpad_visible: false,
        scratchpad_surface: None,
        scratchpad_pending: false,
        launcher_visible: false,
        launcher_query: String::new(),
        launcher_apps: Vec::new(),
        launcher_selected: 0,
        ws_anim: WsAnimation { start: None, from_ws: 0, to_ws: 0, duration_ms: 200, direction: 0 },
        output_sizes,
    };
    let listener = ListeningSocket::bind("wayland-anchor")?;
    std::env::set_var("WAYLAND_DISPLAY", "wayland-anchor");
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }));
    }
    info!("✅ wayland-anchor");

    let mut eloop: EventLoop<App> = EventLoop::try_new()?;
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

    info!("🔄 渲染中...");

    while state.run {
        if state.active != dev_active {
            if state.active {
                device.activate(true)?;
                for out in &mut anchor_outputs { out.buf_surf.reset_buffers(); out.pending_flip = false; }
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

        for out in &mut anchor_outputs {
            if state.vblank_crtcs.remove(&out.crtc) {
                if let Err(e) = out.buf_surf.frame_submitted() { warn!("VBlank err: {:?}", e); }
                out.pending_flip = false;
            }
        }

        if state.dirty {
            // 只清理当前工作区的死亡窗口
            state.workspaces[state.active_ws].tops.retain(|tl| tl.alive());
            let bar_h = if state.cfg.bar.enabled { state.cfg.bar.height } else { 0 };
            let focus_idx = state.focus_idx();
            let time_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
            let window_title = state.window_titles.get(&focus_idx.unwrap_or(0))
                .cloned().unwrap_or_default();
            let primary_crtc = anchor_outputs.first().map(|o| o.crtc);
            let ws = &state.workspaces[state.active_ws];
            let n_windows = ws.tops.len();
            let fullscreen = ws.fullscreen;
            
            // 计算工作区切换动画偏移（稍后在循环中使用 ow 计算）
            let ws_anim_active = state.ws_anim.start.is_some();
            let ws_anim_dir = state.ws_anim.direction;
            let ws_anim_duration = state.ws_anim.duration_ms;
            let ws_anim_elapsed = state.ws_anim.start.map(|s| s.elapsed().as_millis() as u64);

            for oi in 0..anchor_outputs.len() {
                let out = &mut anchor_outputs[oi];
                if out.pending_flip { continue; }

                match out.buf_surf.next_buffer() {
                    Ok((mut dmabuf, _)) => {
                        let ow = out.size.w;
                        let oh = out.size.h;

                        // 壁纸 mmap 写入
                        if Some(out.crtc) == primary_crtc {
                            let fb_size = (ow * oh * 4) as usize;
                            use smithay::backend::allocator::dmabuf::{DmabufMappingMode};
                            if let Ok(mapping) = dmabuf.map_plane(0, DmabufMappingMode::WRITE) {
                                let ptr = mapping.ptr();
                                if !ptr.is_null() {
                                    let buf = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, fb_size) };
                                    if let Some(ref wp) = state.wallpaper_cache.pixels {
                                        let (ww, wh) = state.wallpaper_cache.size;
                                        if ww == ow as usize && wh == oh as usize && wp.len() == fb_size {
                                            for i in (0..fb_size).step_by(4) {
                                                buf[i] = wp[i + 2];
                                                buf[i + 1] = wp[i + 1];
                                                buf[i + 2] = wp[i];
                                                buf[i + 3] = 0xFF;
                                            }
                                        }
                                    }
                                }
                                drop(mapping);
                            }
                        }

                        // ═══════════════════════════════════════════════
                        // Phase 1: 收集渲染元素（bind 之前，需要 &mut renderer）
                        // ═══════════════════════════════════════════════
                        let mut all_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
                        let mut ws_offset: i32 = 0;
                        
                        if Some(out.crtc) == primary_crtc {
                            if let Some(fi) = fullscreen {
                                // 全屏：只渲染聚焦窗口
                                if let Some(tl) = ws.tops.get(fi) {
                                    for e in render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), (0, bar_h), 1.0, 1.0, Kind::Unspecified) {
                                        all_elems.push(e);
                                    }
                                }
                            } else {
                                // 工作区切换动画偏移
                                ws_offset = if ws_anim_active {
                                    if let Some(elapsed) = ws_anim_elapsed {
                                        if elapsed < ws_anim_duration {
                                            let t = elapsed as f32 / ws_anim_duration as f32;
                                            let t_ease = 1.0 - (1.0 - t).powi(3);
                                            (ws_anim_dir as f32 * ow as f32 * (1.0 - t_ease)) as i32
                                        } else { 0 }
                                    } else { 0 }
                                } else { 0 };
                                
                                // 正序收集所有窗口元素到同一个 vec
                                // 后收集的窗口渲染时覆盖先收集的溢出
                                for (i, tl) in ws.tops.iter().enumerate() {
                                    let (x, y, _w, _h) = layout::slot(i, n_windows, ow, oh, bar_h, &state.cfg, state.workspaces[state.active_ws].layout, state.workspaces[state.active_ws].split);
                                    for e in render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), (x + ws_offset, y), 1.0, 1.0, Kind::Unspecified) {
                                        all_elems.push(e);
                                    }
                                }
                            }
                        }

                        // Scratchpad 元素收集（bind 之前）
                        let sp_data = if Some(out.crtc) == primary_crtc {
                            if let Some(ref sp_surf) = state.scratchpad_surface {
                                if sp_surf.alive() && state.scratchpad_visible {
                                    let sp_w = ow * 3 / 4;
                                    let sp_h = oh / 3;
                                    let sp_x = (ow - sp_w) / 2;
                                    let sp_y = bar_h + 8;
                                    let sp_elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
                                        &mut renderer, sp_surf.wl_surface(), (sp_x, sp_y), 1.0, 1.0, Kind::Unspecified
                                    );
                                    Some((sp_elems, sp_x, sp_y, sp_w, sp_h))
                                } else { None }
                            } else { None }
                        } else { None };

                        // ═══════════════════════════════════════════════
                        // Phase 2: bind + render（一次性绘制）
                        // ═══════════════════════════════════════════════
                        let mut target = renderer.bind(&mut dmabuf)?;
                        let sp_size = Size::<i32, Physical>::new(ow, oh);
                        let mut f = renderer.render(&mut target, sp_size, Transform::Normal)?;
                        let dmg = Rectangle::from_size(sp_size);

                        // Step 1: 壁纸（最底层）
                        if state.wallpaper_cache.pixels.is_none() {
                            layout::render_wallpaper(&mut f, &state.cfg, ow, oh, state.frame);
                        }

                        // Step 2: 所有窗口内容（一次性绘制）
                        // draw_render_elements 按元素在 vec 中的顺序绘制
                        // 因为正序收集，后面的窗口元素自然覆盖前面的溢出
                        draw_render_elements(&mut f, 1.0, &all_elems, &[dmg])?;

                        // Step 3: 装饰边框（在窗口内容之上）
                        if Some(out.crtc) == primary_crtc && fullscreen.is_none() {
                            for (i, _) in ws.tops.iter().enumerate() {
                                layout::render_window_decorations_anim(
                                    &mut f, &state.cfg, i, n_windows, focus_idx,
                                    ow, oh, bar_h, state.workspaces[state.active_ws].layout, state.workspaces[state.active_ws].split, ws_offset
                                );
                            }
                        }

                        // Step 4: Scratchpad（在装饰之上，不透明背景完全覆盖）
                        if let Some((ref sp_elems, sp_x, sp_y, sp_w, sp_h)) = sp_data {
                            let accent = crate::config::parse_color(&state.cfg.colors.focus_border);
                            let bw = 4;
                            // 不透明背景
                            let sp_bg = layout::opaque(0.06, 0.06, 0.10);
                            f.clear(sp_bg, &[layout::rect(sp_x - bw, sp_y - bw, sp_w + 2 * bw, sp_h + 2 * bw)]).ok();
                            // 边框
                            let border = layout::opaque(accent.0, accent.1, accent.2);
                            f.clear(border, &[layout::rect(sp_x - bw, sp_y - bw, sp_w + 2 * bw, bw)]).ok();
                            f.clear(border, &[layout::rect(sp_x - bw, sp_y + sp_h, sp_w + 2 * bw, bw)]).ok();
                            f.clear(border, &[layout::rect(sp_x - bw, sp_y, bw, sp_h)]).ok();
                            f.clear(border, &[layout::rect(sp_x + sp_w, sp_y, bw, sp_h)]).ok();
                            // 终端内容
                            draw_render_elements(&mut f, 1.0, sp_elems, &[dmg])?;
                            crate::text_render::draw_text(&mut f, "SCRATCHPAD", sp_x + 6, sp_y - 22, 14.0, (accent.0, accent.1, accent.2));
                        }

                        // Step 5: Headbar
                        let ws_counts: Vec<usize> = state.workspaces.iter().map(|w| w.tops.len()).collect();
                        layout::render_headbar(&mut f, &state.cfg, ow, oh, n_windows, focus_idx, time_secs, &window_title, state.active_ws, NUM_WORKSPACES, &ws_counts);

                        // Step 6: 通知弹窗
                        if Some(out.crtc) == primary_crtc && !state.notifications.is_empty() {
                            let accent = crate::config::parse_color(&state.cfg.colors.focus_border);
                            let notif_data: Vec<(String, std::time::Instant, std::time::Duration)> = state.notifications.iter()
                                .map(|n| (n.text.clone(), n.created, n.duration)).collect();
                            layout::render_notifications(&mut f, &notif_data, ow, state.cfg.bar.height, accent);
                        }

                        // Step 7: 内置启动器
                        if Some(out.crtc) == primary_crtc && state.launcher_visible {
                            let filtered = state.launcher_filter();
                            layout::render_launcher(&mut f, &state.cfg, ow, oh, &state.launcher_query, &filtered, state.launcher_selected);
                        }

                        // Step 8: 光标
                        if Some(out.crtc) == primary_crtc {
                            let cx = state.pointer_pos.0 as i32 - state.cursor_img.hotspot_x as i32;
                            let cy = state.pointer_pos.1 as i32 - state.cursor_img.hotspot_y as i32;
                            state.cursor_img.render(&mut f, cx, cy);
                        }

                        let _ = f.finish()?;
                        drop(target);

                        out.buf_surf.queue_buffer(None, None, ())?;
                        out.pending_flip = true;
                    }
                    Err(e) => { if state.frame == 0 { error!("❌ {e:?}"); } }
                }
            }
            state.dirty = false;
            // 发送 frame callback（仅在渲染后发送）
            let now = start.elapsed().as_millis() as u32;
            for s in state.xdg.toplevel_surfaces() { send_frames(s.wl_surface(), now); }
            // 动画进行中时持续请求渲染
            if state.ws_anim.start.map(|s| (s.elapsed().as_millis() as u64) < state.ws_anim.duration_ms).unwrap_or(false) {
                state.dirty = true;
            }
            state.frame += 1;
            state.drain_notifications();
            if state.frame == 1 { info!("✅ 第一帧渲染！"); }
            if state.frame % 600 == 0 { info!("📊 {} 帧", state.frame); }
        }

        eloop.dispatch(Some(Duration::from_millis(16)), &mut state)?;
        // 时钟每秒更新（bar enabled 时）
        if state.frame % 60 == 0 && state.cfg.bar.enabled { state.dirty = true; }
        // CPU/MEM 状态每 5 秒更新
        if state.frame % 300 == 0 { state.dirty = true; }

        if let Ok(Some(stream)) = listener.accept() {
            clients.push(display.handle().insert_client(stream, Arc::new(ClientState::default()))?);
        }
        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }

    info!("👋"); Ok(())
}

delegate_xdg_shell!(App);
delegate_compositor!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_data_device!(App);
delegate_output!(App);
delegate_input_method_manager!(App);
delegate_text_input_manager!(App);
delegate_virtual_keyboard_manager!(App);
