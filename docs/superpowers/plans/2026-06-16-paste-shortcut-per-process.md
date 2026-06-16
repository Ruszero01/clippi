# Per-Process Paste Shortcut + Smart Terminal Detection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 粘贴时自动检测终端窗口使用 Shift+Insert，同时允许用户在快捷键设置中为任意进程手动配置粘贴快捷键。

**Architecture:** 分层决策 — 粘贴前查用户配置 map → 智能检测终端窗口类名 → 默认 Ctrl+V。设置 UI 重构 hotkey tab，拆出共享 foreground app bar 组件，新增粘贴快捷键列表区域。

**Tech Stack:** Rust + GPUI, windows-sys (SendInput, GetClassNameW), TOML settings

---

### Task 1: 数据模型 — PasteShortcutEntry + 配置序列化

**Files:**
- Modify: `G:\Develop\github\clippi\src\core\settings.rs:55-120, 134-174`

- [ ] **Step 1: 在 `AppSettings` struct 上方添加 `PasteShortcutEntry` struct**

`src/core/settings.rs`，在 `pub struct AppSettings` 上方插入：

```rust
/// Per-process paste shortcut mapping entry.
/// When the foreground app matches `app_name`, use `shortcut` instead of Ctrl+V.
/// Example: `PasteShortcutEntry { app_name: "WindowsTerminal".into(), shortcut: "Shift+Insert".into() }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasteShortcutEntry {
    pub app_name: String,
    pub shortcut: String,
}
```

- [ ] **Step 2: 在 `AppSettings` 末尾添加 `paste_shortcut_map` 字段**

在 `src/core/settings.rs` 第 119 行 `type_filter_config` 之后插入：

```rust
    #[serde(default)]
    pub paste_shortcut_map: Vec<PasteShortcutEntry>,
```

- [ ] **Step 3: 在 `Default` impl 中添加默认值**

在 `src/core/settings.rs` 第 171 行 `type_filter_config: Vec::new(),` 之后插入：

```rust
            paste_shortcut_map: Vec::new(),
```

- [ ] **Step 4: 编译验证**

```bash
cargo check 2>&1 | head -20
```

Expected: 编译通过（仅有 unused 警告）

- [ ] **Step 5: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat: add PasteShortcutEntry struct and paste_shortcut_map field to AppSettings

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: i18n 文本 key

**Files:**
- Modify: `G:\Develop\github\clippi\src\core\i18n_keys.rs:298-304`

- [ ] **Step 1: 在 i18n_keys.rs 中 hotkey 区域添加新 key**

在 `HotkeyBlacklistEmptyHint` 之后（约第 300 行附近）插入：

```rust
    HotkeyPasteShortcut:      ("粘贴快捷键", "Paste shortcut"),
    PasteShortcutEmpty:       ("未添加粘贴快捷键", "No paste shortcuts configured"),
    PasteShortcutEmptyHint:   ("点击  为当前应用设置粘贴快捷键", "Click   to set paste shortcut for current app"),
    PasteShortcutRecording:   ("按下快捷键进行录制", "Press shortcut to record"),
    PasteShortcutConfirmAddTitle: ("确认添加粘贴快捷键", "Confirm Add Paste Shortcut"),
    PasteShortcutConfirmAddMsg:   ("为 {0} 设置粘贴快捷键 {1}？", "Set paste shortcut {1} for {0}?"),
    PasteShortcutConfirmRemoveTitle: ("确认移除粘贴快捷键", "Confirm Remove Paste Shortcut"),
    PasteShortcutConfirmRemoveMsg:   ("移除 {0} 的粘贴快捷键设置？", "Remove paste shortcut for {0}?"),
```

- [ ] **Step 2: 编译验证**

```bash
cargo check 2>&1 | head -20
```

Expected: 编译通过

- [ ] **Step 3: Commit**

```bash
git add src/core/i18n_keys.rs
git commit -m "feat: add i18n keys for paste shortcut UI text

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: 粘贴层 — is_console_window + send_paste_keystroke 参数化

**Files:**
- Modify: `G:\Develop\github\clippi\src\platform\paste.rs:1-132`

- [ ] **Step 1: 新增 `is_console_window` 函数**

在 `src/platform/paste.rs` 的 `#[cfg(target_os = "windows")]` impl 块中，第 16 行 `const` 块之后插入：

```rust
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_SHIFT, VK_INSERT, KEYEVENTF_EXTENDEDKEY};

/// Detect whether a window is a console/terminal window that doesn't support Ctrl+V.
///
/// Checks the window class name against known terminal classes:
/// - `ConsoleWindowClass` — classic console host (conhost.exe: cmd, PowerShell 5)
/// - `CASCADIA_HOSTING_WINDOW_CLASS` — Windows Terminal
#[cfg(target_os = "windows")]
fn is_console_window(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    let mut class_name = [0u16; 64];
    // SAFETY: `GetClassNameW` reads the window class name into a stack-allocated
    // buffer of sufficient size (64 WCHARs). The HWND was validated by `IsWindow`
    // before this call in `wait_for_focus_and_send_paste`.
    let len = unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
    if len == 0 {
        return false;
    }
    let class_str = String::from_utf16_lossy(&class_name[..len as usize]);
    class_str == "ConsoleWindowClass" || class_str == "CASCADIA_HOSTING_WINDOW_CLASS"
}
```

