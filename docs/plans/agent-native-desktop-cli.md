# Agent Native Desktop CLI 方案设计

> 目标：把 Anchor 变成“agent 原生桌面”。任何 AI agent、脚本或终端工具，都可以通过一个统一的 `anchorctl` CLI 读写桌面状态、操作窗口/工作区/截图/设置，并订阅桌面事件流。

## 1. Done 定义

本方案完成的标志不是“先写很多代码”，而是明确可验证的交付物：

1. `anchor` 主进程对外暴露一个稳定的运行时控制面。
2. 新增 `anchorctl` CLI，可以直接发命令控制桌面。
3. 控制面支持两类能力：
   - **命令型**：切工作区、切布局、锁屏、切 scratchpad、截图、开关 overview / settings。
   - **查询/订阅型**：查询当前工作区、窗口、布局、GPU、录制/锁屏状态；订阅焦点/工作区/窗口变化事件。
4. 安全边界明确：默认只允许同 UID、本地会话进程访问；高风险能力单独授权或禁用。
5. 方案与 Anchor 当前单线程 calloop + Smithay 架构兼容，不引入会冻结桌面的阻塞路径。

---

## 2. 为什么这是“agent 原生桌面”

传统桌面只对人类暴露快捷键和 GUI；agent 只能“模拟键鼠”。

Agent 原生桌面应提供：

- **语义化命令**：`anchorctl workspace switch 3`，而不是模拟 `Super+3`
- **结构化查询**：`anchorctl status --json`
- **事件流**：Agent 可以订阅 `workspace_changed` / `window_focused`
- **可组合操作**：脚本与 agent 可串联复杂工作流
- **安全可控**：不是把 compositor 当 root shell，而是暴露受限桌面 API

这让桌面从“只能人工操作的 UI”变成“可编排的操作系统前端”。

---

## 3. 现有能力盘点（可直接 CLI 化）

Anchor 目前已经具备大量可控能力，只是入口都在快捷键逻辑里：

### 3.1 工作区 / 布局 / 窗口

- 切换工作区 `App::switch_workspace(target)`
- 相邻工作区切换 `App::switch_workspace_direction(dir)`
- 移动窗口到工作区 `App::move_window_to_workspace(target)`
- 切换布局 preset
- 焦点方向切换 / 窗口交换
- 关闭窗口
- 全屏切换

### 3.2 桌面功能

- 启动终端
- 打开/关闭 Launcher
- 切换 Scratchpad
- 打开/关闭 Overview / Task Panel
- 打开/关闭 Settings Panel
- 锁屏
- 触发截图 / 区域截图
- 开始/停止录屏
- 热重载配置
- 合成器内部通知 toast

### 3.3 可查询状态

- 当前工作区、每个 workspace 布局
- 焦点窗口、窗口列表（Wayland / X11）
- 锁屏状态
- 录屏状态
- GPU 设备与 vendor
- 输出布局（多显示器）
- 当前配置快照

结论：**Anchor 并不缺功能，缺的是一个统一的、可编程的控制面。**

---

## 4. 总体架构

推荐架构：**Compositor 内置 Unix Domain Socket 控制面 + `anchorctl` CLI + 事件订阅流**。

```text
anchorctl / AI agent / shell script
          │
          │ JSON-RPC over Unix socket
          ▼
/run/user/$UID/anchor/ctl.sock
          │
          ▼
Anchor Command Dispatcher (calloop source)
          │
          ├─ 写操作：直接调用 App 方法 / 修改状态
          ├─ 查询：读取 App 当前状态并序列化
          └─ 异步操作：登记 request_id，等后续帧或后台状态回传结果
```

### 4.1 为什么选 Unix Socket，而不是纯 DBus / 纯 Wayland 协议

#### 选它的原因

1. **和 calloop 天然兼容**：可以作为 event source 插进现有主循环。
2. **本地低延迟**：适合 CLI 高频小命令。
3. **结构灵活**：JSON/JSON-RPC 易调试、易跨语言接入。
4. **适合 agent**：Python/Rust/Shell 都能立刻用。
5. **不依赖 session bus 健康度**：避免 DBus 挂了桌面就失控。

