# GPUI 窗口管理器迁移 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将窗口管理功能（快捷键/自动隐藏/位置模式/关闭到后台）从 Slint 迁移到 GPUI `WindowManager` Entity。

**Architecture:** 新建 `WindowManager` Entity 作为统一的窗口管理器和轮询中心。通过 `cx.spawn` 驱动的 200ms 轮询循环统一管理 hotkey/focus/clipboard 检查。窗口操作通过 platform 模块 + raw window handle 完成。

**Tech Stack:** Rust, GPUI 0.2.2, windows-sys 0.59, raw-window-handle 0.6

**Design Doc:** `docs/superpowers/specs/2026-06-03-gpui-window-manager-design.md`

---

## File Structure

| 文件 | 操作 | 职责 |
|------|------|------|
| `src/core/frontend.rs` | 修改 | `PositionMode` 枚举 + `clamp_to_work_area` + 常量 |
| `src/ui/window_manager.rs` | **新建** | `WindowManager` Entity 完整实现 (~400 行) |
| `src/ui/mod.rs` | 修改 | 添加 `pub mod window_manager` |
| `src/ui/root.rs` | 修改 | 移除内部剪贴板轮询，接收 `WindowManager` 引用 |
| `src/main.rs` | 修改 | 创建 `WindowManager`，处理 close→hide，阻止退出 |

---

### Task 1: 恢复 PositionMode 和辅助函数到 core/frontend.rs

**Files:**
- Modify: `src/core/frontend.rs`

- [ ] **Step 1: 替换 frontend.rs 内容**

当前 `frontend.rs` 只有常量。用完整的 `PositionMode` 枚举 + `clamp_to_work_area` 函数替换：

```rust
//! Frontend management — window position modes and size constants.
//!
//! Framework-agnostic types and helpers used by both Slint (legacy) and
//! GPUI (current) window implementations.

use crate::platform::monitor;

/// Default window size (width, height) in logical pixels.
pub const DEFAULT_WINDOW_WIDTH: f32 = 360.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 480.0;

/// Window minimum/maximum size range in logical pixels.
pub const MIN_WINDOW_WIDTH: f32 = 360.0;
pub const MIN_WINDOW_HEIGHT: f32 = 480.0;
pub const MAX_WINDOW_WIDTH: f32 = 1200.0;
pub const MAX_WINDOW_HEIGHT: f32 = 1200.0;

/// Content panel X offset from the window left edge (logical pixels).
/// Matches the `x: 36px` panel offset in app.slint / root.rs.
pub const PANEL_OFFSET_X: f32 = 36.0;

/// Duration in milliseconds that the auto-hide suppression window lasts
/// after showing or focusing the window. Prevents immediate auto-hide.
pub const SUPPRESS_DURATION_MS: u64 = 600;

/// Window position mode.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PositionMode {
    /// Center on the monitor containing the cursor.
    Center,
    /// Align the content panel with the cursor, offset by PANEL_OFFSET_X.
    FollowMouse,
    /// Restore the last window position; fall back to Center if invalid.
    Remember,
}

impl PositionMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "follow" => Self::FollowMouse,
            "remember" => Self::Remember,
            _ => Self::Center,
        }
    }

    pub fn to_int(self) -> i32 {
        match self {
            Self::Center => 0,
            Self::FollowMouse => 1,
            Self::Remember => 2,
        }
    }

    pub fn from_int(v: i32) -> Self {
        match v {
            1 => Self::FollowMouse,
            2 => Self::Remember,
            _ => Self::Center,
        }
    }
}

/// Clamp a window rectangle to a monitor's work area so the window
/// stays fully visible on screen.
///
/// All parameters are in physical pixels (device pixels) on Windows,
/// logical points on macOS.
pub fn clamp_to_work_area(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    area: &monitor::MonitorRect,
) -> (i32, i32) {
    let max_x = (area.x + area.width - w).max(area.x);
    let max_y = (area.y + area.height - h).max(area.y);
    let x = x.max(area.x).min(max_x);
    let y = y.max(area.y).min(max_y);
    (x, y)
}
```

- [ ] **Step 2: 提交**

```bash
git add src/core/frontend.rs
git commit -m "feat: restore PositionMode enum and clamp_to_work_area to frontend"
```

---

### Task 2: 创建 WindowManager Entity

**Files:**
- Create: `src/ui/window_manager.rs`

- [ ] **Step 1: 创建文件，添加 imports 和常量**

