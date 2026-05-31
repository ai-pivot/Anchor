//! XWayland support for Anchor
//!
//! Allows X11 applications (Feishu, etc.) to run under the Anchor compositor.
//! Spawns an XWayland server and acts as its X11 Window Manager.
//! X11 surfaces integrate into the same tiling layout as native Wayland windows.
//!
//! ## Integration (main.rs)
//!
//! ```ignore
//! // 1. Cargo.toml: add "xwayland" to smithay features
//!
//! // 2. Add module + field
//! mod xwayland;
//! // In App struct:
//!     xw: xwayland::XWaylandState,
//!
//! // 3. Init (before spawn, after Display created)
//!     let xw = xwayland::XWaylandState::new::<App>(&dh);
//!
//! // 4. Spawn (after session activation)
//!     let (xwayland_src, xw_client) = xwayland::spawn_xwayland(&dh)?;
//!     let xw_client_clone = xw_client.clone();
//!     eloop.handle().insert_source(xwayland_src, |event, _, state: &mut App| {
//!         xwayland::handle_xwayland_event(event, state, &xw_client_clone);
//!     })?;
//!
//! // 5. impl XWaylandShellHandler for App + delegate_xwayland_shell!(App);
//! // 6. impl XwmHandler for App — delegate to self.xw.on_*()
//! // 7. In render loop: render state.xw.renderable_surfaces()
//! ```

use std::os::unix::io::OwnedFd;

use smithay::{
    reexports::wayland_server::{
        Client, DisplayHandle,
        protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Rectangle},
    wayland::selection::SelectionTarget,
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent,
        xwm::{Reorder, ResizeEdge, WmWindowProperty, X11Window, XwmId},
    },
    wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
};

// ── XWaylandState ───────────────────────────────────────────

/// All XWayland-related state, stored as a field in the App struct.
///
/// Lifecycle:
/// - `new()` during App init (BEFORE spawn)
/// - `spawn_xwayland()` + `handle_xwayland_event()` after session activation
/// - X11Wm becomes available asynchronously when XWayland reports Ready
pub struct XWaylandState {
    /// X11 Window Manager. `Some` once XWayland reports Ready.
    pub xwm: Option<X11Wm>,
    /// Serial-based wl_surface ↔ X11Surface matching.
    pub shell: XWaylandShellState,
    /// Normal X11 windows (tiled, non-override-redirect).
    pub surfaces: Vec<X11Surface>,
    /// Override-redirect X11 windows (popups, tooltips — floating overlay).
    pub or_surfaces: Vec<X11Surface>,
    /// Flag: set when a surface is added/removed. main.rs should do_layout().
    pub needs_layout: bool,
}

impl XWaylandState {
    /// Create state. Must be called BEFORE `spawn_xwayland()` so the
    /// xwayland_shell global exists when XWayland tries to bind it.
    pub fn new<D>(dh: &DisplayHandle) -> Self
    where
        D: 'static,
        D: smithay::reexports::wayland_server::GlobalDispatch<
            smithay::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_shell_v1::XwaylandShellV1, (),
        >,
        D: smithay::reexports::wayland_server::Dispatch<
            smithay::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_shell_v1::XwaylandShellV1, (),
        >,
        D: smithay::reexports::wayland_server::Dispatch<
            smithay::reexports::wayland_protocols::xwayland::shell::v1::server::xwayland_surface_v1::XwaylandSurfaceV1,
            smithay::wayland::xwayland_shell::XWaylandSurfaceUserData,
        >,
    {
        Self {
            xwm: None,
            shell: XWaylandShellState::new::<D>(dh),
            surfaces: Vec::new(),
            or_surfaces: Vec::new(),
            needs_layout: false,
        }
    }

    /// Whether the X11 WM is running and ready.
    pub fn is_available(&self) -> bool {
        self.xwm.is_some()
    }

    /// Find a tiled X11 surface by its wl_surface.
    pub fn find_surface(&self, wl: &WlSurface) -> Option<(usize, &X11Surface)> {
        self.surfaces.iter().enumerate()
            .find(|(_, s)| s.wl_surface().as_ref() == Some(wl))
    }

    /// Remove an X11 surface by its wl_surface. Returns true if removed.
    pub fn remove_surface(&mut self, wl: &WlSurface) -> bool {
        let before = self.surfaces.len();
        self.surfaces.retain(|s| s.wl_surface().as_ref() != Some(wl));
        self.surfaces.len() < before
    }

    /// Remove an X11 surface by window ID from either list.
    pub fn remove_by_wid(&mut self, wid: X11Window) -> bool {
        let before = self.surfaces.len();
        self.surfaces.retain(|s| s.window_id() != wid);
        if self.surfaces.len() < before { return true; }
        let before = self.or_surfaces.len();
        self.or_surfaces.retain(|s| s.window_id() != wid);
        self.or_surfaces.len() < before
    }

    /// Iterate tiled X11 surfaces that have a wl_surface (ready for rendering).
    pub fn renderable_surfaces(&self) -> impl Iterator<Item = (usize, &X11Surface)> {
        self.surfaces.iter().enumerate()
            .filter(|(_, s)| s.wl_surface().is_some())
    }

    /// Configure an X11 surface to the given layout rectangle.
    pub fn configure_surface(&self, idx: usize, rect: Rectangle<i32, Logical>) {
        if let Some(surface) = self.surfaces.get(idx) {
            if let Err(e) = surface.configure(Some(rect)) {
                tracing::warn!("⚠️  X11 configure #{} failed: {:?}", idx, e);
            }
        }
    }

    /// Set activation state on an X11 surface.
    pub fn set_activated(&self, idx: usize, activated: bool) {
        if let Some(surface) = self.surfaces.get(idx) {
            if let Err(e) = surface.set_activated(activated) {
                tracing::warn!("⚠️  X11 set_activated failed: {:?}", e);
            }
        }
    }

