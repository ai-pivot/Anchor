# Titan

A Wayland tiling compositor built with [Smithay](https://github.com/Smithay/smithay) 0.7 and Rust.

Designed for NVIDIA proprietary driver environments where GBM/EGL GLES renderers don't work. Uses Pixman CPU rendering as a reliable fallback.

## Features

- **4 Layout Presets** — MasterStack, Columns, Center, Grid. Per-workspace layout. `Super+Space` to cycle.
- **9 Workspaces** — `Super+1` through `Super+9`. Move windows with `Super+Shift+1-9`.
- **Scratchpad Terminal** — `Super+`` toggles a quake-style dropdown terminal overlay.
- **Built-in App Launcher** — `Super+D` opens a searchable app menu from `.desktop` files.
- **Window Rules** — Auto-assign apps to workspaces via `config.toml`.
- **Notifications** — Toast overlay triggered on layout/workspace/scratchpad changes.
- **Screenshot** — `Super+P` dumps the DRM framebuffer.
- **Fullscreen** — `Super+F` per-window fullscreen.
- **Configurable** — Colors, gaps, borders, headbar, wallpaper via TOML config.
- **fontdue TTF Rendering** — Clean text rendering that survives NVIDIA block-linear scanout.

## Requirements

- Linux with DRM/KMS
- NVIDIA proprietary driver with `nvidia-drm.modeset=1`
- GDM (or any logind-backed display manager)
- [Smithay 0.7 patches](#smithay-patches) applied to cargo registry

### Runtime Dependencies

- `libseat` / logind for session management
- Pixman (linked via Smithay)
- A terminal emulator (default: `foot`)
- Input method (optional): `fcitx5` or `ibus`

## Quick Start

### 1. Build

```bash
cargo build --release --bin titan
```

### 2. Apply Smithay Patches

Required for NVIDIA proprietary driver. See [Smithay Patches](#smithay-patches) section.

### 3. Install GDM Session

```bash
sudo cp scripts/titan.desktop /usr/share/wayland-sessions/
```

The `titan-session` wrapper auto-detects the project directory and sets required
environment variables (NVIDIA GBM, input method, Wayland hints).

### 4. Configure (Optional)

Copy `config.toml` to `~/.config/titan/config.toml` or edit in project root.

## Configuration

```toml
[colors]
background = "#0d0d1a"
focus_border = "#7aa2f7"
unfocus_border = "#24253a"
bar_background = "#06060e"

[bar]
enabled = true
height = 48

[layout]
border_width = 4
gap = 14
margin = 6

[terminal]
command = "foot"

# Window rules: auto-assign apps to workspaces
[[window_rule]]
app_id = "firefox"
workspace = 1
layout = "master-stack"

[[window_rule]]
app_id = "code"
workspace = 2
layout = "columns"
```

## Keybindings

| Key | Action |
|-----|--------|
| `Super+Return` | Open terminal |
| `Super+Q` | Close window |
| `Super+D` | App launcher |
| `Super+F` | Fullscreen |
| `Super+Space` | Cycle layout |
| `Super+`` | Toggle scratchpad |
| `Super+P` | Screenshot |
| `Super+1-9` | Switch workspace |
| `Super+Shift+1-9` | Move window to workspace |
| `Super+W` | Next wallpaper |
| `Super+Shift+Esc` | Quit |

## Smithay Patches

Two files in the cargo registry must be patched for NVIDIA support:

### `backend/allocator/gbm.rs` (line ~227)

Set `implicit=true` fallback when GBM buffer creation fails with `Invalid` or `Linear`
modifiers. NVIDIA GBM returns a block-linear modifier that causes `addfb2` to fail.

### `backend/renderer/pixman/mod.rs`

- `import_dmabuf`: Accept any modifier (not just Linear)
- `dmabuf_formats`: Return `[Linear, Invalid]` for each format

These patches survive `cargo build` but NOT `cargo clean`.

## Architecture

```
Rendering Pipeline (8 steps, single Pixman frame):
  1. Wallpaper          (gradient/solid)
  2. Window content     (single draw call, all windows)
  3. Window decorations (border lines)
  4. Scratchpad         (opaque overlay)
  5. Headbar            (workspace indicators, clock, date)
  6. Notifications      (toast overlay)
  7. App launcher       (search + list)
  8. Cursor             (solid triangle)
```

Windows are collected in order into a single element vector. Later windows'
draw commands naturally overwrite earlier windows' overflow — no clipping needed.

## Project Structure

```
src/
  main.rs        — Compositor loop, App/Workspace structs, input handling
  layout.rs      — Layout engine, slot(), headbar, decorations, wallpaper
  text_render.rs — fontdue TTF text rendering
  config.rs      — TOML config parsing
scripts/
  titan-session          — GDM session wrapper
  drm-dump-fb.c          — DRM framebuffer dump tool
  sendkey.c              — uinput keyboard event injector (for testing)
  titan-launcher         — External launcher fallback
```

## Known Limitations

- **NVIDIA block-linear scanout**: Pixman renders linear pixels, NVIDIA scans block-linear.
  This causes minor pixel distortion at small scales. Window-sized features render correctly.
- **No GPU acceleration**: Pixman is CPU-only. No EGL/GLES support on NVIDIA proprietary.
- **No layer-shell**: External overlays (wmenu, waybar) won't work. Built-in alternatives provided.
- **No xdg-decoration protocol**: CSD must be disabled in terminal config (e.g., `foot.ini`).

## License

MIT