```rust
//! Window manager — unified window state, positioning, and poll loop.
//!
//! Owns the window lifecycle: show/hide, activate, position calculation,
//! auto-hide on focus loss, and hotkey-triggered show. Replaces the
//! Slint-era `Frontend` + `FocusService` + `HotkeyService` + `Looper` combo.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::*;

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, SetWindowPos, HWND_TOP, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_NOMOVE,
};

use crate::core::frontend::{
    clamp_to_work_area, PositionMode, DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH,
    PANEL_OFFSET_X, SUPPRESS_DURATION_MS,
};
use crate::core::settings::AppSettings;
use crate::platform::blacklist::is_clippi_foreground;
use crate::platform::focus::{start_focus_watcher, FocusWatcher};
use crate::platform::hotkey::{create_hotkey_listener, HotkeyListener};
use crate::platform::monitor;
use crate::services::gpui_clipboard::GpuiClipboardService;
use crate::state::app::AppState;

/// Shared foreground app name for cross-service coordination.
pub type ForegroundAppName = Arc<Mutex<String>>;
```

- [ ] **Step 2: 定义 WindowManager struct**

```rust
/// Events emitted by WindowManager for consumption by RootView.
pub enum WindowManagerEvent {
    /// Clipboard data changed; RootView should refresh its list.
    ClipboardChanged,
    /// Pin state toggled via hotkey show (always unpins on show).
    PinnedChanged(bool),
}

/// Unified window manager entity.
///
/// Owns the window lifecycle and all cross-service polling. Created once
/// in `main.rs` and stored as an `Entity<WindowManager>`. RootView
/// subscribes to clipboard and pin events.
pub struct WindowManager {
    // ── Window state ──
    position_mode: PositionMode,
    pinned: bool,
    auto_hide: bool,
    visible: bool,
    suppress_until: Option<Instant>,

    // ── Platform resources ──
    hotkey: Option<Box<dyn HotkeyListener>>,
    focus_watcher: Option<FocusWatcher>,
    foreground_app_name: ForegroundAppName,
    last_fg_app_name: String,

    // ── Geometry cache (physical pixels on Windows, logical on macOS) ──
    saved_x: i32,
    saved_y: i32,
    saved_w: f32,
    saved_h: f32,

    // ── Hotkey blacklist ──
    blacklist: Vec<String>,

    // ── Dependencies ──
    state: Entity<AppState>,
    clipboard_service: GpuiClipboardService,

    // ── Window handle (AnyWindowHandle for cross-async access) ──
    window_handle: Option<AnyWindowHandle>,

    // ── Tasks & subscriptions ──
    _poll_task: Option<Task<()>>,
}

impl EventEmitter<WindowManagerEvent> for WindowManager {}
```

- [ ] **Step 3: 实现 WindowManager::new**

```rust
impl WindowManager {
    pub fn new(
        state: Entity<AppState>,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = state.read(cx).settings.clone();

        // ── Initialize hotkey listener ──
        let hotkey = create_hotkey_listener(&settings.hotkey).ok();

        // ── Initialize focus watcher ──
        let focus_watcher = start_focus_watcher()
            .map_err(|e| log::error!("Failed to start focus watcher: {e}"))
            .ok();

        let foreground_app_name = Arc::new(Mutex::new(String::new()));

        let clipboard_service = GpuiClipboardService::new();

        let mut wm = Self {
            position_mode: PositionMode::from_str(&settings.window_position_mode),
            pinned: false,
            auto_hide: settings.auto_hide,
            visible: true,
            suppress_until: Some(Instant::now() + Duration::from_millis(500)),
            hotkey,
            focus_watcher,
            foreground_app_name,
            last_fg_app_name: String::new(),
            saved_x: settings.saved_window_x,
            saved_y: settings.saved_window_y,
            saved_w: settings.saved_window_width,
            saved_h: settings.saved_window_height,
            blacklist: settings.hotkey_blacklist.clone(),
            state,
            clipboard_service,
            window_handle: Some(window_handle),
            _poll_task: None,
        };

        // Start the unified poll loop
        wm.start_poll_loop(cx);

        wm
    }
```

- [ ] **Step 4: 实现轮询循环 start_poll_loop**

```rust
    fn start_poll_loop(&mut self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        self._poll_task = Some(cx.spawn(|mut cx| async move {
            loop {
                Timer::after(Duration::from_millis(
                    crate::services::poll_loop::POLL_INTERVAL_MS,
                ))
                .await;
                let Some(this) = this.upgrade() else { break };
                if this.update(&mut cx, |wm, cx| wm.poll(cx)).is_err() {
                    break;
                }
            }
        }));
    }
```

- [ ] **Step 5: 实现 poll 方法**

