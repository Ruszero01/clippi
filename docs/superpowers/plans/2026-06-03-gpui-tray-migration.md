# GPUI 托盘功能迁移 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将系统托盘功能从 Slint 迁移到 GPUI，在 WindowManager 的 200ms poll 循环中集成托盘事件轮询。

**Architecture:** 平台层 `TrayManager`（`src/platform/tray.rs`）无 Slint 依赖，直接复用。在 `WindowManager` 中新增 `tray` 字段和 `poll_tray()` 方法，处理托盘 Action 分发。退出/重启使用 GPUI 的 `cx.shutdown()` API。

**Tech Stack:** Rust, GPUI 0.2.2, tray-icon 0.19, windows-sys

---

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/ui/window_manager.rs` | 修改 | 新增 tray 字段、poll_tray、quit/restart 方法、OpenSettings 事件 |
| `src/ui/root.rs` | 修改 | RootView subscription 处理 OpenSettings 事件（TODO 占位） |

---

### Task 1: 添加导入、常量和字段声明

**Files:**
- Modify: `src/ui/window_manager.rs:7-21`

- [ ] **Step 1: 新增 use 导入**

在 `window_manager.rs` 第 21 行之后（`use crate::state::app::AppState;` 之后）加入：

```rust
use crate::platform::tray::{TrayAction, TrayManager};
use crate::services::update;
```

- [ ] **Step 2: 新增 RELEASES_URL 常量**

在第 24 行之后（`pub type ForegroundAppName = ...` 之后）加入：

```rust
/// GitHub releases page for the Clippi project.
const RELEASES_URL: &str = "https://github.com/Ruszero01/clippi/releases";
```

- [ ] **Step 3: 新增 WindowManagerEvent::OpenSettings 变体**

将 `WindowManagerEvent` 枚举（第 27-32 行）修改为：

```rust
pub enum WindowManagerEvent {
    /// Clipboard data changed; RootView should refresh its list.
    ClipboardChanged,
    /// Pin state changed (unpinned on hotkey show, or toggled by titlebar).
    PinnedChanged(bool),
    /// Tray menu "Settings" clicked — switch to settings view.
    /// TODO: Implement when settings panel GPUI migration is complete.
    OpenSettings,
}
```

- [ ] **Step 4: 新增 tray 字段到 WindowManager 结构体**

在 `WindowManager` 结构体的字段区域，`_poll_task` 字段之前加入（约第 70 行附近）：

```rust
    // ── 系统托盘 ──
    tray: Option<TrayManager>,
```

- [ ] **Step 5: 编译检查**

```bash
cargo build 2>&1
```

预期：编译失败（tray 字段未在构造函数中初始化），确认下一步需要修改构造函数。

---

### Task 2: 在 WindowManager::new() 中初始化 TrayManager

**Files:**
- Modify: `src/ui/window_manager.rs:76-131`

- [ ] **Step 1: 在 new() 方法开头初始化 TrayManager**

在 `WindowManager::new()` 方法中，`let clipboard_service = GpuiClipboardService::new();` 语句之后（约第 104 行）加入：

```rust
        // ── Initialize tray ──
        let tray = Some(TrayManager::new());
```

- [ ] **Step 2: 在 Self 构造体中添加 tray 字段**

在 `Self { ... }` 构造体中，`clipboard_service` 字段之后加入：

```rust
            tray,
```

- [ ] **Step 3: 编译检查**

```bash
cargo build 2>&1
```

预期：编译通过，无警告。

---

### Task 3: 实现 poll_tray() 方法

**Files:**
- Modify: `src/ui/window_manager.rs`（在 poll_loop 相关方法区域之后新增方法）

- [ ] **Step 1: 新增 poll_tray() 方法**

在 `poll_focus()` 方法之后（约第 211 行之后，`// ── Foreground detection ──` 注释之前）加入：

```rust
    // ── Tray event polling ───────────────────────────────────────────

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
                self.do_restart(cx);
            }
            Some(TrayAction::Quit) => {
                self.do_quit(cx);
            }
            Some(TrayAction::CheckUpdate) => {
                update::open_releases_page(RELEASES_URL);
            }
            None => {}
        }
    }
```

- [ ] **Step 2: 将 poll_tray() 加入 poll() 方法**

在 `poll()` 方法（约第 150-162 行）的末尾（`self.poll_focus(cx);` 之后，闭合花括号之前）加入：

```rust
        // 5. Tray events
        self.poll_tray(cx);
```

- [ ] **Step 3: 编译检查**

```bash
cargo build 2>&1
```

预期：编译失败 — `do_restart()` 和 `do_quit()` 方法尚未定义。

---

