// Titan — 极简平铺 Wayland 合成器 (winit X11 窗口模式, v5)
//
// 在 X11 窗口里运行，用于测试和截图
// 编译: cargo build --release --bin titan-winit --no-default-features --features winit
// 运行: DISPLAY=:0 ./target/release/titan-winit

use std::{
    os::unix::io::OwnedFd,
    sync::Arc,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        input::{InputEvent, KeyboardKeyEvent},
        renderer::{
            Bind, Frame,
            gles::GlesRenderer,
            element::{
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
                Kind,
            },
            utils::{draw_render_elements, on_commit_buffer_handler},
            Color32F, Renderer,
        },
        winit::{self, WinitEvent},
    },
    delegate_compositor, delegate_data_device, delegate_seat, delegate_shm, delegate_xdg_shell,
    input::{
        pointer::CursorImageStatus,
        Seat, SeatHandler, SeatState,
    },
    reexports::{
        calloop::EventLoop,
        wayland_server::{
            Display, DisplayHandle,
            protocol::{wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Size, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            with_surface_tree_downward, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction,
        },
        selection::{
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};
use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{
    Client, ListeningSocket,
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::wl_buffer,
};
use xkbcommon::xkb::{keysyms, Keysym};
use tracing::info;

// ── 平铺布局 ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum SplitDir { Horizontal, Vertical }

#[derive(Debug)]
enum TileNode {
    Window { surface: WlSurface },
    Split { dir: SplitDir, ratio: f64, children: Vec<TileNode> },
}

impl TileNode {
    fn clone_shallow(&self) -> Self {
        match self {
            TileNode::Window { surface } => TileNode::Window { surface: surface.clone() },
            TileNode::Split { dir, ratio, children } => TileNode::Split {
                dir: *dir, ratio: *ratio,
                children: children.iter().map(|c| c.clone_shallow()).collect(),
            },
        }
    }
    fn child_count(&self) -> usize {
        match self {
            TileNode::Window { .. } => 1,
            TileNode::Split { children, .. } => children.iter().map(|c| c.child_count()).sum(),
        }
    }
    fn layout(&self, area: Rectangle<i32, Logical>) -> Vec<(WlSurface, Rectangle<i32, Logical>)> {
        let mut out = Vec::new(); self.layout_rec(area, &mut out); out
    }
    fn layout_rec(&self, area: Rectangle<i32, Logical>, out: &mut Vec<(WlSurface, Rectangle<i32, Logical>)>) {
        match self {
            TileNode::Window { surface } => { out.push((surface.clone(), area)); }
            TileNode::Split { dir, ratio, children } => {
                if children.is_empty() { return; }
                if children.len() == 1 { return children[0].layout_rec(area, out); }
                let gap = 4; let n = children.len() as i32;
                let total = match dir { SplitDir::Horizontal => area.size.w, SplitDir::Vertical => area.size.h };
                let usable = total - gap * (n - 1);
                let sizes: Vec<i32> = {
                    let first = (usable as f64 * ratio) as i32;
                    let rest = if n > 1 { (usable - first) / (n - 1) } else { usable };
                    let mut s = vec![first]; for _ in 1..n { s.push(rest); } s
                };
                let mut offset = 0;
                for (i, child) in children.iter().enumerate() {
                    let sz = sizes[i];
                    let ca = match dir {
                        SplitDir::Horizontal => Rectangle {
                            loc: Point::new(area.loc.x + offset, area.loc.y), size: Size::new(sz, area.size.h) },
                        SplitDir::Vertical => Rectangle {
                            loc: Point::new(area.loc.x, area.loc.y + offset), size: Size::new(area.size.w, sz) },
                    };
                    child.layout_rec(ca, out); offset += sz + gap;
                }
            }
        }
    }
    fn insert_next_to(&mut self, t: &WlSurface, node: TileNode, dir: SplitDir) -> bool {
        match self {
            TileNode::Window { surface } if surface == t => {
                let old = std::mem::replace(self, TileNode::Split { dir, ratio: 0.5, children: vec![] });
                if let TileNode::Split { children, .. } = self { children.push(old); children.push(node); }
                true
            }
            TileNode::Split { children, .. } => children.iter_mut().any(|c| c.insert_next_to(t, node.clone_shallow(), dir)),
            _ => false,
        }
    }
    fn remove(&mut self, t: &WlSurface) -> bool {
        match self {
            TileNode::Window { surface } => surface == t,
            TileNode::Split { children, .. } => {
                let changed = children.iter_mut().fold(false, |a, c| c.remove(t) || a);
                children.retain(|c| c.child_count() > 0);
                if children.len() == 1 { *self = children.remove(0); }
                changed
            }
        }
    }
    fn all_surfaces(&self) -> Vec<WlSurface> { let mut v = vec![]; self.collect(&mut v); v }
    fn collect(&self, o: &mut Vec<WlSurface>) { match self { TileNode::Window { surface } => o.push(surface.clone()), TileNode::Split { children, .. } => children.iter().for_each(|c| c.collect(o)), } }
    fn next_after(&self, t: &WlSurface) -> Option<WlSurface> { let a = self.all_surfaces(); a.iter().position(|s| s == t).map(|i| a[(i+1)%a.len()].clone()).or_else(|| a.first().cloned()) }
    fn prev_before(&self, t: &WlSurface) -> Option<WlSurface> { let a = self.all_surfaces(); a.iter().position(|s| s == t).map(|i| a[if i==0{a.len()-1}else{i-1}].clone()).or_else(|| a.last().cloned()) }
}

// ── App 状态 ────────────────────────────────────────────

struct App {
    comp: CompositorState, xdg: XdgShellState, shm: ShmState, seat_state: SeatState<Self>,
    dd: DataDeviceState, seat: Seat<Self>,
    root: TileNode, focus: Option<WlSurface>, next_dir: SplitDir, osize: Size<i32, Logical>,
    tops: Vec<ToplevelSurface>, run: bool,
    #[allow(dead_code)]
    dh: DisplayHandle,
}

impl BufferHandler for App { fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {} }
impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg }
    fn new_toplevel(&mut self, s: ToplevelSurface) {
        let w = s.wl_surface().clone();
        s.with_pending_state(|st| { st.states.set(xdg_toplevel::State::Activated); });
        s.send_configure();
        info!("➕ 窗口");
        let n = TileNode::Window { surface: w.clone() };
        if let Some(ref f) = self.focus {
            if !self.root.insert_next_to(f, n.clone_shallow(), self.next_dir) { self.append_to_root(n); }
        } else { self.append_to_root(n); }
        self.set_focus(&w); self.tops.push(s);
    }
    fn new_popup(&mut self, _: PopupSurface, _: PositionerState) {}
    fn grab(&mut self, _: PopupSurface, _: wl_seat::WlSeat, _: smithay::utils::Serial) {}
    fn reposition_request(&mut self, _: PopupSurface, _: PositionerState, _: u32) {}
}
impl SelectionHandler for App { type SelectionUserData = (); }
impl DataDeviceHandler for App { fn data_device_state(&self) -> &DataDeviceState { &self.dd } }
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
    fn new(dh: &DisplayHandle) -> Self {
        App {
            comp: CompositorState::new::<App>(dh), xdg: XdgShellState::new::<App>(dh),
            shm: ShmState::new::<App>(dh, vec![]),
            seat_state: SeatState::new(), seat: SeatState::new().new_wl_seat(dh, "seat0"),
            dd: DataDeviceState::new::<App>(dh),
            root: TileNode::Split { dir: SplitDir::Horizontal, ratio: 0.5, children: vec![] },
            focus: None, next_dir: SplitDir::Horizontal, osize: Size::new(1280, 720),
            tops: vec![], run: true, dh: dh.clone(),
        }
    }
    fn append_to_root(&mut self, node: TileNode) {
        match &mut self.root {
            TileNode::Split { children, .. } => children.push(node),
            _ => { let old = std::mem::replace(&mut self.root, TileNode::Split { dir: self.next_dir, ratio: 0.5, children: vec![] }); if let TileNode::Split { children, .. } = &mut self.root { children.push(old); children.push(node); } }
        }
    }
    fn set_focus(&mut self, s: &WlSurface) {
        if let Some(ref old) = self.focus { if old != s { if let Some(t) = self.find_top(old) { t.with_pending_state(|st|{st.states.unset(xdg_toplevel::State::Activated);}); t.send_configure(); }}}
        self.focus = Some(s.clone());
        if let Some(t) = self.find_top(s) { t.with_pending_state(|st|{st.states.set(xdg_toplevel::State::Activated);}); t.send_configure(); }
    }
    fn find_top(&self, s: &WlSurface) -> Option<&ToplevelSurface> { self.tops.iter().find(|t| t.wl_surface() == s) }
    fn remove_win(&mut self, s: &WlSurface) {
        if self.focus.as_ref() == Some(s) { self.focus = self.root.next_after(s); }
        if self.root.remove(s) && self.root.all_surfaces().is_empty() { self.root = TileNode::Split { dir: SplitDir::Horizontal, ratio: 0.5, children: vec![] }; self.focus = None; }
        self.tops.retain(|t| t.wl_surface() != s);
    }
    fn fnxt(&mut self) { if let Some(ref c) = self.focus { if let Some(n) = self.root.next_after(c) { self.set_focus(&n); }}}
    fn fprv(&mut self) { if let Some(ref c) = self.focus { if let Some(p) = self.root.prev_before(c) { self.set_focus(&p); }}}
}

