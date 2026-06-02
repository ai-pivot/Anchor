# 计划：Anchor Overview 三大特性 — 无限滚动 · 任务面板 · 鸟瞰视图

> 生成时间：2026-06-02 17:14 CST
> 状态：待确认
> 灵感来源：niri compositor（但不完全复刻，追求更 fancy 的视觉体验）

## 背景与目标

用户希望 Anchor 支持 niri 风格的三个核心交互特性：

1. **无限滚动工作区** — 工作区不再是离散编号切换，而是连续滚动的无限条带，带物理惯性和弹性吸附
2. **任务面板（Task Panel）** — 底部/侧边弹出的窗口缩略图面板，快速浏览和切换当前工作区所有窗口
3. **鸟瞰视图（Overview）** — 一键拉远视角，同时俯瞰所有工作区的缩略图，点击即可跳转

**设计原则**：
- 动画流畅，物理感强（弹簧/惯性/弹性）
- 视觉 fancy，有深度感和层次感（视差、毛玻璃、3D 透视）
- 性能优先，GPU 密集操作用纹理缓存
- 渐进式实施，每阶段可独立验证

---

## 现状分析

### 关键文件
| 文件 | 职责 | 修改类型 |
|------|------|----------|
| `src/workspace.rs` | Workspace 结构体、WindowSlot、NUM_WORKSPACES=9 | 修改 |
| `src/main.rs` | App 结构体、工作区切换、动画、渲染管线、输入处理 | 大量修改 |
| `src/layout/geom.rs` | 布局几何计算 slot() | 修改（支持缩略图布局） |
| `src/layout/headbar.rs` | 顶部栏渲染（工作区指示器） | 修改（动态指示器） |
| `src/layout/util.rs` | 共享工具函数 | 修改（新增缓动/物理函数） |
| `src/layout/decorations.rs` | 窗口边框装饰 | 修改 |
| `src/config.rs` | TOML 配置解析 | 修改（新增配置项） |
| `src/physics.rs` | **新增** — 物理引擎（弹簧、惯性、阻尼） | 新增 |
| `src/overview.rs` | **新增** — Overview 状态机（鸟瞰视图 + 任务面板） | 新增 |
| `src/layout/thumbnail.rs` | **新增** — 缩略图渲染（自定义 RenderElement） | 新增 |
| `src/layout/overview.rs` | **新增** — Overview overlay 渲染 | 新增 |

### 核心架构约束

1. **窗口隐藏策略** — 非活跃工作区窗口被缩到 `1×1`（`main.rs:968`），buffer 被客户端释放。缩略图功能必须解决这个问题。
2. **渲染管线** — 10 步分层，新的 overlay 层需插入到合适位置（Step 4.5 之后，Headbar 之前）。
3. **动画无框架** — 每种动画独立实现，需建立通用动画基础设施。
4. **无 alpha blending** — `Frame::clear()` 不支持半透明，需用纹理中间层或颜色调制模拟。
5. **GlesRenderer 无变换矩阵** — 所有变换需手动计算像素坐标。

### 依赖关系
```
physics.rs (新增)
  └── main.rs (滚动偏移驱动渲染)
      └── layout/overview.rs (Overview 渲染)
          └── layout/thumbnail.rs (缩略图渲染)
              └── workspace.rs (窗口数据)
```

---

## 详细计划

### 阶段零：基础设施 — 物理引擎 & 动画框架

> 目标：建立通用的物理模拟和动画基础设施，为三个特性提供底层支持。

- [ ] **0.1** 新建 `src/physics.rs` — 弹簧-阻尼物理引擎
  - `Spring::new(stiffness, damping)` — 弹簧参数化构造
  - `Spring::update(&mut self, target: f64, dt: f64) -> f64` — 帧率无关的物理步进
  - `Spring::velocity()` — 当前速度（用于判断是否停止）
  - `Spring::is_settled(threshold: f64)` — 弹簧是否静止
  - `Momentum::new(friction: f64)` — 惯性滚动
  - `Momentum::apply_impulse(&mut self, velocity: f64)` — 施加冲量
  - `Momentum::update(&mut self, dt: f64) -> f64` — 摩擦衰减 + 位移积分
  - `snap_to_nearest(current: f64, spacing: f64) -> f64` — 吸附到最近整数倍
  - 涉及文件：`src/physics.rs`（新增）