    // ── XwmHandler callback helpers ─────────────────────────
    //
    // Call these from your `impl XwmHandler for App`.
    // Each handles the data management side; your impl then
    // calls do_layout() / focus / mark dirty as needed.

    /// Handle `new_override_redirect_window`.
    pub fn on_new_or_window(&mut self, window: X11Surface) {
        let wid = window.window_id();
        if !self.or_surfaces.iter().any(|s| s.window_id() == wid) {
            self.or_surfaces.push(window);
        }
    }

    /// Handle `map_window_request`. Sets mapped and adds to surfaces list.
    /// Returns `Some(index)` if newly added, `None` on error.
    pub fn on_map_request(&mut self, window: &X11Surface) -> Option<usize> {
        if let Err(e) = window.set_mapped(true) {
            tracing::warn!("⚠️  X11 set_mapped failed: {:?}", e);
            return None;
        }
        let wid = window.window_id();
        let is_new = !self.surfaces.iter().any(|s| s.window_id() == wid);
        if is_new {
            self.surfaces.push(window.clone());
            self.needs_layout = true;
            Some(self.surfaces.len() - 1)
        } else {
            self.surfaces.iter().position(|s| s.window_id() == wid)
        }
    }

    /// Handle `unmapped_window` / `destroyed_window`.
    pub fn on_unmapped(&mut self, window: &X11Surface) {
        self.remove_by_wid(window.window_id());
        self.needs_layout = true;
    }

    /// Handle `configure_request`. Acknowledges with current or default geometry.
    /// Does NOT change tiled layout — do_layout() controls positions.
    /// For OR windows (input method popups), accepts the client's requested position.
    pub fn on_configure_request(&self, window: &X11Surface, x: Option<i32>, y: Option<i32>, w: Option<u32>, h: Option<u32>) {
        // Check if this is an override-redirect window
        let is_or = self.or_surfaces.iter().any(|s| s.window_id() == window.window_id());
        
        if window.wl_surface().is_some() {
            if is_or {
                // OR windows: accept client's requested position and size
                let cur_geo = window.geometry();
                let new_x = x.unwrap_or(cur_geo.loc.x);
                let new_y = y.unwrap_or(cur_geo.loc.y);
                let new_w = w.map(|v| v as i32).unwrap_or(cur_geo.size.w);
                let new_h = h.map(|v| v as i32).unwrap_or(cur_geo.size.h);
                let _ = window.configure(Some(Rectangle::from_loc_and_size(
                    (new_x, new_y),
                    (new_w, new_h),
                )));
            } else {
                // Tiled windows: compositor controls position, just ack with current geometry
                let geo = window.geometry();
                let _ = window.configure(Some(geo));
            }
        } else {
            let sw = w.unwrap_or(800) as i32;
            let sh = h.unwrap_or(600) as i32;
            let _ = window.configure(Some(Rectangle::from_size((sw, sh).into())));
        }
    }

    /// Acknowledge a move/resize/maximize with current geometry.
    pub fn ack_with_current_geometry(&self, window: &X11Surface) {
        let geo = window.geometry();
        let _ = window.configure(Some(geo));
    }
}

// ── Spawn + Event Handling ──────────────────────────────────

/// Spawn the XWayland process. Returns the calloop event source and the
/// Wayland `Client` for XWayland.
///
/// Usage in main.rs:
/// ```ignore
/// let (xwayland_src, xw_client) = xwayland::spawn_xwayland(&dh)?;
/// let client = xw_client.clone();
/// eloop.handle().insert_source(xwayland_src, |event, _, state: &mut App| {
///     xwayland::handle_xwayland_event(event, &state.dh, &client, &mut state.xw);
/// })?;
/// ```
pub fn spawn_xwayland(
    dh: &DisplayHandle,
) -> Result<(XWayland, Client), Box<dyn std::error::Error>> {
    let (xwayland, client) = XWayland::spawn(
        dh,
        None,                                              // auto display number
        std::iter::empty::<(String, String)>(),
        true,                                              // open abstract socket
        std::process::Stdio::null(),
        std::process::Stdio::null(),
        |_| (),
    )?;
    tracing::info!("✅ XWayland spawning...");
    Ok((xwayland, client))
}

/// Handle an `XWaylandEvent` from the calloop source.
///
/// On `Ready`, starts the X11 Window Manager and stores it in `state.xwm`.
/// On `Error`, logs a warning.
///
/// Requires a `LoopHandle` from somewhere — we take it via the state
/// that implements `HasLoopHandle`. Or simpler: pass it directly.
pub fn handle_xwayland_event<D>(
    event: XWaylandEvent,
    loop_handle: &smithay::reexports::calloop::LoopHandle<'static, D>,
    client: &Client,
    state: &mut XWaylandState,
) where
    D: 'static,
    D: XWaylandShellHandler,
    D: smithay::xwayland::XwmHandler,
{
    match event {
        XWaylandEvent::Ready { x11_socket, display_number } => {
            tracing::info!("🖥️  XWayland ready (display :{})", display_number);

            // Set DISPLAY so child processes (Feishu, Chrome, etc.) find X11
            std::env::set_var("DISPLAY", format!(":{}", display_number));

            match X11Wm::start_wm(loop_handle.clone(), x11_socket, client.clone()) {
                Ok(wm) => {
                    tracing::info!("✅ X11 Window Manager started");
                    state.xwm = Some(wm);
                }
                Err(e) => {
                    tracing::error!("❌ X11 WM start failed: {:?}", e);
                }
            }
        }
        XWaylandEvent::Error => {
            tracing::warn!("⚠️  XWayland failed to start — X11 apps won't work");
        }
    }
}
