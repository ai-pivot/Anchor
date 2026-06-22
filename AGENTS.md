# Anchor — AGENTS.md

> Wayland tiling compositor in Rust, using Smithay 0.7.
> Cross-GPU: NVIDIA proprietary / AMD / Intel. Any display manager (GDM/SDDM/LightDM).

## Architecture

- **Language**: Rust
- **Compositor framework**: Smithay 0.7
- **GPU**: Auto-detected (NVIDIA / AMD / Intel via `/sys/class/drm/*/device/vendor`)
- **Renderer**: GlesRenderer (GPU OpenGL ES via Smithay; works on NVIDIA proprietary, AMD, Intel)
- **Session**: libseat (logind backend) — works with GDM, SDDM, LightDM

## Project Structure

```
src/
  main.rs        — Main compositor loop, App struct, keyboard/mouse handling, GPU auto-detect
  workspace.rs   — Workspace struct, WindowSlot enum, unified render order
  lock.rs        — Lock screen state machine (PAM auth, shake animation, random styles)
  launcher.rs    — Built-in app launcher (XDG .desktop scanning, search filter)
  scratchpad.rs  — Quake-style dropdown terminal state machine
  xwayland.rs    — XWayland support (X11 app compatibility: spawn, surface tracking, helpers)
  config.rs      — TOML config parsing (includes GPU config section)
  text_render.rs — fontdue-based TTF text rendering
  auth.rs        — PAM authentication via FFI (lock screen password verification)
  wallpaper.rs   — Wallpaper loading and caching (gradient, image, random)
  cursor.rs      — XCursor theme loading and rendering
  notify.rs      — DBus notification listener (org.freedesktop.Notifications)
  screenshot.rs  — Screenshot capture (area selection, DRM framebuffer dump)
  overview.rs    — Overview/Task Panel state machine (Cover Flow, animation)
  physics.rs     — Spring physics engine (scroll snap, momentum)
  record.rs      — Screen recording state
  block_linear.rs — Block-linear memory layout helpers (NVIDIA)
  headerbar.rs   — Header Bar protocol (anchor-header-bar-v1, client-side header bar support)
  layout/
    mod.rs        — Module entry point, re-exports all public API
    geom.rs       — Layout geometry (LayoutPreset, SplitDir, slot calculation)
    util.rs       — Shared helpers (opaque, color_hex, rect, spacing constants)
    wallpaper.rs  — Wallpaper rendering (gradient grid, 5 glow spots, breathing pulse)
    decorations.rs — Window border decorations (multi-layer glow, gradient, corners)
    headbar.rs    — Top bar (gradient ws, sliding indicator, pulse clock, CPU/MEM glow)
    notifications.rs — Toast notification rendering (shadow, fade animation)
    launcher.rs   — Launcher overlay rendering (blinking cursor, glow border)
    lock_screen.rs — Lock screen rendering (5 animated styles + dim overlay)
    overview.rs   — Overview/Task Panel background rendering (shadow, dim)
```
scripts/
  anchor-session   — DM session wrapper (auto-detects GPU, sets env vars conditionally)
  anchor-launcher  — App launcher script (dmenu/wmenu based)
config.toml       — User configuration
Cargo.toml        — Dependencies (smithay, fontdue, libc, chrono, image, zbus, etc.)
build.rs          — Build script (links libpam)
```

## Features (v30)

| Feature | Keybinding | Description |
|---------|-----------|-------------|
| Layout presets | `Super+Space` | MasterStack / Columns / Center / Grid per-workspace |
| Notifications | Auto | Toast overlay with fade-in/out, 3s duration, shadow |
| Window animations | Auto | Layout slide (350ms ease), appear flash (350ms accent pulse) |
| Workspace scroll | Touchpad/Scroll | Infinite scroll with spring physics, adjacent ws visible |
| Scratchpad | `Super+`` | Quake-style dropdown terminal (glow border + shadow) |
| Window rules | config.toml | Auto-assign apps to workspaces by app_id |
| Screenshot | `Super+P` | DRM framebuffer dump → `$HOME/Pictures/Screenshots/` |
| App launcher | `Super+D` | Built-in launcher with search filter, blinking cursor |
| Fullscreen | `Super+F` | Per-window fullscreen |
| Move window | `Super+Shift+1-9` | Move focused window to workspace |
| Close | `Super+Q` | Close focused window |
| XWayland | Auto | X11 app support (Feishu, Chrome, Edge, etc.) |
| Lock screen | `Super+Esc` | 5 random animated styles, PAM password auth, shake on error |
| Task Panel | `Super+Tab` | Real window thumbnails with titles + borders |
| Overview | `Super+A` | Cover Flow 3D with hover pulse + window titles |