- [ ] **0.2** 在 `src/layout/util.rs` 新增缓动函数集合
  - `ease_out_cubic(t: f32) -> f32` — 已有（抽取公共）
  - `ease_out_expo(t: f32) -> f32` — 快速减速
  - `ease_in_out_cubic(t: f32) -> f32` — 平滑过渡
  - `ease_out_back(t: f32) -> f32` — 回弹过冲
  - `spring_interp(t: f32) -> f32` — 弹簧插值（用于面板弹出）
  - `ease_out_quart(t: f32) -> f32` — 更柔和的减速
  - 涉及文件：`src/layout/util.rs`（修改）

- [ ] **0.3** 在 `src/main.rs` App 结构体新增 `last_frame_time: Instant` 字段
  - 渲染回调中每帧记录时间，计算 `dt = last_frame.elapsed`
  - 为物理引擎提供帧率无关的时间步长
  - 涉及文件：`src/main.rs`（修改 App 结构体 + 渲染循环）

---

### 阶段一：无限滚动工作区

> 目标：工作区从离散编号切换变为连续滚动条带，支持触摸板手势和键盘方向键，带物理惯性和弹性吸附。

- [ ] **1.1** 将工作区坐标从离散索引改为连续浮点坐标
  - `App` 新增 `scroll_offset: f64` — 当前滚动偏移量（单位：工作区宽度倍数）
  - `scroll_offset = 0.0` 表示工作区 0 居中，`scroll_offset = 1.0` 表示工作区 1 居中
  - `scroll_velocity: f64` — 当前滚动速度
  - `scroll_spring: Spring` — 吸附弹簧（stiffness=300, damping=30，提供 ~200ms 的弹性吸附）
  - `is_scrolling: bool` — 是否正在滚动（拦截手势/键盘输入）
  - 涉及文件：`src/main.rs`（修改 App 结构体）

- [ ] **1.2** 修改渲染管线的 `ws_offset` 计算
  - 当前：`ws_offset = dir * ow * (1.0 - ease_out_cubic(t))`（离散切换）
  - 新：`ws_offset = (scroll_offset - active_ws as f64) * ow as f64`
  - 同时渲染 **相邻 ±1 个工作区** 的窗口（用于滑动过渡期间可见）
  - 修改 Phase 1 的 surface element 收集逻辑，遍历 `active_ws - 1`、`active_ws`、`active_ws + 1`
  - 涉及文件：`src/main.rs`（修改渲染循环 ~main.rs:3196-3350）

- [ ] **1.3** 修改窗口隐藏策略 — 支持多工作区同时可见
  - 当前 `switch_workspace()` 将旧工作区窗口缩到 `(1,1)`
  - 新策略：**不立即隐藏**，改为维护一个 `visible_range: Range<usize>`（当前 ±1）
  - 只在 `scroll_offset` 变化导致工作区离开 `visible_range` 时才缩小到 `(1,1)`
  - 工作区进入 `visible_range` 时恢复真实尺寸 + `send_configure()`
  - 新增 `update_workspace_visibility()` 方法统一管理
  - 涉及文件：`src/main.rs`（修改 switch_workspace、新增 update_workspace_visibility）

- [ ] **1.4** 实现物理滚动更新循环
  - 每帧在渲染前调用：
    ```
    dt = last_frame.elapsed
    // 惯性衰减
    scroll_velocity *= friction.powf(dt * 60.0)
    scroll_offset += scroll_velocity * dt
    // 弹簧吸附
    let nearest_ws = scroll_offset.round() as i32;
    scroll_offset = scroll_spring.update(nearest_ws as f64, dt);
    // dirty 标记
    if !scroll_spring.is_settled(0.001) || scroll_velocity.abs() > 0.01 {
        dirty = true
    }
    ```
  - 涉及文件：`src/main.rs`（修改渲染回调）

- [ ] **1.5** 触摸板手势驱动
  - 在 `handle_input_event()` 新增 `GestureSwipeBegin/Update/End` 分支
  - 3 指水平滑动 → 工作区滚动
    - `Begin`：记录起始 scroll_offset，开始手势模式
    - `Update`：`scroll_offset += delta_x / screen_width`（归一化为工作区宽度）
    - `End`：根据剩余速度施加惯性冲量 `scroll_velocity = accumulated_velocity`
  - 4 指上滑 → 进入 Overview（阶段三实现，此处预留接口）
  - 涉及文件：`src/main.rs`（修改 handle_input_event ~main.rs:1914）

