// Titan — Wayland tiling compositor v9
// Features: multi-workspace, multi-monitor, wallpaper, config
// Config: ~/.config/titan/config.toml

mod config;
mod layout;
mod font;
mod block_linear;
mod wallpaper;

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
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice}},
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, GbmBufferedSurface},
        input::{InputEvent, KeyState},
        libinput::LibinputInputBackend,
        renderer::{Bind, Frame, Renderer,
            pixman::PixmanRenderer,
            element::{surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement}, Kind},
            utils::{draw_render_elements, on_commit_buffer_handler}, Color32F},
        session::{Session, Event as SessionEvent, libseat::{LibSeatSession, LibSeatSessionNotifier}},
    },
    delegate_compositor, delegate_data_device, delegate_input_method_manager,
    delegate_output, delegate_seat, delegate_shm, delegate_text_input_manager, delegate_xdg_shell,
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState, XkbConfig},
        pointer::CursorImageStatus, Seat, SeatHandler, SeatState,
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
}

impl Workspace {
    fn new() -> Self {
        Self { tops: Vec::new(), focus: None, fullscreen: None }
    }
}

// ── TitanOutput ────────────────────────────────────────────

struct TitanOutput {
    output: Output,
    size: Size<i32, Logical>,
    crtc: crtc::Handle,
    connector: connector::Handle,
    buf_surf: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, ()>,
    pending_flip: bool,
    position: (i32, i32),
}

// ── App ──────────────────────────────────────────────────

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
    pointer_pos: (f64, f64),
    cfg: Config,
    window_titles: std::collections::HashMap<usize, String>,
    vblank_crtcs: std::collections::HashSet<crtc::Handle>,
    wallpaper_cache: wallpaper::WallpaperCache,
    // 多显示器尺寸信息（用于鼠标穿越）
    output_sizes: Vec<(i32, i32, i32, i32)>, // (x, y, w, h) per output
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
                let (_x, _y, w, h) = layout::slot(i, n, osize_w, osize_h, bar_h, &self.cfg);
                tl.with_pending_state(|st| {
                    st.states.set(xdg_toplevel::State::Activated);
                    st.states.unset(xdg_toplevel::State::Fullscreen);
                    st.states.set(xdg_toplevel::State::TiledLeft);
                    st.states.set(xdg_toplevel::State::TiledRight);
                    st.states.set(xdg_toplevel::State::TiledTop);
                    st.states.set(xdg_toplevel::State::TiledBottom);
                    st.size = Some((w, h).into());
                });
                tl.send_configure();
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

        // 隐藏当前工作区的窗口（最小化到 1x1）
        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        for tl in &self.workspaces[self.active_ws].tops {
            tl.with_pending_state(|st| {
                st.states.unset(xdg_toplevel::State::Activated);
                st.states.unset(xdg_toplevel::State::Fullscreen);
                st.size = Some((1, 1).into());
            });
            tl.send_configure();
        }

        self.active_ws = target;

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
                        if state == KeyState::Pressed && mods.logo {
                            let uid = unsafe { libc::getuid() };
                            match keysym.modified_sym() {
                                Keysym::Return => {
                                    info!("⌨️  启动终端");
                                    std::process::Command::new(&data.cfg.terminal.command)
                                        .env("WAYLAND_DISPLAY", "wayland-titan")
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
                                    std::process::Command::new("./scripts/titan-launcher")
                                        .env("WAYLAND_DISPLAY", "wayland-titan")
                                        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
                                        .spawn().ok();
                                    return FilterResult::Intercept(());
                                }
                                Keysym::f => { data.toggle_fullscreen(); return FilterResult::Intercept(()); }
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
                    // 跑到左边屏幕
                    if self.output_sizes.len() > 1 {
                        self.pointer_pos.0 = 0.0; // TODO: warp to left screen
                    } else {
                        self.pointer_pos.0 = 0.0;
                    }
                }
                if self.pointer_pos.0 > screen_w {
                    // 跑到右边屏幕
                    if self.output_sizes.len() > 1 {
                        self.pointer_pos.0 = screen_w - 1.0; // TODO: warp to right screen
                    } else {
                        self.pointer_pos.0 = screen_w - 1.0;
                    }
                }
                self.pointer_pos.1 = self.pointer_pos.1.clamp(0.0, screen_h - 1.0);
                self.dirty = true;
            }
            InputEvent::PointerButton { event } => {
                use smithay::backend::input::ButtonState;
                if event.state() == ButtonState::Pressed {
                    let px = self.pointer_pos.0 as i32;
                    let py = self.pointer_pos.1 as i32;
                    let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
                    if py < bar_h { return; }
                    let ws = &self.workspaces[self.active_ws];
                    for (i, tl) in ws.tops.iter().enumerate() {
                        let (x, y, w, h) = layout::slot(i, ws.tops.len(), self.osize.w, self.osize.h, bar_h, &self.cfg);
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
            _ => {}
        }
    }
}

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg }
    fn new_toplevel(&mut self, s: ToplevelSurface) {
        // 新窗口总是添加到当前工作区
        self.workspaces[self.active_ws].tops.push(s);
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
    fn new_popup(&mut self, _: PopupSurface, _: PositionerState) {}
    fn grab(&mut self, _: PopupSurface, _: wl_seat::WlSeat, _: smithay::utils::Serial) {}
    fn reposition_request(&mut self, _: PopupSurface, _: PositionerState, _: u32) {}
}

