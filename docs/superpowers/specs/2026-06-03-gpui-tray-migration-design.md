# GPUI 托盘功能迁移 — 设计文档

> 日期: 2026-06-03
> 分支: experiment/gpui-migration
> 状态: 待实现

## 概述

将系统托盘功能从 Slint 架构迁移到 GPUI。托盘管理（创建图标、菜单、事件轮询）由平台层 `TrayManager` 提供（不依赖 Slint，无需修改），服务层逻辑（Action 分发）从 `TrayService` 迁移内聚到 `WindowManager` 的 poll 循环中。

## 背景

当前 Slint 架构中：
- `platform/tray.rs` — `TrayManager` 结构体，使用 `tray-icon` crate，不依赖 Slint
- `services/tray.rs` — `TrayService` 实现 `Pollable` trait，通过 `Looper` 200ms 轮询
- `app.rs` — 创建 `TrayService` 并注册到 `Looper`

GPUI 迁移中 `services/tray.rs` 被注释掉（`mod.rs` 第 11 行），因为它依赖 `slint::quit_event_loop()`。`platform/tray.rs` 无 Slint 依赖，可直接复用。

设计依据参考 [GPUI 窗口管理器迁移设计](2026-06-03-gpui-window-manager-design.md)，其中托盘迁移被列为后续工作。

## 架构设计

### 组件图

```
┌──────────────────────────────────────────────────┐
│ WindowManager.poll()             (200ms)          │
│                                                  │
│  1. poll_hotkey()     — 已有                      │
│  2. poll_blacklist()  — 已有                      │
│  3. poll_clipboard()  — 已有                      │
│  4. poll_focus()      — 已有                      │
│  5. poll_tray()       — 新增 ✨                   │
│     └─ TrayManager.poll()                        │
│        └─ Option<TrayAction>                     │
│           ├─ Show         → show_and_focus()     │
│           ├─ OpenSettings → emit event (暂留)     │
│           ├─ Restart      → prepare_and_quit()   │
│           ├─ Quit         → prepare_and_quit()   │
│           └─ CheckUpdate  → open_releases_page() │
└──────────────────────────────────────────────────┘
```

### 数据流

```
tray-icon (系统托盘)         tray-icon (菜单点击)
    │ TrayIconEvent              │ MenuEvent
    ▼                            ▼
┌──────────────────────────────────────────────────┐
│ TrayManager.poll()                               │
│                                                  │
│  drain(TrayIconEvent::receiver())                 │
│    → DoubleClick → TrayAction::Show               │
│                                                  │
│  drain(MenuEvent::receiver())                     │
│    → MenuId match → TrayAction::*                 │
└──────────────────────────────────────────────────┘
    │
    ▼ Option<TrayAction>
WindowManager.poll_tray()
    │
    ▼ match action
show_and_focus() | emit(OpenSettings) | prepare_and_quit() | open URL
```

## 详细设计

### 1. WindowManager 新增字段

```rust
pub struct WindowManager {
    // ... 现有字段 ...

    // ── 系统托盘 ──
    tray: Option<TrayManager>,      // ✨ 新增
}
```

`TrayManager::new()` 在 `WindowManager::new()` 中创建，失败不阻塞（tray 为 None，托盘功能静默不可用）。

### 2. poll_tray() 方法

```rust
fn poll_tray(&mut self, cx: &mut Context<Self>) {
    let action = match self.tray.as_ref() {
        Some(t) => t.poll(),
        None => return,
    };

    match action {
        Some(TrayAction::Show) => {
            self.show_and_focus(cx);
        }
        Some(TrayAction::OpenSettings) => {
            cx.emit(WindowManagerEvent::OpenSettings);
        }
        Some(TrayAction::Restart) => {
            self.do_restart();
        }
        Some(TrayAction::Quit) => {
            self.do_quit();
        }
        Some(TrayAction::CheckUpdate) => {
            update::open_releases_page(RELEASES_URL);
        }
        None => {}
    }
}
```

### 3. 退出与重启

#### 退出流程

```rust
fn do_quit(&mut self) {
    self.prepare_shutdown();
    // GPUI: 关闭所有窗口并退出事件循环
    // 调用方式取决于 GPUI 版本 API：
    // - cx.shutdown() — 在 Context 内调用
    // - AppContext::quit() — 通过全局 AppContext
}
```

#### 重启流程

```rust
fn do_restart(&mut self) {
    self.prepare_shutdown();
    spawn_new_process();  // 复用 core::settings::spawn_new_process
    // 同上退出流程
}
```

#### 退出前清理

```rust
fn prepare_shutdown(&mut self) {
    // 1. Flush WAL checkpoint
    self.state.update(cx, |state, _| {
        let _ = state.db.checkpoint();
    });
    // 2. 保存窗口几何到 settings
    // 3. 停止 hotkey、focus watcher（复用现有 shutdown()）
    self.shutdown();
}
```

> **注意**: 实际的 GPUI 退出 API 需要在实现阶段确认。可能是 `cx.shutdown()`、`AppContext::quit()` 或其他等效方法。

### 4. WindowManagerEvent 扩展

```rust
pub enum WindowManagerEvent {
    ClipboardChanged,      // 已有
    PinnedChanged(bool),   // 已有
    OpenSettings,          // ✨ 新增 — 设置面板（暂留，设置面板 GPUI 迁移未完成）
}
```

RootView 的 subscription 中新增对 `OpenSettings` 的处理（TODO 占位，待设置面板迁移完成后实现）。

### 5. 常量

```rust
/// GitHub releases page URL
const RELEASES_URL: &str = "https://github.com/Ruszero01/clippi/releases";
```

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/ui/window_manager.rs` | **修改** | 新增 `tray` 字段、`poll_tray()`、`do_quit()`、`do_restart()`、`prepare_shutdown()`；`WindowManagerEvent` 加 `OpenSettings`；poll 循环中调用 `poll_tray()` |
| `src/platform/tray.rs` | **不变** | 平台层无 Slint 依赖，`TrayManager` 直接复用 |
| `src/services/mod.rs` | **修改** | 移除 `services/tray.rs` 注释（不再需要 `services/tray.rs`） |
| `src/services/tray.rs` | **删除** | 逻辑已内聚到 `WindowManager`，不再需要独立的 TrayService |

## 不包含（后续迁移）

- OpenSettings 实际切换到设置面板（设置面板 GPUI 迁移未完成）
- 托盘图标更新（状态指示、未读计数等）
- 托盘通知气泡

## 测试要点

1. 启动后系统托盘出现 Clippi 图标
2. 双击托盘图标 → 窗口显示并聚焦
3. 右键托盘菜单 → "显示窗口" → 窗口显示
4. 托盘菜单 → "退出" → 程序完全退出（进程结束）
5. 托盘菜单 → "重启应用" → 程序退出后自动重新启动
6. 托盘菜单 → "检查更新" → 打开 GitHub Releases 页面
7. 窗口关闭后托盘图标仍然存在（隐藏到后台，不退出）
8. 托盘图标 tooltip 显示 "Clippi"
