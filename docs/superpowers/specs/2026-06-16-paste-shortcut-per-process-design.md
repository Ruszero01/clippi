# Per-Process Paste Shortcut + Smart Detection

## Overview

粘贴快速粘贴（双击/回车）时，根据目标前台应用的进程类型自动选择正确的粘贴快捷键，同时允许用户在设置中为特定进程手动配置粘贴快捷键。解决终端类应用不接受 Ctrl+V 的问题，零配置默认可用，高级用户可覆盖。

## Motivation

Windows 终端程序（Windows Terminal、conhost、ConEmu 等）通常不支持 Ctrl+V 粘贴，需要使用 Shift+Insert。当前 Clippi 硬编码 Ctrl+V，导致在终端中粘贴失败。论坛用户反馈了此问题并建议按进程配置快捷键。

## Design

### Paste Decision Flow

```
粘贴触发
  ↓
获取目标 HWND → 提取进程名
  ↓
查 paste_shortcut_map（用户配置，优先）
  ↓ 有匹配
使用用户配置的快捷键
  ↓ 无匹配
is_console_window(hwnd)?
  ↓ 是
Shift+Insert
  ↓ 否
Ctrl+V（默认）
```

### Data Model

#### New structs in `src/core/settings.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasteShortcutEntry {
    pub app_name: String,       // 进程名
    pub shortcut: String,       // 如 "Shift+Insert", "Ctrl+Shift+V"
}
```

#### New field in `AppSettings`

```rust
#[serde(default)]
pub paste_shortcut_map: Vec<PasteShortcutEntry>,
```

Vec 顺序查找，后添加的同名条目覆盖先前的。

### Smart Terminal Detection (`src/platform/paste.rs`)

新增 `is_console_window(hwnd: HWND) -> bool`：

- 检查窗口类名：`ConsoleWindowClass`（conhost）、`CASCADIA_HOSTING_WINDOW_CLASS`（Windows Terminal）
- 轻量级，仅 `GetClassNameW` + 字符串比较，开销 < 10µs

`wait_for_focus_and_send_ctrl_v` 改为接受快捷键参数，拆出 `send_paste_keystroke(use_shift_insert: bool)`。

### UI Refactoring (`src/ui/settings/hotkey.rs`)

#### Layout (由上到下)

```
┌─ Hotkey Recording Card (66px) ─────────────┐
│ 全局热键                       [Alt+V]      │
├─ Foreground App Info Card (44px) ───────────┤  ← render_foreground_app_bar() 拆为独立方法
│ [图标] AppName — WindowTitle   [⊞] [⊘]     │     两个按钮: 粘贴快捷键 / 黑名单
├─────────────────────────────────────────────┤
│ 黑名单                                      │  ← 文字标签
│ ┌─ List Box (动态高度, max 160px) ───────┐  │
│ │ [图标] App1                      [✕]   │  │
│ │ 空态: "点击 ⊘ 将当前应用加入黑名单"    │  │
│ └────────────────────────────────────────ｖ  │
├─────────────────────────────────────────────┤
│ 粘贴快捷键                                  │  ← 文字标签
│ ┌─ List Box (动态高度, max 160px) ───────┐  │
│ │ [图标] App1    [Shift+Insert]    [✕]   │  │
│ │ 空态: "点击 ⊞ 为当前应用设置粘贴快捷键" │  │
│ └────────────────────────────────────────┘  │
└─────────────────────────────────────────────┘
```

#### 拆出的共享方法

- `render_foreground_app_bar(theme, fg_info, on_paste_shortcut, on_blacklist)` — app icon + name + 两个操作按钮
- `render_app_list_section(title, empty_hint, entries, render_right)` — 标签 + 动态高度列表

### Events (`HotkeyConfirmAction` 扩展)

```rust
pub enum HotkeyConfirmAction {
    AddBlacklist { app_name: String },
    RemoveBlacklist { app_name: String },
    AddPasteShortcut { app_name: String, shortcut: String },
    RemovePasteShortcut { app_name: String },
}
```

### Recording

粘贴快捷键录制复用 `WindowManager.start_hotkey_recording()` 和 `poll_recording_pressed()`，新增 `is_recording_paste_shortcut: bool` 区分录制目标。录制完成后写入 `paste_shortcut_map`，触发 `settings.save()`。

### Files Changed

| File | Change |
|------|--------|
| `src/core/settings.rs` | `PasteShortcutEntry` struct + `paste_shortcut_map` field + serialization |
| `src/ui/settings/hotkey.rs` | Refactor: extract shared component methods, add paste shortcut list section |
| `src/ui/settings/mod.rs` | New `SettingsEvent` variants, handle paste shortcut confirm lifecycle |
| `src/ui/window_manager.rs` | New recording state (`is_recording_paste_shortcut`), paste-shortcut-aware poll logic |
| `src/ui/root.rs` | Handle new `HotkeyConfirmAction` variants |
| `src/platform/paste.rs` | `is_console_window()`, parameterized `send_paste_keystroke()`, decision flow |
| `src/core/i18n_keys.rs` | New i18n keys for paste shortcut UI text |