- [ ] **1.6** 键盘方向键支持
  - `Super+Left` / `Super+Right` → 切换到相邻工作区（带弹簧吸附动画）
  - 不改变 `scroll_offset` 直接设为目标值，让弹簧动画自然过渡
  - 涉及文件：`src/main.rs`（修改键盘事件处理 ~main.rs:1515-1549）

- [ ] **1.7** Headbar 动态工作区指示器
  - 当前：固定渲染 9 个方块
  - 新：根据 `scroll_offset` 做视口偏移
    - 只渲染可见范围内的工作区指示器（当前 ±2）
    - 指示器大小随距离中心远近变化（中心最大，边缘缩小）— **视差深度效果**
    - 活跃指示器带弹性缩放动画（按下时放大，释放回弹）
  - 涉及文件：`src/layout/headbar.rs`（修改 render_headbar）

- [ ] **1.8** 配置项
  - `[scroll]` section：
    - `enabled = true` — 是否启用无限滚动
    - `workspace_count = 9` — 工作区数量（替代硬编码 NUM_WORKSPACES）
    - `swipe_threshold = 0.15` — 手势触发阈值（工作区宽度的比例）
    - `spring_stiffness = 300.0` — 弹簧刚度
    - `spring_damping = 30.0` — 弹簧阻尼
    - `friction = 0.92` — 惯性摩擦系数
  - 涉及文件：`src/config.rs`、`config.toml`

---

### 阶段二：任务面板（Task Panel）

> 目标：底部弹出式面板，显示当前工作区所有窗口的缩略图，支持点击切换焦点和拖拽排序。

- [ ] **2.1** 新建 `src/overview.rs` — Overview 状态机
  - `OverviewState` 枚举：
    - `Inactive` — 正常桌面
    - `TaskPanel { progress: f32 }` — 任务面板（progress 0→1 = 弹出，1→0 = 收回）
    - `Overview { progress: f32, selected_ws: Option<usize> }` — 鸟瞰视图
  - `App` 新增 `overview: OverviewState` 字段
  - 触发方式：`Super+Tab`（任务面板）、`Super+Down`（鸟瞰视图）、4 指下滑（任务面板）
  - 涉及文件：`src/overview.rs`（新增）

- [ ] **2.2** 新建 `src/layout/thumbnail.rs` — 缩略图渲染引擎
  - `ThumbnailElement` 自定义 `RenderElement<GlesRenderer>`：
    ```rust
    struct ThumbnailElement {
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        thumbnail_dst: Rectangle<i32, Physical>,  // 缩略图目标矩形
        border_color: [u8; 4],
        title: String,
    }
    ```
  - `draw()` 方法：用 `inner` 的纹理 + `thumbnail_dst` 矩形做 GPU 缩放
  - 设置 `renderer.downscale_filter(TextureFilter::Linear)` 获得平滑缩略图
  - 渲染后恢复 `TextureFilter::Nearest`
  - `render_thumbnails()` 函数：为工作区的所有窗口生成缩略图元素
  - 涉及文件：`src/layout/thumbnail.rs`（新增）

- [ ] **2.3** 新建 `src/layout/overview.rs` — Overview overlay 渲染
  - `render_task_panel()` — 任务面板渲染：
    - **背景**：底部 1/3 屏幕的深色半透明遮罩（颜色调制模拟，类似通知的 alpha 技巧）
    - **缩略图网格**：当前工作区窗口的缩略图，等间距排列
    - **窗口标题**：每个缩略图下方显示窗口标题（fontdue 文字渲染）
    - **焦点指示**：当前焦点窗口的缩略图有高亮边框
  - 动画：`progress` 从 0→1 控制面板从底部滑入 + 淡入
    - `panel_y = screen_h * (1.0 - ease_out_back(progress) * 0.35)` — 从底部弹起，带回弹
    - 缩略图依次延迟出现（staggered animation），每个延迟 30ms
  - 涉及文件：`src/layout/overview.rs`（新增）

- [ ] **2.4** 修改渲染管线 — 插入 Overview 层
  - 在 Step 5（Headbar）之前插入 Step 4.8：Overview overlay
  - 当 `overview.active()` 时：
    - 任务面板：渲染面板背景 + 缩略图 + 窗口标题
    - 鸟瞰视图：渲染全屏遮罩 + 所有工作区缩略图网格（阶段三实现）
  - 涉及文件：`src/main.rs`（修改渲染循环 ~main.rs:3700 之前）