## Rendering Pipeline

10-step layered pipeline, all rendered via GlesRenderer (GPU OpenGL ES):

```
Step 1:   Wallpaper (gradient/solid/image texture + 5 glow spots + breathing pulse)
Step 2:   Window content (all elements, single draw call)
Step 2.5: IM popup (Wayland input method)
Step 2.6: Window appear flash (accent pulse on new window, 350ms)
Step 3:   Window decorations (multi-layer glow + gradient borders + corner highlights)
Step 4:   Scratchpad overlay (glow border + shadow + content)
Step 4.5: X11 override-redirect windows (input method popups, tooltips)
Step 4.8: Overview/Task Panel (Cover Flow 3D / real thumbnails + titles + shadows)
Step 5:   Headbar (gradient ws blocks + sliding indicator + pulse clock + CPU/MEM glow)
Step 6:   Notifications (toast overlay with shadow)
Step 7:   App launcher (search with blinking cursor + glow border)
Step 8:   Cursor
Step 9:   Screenshot area selection overlay
Step 10:  Screenshot capture (copy_framebuffer after finish, before drop target)
```

**Key principle**: Window elements are collected in order (0→N) into a single vec,
then `draw_render_elements` draws them all at once. Later windows naturally
overwrite earlier windows' overflow. No slot-based clipping needed.

## GPU Support

Anchor auto-detects the GPU at startup via `/sys/class/drm/card*/device/vendor`:

| Vendor ID | GPU | Notes |
|-----------|-----|-------|
| `0x10de` | NVIDIA | Requires `nvidia-drm.modeset=1` kernel param. Sets `GBM_BACKEND=nvidia-drm` etc. |
| `0x1002` | AMD | Works out of box with Mesa/amdgpu. |
| `0x8086` | Intel | Works out of box with Mesa/i915. |

### GPU Selection Priority

1. `TITAN_GPU` env var (absolute path, e.g. `/dev/dri/card1`)
2. `config.toml` `[gpu].device` field
3. `config.toml` `[gpu].vendor` preference + auto-detect
4. First available `/dev/dri/card*`

### Configuration

```toml
[gpu]
vendor = "auto"    # "auto" | "nvidia" | "amd" | "intel"
device = ""        # "/dev/dri/card1" or empty for auto
```

### NVIDIA-specific quirks

- Only `drmModeAddFB2WithModifiers` works (legacy `drmModeAddFB` unsupported)
- Dumb buffers: Not supported for scanout
- `anchor-session` auto-sets `GBM_BACKEND=nvidia-drm`, `__GLX_VENDOR_LIBRARY_NAME=nvidia`, etc.

## Configuration

Config file: `config.toml` in project root or `~/.config/anchor/config.toml`.

```toml
[colors]
background = "#0d0d1a"
focus_border = "#7aa2f7"
unfocus_border = "#24253a"

[bar]
enabled = true
height = 48

[layout]
border_width = 4
gap = 14
margin = 6
header_bar_height = 0    # 全局 header bar 高度（0=禁用，>0=在每个窗口顶部预留空间）

[terminal]
command = "foot"

[input_method]
method = "fcitx"  # "fcitx", "ibus", or "none"

[gpu]
vendor = "auto"   # "auto" | "nvidia" | "amd" | "intel"
device = ""       # manual DRM device path, empty = auto

[[window_rule]]
app_id = "firefox"
workspace = 1
layout = "master-stack"
```

## Session Lifecycle

`LibSeatSession::new()` returns `(LibSeatSession, LibSeatSessionNotifier)`:
- **Notifier** holds the ONLY strong reference to the libseat connection state.
- **If notifier is dropped, the libseat connection closes immediately.**
- All DRM devices opened via `session.open()` are released. DRM master is lost.

