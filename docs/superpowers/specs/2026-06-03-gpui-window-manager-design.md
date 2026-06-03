# GPUI 窗口管理器迁移 — 设计文档

> 日期: 2026-06-03
> 分支: experiment/gpui-migration
> 状态: 待实现

## 概述

将 Clippi 的窗口管理功能从 Slint 迁移到 GPUI，包括：快捷键显示/隐藏窗口、失去焦点自动隐藏、固定窗口阻止隐藏、三种窗口位置模式（居中/跟随鼠标/记忆位置）。同时建立统一的 200ms 轮询循环，替代旧的 Slint `Looper` 模式。

## 背景

当前 Slint 代码中，窗口管理分散在 `Frontend`（position/hide/show）、`FocusService`（auto-hide）、`HotkeyService`（快捷键唤起）三个组件中，通过 `Looper` 的 200ms Slint Timer 统一驱动。

GPUI 迁移中需要进行以下调整：
- 窗口管理统一到独立的 `WindowManager` Entity
- 轮询机制从 Slint Timer 转为 GPUI `cx.spawn` + async Timer
- 关闭窗口时隐藏而非退出进程（后续集成托盘）
- 物理像素与逻辑像素的正确处理

## 架构设计

### 组件图

```
┌─────────────────────────────────────────────────┐
│ main.rs                                          │
│  cx.open_window(...)                             │
│  WindowManager::new(window, state, cx)           │
│  └── 统一 poll_loop (200ms)                       │
│       ├── Hotkey poll → show_and_focus()          │
│       ├── Focus poll  → auto_hide 判断            │
│       ├── Clipboard  → 剪贴板变更检测              │
│       └── Suppress   → 抑制期检查                 │
└─────────────────────────────────────────────────┘
```

### 数据流

```
HotkeyListener (平台线程)          FocusWatcher (WinEventHook)
    │ poll_pressed()                    │ is_clippi_foreground()
    ▼                                   ▼
┌─────────────────────────────────────────────────┐
│ WindowManager.poll()                            │
│                                                 │
│  hotkey pressed? → show_and_focus(window)        │
│  focus lost?     → auto_hide? → hide(window)    │
│  clipboard?      → state.update() → notify()    │
│  suppress?       → 检查 Instant::now()          │
└─────────────────────────────────────────────────┘
    │
    ▼
WindowHandle: set_visible / set_position / activate_window
```

## 详细设计

### 1. PositionMode 枚举 (`src/core/frontend.rs`)

```rust
#[derive(Clone, Copy, PartialEq)]
pub enum PositionMode {
    Center,       // 光标所在显示器居中
    FollowMouse,  // 光标位置偏移 sidebar 36px
    Remember,     // 上次关闭位置（fallback → Center）
}

impl PositionMode {
    pub fn from_str(s: &str) -> Self;
    pub fn from_int(v: i32) -> Self;
    pub fn to_int(self) -> i32;
}
```

### 2. WindowManager Entity (`src/ui/window_manager.rs`)

```rust
pub struct WindowManager {
    // ── 窗口状态 ──
    position_mode: PositionMode,
    pinned: bool,
    auto_hide: bool,
    visible: bool,
    suppress_until: Option<Instant>,

    // ── 平台资源 ──
    hotkey: Option<Box<dyn HotkeyListener>>,
    focus_watcher: Option<FocusWatcher>,
    foreground_app_name: ForegroundAppName,

    // ── 几何缓存 ──
    saved_x: i32,
    saved_y: i32,
    saved_w: f32,
    saved_h: f32,

    // ── 依赖 Entity ──
    state: Entity<AppState>,
    list_view: Entity<ClipboardListView>,
    search_bar: Entity<SearchBar>,

    // ── 剪贴板服务 ──
    clipboard_service: GpuiClipboardService,

    // ── 窗口事件 ──
    _subscriptions: Vec<Subscription>,
    _poll_task: Task<()>,
}
```

### 3. 窗口生命周期

#### 初始显示
- `main.rs` 中 `cx.open_window` 创建窗口后调用 `wm.set_initial_suppress()`
- 窗口按 `position_mode` 计算初始位置

