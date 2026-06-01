# Plan: 智能布局 + 全方向动画 + 启动器毛玻璃

## Summary
三大改动：(1) 启动器区域毛玻璃；(2) 新窗口根据 split 方向智能插入到当前窗口旁边；(3) Super+Shift+方向键按屏幕真实几何位置交换窗口。所有布局变化都有多方向动画（从旧位置→新位置），删除窗口时旧位置正确推断。

## Changes

### 1. `src/layout/launcher.rs` — 启动器毛玻璃
- **What**: 移除面板自身的 `f.clear(背景)` 。只保留发光边框 + 搜索框背景 + 文字。面板区域完全透明 → 桌面透过来 = 毛玻璃。
- **Why**: `f.clear()` = `glClear()` 完全替换像素。无法做 alpha 混合。唯一的方法是不清除面板区域。

### 2. `src/workspace.rs` — 智能窗口插入
- **What**: 添加 `insert_after_focus(split: SplitDir)` 方法。当 `pending_split` 设置时，新窗口插入到 focused 窗口旁边（Vertical=下方，Horizontal=右方），而不是总是追加到末尾。
- **Why**: 目前新窗口总是追加到 `window_order` 末尾。用户期望 `Super+V` 后新窗口出现在当前窗口下方。

### 3. `src/layout/geom.rs` — Grid 布局不使用
- **What**: 无需修改 geom.rs。slot() 函数是纯几何计算，已经正确。关键是 `window_order` 的排列顺序。

### 4. `src/main.rs` — 方向感知的窗口交换
- **What**: 重写 `swap_window(direction)`: 计算所有窗口的 slot 位置，找到当前窗口在指定方向上最近的邻居窗口，交换它们在 `window_order` 中的位置。
- **Why**: 目前 Super+Shift+Left 总是 swap(fi, fi-1)，不关心实际屏幕位置。

### 5. `src/main.rs` — 全方向删除动画
- **What**: `do_layout_animated()` 中，为 prev_slots 里存在但新 layout 中不存在的窗口记录"消失方向"（它们的最后位置）。同时为新增窗口从智能方向滑入（根据 split 方向决定从右/下方滑入）。
- **Why**: 目前只有从底部滑入。删除窗口时没有退出动画。

### 6. `src/main.rs` — 布局切换动画
- **What**: `Super+Space` 布局切换改用 `do_layout_animated()` 替换当前的 `do_layout()`。

## Risks
- **窗口插入逻辑**: `insert_after_focus` 需要正确处理边界情况（空工作区、无焦点窗口）
- **方向感知交换**: 需要计算所有 slot 几何位置，O(n) 但 n 很小（<20），性能无问题
- **毛玻璃效果**: 由于 GlesRenderer 没有 blur 支持，只能是"桌面透过可见"的半透明效果

## Definition of Done
- [ ] 启动器打开时面板区域可见桌面内容（毛玻璃）
- [ ] Super+V → 新窗口出现在焦点窗口下方
- [ ] Super+B → 新窗口出现在焦点窗口右方
- [ ] Super+Shift+方向键 → 按屏幕真实位置交换
- [ ] 新增窗口有滑入动画（方向由 split 决定）
- [ ] 删除窗口时剩余窗口平滑收拢
- [ ] Super+Space 布局切换有动画
- [ ] `cargo build --release` 无错误

## Open Questions
- 无
