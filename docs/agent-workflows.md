# Anchor Agent Workflows

> 目标：给 AI agent / shell automation 一套可直接复制的 Anchor 桌面工作流。

## 1. 查询当前桌面状态

```bash
anchorctl status --json
anchorctl workspace list --json
anchorctl window list --json
anchorctl window focused --json
```

建议：
- 查询一律优先 `--json`
- 如果要进入长流消费，事件推荐 `--ndjson`

---

## 2. 启动应用并等待确认

```bash
anchorctl app launch firefox --wait
anchorctl app launch firefox --wait --wait-for window.focused --match app_id=firefox
```

如果需要显式订阅：

```bash
anchorctl events app.launched --ndjson
```

---

## 3. 切工作区并观察焦点变化

```bash
anchorctl workspace switch 3
anchorctl events workspace.changed window.focused --ndjson
```

---

## 4. 截图并等待完成

全屏截图：

```bash
anchorctl screenshot full --wait
```

区域截图：

```bash
anchorctl screenshot area 100 100 800 600 --wait
```

事件流模式：

```bash
anchorctl events screenshot.completed screenshot.failed --ndjson
```

---

## 5. 典型 agent 闭环：观察 → 操作 → 等待 → 验证

### 场景：打开 Firefox，切到第 3 个工作区，截图留档

```bash
anchorctl app launch firefox --wait
anchorctl workspace switch 3
anchorctl screenshot full --wait
anchorctl status --json
```

### 场景：关闭或移动指定窗口

```bash
anchorctl window list --workspace 2 --json
anchorctl window focus --workspace 2 --title Terminal
anchorctl window close --workspace 2 --title Terminal
anchorctl window move 3 --workspace 2 --title Terminal
```

---

## 6. 用 raw JSON 直接驱动

```bash
anchorctl exec '{"id":1,"method":"system.status","params":{}}'
anchorctl exec '{"id":2,"method":"layout.set","params":{"preset":"center"}}'
```

---

## 7. 推荐事件订阅组合

### 桌面导航 agent

```bash
anchorctl events workspace.changed layout.changed window.focused --ndjson
```

### 截图/录屏 agent

```bash
anchorctl events screenshot.completed screenshot.failed record.changed --ndjson
```

### 应用调度 agent

```bash
anchorctl events app.launched window.opened window.focused window.closed --ndjson
```

---

## 8. 实战建议

1. 查询命令用 `--json`
2. 事件长流用 `--ndjson`
3. `--wait` 适合短闭环自动化
4. 对异步操作，优先以事件为真相来源
5. 不要依赖 unlock；Anchor 只暴露 `lock.activate`