- [ ] **2.5** 交互 — 点击切换焦点
  - Overview 激活时，pointer_button 做缩略图命中测试
  - 计算点击坐标落在哪个缩略图区域内
  - 命中 → 切换焦点到该窗口 + 关闭面板（带收起动画）
  - 命中面板外区域 → 关闭面板
  - 涉及文件：`src/main.rs`（修改 pointer_button 处理 + `src/overview.rs` 命中测试逻辑）

- [ ] **2.6** 动画更新循环
  - 每帧更新 `overview.progress`：
    - 任务面板弹出：`progress` 向 1.0 弹簧过渡
    - 任务面板收起：`progress` 向 0.0 弹簧过渡
    - 动画期间 `dirty = true`
  - 涉及文件：`src/main.rs`（渲染回调中新增 overview 动画更新）

---

### 阶段三：鸟瞰视图（Overview）

> 目标：Mission Control 风格的全局视图，同时展示所有工作区的缩略图，带 3D 透视过渡动画。

- [ ] **3.1** 扩展 `OverviewState` — Overview 模式
  - `Overview { progress: f32, selected_ws: Option<usize>, hover_ws: Option<usize> }`
  - `progress` 控制从正常视图到鸟瞰视图的过渡（0=正常，1=完全鸟瞰）
  - 涉及文件：`src/overview.rs`（修改）

- [ ] **3.2** 窗口活跃策略 — 所有工作区保持活跃
  - 这是最大的架构改动
  - 策略：**延迟激活（lazy activation）**
    - 进入 Overview 时：遍历所有工作区，恢复所有窗口的真实尺寸 + `send_configure()`
    - 等待客户端提交新 buffer（约 1-2 帧）
    - 退出 Overview 时：将非活跃工作区的窗口缩回 `(1,1)`
  - 性能保护：限制同时活跃的工作区数量（例如最多 9 个），超出范围的只保留最近 N 个的 buffer
  - GPU 内存评估：9 工作区 × 4 窗口 × 1920×1080×4 ≈ 296MB（可接受）
  - 涉及文件：`src/main.rs`（新增 `activate_all_workspaces()` / `deactivate_non_active_workspaces()`）

- [ ] **3.3** Overview overlay 渲染 — 工作区网格
  - `render_overview()` — 鸟瞰视图渲染：
    - **布局**：3×3 网格，每个工作区占屏幕 1/3 × 1/3，留间距
    - **背景遮罩**：深色半透明（模拟毛玻璃效果）
    - **工作区标签**：每个缩略图上方显示工作区编号 + 布局图标
    - **缩略图**：工作区内所有窗口以 1/3 比例渲染
    - **高亮**：鼠标悬停的工作区有发光边框
    - **当前活跃工作区**：有特殊标记（如金色边框）
  - 涉及文件：`src/layout/overview.rs`（修改）

- [ ] **3.4** 3D 透视过渡动画 — 进入/退出 Overview
  - **进入动画**（0→1，约 400ms）：
    - 所有窗口从原位置缩小到各自工作区的缩略图位置
    - 同时工作区布局背景从全尺寸缩小到 1/3
    - **视差效果**：前景元素（窗口内容）比背景（壁纸）移动更快
    - `scale = 1.0 - progress * 0.667`（1.0 → 0.333）
    - 窗口位置 lerp 到缩略图位置
  - **退出动画**（1→0，约 300ms，ease-out expo）：
    - 从缩略图位置放大回原位置
    - 被选中的工作区放大回全屏（其他工作区淡出）
    - `ease_out_expo` 产生快速展开的感觉
  - 涉及文件：`src/main.rs`（修改渲染循环）+ `src/layout/overview.rs`

- [ ] **3.5** 交互 — 点击跳转工作区
  - Overview 模式下，pointer_button 做工作区缩略图命中测试
  - 命中 → 设置 `selected_ws`，触发退出动画，动画结束后切换到目标工作区
  - 命中遮罩区域 → 取消选中，退出 Overview
  - 涉及文件：`src/main.rs`（修改 pointer_button）+ `src/overview.rs`