#### 不选 DBus 作为主通道的原因

- DBus 更适合系统服务与标准桌面协议，不适合作为高频 CLI 主命令总线。
- 需要额外 interface 设计和线程桥接。
- 对 agent 来说调试体验差于 Unix socket 文本协议。

#### 不选 Wayland 协议作为第一阶段的原因

- CLI 工具会被迫成为 Wayland client，增加接入复杂度。
- SSH/非图形会话中不好远程调度。

**结论：第一阶段主通道用 Unix socket；第二阶段可选补一个 DBus façade。**

---

## 5. 控制面分层设计

### 5.1 三层模型

#### Layer A — Runtime Command Bus

负责接收外部请求、解析协议、路由到具体 handler。

建议新增模块：

- `src/ipc/mod.rs`
- `src/ipc/protocol.rs`
- `src/ipc/server.rs`
- `src/ipc/handlers.rs`
- `src/ipc/events.rs`

#### Layer B — App Control API

把现在散落在快捷键处理里的逻辑，抽成可复用的“语义动作”。

例如：

- `appctl::switch_workspace(&mut App, index)`
- `appctl::set_layout(&mut App, preset)`
- `appctl::toggle_scratchpad(&mut App)`
- `appctl::show_overview(&mut App, mode)`
- `appctl::request_screenshot(&mut App, req)`
- `appctl::reload_config(&mut App)`

这样做的关键价值：

- 键盘快捷键继续能用
- CLI / agent 也能复用同一套逻辑
- 避免“一套快捷键逻辑 + 一套 CLI 逻辑”双份漂移

#### Layer C — Event Stream

把桌面状态变化变成结构化事件：

- `workspace.changed`
- `window.focused`
- `window.opened`
- `window.closed`
- `layout.changed`
- `lock.changed`
- `record.changed`
- `screenshot.completed`
- `output.focused`
- `config.reloaded`

Agent 真正“原生”的核心，不只是能发命令，而是**桌面自己会说话**。

---

## 6. 协议设计

### 6.1 请求/响应模型

采用轻量 JSON-RPC 风格：

```json
{"id":1,"method":"workspace.switch","params":{"index":2}}
```

成功：

```json
{"id":1,"result":{"ok":true,"active_workspace":2}}
```

失败：

```json
{"id":1,"error":{"code":"invalid_workspace","message":"workspace 12 out of range"}}
```

服务端事件推送：

```json
{"method":"event.workspace.changed","params":{"from":1,"to":2}}
```

### 6.2 命令分类

#### A. 查询类

- `system.status`
- `workspace.list`
- `workspace.current`
- `window.list`
- `window.focused`
- `output.list`
- `config.get`
- `gpu.info`
- `lock.status`
- `record.status`

#### B. 控制类

- `workspace.switch`
- `workspace.next`
- `workspace.prev`
- `window.move_to_workspace`
- `layout.set`
- `layout.cycle`
- `window.focus`
- `window.swap`
- `window.close`
- `window.fullscreen.toggle`
- `launcher.toggle`
- `scratchpad.toggle`
- `overview.show`
- `overview.hide`
- `settings.show`
- `settings.hide`
- `lock.activate`
- `config.reload`
- `notify.send`

#### C. 异步类

- `screenshot.capture`
- `record.start`
- `record.stop`
- `app.launch`

#### D. 订阅类

- `event.subscribe`
- `event.unsubscribe`

---

## 7. CLI 设计

CLI 名称建议：`anchorctl`

### 7.1 示例命令

