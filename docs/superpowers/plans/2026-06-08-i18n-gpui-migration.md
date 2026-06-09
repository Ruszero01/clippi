# i18n GPUI Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite Slint-era `i18n::tr("中文","English")` into a compile-time-safe `I18nKey` enum system and apply i18n to all GPUI UI code.

**Architecture:** Two-file engine: `i18n.rs` (macro + AtomicBool state) and `i18n_keys.rs` (translation data). GPUI UI re-renders on language switch via `cx.notify()`; tray menu updates live via `muda::MenuItem::set_text()` — zero restart needed.

**Tech Stack:** Rust, GPUI 0.2.2, tray-icon 0.19 (muda backend), no new external dependencies.

---

## File Change Map

```
NEW:
  src/core/i18n_keys.rs          — define_i18n! invocation (~120 keys)

MODIFY:
  src/core/mod.rs                — add `pub mod i18n_keys;`
  src/core/i18n.rs               — rewrite engine with define_i18n! macro
  src/core/settings.rs           — i18n::tr() → I18nKey
  src/core/types.rs              — i18n::tr() → I18nKey
  src/main.rs                    — adapt language init, import I18nKey
  src/platform/tray.rs           — I18nKey in constructor + update_language()
  src/platform/hotkey.rs         — i18n::tr() → I18nKey
  src/platform/source.rs         — i18n::tr() → I18nKey
  src/services/backends/local_folder.rs — i18n::tr() → I18nKey
  src/services/backends/webdav.rs       — i18n::tr() → I18nKey
  src/ui/settings/mod.rs         — TAB_NAMES → function, Slint comment cleanup
  src/ui/settings/general.rs     — add Language dropdown, i18n all labels
  src/ui/settings/clipboard.rs   — i18n all labels
  src/ui/settings/data.rs        — i18n::tr() → I18nKey
  src/ui/settings/sync.rs        — i18n all labels
  src/ui/settings/hotkey.rs      — i18n all labels
  src/ui/root.rs                 — i18n all labels
  src/ui/add_backend.rs          — i18n all labels + placeholders
  src/ui/window_manager.rs       — tray update_language() wire-up
```

---

### Task 1: Rewrite i18n Engine (`i18n.rs`)

**Files:**
- Modify: `src/core/i18n.rs`

- [ ] **Step 1: Replace i18n.rs content with the new engine**

Replace the entire file content with:

```rust
//! Compile-time-safe i18n engine.
//!
//! Uses a global atomic flag so any code path can check the current language
//! without threading a settings reference through every call chain.
//!
//! Translation keys are defined in `i18n_keys.rs` via the `define_i18n!` macro.

use std::sync::atomic::{AtomicBool, Ordering};

static IS_ENGLISH: AtomicBool = AtomicBool::new(false);

/// Set the current language. Call once at startup and on every language switch.
pub fn set_language(lang: &str) {
    IS_ENGLISH.store(lang == "en", Ordering::Relaxed);
}

/// Check if the current language is English.
#[inline]
pub fn is_en() -> bool {
    IS_ENGLISH.load(Ordering::Relaxed)
}

/// Get the current language code.
#[inline]
pub fn current_language() -> &'static str {
    if is_en() {
        "en"
    } else {
        "zh_CN"
    }
}

/// Defines the `I18nKey` enum and its `text()` / `fmt()` methods.
///
/// Usage in `i18n_keys.rs`:
/// ```ignore
/// define_i18n! {
///     key_name: ("中文", "English"),
/// }
/// ```
///
/// Then use: `I18nKey::KeyName.text()`
#[macro_export]
macro_rules! define_i18n {
    ($($key:ident: ($zh:literal, $en:literal)),* $(,)?) => {
        /// Every user-visible string key — compile-time safe.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum I18nKey {
            $($key,)*
        }

        impl I18nKey {
            /// Look up the translation for the current language.
            /// Returns `&'static str` — zero allocation.
            pub fn text(self) -> &'static str {
                if $crate::core::i18n::is_en() {
                    match self {
                        $(I18nKey::$key => $en,)*
                    }
                } else {
                    match self {
                        $(I18nKey::$key => $zh,)*
                    }
                }
            }

            /// Format with positional placeholders: `{0}`, `{1}`, …
            /// Allocates a `String` only when args are provided.
            pub fn fmt(self, args: &[&str]) -> String {
                let tmpl = self.text();
                if args.is_empty() {
                    return tmpl.to_string();
                }
                let mut result = tmpl.to_string();
                for (i, arg) in args.iter().enumerate() {
                    result = result.replace(&format!("{{{i}}}"), arg);
                }
                result
            }
        }
    };
}

// ─── Backward-compatible tr() — remove after all call sites migrated ───

/// Simple static translation: `tr("中文", "English")` returns the right string.
/// DEPRECATED: use `I18nKey::KeyName.text()` instead.
#[inline]
pub fn tr<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if is_en() {
        en
    } else {
        zh
    }
}
```

- [ ] **Step 2: Build to verify no compilation errors**

```bash
cargo build 2>&1
```

Expected: builds successfully (old `i18n::tr()` still works).

- [ ] **Step 3: Commit**

```bash
git add src/core/i18n.rs
git commit -m "feat(i18n): rewrite engine with define_i18n! macro

- Add compile-time-safe I18nKey enum generation
- Keep backward-compatible tr() during migration
- Add fmt() for parameter interpolation"
```

---

### Task 2: Create Translation Keys (`i18n_keys.rs`)

**Files:**
- Create: `src/core/i18n_keys.rs`
- Modify: `src/core/mod.rs`

- [ ] **Step 1: Register module in core/mod.rs**

In `src/core/mod.rs`, add after `pub mod i18n;`:

```rust
pub mod i18n_keys;
```

Full mod.rs should read:

```rust
//! --- Core layer - pure Rust, no platform code ---