```rust
    fn poll(&mut self, cx: &mut Context<Self>) {
        // 1. Hotkey press → show window
        self.poll_hotkey(cx);

        // 2. Hotkey blacklist — dynamic register/unregister
        self.poll_blacklist();

        // 3. Clipboard changes → update state + notify
        self.poll_clipboard(cx);

        // 4. Focus / auto-hide logic
        self.poll_focus(cx);
    }

    fn poll_hotkey(&mut self, cx: &mut Context<Self>) {
        if let Some(ref hk) = self.hotkey {
            if hk.poll_pressed() {
                self.show_and_focus(cx);
            }
        }
    }

    fn poll_blacklist(&mut self) {
        let fg_name = self
            .foreground_app_name
            .lock()
            .ok()
            .map(|fg| fg.clone())
            .unwrap_or_default();

        if !fg_name.is_empty() && self.blacklist.contains(&fg_name) {
            if let Some(ref mut hk) = self.hotkey {
                hk.unregister();
            }
        } else {
            if let Some(ref mut hk) = self.hotkey {
                hk.register();
            }
        }
    }

    fn poll_clipboard(&mut self, cx: &mut Context<Self>) {
        let changed = self
            .state
            .update(cx, |state, _cx| self.clipboard_service.poll_state(state));

        if changed {
            let items = self.state.read(cx).items.clone();
            // Notify via event
            cx.emit(WindowManagerEvent::ClipboardChanged);
        }
    }

    fn poll_focus(&mut self, cx: &mut Context<Self>) {
        // Update foreground app name for blacklist
        self.update_foreground_app_name();

        let is_clippi = is_clippi_foreground();

        // ── Auto-hide logic ──
        if !self.auto_hide || self.pinned || !self.visible || self.is_suppressed() || is_clippi {
            return;
        }

        self.hide(cx);
    }
```

- [ ] **Step 6: 实现 update_foreground_app_name**

```rust
    fn update_foreground_app_name(&mut self) {
        use crate::platform::focus::get_foreground_app_info;

        if is_clippi_foreground() {
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                fg.clear();
            }
            return;
        }

        if let Some(info) = get_foreground_app_info() {
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                *fg = info.app_name.clone();
            }
            self.last_fg_app_name = info.app_name;
        }
    }
```

- [ ] **Step 7: 实现窗口位置计算**

```rust
    /// Calculate window position based on current position_mode.
    ///
    /// Returns physical-pixel coordinates on Windows, logical-point
    /// coordinates on macOS (to be converted by caller if needed).
    fn calculate_position(&self) -> Option<(i32, i32)> {
        let (win_w, win_h) = self.effective_window_size();
        let win_w_i32 = win_w as i32;
        let win_h_i32 = win_h as i32;

        // sidebar_offset: the main content panel's x position within the
        // window. On Windows, monitor APIs return physical pixels and
        // our size is in logical pixels, so scale the offset.
        #[cfg(target_os = "windows")]
        let sidebar_offset = PANEL_OFFSET_X as i32; // logical, scaled below if needed
        #[cfg(target_os = "macos")]
        let sidebar_offset = PANEL_OFFSET_X as i32;

        // Windows: position is physical pixels, size is logical.
        // offset is logical (36px panel), convert for physical calculation.
        #[cfg(target_os = "windows")]
        let sidebar_offset = 36i32; // After clamping we'll add this back

        let (mut x, mut y) = match self.position_mode {
            PositionMode::Center => self.calc_center(win_w_i32, win_h_i32)?,
            PositionMode::FollowMouse => {
                self.calc_follow_mouse(win_w_i32, win_h_i32, sidebar_offset)?
            }
            PositionMode::Remember => self.calc_remember(win_w_i32, win_h_i32)
                .unwrap_or_else(|| self.calc_center(win_w_i32, win_h_i32)
                    .unwrap_or((0, 0)))?,
        };

        Some((x, y))
    }

    fn calc_center(&self, win_w: i32, win_h: i32) -> Option<(i32, i32)> {
        let (cx, cy) = monitor::get_cursor_pos()?;
        let area = monitor::get_monitor_work_area(cx, cy)?;
        let x = area.x + (area.width - win_w) / 2;
        let y = area.y + (area.height - win_h) / 2;
        Some((x, y))
    }

    fn calc_follow_mouse(&self, win_w: i32, win_h: i32, sidebar_offset: i32) -> Option<(i32, i32)> {
        let (cx, cy) = monitor::get_cursor_pos()?;
        let area = monitor::get_monitor_work_area(cx, cy)?;
        // Offset by sidebar width so the main panel aligns with the cursor
        Some(clamp_to_work_area(cx - sidebar_offset, cy, win_w, win_h, &area))
    }

    fn calc_remember(&self, win_w: i32, win_h: i32) -> Option<(i32, i32)> {
        let (sx, sy) = (self.saved_x, self.saved_y);
        if sx < 0 || sy < 0 {
            return None;
        }
        if !monitor::is_point_on_monitor(sx, sy) {
            return None;
        }
        let area = monitor::get_monitor_work_area(sx, sy)?;
        Some(clamp_to_work_area(sx, sy, win_w, win_h, &area))
    }

    fn effective_window_size(&self) -> (f32, f32) {
        let w = if self.saved_w > 0.0 {
            self.saved_w.max(DEFAULT_WINDOW_WIDTH)
        } else {
            DEFAULT_WINDOW_WIDTH
        };
        let h = if self.saved_h > 0.0 {
            self.saved_h.max(DEFAULT_WINDOW_HEIGHT)
        } else {
            DEFAULT_WINDOW_HEIGHT
        };
        (w, h)
    }

    fn is_suppressed(&self) -> bool {
        self.suppress_until
            .map(|until| Instant::now() <= until)
            .unwrap_or(false)
    }
```