```bash
# 查询
anchorctl status
anchorctl status --json
anchorctl workspace list
anchorctl window list --json
anchorctl gpu info

# 控制工作区/布局
anchorctl workspace switch 3
anchorctl workspace next
anchorctl layout set center
anchorctl layout cycle

# 窗口控制
anchorctl window focus --app-id firefox
anchorctl window close --focused
anchorctl window move --focused --workspace 5
anchorctl window fullscreen toggle

# 桌面功能
anchorctl scratchpad toggle
anchorctl overview show
anchorctl settings show
anchorctl lock
anchorctl config reload
anchorctl notify send "构建完成"

# 截图/录屏
anchorctl screenshot full
anchorctl screenshot area --x 100 --y 100 --w 800 --h 600
anchorctl record start
anchorctl record stop

# 事件流
anchorctl events subscribe workspace.changed window.focused
```

### 7.2 JSON 输出约定

- 默认给人看：简洁文本
- `--json` 给 agent / 脚本看：稳定字段，不做花哨格式化

示例：

```json
{
  "active_workspace": 2,
  "layout": "master-stack",
  "focused_window": {
    "kind": "xwayland",
    "app_id": "firefox",
    "title": "Docs"
  },
  "locked": false,
  "recording": false,
  "gpu": {
    "vendor": "NVIDIA",
    "device": "/dev/dri/card1"
  }
}
```

### 7.3 Agent 友好设计

增加两个专门模式：

```bash
anchorctl exec '<json-request>'
anchorctl events subscribe --ndjson
```

原因：

- 大模型 agent 往往更适合拼 JSON，而不是处理人类 CLI 解析细节。
- NDJSON 流方便 Python、Node、Shell 管道消费。

---

## 8. 与 Anchor 当前架构的耦合点

### 8.1 必须新增的核心抽象

#### 抽象 1：`DesktopAction`

把快捷键触发动作抽象成统一动作枚举。

```rust
pub enum DesktopAction {
    SwitchWorkspace { index: usize },
    SetLayout { preset: LayoutPreset },
    ToggleScratchpad,
    ShowOverview,
    HideOverview,
    ShowSettings,
    HideSettings,
    Lock,
    ReloadConfig,
    Notify { text: String },
    RequestScreenshot { mode: ScreenshotMode },
}
```

快捷键处理和 CLI handler 都只负责“翻译输入”，最后统一调用：

```rust
fn dispatch_action(app: &mut App, action: DesktopAction) -> ActionResult
```

#### 抽象 2：`DesktopQuery`

统一状态读取出口，避免 CLI 为了查状态到处摸 `App` 内部字段。

#### 抽象 3：`DesktopEvent`

把原本隐含在状态变化中的东西显式化。

### 8.2 对现有代码最小侵入的接入点

1. `main.rs` 启动阶段：初始化 IPC server，并注册到 calloop。
2. 键盘处理路径：把已有逻辑搬到 `dispatch_action`。
3. 状态变化点：发出 event。
4. 截图 / 录屏完成点：回填异步请求结果。

这意味着 **第一阶段不需要大规模重写渲染或布局系统**。

---

## 9. 安全模型

Agent 原生 ≠ 无限制控制。

### 9.1 默认允许

- 查询状态
- 切工作区/布局
- 聚焦/关闭窗口
- 打开 overview / settings / scratchpad
- 发桌面通知
- 请求截图
- 热重载配置

### 9.2 默认拒绝或额外授权

#### 禁止 1：CLI 解锁桌面

锁屏解锁绝不能被 CLI/API 暴露。

原因：

- 会绕开物理交互信任边界
- `lock.rs` 当前模型围绕 PAM + 本地键盘输入设计
- 给 agent 提供“解锁 API”本质上就是安全后门

**结论：只允许 `lock.activate`，不允许 `lock.unlock`。**

#### 限制 2：任意命令执行

不应提供 `exec shell command in compositor` 这种万能接口。

正确做法：

- 只暴露受控 `app.launch`
- 仅允许从 launcher 已索引的 `.desktop` 应用中选择
- 可选支持 `terminal.command` 这种现有白名单式入口

#### 限制 3：配置写入

`config.set` 第一阶段建议只改运行时内存态，或者只改有限字段。

写回磁盘前应：

