# Titan — AGENTS.md

> Wayland tiling compositor in Rust, using Smithay 0.7.
> Targets NVIDIA proprietary driver.

## Architecture

- **Language**: Rust
- **Compositor framework**: Smithay 0.7
- **GPU**: NVIDIA proprietary (`nvidia-drm.modeset=1`)
- **Renderer**: Pixman (CPU-only; NVIDIA proprietary doesn't support GBM/EGL GLES)
- **Session**: libseat (logind backend) via GDM

## Project Structure

```
src/
  main.rs        — Main compositor loop, App struct, Workspace, keyboard/mouse handling
  layout.rs      — Layout engine (slot calculation, headbar, decorations, wallpaper)
  text_render.rs — fontdue-based TTF text rendering
  config.rs      — TOML config parsing
scripts/
  titan-session   — GDM session wrapper (auto-detects project dir)
  drm-dump-fb.c   — DRM framebuffer dump tool (compiled separately)
  titan-launcher  — App launcher script (dmenu/wmenu based)
config.toml       — User configuration
Cargo.toml        — Dependencies (fontdue, smithay, pixman, etc.)
```

## Features (v27)

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

## Configuration

Config file: `config.toml` in project root or `~/.config/titan/config.toml`.

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

## NVIDIA DRM Quirks

- Only `drmModeAddFB2WithModifiers` works (legacy `drmModeAddFB` unsupported)
- Dumb buffers: Not supported for scanout
- Pixman renders in linear layout, NVIDIA scanout expects block-linear.
  Renders correctly at macro scale (window-sized regions) but small pixels may distort.

## Deployment

```bash
# 1. Build
cargo build --release --bin titan

# 2. Restart (kill process, GDM restarts it)
kill $(pgrep -f "target/release/titan")

# 3. Verify
sleep 8 && pgrep -a titan
```

### GDM Setup
- Session file: `/usr/share/wayland-sessions/titan.desktop`
- Session wrapper: `scripts/titan-session` (auto-detects project dir via `dirname`)
- `titan-session` runs `target/release/titan` directly — no `cp` needed

### 🚫 Forbidden Operations

1. **NEVER `systemctl restart gdm3`** — destroys logind session → NVIDIA modes lost → Titan broken
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