### Task 4: 实现退出和重启方法

**Files:**
- Modify: `src/ui/window_manager.rs`（在 poll_tray 之后新增方法）

- [ ] **Step 1: 新增 prepare_shutdown() 方法**

在 `poll_tray()` 之后加入：

```rust
    /// Prepare for graceful shutdown: save geometry, flush WAL,
    /// release platform resources.
    fn prepare_shutdown(&mut self, cx: &mut Context<Self>) {
        let (sx, sy, sw, sh) = (self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.state.update(cx, |state, _cx| {
            let _ = state.db.checkpoint();
            let mut settings = state.settings.lock().expect("settings lock poisoned");
            if sx >= 0 && sy >= 0 {
                settings.saved_window_x = sx;
                settings.saved_window_y = sy;
            }
            if sw > 0.0 && sh > 0.0 {
                settings.saved_window_width = sw;
                settings.saved_window_height = sh;
            }
            settings.save();
        });
        self.shutdown();
    }
```

- [ ] **Step 2: 新增 do_quit() 方法**

在 `prepare_shutdown` 之后加入：

```rust
    /// Fully quit the application.
    fn do_quit(&mut self, cx: &mut Context<Self>) {
        self.prepare_shutdown(cx);
        cx.shutdown();
    }
```

- [ ] **Step 3: 新增 do_restart() 方法**

在 `do_quit()` 之后加入：

```rust
    /// Restart the application: flush, spawn new process, then quit.
    fn do_restart(&mut self, cx: &mut Context<Self>) {
        self.prepare_shutdown(cx);
        crate::core::settings::spawn_new_process();
        cx.shutdown();
    }
```

- [ ] **Step 4: 编译检查**

```bash
cargo build 2>&1
```

预期：编译通过，无警告。

---

### Task 5: RootView 处理 OpenSettings 事件

**Files:**
- Modify: `src/ui/root.rs:64-81`

- [ ] **Step 1: 在 WindowManager subscription 中新增 OpenSettings 分支**

在 `RootView::new()` 的 `_wm_subscription` 回调（第 67-81 行）中，在 `WindowManagerEvent::PinnedChanged(pinned)` 分支之后加入：

```rust
                WindowManagerEvent::OpenSettings => {
                    // TODO: Switch to settings view when settings panel
                    // is fully migrated to GPUI.
                    // this.current_view = "settings".into();
                    // cx.notify();
                }
```

- [ ] **Step 2: 编译检查**

```bash
cargo build 2>&1
```

预期：编译通过，无警告。

---

### Task 6: 清理 — 确认 services/tray.rs 状态

**Files:**
- Check: `src/services/mod.rs`
- Check: `src/services/tray.rs`

- [ ] **Step 1: 确认 services/mod.rs 中 tray 模块的注释状态**

读取 `src/services/mod.rs`，确认 `pub mod tray;` 已经被注释掉。当前状态（已注释）无需修改。

- [ ] **Step 2: 全局编译检查（含所有警告）**

```bash
cargo build 2>&1
```

预期：编译通过，clippy 无警告。

---

### Task 7: 运行 Clippy 并验证

**Files:**
- Verify: `src/ui/window_manager.rs`
- Verify: `src/ui/root.rs`

- [ ] **Step 1: 运行 clippy**

```bash
cargo clippy -- -D warnings 2>&1
```

预期：无警告，全部通过。

- [ ] **Step 2: 检查 dead_code 警告**

确认 `TrayManager` 结构体的 `#[allow(dead_code)]` 标注仍然有效（`tray` 字段和内部 menu item 保持字段会被使用）。

- [ ] **Step 3: Commit**

```bash
git add src/ui/window_manager.rs src/ui/root.rs src/services/mod.rs
git commit -m "feat: migrate tray functionality from Slint to GPUI

- Add tray field and poll_tray() to WindowManager
- Integrate tray polling into 200ms poll loop
- Implement do_quit() / do_restart() using GPUI cx.shutdown()
- Add OpenSettings event (TODO until settings panel migrated)
- Platform-layer TrayManager reused as-is (no Slint dependency)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: 最终验证清单

- [ ] `cargo build` 通过
- [ ] `cargo clippy -- -D warnings` 无警告
- [ ] 托盘图标在启动后出现
- [ ] 双击托盘图标显示窗口
- [ ] 托盘菜单「显示窗口」正常工作
- [ ] 托盘菜单「退出」完全退出进程
- [ ] 托盘菜单「重启应用」退出后重新启动
- [ ] 托盘菜单「检查更新」打开 GitHub Releases 页面
- [ ] 窗口关闭后托盘图标仍然存在（隐藏到后台）