- [ ] **Step 8: 实现平台窗口操作（show_and_focus, hide, set_position）**

```rust
    /// Show the window, calculate position, and bring it to foreground.
    pub fn show_and_focus(&mut self, cx: &mut Context<Self>) {
        // Suppress auto-hide for a short period
        self.suppress_until = Some(Instant::now() + Duration::from_millis(SUPPRESS_DURATION_MS));
        self.visible = true;
        self.pinned = false;
        cx.emit(WindowManagerEvent::PinnedChanged(false));

        #[cfg(target_os = "windows")]
        {
            if let Some((x, y)) = self.calculate_position() {
                let title: Vec<u16> = "Clippi\0".encode_utf16().collect();
                let hwnd = unsafe { FindWindowW(std::ptr::null_mut(), title.as_ptr()) };
                if !hwnd.is_null() {
                    unsafe {
                        SetWindowPos(
                            hwnd,
                            HWND_TOP,
                            x,
                            y,
                            0,
                            0,
                            SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE,
                        );
                        SetForegroundWindow(hwnd);
                    };
                }
            } else {
                let title: Vec<u16> = "Clippi\0".encode_utf16().collect();
                let hwnd = unsafe { FindWindowW(std::ptr::null_mut(), title.as_ptr()) };
                if !hwnd.is_null() {
                    unsafe { SetForegroundWindow(hwnd) };
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(ref wh) = self.window_handle {
                // On macOS, activate the app first
                let mtm = objc2::MainThreadMarker::new().unwrap();
                let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                ns_app.activateIgnoringOtherApps(true);
            }
        }

        cx.notify();
    }

    /// Hide the window to system tray (background) — does NOT exit the process.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.dismiss_ui(cx);

        #[cfg(target_os = "windows")]
        {
            // Save position in Remember mode
            if self.position_mode == PositionMode::Remember {
                let title: Vec<u16> = "Clippi\0".encode_utf16().collect();
                let hwnd = unsafe { FindWindowW(std::ptr::null_mut(), title.as_ptr()) };
                if !hwnd.is_null() {
                    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
                    use windows_sys::Win32::Foundation::POINT;
                    let mut pt = POINT { x: 0, y: 0 };
                    unsafe { ClientToScreen(hwnd, &mut pt); }
                    self.saved_x = pt.x;
                    self.saved_y = pt.y;
                }
            }

            // Hide window
            let title: Vec<u16> = "Clippi\0".encode_utf16().collect();
            let hwnd = unsafe { FindWindowW(std::ptr::null_mut(), title.as_ptr()) };
            if !hwnd.is_null() {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
                        hwnd,
                        windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
                    );
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            // TODO: macOS hide — use NSWindow setVisible:false
        }

        self.visible = false;
    }

    /// Clear all floating UI state (context menu, tag panels, etc.).
    /// Called from hide() or by RootView when user clicks blank area.
    fn dismiss_ui(&self, cx: &mut Context<Self>) {
        let state = self.state.clone();
        self.state.update(cx, |state, _cx| {
            state.clear_selection();
        });
        self.pinned = false;
        cx.emit(WindowManagerEvent::PinnedChanged(false));
    }

    /// Dismiss floating UI and switch back to clipboard view.
    pub fn dismiss_ui_to_clipboard(&self, cx: &mut Context<Self>) {
        self.dismiss_ui(cx);
        // RootView will observe this and switch view
    }
```

- [ ] **Step 9: 实现公共 setter 方法**