pub mod cache_cleanup;
pub mod color;
pub mod db;
pub mod filters;
pub mod frontend;
pub mod i18n;
pub mod i18n_keys;
pub mod migration;
pub mod ocr;
pub mod paths;
pub mod qr;
pub mod settings;
pub mod sync;
pub mod types;
```

- [ ] **Step 2: Create i18n_keys.rs with all translation keys**

Write the complete file `src/core/i18n_keys.rs`:

```rust
//! Translation keys — all user-visible strings in one place.
//!
//! Each key: `key_name: ("中文", "English")`
//! Usage: `I18nKey::KeyName.text()`

use crate::define_i18n;

define_i18n! {
    // ─── App ───
    app_name:            ("Clippi 剪贴板", "Clippi Clipboard"),

    // ─── Tray ───
    tray_show:           ("显示窗口", "Show Window"),
    tray_settings:       ("设置", "Settings"),
    tray_restart:        ("重启应用", "Restart"),
    tray_quit:           ("退出", "Quit"),
    tray_check_update:   ("检查更新", "Check for Updates"),

    // ─── Settings window ───
    settings_title:      ("设置", "Settings"),

    // ─── Settings tabs ───
    tab_general:         ("通用", "General"),
    tab_clipboard:       ("剪贴板", "Clipboard"),
    tab_hotkey:          ("快捷键", "Hotkey"),
    tab_data:            ("数据", "Data"),
    tab_sync:            ("同步", "Sync"),

    // ─── General settings ───
    setting_auto_start:  ("开机自启", "Auto-start"),
    desc_auto_start:     ("系统启动时自动运行", "Run on system startup"),
    setting_auto_hide:   ("自动隐藏", "Auto-hide"),
    desc_auto_hide:      ("失去焦点时隐藏窗口", "Hide on focus loss"),
    setting_silent_start:("静默启动", "Silent start"),
    desc_silent_start:   ("启动后最小化到托盘", "Start silently in tray"),
    setting_theme:       ("主题", "Theme"),
    desc_theme:          ("选择界面主题", "Select theme"),
    setting_position:    ("弹出位置", "Position"),
    desc_position:       ("窗口弹出位置", "Popup position"),
    setting_language:    ("语言", "Language"),
    desc_language:       ("选择界面语言", "Select interface language"),

    // ─── Theme options ───
    theme_system:        ("跟随系统", "Auto"),
    theme_dark:          ("深色", "Dark"),
    theme_light:         ("浅色", "Light"),

    // ─── Position options ───
    pos_center:          ("居中", "Center"),
    pos_follow:          ("跟随鼠标", "Follow"),
    pos_remember:        ("记住位置", "Pin"),

    // ─── Language options ───
    lang_system:         ("跟随系统", "Auto"),
    lang_zh:             ("中文", "中文"),
    lang_en:             ("English", "English"),

    // ─── Clipboard settings ───
    setting_sort_created:   ("按创建时间排序", "Sort by created"),
    desc_sort_first:        ("最先创建的在前", "First created"),
    desc_sort_last:         ("最近修改的在前", "Last modified"),
    setting_card_height:    ("卡片高度", "Card height"),
    desc_card_height:       ("调整卡片高度", "Adjust card height"),
    card_height_tall:       ("高", "Tall"),
    card_height_med:        ("中", "Med"),
    card_height_short:      ("低", "Short"),
    card_height_auto:       ("自动", "Auto"),
    setting_show_source:    ("显示来源应用", "Show source app"),
    desc_show_source_on:    ("显示来源应用图标", "Show source app icon"),
    desc_show_source_off:   ("仅显示内容类型", "Show content type only"),
    setting_scroll_top:     ("滚动到顶部", "Scroll to top"),
    desc_scroll_top_on:     ("打开时滚动到顶部", "Scroll to top on open"),
    desc_scroll_top_off:    ("保持上次滚动位置", "Keep last scroll position"),
    setting_copy_plain:     ("复制为纯文本", "Copy as plain text"),
    desc_copy_plain_on:     ("仅保存纯文本", "Save as plain text only"),
    desc_copy_plain_off:    ("保留富文本格式", "Keep rich formatting"),
    setting_show_original:  ("悬停显示原文", "Show original on hover"),
    desc_show_original_on:  ("悬停时显示原始内容", "Show original on hover"),
    desc_show_original_off: ("有备注的卡片显示备注", "Cards with notes show note"),
    setting_ocr:            ("自动图片 OCR", "Auto Image OCR"),
    desc_ocr_on:            ("自动识别图片文字", "Auto OCR for images"),
    desc_ocr_off:           ("OCR 已关闭", "OCR disabled"),
    setting_qr:             ("自动二维码识别", "Auto QR Detection"),
    desc_qr_on:             ("自动识别图片中的二维码", "Auto detect QR in images"),
    desc_qr_off:            ("二维码识别已关闭", "QR detection disabled"),

    // ─── Data settings ───
    setting_db_path:     ("数据库路径", "Database path"),
    btn_change:          ("更改", "Change"),
    btn_reset:           ("重置", "Reset"),
    btn_cancel:          ("取消", "Cancel"),
    btn_apply:           ("应用", "Apply"),
    btn_reset_data_dir:  ("重置数据目录", "Reset Data Directory"),
    setting_max_items:   ("最大保存条目数", "Max items"),
    desc_max_items:      ("0 表示不限制数量", "0 for unlimited"),
    unlimited:           ("不限制", "Unlimited"),
    system_default:      ("系统默认", "System default"),
    desc_reset_data:     ("将数据和配置恢复到默认位置，原文件不会删除", "Restore data and config to default location. Old files won't be deleted"),
    confirm_reset_title: ("确认重置", "Confirm Reset"),
    confirm_reset_msg:   ("确定要恢复默认数据目录吗？应用将自动重启。", "Restore default data directory? The app will restart."),

    // ─── Hotkey settings ───
    hotkey_current:      ("当前快捷键", "Current hotkey"),
    hotkey_press_hint:   ("按下组合键...", "Press keys..."),
    hotkey_blacklist:    ("快捷键黑名单", "Hotkey blacklist"),
    hotkey_add_blacklist:("添加应用", "Add app"),
    hotkey_clear:        ("清除", "Clear"),
    hotkey_recording:    ("正在录制快捷键，按 Esc 取消", "Recording hotkey, press Esc to cancel"),

    // ─── Sync settings ───
    sync_title:          ("同步", "Sync"),
    sync_favorites_only: ("仅同步收藏", "Favorites only"),
    sync_add_backend:    ("添加后端", "Add backend"),
    sync_now:            ("立即同步", "Sync now"),
    sync_syncing:        ("同步中", "Syncing"),
    sync_edit_backend:   ("编辑后端", "Edit backend"),

    // ─── Add backend ───
    backend_add_title:   ("添加后端", "Add backend"),
    backend_edit_title:  ("编辑后端", "Edit backend"),
    backend_local_folder:("本地文件夹", "Local Folder"),
    backend_webdav:      ("WebDAV", "WebDAV"),
    backend_select_type: ("选择后端类型", "Select backend type"),
    backend_local_desc:  ("OneDrive、iCloud 等", "OneDrive, iCloud, etc."),
    backend_webdav_desc: ("NAS、Nextcloud 等", "NAS, Nextcloud, etc."),
    backend_quick_add:   ("快速添加", "Quick add"),
    backend_name:        ("名称", "Name"),
    backend_folder:      ("文件夹", "Folder"),
    backend_server_url:  ("服务器地址", "Server URL"),
    backend_username:    ("用户名", "Username"),
    backend_password:    ("密码", "Password"),
    backend_browse:      ("浏览", "Browse"),
    backend_save:        ("保存", "Save"),
    backend_test:        ("测试连接", "Test connection"),
    backend_testing:     ("测试中...", "Testing..."),
    backend_test_fail:   ("连接失败，请检查地址和凭据", "Connection failed. Check URL and credentials."),
    backend_placeholder_name:   ("后端名称", "Backend name"),
    backend_placeholder_folder: ("文件夹路径", "Folder path"),
    backend_placeholder_url:    ("https://example.com/dav", "https://example.com/dav"),
    backend_placeholder_user:   ("用户名", "Username"),
    backend_placeholder_pass:   ("密码", "Password"),

    // ─── Root view / Clipboard list ───
    clipboard_empty:     ("剪贴板为空", "Clipboard is empty"),
    clipboard_hint:      ("复制内容后将显示在这里", "Copied content will appear here"),
    search_placeholder:  ("搜索...", "Search..."),
    filter_all:          ("全部", "All"),
    filter_text:         ("文本", "Text"),
    filter_image:        ("图片", "Image"),
    filter_file:         ("文件", "File"),
    filter_link:         ("链接", "Link"),
    filter_color:        ("颜色", "Color"),
    filter_fav:          ("收藏", "Fav"),
    batch_select:        ("批量选择", "Select"),
    batch_delete:        ("批量删除", "Delete"),
    batch_tag:           ("批量标签", "Tag"),
    batch_copy:          ("批量复制", "Copy"),
    batch_paste:         ("批量粘贴", "Paste"),
    confirm_delete:      ("确认删除", "Confirm Delete"),
    confirm_delete_msg:  ("确定要删除所选项吗？此操作不可撤销。", "Delete selected items? This cannot be undone."),

    // ─── Context menu ───
    ctx_copy:            ("复制", "Copy"),
    ctx_paste:           ("粘贴", "Paste"),
    ctx_edit:            ("编辑", "Edit"),
    ctx_favorite:        ("收藏", "Favorite"),
    ctx_unfavorite:      ("取消收藏", "Unfavorite"),
    ctx_delete:          ("删除", "Delete"),
    ctx_select_all:      ("全选", "Select All"),
    ctx_deselect_all:    ("取消全选", "Deselect All"),
    ctx_add_tag:         ("添加标签", "Add Tag"),
    ctx_copy_plain:      ("复制纯文本", "Copy Plain Text"),
    ctx_copy_html:       ("复制 HTML", "Copy HTML"),
    ctx_save_image:      ("保存图片", "Save Image"),
    ctx_open_link:       ("打开链接", "Open Link"),
    ctx_open_file:       ("打开文件", "Open File"),
    ctx_open_folder:     ("打开文件夹", "Open Folder"),

    // ─── Edit panel ───
    edit_title:          ("编辑", "Edit"),
    edit_save:           ("保存", "Save"),
    edit_note:           ("备注", "Note"),
    edit_content:        ("内容", "Content"),

    // ─── Sidebar ───
    sidebar_all_tags:    ("所有标签", "All Tags"),

    // ─── Tag management ───
    tag_new:             ("新建标签", "New Tag"),
    tag_name:            ("标签名", "Tag name"),
    tag_color:           ("颜色", "Color"),
    tag_delete:          ("删除标签", "Delete Tag"),
    tag_confirm_delete:  ("确定要删除此标签吗？", "Delete this tag?"),
    tag_manage:          ("管理标签", "Manage Tags"),
    tag_no_tags:         ("暂无标签", "No tags"),

    // ─── Titlebar ───
    titlebar_settings:   ("设置", "Settings"),
    titlebar_pin:        ("固定窗口", "Pin Window"),
    titlebar_unpin:      ("取消固定", "Unpin"),

    // ─── Hover toolbar ───
    hover_copy:          ("复制", "Copy"),
    hover_favorite:      ("收藏", "Favorite"),
    hover_delete:        ("删除", "Delete"),
    hover_note:          ("备注", "Note"),

    // ─── Toast ───
    toast_copied:        ("已复制", "Copied"),
    toast_deleted:       ("已删除", "Deleted"),
    toast_saved:         ("已保存", "Saved"),
    toast_error:         ("操作失败", "Operation failed"),
    toast_tag_added:     ("标签已添加", "Tag added"),
    toast_tag_removed:   ("标签已移除", "Tag removed"),

    // ─── Icons / UI chrome ───
    // (none — iconfont codepoints are not translated)

    // ─── Types / Data (matching old i18n::tr calls) ───
    format_just_now:     ("刚刚", "Just now"),
    format_minutes_ago:  ("{0}分钟前", "{0} min ago"),
    format_hours_ago:    ("{0}小时前", "{0} h ago"),
    format_days_ago:     ("{0}天前", "{0} d ago"),
    content_type_file:   ("文件", "File"),
    unknown_app:         ("未知应用", "Unknown app"),

    // ─── Settings migration / errors ───
    err_registry_open:       ("打开注册表失败", "Failed to open registry"),
    err_get_exe_path:        ("获取程序路径失败", "Failed to get program path"),
    err_registry_write:      ("写入注册表失败", "Failed to write registry"),
    err_launch_agents_path:  ("无法获取 LaunchAgents 路径", "Cannot get LaunchAgents path"),
    err_create_launch_agents:("创建 LaunchAgents 目录失败", "Failed to create LaunchAgents directory"),
    err_write_plist:         ("写入 plist 失败", "Failed to write plist"),
    err_delete_plist:        ("删除 plist 失败", "Failed to delete plist"),
    err_same_path:           ("新路径与当前路径相同", "New path is same as current"),
    err_create_dir:          ("创建目录失败", "Failed to create directory"),
    err_copy_db:             ("复制数据库失败", "Failed to copy database"),

    // ─── Sync backends ───
    sync_err_not_dir:        ("路径不是目录", "Path is not a directory"),
    sync_err_not_found:      ("同步文件不存在", "Sync file not found"),
    sync_err_read:           ("读取同步文件失败", "Failed to read sync file"),
    sync_err_parse:          ("解析同步文件失败", "Failed to parse sync file"),
    sync_err_serialize:      ("序列化失败", "Serialization failed"),
    sync_err_write_temp:     ("写入临时文件失败", "Failed to write temp file"),
    sync_err_replace:        ("替换同步文件失败", "Failed to replace sync file"),
    sync_err_no_url:         ("未配置 URL", "URL not configured"),
    sync_err_auth:           ("认证失败", "Authentication failed"),
    sync_err_connect:        ("连接失败", "Connection failed"),
    sync_err_read_resp:      ("读取响应失败", "Failed to read response"),
    sync_err_pull:           ("拉取同步文件失败", "Failed to pull sync file"),
    sync_err_push:           ("推送同步文件失败", "Failed to push sync file"),

    // ─── Hotkey registration ───
    hotkey_err_register:     ("注册快捷键失败", "Failed to register hotkey"),
    hotkey_err_no_key:       ("未指定按键", "No key specified"),

    // ─── Data operation ───
    err_data_op:             ("数据操作失败", "Data operation failed"),

    // ─── OCR / QR labels ───
    ocr_label:               ("OCR 文字", "OCR Text"),
    qr_label:                ("二维码", "QR Code"),

    // ─── Image save ───
    image_saved:             ("图片已保存", "Image saved"),

    // ─── Update ───
    update_available:        ("发现新版本", "Update available"),
    update_latest:           ("已是最新版本", "Up to date"),
}
```

- [ ] **Step 3: Build to verify compilation**

```bash
cargo build 2>&1
```

Expected: builds successfully. `I18nKey` enum is available for import.

- [ ] **Step 4: Commit**

```bash
git add src/core/i18n_keys.rs src/core/mod.rs
git commit -m "feat(i18n): add i18n_keys.rs with ~120 translation keys"
```

---

### Task 3: Migrate Core + Platform + Services Layer

**Files:**
- Modify: `src/core/settings.rs`, `src/core/types.rs`, `src/platform/hotkey.rs`, `src/platform/source.rs`, `src/platform/tray.rs`, `src/services/backends/local_folder.rs`, `src/services/backends/webdav.rs`

- [ ] **Step 1: Migrate core/settings.rs — replace all i18n::tr() calls**

In `src/core/settings.rs`:
- Replace import: `use super::i18n;` → `use super::i18n_keys::I18nKey;`
- Replace all `i18n::tr("中文", "English")` → `I18nKey::KeyName.text()`

Mapping:
```
Line 310: i18n::tr("打开注册表失败", "Failed to open registry")
          → I18nKey::ErrRegistryOpen.text()

