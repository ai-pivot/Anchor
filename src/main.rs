// Titan — 极简平铺 Wayland 合成器 (DRM 后端, v7)
// GDM: titan-session | TTY: sudo ./titan --direct
// 退出: 从 SSH 运行 killall titan

use std::{
    os::unix::io::OwnedFd,
    os::fd::AsRawFd,
    sync::Arc,
    time::{Duration, Instant},
};

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
        drm::control::{connector, Device as _},
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

// ── App ──────────────────────────────────────────────────

struct App {
    comp: CompositorState, xdg: XdgShellState, shm: ShmState, seat_state: SeatState<Self>,
    dd: DataDeviceState, seat: Seat<Self>,
    output: Output,
    osize: Size<i32, Logical>, tops: Vec<ToplevelSurface>, run: bool, frame: u32,
    dh: DisplayHandle, active: bool, vblank: bool,
    dirty: bool,
    kbd: smithay::input::keyboard::KeyboardHandle<Self>,
    focus: Option<WlSurface>,
    pointer_pos: (f64, f64),
    fullscreen: Option<usize>,  // 全屏窗口的索引（None 表示平铺模式）
}

impl BufferHandler for App { fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {} }
impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg }
    fn new_toplevel(&mut self, s: ToplevelSurface) {
        self.tops.push(s);
        let idx = self.tops.len() - 1;
        info!("➕ 窗口 #{}", idx);
        // 重新计算所有窗口的布局并发 configure
        self.layout();
        // 焦点设为新窗口
        if let Some(tl) = self.tops.get(idx) {
            self.focus = Some(tl.wl_surface().clone());
            let kbd = self.kbd.clone();
            let serial = SERIAL_COUNTER.next_serial();
            kbd.set_focus(self, Some(tl.wl_surface().clone()), serial);
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
    fn parent_geometry(&self, _parent: &WlSurface) -> Rectangle<i32, Logical> {
        Rectangle::default()
    }
}
impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState { &mut self.comp }
    fn client_compositor_state<'a>(&self, c: &'a Client) -> &'a CompositorClientState { &c.get_data::<ClientState>().unwrap().comp }
    fn commit(&mut self, s: &WlSurface) {
        self.dirty = true;
        on_commit_buffer_handler::<Self>(s);
    }
    fn destroyed(&mut self, surface: &WlSurface) {
        let before = self.tops.len();
        let closed_idx = self.tops.iter().position(|tl| tl.wl_surface() == surface);
        self.tops.retain(|tl| tl.wl_surface() != surface);
        if self.tops.len() < before {
            info!("🗑️ 窗口关闭，剩余 {}", self.tops.len());
            // 更新 fullscreen 索引
            if let Some(fi) = self.fullscreen {
                if let Some(ci) = closed_idx {
                    if fi == ci {
                        self.fullscreen = None; // 全屏窗口被关闭
                    } else if fi > ci {
                        self.fullscreen = Some(fi - 1); // 索引前移
                    }
                }
            }
            if self.focus.as_ref() == Some(surface) {
                if let Some(tl) = self.tops.last() {
                    self.focus = Some(tl.wl_surface().clone());
                    let kbd = self.kbd.clone();
                    let serial = SERIAL_COUNTER.next_serial();
                    kbd.set_focus(self, Some(tl.wl_surface().clone()), serial);
                } else {
                    self.focus = None;
                }
            }
            self.layout();
            self.dirty = true;
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

/// 计算第 i 个窗口在 n 个窗口中的平铺位置和大小（纯函数，可独立测试）
fn slot(i: usize, n: usize, ow: i32, oh: i32) -> (i32, i32, i32, i32) {
    match n {
        0 | 1 => (0, 0, ow, oh),
        2 => {
            let half = ow / 2;
            (i as i32 * half, 0, half, oh)
        }
        _ => {
            let cols = 2i32;
            let rows = ((n + 1) / 2) as i32;
            let sw = ow / cols;
            let sh = oh / rows;
            let col = (i % 2) as i32;
            let row = (i / 2) as i32;
            (col * sw, row * sh, sw, sh)
        }
    }
}

impl App {
    /// 终端模拟器命令
    const TERMINAL: &'static str = "foot";

    /// 对所有窗口发送 configure，告诉客户端目标尺寸
    fn layout(&mut self) {
        let n = self.tops.len();
        if n == 0 { return; }

        // 检查 fullscreen 索引是否有效
        if let Some(fi) = self.fullscreen {
            if fi >= n { self.fullscreen = None; }
        }

        if let Some(fi) = self.fullscreen {
            // 全屏模式：全屏窗口占满屏幕，其他窗口最小化
            for (i, tl) in self.tops.iter().enumerate() {
                if i == fi {
                    tl.with_pending_state(|st| {
                        st.states.set(xdg_toplevel::State::Activated);
                        st.states.set(xdg_toplevel::State::Fullscreen);
                        st.size = Some((self.osize.w, self.osize.h).into());
                    });
                } else {
                    tl.with_pending_state(|st| {
                        st.states.unset(xdg_toplevel::State::Activated);
                        st.states.unset(xdg_toplevel::State::Fullscreen);
                        st.size = Some((1, 1).into()); // 最小化
                    });
                }
                tl.send_configure();
            }
        } else {
            // 平铺模式
            for (i, tl) in self.tops.iter().enumerate() {
                let (_x, _y, w, h) = slot(i, n, self.osize.w, self.osize.h);
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

    /// 切换焦点窗口的全屏状态
    fn toggle_fullscreen(&mut self) {
        let fi = self.focus_idx();
        match (fi, self.fullscreen) {
            (Some(idx), Some(fs)) if idx == fs => {
                // 当前全屏窗口，取消全屏
                info!("🔳 取消全屏窗口 #{}", idx);
                self.fullscreen = None;
            }
            (Some(idx), _) => {
                // 切换到全屏
                info!("🔳 全屏窗口 #{}", idx);
                self.fullscreen = Some(idx);
            }
            _ => return,
        }
        self.layout();
        self.dirty = true;
    }

    /// 获取焦点窗口的索引
    fn focus_idx(&self) -> Option<usize> {
        self.focus.as_ref().and_then(|surf| {
            self.tops.iter().position(|tl| tl.wl_surface() == surf)
        })
    }

    /// 清理已关闭的窗口（通过 wl_surface destroyed 回调处理，这里做兜底检查）
    fn cleanup_dead(&mut self) {
        let before = self.tops.len();
        self.tops.retain(|tl| tl.alive());
        if self.tops.len() < before {
            info!("🗑️ 清理死窗口，剩余 {}", self.tops.len());
            self.layout();
            self.dirty = true;
        }
    }

    /// 处理 libinput 输入事件
    fn handle_input_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        use smithay::backend::input::{KeyboardKeyEvent as _, PointerButtonEvent as _, PointerMotionEvent as _, Event as _};
        match event {
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let state = event.state();
                let time = (event.time() / 1000) as u32;
                let serial = SERIAL_COUNTER.next_serial();

                let kbd = self.kbd.clone();
                let _result: Option<()> = smithay::input::keyboard::KeyboardHandle::<Self>::input(
                    &kbd, self, keycode, state, serial, time,
                    |data: &mut App, mods: &ModifiersState, keysym: smithay::input::keyboard::KeysymHandle<'_>| {
                        // 仅在按下时处理快捷键
                        if state == KeyState::Pressed {
                            let super_pressed = mods.logo;
                            match keysym.modified_sym() {
                                Keysym::Return if super_pressed => {
                                    info!("⌨️  启动终端: {}", Self::TERMINAL);
                                    if let Err(e) = std::process::Command::new(Self::TERMINAL)
                                        .env("WAYLAND_DISPLAY", "wayland-titan")
                                        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }))
                                        .env("XMODIFIERS", "@im=fcitx")
                                        .env("QT_IM_MODULE", "fcitx")
                                        .env("GTK_IM_MODULE", "fcitx")
                                        .env("ELECTRON_OZONE_PLATFORM_HINT", "wayland")
                                        .spawn()
                                    {
                                        error!("❌ 启动终端失败: {}", e);
                                    }
                                    return FilterResult::Intercept(());
                                }
                                Keysym::Escape if super_pressed && mods.shift => {
                                    info!("⌨️  退出 (Win+Shift+Esc)");
                                    data.run = false;
                                    return FilterResult::Intercept(());
                                }
                                // Win+Q: 关闭焦点窗口
                                Keysym::q if super_pressed => {
                                    if let Some(ref surf) = data.focus.clone() {
                                        if let Some(tl) = data.tops.iter().find(|tl| tl.wl_surface() == surf) {
                                            info!("⌨️  关闭窗口 (Win+Q)");
                                            tl.send_close();
                                        }
                                    }
                                    return FilterResult::Intercept(());
                                }
                                // Win+D: 程序启动器
                                Keysym::d if super_pressed => {
                                    info!("⌨️  启动器 (Win+D)");
                                    std::process::Command::new("./scripts/titan-launcher")
                                        .env("WAYLAND_DISPLAY", "wayland-titan")
                                        .spawn()
                                        .ok();
                                    return FilterResult::Intercept(());
                                }
                                // Win+F: 切换全屏
                                Keysym::f if super_pressed => {
                                    info!("⌨️  切换全屏 (Win+F)");
                                    data.toggle_fullscreen();
                                    return FilterResult::Intercept(());
                                }
                                _ => {}
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
            // 鼠标移动：跟踪指针位置并标记 dirty 重绘光标
            InputEvent::PointerMotion { event } => {
                self.pointer_pos.0 += event.delta_x();
                self.pointer_pos.1 += event.delta_y();
                self.pointer_pos.0 = self.pointer_pos.0.clamp(0.0, self.osize.w as f64);
                self.pointer_pos.1 = self.pointer_pos.1.clamp(0.0, self.osize.h as f64);
                self.dirty = true;
            }
            // 鼠标点击：切换焦点到点击的窗口
            InputEvent::PointerButton { event } => {
                use smithay::backend::input::ButtonState;
                if event.state() == ButtonState::Pressed {
                    let px = self.pointer_pos.0 as i32;
                    let py = self.pointer_pos.1 as i32;
                    for (i, tl) in self.tops.iter().enumerate() {
                        let (x, y, w, h) = slot(i, self.tops.len(), self.osize.w, self.osize.h);
                        if px >= x && px < x + w && py >= y && py < y + h {
                            info!("🖱️ 点击窗口 #{}", i);
                            self.focus = Some(tl.wl_surface().clone());
                            let kbd = self.kbd.clone();
                            let serial = SERIAL_COUNTER.next_serial();
                            kbd.set_focus(self, Some(tl.wl_surface().clone()), serial);
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }
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
    // --screenshot <path>: 渲染 30 帧后保存截图并退出（自动化测试用）
    let screenshot_path = args.iter().position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1)).map(|s| s.clone());
    let test_frames: u32 = args.iter().position(|a| a == "--test-frames")
        .and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(0);
    info!("🚀 Titan v7 ({}){}", if direct { "direct" } else { "session" },
        screenshot_path.as_ref().map(|p| format!(" screenshot={}", p)).unwrap_or_default());

    // 找 NVIDIA GPU
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

    // Session
    // session 需要 Arc<Mutex> 共享给 libinput 的 open_restricted 回调，
    // 因为 /dev/input/* 只能通过 libseat/logind TakeDevice 获取权限。
    let (dev_fd, session, notifier): (DrmDeviceFd, Option<Arc<std::sync::Mutex<LibSeatSession>>>, Option<LibSeatSessionNotifier>) = if direct {
        let fd = Arc::new(std::fs::OpenOptions::new().read(true).write(true).open(&gpu_path)?);
        let ret = unsafe { libc::ioctl(fd.as_raw_fd(), 0x4000641eu64 as _) };
        if ret == 0 { info!("✅ DRM master"); } else { warn!("⚠️  {}", std::io::Error::last_os_error()); }
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

    // DrmDevice + GBM + Pixman
    let (mut device, dn) = DrmDevice::new(dev_fd.clone(), false)?;
    info!("✅ DrmDevice");
    let gbm = GbmDevice::new(dev_fd)?;
    let mut renderer = PixmanRenderer::new()?;
    info!("✅ Pixman");
    let alloc = GbmAllocator::new(gbm, GbmBufferFlags::SCANOUT);

    // Connector
    let res = device.resource_handles()?;
    let mut conn_h = None; let mut mode = None;
    for &c in res.connectors() {
        for f in [false, true] {
            if let Ok(i) = device.get_connector(c, f) {
                if i.state() == connector::State::Connected && !i.modes().is_empty() {
                    conn_h = Some(c); mode = i.modes().first().copied(); break;
                }
            }
        }
        if conn_h.is_some() { break; }
    }
    let (mw, mh) = mode.ok_or("无模式")?.size();
    info!("🖥️  {}x{}", mw, mh);

    // Wayland
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();
    // Seat + Keyboard
    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "seat0");
    let kbd = seat.add_keyboard(XkbConfig::default(), 200, 25)?;

    // Output (wl_output global)
    let output = Output::new(
        "DP-4".to_string(),
        PhysicalProperties {
            size: (600, 340).into(),
            subpixel: Subpixel::Unknown,
            make: "NVIDIA".into(),
            model: "5080D".into(),
        },
    );
    let output_mode = Mode { size: (mw as i32, mh as i32).into(), refresh: 59000 };
    output.add_mode(output_mode);
    output.set_preferred(output_mode);
    output.change_current_state(
        Some(output_mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some(Point::from((0, 0))),
    );
    let _output_manager = OutputManagerState::new();
    output.create_global::<App>(&dh);
    info!("✅ wl_output");

    // 输入法协议：text-input-v3 + input-method-v2
    // fcitx5 需要 input-method-v2，客户端(foot)需要 text-input-v3
    InputMethodManagerState::new::<App, _>(&dh, |_client| true);
    TextInputManagerState::new::<App>(&dh);
    info!("✅ text-input / input-method");

    let mut state = App {
        comp: CompositorState::new::<App>(&dh), xdg: XdgShellState::new::<App>(&dh),
        shm: ShmState::new::<App>(&dh, vec![]), seat_state, seat,
        dd: DataDeviceState::new::<App>(&dh),
        output,
        osize: Size::new(mw as i32, mh as i32), tops: vec![], run: true, frame: 0,
        dh: dh.clone(), active: false, vblank: false, dirty: true,
        kbd,
        focus: None,
        pointer_pos: (0.0, 0.0),
        fullscreen: None,
    };
    let listener = ListeningSocket::bind("wayland-titan")?;
    std::env::set_var("WAYLAND_DISPLAY", "wayland-titan");
    // 确保 XDG_RUNTIME_DIR 设置正确（Edge/Feishu 等应用需要）
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }));
    }
    info!("✅ wayland-titan");

    // Event loop（必须在创建 surface 之前建好：mode-setting 需要先通过 libseat
    // 激活会话拿到 DRM master，否则会 EPERM）
    let mut eloop: EventLoop<App> = EventLoop::try_new()?;
    let mut clients: Vec<Client> = vec![];
    eloop.handle().insert_source(dn, |e,_,state: &mut App| match e {
        DrmEvent::VBlank(_) => { state.vblank = true; }
        DrmEvent::Error(e) => error!("DRM:{e:?}"),
    })?;
    // libseat 会话事件：激活/暂停（VT 切换、GDM 交接）。
    if let Some(notifier) = notifier {
        eloop.handle().insert_source(notifier, |event, _, state: &mut App| match event {
            SessionEvent::ActivateSession => { info!("▶️  会话激活"); state.active = true; }
            SessionEvent::PauseSession => { info!("⏸️  会话暂停"); state.active = false; }
        })?;
    }

    // 等待会话激活并取得 DRM master。GDM 交接时上一个合成器(greeter)可能还没
    // 完全释放 master，需要泵事件循环等待 libseat 的激活事件。
    if let Some(session) = session.as_ref() {
        state.active = session.lock().unwrap().is_active();
        let t0 = Instant::now();
        while !state.active && t0.elapsed() < Duration::from_secs(10) {
            eloop.dispatch(Some(Duration::from_millis(100)), &mut state)?;
            state.active = session.lock().unwrap().is_active();
        }
        if !state.active {
            return Err("libseat 会话 10s 内未激活，无法获取 DRM master".into());
        }
        device.activate(true)?;
        info!("✅ DRM master (libseat 会话已激活)");
    } else {
        state.active = true;
    }

    // DRM surface
    let crtc = *res.crtcs().first().ok_or("无 CRTC")?;
    let surface = device.create_surface(crtc, mode.unwrap(), &[conn_h.unwrap()])?;
    let fmts: Vec<Format> = [Fourcc::Argb8888, Fourcc::Xrgb8888].iter()
        .flat_map(|&c| [Format{code:c,modifier:Modifier::Linear}, Format{code:c,modifier:Modifier::Invalid}]).collect();
    let mut buf_surf = GbmBufferedSurface::new(surface, alloc, &[Fourcc::Argb8888, Fourcc::Xrgb8888], fmts.into_iter())?;
    info!("✅ Surface");

    // Libinput: 通过 udev 创建 libinput 上下文，注册到事件循环以接收键盘事件。
    // 用 libseat session 打开 /dev/input/*（通过 logind TakeDevice 获取 fd 权限），
    // 因为当前用户不在 input 组，直接 open 会被 EACCES 拒绝。
    {
        struct SessionInputInterface {
            session: Arc<std::sync::Mutex<LibSeatSession>>,
        }
        impl libinput_crate::LibinputInterface for SessionInputInterface {
    fn open_restricted(&mut self, path: &std::path::Path, flags: i32) -> Result<std::os::unix::io::OwnedFd, i32> {
                use smithay::reexports::rustix::fs::OFlags;
                use smithay::backend::session::AsErrno;
                self.session.lock().unwrap()
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
                warn!("⚠️  libinput assign_seat 失败: {:?}", e);
            } else {
                info!("✅ libinput (seat0)");
                let backend = LibinputInputBackend::new(libinput_ctx);
                eloop.handle().insert_source(backend, |event, _, state: &mut App| {
                    state.handle_input_event(event);
                })?;
            }
        }
    }

    let mut dev_active = state.active;
    // 是否有一次翻页(page flip)正在等待 VBlank。GbmBufferedSurface 要求：queue_buffer
    // 之后必须等到 DRM VBlank 事件、调用 frame_submitted() 后才能再提交下一帧，
    // 否则内核会以 EBUSY 拒绝提交。
    let mut pending_flip = false;
    let start = Instant::now();

    // 启动 fcitx5 输入法框架（中文输入需要）
    std::process::Command::new("fcitx5")
        .env("WAYLAND_DISPLAY", "wayland-titan")
        .env("XMODIFIERS", "@im=fcitx")
        .env("QT_IM_MODULE", "fcitx")
        .env("GTK_IM_MODULE", "fcitx")
        .spawn()
        .ok();
    info!("✅ fcitx5 已启动");

    info!("🔄 渲染中...");

    while state.run {
        // 处理会话激活/暂停（VT 切换、GDM 交接）
        if state.active != dev_active {
            if state.active { device.activate(true)?; buf_surf.reset_buffers(); pending_flip = false; info!("▶️  恢复渲染"); }
            else { device.pause(); pending_flip = false; info!("⏸️  暂停渲染"); }
            dev_active = state.active;
        }
        if !dev_active {
            eloop.dispatch(Some(Duration::from_millis(100)), &mut state)?;
            display.dispatch_clients(&mut state)?;
            display.flush_clients()?;
            continue;
        }

        // 渲染并提交新的一帧（仅在 VBlank 之后 pending_flip=false 且有 dirty 时）
        if !pending_flip && state.dirty {
            // 清理已关闭的窗口
            state.cleanup_dead();

            match buf_surf.next_buffer() {
                Ok((mut dmabuf, _)) => {
                    let mut elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();

                    if let Some(fi) = state.fullscreen {
                        // 全屏模式：只渲染全屏窗口
                        if let Some(tl) = state.tops.get(fi) {
                            for elem in render_elements_from_surface_tree(
                                &mut renderer, tl.wl_surface(), (0, 0), 1.0, 1.0, Kind::Unspecified)
                            {
                                elems.push(elem);
                            }
                        }
                    } else {
                        // 平铺模式
                        for (i, tl) in state.tops.iter().enumerate() {
                            let (x, y, _w, _h) = slot(i, state.tops.len(), state.osize.w, state.osize.h);
                            for elem in render_elements_from_surface_tree(
                                &mut renderer, tl.wl_surface(), (x, y), 1.0, 1.0, Kind::Unspecified)
                            {
                                elems.push(elem);
                            }
                        }
                    }

                    let mut target = renderer.bind(&mut dmabuf)?;
                    let sp = Size::<i32, Physical>::new(state.osize.w, state.osize.h);
                    let mut f = renderer.render(&mut target, sp, Transform::Normal)?;
                    let dmg = Rectangle::from_size(sp);

                    // 亮蓝色背景 (#2a1a4e)
                    f.clear(Color32F::new(0.16, 0.10, 0.31, 1.0), &[dmg])?;

                    // 屏幕中央画一个亮色 "TITAN" 标记 — 两个矩形
                    let cx = state.osize.w / 2; let cy = state.osize.h / 2;
                    // 白色横条
                    let bar = Rectangle::<i32, Physical>::new(Point::new(cx - 120, cy - 30), Size::new(240, 20));
                    f.clear(Color32F::new(1.0, 1.0, 1.0, 1.0), &[bar])?;
                    // 白色竖条
                    let bar2 = Rectangle::<i32, Physical>::new(Point::new(cx - 8, cy - 30), Size::new(16, 80));
                    f.clear(Color32F::new(1.0, 1.0, 1.0, 1.0), &[bar2])?;

                    // 帧计数器 — 在角落画一个随帧数变化的小方块
                    let fc = state.frame % 60;
                    let ind = Rectangle::<i32, Physical>::new(Point::new(10 + (fc as i32) * 4, 10), Size::new(3, 3));
                    f.clear(Color32F::new(0.0, 1.0, 0.0, 1.0), &[ind])?;

                    draw_render_elements(&mut f, 1.0, &elems, &[dmg])?;

                    // 软件光标：在 pointer_pos 画一个白色箭头形光标
                    let cx = state.pointer_pos.0 as i32;
                    let cy = state.pointer_pos.1 as i32;
                    // 光标主体（竖线）
                    let cursor = Rectangle::<i32, Physical>::new(
                        Point::new(cx, cy), Size::new(2, 18));
                    f.clear(Color32F::new(1.0, 1.0, 1.0, 1.0), &[cursor])?;
                    // 光标斜边
                    let cursor2 = Rectangle::<i32, Physical>::new(
                        Point::new(cx + 1, cy + 2), Size::new(1, 1));
                    f.clear(Color32F::new(1.0, 1.0, 1.0, 1.0), &[cursor2])?;
                    let cursor3 = Rectangle::<i32, Physical>::new(
                        Point::new(cx + 2, cy + 4), Size::new(1, 1));
                    f.clear(Color32F::new(1.0, 1.0, 1.0, 1.0), &[cursor3])?;

                    let _ = f.finish()?;
                    drop(target);
                    buf_surf.queue_buffer(None, None, ())?;
                    pending_flip = true;
                    state.dirty = false;
                    state.frame += 1;
                    if state.frame == 1 { info!("✅ 第一帧渲染！"); }
                    if state.frame % 600 == 0 { info!("📊 {} 帧", state.frame); }
                }
                Err(e) => { if state.frame == 0 { error!("❌ {e:?}"); } }
            }
        }

        eloop.dispatch(Some(Duration::from_millis(16)), &mut state)?;

        // VBlank 到达：上一帧已成功扫描输出，允许提交下一帧
        if state.vblank {
            state.vblank = false;
            buf_surf.frame_submitted()?;
            pending_flip = false;
        }

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

// ── 测试 ──────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_one_window_fills_screen() {
        // 1 窗口占满 2560×1440
        let (x, y, w, h) = slot(0, 1, 2560, 1440);
        assert_eq!((x, y, w, h), (0, 0, 2560, 1440));
    }

    #[test]
    fn test_slot_two_windows_split_horizontal() {
        // 2 窗口：左右各半
        let a = slot(0, 2, 2560, 1440);
        let b = slot(1, 2, 2560, 1440);
        assert_eq!(a, (0, 0, 1280, 1440));
        assert_eq!(b, (1280, 0, 1280, 1440));
        // 不重叠
        assert!(a.0 + a.2 <= b.0);
    }

    #[test]
    fn test_slot_three_windows_grid() {
        // 3 窗口：2 列 2 行，每格 1280×720
        let a = slot(0, 3, 2560, 1440);
        let b = slot(1, 3, 2560, 1440);
        let c = slot(2, 3, 2560, 1440);
        assert_eq!(a, (0, 0, 1280, 720));
        assert_eq!(b, (1280, 0, 1280, 720));
        assert_eq!(c, (0, 720, 1280, 720));
        // 同行不重叠
        assert!(a.0 + a.2 <= b.0);
        // 上下行不重叠
        assert!(a.1 + a.3 <= c.1);
    }

    #[test]
    fn test_slot_four_windows_grid() {
        // 4 窗口：2×2 网格，每格 1280×720
        let a = slot(0, 4, 2560, 1440);
        let b = slot(1, 4, 2560, 1440);
        let c = slot(2, 4, 2560, 1440);
        let d = slot(3, 4, 2560, 1440);
        assert_eq!(a, (0, 0, 1280, 720));
        assert_eq!(b, (1280, 0, 1280, 720));
        assert_eq!(c, (0, 720, 1280, 720));
        assert_eq!(d, (1280, 720, 1280, 720));
    }

    #[test]
    fn test_hit_test_finds_correct_window() {
        // 2 窗口布局，点击左半边应命中窗口 0，右半边命中窗口 1
        let n = 2;
        let (x0, y0, w0, h0) = slot(0, n, 2560, 1440);
        let (x1, y1, w1, h1) = slot(1, n, 2560, 1440);

        // 点击左边中心
        let px = 640;
        let py = 720;
        assert!(px >= x0 && px < x0 + w0 && py >= y0 && py < y0 + h0);
        assert!(px < x1 || py < y1); // 不在窗口 1

        // 点击右边中心
        let px = 1920;
        assert!(px >= x1 && px < x1 + w1 && py >= y1 && py < y1 + h1);
        assert!(px >= x0 + w0); // 不在窗口 0
    }

    #[test]
    fn test_slot_no_overlap() {
        // 对 n=1..6 的所有布局，验证窗口不重叠
        for n in 1..=6 {
            let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
            for i in 0..n {
                let (x, y, w, h) = slot(i, n, 2560, 1440);
                let rect = (x, y, w, h);
                for (j, prev) in rects.iter().enumerate() {
                    // 两个矩形不应重叠
                    let overlap = rect.0 < prev.0 + prev.2
                        && rect.0 + rect.2 > prev.0
                        && rect.1 < prev.1 + prev.3
                        && rect.1 + rect.3 > prev.1;
                    assert!(!overlap, "overlap: window {} {:?} vs window {} {:?}", j, prev, i, rect);
                }
                rects.push(rect);
            }
        }
    }

    #[test]
    fn test_slot_covers_screen() {
        // 偶数窗口完全覆盖，奇数窗口有空格（最后一行少一个）
        for n in [1, 2, 4, 6] {
            let total_area: i64 = (0..n)
                .map(|i| {
                    let (_, _, w, h) = slot(i, n, 2560, 1440);
                    (w as i64) * (h as i64)
                })
                .sum();
            let screen_area = 2560i64 * 1440;
            assert!(total_area >= screen_area * 99 / 100,
                "n={}: covers {} / {} = {:.0}%", n, total_area, screen_area, total_area * 100 / screen_area);
        }
        // 奇数窗口覆盖至少 70%
        for n in [3, 5] {
            let total_area: i64 = (0..n)
                .map(|i| {
                    let (_, _, w, h) = slot(i, n, 2560, 1440);
                    (w as i64) * (h as i64)
                })
                .sum();
            let screen_area = 2560i64 * 1440;
            assert!(total_area >= screen_area * 70 / 100,
                "n={}: covers {} / {} = {:.0}%", n, total_area, screen_area, total_area * 100 / screen_area);
        }
    }
}