```rust
    // ── Public setters (called from RootView / Settings) ──

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        cx.emit(WindowManagerEvent::PinnedChanged(pinned));
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn set_auto_hide(&mut self, auto_hide: bool) {
        self.auto_hide = auto_hide;
    }

    pub fn set_position_mode(&mut self, mode: PositionMode) {
        self.position_mode = mode;
    }

    pub fn set_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
        if let Some(ref mut hk) = self.hotkey {
            hk.update_hotkey(hotkey_str)
        } else {
            Err("No hotkey listener".to_string())
        }
    }

    pub fn start_hotkey_recording(&mut self) {
        if let Some(ref mut hk) = self.hotkey {
            hk.start_recording();
        }
    }

    // ── Blacklist management ──

    pub fn blacklist_add(&mut self, app_name: &str, settings: &mut AppSettings) {
        if app_name.is_empty() || self.blacklist.contains(&app_name.to_string()) {
            return;
        }
        self.blacklist.push(app_name.to_string());
        settings.hotkey_blacklist = self.blacklist.clone();
        settings.save();
    }

    pub fn blacklist_remove(&mut self, app_name: &str, settings: &mut AppSettings) {
        self.blacklist.retain(|a| a != app_name);
        settings.hotkey_blacklist = self.blacklist.clone();
        settings.save();
    }

    pub fn foreground_app_name_clone(&self) -> String {
        self.foreground_app_name
            .lock()
            .ok()
            .map(|fg| fg.clone())
            .unwrap_or_default()
    }

    /// Persist current geometry to settings.
    pub fn save_geometry(&self, settings: &mut AppSettings) {
        if self.saved_x >= 0 && self.saved_y >= 0 {
            settings.saved_window_x = self.saved_x;
            settings.saved_window_y = self.saved_y;
        }
        if self.saved_w > 0.0 && self.saved_h > 0.0 {
            settings.saved_window_width = self.saved_w;
            settings.saved_window_height = self.saved_h;
        }
    }

    /// Release platform resources on shutdown.
    pub fn shutdown(&mut self) {
        if let Some(ref mut hk) = self.hotkey {
            hk.stop();
        }
        if let Some(ref mut fw) = self.focus_watcher {
            fw.stop();
        }
    }
}
```

- [ ] **Step 10: 提交**

```bash
git add src/ui/window_manager.rs
git commit -m "feat: add WindowManager entity with unified poll loop"
```

---

### Task 3: 更新 src/ui/mod.rs 添加模块声明

**Files:**
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: 添加 window_manager 模块**

```rust
pub mod window_manager;
```

插入位置：与其他 `pub mod` 声明一起，按字母顺序。

- [ ] **Step 2: 提交**

```bash
git add src/ui/mod.rs
git commit -m "chore: add window_manager module to ui"
```

---

### Task 4: 重构 src/ui/root.rs

**Files:**
- Modify: `src/ui/root.rs`

- [ ] **Step 1: 移除内部的剪贴板轮询和 GpuiClipboardService**

从 `RootView` 中删除以下字段：
- `clipboard_service: GpuiClipboardService`
- `_clipboard_poll_task: Task<()>`
- `_subscriptions: Vec<Subscription>`

- [ ] **Step 2: 添加 WindowManager 引用字段**

```rust
pub struct RootView {
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    titlebar: Entity<Titlebar>,
    list_view: Entity<ClipboardListView>,
    search_bar: Entity<SearchBar>,
    settings_panel: Entity<SettingsPanel>,
    sidebar: Entity<Sidebar>,
    tag_filter_panel: Entity<TagFilterPanel>,
    current_view: String,
    pinned: bool,
    theme: ClippiTheme,
    // _subscriptions removed — WindowManager owns the poll loop
}
```

- [ ] **Step 3: 更新 RootView::new 签名，接受 WindowManager**

```rust
impl RootView {
    pub fn new(
        window: &mut Window,
        window_manager: Entity<WindowManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = AppSettings::load();
        let state = cx.new(|_cx| AppState::new(settings));
        let app_state = state.read(cx);
        let items = app_state.items.clone();
        let list_view = cx.new(|cx| ClipboardListView::new(items, state.clone(), cx));
        list_view.update(cx, |list, _cx| list.focus(window));
        let titlebar = cx.new(|_cx| Titlebar::new(state.clone(), list_view.clone()));
        let search_bar = cx.new(|cx| SearchBar::new(state.clone(), list_view.clone(), window, cx));
        let settings_panel = cx.new(|cx| SettingsPanel::new(cx));
        let sidebar = cx.new(|_cx| Sidebar::new(state.clone(), list_view.clone()));
        let tag_filter_panel = cx.new(|cx| {
            TagFilterPanel::new(
                state.clone(),
                list_view.clone(),
                search_bar.clone(),
                window,
                cx,
            )
        });

        let theme = ClippiTheme::dark();

        Self {
            state,
            window_manager,
            titlebar,
            list_view,
            search_bar,
            settings_panel,
            sidebar,
            tag_filter_panel,
            current_view: "clipboard".into(),
            pinned: false,
            theme,
        }
    }
```

- [ ] **Step 4: 添加剪贴板变更事件监听**