Line 318: i18n::tr("获取程序路径失败", "Failed to get program path")
          → I18nKey::ErrGetExePath.text()

Line 325: i18n::tr("写入注册表失败", "Failed to write registry")
          → I18nKey::ErrRegistryWrite.text()

Line 344-346: i18n::tr("无法获取 LaunchAgents 路径", "Cannot get LaunchAgents path")
              → I18nKey::ErrLaunchAgentsPath.text()

Line 353-357: i18n::tr("创建 LaunchAgents 目录失败", "Failed to create LaunchAgents directory")
              → I18nKey::ErrCreateLaunchAgents.text()

Line 365: i18n::tr("获取程序路径失败", "Failed to get program path")
          → I18nKey::ErrGetExePath.text()

Line 392: i18n::tr("写入 plist 失败", "Failed to write plist")
          → I18nKey::ErrWritePlist.text()

Line 400: i18n::tr("删除 plist 失败", "Failed to delete plist")
          → I18nKey::ErrDeletePlist.text()

Line 431: i18n::tr("新路径与当前路径相同", "New path is same as current")
          → I18nKey::ErrSamePath.text()

Line 438: i18n::tr("创建目录失败", "Failed to create directory")
          → I18nKey::ErrCreateDir.text()

Line 446: i18n::tr("复制数据库失败", "Failed to copy database")
          → I18nKey::ErrCopyDb.text()