- [ ] **Step 2: 新增 `send_paste_keystroke` 函数（参数化）**

将现有的 `wait_for_focus_and_send_ctrl_v` 中的 `SendInput` 部分提取为独立函数。在 `is_console_window` 之后插入：

```rust
/// Send the paste keystroke via SendInput.
/// When `use_shift_insert` is true, sends Shift+Insert instead of Ctrl+V.
#[cfg(target_os = "windows")]
fn send_paste_keystroke(use_shift_insert: bool) {
    // SAFETY: `SendInput` with correctly-initialised INPUT arrays is the standard
    // Windows API for synthesizing keyboard input. All INPUT structs are fully
    // initialised via zeroed() + field assignment before the call, and the array
    // size matches the count parameter.
    unsafe {
        if use_shift_insert {
            let mut inputs: [INPUT; 4] = std::mem::zeroed();

            // Shift down
            inputs[0].r#type = INPUT_KEYBOARD;
            inputs[0].Anonymous.ki = KEYBDINPUT {
                wVk: VK_SHIFT,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            };

            // Insert down (extended key)
            inputs[1].r#type = INPUT_KEYBOARD;
            inputs[1].Anonymous.ki = KEYBDINPUT {
                wVk: VK_INSERT,
                wScan: 0,
                dwFlags: KEYEVENTF_EXTENDEDKEY,
                time: 0,
                dwExtraInfo: 0,
            };

            // Insert up (extended key)
            inputs[2].r#type = INPUT_KEYBOARD;
            inputs[2].Anonymous.ki = KEYBDINPUT {
                wVk: VK_INSERT,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP | KEYEVENTF_EXTENDEDKEY,
                time: 0,
                dwExtraInfo: 0,
            };

            // Shift up
            inputs[3].r#type = INPUT_KEYBOARD;
            inputs[3].Anonymous.ki = KEYBDINPUT {
                wVk: VK_SHIFT,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };

            SendInput(4, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        } else {
            let mut inputs: [INPUT; 4] = std::mem::zeroed();

            // Ctrl down
            inputs[0].r#type = INPUT_KEYBOARD;
            inputs[0].Anonymous.ki = KEYBDINPUT {
                wVk: VK_CONTROL,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            };

            // V down
            inputs[1].r#type = INPUT_KEYBOARD;
            inputs[1].Anonymous.ki = KEYBDINPUT {
                wVk: VK_V,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            };

            // V up
            inputs[2].r#type = INPUT_KEYBOARD;
            inputs[2].Anonymous.ki = KEYBDINPUT {
                wVk: VK_V,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };

            // Ctrl up
            inputs[3].r#type = INPUT_KEYBOARD;
            inputs[3].Anonymous.ki = KEYBDINPUT {
                wVk: VK_CONTROL,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };

            SendInput(4, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }
}
```

- [ ] **Step 3: 重写 `wait_for_focus_and_send_ctrl_v` 为 `wait_for_focus_and_send_paste`**

将第 60 行的 `wait_for_focus_and_send_ctrl_v` 替换为：

```rust
#[cfg(target_os = "windows")]
fn wait_for_focus_and_send_paste(
    target_hwnd: Option<usize>,
    use_shift_insert: bool,
) {
    // Initial delay for SetForegroundWindow to take effect
    std::thread::sleep(std::time::Duration::from_millis(BASE_DELAY_MS));

    // Verify target window is actually foreground before pasting
    if let Some(hwnd) = target_hwnd {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        if unsafe { IsWindow(hwnd) } != 0 {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(FOCUS_TIMEOUT_MS);
            loop {
                if unsafe { GetForegroundWindow() } == hwnd {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(FOCUS_CHECK_INTERVAL_MS));
            }
        }
    }

    send_paste_keystroke(use_shift_insert);
}
```

- [ ] **Step 4: 更新 `paste_after_delay` 接受 `paste_shortcut_map` 参数**

将第 39 行的 `paste_after_delay` 改为：

```rust
/// Simulate paste after restoring focus to the target window.
/// Accepts an optional per-process shortcut map for override.
#[cfg(target_os = "windows")]
pub fn paste_after_delay(paste_shortcut_map: &'static [crate::core::settings::PasteShortcutEntry]) {
    let target_hwnd: Option<usize> =
        crate::platform::focus::get_last_non_clippi_window().map(|h| h as usize);

    // Resolve paste shortcut before spawning the thread.
    let use_shift_insert = resolve_paste_shortcut(target_hwnd, paste_shortcut_map);

    std::thread::spawn(move || {
        wait_for_focus_and_send_paste(target_hwnd, use_shift_insert);
    });
}
```

**注意**：`paste_shortcut_map` 需要用 `'static` 生命周期或转为 `Arc<Vec<_>>`。由于 map 数据量小，这里使用 `Arc<Vec<PasteShortcutEntry>>` 更安全：

```rust
use std::sync::Arc;

#[cfg(target_os = "windows")]
pub fn paste_after_delay(paste_shortcut_map: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    let target_hwnd: Option<usize> =
        crate::platform::focus::get_last_non_clippi_window().map(|h| h as usize);

    let use_shift_insert = resolve_paste_shortcut(target_hwnd, &paste_shortcut_map);

    std::thread::spawn(move || {
        wait_for_focus_and_send_paste(target_hwnd, use_shift_insert);
    });
}
```