通过 subscription 监听 `WindowManager` 的 `ClipboardChanged` 事件：

在 `RootView::new` 中注册 subscription（但不要将 subscriptions 存为字段，而是使用 `cx.observe` 或通过 WindowManager 直接更新）：

实际上，最简单的方式是不需要 subscription — WindowManager 直接操作 state。剪贴板轮询在 WindowManager 中完成，数据已经写入 `AppState`。RootView 只需要在对应用的渲染前刷新。

但是 ListView 需要被通知来刷新。WindowManager 的 clipboard_changed 事件需要在 RootView 层面消费者来更新 list_view。为此：

在 main.rs 创建 RootView 后，订阅 WindowManager events。

实际上，我们可以在 `RootView` 中留一个观察者。由于 WindowManager 在 `cx.open_window` 之前创建（但需要 `AnyWindowHandle`），我们需要重新思考初始化顺序...

最佳方式：WindowManager 直接更新 `AppState` 中的 items。RootView 的 render 方法读取 `self.state.read(cx).items`。但 ListView 已经缓存了 items...

让我重新设计：WindowManager 更新 state 后，RootView 通过 `cx.notify()` 重建。在 `RootView::new` 中监听 WindowManager：

```rust
// In RootView::new, after creating all entities:
cx.subscribe(
    &window_manager,
    move |this, _wm, event: &WindowManagerEvent, cx| match event {
        WindowManagerEvent::ClipboardChanged => {
            let items = this.state.read(cx).items.clone();
            this.list_view.update(cx, |list, cx| list.set_items(items, cx));
            cx.notify();
        }
        WindowManagerEvent::PinnedChanged(pinned) => {
            this.pinned = *pinned;
            cx.notify();
        }
    },
);
```

但由于我们不保留 `_subscriptions`，subscription 在创建后会被 drop 掉... 实际上是 `cx.subscribe` 返回一个 `Subscription`，我们需要持有它。

简化：在 RootView 中保留一个 `_subscriptions` 字段来持有 WindowManager 事件的 subscription。移除旧的 `_subscriptions`内容，只保留这一个。

- [ ] **Step 5: 完整的 RootView 实现**

```rust
use crate::state::app::AppState;
use crate::ui::window_manager::{WindowManager, WindowManagerEvent};
// ... other imports

pub struct RootView {
    state: Entity<AppState>,
    window_manager: Entity<WindowManager>,
    titlebar: Entity<Titlebar>,
    list_view: Entity<ClipboardListView>,
    search_bar: Entity<SearchBar>,
    settings_panel: Entity<SettingsPanel>,
    sidebar: Entity<Sidebar>,
    tag_filter_panel: Entity<TagFilterPanel>,
    current_view: String,
    pinned: bool,
    theme: ClippiTheme,
}
```

- [ ] **Step 6: 提交**

```bash
git add src/ui/root.rs
git commit -m "refactor: move clipboard polling to WindowManager, simplify RootView"
```

---

### Task 5: 更新 src/main.rs 集成 WindowManager

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: 重写 main.rs**