```

- [ ] **Step 2: Migrate core/types.rs — replace i18n calls**

In `src/core/types.rs`:
- Replace import: `use super::i18n;` → `use super::i18n_keys::I18nKey;`
- Replace `i18n::is_en()` → `crate::core::i18n::is_en()` (keep inline check pattern)
- Replace `i18n::tr("文件", "File")` → `I18nKey::ContentTypeFile.text()`
- Replace `i18n::tr("刚刚", "Just now")` → `I18nKey::FormatJustNow.text()`
- For the `format_relative_time()` function, the `if i18n::is_en()` branches use different format strings. Update to use I18nKey:
  ```
  "{0}分钟前" → I18nKey::FormatMinutesAgo.fmt(&[&mins.to_string()])
  ```
  etc.

- [ ] **Step 3: Migrate platform/hotkey.rs — replace i18n::tr() calls**

In `src/platform/hotkey.rs`:
- Replace import: `use crate::core::i18n;` → `use crate::core::i18n_keys::I18nKey;`
- Replace each `i18n::tr(...)` call:
  ```
  "注册快捷键失败" → I18nKey::HotkeyErrRegister.text()
  "未指定按键"     → I18nKey::HotkeyErrNoKey.text()
  ```

- [ ] **Step 4: Migrate platform/source.rs — replace i18n::tr() calls**

In `src/platform/source.rs`:
- Replace import: `use crate::core::i18n;` → `use crate::core::i18n_keys::I18nKey;`
- Replace `i18n::tr("未知应用", "Unknown app")` → `I18nKey::UnknownApp.text()`

- [ ] **Step 5: Migrate platform/tray.rs — I18nKey + add update_language()**

In `src/platform/tray.rs`:
- Replace import: `use crate::core::i18n;` → `use crate::core::i18n_keys::I18nKey;`
- In `TrayManager::new()`, replace each `i18n::tr(...)`:
  ```
  "检查更新" → I18nKey::TrayCheckUpdate.text()
  "显示窗口" → I18nKey::TrayShow.text()
  "设置"     → I18nKey::TraySettings.text()
  "重启应用" → I18nKey::TrayRestart.text()
  "退出"     → I18nKey::TrayQuit.text()
  ```

Then add the `update_language` method to `impl TrayManager`:

```rust
/// Update all menu item texts when language changes.
/// muda supports live text updates — no tray recreation needed.
pub fn update_language(&mut self) {
    self._check_update_item.set_text(I18nKey::TrayCheckUpdate.text());
    self._items[0].set_text(I18nKey::TrayShow.text());
    self._items[1].set_text(I18nKey::TraySettings.text());
    self._items[2].set_text(I18nKey::TrayRestart.text());
    self._items[3].set_text(I18nKey::TrayQuit.text());
}
```

- [ ] **Step 6: Migrate services/backends/local_folder.rs — replace i18n::tr() calls**

In `src/services/backends/local_folder.rs`:
- Replace import: `use crate::core::i18n;` → `use crate::core::i18n_keys::I18nKey;`
- Replace each `i18n::tr(...)`:
  ```
  "路径不是目录"       → I18nKey::SyncErrNotDir.text()
  "同步文件不存在"     → I18nKey::SyncErrNotFound.text()
  "读取同步文件失败"   → I18nKey::SyncErrRead.text()
  "解析同步文件失败"   → I18nKey::SyncErrParse.text()
  "创建目录失败"       → I18nKey::ErrCreateDir.text()
  "序列化失败"         → I18nKey::SyncErrSerialize.text()
  "写入临时文件失败"   → I18nKey::SyncErrWriteTemp.text()
  "替换同步文件失败"   → I18nKey::SyncErrReplace.text()
  ```

- [ ] **Step 7: Migrate services/backends/webdav.rs — replace i18n::tr() calls**

In `src/services/backends/webdav.rs`:
- Replace import: `use crate::core::i18n;` → `use crate::core::i18n_keys::I18nKey;`
- Replace each `i18n::tr(...)`:
  ```
  "未配置 URL"         → I18nKey::SyncErrNoUrl.text()
  "认证失败"           → I18nKey::SyncErrAuth.text()
  "连接失败"           → I18nKey::SyncErrConnect.text()
  "读取响应失败"       → I18nKey::SyncErrReadResp.text()
  "解析同步文件失败"   → I18nKey::SyncErrParse.text()
  "同步文件不存在"     → I18nKey::SyncErrNotFound.text()
  "拉取同步文件失败"   → I18nKey::SyncErrPull.text()
  "序列化失败"         → I18nKey::SyncErrSerialize.text()
  "推送同步文件失败"   → I18nKey::SyncErrPush.text()
  ```

- [ ] **Step 8: Build and verify**

```bash
cargo build 2>&1
```

Expected: builds successfully. `i18n::tr()` still exists as fallback but all migrated calls use `I18nKey`.

- [ ] **Step 9: Commit**

```bash
git add src/core/settings.rs src/core/types.rs src/platform/hotkey.rs src/platform/source.rs src/platform/tray.rs src/services/backends/local_folder.rs src/services/backends/webdav.rs
git commit -m "feat(i18n): migrate core + platform + services to I18nKey