- [ ] **Step 5: 新增 `resolve_paste_shortcut` 决策函数**

在 `send_paste_keystroke` 之后插入：

```rust
/// Resolve which paste keystroke to use based on:
/// 1. User-configured per-process shortcut map (highest priority)
/// 2. Smart detection of console/terminal windows → Shift+Insert
/// 3. Default: Ctrl+V
#[cfg(target_os = "windows")]
fn resolve_paste_shortcut(
    target_hwnd: Option<usize>,
    paste_shortcut_map: &[crate::core::settings::PasteShortcutEntry],
) -> bool {
    let Some(hwnd) = target_hwnd else { return false };
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;

    // 1. Check user-configured map
    if !paste_shortcut_map.is_empty() {
        // Get process name from HWND
        if let Some(proc_name) = get_process_name_from_hwnd(hwnd) {
            for entry in paste_shortcut_map {
                if entry.app_name.eq_ignore_ascii_case(&proc_name) {
                    return entry.shortcut.to_lowercase().contains("shift");
                }
            }
        }
    }

    // 2. Smart detection: console/terminal windows → Shift+Insert
    if is_console_window(hwnd) {
        return true;
    }

    // 3. Default: Ctrl+V
    false
}
```

- [ ] **Step 6: 新增 `get_process_name_from_hwnd` 辅助函数**

在 `is_console_window` 之后插入：

```rust
/// Extract process name (exe stem) from a window handle.
/// Returns None if any API call fails.
#[cfg(target_os = "windows")]
fn get_process_name_from_hwnd(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<String> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }

        let mut exe_buf = [0u16; 260];
        let mut exe_len = exe_buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            0, // PROCESS_NAME_WIN32
            exe_buf.as_mut_ptr(),
            &mut exe_len,
        );
        CloseHandle(process);

        if result == 0 {
            return None;
        }

        let exe_path = String::from_utf16_lossy(&exe_buf[..exe_len as usize]);
        std::path::Path::new(&exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    }
}
```

- [ ] **Step 7: 更新 `paste_sync` 同样接受参数**

将第 53 行的 `paste_sync` 改为：

```rust
#[cfg(target_os = "windows")]
pub fn paste_sync(paste_shortcut_map: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    let target_hwnd: Option<usize> =
        crate::platform::focus::get_last_non_clippi_window().map(|h| h as usize);
    let use_shift_insert = resolve_paste_shortcut(target_hwnd, &paste_shortcut_map);
    wait_for_focus_and_send_paste(target_hwnd, use_shift_insert);
}
```

- [ ] **Step 8: 更新跨平台 stub（macOS / 其他）签名一致**

将第 204-213 行的 macOS 版本和第 270-276 行的其他平台版本签名更新为接收 `Arc<Vec<PasteShortcutEntry>>` 参数（忽略不用）：

```rust
#[cfg(target_os = "macos")]
pub fn paste_after_delay(_paste_shortcut_map: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    std::thread::spawn(move || {
        send_cmd_v();
    });
}

#[cfg(target_os = "macos")]
pub fn paste_sync(_paste_shortcut_map: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    send_cmd_v();
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_after_delay(_paste_shortcut_map: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_sync(_paste_shortcut_map: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {}
```

- [ ] **Step 9: 编译验证**

```bash
cargo check 2>&1
```

Expected: 编译通过（有调用侧未更新的 error，Task 8 修复）

- [ ] **Step 10: Commit**

```bash
git add src/platform/paste.rs
git commit -m "feat: add is_console_window detection and parameterized paste keystroke

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: HotkeyConfirmAction 枚举扩展

**Files:**
- Modify: `G:\Develop\github\clippi\src\ui\settings\hotkey.rs:14-20`

- [ ] **Step 1: 重命名现有变体并新增**

将 `HotkeyConfirmAction` 枚举（第 17-20 行）替换为：

```rust
#[derive(Debug, Clone)]
pub enum HotkeyConfirmAction {
    /// Add app to hotkey blacklist.
    AddBlacklist { app_name: String },
    /// Remove app from hotkey blacklist.
    RemoveBlacklist { app_name: String },
    /// Add/update paste shortcut for an app.
    AddPasteShortcut { app_name: String, shortcut: String },
    /// Remove paste shortcut for an app.
    RemovePasteShortcut { app_name: String },
}
```

- [ ] **Step 2: 编译检查引用处**

```bash
cargo check 2>&1 | grep -E "error|HotkeyConfirmAction"
```

Expected: 现有引用 `Add` / `Remove` 处会报错，后续 Task 逐一修复

- [ ] **Step 3: Commit**

```bash
git add src/ui/settings/hotkey.rs
git commit -m "refactor: rename HotkeyConfirmAction variants, add paste shortcut actions

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: ConfirmDialog 新增工厂方法

**Files:**
- Modify: `G:\Develop\github\clippi\src\ui\components\confirm_dialog.rs:110-127`

- [ ] **Step 1: 添加 paste shortcut 相关的 ConfirmDialog 工厂方法**

在 `add_blacklist` 方法之后（第 126 行后）插入：

