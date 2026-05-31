# Anchor — AGENTS.md

> Wayland tiling compositor in Rust, using Smithay 0.7.
> Cross-GPU: NVIDIA proprietary / AMD / Intel. Any display manager (GDM/SDDM/LightDM).

## Architecture

- **Language**: Rust
- **Compositor framework**: Smithay 0.7
- **GPU**: Auto-detected (NVIDIA / AMD / Intel via `/sys/class/drm/*/device/vendor`)
- **Renderer**: Pixman (CPU-only; works on all GPUs including NVIDIA proprietary without GBM/EGL)
- **Session**: libseat (logind backend) — works with GDM, SDDM, LightDM

## Project Structure

```
src/
  main.rs        — Main compositor loop, App struct, Workspace, keyboard/mouse handling, GPU auto-detect
  xwayland.rs    — XWayland support (X11 app compatibility: spawn, surface tracking, helpers)
  layout.rs      — Layout engine (slot calculation, headbar, decorations, wallpaper)
  text_render.rs — fontdue-based TTF text rendering
  config.rs      — TOML config parsing (includes GPU config section)
scripts/
  anchor-session   — DM session wrapper (auto-detects GPU, sets env vars conditionally)
  drm-dump-fb.c   — DRM framebuffer dump tool (compiled separately)
  anchor-launcher  — App launcher script (dmenu/wmenu based)
config.toml       — User configuration
Cargo.toml        — Dependencies (fontdue, smithay, pixman, etc.)
```

## Features (v28)

| Feature | Keybinding | Description |
|---------|-----------|-------------|
| Layout presets | `Super+Space` | MasterStack / Columns / Center / Grid per-workspace |
| Notifications | Auto | Toast overlay with fade-in/out, 3s duration |
| Window animations | Auto | Workspace switch slide (200ms ease-out cubic) |
| Scratchpad | `Super+`` | Quake-style dropdown terminal (floating overlay) |
| Window rules | config.toml | Auto-assign apps to workspaces by app_id |
| Screenshot | `Super+P` | DRM framebuffer dump → `$HOME/Pictures/Screenshots/` |
| App launcher | `Super+D` | Built-in launcher with search filter |
| Fullscreen | `Super+F` | Per-window fullscreen |
| Move window | `Super+Shift+1-9` | Move focused window to workspace |
| Close | `Super+Q` | Close focused window |
| XWayland | Auto | X11 app support (Feishu, Chrome, Edge, etc.) |

## Rendering Pipeline

8-step layered pipeline, all rendered via Pixman CPU renderer:

```
Step 1: Wallpaper (gradient/solid)
Step 2: Window content (all elements, single draw call)
Step 3: Window decorations (border lines)
Step 4: Scratchpad overlay (opaque background + border + content)
Step 5: Headbar (workspace indicators, clock, date, CPU/MEM)
Step 6: Notifications (toast overlay)
Step 7: App launcher (search + list)
Step 8: Cursor (solid triangle, block-linear safe)
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
- Pixman renders in linear layout, NVIDIA scanout expects block-linear.
  Renders correctly at macro scale (window-sized regions) but small pixels may distort.
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

## Key Lessons

1. **`do_layout()` MUST be called after layout changes.** Forgetting this means
   windows don't receive `send_configure` → clients release old buffers → new
   buffers haven't arrived → `render_elements_from_surface_tree` returns empty → black screen.
2. **Keep notifier alive.** Dropping it kills the entire session silently.
3. **No slot-based clipping.** Drawing slot backgrounds between windows creates
   visible seams. Let natural draw order handle overlap.
4. **Rendering order matters.** Windows → Decorations → Scratchpad → Headbar → Notifications → Launcher → Cursor.
5. **Scratchpad must intercept `new_toplevel`.** Use a `scratchpad_pending` flag to
   divert the next toplevel into `scratchpad_surface` instead of workspace tops.
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