**Correct usage**: Insert notifier into calloop event loop. Wait for
`SessionEvent::ActivateSession` before creating DRM surfaces.

## Deployment

```bash
# 1. Build
cargo build --release --bin anchor

# 2. Restart (kill process, DM restarts it)
kill $(pgrep -f "target/release/anchor")

# 3. Verify
sleep 8 && pgrep -a anchor
```

### Display Manager Setup

Works with GDM, SDDM, or LightDM. Example for GDM:
- Session file: `/usr/share/wayland-sessions/anchor.desktop`
- Session wrapper: `scripts/anchor-session` (auto-detects project dir via `dirname`)
- `anchor-session` runs `target/release/anchor` directly — no `cp` needed

For SDDM: Create `/usr/share/wayland-sessions/anchor.desktop` with same content.

### 🚫 Forbidden Operations

1. **NEVER `systemctl restart gdm3`** (on NVIDIA) — destroys logind session → NVIDIA modes lost → Anchor broken
2. **NEVER `sudo reboot`** — same + disconnects remote access
3. **NEVER add `systemctl --user` to `scripts/anchor-session`** — hangs GDM session startup.
   The systemd user bus may not be ready when the session script runs. All daemon/portal
   spawning MUST be done from Rust code (`main.rs`) after the Wayland socket is bound.
4. **NEVER spawn daemons from `scripts/anchor-session`** — use Rust `Command::spawn()` instead.
   The shell `&` background processes may be killed by GDM's session wrapper.
5. **NEVER modify `scripts/anchor-session` to manage services** — it should ONLY set env vars
   and `exec` the compositor. Service lifecycle belongs in Rust code.

## Key Lessons

1. **`do_layout()` MUST be called after layout changes.** Forgetting this means
   windows don't receive `send_configure` → clients release old buffers → new
   buffers haven't arrived → `render_elements_from_surface_tree` returns empty → black screen.
2. **Keep notifier alive.** Dropping it kills the entire session silently.
3. **No slot-based clipping.** Drawing slot backgrounds between windows creates
   visible seams. Let natural draw order handle overlap.
4. **Rendering order matters.** Windows → Decorations → Scratchpad → Headbar → Notifications → Launcher → Cursor → Screenshot overlay.
5. **Scratchpad must intercept `new_toplevel`.** Use `scratchpad.pending` flag in
   `ScratchpadState` to divert the next toplevel into `scratchpad.surface` instead of workspace tops.
6. **Disable foot CSD.** Set `csd.preferred=none` in `~/.config/foot/foot.ini`.
7. **GPU auto-detect is robust.** Reads vendor ID from sysfs, falls back to first card.
   No NVIDIA-specific code in main path — env vars are only set by `anchor-session`.
8. **X11 windows live in `Workspace.x11_surfaces`**, not in `tops`. ALL focus/click/
   close/layout/decoration code must cover BOTH `ws.tops` AND `ws.x11_surfaces`.
9. **`client_compositor_state` must handle `XWaylandClientData`.** The XWayland
   client uses a different `ClientData` type. Use `if let` fallback, never `.unwrap()`.
10. **XWayland `DISPLAY` must be passed to child processes.** `std::env::set_var`
    works but `/proc/PID/environ` won't show it. Also pass via `cmd.env("DISPLAY", ...)`
    in launcher/terminal spawns for reliability.
11. **X11 OR windows: `mapped_override_redirect_window` must re-add to `or_surfaces`.**
    fcitx5 reuses the same X11 window — unmapped removes it, next map won't trigger
    `new_override_redirect_window`. Without re-adding, the popup disappears permanently.
12. **X11 OR `configure_request` must accept client position (x, y).** OR windows
    (input method popups) control their own position. Ignoring x/y causes fcitx5
    candidate box to snap back to wrong position.
13. **TTC fonts need `collection_index: 0` in `FontSettings`.** fontdue supports TTC
    (TrueType Collection) but requires explicit `collection_index`. NotoSansCJK is TTC.
    Also set `load_substitutions: false` (required field in fontdue 0.9.3).