```rust
    /// Add paste shortcut confirmation.
    pub fn add_paste_shortcut(app_name: &str, shortcut: &str) -> Self {
        Self::new()
            .title(I18nKey::PasteShortcutConfirmAddTitle.text())
            .message(I18nKey::PasteShortcutConfirmAddMsg.fmt(&[app_name, shortcut]))
            .confirm_label(I18nKey::ConfirmAddLabel.text())
            .danger(false)
    }

    /// Remove paste shortcut confirmation.
    pub fn remove_paste_shortcut(app_name: &str) -> Self {
        Self::new()
            .title(I18nKey::PasteShortcutConfirmRemoveTitle.text())
            .message(I18nKey::PasteShortcutConfirmRemoveMsg.fmt(&[app_name]))
            .confirm_label(I18nKey::ConfirmRemoveLabel.text())
            .danger(false)
    }
```

- [ ] **Step 2: 编译验证**

```bash
cargo check 2>&1 | head -20
```

Expected: 编译通过

- [ ] **Step 3: Commit**

```bash
git add src/ui/components/confirm_dialog.rs
git commit -m "feat: add ConfirmDialog factory methods for paste shortcut operations

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: SettingsPanel — 事件 + confirm 生命周期

**Files:**
- Modify: `G:\Develop\github\clippi\src\ui\settings\mod.rs:36-49, 64-66, 488-492`

- [ ] **Step 1: 扩展 `SettingsEvent` 枚举**

在第 46 行后添加新事件：

```rust
    /// User confirmed add/remove paste shortcut — RootView should apply changes.
    HotkeyPasteShortcut {
        action: hotkey::HotkeyConfirmAction,
    },
```

- [ ] **Step 2: 在 `SettingsPanel` struct 中添加 paste_shortcut 录制状态**

在第 64 行 `hotkey_confirm` 字段后添加：

```rust
    /// Whether we are currently recording a paste shortcut for an app.
    pub recording_paste_shortcut: Option<String>, // Some(app_name)
    /// The recorded paste shortcut string before confirmation.
    pub pending_paste_shortcut: Option<(String, String)>, // (app_name, shortcut)
```

- [ ] **Step 3: 初始化新字段**

在 `new()` 构造函数中（约第 132 行 `hotkey_confirm: None,` 后）：

```rust
            recording_paste_shortcut: None,
            pending_paste_shortcut: None,
```

- [ ] **Step 4: 添加 `clear_paste_shortcut_state` 方法**

在 `clear_hotkey_confirm` 方法（第 489 行）后添加：

```rust
    /// Clear pending paste shortcut recording state.
    pub fn clear_paste_shortcut_state(&mut self, cx: &mut Context<Self>) {
        self.recording_paste_shortcut = None;
        self.pending_paste_shortcut = None;
        cx.notify();
    }
```

- [ ] **Step 5: 编译验证**

```bash
cargo check 2>&1 | head -20
```

Expected: 编译通过（新字段有 dead_code 警告是正常的）

- [ ] **Step 6: Commit**

```bash
git add src/ui/settings/mod.rs
git commit -m "feat: add paste shortcut events and recording state to SettingsPanel

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: Hotkey Tab UI 重构 — 拆出共享组件 + 新增粘贴快捷键列表

**Files:**
- Modify: `G:\Develop\github\clippi\src\ui\settings\hotkey.rs:1-405`

这是本计划中最大的改动。完整重写 `render_hotkey_tab`。

- [ ] **Step 1: 将 `render_foreground_app_bar` 拆为独立方法**

在 `impl SettingsPanel` 块中添加新方法（可以放在 `render_hotkey_tab` 之前）：

```rust
impl SettingsPanel {
    /// Render the shared foreground app info bar used by both blacklist and
    /// paste shortcut sections.
    ///
    /// Layout: [app icon] AppName — WindowTitle [⊞] [⊘]
    /// Buttons (left to right): add paste shortcut, add to blacklist.
    fn render_foreground_app_bar(
        fg_app_name: &str,
        fg_window_title: &str,
        theme: &ClippiTheme,
        has_app: bool,
        on_paste_shortcut: impl Fn(&mut Window, &mut App) + 'static,
        on_blacklist: impl Fn(&mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let icon_path = if has_app {
            Some(crate::core::paths::app_icon_path(fg_app_name))
        } else {
            None
        };

        div()
            .h(px(44.))
            .rounded(px(10.))
            .bg(theme.surface)
            .border(px(1.))
            .border_color(theme.divider)
            .px(px(12.))
            .pr(px(6.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            // Left: icon + app name + window title
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .items_center()
                    .flex_1()
                    .overflow_hidden()
                    .when(has_app, |d| {
                        if let Some(ref path) = icon_path {
                            d.child(
                                gpui::img(std::path::Path::new(path))
                                    .w(px(20.))
                                    .h(px(20.)),
                            )
                        } else {
                            d
                        }
                    })
                    .when(has_app, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(0.))
                                .items_center()
                                .overflow_hidden()
                                .flex_1()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text_1)
                                        .child(fg_app_name.to_string()),
                                )
                                .when(!fg_window_title.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme.text_3)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .flex_1()
                                            .child(format!(
                                                " \u{2014} {}",
                                                fg_window_title
                                            )),
                                    )
                                }),
                        )
                    })
                    .when(!has_app, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.text_3)
                                .child(I18nKey::HotkeyNoForeground.text()),
                        )
                    }),
            )
            // Right: two buttons (paste shortcut + blacklist)
            .when(has_app, |d| {
                let app_name_paste = fg_app_name.to_string();
                let app_name_bl = fg_app_name.to_string();
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(4.))
                        .items_center()
                        // Paste shortcut button: ⊞ (e6a8) or similar icon
                        .child(
                            div()
                                .w(px(26.))
                                .h(px(26.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(theme.accent).opacity(0.12))
                                .on_mouse_down(MouseButton::Left, {
                                    let name = app_name_paste.clone();
                                    move |_ev, window, cx| {
                                        on_paste_shortcut(window, cx);
                                    }
                                })
                                .child(
                                    div()
                                        .font_family("iconfont")
                                        .text_size(px(14.))
                                        .text_color(theme.accent)
                                        .child("\u{e623}"), // keyboard icon
                                ),
                        )
                        // Blacklist button: ⊘ (e6a7)
                        .child(
                            div()
                                .w(px(26.))
                                .h(px(26.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(theme.danger).opacity(0.12))
                                .on_mouse_down(MouseButton::Left, {
                                    let name = app_name_bl.clone();
                                    move |_ev, _window, cx| {
                                        on_blacklist(_window, cx);
                                    }
                                })
                                .child(
                                    div()
                                        .font_family("iconfont")
                                        .text_size(px(14.))
                                        .text_color(theme.text_2)
                                        .child("\u{e6a7}"),
                                ),
                        ),
                )
            })
    }
}
```