#[derive(Default)] struct ClientState { comp: CompositorClientState }
impl ClientData for ClientState { fn initialized(&self, _: ClientId) {} fn disconnected(&self, _: ClientId, _: DisconnectReason) {} }

fn handle_sym(app: &mut App, sym: &Keysym, shift: bool) {
    let k = |x: u32| Keysym::new(x);
    if *sym == k(keysyms::KEY_Return) && !shift { spawn_term(); }
    else if *sym == k(0x0071) && shift { let s = app.focus.clone(); if let Some(s) = s { app.remove_win(&s); } }
    else if (*sym == k(keysyms::KEY_Left) || *sym == k(keysyms::KEY_Up)) && !shift { app.fprv(); }
    else if (*sym == k(keysyms::KEY_Right) || *sym == k(keysyms::KEY_Down)) && !shift { app.fnxt(); }
    else if *sym == k(0x0076) && shift { app.next_dir = SplitDir::Vertical; info!("📐 ↓"); }
    else if *sym == k(0x0062) && shift { app.next_dir = SplitDir::Horizontal; info!("📐 →"); }
    else if *sym == k(keysyms::KEY_space) && shift {
        app.next_dir = match app.next_dir { SplitDir::Horizontal => SplitDir::Vertical, SplitDir::Vertical => SplitDir::Horizontal };
        info!("📐 {}", match app.next_dir { SplitDir::Horizontal => "→", SplitDir::Vertical => "↓" });
    }
}
fn spawn_term() { std::process::Command::new("sh").arg("-c").arg("foot &").spawn().ok(); }
fn send_frames(s: &WlSurface, t: u32) {
    with_surface_tree_downward(s, (), |_,_,&()| TraversalAction::DoChildren(()),
        |_,st,&()| { for cb in st.cached_state.get::<SurfaceAttributes>().current().frame_callbacks.drain(..) { cb.done(t); } },
        |_,_,&()| true,
    );
}