1. 校验 key 是否允许
2. 校验值类型
3. 生成 TOML 预览
4. 原子写盘 + `.bak` 备份

### 9.3 认证方案

第一阶段：

- socket 路径：`$XDG_RUNTIME_DIR/anchor/ctl.sock`
- 权限：`0600`
- 校验 `SO_PEERCRED`，只允许同 UID

第二阶段：

- 可选增加 `anchor agent token`
- 某些高权限命令要求 token（如录屏、批量截图、配置写盘）

### 9.4 审计日志

建议对所有外部命令记日志：

```text
[anchorctl] pid=12345 uid=1000 method=workspace.switch params={index:3}
```

这对排查“是用户按键还是 agent 发命令造成的状态变化”很重要。

---

## 10. 异步任务设计

### 10.1 为什么要有异步模型

有些操作无法同步立即返回：

- 截图：要等下一帧渲染时 copy framebuffer
- 录屏：启动/停止涉及后台 ffmpeg 管线
- 启动应用：spawn 成功不代表窗口已经 map

### 10.2 统一任务模型

```json
{"id":7,"method":"screenshot.capture","params":{"mode":"full"}}
```

立即返回：

```json
{"id":7,"result":{"accepted":true,"task_id":"shot-20260714-001"}}
```

完成事件：

```json
{"method":"event.task.completed","params":{
  "task_id":"shot-20260714-001",
  "kind":"screenshot",
  "output":{"path":"/home/user/Pictures/Screenshots/2026-07-14_20-10-00.png"}
}}
```

失败事件：

```json
{"method":"event.task.failed","params":{
  "task_id":"shot-20260714-001",
  "kind":"screenshot",
  "error":"copy_framebuffer failed"
}}
```

### 10.3 好处

- CLI 可同步等待，也可 fire-and-forget
- Agent 容易做工作流编排
- 所有异步能力统一抽象，不会每个功能一套特殊语义

---

## 11. 推荐实施路径

### Phase 0 — 重构内核动作层（先不暴露外部接口）

目标：把快捷键逻辑抽成可复用动作。

工作项：

1. 提炼 `DesktopAction` / `dispatch_action`
2. 提炼 `DesktopQuery`
3. 在关键状态变化点埋 `DesktopEvent`
4. 补一层最小单元测试（协议解析/动作分发）

**验收标准**：
- 快捷键路径仍正常
- `dispatch_action` 能从测试里直接调用

### Phase 1 — 内置 Unix Socket RPC Server

目标：让 compositor 首次拥有正式的运行时控制面。

工作项：

1. 新增 `src/ipc/*`
2. 启动时创建 `ctl.sock`
3. 实现最小命令集：
   - `system.status`
   - `workspace.switch/list`
   - `layout.set/cycle`
   - `scratchpad.toggle`
   - `overview.show/hide`
   - `settings.show/hide`
   - `lock.activate`
   - `config.reload`
   - `notify.send`
4. 支持 `event.subscribe`

**验收标准**：
- `printf ... | socat - $XDG_RUNTIME_DIR/anchor/ctl.sock` 可直接控制桌面
- 不引入主循环卡顿

### Phase 2 — `anchorctl` CLI

目标：给人和 agent 一个可用的人机接口。

工作项：

1. 新增二进制 `src/bin/anchorctl.rs`
2. 使用 `clap` 或轻量手写 parser
3. 支持 `--json` / `--ndjson`
4. 对异步任务支持 `--wait`

**验收标准**：
- `anchorctl status --json` 可稳定输出
- `anchorctl workspace switch 4` 可立即生效

### Phase 3 — 高阶桌面能力接入

1. `window.list/focus/close/move`
2. `screenshot.capture`
3. `record.start/stop/status`
4. `app.launch`
5. `config.get/set`

**验收标准**：
- 能支撑基础 agent workflow：观察 → 决策 → 操作 → 验证

### Phase 4 — Agent 工作流增强

1. 事件订阅增强：窗口 map/unmap、焦点、输出切换
2. 宏命令 / 场景模式
3. 可选 DBus facade
4. 可选“策略引擎”：限制某类 agent 只能调用某些命令