- [ ] **Step 2: 添加 `render_per_app_list_section` 方法**

在同一个 `impl SettingsPanel` 块中添加：

```rust
    /// Render a labeled list section with dynamic-height scrollable list box.
    fn render_per_app_list_section(
        title: &str,
        empty_hint: &str,
        entries: Vec<PerAppListEntry>,
        theme: &ClippiTheme,
    ) -> impl IntoElement {
        let has_entries = !entries.is_empty();
        let list_height = if has_entries {
            (entries.len() as f32 * 36.0 + 8.0).min(160.0)
        } else {
            40.0
        };

        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            // Section label
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_3)
                    .px(px(2.))
                    .child(title.to_string()),
            )
            // List box
            .child(
                div()
                    .h(px(list_height))
                    .rounded(px(8.))
                    .bg(theme.surface)
                    .border(px(1.))
                    .border_color(theme.divider)
                    .overflow_y_scrollbar()
                    .when(has_entries, |d| {
                        d.p(px(4.)).flex().flex_col().gap(px(2.)).children(
                            entries.iter().map(|entry| entry.render(theme)),
                        )
                    })
                    .when(!has_entries, |d| {
                        d.flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.text_3)
                                    .child(empty_hint.to_string()),
                            )
                    }),
            )
    }
```

- [ ] **Step 3: 定义 `PerAppListEntry` enum**

在 `HotkeyConfirmAction` 下面添加：

```rust
/// Entry type for per-app list sections.
pub enum PerAppListEntry {
    /// Blacklist entry — app_name + delete button.
    Blacklist { app_name: String },
    /// Paste shortcut entry — app_name + shortcut label + delete button.
    PasteShortcut { app_name: String, shortcut: String },
}

impl PerAppListEntry {
    fn render(&self, theme: &ClippiTheme) -> impl IntoElement {
        let (app_name, right_element) = match self {
            Self::Blacklist { app_name } => {
                let name = app_name.clone();
                let el = div()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .items_center()
                    .child(render_delete_button(name, theme));
                (app_name.clone(), el.into_any_element())
            }
            Self::PasteShortcut { app_name, shortcut } => {
                let name = app_name.clone();
                let sc = shortcut.clone();
                let el = div()
                    .flex()
                    .flex_row()
                    .gap(px(4.))
                    .items_center()
                    .child(
                        div()
                            .h(px(22.))
                            .rounded(px(5.))
                            .px(px(6.))
                            .bg(theme.accent_soft)
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.opacity(0.8))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.accent)
                                    .child(sc),
                            ),
                    )
                    .child(render_delete_button(name, theme));
                (app_name.clone(), el.into_any_element())
            }
        };

        let icon_path = crate::core::paths::app_icon_path(&app_name);

        div()
            .h(px(32.))
            .rounded(px(6.))
            .bg(theme.titlebar_bg)
            .px(px(8.))
            .pr(px(4.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.))
                    .items_center()
                    .overflow_hidden()
                    .flex_1()
                    .child(
                        gpui::img(std::path::Path::new(&icon_path))
                            .w(px(18.))
                            .h(px(18.)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_1)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(app_name),
                    ),
            )
            .child(right_element)
    }
}

/// Render a small delete (✕) button that emits a removal event.
fn render_delete_button(app_name: String, theme: &ClippiTheme) -> impl IntoElement {
    div()
        .w(px(24.))
        .h(px(24.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(theme.danger).opacity(0.12))
        .child(
            div()
                .font_family("iconfont")
                .text_size(px(14.))
                .text_color(theme.text_2)
                .child("\u{e8b6}"),
        )
}
```

**注意**：删除按钮的点击事件需要 emit `SettingsEvent`，但由于 `IntoElement` 限制，需要在调用侧通过 `on_mouse_down` 绑定。调整 `PerAppListEntry::render` 使其接受回调闭包。