Replace all i18n::tr() calls with I18nKey::KeyName.text().
Add TrayManager::update_language() for live tray menu language switch."
```

---

### Task 4: i18n Settings UI — mod.rs + general.rs

**Files:**
- Modify: `src/ui/settings/mod.rs`, `src/ui/settings/general.rs`

- [ ] **Step 1: Update mod.rs — TAB_NAMES and Slint comments**

In `src/ui/settings/mod.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace the `const TAB_NAMES` line:
  ```rust
  // DELETE:
  const TAB_NAMES: &[&str] = &["General", "Clipboard", "Hotkey", "Data", "Sync"];
  
  // ADD:
  fn tab_names() -> [&'static str; 5] {
      [
          I18nKey::TabGeneral.text(),
          I18nKey::TabClipboard.text(),
          I18nKey::TabHotkey.text(),
          I18nKey::TabData.text(),
          I18nKey::TabSync.text(),
      ]
  }
  ```
- Update references: change `TAB_NAMES.iter()` → `tab_names().iter()` (line 212)
- Change `.child(*name)` → `.child(*name)` (no change needed, name is already `&'static str`)
- Change the Settings title `.child("Settings")` → `.child(I18nKey::SettingsTitle.text())` (line 199)
- Update comment on line 1: `Slint `SettingsPanel.slint`` → `GPUI SettingsPanel`