// ── winit 主循环 ─────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info,smithay=warn").init();
    info!("🚀 Titan winit (X11 窗口模式)");

    // Display
    let mut display: Display<App> = Display::new()?;
    let dh = display.handle();
    let mut state = App::new(&dh);

    // Socket
    let listener = ListeningSocket::bind("wayland-titan")?;
    std::env::set_var("WAYLAND_DISPLAY", "wayland-titan");
    info!("✅ Wayland socket: wayland-titan");

    // winit backend — GlesRenderer 可以从 EGL 获取
    let (mut backend, mut input) = winit::init::<GlesRenderer>()?;
    info!("✅ winit + GLES 渲染器就绪");

    // Keyboard
    let keyboard = state.seat.add_keyboard(Default::default(), 200, 200)?;

    // Event loop
    let mut eloop: EventLoop<App> = EventLoop::try_new()?;
    let mut clients: Vec<Client> = vec![];
    let start = Instant::now();
    let mut frame_count = 0u32;

    info!("🔄 主循环");

    loop {
        // Process winit events
        let kbd = keyboard.clone();
        input.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                state.osize = Size::new(size.w, size.h);
                info!("📏 窗口大小: {}x{}", size.w, size.h);
            }
            WinitEvent::Input(input_event) => {
                if let InputEvent::Keyboard { event } = input_event {
                    kbd.input::<(), _>(&mut state, event.key_code(), event.state(), 0.into(), 0,
                        |app, modifiers, handle| {
                            let sym = handle.modified_sym();
                            let shift = modifiers.shift;
                            if modifiers.logo { handle_sym(app, &sym, shift); }
                            smithay::input::keyboard::FilterResult::Forward
                        },
                    );
                }
            }
            WinitEvent::CloseRequested => {
                info!("👋 窗口关闭");
                state.run = false;
            }
            _ => {}
        });

        if !state.run { break; }

        // Render
        let size = backend.window_size();
        let damage = Rectangle::<i32, Physical>::from_size(size);

        // Render in a block to release backend borrow before submit
        {
            let (renderer, mut framebuffer) = backend.bind()?;
            
            let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = state.tops.iter()
                .flat_map(|tl| {
                    let wl = tl.wl_surface();
                    let layout = state.root.layout(Rectangle {
                        loc: Point::new(0, 0), size: state.osize
                    });
                    let pos = layout.iter()
                        .find(|(s, _)| s == wl)
                        .map(|(_, r)| (r.loc.x, r.loc.y))
                        .unwrap_or((0, 0));
                    render_elements_from_surface_tree(renderer, wl, pos, 1.0, 1.0, Kind::Unspecified)
                }).collect();

            let mut frame = renderer.render(&mut framebuffer, size, Transform::Normal)?;
            frame.clear(Color32F::new(0.12, 0.12, 0.15, 1.0), &[damage])?;
            draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;
            frame.finish()?;
        }

        backend.submit(Some(&[damage]))?;

        frame_count += 1;
        if frame_count == 1 {
            info!("✅ 第一帧渲染完成！");
        }

        eloop.dispatch(Some(Duration::from_millis(16)), &mut state)?;

        if let Ok(Some(stream)) = listener.accept() {
            clients.push(display.handle().insert_client(stream, Arc::new(ClientState::default()))?);
        }
        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;

        let now = start.elapsed().as_millis() as u32;
        for s in state.xdg.toplevel_surfaces() { send_frames(s.wl_surface(), now); }
    }

    info!("👋 退出"); Ok(())
}

delegate_xdg_shell!(App);
delegate_compositor!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_data_device!(App);