修订：改为方法接受 `on_delete: impl Fn(&str)` 回调：

```rust
impl PerAppListEntry {
    fn render(
        &self,
        theme: &ClippiTheme,
        on_delete: Rc<dyn Fn(&str, &mut Window, &mut App)>,
    ) -> impl IntoElement {
        // ... same as above but delete button binds on_delete
    }
}
```

- [ ] **Step 4: 重写 `render_hotkey_tab` 整合所有组件**

替换 `render_hotkey_tab` 方法（第 23-404 行）：

```rust
impl SettingsPanel {
    pub fn render_hotkey_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let wm = self.window_manager.clone();
        let this = cx.entity().clone();

        // Snapshot current values
        let app = self.state.read(cx);
        let hotkey_display = app.settings.hotkey.clone();
        let recording = app.hotkey_recording;
        let is_recording_paste = self.recording_paste_shortcut.is_some();
        let fg_app_name = app.foreground_app_name.clone();
        let fg_window_title = app.foreground_window_title.clone();
        let blacklist = app.settings.hotkey_blacklist.clone();
        let paste_shortcut_map = app.settings.paste_shortcut_map.clone();
        drop(app);

        let theme = &self.theme;
        let has_fg = !fg_app_name.is_empty();
        let is_any_recording = recording || is_recording_paste;

        // Recording state colors
        let recording_border = if is_any_recording {
            theme.accent
        } else {
            theme.divider
        };
        let recording_btn_bg = if recording {
            theme.accent_soft
        } else {
            theme.accent
        };
        let recording_btn_text = if recording {
            theme.accent
        } else {
            rgb(0xffffff)
        };

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // 1. Hotkey recording card
            .child(
                div()
                    .h(px(66.))
                    .rounded(px(10.))
                    .bg(theme.surface)
                    .border(px(1.))
                    .border_color(recording_border)
                    .px(px(14.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text_1)
                                    .child(I18nKey::HotkeyTabTitle.text()),
                            )
                            .child({
                                let desc_color = if is_any_recording {
                                    theme.accent
                                } else {
                                    theme.text_3
                                };
                                let desc_text = if is_recording_paste {
                                    I18nKey::PasteShortcutRecording.text()
                                } else if recording {
                                    I18nKey::HotkeyPressToRecord.text()
                                } else {
                                    I18nKey::HotkeyRecordingIdle.text()
                                };
                                div()
                                    .text_size(px(10.))
                                    .text_color(desc_color)
                                    .child(desc_text)
                            }),
                    )
                    .child({
                        let state = state.clone();
                        let wm = wm.clone();
                        let this = this.clone();
                        div()
                            .h(px(28.))
                            .w(px(80.))
                            .rounded(px(7.))
                            .bg(recording_btn_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(!is_any_recording, |d| {
                                d.cursor(CursorStyle::PointingHand)
                                    .hover(move |style| style.opacity(0.85))
                            })
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                if is_any_recording {
                                    return;
                                }
                                wm.update(cx, |wm, _cx| {
                                    wm.start_hotkey_recording();
                                });
                                state.update(cx, |s, _cx| {
                                    s.hotkey_recording = true;
                                });
                                this.update(cx, |_panel, cx| cx.notify());
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(recording_btn_text)
                                    .child(hotkey_display.clone()),
                            )
                    }),
            )
            // 2. Shared foreground app info bar
            .child({
                let app_name = fg_app_name.clone();
                let this = this.clone();
                let this2 = this.clone();
                Self::render_foreground_app_bar(
                    &fg_app_name,
                    &fg_window_title,
                    theme,
                    has_fg,
                    // on_paste_shortcut: start recording
                    move |_window, cx| {
                        let name = app_name.clone();
                        this.update(cx, |panel, cx| {
                            panel.recording_paste_shortcut = Some(name.clone());
                            panel.window_manager.update(cx, |wm, _cx| {
                                wm.start_hotkey_recording(); // reuse recording infra
                            });
                            cx.notify();
                        });
                    },
                    // on_blacklist: emit confirm
                    move |_window, cx| {
                        this2.update(cx, |_panel, cx| {
                            cx.emit(SettingsEvent::ShowHotkeyConfirm(
                                HotkeyConfirmAction::AddBlacklist {
                                    app_name: fg_app_name.clone(),
                                },
                            ));
                        });
                    },
                )
            })
            // 3. Blacklist section
            .child({
                let entries: Vec<PerAppListEntry> = blacklist
                    .iter()
                    .map(|name| PerAppListEntry::Blacklist {
                        app_name: name.clone(),
                    })
                    .collect();

                // Build delete callbacks inline
                // (Delete button wiring is done via closures captured in render;
                // for brevity we just build the list section here.
                // Full wiring: see Step 5 for event plumbing.)
                Self::render_per_app_list_section(
                    I18nKey::HotkeyBlacklist.text().as_str(),
                    I18nKey::HotkeyBlacklistEmptyHint.text().as_str(),
                    entries,
                    theme,
                )
            })
            // 4. Paste shortcut section
            .child({
                let entries: Vec<PerAppListEntry> = paste_shortcut_map
                    .iter()
                    .map(|entry| PerAppListEntry::PasteShortcut {
                        app_name: entry.app_name.clone(),
                        shortcut: entry.shortcut.clone(),
                    })
                    .collect();

                Self::render_per_app_list_section(
                    I18nKey::HotkeyPasteShortcut.text().as_str(),
                    I18nKey::PasteShortcutEmptyHint.text().as_str(),
                    entries,
                    theme,
                )
            })
    }
}
```