- [ ] **Step 2: Update general.rs — add Language dropdown + i18n all labels**

In `src/ui/settings/general.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;` and `use crate::core;`
- Read current language from AppState:
  After line 28 (`// --- borrow released here`), add:
  ```rust
  let lang = app.settings.language.clone();
  ```
- Replace all hardcoded strings with `I18nKey`:
  ```
  "Auto-start"              → I18nKey::SettingAutoStart.text()
  "Run on system startup"  → I18nKey::DescAutoStart.text()
  "Auto-hide"              → I18nKey::SettingAutoHide.text()
  "Hide on focus loss"     → I18nKey::DescAutoHide.text()
  "Silent start"           → I18nKey::SettingSilentStart.text()
  "Start silently in tray" → I18nKey::DescSilentStart.text()
  "Theme"                  → I18nKey::SettingTheme.text()
  "Select theme"           → I18nKey::DescTheme.text()
  "Position"               → I18nKey::SettingPosition.text()
  "Popup position"         → I18nKey::DescPosition.text()
  ```
- Replace option labels:
  ```
  ("system", "Auto")   → ("system", I18nKey::ThemeSystem.text())
  ("dark", "Dark")     → ("dark", I18nKey::ThemeDark.text())
  ("light", "Light")   → ("light", I18nKey::ThemeLight.text())
  ("center", "Center") → ("center", I18nKey::PosCenter.text())
  ("follow", "Follow") → ("follow", I18nKey::PosFollow.text())
  ("remember", "Pin")  → ("remember", I18nKey::PosRemember.text())
  ```
- Add the Language setting row between Theme and Position:

```rust
// --- Language ---
.child({
    let state = state.clone();
    let this = this.clone();
    let wm = wm.clone();
    let lang_clone = lang.clone();
    self.setting_row_with_options(
        I18nKey::SettingLanguage.text(),
        I18nKey::DescLanguage.text(),
        &[
            ("system", I18nKey::LangSystem.text()),
            ("zh_CN", I18nKey::LangZh.text()),
            ("en", I18nKey::LangEn.text()),
        ],
        if lang.is_empty() { "system" } else { &lang },
        move |key, _window, _cx| {
            let new_lang = if key == "system" { String::new() } else { key.to_string() };
            let effective = if new_lang.is_empty() {
                crate::core::settings::detect_system_language()
            } else {
                new_lang.clone()
            };
            crate::core::i18n::set_language(&effective);
            state.update(_cx, |s, _cx| {
                s.settings.language = new_lang;
                s.settings.save();
            });
            wm.update(_cx, |wm, _cx| {
                if let Some(ref mut tray) = wm.tray {
                    tray.update_language();
                }
            });
            this.update(_cx, |_, cx| cx.notify());
        },
    )
})
```

- [ ] **Step 3: Build and verify**

```bash
cargo build 2>&1
```

Expected: builds successfully.

- [ ] **Step 4: Commit**

```bash
git add src/ui/settings/mod.rs src/ui/settings/general.rs
git commit -m "feat(i18n): add language dropdown to general settings

- Replace all hardcoded labels with I18nKey
- Add Language option (system/zh_CN/en)
- Wire tray.update_language() on language switch"
```

---

### Task 5: i18n Clipboard + Data + Hotkey + Sync Settings Pages

**Files:**
- Modify: `src/ui/settings/clipboard.rs`, `src/ui/settings/data.rs`, `src/ui/settings/hotkey.rs`, `src/ui/settings/sync.rs`

- [ ] **Step 1: Update clipboard.rs — i18n all labels**

In `src/ui/settings/clipboard.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace all hardcoded strings:
  ```
  "Sort by created"         → I18nKey::SettingSortCreated.text()
  "First created"           → I18nKey::DescSortFirst.text()
  "Last modified"           → I18nKey::DescSortLast.text()
  "Card height"             → I18nKey::SettingCardHeight.text()
  "Adjust card height"      → I18nKey::DescCardHeight.text()
  "Tall" / "Med" / "Short" / "Auto" → card_height_*.text()
  "Show source app"         → I18nKey::SettingShowSource.text()
  "Show source app icon"    → I18nKey::DescShowSourceOn.text()
  "Show content type only"  → I18nKey::DescShowSourceOff.text()
  "Scroll to top"           → I18nKey::SettingScrollTop.text()
  "Scroll to top on open"   → I18nKey::DescScrollTopOn.text()
  "Keep last scroll position"→ I18nKey::DescScrollTopOff.text()
  "Copy as plain text"      → I18nKey::SettingCopyPlain.text()
  "Save as plain text only" → I18nKey::DescCopyPlainOn.text()
  "Keep rich formatting"    → I18nKey::DescCopyPlainOff.text()
  "Show original on hover"  → I18nKey::SettingShowOriginal.text()
  "Show original on hover"  → I18nKey::DescShowOriginalOn.text()
  "Cards with notes show note"→ I18nKey::DescShowOriginalOff.text()
  "Auto Image OCR"          → I18nKey::SettingOcr.text()
  "Auto OCR for images"     → I18nKey::DescOcrOn.text()
  "OCR disabled"            → I18nKey::DescOcrOff.text()
  "Auto QR Detection"       → I18nKey::SettingQr.text()
  "Auto detect QR in images"→ I18nKey::DescQrOn.text()
  "QR detection disabled"   → I18nKey::DescQrOff.text()
  ```

- [ ] **Step 2: Update data.rs — i18n::tr() → I18nKey**

In `src/ui/settings/data.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace all `i18n::tr("中文", "English")` → `I18nKey::KeyName.text()` using the keys already defined (btn_change, btn_reset, btn_cancel, btn_apply, setting_db_path, setting_max_items, desc_max_items, unlimited, system_default, btn_reset_data_dir, desc_reset_data, confirm_reset_title, confirm_reset_msg)