---

## 12. MVP 建议（最小可行实现）

如果目标是最快把桌面变成可用的 agent 平台，我建议 MVP 只做这 8 个命令：

1. `system.status`
2. `workspace.switch`
3. `workspace.list`
4. `layout.set`
5. `scratchpad.toggle`
6. `overview.show`
7. `lock.activate`
8. `config.reload`

再加 3 个事件：

1. `workspace.changed`
2. `window.focused`
3. `lock.changed`

为什么这样选：

- 覆盖“观察 + 操作”的闭环
- 风险低，不碰最复杂的截图/录屏异步路径
- 足够让 agent 先接管桌面导航与任务切换

---

## 13. 关键风险与规避

### 风险 1：把阻塞 I/O 放进主循环

**规避**：
- socket 全部 nonblocking
- 单请求大小限制
- 重型任务只登记 request，不在回调里等待

### 风险 2：CLI 路径和快捷键路径分叉

**规避**：
- 所有操作都统一走 `dispatch_action`
- 快捷键只是 action 的一个 frontend
- CLI 只是另一个 frontend

### 风险 3：截图/录屏语义不一致

**规避**：
- 统一异步 task_id 模型
- 一律事件回传完成/失败

### 风险 4：过度开放导致安全事故

**规避**：
- 不暴露 unlock
- 不暴露任意 shell exec
- 默认只允许同 UID + 本地 socket
- 高风险命令单独 capability 开关

### 风险 5：未来扩展越来越乱

**规避**：
- 从第一天开始就把协议、handler、事件拆模块
- 别把 JSON 解析和 App 改状态代码都塞回 `main.rs`

---

## 14. 推荐代码落点

### 14.1 新增模块

```text
src/
  ipc/
    mod.rs
    protocol.rs      # request/response/event schema
    server.rs        # unix listener/client management
    handlers.rs      # method -> App action/query
    events.rs        # event emission helpers
    auth.rs          # peer credential checks
  appctl.rs          # DesktopAction / DesktopQuery / dispatch_action
src/bin/
  anchorctl.rs       # CLI client
```

### 14.2 现有模块修改点

- `src/main.rs`
  - 初始化 IPC server
  - 在状态变化点发出事件
  - 把快捷键逻辑部分迁到 `appctl.rs`

- `src/screenshot.rs`
  - 增加异步任务结果回传 hook

- `src/record.rs`
  - 增加状态查询/任务回传接口

- `src/settings/mod.rs`
  - 暴露更清晰的 show/hide/apply/reset API

---

## 15. 对外能力模型（Agent Capability Model）

为了让将来的 agent 权限清晰，建议把命令分 capability：

- `read.desktop`
- `control.workspace`
- `control.window`
- `control.overlay`
- `control.lock`
- `capture.screen`
- `record.screen`
- `config.runtime`
- `config.persist`

未来即使接入多 agent，也能给不同 agent 发不同 token / policy。

---

## 16. 结论

### 我给你的结论很明确：

**最佳方案不是让 agent 去“模拟键鼠”，而是在 Anchor 内部建立一个正式的桌面控制面：Unix Socket RPC + `anchorctl` + 事件流。**

这套方案的优点是：

- 完全契合 Anchor 当前单线程 calloop 架构
- 改造成本可控
- 对 agent 极友好
- 安全边界清晰
- 后续可以自然扩展为 DBus façade、策略授权、宏命令系统

### 最推荐的落地顺序：

1. 先抽 `dispatch_action` / `DesktopQuery`
2. 再做 compositor 内的 socket server
3. 然后做 `anchorctl`
4. 最后补截图/录屏/窗口精细控制和事件流增强

如果你认可，我下一步可以继续直接进入**实现阶段**，先给你落 MVP：

- compositor 内置 `ctl.sock`
- `anchorctl` CLI
- `status / workspace switch / layout set / scratchpad toggle / lock / config reload`
- 事件订阅基础版
