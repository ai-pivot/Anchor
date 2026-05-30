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
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell,
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
        output::OutputManagerState,
        selection::{
            SelectionHandler,
            data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler}},
        shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState},
        shm::{ShmHandler, ShmState},
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
    /// libinput keyboard state — xkb context + keymap for keysym translation
    kbd: smithay::input::keyboard::KeyboardHandle<Self>,
}

impl BufferHandler for App { fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {} }
impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg }
    fn new_toplevel(&mut self, s: ToplevelSurface) {
        s.with_pending_state(|st| st.states.set(xdg_toplevel::State::Activated));
        s.send_configure();
        // 自动聚焦新窗口
        let kbd = self.kbd.clone();
        let surface = s.wl_surface().clone();
        let serial = SERIAL_COUNTER.next_serial();
        kbd.set_focus(self, Some(surface), serial);
        info!("➕ 窗口");
        self.tops.push(s);
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
impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState { &mut self.comp }
    fn client_compositor_state<'a>(&self, c: &'a Client) -> &'a CompositorClientState { &c.get_data::<ClientState>().unwrap().comp }
    fn commit(&mut self, s: &WlSurface) { on_commit_buffer_handler::<Self>(s); }
}
impl ShmHandler for App { fn shm_state(&self) -> &ShmState { &self.shm } }
impl SeatHandler for App {
    type KeyboardFocus = WlSurface; type PointerFocus = WlSurface; type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> { &mut self.seat_state }
    fn focus_changed(&mut self, _: &Seat<Self>, _: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {}
}

impl App {
    /// 终端模拟器命令
    const TERMINAL: &'static str = "foot";

    /// 处理 libinput 输入事件
    fn handle_input_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        use smithay::backend::input::{KeyboardKeyEvent as _, Event as _};
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
                                _ => {}
                            }
                        }
                        FilterResult::Forward
                    },
                );
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

    let direct = std::env::args().any(|a| a == "--direct");
    info!("🚀 Titan v7 ({})", if direct { "direct" } else { "session" });

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

    let mut state = App {
        comp: CompositorState::new::<App>(&dh), xdg: XdgShellState::new::<App>(&dh),
        shm: ShmState::new::<App>(&dh, vec![]), seat_state, seat,
        dd: DataDeviceState::new::<App>(&dh),
        output,
        osize: Size::new(mw as i32, mh as i32), tops: vec![], run: true, frame: 0,
        dh: dh.clone(), active: false, vblank: false,
        kbd,
    };
    let listener = ListeningSocket::bind("wayland-titan")?;
    std::env::set_var("WAYLAND_DISPLAY", "wayland-titan");
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

        // 仅在没有翻页在途时渲染并提交新的一帧
        if !pending_flip {
            match buf_surf.next_buffer() {
                Ok((mut dmabuf, _)) => {
                    let elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = state.tops.iter()
                        .flat_map(|tl| render_elements_from_surface_tree(
                            &mut renderer, tl.wl_surface(), (0,0), 1.0, 1.0, Kind::Unspecified)).collect();

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
                    let _ = f.finish()?;
                    drop(target);
                    buf_surf.queue_buffer(None, None, ())?;
                    pending_flip = true;
                    state.frame += 1;
                    if state.frame == 1 { info!("✅ 第一帧渲染！"); }
                    if state.frame % 600 == 0 { info!("📊 {} 帧", state.frame); }
                }
                Err(e) => { if state.frame == 0 { error!("❌ {e:?}"); } }
            }
        }

        eloop.dispatch(Some(Duration::from_millis(16)), &mut state)?;

        // VBlank 到达：上一帧已成功扫描输出，标记完成并允许提交下一帧
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