- [ ] **Step 3: Update hotkey.rs — i18n all labels**

In `src/ui/settings/hotkey.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace all hardcoded label strings with corresponding `I18nKey`:
  - "Current hotkey" / "当前快捷键" → `I18nKey::HotkeyCurrent.text()`
  - "Press keys..." → `I18nKey::HotkeyPressHint.text()`
  - "Hotkey blacklist" → `I18nKey::HotkeyBlacklist.text()`
  - "Add app" → `I18nKey::HotkeyAddBlacklist.text()`
  - "Clear" → `I18nKey::HotkeyClear.text()`

- [ ] **Step 4: Update sync.rs — i18n all labels**

In `src/ui/settings/sync.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace hardcoded strings:
  ```
  "Sync"           → I18nKey::SyncTitle.text()
  "Favorites only" → I18nKey::SyncFavoritesOnly.text()
  "Add backend"    → I18nKey::SyncAddBackend.text()
  "Sync now"       → I18nKey::SyncNow.text()
  "Syncing"        → I18nKey::SyncSyncing.text()
  ```

- [ ] **Step 5: Build and verify**

```bash
cargo build 2>&1
```

- [ ] **Step 6: Commit**

```bash
git add src/ui/settings/clipboard.rs src/ui/settings/data.rs src/ui/settings/hotkey.rs src/ui/settings/sync.rs
git commit -m "feat(i18n): i18n all settings page labels"
```

---

### Task 6: i18n Remaining UI Components

**Files:**
- Modify: `src/ui/root.rs`, `src/ui/add_backend.rs`, `src/ui/search_bar.rs`, `src/ui/context_menu.rs`, `src/ui/titlebar.rs`, `src/ui/hover_toolbar.rs`, `src/ui/tag_filter.rs`, `src/ui/tag_picker.rs`, `src/ui/edit_panel.rs`, `src/ui/components/confirm_dialog.rs`, `src/ui/components/toast.rs`, `src/ui/clipboard_list.rs`, `src/ui/clipboard_card.rs`

- [ ] **Step 1: Update root.rs — i18n error message**

In `src/ui/root.rs`:
- Replace `use crate::core::i18n;` → `use crate::core::i18n_keys::I18nKey;`
- Replace `i18n::tr("数据操作失败", "Data operation failed")` → `I18nKey::ErrDataOp.text()`

- [ ] **Step 2: Update add_backend.rs — i18n all labels + placeholders**

In `src/ui/add_backend.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace all `&'static str` values in:
  - `title()` method: "Edit backend" → `I18nKey::BackendEditTitle.text()`, "Add backend" → `I18nKey::BackendAddTitle.text()`, "Local Folder" → `I18nKey::BackendLocalFolder.text()`, "WebDAV" → `I18nKey::BackendWebdav.text()`
  - `render_type_picker()`: "Select backend type" → `I18nKey::BackendSelectType.text()`, "Local Folder" → `I18nKey::BackendLocalFolder.text()`, "OneDrive, iCloud, etc." → `I18nKey::BackendLocalDesc.text()`, "WebDAV" → `I18nKey::BackendWebdav.text()`, "NAS, Nextcloud, etc." → `I18nKey::BackendWebdavDesc.text()`
  - `render_local_form()`: "Quick add" → `I18nKey::BackendQuickAdd.text()`, "Name" → `I18nKey::BackendName.text()`, "Folder" → `I18nKey::BackendFolder.text()`, "Browse" → `I18nKey::BackendBrowse.text()`, "Save" → `I18nKey::BackendSave.text()`, "Add backend" → `I18nKey::BackendAddTitle.text()`
  - `render_webdav_form()`: "Server URL" → `I18nKey::BackendServerUrl.text()`, "Name" → `I18nKey::BackendName.text()`, "Username" → `I18nKey::BackendUsername.text()`, "Password" → `I18nKey::BackendPassword.text()`, "Testing..." → `I18nKey::BackendTesting.text()`, "Test connection" → `I18nKey::BackendTest.text()`, "Save" → `I18nKey::BackendSave.text()`, "Add backend" → `I18nKey::BackendAddTitle.text()`
  - `new()` placeholders: "Backend name" → `I18nKey::BackendPlaceholderName.text()`, "Folder path" → `I18nKey::BackendPlaceholderFolder.text()`, "Username" → `I18nKey::BackendPlaceholderUser.text()`, "Password" → `I18nKey::BackendPlaceholderPass.text()`
  - `start_webdav_test()` error: "Connection failed..." → `I18nKey::BackendTestFail.text()`

- [ ] **Step 3: Update search_bar.rs — i18n placeholder**

In `src/ui/search_bar.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace search placeholder "Search..." → `I18nKey::SearchPlaceholder.text()`
- Replace filter button labels if any hardcoded

- [ ] **Step 4: Update context_menu.rs — i18n menu items**

In `src/ui/context_menu.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace all context menu label strings with corresponding `I18nKey`

- [ ] **Step 5: Update titlebar.rs — i18n labels**