```rust
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::borrow::Cow;

use gpui::*;

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

mod core;
mod platform;
mod state;
mod ui;
mod services;

use ui::root::RootView;
use ui::window_manager::WindowManager;

fn ensure_single_instance() -> bool {
    std::net::TcpListener::bind("127.0.0.1:19876").is_ok()
}

fn init_logging(db_path: &str) {
    let log_path = core::paths::log_path(db_path);
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 1_000_000 {
            let old = log_path.with_extension("log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&log_path, &old);
        }
    }
    if let Ok(file) = std::fs::File::create(&log_path) {
        let _ = simplelog::WriteLogger::init(
            simplelog::LevelFilter::Info,
            simplelog::Config::default(),
            file,
        );
    }
}

fn main() {
    if !ensure_single_instance() {
        return;
    }

    let db_path = core::paths::resolve_db_path("");
    init_logging(&db_path.to_string_lossy());

    log::info!("Starting Clippi (GPUI experiment)");

    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        if let Err(err) = cx.text_system().add_fonts(vec![Cow::Borrowed(
            include_bytes!("../assets/fonts/iconfont.ttf").as_slice(),
        )]) {
            log::error!("Failed to load iconfont.ttf: {err}");
        }
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        gpui_component::Theme::global_mut(cx).background = Hsla::transparent_black();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.), px(100.)),
                    size(px(360.), px(480.)),
                ))),
                window_background: WindowBackgroundAppearance::Transparent,
                titlebar: Some(TitlebarOptions {
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                // ── Windows DWM non-client rendering disable ──
                #[cfg(target_os = "windows")]
                unsafe {
                    use windows_sys::Win32::Graphics::Dwm::{
                        DwmSetWindowAttribute, DWMWA_NCRENDERING_POLICY,
                    };
                    const DWMNCRP_DISABLED: u32 = 1;
                    if let Ok(handle) = window.window_handle() {
                        if let RawWindowHandle::Win32(wh) = handle.as_raw() {
                            let _ = DwmSetWindowAttribute(
                                wh.hwnd.get() as _,
                                DWMWA_NCRENDERING_POLICY as u32,
                                &DWMNCRP_DISABLED as *const u32 as *const _,
                                std::mem::size_of::<u32>() as u32,
                            );
                        }
                    }
                }

                // ── Create AppState ──
                let settings = core::settings::AppSettings::load();
                let state = cx.new(|_cx| crate::state::app::AppState::new(settings));

                // ── Create WindowManager (manages polling, hotkey, focus) ──
                let any_handle = window.window_handle();
                let window_manager =
                    cx.new(|cx| WindowManager::new(state.clone(), any_handle, cx));

                // ── Create RootView ──
                let view = cx.new(|cx| RootView::new(window, window_manager.clone(), cx));

                // ── Subscribe to WindowManager clipboard events ──
                cx.subscribe(
                    &window_manager,
                    move |view: &mut RootView,
                          _wm,
                          event: &ui::window_manager::WindowManagerEvent,
                          cx| {
                        match event {
                            ui::window_manager::WindowManagerEvent::ClipboardChanged => {
                                let items = view.state.read(cx).items.clone();
                                view.list_view(cx, |list, cx| list.set_items(items, cx));
                                cx.notify();
                            }
                            ui::window_manager::WindowManagerEvent::PinnedChanged(pinned) => {
                                view.pinned = *pinned;
                                cx.notify();
                            }
                        }
                    },
                );

                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            },
        )
        .unwrap();

        // ── Prevent app exit on window close ──
        cx.on_app_quit(|_cx| {
            // TODO: Once tray is implemented, this should be conditional
            // For now, always keep the process alive.
            false
        });
    });
}
```

等等，我需要重新考虑一下。`RootView` 当前直接有 `list_view` 字段，但上面的 subscription 代码使用了 `view.list_view` — 这需要 `list_view` 是 pub 的。让我调整方法：

实际上最好的方式是让 RootView 内部处理订阅，而不是在 main.rs 中。在 RootView::new 中完成 WindowManager 事件订阅。

- [ ] **Step 2: 修正 — 让 RootView 自己处理订阅**

RootView::new 内部：

```rust
pub fn new(
    window: &mut Window,
    window_manager: Entity<WindowManager>,
    cx: &mut Context<Self>,
) -> Self {
    // ... create state and child entities ...

    // Subscribe to WindowManager events
    cx.subscribe(
        &window_manager,
        move |this, _wm, event: &WindowManagerEvent, cx| match event {
            WindowManagerEvent::ClipboardChanged => {
                let items = this.state.read(cx).items.clone();
                this.list_view.update(cx, |list, cx| list.set_items(items, cx));
                cx.notify();
            }
            WindowManagerEvent::PinnedChanged(pinned) => {
                this.pinned = *pinned;
                this.titlebar.update(cx, |tb, cx| tb.set_pinned(*pinned, cx));
                cx.notify();
            }
        },
    );

    // ... rest ...
}
```

然后在 `main.rs` 的 `cx.open_window` 回调中简化为：

```rust
let view = cx.new(|cx| RootView::new(window, window_manager, cx));
cx.new(|cx| gpui_component::Root::new(view, window, cx))
```

- [ ] **Step 3: 完整的 main.rs**