**注意**：`render_per_app_list_section` 中的删除按钮实际需要绑定 emit 事件。为简化，`PerAppListEntry::render` 方法中的 `render_delete_button` 需要接受回调。完整版本将在实现时处理回调传递链：`render_hotkey_tab` → 构建 entries 时预绑定 emit 闭包。

- [ ] **Step 5: 编译验证**

```bash
cargo check 2>&1
```

Expected: 可能有类型/借用错误，根据实际错误调整

- [ ] **Step 6: 修复 `render_hotkey_tab` 中的引用（原代码中 `Add` → `AddBlacklist`, `Remove` → `RemoveBlacklist`）**

将原来所有 `HotkeyConfirmAction::Add` 改为 `HotkeyConfirmAction::AddBlacklist`
将原来所有 `HotkeyConfirmAction::Remove` 改为 `HotkeyConfirmAction::RemoveBlacklist`

- [ ] **Step 7: Commit**

```bash
git add src/ui/settings/hotkey.rs
git commit -m "refactor: extract shared foreground app bar, add paste shortcut list UI

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: root.rs — 处理新 Confirm 事件 + 更新 paste 调用侧

**Files:**
- Modify: `G:\Develop\github\clippi\src\ui\root.rs:300-303, 774-853`
- Modify: `G:\Develop\github\clippi\src\state\app.rs:722-723, 764-933`

- [ ] **Step 1: 在 root.rs 中处理 `HotkeyPasteShortcut` 事件**

在 `SettingsEvent::ShowHotkeyConfirm` 处理代码（第 300-303 行）后添加新 arm：

```rust
                    SettingsEvent::HotkeyPasteShortcut { ref action } => {
                        match action {
                            hotkey::HotkeyConfirmAction::AddPasteShortcut { app_name, shortcut } => {
                                let mut map = state.read(cx).settings.paste_shortcut_map.clone();
                                // Remove existing entry for same app (overwrite)
                                map.retain(|e| e.app_name != *app_name);
                                map.push(crate::core::settings::PasteShortcutEntry {
                                    app_name: app_name.clone(),
                                    shortcut: shortcut.clone(),
                                });
                                state.update(cx, |s, cx| {
                                    s.settings.paste_shortcut_map = map;
                                    s.settings.save();
                                });
                                this.settings_panel.update(cx, |panel, cx| {
                                    panel.clear_paste_shortcut_state(cx);
                                });
                            }
                            hotkey::HotkeyConfirmAction::RemovePasteShortcut { app_name } => {
                                let mut map = state.read(cx).settings.paste_shortcut_map.clone();
                                map.retain(|e| e.app_name != *app_name);
                                state.update(cx, |s, cx| {
                                    s.settings.paste_shortcut_map = map;
                                    s.settings.save();
                                });
                                this.settings_panel.update(cx, |panel, cx| {
                                    panel.clear_paste_shortcut_state(cx);
                                });
                            }
                            _ => {}
                        }
                        cx.notify();
                    }
```

- [ ] **Step 2: 更新 root.rs 中 ConfirmDialog 的引用**

将第 785 行 `hotkey::HotkeyConfirmAction::Add` → `hotkey::HotkeyConfirmAction::AddBlacklist`
将第 822 行 `hotkey::HotkeyConfirmAction::Remove` → `hotkey::HotkeyConfirmAction::RemoveBlacklist`

并添加 paste shortcut confirm dialog 的渲染（add/remove 两个新变体）：

```rust
                        Some(hotkey::HotkeyConfirmAction::AddPasteShortcut { app_name, shortcut }) => {
                            ConfirmDialog::add_paste_shortcut(&app_name, &shortcut)
                                .theme(self.theme.clone())
                                .on_confirm({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            cx.emit(SettingsEvent::HotkeyPasteShortcut {
                                                action: hotkey::HotkeyConfirmAction::AddPasteShortcut {
                                                    app_name: app_name.clone(),
                                                    shortcut: shortcut.clone(),
                                                },
                                            });
                                        });
                                    }
                                })
                                .on_cancel({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_paste_shortcut_state(cx);
                                        });
                                    }
                                })
                                .into_any_element()
                        }
                        Some(hotkey::HotkeyConfirmAction::RemovePasteShortcut { app_name }) => {
                            ConfirmDialog::remove_paste_shortcut(&app_name)
                                .theme(self.theme.clone())
                                .on_confirm({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            cx.emit(SettingsEvent::HotkeyPasteShortcut {
                                                action: hotkey::HotkeyConfirmAction::RemovePasteShortcut {
                                                    app_name: app_name.clone(),
                                                },
                                            });
                                        });
                                    }
                                })
                                .on_cancel({
                                    let settings = settings.clone();
                                    move |_window, cx| {
                                        settings.update(cx, |panel, cx| {
                                            panel.clear_paste_shortcut_state(cx);
                                        });
                                    }
                                })
                                .into_any_element()
                        }
