# Titan — AGENTS.md

> Wayland tiling compositor in Rust, using Smithay 0.7, targeting NVIDIA 5080D (proprietary driver).

## Architecture

- **Language**: Rust
- **Compositor framework**: Smithay 0.7
- **GPU**: NVIDIA 5080D (proprietary, `nvidia-drm.modeset=1`)
- **Renderer**: Pixman (CPU-only; NVIDIA proprietary doesn't support GBM/EGL GLES)
- **Session**: libseat (logind backend) via GDM
- **Monitor**: DP-4 on card1 (connector 128), 2560×1440@59Hz

## Project Structure

```
src/main.rs              — Main compositor (~300 lines)
scripts/titan-session    — GDM session wrapper
Cargo.toml               — Dependencies
/usr/share/wayland-sessions/titan.desktop — GDM login entry
```

## Critical: Smithay Source Patches (NOT in git)

The following files in the cargo registry are patched. They survive `cargo build` but NOT `cargo clean` — must re-apply after clean.

### 1. `~/.cargo/registry/src/index.crates.io-*/smithay-0.7.0/src/backend/allocator/gbm.rs`

- **Line ~227**: `implicit=true` fallback when GBM `create_buffer_object_with_modifiers2` fails and modifiers contain `Invalid` or `Linear`. Original is `false`. NVIDIA GBM returns a block-linear modifier that causes `addfb2` to fail; setting `implicit=true` forces the buffer to report `Modifier::Invalid` instead.

### 2. `~/.cargo/registry/src/index.crates.io-*/smithay-0.7.0/src/backend/renderer/pixman/mod.rs`

- **`import_dmabuf`**: Relaxed to accept any modifier (not just Linear). NVIDIA GBM creates buffers with modifier `0x300000000606014` (block-linear) which Pixman can't import with strict Linear-only check.
- **`dmabuf_formats`**: Returns both `Linear` and `Invalid` modifiers for each supported format. Without `Invalid`, the intersection with NVIDIA plane formats (which report `{Invalid, 0x3fffff}`) is empty, causing `NoSupportedRendererFormat`.

### 3. `~/.cargo/registry/src/index.crates.io-*/smithay-0.7.0/src/backend/drm/surface/gbm.rs`

- **`test_state` error handling**: Currently STOCK (restored). Do NOT patch this to ignore errors — `test_state` failure means real permission or format issues.

### 4. `~/.cargo/registry/src/index.crates.io-*/smithay-0.7.0/src/backend/drm/device/fd.rs`

- **STOCK.** Do NOT modify. The `acquire_master_lock()` EACCES is expected on logind sessions.

## Critical: Session Lifecycle (THE ROOT CAUSE OF MANY BUGS)

### The LibSeatSession + Notifier Pattern

`LibSeatSession::new()` returns `(LibSeatSession, LibSeatSessionNotifier)`:

- **`LibSeatSessionNotifier`** holds `Rc<LibSeatSessionImpl>` — the **ONLY strong reference** to the libseat connection state.
- **`LibSeatSession`** holds only `Weak<LibSeatSessionImpl>`.
- **If `notifier` is dropped, the libseat connection closes immediately.** All DRM devices opened via `session.open()` are released. DRM master is lost. All subsequent atomic commits return `EACCES / Permission denied (os error 13)`.

**Correct usage (as in anvil and all Smithay examples):**

1. Keep `notifier` alive for the entire lifetime of the compositor.
2. Insert `notifier` into the calloop event loop via `eloop.handle().insert_source(notifier, ...)`.
3. Wait for `SessionEvent::ActivateSession` before creating DRM surfaces.
4. Handle `SessionEvent::PauseSession` / `SessionEvent::ActivateSession` for VT switching.

**Pattern:**
```rust
let (session, notifier) = LibSeatSession::new()?;
let fd = session.open(&gpu_path, OFlags::RDWR)?;
// ... create DrmDevice, surfaces, etc. ...

// CRITICAL: insert notifier into event loop, keep it alive
eloop.handle().insert_source(
    Generic::new(notifier, Interest::READ, Mode::Level),
    |_, _, state| { /* handle ActivateSession/PauseSession */ }
)?;
```

### NVIDIA DRM Quirks

- **`drmModeAddFB` (legacy)**: Not supported. Only `drmModeAddFB2WithModifiers` works.
- **GBM buffer modifier**: `0x300000000606014` (NVIDIA block-linear tiling).
- **Plane formats**: Reports `{Invalid, 0x3fffff}` modifiers, not Linear.
- **`drmSetMaster`**: Returns EACCES on logind sessions (expected, logind manages master via fd passing). Do NOT add manual `drmSetMaster` calls — it triggers kernel `Failed to grab modeset ownership` errors.
- **Modes via ioctl**: Reports 0 modes through standard ioctl; must use EDID to get mode info.
- **Dumb buffers**: Not supported for scanout.

## Known Issues

### Black Screen (UNRESOLVED)

Pixman renders pixels in **linear** layout. NVIDIA scanout expects **block-linear** (`0x300000000606014`). The rendered data doesn't match the display format, resulting in a black screen despite frames being successfully submitted (verified: 2040 frames rendered in v5/v6).

**Possible fixes (not yet attempted):**
1. GLES renderer via GBM EGL display (EGL init previously hung — may work in GDM session context).
2. CPU-side block-linear tiling before submit.
3. Force GBM to create Linear buffers (NVIDIA GBM rejects explicit Linear modifier).

## Critical: Deployment & Testing (READ THIS FIRST)

### 正确的部署方法

`titan-session` 脚本直接运行 `./target/release/titan`。所以：

```bash
# 1. 编译 — cargo build 完就自动是新版本，不需要 cp 任何东西
cargo build --release --bin titan

# 2. 杀 Titan 进程让 GDM 重启 session（同一个 logind session，DRM modes 不丢失）
kill $(pgrep -f "target/release/titan")

# 3. 等待 GDM 重新启动 Titan（约 5-8 秒）
sleep 8 && pgrep -a titan

# 4. 截图验证
sudo scripts/drm-dump-fb /dev/dri/card1 /tmp/titan-current.raw
```

### 🚫 绝对禁止的操作

1. **禁止 `sudo systemctl restart gdm3`** — 这会销毁当前 logind session，NVIDIA DRM modes 丢失，新 session 里 `drmModeGetConnector` 返回 0 modes，Titan 永远启动失败。
2. **禁止 `sudo reboot`** — 同上，而且还会断开 xbot 连接。
3. **禁止 `sudo cp target/release/titan /usr/local/bin/titan`** — 没有用。`titan-session` 直接运行 `target/release/titan`，不需要复制到其他路径。
4. **禁止尝试从 EDID/sysfs 构造 DRM Mode** — NVIDIA modes 问题只能在已有 session 内解决。如果 session 丢失了 modes，只能通过完整的系统启动流程恢复。
5. **禁止修改 `titan-session` 添加 `--direct`** — `--direct` 模式在 GDM logind session 中无法工作。

### 如果 Titan 确实无法启动（session 彻底坏了）

只有在这种情况下才能 `systemctl restart gdm3`，但要知道重启后 Titan 可能因 NVIDIA 0-modes 问题无法自动登录。此时只能等完整的系统 reboot（从 GRUB 开始），GDM greeter 在首次启动时会正确设置 modes。

## Build & Run

```bash
cd .
cargo build --release --bin titan
# After cargo clean: re-apply all Smithay patches listed above!
```

**From GDM**: Select "Titan" from the session menu. `titan-session` wrapper sets NVIDIA env vars and launches the binary.

**Important**: Before testing from GDM, ensure no stale sessions hold DRM master:
```bash
loginctl list-sessions   # Should only show GDM greeter session
# If stale sessions exist: loginctl terminate-session <id>
```

## Git Conventions

- Always `git commit` before changes.
- Smithay patches are in cargo registry, NOT tracked by git.
- After `cargo clean`, patches must be re-applied manually.

## Key Lessons (Hard-Won)

1. **Read type definitions before using APIs.** The `Rc`/`Weak` ownership in `LibSeatSession`/`Notifier` is the single most important thing to understand.
2. **Keep notifier alive.** Dropping it kills the entire session silently.
3. **Don't fight NVIDIA on `drmSetMaster`.** Logind manages master; manual calls break things.
4. **`test_state` failure is a real error.** Don't patch it to be non-fatal — it indicates actual problems (like a dead session).
5. **Symptoms ≠ root cause.** "Permission denied" on atomic commit means the fd lost master, not that you need to call `drmSetMaster` harder.
6. **部署 = `cargo build` + `kill titan`。** 不需要 cp、不需要 restart gdm、不需要 reboot。`titan-session` 直接运行 `target/release/titan`，build 完就是新版本。
7. **绝对不要 `systemctl restart gdm3`。** 会销毁 logind session → NVIDIA modes 丢失 → Titan 无法启动 → 只能完整 reboot 恢复。正确的做法是 `kill` Titan 进程让 GDM 在同一个 session 里重启它。