#### 关闭（隐藏到后台）
- `cx.on_window_closed` 中拦截关闭事件，调用 `wm.hide()`
- `hide()` 内部: 保存几何 → `set_visible(false)` → 释放资源
- 不退出进程，hotkey 线程保持活跃

#### 快捷键唤起
- 200ms 轮询中检测 `hotkey.poll_pressed()` → `wm.show_and_focus()`
- `show_and_focus()`: suppress 600ms → 计算位置 → `set_visible(true)` → `activate_window()` → 重置 pin

### 4. Auto-hide 判定

```rust
fn should_auto_hide(&self) -> bool {
    if !self.auto_hide { return false; }       // 设置关闭
    if self.pinned { return false; }            // 窗口固定
    if self.is_suppressed() { return false; }   // 刚显示 600ms 内
    if is_clippi_foreground() { return false; } // 正在操作 Clippi
    if !self.visible { return false; }          // 已隐藏
    true
}
```

### 5. 位置计算

复用 `platform::monitor` 模块（`get_cursor_pos`, `get_monitor_work_area`, `is_point_on_monitor`, `clamp_to_work_area`）。

**物理/逻辑像素策略：**
- Windows: `get_cursor_pos()` 和 `GetMonitorInfo` 返回物理像素，与 GPUI `DevicePixels` 一致，直接使用
- macOS: monitor APIs 返回逻辑点，需 `* window.scale_factor()` 转为物理像素
- GPUI `px()` 函数使用逻辑像素 (Pixels)，`WindowBounds::Windowed` 使用 `DevicePixels`

```rust
fn calc_center(cursor: (i32, i32), win_size: Size<DevicePixels>) -> DevicePoint;
fn calc_follow_mouse(cursor: (i32, i32), win_size: Size<DevicePixels>) -> DevicePoint;
fn calc_remember(win_size: Size<DevicePixels>) -> Option<DevicePoint>;
fn clamp_to_work_area(pt: DevicePoint, size: Size<DevicePixels>, area: &MonitorRect) -> DevicePoint;
```

### 6. 统一轮询

WindowManager 内部一个 async task，200ms 间隔：

```rust
let _poll_task = cx.spawn(|this, mut cx| async move {
    loop {
        Timer::after(Duration::from_millis(200)).await;
        let Some(this) = this.upgrade() else { break };
        if this.update(&mut cx, |wm, cx| wm.poll(cx)).is_err() { break };
    }
});
```

`poll()` 方法内：
1. Hotkey check → `show_and_focus()` if pressed
2. Focus check → `hide()` if should auto-hide
3. Clipboard check → update `AppState.items` and notify list_view
4. Blacklist check → dynamic hotkey register/unregister

### 7. 设置同步

从 `AppState.settings` 读取初始值：
- `settings.auto_hide` → `WindowManager.auto_hide`
- `settings.window_position_mode` → `WindowManager.position_mode`
- `settings.saved_window_x/y/w/h` → 几何缓存

窗口关闭时写回：
- `saved_x, saved_y` → `settings.saved_window_x, saved_window_y`
- `saved_w, saved_h` → `settings.saved_window_width, saved_window_height`

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/core/frontend.rs` | 修改 | 补回 `PositionMode` 枚举、`clamp_to_work_area` 函数 |
| `src/ui/window_manager.rs` | **新建** | WindowManager Entity 完整实现 |
| `src/ui/mod.rs` | 修改 | 添加 `pub mod window_manager` |
| `src/ui/root.rs` | 修改 | 移除 `_clipboard_poll_task`、`clipboard_service`；接收 WindowManager 事件 |
| `src/main.rs` | 修改 | 集成 WindowManager，处理 on_window_closed |

## 不包含（后续迁移）

- 系统托盘图标和菜单（`tray.rs`）
- 设置面板中窗口位置/auto-hide 的 UI 切换
- 窗口 resize 拖拽手柄
- 右键菜单定位
- Sync 服务轮询

## 测试要点

1. 快捷键能正确唤起/隐藏窗口
2. 点击 Clippi 外部后窗口自动隐藏（auto_hide = true 时）
3. Pin 状态下失去焦点不隐藏
4. 三种位置模式各自正确定位
5. 关闭窗口后进程仍在后台运行
6. 再次快捷键唤起后窗口出现在正确位置
7. 多显示器场景下窗口不出界
