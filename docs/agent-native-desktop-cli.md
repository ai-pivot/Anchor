# Agent Native Desktop CLI 使用说明

## Socket

Anchor 启动后会创建：

- `$XDG_RUNTIME_DIR/anchor/ctl.sock`

协议是 **JSON line**：每行一条 JSON 请求/响应/事件。

## 请求格式

```json
{"id":1,"method":"system.status","params":{}}
```

成功响应：

```json
{"id":1,"result":{"active_workspace":0}}
```

失败响应：

```json
{"id":1,"error":{"code":"command_error","message":"..."}}
```

事件推送：

```json
{"method":"window.focused","params":{"workspace":0,"title":"Firefox","app_id":"firefox","kind":"wayland"}}
```

## 已支持方法

### 查询
- `system.status`（推荐给 agent，完整桌面快照）
- `system.state`（精简运行时状态摘要）
- `output.list`
- `workspace.list`
- `window.list`（支持 `app_id` / `title` / `workspace` 过滤）
- `window.focused`
- `config.get`

### 窗口选择/控制
- `window.focus`（至少指定 `app_id/title/workspace` 之一）
- `window.move_to_workspace`（支持按 `app_id/title/source_workspace` 选择目标）
- `window.fullscreen.toggle`（至少指定 `app_id/title/workspace` 之一）
- `window.close`（至少指定 `app_id/title/workspace` 之一）

### 控制
- `workspace.switch`
- `workspace.next`
- `workspace.prev`
- `window.move_to_workspace`
- `layout.set`
- `layout.cycle`
- `scratchpad.toggle`
- `overview.show`
- `overview.hide`
- `settings.show`
- `settings.hide`
- `lock.activate`
- `config.reload`
- `window.fullscreen.toggle`
- `window.close`
- `notify.send`
- `screenshot.capture`
- `record.start`
- `record.stop`
- `app.launch`
- `event.subscribe`

## 事件类型

- `workspace.changed`
- `layout.changed`
- `window.focused`
- `window.opened`
- `window.closed`
- `window.moved`
- `window.fullscreen.changed`
- `scratchpad.changed`
- `overview.changed`
- `settings.changed`
- `output.focused.changed`
- `lock.changed`
- `screenshot.completed`
- `screenshot.failed`
- `record.changed`
- `app.launched`
- `config.reloaded`

## wait/filter

部分命令支持：

- `--wait`：等待相关事件
- `--wait-for <event-name>`：仅接受指定事件名
- `--match key=value`：仅接受 payload 中匹配该字段的事件，可重复传入

例如：

```bash
anchorctl app launch firefox --wait --wait-for window.focused --match app_id=firefox
anchorctl screenshot full --wait --wait-for screenshot.completed
```

## 索引语义

- CLI 输入的 workspace 序号使用 **1-based**（例如 `workspace switch 3`）
- IPC/JSON 输出中的 `workspace` / `index` 字段使用 **0-based**
- 对通用 agent，推荐优先消费 JSON，再在调用 CLI 时显式做 `+1` 转换

## anchorctl 示例

```bash
anchorctl status --json
anchorctl workspace switch 3
anchorctl layout set center
anchorctl window list --app-id firefox --json
anchorctl window close --app-id firefox
anchorctl screenshot full
anchorctl app launch firefox
anchorctl events workspace.changed window.focused window.closed
```

说明：
- `anchorctl events ...` 默认按 **NDJSON** 输出，便于 pipeline / agent 消费
- 若只看普通 RPC 查询结果，可继续使用 `--json`

## 面向 Agent 的建议

1. 查询优先用 `--json`
2. 监听长流优先用 `anchorctl events ...`
3. 截图、录屏、启动应用等能力建议结合事件流确认状态
4. `lock.activate` 允许；不提供 unlock API

## 安全边界

- 仅本地 Unix socket
- socket 权限 `0600`
- 服务端校验 `SO_PEERCRED`，只接受同 UID 客户端
- 不提供任意 shell 执行接口
