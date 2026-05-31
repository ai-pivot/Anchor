# Plan: Add XWayland Support to Anchor

## Summary

Add XWayland support so X11 apps (like Feishu) work out-of-the-box under Anchor without requiring `--ozone-platform=wayland` flags. Sway supports them because it has XWayland; Anchor currently does not.

## Root Cause Analysis

Sway works with Feishu because:
1. Sway creates standard `wayland-0` socket + has XWayland support
2. Feishu's custom Chromium defaults to X11 backend → XWayland handles it transparently

Anchor fails because:
1. No XWayland → X11 fallback fails silently → process becomes zombie
2. Non-standard `wayland-anchor` socket name (but this is secondary — even with standard name, Feishu would try X11 first)

## Changes

### `Cargo.toml`
- Add `"xwayland"` to smithay features list
- This pulls in `x11rb`, `encoding_rs`, and enables the `smithay::xwayland` module

### `src/main.rs` — Imports
- Add imports for `XWayland`, `XWaylandEvent`, `X11Wm`, `XwmHandler`, `XWaylandClientData`
- Add imports for `XWaylandShellHandler`, `XWaylandShellState`
- Add `delegate_xwayland_shell` macro

### `src/main.rs` — App struct
- Add fields:
  - `xwayland: Option<XWayland>` — keep XWayland alive
  - `xwm: Option<X11Wm>` — X11 window manager state
  - `xwayland_shell: XWaylandShellState` — serial matching for X11↔Wayland surface association
  - `x11_surfaces: Vec<X11Surface>` — tracked X11 windows (mapped, non-override-redirect)
  - `x11_or_surfaces: Vec<X11Surface>` — override-redirect windows (popups, tooltips)

### `src/main.rs` — XWayland spawn (after session activation, before main loop)
- Call `XWayland::spawn(&dh, None, std::iter::empty(), true, ...)` 
- Insert as calloop source
- On `XWaylandEvent::Ready`: call `X11Wm::start_wm()` and store in `App.xwm`
- On `XWaylandEvent::Error`: log warning, continue without XWayland

### `src/main.rs` — XWaylandShellHandler impl
- `fn xwayland_shell_state()` → return `&mut self.xwayland_shell`
- `fn surface_associated()` → log + mark dirty (wl_surface now available for rendering)

### `src/main.rs` — XwmHandler impl (the main event handler)
Required methods:
- `xwm_state()` → `self.xwm.as_mut().unwrap()`
- `new_window()` → log + store (wait for map_request)
- `new_override_redirect_window()` → store separately, set mapped
- `map_window_request()` → `window.set_mapped(true)`, add to active workspace, `do_layout()`, focus
- `mapped_override_redirect_window()` → mark dirty for rendering
- `unmapped_window()` → remove from workspace, `do_layout()`
- `destroyed_window()` → remove from tracking, `do_layout()`
- `configure_request()` → `window.configure()` with layout position
- `resize_request()` → handle edge resize
- `move_request()` → handle window move
- `send_selection()` → implement clipboard forwarding

### `src/main.rs` — Render integration
- In Phase 1 (surface collection): iterate `x11_surfaces`, for each that has `wl_surface()`, call `render_elements_from_surface_tree()` at its configured position
- In Step 2 (window rendering): draw X11 surface elements alongside Wayland toplevels
- In frame callbacks: also send frame callbacks to X11 surfaces' wl_surfaces

### `src/main.rs` — Focus integration
- When focusing an X11 window, set keyboard focus on the `X11Surface` (handles X11 WM_TAKE_FOCUS protocol)
- In `destroyed()` handler: also check x11_surfaces for cleanup

### `src/main.rs` — Delegate macros (bottom of file)
- Add `delegate_xwayland_shell!(App);`

## Risks
- **XWayland process management**: XWayland must stay alive for the session. Store in `Option<>` and handle gracefully if it fails to start. ✅ Non-blocking — compositor works without it.
- **X11 surface lifecycle**: X11 windows have different lifecycle than Wayland toplevels. Must handle both `unmapped_window` and `destroyed_window` callbacks. X11 windows can be unmapped without being destroyed.
- **Override-redirect windows**: These bypass WM control. Don't try to tile them. Render them as floating overlays (like tooltips, popup menus from X11 apps).
- **Focus conflicts**: X11 focus (set_input_focus) and Wayland focus (keyboard_enter) must be synchronized. Use `X11Surface` for keyboard focus when appropriate.
- **No GPU acceleration for X11**: XWayland renders via its own GL, then composites via wl_surface. The Pixman renderer in Anchor won't affect XWayland's internal rendering — it just receives the composited buffer.

## Definition of Done
- [ ] `cargo build --release` succeeds
- [ ] Feishu launches from Anchor's app launcher without any extra flags
- [ ] X11 app appears in tiling layout alongside native Wayland apps
- [ ] Closing X11 app removes it from layout, remaining windows re-tile
- [ ] Focus works: keyboard input reaches the X11 app
- [ ] Graceful fallback: if XWayland binary not installed, Anchor still works (X11 apps simply won't run)

## Open Questions
- Clipboard integration between X11 and Wayland apps — may need `send_selection` implementation (can defer to follow-up)