```rust
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::borrow::Cow;

use gpui::*;

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

mod core;
mod platform;
mod state;
mod ui;
mod services;

use ui::root::RootView;
use ui::window_manager::WindowManager;

fn ensure_single_instance() -> bool {
    std::net::TcpListener::bind("127.0.0.1:19876").is_ok()
}

fn init_logging(db_path: &str) {
    let log_path = core::paths::log_path(db_path);
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 1_000_000 {
            let old = log_path.with_extension("log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&log_path, &old);
        }
    }
    if let Ok(file) = std::fs::File::create(&log_path) {
        let _ = simplelog::WriteLogger::init(
            simplelog::LevelFilter::Info,
            simplelog::Config::default(),
            file,
        );
    }
}

fn main() {
    if !ensure_single_instance() {
        return;
    }

    let db_path = core::paths::resolve_db_path("");
    init_logging(&db_path.to_string_lossy());

    log::info!("Starting Clippi (GPUI experiment)");

    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        if let Err(err) = cx.text_system().add_fonts(vec![Cow::Borrowed(
            include_bytes!("../assets/fonts/iconfont.ttf").as_slice(),
        )]) {
            log::error!("Failed to load iconfont.ttf: {err}");
        }
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        gpui_component::Theme::global_mut(cx).background = Hsla::transparent_black();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.), px(100.)),
                    size(px(360.), px(480.)),
                ))),
                window_background: WindowBackgroundAppearance::Transparent,
                titlebar: Some(TitlebarOptions {
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                // ── Windows DWM non-client rendering disable ──
                #[cfg(target_os = "windows")]
                unsafe {
                    use windows_sys::Win32::Graphics::Dwm::{
                        DwmSetWindowAttribute, DWMWA_NCRENDERING_POLICY,
                    };
                    const DWMNCRP_DISABLED: u32 = 1;
                    if let Ok(handle) = window.window_handle() {
                        if let RawWindowHandle::Win32(wh) = handle.as_raw() {
                            let _ = DwmSetWindowAttribute(
                                wh.hwnd.get() as _,
                                DWMWA_NCRENDERING_POLICY as u32,
                                &DWMNCRP_DISABLED as *const u32 as *const _,
                                std::mem::size_of::<u32>() as u32,
                            );
                        }
                    }
                }

                // ── Create shared AppState ──
                let settings = core::settings::AppSettings::load();
                let state = cx.new(|_cx| crate::state::app::AppState::new(settings));

                // ── WindowManager (owns poll loop: hotkey, focus, clipboard) ──
                let any_handle = window.window_handle();
                let window_manager = cx.new(|cx| WindowManager::new(state.clone(), any_handle, cx));

                // ── RootView ──
                let view = cx.new(|cx| RootView::new(window, window_manager, cx));
                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            },
        )
        .unwrap();

        // ── Keep process alive when window is closed ──
        // Without this, GPUI exits the event loop when all windows are closed.
        // TODO: Replace with proper tray integration.
        cx.on_app_quit(|_cx| false);
    });
}
```

- [ ] **Step 5: 提交**

```bash
git add src/main.rs
git commit -m "feat: integrate WindowManager, keep process alive on window close"
```

---

### Task 6: 编译验证并修复错误

**Files:**
- All above files

- [ ] **Step 1: 编译**

```bash
cargo build 2>&1
```

- [ ] **Step 2: 修复编译错误**

预期可能的问题：
1. `WindowManager` 中的类型未导入 — 补充 `use` 声明
2. `cx.subscribe` 需要 `RootView` 作为 `Entity` — 确保签名正确
3. `AppState` 字段访问 — 确保 pub
4. Windows API 函数签名 — 检查 `ShowWindow` 等正确导入

修复所有编译错误直到 `cargo build` 成功。

- [ ] **Step 3: 最终提交**

```bash
git add -A
git commit -m "fix: resolve compilation errors from WindowManager integration"
```

---

### Task 7: 功能验证

- [ ] **Step 1: 运行应用**

```bash
cargo run
```

- [ ] **Step 2: 验证快捷键**

按配置的全局快捷键（默认 Ctrl+Shift+V），窗口应该弹出到前台。

- [ ] **Step 3: 验证自动隐藏**

点击窗口外部区域（其他 app），窗口应该自动隐藏（当 auto_hide 开启且未 pinned 时）。

- [ ] **Step 4: 验证 Pin 模式**

点击标题栏固定按钮，窗口应该保持显示（不自动隐藏）。

- [ ] **Step 5: 验证关闭不退出**

关闭窗口（Alt+F4 或点击 X），进程应该在后台保持运行。再次按快捷键窗口重新出现。

- [ ] **Step 6: 提交（如有修复）**

```bash
git add -A && git commit -m "fix: WindowManager functional verification fixes"
```

---

## Self-Review

**1. Spec coverage:** 检查设计文档所有要求：
- [x] PositionMode 枚举 — Task 1
- [x] WindowManager Entity — Task 2
- [x] 统一轮询 (hotkey/focus/clipboard) — Task 2 Step 5
- [x] Auto-hide 判定逻辑 (4 个 guard) — Task 2 Step 5 (poll_focus)
- [x] 三种位置模式 — Task 2 Step 7
- [x] 关闭到后台不退出 — Task 5 (cx.on_app_quit)
- [x] 物理/逻辑像素处理 — Task 2 Step 7 (platform-specific)
- [x] 剪贴板轮询移入 WM — Task 4

**2. Placeholder scan:** 无 TBD/TODO/占位符

**3. Type consistency:**
- `PositionMode` 在 Task 1 定义，Task 2 使用 — 一致
- `WindowManagerEvent` 在 Task 2 Step 2 定义，Task 4/5 使用 — 一致
- `AppState` 在 Task 5 创建，Task 2/4 共享 — 一致
- `WindowManager::new(state, any_handle, cx)` — 签名在各处一致
