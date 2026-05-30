// Titan — 极简平铺 Wayland 合成器 (DRM 后端, v8)
// 配置: ~/.config/titan/config.toml

mod config;
mod layout;
mod font;
mod block_linear;

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
    fullscreen: Option<usize>,
    cfg: Config,
    window_titles: std::collections::HashMap<usize, String>,
}

impl BufferHandler for App { fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {} }

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg }
    fn new_toplevel(&mut self, s: ToplevelSurface) {
        self.tops.push(s);
        let idx = self.tops.len() - 1;
        info!("➕ 窗口 #{}", idx);
        self.do_layout();
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
        let before = self.tops.len();
        let closed_idx = self.tops.iter().position(|tl| tl.wl_surface() == surface);
        self.tops.retain(|tl| tl.wl_surface() != surface);
        if self.tops.len() < before {
            info!("🗑️ 窗口关闭，剩余 {}", self.tops.len());
            if let Some(fi) = self.fullscreen {
                if let Some(ci) = closed_idx {
                    if fi == ci { self.fullscreen = None; }
                    else if fi > ci { self.fullscreen = Some(fi - 1); }
                }
            }
            if self.focus.as_ref() == Some(surface) {
                if let Some(tl) = self.tops.last() {
                    self.focus = Some(tl.wl_surface().clone());
                    let kbd = self.kbd.clone();
                    let serial = SERIAL_COUNTER.next_serial();
                    kbd.set_focus(self, Some(tl.wl_surface().clone()), serial);
                } else { self.focus = None; }
            }
            self.do_layout();
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

impl App {
    fn focus_idx(&self) -> Option<usize> {
        self.focus.as_ref().and_then(|s| self.tops.iter().position(|tl| tl.wl_surface() == s))
    }

    fn do_layout(&mut self) {
        let n = self.tops.len();
        if n == 0 { return; }
        if let Some(fi) = self.fullscreen {
            if fi >= n { self.fullscreen = None; }
        }
        let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
        if let Some(fi) = self.fullscreen {
            for (i, tl) in self.tops.iter().enumerate() {
                if i == fi {
                    tl.with_pending_state(|st| {
                        st.states.set(xdg_toplevel::State::Activated);
                        st.states.set(xdg_toplevel::State::Fullscreen);
                        st.size = Some((self.osize.w, self.osize.h - bar_h).into());
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
            for (i, tl) in self.tops.iter().enumerate() {
                let (_x, _y, w, h) = layout::slot(i, n, self.osize.w, self.osize.h, bar_h, &self.cfg);
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
        match (fi, self.fullscreen) {
            (Some(idx), Some(fs)) if idx == fs => { info!("🔳 取消全屏窗口 #{}", idx); self.fullscreen = None; }
            (Some(idx), _) => { info!("🔳 全屏窗口 #{}", idx); self.fullscreen = Some(idx); }
            _ => return,
        }
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
                                    info!("⌨️  启动终端: {}", data.cfg.terminal.command);
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
                                    if let Some(ref surf) = data.focus.clone() {
                                        if let Some(tl) = data.tops.iter().find(|tl| tl.wl_surface() == surf) { tl.send_close(); }
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
                                _ => {}
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::PointerMotion { event } => {
                self.pointer_pos.0 += event.delta_x();
                self.pointer_pos.1 += event.delta_y();
                self.pointer_pos.0 = self.pointer_pos.0.clamp(0.0, self.osize.w as f64);
                self.pointer_pos.1 = self.pointer_pos.1.clamp(0.0, self.osize.h as f64);
                self.dirty = true;
            }
            InputEvent::PointerButton { event } => {
                use smithay::backend::input::ButtonState;
                if event.state() == ButtonState::Pressed {
                    let px = self.pointer_pos.0 as i32;
                    let py = self.pointer_pos.1 as i32;
                    let bar_h = if self.cfg.bar.enabled { self.cfg.bar.height } else { 0 };
                    if py < bar_h { return; }
                    for (i, tl) in self.tops.iter().enumerate() {
                        let (x, y, w, h) = layout::slot(i, self.tops.len(), self.osize.w, self.osize.h, bar_h, &self.cfg);
                        if px >= x && px < x + w && py >= y && py < y + h {
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
    let cfg = Config::load();
    info!("🚀 Titan v8 ({})", if direct { "direct" } else { "session" });

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
    let gbm = GbmDevice::new(dev_fd)?;
    let mut renderer = PixmanRenderer::new()?;
    info!("✅ Pixman");
    let alloc = GbmAllocator::new(gbm, GbmBufferFlags::SCANOUT);

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

    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();
    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "seat0");
    let kbd = seat.add_keyboard(XkbConfig::default(), 200, 25)?;

    let output = Output::new("DP-4".to_string(), PhysicalProperties {
        size: (600, 340).into(), subpixel: Subpixel::Unknown, make: "NVIDIA".into(), model: "5080D".into(),
    });
    let output_mode = Mode { size: (mw as i32, mh as i32).into(), refresh: 59000 };
    output.add_mode(output_mode);
    output.set_preferred(output_mode);
    output.change_current_state(Some(output_mode), Some(Transform::Normal), Some(Scale::Integer(1)), Some(Point::from((0, 0))));
    let _output_manager = OutputManagerState::new();
    output.create_global::<App>(&dh);
    info!("✅ wl_output");

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
        kbd, focus: None, pointer_pos: (0.0, 0.0), fullscreen: None, cfg,
        window_titles: std::collections::HashMap::new(),
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
        DrmEvent::VBlank(_) => { state.vblank = true; }
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

    let crtc = *res.crtcs().first().ok_or("无 CRTC")?;
    let surface = device.create_surface(crtc, mode.unwrap(), &[conn_h.unwrap()])?;
    let fmts: Vec<Format> = [Fourcc::Argb8888, Fourcc::Xrgb8888].iter()
        .flat_map(|&c| [Format{code:c,modifier:Modifier::Linear}, Format{code:c,modifier:Modifier::Invalid}]).collect();
    let mut buf_surf = GbmBufferedSurface::new(surface, alloc, &[Fourcc::Argb8888, Fourcc::Xrgb8888], fmts.into_iter())?;
    info!("✅ Surface");

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
    let mut pending_flip = false;
    let start = Instant::now();

    std::process::Command::new("fcitx5")
        .env("WAYLAND_DISPLAY", "wayland-titan")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", unsafe { libc::getuid() }))
        .env("XMODIFIERS", "@im=fcitx").env("QT_IM_MODULE", "fcitx").env("GTK_IM_MODULE", "fcitx")
        .spawn().ok();

    info!("🔄 渲染中...");

    while state.run {
        if state.active != dev_active {
            if state.active { device.activate(true)?; buf_surf.reset_buffers(); pending_flip = false; }
            else { device.pause(); pending_flip = false; }
            dev_active = state.active;
        }
        if !dev_active {
            eloop.dispatch(Some(Duration::from_millis(100)), &mut state)?;
            display.dispatch_clients(&mut state)?;
            display.flush_clients()?;
            continue;
        }

        if !pending_flip && state.dirty {
            state.tops.retain(|tl| tl.alive());
            match buf_surf.next_buffer() {
                Ok((mut dmabuf, _)) => {
                    let mut elems: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = Vec::new();
                    let bar_h = if state.cfg.bar.enabled { state.cfg.bar.height } else { 0 };

                    if let Some(fi) = state.fullscreen {
                        if let Some(tl) = state.tops.get(fi) {
                            for elem in render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), (0, bar_h), 1.0, 1.0, Kind::Unspecified) {
                                elems.push(elem);
                            }
                        }
                    } else {
                        for (i, tl) in state.tops.iter().enumerate() {
                            let (x, y, _w, _h) = layout::slot(i, state.tops.len(), state.osize.w, state.osize.h, bar_h, &state.cfg);
                            for elem in render_elements_from_surface_tree(&mut renderer, tl.wl_surface(), (x, y), 1.0, 1.0, Kind::Unspecified) {
                                elems.push(elem);
                            }
                        }
                    }

                    let mut target = renderer.bind(&mut dmabuf)?;
                    let sp = Size::<i32, Physical>::new(state.osize.w, state.osize.h);
                    let mut f = renderer.render(&mut target, sp, Transform::Normal)?;
                    let dmg = Rectangle::from_size(sp);

                    // 壁纸/背景
                    layout::render_wallpaper(&mut f, &state.cfg, state.osize.w, state.osize.h, state.frame);

                    // 窗口内容
                    draw_render_elements(&mut f, 1.0, &elems, &[dmg])?;

                    let focus_idx = state.focus_idx();

                    // ★ 窗口装饰在内容之后 — glow/阴影覆盖在窗口边缘 ★
                    if state.fullscreen.is_none() {
                        for (i, _) in state.tops.iter().enumerate() {
                            layout::render_window_decorations(&mut f, &state.cfg, i, state.tops.len(), focus_idx, state.osize.w, state.osize.h, bar_h);
                        }
                    }

                    // ★ Headbar 渲染在窗口之后 — 确保永远在最顶层 ★
                    let time_secs = start.elapsed().as_secs();
                    // 窗口标题（从 App state 获取）
                    let window_title = &state.window_titles.get(&focus_idx.unwrap_or(0))
                        .cloned().unwrap_or_default();
                    layout::render_headbar(&mut f, &state.cfg, state.osize.w, state.osize.h, state.tops.len(), focus_idx, time_secs, window_title);

                    // 光标
                    let cx = state.pointer_pos.0 as i32;
                    let cy = state.pointer_pos.1 as i32;
                    let cc = Color32F::new(1.0, 1.0, 1.0, 0.9);
                    f.clear(cc, &[Rectangle::new(Point::new(cx, cy), Size::new(2, 18))])?;
                    f.clear(cc, &[Rectangle::new(Point::new(cx + 1, cy + 2), Size::new(1, 1))])?;
                    f.clear(cc, &[Rectangle::new(Point::new(cx + 2, cy + 4), Size::new(1, 1))])?;

                    let _ = f.finish()?;
                    drop(target);

                    // ★ Block-linear 转换：Pixman 渲染线性像素 → NVIDIA scanout 期望 block-linear ★
                    let fb_size = (state.osize.w * state.osize.h * 4) as usize;
                    let bh_gobs = state.cfg.wallpaper.block_height_gobs;
                    if bh_gobs > 0 {
                        use smithay::backend::allocator::dmabuf::{DmabufMappingMode, DmabufSyncFlags};
                        if let Ok(mapping) = dmabuf.map_plane(0, DmabufMappingMode::READ | DmabufMappingMode::WRITE) {
                            let ptr = mapping.ptr();
                            if !ptr.is_null() {
                                let slice = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, fb_size) };
                                block_linear::convert_in_place(slice, state.osize.w as usize, state.osize.h as usize, bh_gobs);
                            }
                            let _ = dmabuf.sync_plane(0, DmabufSyncFlags::WRITE);
                            drop(mapping);
                        }
                    }

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
        if state.vblank { state.vblank = false; buf_surf.frame_submitted()?; pending_flip = false; }
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