- [ ] **3.6** 键盘导航
  - Overview 模式下 `Left/Right/Up/Down` 在网格中移动选中高亮
  - `Enter` 确认选中，`Escape` 取消
  - 涉及文件：`src/main.rs`（修改键盘事件处理）

---

### 阶段四：视觉增强 & 抛光

> 目标：让三个特性的动画更加 fancy，增加视觉层次和深度感。

- [ ] **4.1** 视差深度效果 — 工作区切换时
  - 壁纸层移动速度 = 窗口层 × 0.5（背景更慢 → 远处感）
  - Headbar 层移动速度 = 窗口层 × 1.2（前景更快 → 近处感）
  - 装饰边框随视角偏移产生微妙的 3D 倾斜感
  - 涉及文件：`src/main.rs`（渲染循环中多层不同 ws_offset）

- [ ] **4.2** 工作区切换的连续内容渲染
  - 滑动过程中，相邻工作区的窗口逐渐出现（不是突然显示/隐藏）
  - 窗口透明度随与屏幕中心的距离渐变（边缘半透明 → 中心完全不透明）
  - 涉及文件：`src/main.rs`（渲染循环 + 自定义 RenderElement 带透明度）

- [ ] **4.3** Overview 进入时的背景模糊效果
  - 模拟毛玻璃效果：对底层内容做 GPU 降采样 + 近邻采样（产生像素模糊感）
  - 或者：在 Overview 遮罩中使用更深的颜色 + 微妙的渐变边缘
  - 涉及文件：`src/layout/overview.rs`

- [ ] **4.4** 工作区边缘的微妙光影效果
  - 滚动到工作区边界时，边缘有微光/光晕效果
  - 暗示"还有更多内容"的方向
  - 涉及文件：`src/layout/util.rs`（新增渐变渲染辅助）+ `src/main.rs`

- [ ] **4.5** 平滑的 scratchpad / launcher 过渡动画
  - 当前：瞬间出现/消失
  - 新：弹簧弹出动画（复用阶段零的 Spring）
  - scratchpad 从顶部滑入 + 缩放回弹
  - launcher 从中心扩展 + 淡入
  - 涉及文件：`src/main.rs`（修改 scratchpad/launcher 状态管理）

---

## 验证方案

### 阶段零验证
- `cargo build` 编译通过
- 单元测试 `physics.rs` 的弹簧/惯性计算（手动验证数值正确性）

### 阶段一验证
- `Super+Left/Right` 平滑切换工作区，带弹性吸附
- 触摸板 3 指左右滑动驱动工作区滚动
- 快速滑动后有惯性，自然减速并吸附到最近工作区
- 渲染不撕裂（无闪烁/黑帧）
- 多显示器各自独立滚动

### 阶段二验证
- `Super+Tab` 弹出任务面板，带弹簧回弹动画
- 缩略图正确显示当前工作区的窗口内容
- 点击缩略图切换焦点，面板收起
- 面板外点击关闭面板
- 动画无跳帧

### 阶段三验证
- `Super+Down` 触发鸟瞰视图，带 3D 缩放过渡
- 9 个工作区以 3×3 网格显示
- 鼠标悬停高亮工作区
- 点击工作区跳转，带缩放回位动画
- 键盘方向键导航
- 所有窗口保持正确状态（无 focus 泄漏）

### 阶段四验证
- 视差效果可感知且不突兀
- 边缘光影效果自然
- Overview 背景模糊效果美观

---

## 回滚策略

每个阶段都是独立的代码改动，可以逐步合并：

1. **阶段零**：纯新增文件，不影响现有功能，删除即可回滚
2. **阶段一**：修改 `switch_workspace` 逻辑。回滚方式：恢复 `scroll_offset` 为离散切换。保留 `physics.rs` 不影响其他代码。
3. **阶段二**：新增 overlay 层，通过 `overview.active()` 守卫。回滚方式：在渲染管线中移除 Step 4.8。
4. **阶段三**：最大改动（窗口活跃策略）。回滚方式：恢复 `size = (1,1)` 隐藏逻辑。
5. **阶段四**：纯视觉增强，移除不影响功能。

---

## 风险评估