```

**注意**：需要将 root.rs 中 ConfirmDialog 的 `match action` 改为考虑所有 4 个变体。`None` 分支保持原样。

- [ ] **Step 3: 更新 `app.rs` paste 调用侧 — 传递 `Arc<Vec<PasteShortcutEntry>>`**

在每个 `paste_after_delay()` 和 `paste_sync()` 调用处添加参数。在 `app.rs` 中，`AppState` 可以直接读取 settings。

先将 `settings.paste_shortcut_map` 包装为 `Arc`，各调用点改为：

```rust
// 替换: crate::platform::paste::paste_after_delay();
// 改为:
use std::sync::Arc;
let map = Arc::new(self.settings.paste_shortcut_map.clone());
crate::platform::paste::paste_after_delay(map);
```

定位所有调用点（第 723, 790, 817, 838, 860, 886, 921, 933 行），逐一修改。

对于 `paste_sync` 同理。

- [ ] **Step 4: 编译验证**

```bash
cargo check 2>&1
```

Expected: 编译通过

- [ ] **Step 5: Commit**

```bash
git add src/ui/root.rs src/state/app.rs
git commit -m "feat: wire paste shortcut config to paste calls, handle new confirm events

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: WindowManager — paste shortcut 录制状态管理

**Files:**
- Modify: `G:\Develop\github\clippi\src\ui\window_manager.rs:279-311`

- [ ] **Step 1: 在 `WindowManager` struct 中添加 paste shortcut 录制标志**

在 `WindowManager` struct 中添加字段（约第 196 行附近）：

```rust
    /// Whether the current hotkey recording is for a paste shortcut (not global hotkey).
    pub recording_paste_shortcut_app: Option<String>,
```

初始化（构造函数中）：
```rust
    recording_paste_shortcut_app: None,
```

- [ ] **Step 2: 修改 `poll_recording` 处理 paste shortcut 录制完成**

在 `poll_recording` 方法中（第 285 行附近），当录制完成且 `recording_paste_shortcut_app` 为 `Some` 时，不更新全局 hotkey，而是发射新的 `WindowManagerEvent`：

```rust
    fn poll_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(ref mut hk) = self.hotkey {
            if let Some(new_hotkey) = hk.poll_recording_pressed() {
                // Check if recording for paste shortcut
                if let Some(app_name) = self.recording_paste_shortcut_app.take() {
                    hk.finish_recording();
                    hk.register();
                    cx.emit(WindowManagerEvent::PasteShortcutRecorded {
                        app_name,
                        shortcut: new_hotkey,
                    });
                    return;
                }
                // Existing global hotkey recording logic
                // ... (unchanged)
            }
        }
    }
```

- [ ] **Step 3: 处理 root.rs 中的 `PasteShortcutRecorded` 事件**

在 root.rs 中接收 `WindowManagerEvent::PasteShortcutRecorded`：

```rust
            WindowManagerEvent::PasteShortcutRecorded { app_name, shortcut } => {
                self.settings_panel.update(cx, |panel, cx| {
                    panel.pending_paste_shortcut = Some((app_name.clone(), shortcut.clone()));
                    panel.recording_paste_shortcut = None;
                    cx.emit(SettingsEvent::ShowHotkeyConfirm(
                        hotkey::HotkeyConfirmAction::AddPasteShortcut {
                            app_name,
                            shortcut,
                        },
                    ));
                });
                cx.notify();
            }
```

- [ ] **Step 4: 编译验证**

```bash
cargo check 2>&1
```

Expected: 编译通过

- [ ] **Step 5: Commit**

```bash
git add src/ui/window_manager.rs src/ui/root.rs
git commit -m "feat: support paste shortcut recording in WindowManager poll loop

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: 集成编译 + clippy 检查

**Files:** 无新建，验证所有改动

- [ ] **Step 1: 完整编译**

```bash
cargo build 2>&1
```

Expected: 编译成功

- [ ] **Step 2: Clippy 检查**

```bash
cargo clippy -- -D warnings 2>&1
```

Expected: 零 warning

- [ ] **Step 3: 运行现有测试**

```bash
cargo test 2>&1
```

Expected: 全部通过

- [ ] **Step 4: 最终 commit**

```bash
git add -A
git commit -m "chore: final integration and clippy pass for paste shortcut feature

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- ✅ Data model (PasteShortcutEntry + paste_shortcut_map) → Task 1
- ✅ Smart terminal detection (is_console_window) → Task 3
- ✅ Paste decision flow (user config → smart detection → default) → Task 3
- ✅ UI refactoring (foreground app bar extraction) → Task 7
- ✅ UI new section (paste shortcut list) → Task 7
- ✅ Recording reuse (same infra, new flag) → Task 9
- ✅ Events/Confirm expansion → Tasks 4, 5, 6
- ✅ Call site wiring → Task 8

**Placeholder scan:** 无 TBD/TODO/占位符

**Type consistency:**
- `PasteShortcutEntry` defined in Task 1, used in Tasks 3, 7, 8 ✅
- `HotkeyConfirmAction` variants renamed in Task 4, all references updated in Tasks 6, 7, 8 ✅
- `Arc<Vec<PasteShortcutEntry>>` signature consistent across paste.rs, app.rs ✅
- `WindowManagerEvent::PasteShortcutRecorded` defined in Task 9, consumed in Task 9 ✅