impl SelectionHandler for App { type SelectionUserData = (); }
impl DataDeviceHandler for App { fn data_device_state(&self) -> &DataDeviceState { &self.dd } }
impl smithay::wayland::output::OutputHandler for App {}
impl ClientDndGrabHandler for App {}
impl ServerDndGrabHandler for App { fn send(&mut self, _: String, _: OwnedFd, _: Seat<Self>) {} }

impl InputMethodHandler for App {
    fn new_popup(&mut self, _surface: ImPopupSurface) {}
    fn dismiss_popup(&mut self, _surface: ImPopupSurface) {}
    fn popup_repositioned(&mut self, _surface: ImPopupSurface) {}
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
    fn focus_changed(&mut self, _: &Seat<Self>, _: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {}
}

#[derive(Default)] struct ClientState { comp: CompositorClientState }
impl ClientData for ClientState { fn initialized(&self, _: ClientId) {} fn disconnected(&self, _: ClientId, _: DisconnectReason) {} }

fn send_frames(s: &WlSurface, t: u32) {
    with_surface_tree_downward(s, (), |_,_,&()| TraversalAction::DoChildren(()),
        |_,st,&()| { for cb in st.cached_state.get::<SurfaceAttributes>().current().frame_callbacks.drain(..) { cb.done(t); } },
        |_,_,&()| true);
}

// ── main ─────────────────────────────────────────────────

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
    info!("🚀 Titan v9 ({})", if direct { "direct" } else { "session" });