| 风险 | 严重程度 | 缓解措施 |
|------|---------|----------|
| 多工作区同时活跃导致 GPU 内存压力 | 🔴 高 | 限制 `visible_range` 为 ±1（阶段一）/ lazy activation + 超时释放（阶段三） |
| 窗口频繁 resize（1×1 ↔ 真实尺寸）导致客户端闪烁 | 🟡 中 | 使用 `Unmap` 而非 `size=(1,1)` 隐藏；或接受 1 帧延迟 |
| XWayland 窗口对 resize 反应慢，Overview 进入时黑框 | 🟡 中 | Overview 进入时等待 1-2 帧让 X11 客户端重绘再渲染缩略图 |
| 弹簧参数不适配不同刷新率显示器 | 🟢 低 | 帧率无关的物理计算（`dt` 驱动，非固定步长） |
| `Frame::clear` 无真正 alpha blending | 🟢 低 | 用颜色调制模拟（已有的通知技术），缩略图用纹理缩放 |
| Smithay `render_elements_from_surface_tree` CPU 开销 | 🟡 中 | 缓存 surface elements，只在窗口内容变化时重建 |

---

## 注意事项

1. **`render_elements_from_surface_tree` 的 `scale` 参数是 HiDPI 倍率，不是缩放因子。** 缩略图缩放必须通过自定义 `RenderElement` 的 `dst` 矩形实现。
2. **`downscale_filter` 是全局状态。** 渲染缩略图后必须恢复为 `Nearest`，否则正常窗口会变模糊。
3. **锁屏优先级最高。** Overview 激活时如果锁屏触发，必须先关闭 Overview 再进入锁屏。
4. **全屏窗口特殊处理。** Overview 模式下全屏窗口不应全屏渲染，应缩小为缩略图。
5. **`active_ws` 与 `output_active_ws` 的双重追踪。** 无限滚动需明确哪个是 source of truth。建议：`scroll_offset` 为 source of truth，`active_ws` 由 `scroll_offset.round()` 派生。
6. **渐进式实施。** 每个阶段独立可验证，不要试图一次性实现所有特性。阶段一完成后即可发布，后续阶段作为增量更新。

---

## 技术规格补充

### 物理引擎参数设计

```
弹簧参数（工作区吸附）:
  stiffness = 300.0   # 刚度 — 控制吸附速度
  damping   = 30.0    # 阻尼 — 控制振荡次数（~1次轻微过冲）
  → 自然频率 ω₀ = √(300/1) ≈ 17.3 rad/s
  → 阻尼比 ζ = 30/(2×√300) ≈ 0.866（欠阻尼，有轻微过冲）
  → 建立时间 ≈ 4/(ζω₀) ≈ 270ms

弹簧参数（面板弹出）:
  stiffness = 200.0
  damping   = 20.0
  → ζ ≈ 0.707（临界阻尼附近，快速无过冲）

惯性参数（手势滑动）:
  friction = 0.92      # 每帧（@60fps）保留 92% 速度
  → 1 秒后速度衰减到 0.92^60 ≈ 0.6% → 自然停止
  → snap_threshold: |velocity| < 0.1 时开始弹簧吸附
```

### 渲染管线更新（最终版）

```
Step 0:  锁屏（优先，覆盖一切）
Step 1:  壁纸（含视差偏移: ws_offset × 0.5）
Step 2:  窗口内容（含视差偏移: ws_offset × 1.0，相邻工作区可见）
Step 2.5: IM popup
Step 3:  窗口装饰（含视差偏移）
Step 4:  Scratchpad（含弹簧动画）
Step 4.5: X11 OR 窗口
Step 4.8: Overview overlay（任务面板 / 鸟瞰视图）← 新增
Step 5:  Headbar（含视差偏移: ws_offset × 1.2）
Step 6:  通知
Step 7:  启动器
Step 8:  光标
Step 9:  截图选区
Step 10: 截图捕获
```

### 键绑定更新

| 绑定 | 功能 | 阶段 |
|------|------|------|
| `Super+Left/Right` | 切换到相邻工作区（无限滚动） | 阶段一 |
| 3 指左右滑动 | 惯性滚动工作区 | 阶段一 |
| 4 指上滑 | 进入鸟瞰视图 | 阶段三 |
| 4 指下滑 | 进入任务面板 | 阶段二 |
| `Super+Tab` | 切换任务面板 | 阶段二 |
| `Super+Down` | 切换鸟瞰视图 | 阶段三 |
| `Escape`（Overview 中） | 退出 Overview | 阶段二/三 |
| 方向键（Overview 中） | 导航选择 | 阶段三 |
| `Enter`（Overview 中） | 确认选择 | 阶段三 |