In `src/ui/titlebar.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace hardcoded labels with I18nKey

- [ ] **Step 6: Update hover_toolbar.rs — i18n labels**

In `src/ui/hover_toolbar.rs`:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace hardcoded labels with I18nKey

- [ ] **Step 7: Update tag_filter.rs + tag_picker.rs + edit_panel.rs**

For each file:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace user-visible hardcoded strings with corresponding `I18nKey` values

- [ ] **Step 8: Update components/confirm_dialog.rs + toast.rs + clipboard_list.rs + clipboard_card.rs**

For each file:
- Add import: `use crate::core::i18n_keys::I18nKey;`
- Replace user-visible hardcoded strings with corresponding `I18nKey` values

- [ ] **Step 9: Build and verify**

```bash
cargo build 2>&1
```

Expected: builds successfully. All UI strings now use I18nKey.

- [ ] **Step 10: Commit**

```bash
git add src/ui/
git commit -m "feat(i18n): i18n all UI component labels

Replace all hardcoded English/Chinese strings in:
- root, add_backend, search_bar, context_menu
- titlebar, hover_toolbar, tag_filter, tag_picker
- edit_panel, confirm_dialog, toast
- clipboard_list, clipboard_card

All labels now use I18nKey::*.text() for runtime language switching."
```

---

### Task 7: Cleanup — Remove Legacy i18n::tr() + Slint Comments

**Files:**
- Modify: `src/core/i18n.rs`, `src/main.rs`, Slint-comment files

- [ ] **Step 1: Remove deprecated i18n::tr() fallback**

In `src/core/i18n.rs`, remove the backward-compatibility `tr()` function (the block starting with `// ─── Backward-compatible tr()` comment). The file should end after the `define_i18n!` macro.

- [ ] **Step 2: Verify no remaining callers**

```bash
cargo build 2>&1
```

If any `i18n::tr(` calls remain, the build will fail. Replace them with the appropriate `I18nKey`. If none remain, the build succeeds.

- [ ] **Step 3: Update main.rs language init**

In `src/main.rs`:
- Add import: `use core::i18n_keys::I18nKey;` (if needed for any remaining references)
- The existing `core::i18n::set_language(&effective_language);` call remains unchanged (API is the same)

- [ ] **Step 4: Update Slint-era comments to GPUI references**

Search for `Slint` in comments and update where the reference is stale. Key files:
- `src/ui/settings/mod.rs:3`: `Slint `SettingsPanel.slint`` → current file reference
- `src/ui/settings/clipboard.rs:3`: `Slint `SettingsTabClipboard.slint`` → current
- `src/ui/settings/data.rs:3`: `Slint `SettingsTabData.slint`` → current
- `src/ui/settings/hotkey.rs:3`: `Slint `SettingsTabHotkey.slint`` → current
- `src/ui/root.rs:3`: `Slint `app.slint`` → `GPUI RootView`
- `src/ui/sidebar.rs:1`: `Slint `SideTagBar.slint`` → `GPUI Sidebar`
- `src/ui/titlebar.rs:1`: `Slint design (app.slint)` → `GPUI Titlebar`
- `src/ui/hover_toolbar.rs:3`: `Slint ClipboardList.slint` → `GPUI hover toolbar`
- `src/ui/context_menu.rs:3`: `Slint ContextMenu.slint` → `GPUI ContextMenu`
- `src/ui/tag_filter.rs:3`: `Slint TagFilterPanel.slint` → `GPUI TagFilterPanel`
- `src/ui/clipboard_card.rs:3`: `Slint ClipboardList.slint` → `GPUI ClipboardCard`
- `src/ui/search_bar.rs:3`: `Slint ClipboardList.slint` → `GPUI SearchBar`
- `src/ui/theme.rs:1`: `Slint` → remove or update
- `src/ui/window_manager.rs:5`: `Slint-era` → keep (historical context)
- `src/state/mod.rs:3`: `Slint-era` → keep
- `src/state/app.rs:617`: `Slint-era` → keep
- `src/services/clipboard_ops.rs:3`: `Slint app.rs` → `GPUI`

Strip "Slint" from doc comments where it describes the current (now GPUI) component. Keep "Slint-era" where it describes historical behavior being mirrored.

- [ ] **Step 5: Final build + clippy check**

```bash
cargo build 2>&1
cargo clippy 2>&1
```

Expected: builds with zero warnings, zero clippy issues.

- [ ] **Step 6: Commit**

```bash
git add src/core/i18n.rs src/main.rs src/ui/
git commit -m "chore(i18n): remove legacy i18n::tr(), clean Slint comments

- Remove deprecated tr() fallback function
- Update Slint-era doc comments to reference GPUI components
- Zero warnings, zero clippy issues"
```

---

### Task 8: Integration Verification

- [ ] **Step 1: Full build**

```bash
cargo build 2>&1
```

Expected: SUCCESS, zero errors, zero warnings.

- [ ] **Step 2: Clippy**

```bash
cargo clippy 2>&1
```

Expected: zero warnings.

- [ ] **Step 3: Verify all I18nKey variants used**

```bash
cargo build 2>&1 | grep -i "never used\|unused"
```

Expected: no warnings about unused I18nKey variants. If any appear, remove them from `i18n_keys.rs` or use them.

- [ ] **Step 4: Verify language switch doesn't need restart**

Code review checklist:
- [ ] `general.rs` language callback calls `i18n::set_language()` + `tray.update_language()` + `cx.notify()`
- [ ] `TrayManager::update_language()` calls `MenuItem::set_text()` on all menu items
- [ ] All UI labels use `I18nKey::Xxx.text()` (runtime lookup, not const)
- [ ] `main.rs` initializes language from settings before UI renders

- [ ] **Step 5: Final commit (if any cleanup)**

```bash
git status
# If clean, done. If not:
git add -A
git commit -m "chore(i18n): final verification cleanup"
```