14. **Lock screen intercepts ALL input.** When `lock_state.locked`, keyboard/pointer events
    are blocked before reaching normal handlers. Lock renders on ALL outputs (full UI
    on focused, dim overlay on others). Password verified via PAM (`auth.rs` FFI),
    managed by `lock::LockState`.
15. **Screenshot must run after `f.finish()` but before `drop(target)`.** The framebuffer
    is only complete after finish, and target must still be alive for `copy_framebuffer`.
    Needs `use smithay::backend::renderer::Renderer` in scope for the trait method.
16. **Fullscreen must be a hard input boundary.** When `ws.fullscreen` is set, `pointer_focus`
    MUST return early at the fullscreen branch and never fall through to non-fullscreen
    hit-test or `ws.focus` fallback. Otherwise, if `ws.focus` still points to a
    non-fullscreen window (because `toggle_fullscreen` didn't update it), pointer events
    will "leak through" the fullscreen window to the underlying window — appearing as
    mouse events reaching the wrong client. Always sync `ws.focus = fullscreen_surface`
    when entering fullscreen (both Wayland `toggle_fullscreen` and X11 `fullscreen_request`).
    The pointer_focus fullscreen branch must `return None` (not fall through) when the
    fullscreen slot is invalid, to prevent the same leak.
17. **Screenshot must use `Fourcc::Abgr8888`, not `Xrgb8888`.** In GlesRenderer,
    `copy_framebuffer` already auto-flips Y (framebuffer Y=0 at bottom → top-down
    RGBA buffer). `Abgr8888` is little-endian = R,G,B,A byte order, matching
    PNG natively — no per-pixel BGR↔RGB swap, no manual row reverse needed.
    Using `Xrgb8888` + manual `(0..h).rev()` row flip + manual BGR→RGB swap
    produces a 180°-rotated image with R/B channels swapped (Xrgb8888's
    byte layout is driver-dependent via `GL_IMPLEMENTATION_COLOR_READ_*`,
    so the BGR→RGB assumption breaks across drivers).
18. **Multi-monitor mouse clamp must use real boundaries.** Clamp range is
    `(ox, ox + ow)` not `(ox, ox + ow - 1)`. The `-1` variant makes the
    pointer unable to ever reach the actual right/bottom edge, and
    across-screen movement becomes erratic because the "nearest output
    center" heuristic can flip-flop when the pointer sits at a screen
    boundary. Always clamp to the real output rectangle in both
    in-bound and out-of-bound cases.
19. **X11 OR windows are root-window absolute coordinates.** `X11Surface::geometry()`
    returns positions in X11 root window space (global). The render pipeline
    uses output-local coordinates. Always subtract `(ox, oy)` of the current
    output before passing to `render_elements_from_surface_tree`. Without
    this, X11 OR popups (fcitx5 candidate box) render at the wrong position
    in multi-monitor setups, and even on single-monitor setups they may
    appear behind fullscreen Wayland windows when the underlying X11
    client has been re-laid-out by a fullscreen toggle.
20. **Screenshot clipboard needs `wl-copy` for X11 paste.** `set_data_device_selection`
    on the compositor side only sets the Wayland `wl_data_device` selection.
    X11 clients (Feishu, Chrome, etc.) paste via X11 `CLIPBOARD` atom +
    `XConvertSelection`. The XWayland bridge has incomplete mime support
    for `image/png` — X11 GTK clients frequently get empty data. The
    reliable fix is to additionally pipe the PNG bytes through `wl-copy
    --type image/png` (external process, async spawn), which uses
    XFixes to set the X11 CLIPBOARD atom directly. wl-copy is
    standard on most Wayland distros (`wl-clipboard` package).
21. **🚫 NEVER spawn external processes (wl-copy, xclip, etc.) in compositor
    callback paths.** A previous fix for X11 clipboard paste spawned
    `wl-copy` from `set_clipboard_png` and the user reported the entire
    desktop froze on every screenshot, requiring reboot. The hang happens
    because: (a) `wl-copy` needs `WAYLAND_DISPLAY` env which anchor-session
    doesn't export, so it retries indefinitely; (b) `stdin.write_all(&png)`
    blocks when the pipe buffer fills while wl-copy is hung;
    (c) `std::thread::spawn(child.wait())` accumulates zombie threads;
    (d) eventually the calloop event loop starves and the compositor
    freezes. **Rule**: compositor code MUST be self-contained. Any
    clipboard interop with X11 must be done through Smithay's own
    selection APIs (`XwmHandler::send_selection`, `X11Wm::set_selection`),
    NEVER via external CLI tools.
22. **电源键物理重启 ≠ session 内重启，处理方式不同。**
    - **session 内重启** (`kill $(pgrep anchor)` 后再启动 anchor)：
      GDM session 没死，D-Bus session bus 仍存活。`scripts/anchor-session`
      中的 `dbus-update-activation-environment` 和 `gnome-keyring-daemon --start`
      可以恢复这些 session 级守护进程。
    - **电源键物理重启**：全新 session，D-Bus session bus 全新启动。
      `gnome-keyring-daemon --start` 启动时**没有解锁密钥**，守护进程
      处于 locked 状态，Secret Service 不可用。
    - 物理重启后找回浏览器登录态的**唯一方法**：
      1. 取消 GDM 自动登录（`/etc/gdm3/custom.conf` 中注释
         `AutomaticLoginEnable` 和 `AutomaticLogin`）
      2. 物理重启后 GDM 显示登录页
      3. 输入密码 → GDM 用密码解锁 GNOME Keyring（密码=keyring 密码）
      4. 浏览器、TLS 证书、密码管理器全部恢复
23. **多显示器 `prev_positions` 必须在 `focused_output` 切换时同步。**
    `prev_positions` 是全局变量（只有当前 `active_ws` 的数据）。当鼠标从
    显示器2 移到显示器1 时，`active_ws` 更新了但 `prev_positions` 还是
    显示器2 的数据。之后新建窗口时 `do_layout_animated` 把已有窗口误判为
    新窗口 → 从屏幕外飞入。**修复**：所有 `focused_output` 切换点都调用
    `self.layout_workspace(self.active_ws)` 同步 `prev_positions`。
24. **`do_layout_animated` 在动画进行中时不能重启动画。** 连续 commit /
    surface 事件会反复调用 `do_layout_animated`，每次都重启 layout 动画 →
    窗口抖动。动画进行中时只执行 `layout_workspace`（更新位置），不重新
    开始动画。
25. **新增窗口 vs 布局切换需要不同的动画策略。** `do_layout_animated` 自动
    检测：纯新增窗口场景（窗口数增加、旧窗口全存在）→ 已有窗口零偏移；
    布局变化场景 → 所有窗口从旧位置动画到新位置。
26. **X11 窗口的 `wl_surface` 是异步关联的。** `map_window_request` 时
    `window.wl_surface()` 通常返回 `None`（Wayland surface 尚未创建）。
    必须在 `surface_associated` 回调中补设 `ws.focus` + `kbd.set_focus` +
    `do_layout_animated()`（触发 ConfigureWindow）。否则 X11 窗口（如飞书
    打开的 MPV）进入 tiling 布局后：没有 focus → 无法交互、Super+Q 关不掉；
    没有 configure → 客户端不渲染 → 窗口透明。`surface_associated` 不能只
    设 `dirty = true`，必须完整处理 focus + layout。

## 渲染循环与动画架构

### ⚠️ dirty 持久化 — 逐帧动画的生命线

Anchor 的渲染循环是**按需驱动**的：只有 `dirty = true` 时才进入渲染管线，渲染完成后 `dirty = false`。

**这意味着任何逐帧动画（弹簧物理、Instant+ease、连续滚动等）必须自己维持 dirty 标记**，否则只有第一帧被渲染，后续帧永远不会被触发。

```rust
// ❌ 错误：动画只有第一帧被渲染，看起来像"延迟几秒后突然出现"
if animation_active {
    animation.update(dt);
    dirty = true;  // ← 渲染前设 dirty，渲染后被 dirty=false 覆盖，没有后续帧
}

// ✅ 正确：在 dirty=false 之后重新设 dirty，触发下一帧（与 ws_anim/layout_anim 同模式）
// 渲染管线末尾：
state.dirty = false;
// 动画持续渲染守卫：
if state.ws_anim_active() { state.dirty = true; }
if state.layout_anim.is_active() { state.dirty = true; }
if state.scroll_spring_active() { state.dirty = true; }  // ← 弹簧动画必须加这个！
if state.overview.is_active() { state.dirty = true; }   // ← overview 动画必须加这个！
```

**已有的参考模式**：`ws_anim`（4131行）和 `layout_anim`（4140行）的 dirty 持久化。

**经验教训**：2026-06-02 实现 Overview/Task Panel 和弹簧滚动时，遗漏了 dirty 持久化。
弹簧物理引擎和 Instant+ease 动画被误认为"有 bug"（延迟、卡顿、方向反），实际原因是：
- 弹簧每帧被 `update(dt)` 推进，但只有第一帧渲染到屏幕
- 等到某个不相关事件（surface damage）偶然触发渲染时，动画已跳到末态 → 看起来像"突然出现"
- 连续快速切换时看到的是中间状态的单帧 → 看起来像"方向反了"

### 动画模式选择

| 模式 | 适用场景 | 优点 | 缺点 |
|------|---------|------|------|
| `Instant + ease_out_cubic` | 一次性过渡（ws切换、布局动画） | 确定性、无状态、零抖动 | 不适合连续交互 |
| 弹簧物理（`Spring`） | 连续交互（滚动吸附、手势跟随） | 物理感、可中断、惯性 | 需要精确的 dirty 持久化 |

**选择原则**：如果动画是"从 A 到 B 的一次性过渡"用 Instant+ease；如果动画需要"持续响应输入/有惯性/可中断"用弹簧。

### Header Bar 协议 (anchor-header-bar-v1)

Anchor 实现了混合装饰模式，支持三种方式：

1. **SSD（Server-Side Decoration）**：默认模式，合成器绘制边框、按钮、编号标签
2. **CSD（Client-Side Decoration）**：客户端请求 `Mode::ClientSide`，合成器只画最小边框高亮
3. **Header Bar 协议**：客户端通过 `anchor-header-bar-v1` 协议声明 header bar 高度，在窗口顶部预留空间

**配置**：
```toml
[layout]
header_bar_height = 0  # 全局默认，0=禁用
```

**协议流程**：
1. 客户端 `get_header_bar` 创建 `anchor_header_bar_v1` 对象
2. 客户端 `set_height(N)` 声明 header bar 高度
3. 合成器 `configured(N)` 确认高度
4. 客户端在 toplevel 表面顶部 N 像素内渲染 header bar 内容（标题、按钮、标签等）
5. 合成器装饰渲染时跳过该窗口的标题/按钮，只画边框和 header bar 分隔线

**关键代码**：
- 协议定义：`protocols/anchor-header-bar-v1.xml`
- 协议实现：`src/headerbar.rs`（辅助函数）+ `src/main.rs`（Dispatch handler）
- 装饰渲染：`src/layout/decorations.rs`（`is_csd` 和 `header_bar_h` 参数）
- 配置：`src/config.rs`（`Layout.header_bar_height`）

### XDG Activation 协议 (xdg-activation-v1)

支持跨应用焦点激活。典型场景：
- 浏览器点击链接 → 打开/聚焦外部应用（邮件客户端、Zoom 等）
- VS Code 点击 URL → 切换到已打开的浏览器窗口

**关键代码**：
- App struct: `xdg_activation: XdgActivationState`
- Handler: `impl XdgActivationHandler for App`（`src/main.rs:6095-6162`）
- delegate: `delegate_xdg_activation!(App)`（`src/main.rs:5393`）
- 初始化: `XdgActivationState::new::<App>(&dh)`（`src/main.rs:3237`）

**工作流**：
1. 应用 A（如浏览器）创建 `XdgActivationToken`
2. Token 通过环境变量 `XDG_ACTIVATION_TOKEN` 传递给应用 B
3. 应用 B 通过 `xdg_activation_v1.activate` 提交 token
4. 合成器调用 `request_activation` → 查找目标 surface → 切换工作区 + 设置焦点
5. 超过 30 秒的旧 token 自动忽略

**已知限制**：Anchor 的 launcher 启动应用时尚未自动生成 activation token。
未来需要在 `src/launcher.rs` 的 `spawn()` 中集成 token 创建和传递。
