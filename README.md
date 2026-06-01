<div align="center">

# ⚓ Anchor

**A Wayland tiling compositor — entirely crafted by AI, zero human-written code.**

[![Rust](https://img.shields.io/badge/Rust-1.94+-orange?logo=rust)](https://www.rust-lang.org/)
[![Smithay](https://img.shields.io/badge/Smithay-0.7-blue)](https://github.com/Smithay/smithay)
[![GPU](https://img.shields.io/badge/GPU-NVIDIA%20%7C%20AMD%20%7C%20Intel-green)]()
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

[Features](#features) · [Quick Start](#quick-start) · [Configuration](#configuration) · [Architecture](#architecture)

</div>

---

> 🤖 **This entire project — every line of Rust, every algorithm, every bug fix — was written by [xbot](https://github.com/ai-pivot/xbot), an autonomous AI coding agent.**
>
> No human has written, edited, or even reviewed a single line of source code in this repository. From the initial `cargo init` to the 7,000+ lines of compositor logic, from GPU auto-detection to the lock screen's PAM authentication — it's all AI.
>
> **100% AI. 0% human code.** This is what an AI agent can build when given a real systems programming challenge.

---

## Features

- 🪟 **4 Layout Presets** — MasterStack, Columns, Center, Grid. Per-workspace layout. `Super+Space` to cycle.
- 🖥️ **9 Workspaces** — `Super+1` through `Super+9`. Move windows with `Super+Shift+1-9`.
- 📟 **Scratchpad Terminal** — `Super+`` toggles a quake-style dropdown terminal overlay.
- 🚀 **Built-in App Launcher** — `Super+D` opens a searchable app menu from XDG `.desktop` files.
- 🔒 **Lock Screen** — `Super+Esc` locks with 5 randomized animated styles. PAM password authentication.
- 📸 **Screenshot** — `Super+P` captures via GPU framebuffer, area selection overlay, clipboard support.
- 🔔 **Notifications** — DBus toast overlay (`org.freedesktop.Notifications`) with fade animations.
- 🎨 **Wallpapers** — Gradient, solid color, or image. Cycle with `Super+W`.
- 🖼️ **Fullscreen** — `Super+F` per-window fullscreen.
- ⌨️ **XWayland** — Full X11 app support (Feishu, Chrome, Edge, etc.). Input method popups work.
- 📐 **Window Rules** — Auto-assign apps to workspaces and layouts via `config.toml`.
- 🎯 **Cross-GPU** — Auto-detects NVIDIA / AMD / Intel, sets appropriate environment variables.
- ✨ **Window Animations** — Smooth workspace switch slide (200ms ease-out cubic).
- 🔤 **Input Method** — `fcitx5` / `ibus` integration. Configurable via config.

## Requirements

| Requirement | Details |
|---|---|
| OS | Linux with DRM/KMS |
| GPU | NVIDIA proprietary / AMD / Intel |
| Display Manager | GDM, SDDM, or LightDM (logind-backed) |
| NVIDIA | `nvidia-drm.modeset=1` kernel parameter required |

### Runtime Dependencies

- `libseat` / logind for session management
- OpenGL ES 3.x (via GPU driver)
- A terminal emulator (default: `foot`)
- Input method (optional): `fcitx5` or `ibus`
- `libpam` for lock screen authentication

## Quick Start

### 1. Build

```bash
cargo build --release --bin anchor
```

### 2. Install Session

```bash
sudo cp scripts/anchor.desktop /usr/share/wayland-sessions/
```

The `anchor-session` wrapper auto-detects:
- **GPU type** (NVIDIA/AMD/Intel) and sets appropriate env vars
- **Project directory** via `dirname` — no hardcoded paths
- **Input method** from `config.toml` (fcitx/ibus/none)

### 3. Configure (Optional)

Copy `config.toml` to `~/.config/anchor/config.toml` or edit in project root.

### 4. Restart After Changes

```bash
kill $(pgrep -f "target/release/anchor")
# DM will auto-restart the session
```

## Configuration

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
vendor = "auto"    # "auto" | "nvidia" | "amd" | "intel"
device = ""        # "/dev/dri/card1" or empty for auto

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
| <code>Super+\`</code> | Toggle scratchpad |
| `Super+P` | Screenshot |
| `Super+W` | Next wallpaper |
| `Super+Esc` | Lock screen |
| `Super+1-9` | Switch workspace |
| `Super+Shift+1-9` | Move window to workspace |
| `Super+Shift+Esc` | Quit |

## Architecture

### Rendering Pipeline

10-step layered pipeline, rendered via GlesRenderer (GPU OpenGL ES):

```
Step 1:   Wallpaper         (gradient / solid / image texture)
Step 2:   Window content    (all elements, single draw call)
Step 2.5: IM popup          (Wayland input method)
Step 3:   Window decorations (border lines, focused/unfocused)
Step 4:   Scratchpad        (opaque overlay + border + content)
Step 4.5: X11 OR windows    (input method popups, tooltips)
Step 5:   Headbar           (workspace indicators, clock, date, CPU/MEM)
Step 6:   Notifications     (toast overlay)
Step 7:   App launcher      (search + list)
Step 8:   Cursor
Step 9:   Screenshot area selection overlay
Step 10:  Screenshot capture (framebuffer copy)
```

### Project Structure

```
src/
  main.rs           — Compositor loop, App struct, keyboard/mouse handling, GPU auto-detect
  workspace.rs      — Workspace struct, WindowSlot enum, unified render order
  lock.rs           — Lock screen state machine (PAM auth, shake animation, random styles)
  launcher.rs       — Built-in app launcher (XDG .desktop scanning, search filter)
  scratchpad.rs     — Quake-style dropdown terminal state machine
  xwayland.rs       — XWayland support (X11 app compatibility)
  config.rs         — TOML config parsing (includes GPU config section)
  text_render.rs    — fontdue-based TTF text rendering
  auth.rs           — PAM authentication via FFI
  wallpaper.rs      — Wallpaper loading and caching
  cursor.rs         — XCursor theme loading and rendering
  notify.rs         — DBus notification listener
  screenshot.rs     — Screenshot capture (area selection, DRM framebuffer dump)
  block_linear.rs   — Block-linear memory layout helpers (NVIDIA)
  layout/
    geom.rs         — Layout geometry (presets, split direction, slot calculation)
    decorations.rs  — Window border decorations
    headbar.rs      — Top bar (workspace indicators, clock, date, CPU/MEM)
    wallpaper.rs    — Wallpaper rendering (gradient grid, animated glow spots)
    notifications.rs — Toast notification rendering
    launcher.rs     — Launcher overlay rendering
    lock_screen.rs  — Lock screen rendering (5 animated styles + dim overlay)
scripts/
  anchor-session    — DM session wrapper (auto-detects GPU, sets env vars)
  anchor-launcher   — App launcher script (fallback)
```

### GPU Support

| Vendor ID | GPU | Notes |
|-----------|-----|-------|
| `0x10de` | NVIDIA | Requires `nvidia-drm.modeset=1`. Auto-sets GBM backend env vars. |
| `0x1002` | AMD | Works out of box with Mesa/amdgpu. |
| `0x8086` | Intel | Works out of box with Mesa/i915. |

GPU selection priority: `TITAN_GPU` env var → `config.toml` device → vendor preference + auto-detect → first available card.

## Known Limitations

- **No layer-shell** — External overlays (wmenu, waybar) won't work. Built-in alternatives provided.
- **No xdg-decoration protocol** — CSD must be disabled in terminal config (e.g., `foot.ini`).
- **NVIDIA block-linear** — Minor pixel distortion at sub-pixel scales (AMD/Intel unaffected).

---

## 🤖 AI Attribution

<div align="center">

**This project is a demonstration of autonomous AI software engineering.**

Every line of code in Anchor was written by **[xbot](https://github.com/ai-pivot/xbot)** — an open-source AI coding agent.

| Metric | Value |
|--------|-------|
| Total lines of Rust | ~7,000+ |
| Human-written lines | **0** |
| Source files | 20+ |
| Features shipped | 15+ |
| GPU vendors supported | 3 (NVIDIA, AMD, Intel) |

The entire development history — architecture decisions, bug fixes, NVIDIA driver quirks, multi-monitor edge cases — lives in the commit log. No human guidance beyond high-level feature requests.

**Built with ❤️ by [xbot](https://github.com/ai-pivot/xbot) — the AI that ships real systems software.**

</div>

---

## License

MIT