    let gpu_path = std::env::var("TITAN_GPU").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        let mut nvidia = None;
        for e in std::fs::read_dir("/dev/dri").unwrap() {
            let e = e.unwrap(); let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("card") {
                if let Ok(v) = std::fs::read_to_string(format!("/sys/class/drm/{}/device/vendor", name)) {
                    if v.trim() == "0x10de" { nvidia = Some(e.path()); }
                }
            }
        }
        nvidia.unwrap()
    });
    info!("🎮 {}", gpu_path.display());

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
    let _gbm = GbmDevice::new(dev_fd)?;
    let mut renderer = PixmanRenderer::new()?;
    info!("✅ Pixman");

    let res = device.resource_handles()?;
    let fmts: Vec<Format> = [Fourcc::Argb8888, Fourcc::Xrgb8888].iter()
        .flat_map(|&c| [Format{code:c,modifier:Modifier::Linear}, Format{code:c,modifier:Modifier::Invalid}]).collect();

    // ── 多显示器枚举 ──
    let mut titan_outputs: Vec<TitanOutput> = Vec::new();
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
            &[Fourcc::Argb8888, Fourcc::Xrgb8888], fmts.clone().into_iter()) {
            Ok(bs) => bs,
            Err(e) => { warn!("⚠️  BufferSurface 创建失败 {}: {:?}", ci.name, e); continue; }
        };

        let wl_output = Output::new(ci.name.clone(), PhysicalProperties {
            size: (mw as i32 / 10, mh as i32 / 10).into(),
            subpixel: Subpixel::Unknown, make: "NVIDIA".into(), model: ci.name.clone(),
        });
        let output_mode = Mode { size: (mw as i32, mh as i32).into(), refresh: 59000 };
        wl_output.add_mode(output_mode);
        wl_output.set_preferred(output_mode);
        wl_output.change_current_state(
            Some(output_mode), Some(Transform::Normal),
            Some(Scale::Integer(1)), Some(Point::from((output_x_offset, 0)))
        );
        wl_output.create_global::<App>(&dh);

        output_sizes.push((output_x_offset, 0, mw as i32, mh as i32));

        titan_outputs.push(TitanOutput {
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
    if titan_outputs.is_empty() { return Err("所有输出创建失败".into()); }
    let primary_size = titan_outputs[0].size;
    info!("✅ {} 个输出已就绪", titan_outputs.len());

    InputMethodManagerState::new::<App, _>(&dh, |_client| true);
    TextInputManagerState::new::<App>(&dh);
    info!("✅ text-input / input-method");

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
        kbd, cfg,
        pointer_pos: (0.0, 0.0),
        window_titles: std::collections::HashMap::new(),
        vblank_crtcs: std::collections::HashSet::new(),
        wallpaper_cache: wallpaper::WallpaperCache::new(),
        output_sizes,
    };
    let listener = ListeningSocket::bind("wayland-titan")?;
    std::env::set_var("WAYLAND_DISPLAY", "wayland-titan");
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }));
    }
    info!("✅ wayland-titan");

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
        .env("WAYLAND_DISPLAY", "wayland-titan")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }))
        .env("XMODIFIERS", "@im=fcitx").env("QT_IM_MODULE", "fcitx").env("GTK_IM_MODULE", "fcitx")
        .spawn().ok();

    info!("🔄 渲染中...");

    while state.run {
        if state.active != dev_active {
            if state.active {
                device.activate(true)?;
                for out in &mut titan_outputs { out.buf_surf.reset_buffers(); out.pending_flip = false; }
            } else {
                device.pause();
                for out in &mut titan_outputs { out.pending_flip = false; }
            }
            dev_active = state.active;
        }
        if !dev_active {
            eloop.dispatch(Some(Duration::from_millis(100)), &mut state)?;
            display.dispatch_clients(&mut state)?;
            display.flush_clients()?;
            continue;
        }

        for out in &mut titan_outputs {
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
            let time_secs = start.elapsed().as_secs();
            let window_title = state.window_titles.get(&focus_idx.unwrap_or(0))
                .cloned().unwrap_or_default();
            let primary_crtc = titan_outputs.first().map(|o| o.crtc);
            let ws = &state.workspaces[state.active_ws];
            let n_windows = ws.tops.len();
            let fullscreen = ws.fullscreen;

            for oi in 0..titan_outputs.len() {
                let out = &mut titan_outputs[oi];
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

                        let mut elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();

                        if Some(out.crtc) == primary_crtc {
                            if let Some(fi) = fullscreen {
                                if let Some(tl) = ws.tops.get(fi) {
                                    for elem in render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), (0, bar_h), 1.0, 1.0, Kind::Unspecified) {
                                        elems.push(elem);
                                    }
                                }
                            } else {
                                for (i, tl) in ws.tops.iter().enumerate() {
                                    let (x, y, _w, _h) = layout::slot(i, n_windows, ow, oh, bar_h, &state.cfg);
                                    for elem in render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), (x, y), 1.0, 1.0, Kind::Unspecified) {
                                        elems.push(elem);
                                    }
                                }
                            }
                        }

                        let mut target = renderer.bind(&mut dmabuf)?;
                        let sp = Size::<i32, Physical>::new(ow, oh);
                        let mut f = renderer.render(&mut target, sp, Transform::Normal)?;
                        let dmg = Rectangle::from_size(sp);

                        if state.wallpaper_cache.pixels.is_none() {
                            layout::render_wallpaper(&mut f, &state.cfg, ow, oh, state.frame);
                        }
                        // 窗口暗色背景（在壁纸之上、窗口内容之下）
                        if Some(out.crtc) == primary_crtc && fullscreen.is_none() {
                            layout::render_window_bg(&mut f, &state.cfg, n_windows, ow, oh, bar_h);
                        }
                        draw_render_elements(&mut f, 1.0, &elems, &[dmg])?;

                        if Some(out.crtc) == primary_crtc && fullscreen.is_none() {
                            for (i, _) in ws.tops.iter().enumerate() {
                                layout::render_window_decorations(&mut f, &state.cfg, i, n_windows, focus_idx, ow, oh, bar_h);
                            }
                        }

                        let ws_counts: Vec<usize> = state.workspaces.iter().map(|w| w.tops.len()).collect();
                        layout::render_headbar(&mut f, &state.cfg, ow, oh, n_windows, focus_idx, time_secs, &window_title, state.active_ws, NUM_WORKSPACES, &ws_counts);

                        // 光标
                        if Some(out.crtc) == primary_crtc {
                            let cx = state.pointer_pos.0 as i32;
                            let cy = state.pointer_pos.1 as i32;
                            let cc = Color32F::new(1.0, 1.0, 1.0, 0.9);
                            let _ = f.clear(cc, &[Rectangle::new(Point::new(cx, cy), Size::new(2, 18))]);
                            let _ = f.clear(cc, &[Rectangle::new(Point::new(cx + 1, cy + 2), Size::new(1, 1))]);
                            let _ = f.clear(cc, &[Rectangle::new(Point::new(cx + 2, cy + 4), Size::new(1, 1))]);
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
            state.frame += 1;
            if state.frame == 1 { info!("✅ 第一帧渲染！"); }
            if state.frame % 600 == 0 { info!("📊 {} 帧", state.frame); }
        }

        eloop.dispatch(Some(Duration::from_millis(16)), &mut state)?;
        if state.frame % 60 == 0 && state.cfg.bar.enabled { state.dirty = true; }

        if let Ok(Some(stream)) = listener.accept() {
            clients.push(display.handle().insert_client(stream, Arc::new(ClientState::default()))?);
        }
        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
        let now = start.elapsed().as_millis() as u32;
        for s in state.xdg.toplevel_surfaces() { send_frames(s.wl_surface(), now); }
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
