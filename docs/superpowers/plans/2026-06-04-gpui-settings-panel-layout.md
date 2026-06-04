# GPUI 设置面板布局迁移 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 Slint `SettingsPanel` 的整体布局样式和返回按钮迁移到 GPUI，对齐 ClippiTheme 主题色，预留 5 个标签页的功能接口。

**Architecture:** 重写 `SettingsPanel::render()` 为三层结构（导航栏 → 标签栏 → 可滚动内容区），通过 `SettingsEvent::Back` 事件通知 `RootView` 切换回剪贴板视图，遵循现有 `TitlebarEvent` 的 emit/subscribe 模式。使用 `cx.entity().clone()` 获取自身引用以在闭包中修改内部状态。

**Tech Stack:** Rust + GPUI 0.2.2

**Spec:** `docs/superpowers/specs/2026-06-04-gpui-settings-panel-layout-design.md`

---

### 文件结构

| 文件 | 操作 | 职责 |
|------|------|------|
| `src/ui/settings/mod.rs` | 重写 | SettingsPanel 组件：导航栏 + 标签栏 + 内容路由 + SettingsEvent |
| `src/ui/root.rs` | 修改 | 订阅 SettingsEvent::Back，切换视图回剪贴板 |

---

### Task 1: 重写 SettingsPanel — 完整布局和事件

**文件:**
- Modify: `src/ui/settings/mod.rs` (完整重写)

- [ ] **Step 1: 替换 `src/ui/settings/mod.rs`**

```rust
//! Settings panel — scrollable settings UI with tabs.
//!
//! Matches the original Slint `SettingsPanel.slint` layout:
//! - Top navigation bar: back button (← icon) + "Settings" title (36px)
//! - Tab bar: 5 equal-width tabs (General/Clipboard/Hotkey/Data/Sync)
//!   with accent-green underline for active tab
//! - Scrollable content area routed by active tab index
//!
//! Individual settings controls will be added in follow-up work.
//! Tab rendering methods (`render_*_tab`) serve as extension points.

use gpui::*;

use crate::core::settings::AppSettings;
use crate::ui::theme::ClippiTheme;

/// Events emitted by the settings panel.
pub enum SettingsEvent {
    /// User clicked the back button — return to clipboard view.
    Back,
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}

/// The settings panel entity.
pub struct SettingsPanel {
    active_tab: usize,
    settings: AppSettings,
    theme: ClippiTheme,
}

const TAB_NAMES: &[&str] = &["General", "Clipboard", "Hotkey", "Data", "Sync"];

impl SettingsPanel {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let settings = AppSettings::load();
        Self {
            active_tab: 0,
            settings,
            theme: ClippiTheme::dark(),
        }
    }

    pub fn set_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.active_tab = tab;
        cx.notify();
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let theme = &self.theme;
        let this = cx.entity().clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg)
            // ── Navigation bar (height 36px, y-offset 8px in Slint) ──
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(36.))
                    .px(px(8.))
                    .mt(px(8.))
                    // Back button (28x28, iconfont ← `\u{e62b}`)
                    .child(
                        div()
                            .w(px(28.))
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let this = this.clone();
                                move |_ev, _window, cx| {
                                    cx.emit(SettingsEvent::Back);
                                    let _ = this.update(cx, |_panel, cx| cx.notify());
                                }
                            })
                            .child(
                                div()
                                    .font_family("iconfont")
                                    .text_size(px(16.))
                                    .text_color(theme.text_2)
                                    .child("\u{e62b}"),
                            ),
                    )
                    // Title "Settings" (14px, 700 weight, text_1)
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text_1)
                            .child("Settings"),
                    ),
            )
            // ── Tab bar (height 36px, positioned below nav bar at y=52px in Slint) ──
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(36.))
                    .px(px(8.))
                    .mt(px(8.))
                    .border_b(px(1.))
                    .border_color(theme.divider)
                    .children(TAB_NAMES.iter().enumerate().map(|(i, name)| {
                        let is_active = i == active;
                        let tab_color = if is_active {
                            theme.accent
                        } else {
                            theme.text_2
                        };
                        let underline_bg = if is_active {
                            theme.accent
                        } else {
                            hsla(0., 0., 0., 0.)
                        };
                        let this = this.clone();

                        div()
                            .w_1_5()
                            .h_full()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                let _ = this.update(cx, |panel, cx| {
                                    panel.active_tab = i;
                                    cx.notify();
                                });
                            })
                            // Tab label
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(if is_active {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(tab_color)
                                    .child(*name),
                            )
                            // Active underline indicator (2px)
                            .child(
                                div()
                                    .w_full()
                                    .h(px(2.))
                                    .mt(px(4.))
                                    .bg(underline_bg),
                            )
                    })),
            )
            // ── Tab content (scrollable, fills remaining space) ──
            .child(
                div()
                    .flex_1()
                    .overflow_y_scroll()
                    .px(px(8.))
                    .pt(px(8.))
                    .child(match active {
                        0 => self.render_general_tab().into_any_element(),
                        1 => self.render_clipboard_tab().into_any_element(),
                        2 => self.render_hotkey_tab().into_any_element(),
                        3 => self.render_data_tab().into_any_element(),
                        4 => self.render_sync_tab().into_any_element(),
                        _ => div().into_any_element(),
                    }),
            )
    }
}

// ── Tab rendering stubs ──
// Each returns a placeholder container. Replace with actual settings
// controls when migrating individual tab content from Slint.
// Signature: `fn render_*_tab(&self) -> impl IntoElement`
// Extend by adding settings rows inside the returned div.

impl SettingsPanel {
    fn render_general_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("General settings")
    }

    fn render_clipboard_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Clipboard settings")
    }

    fn render_hotkey_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Hotkey settings")
    }

    fn render_data_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Data settings")
    }

    fn render_sync_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Sync settings")
    }
}
```

- [ ] **Step 2: 编译验证**

```bash
cd g:/Develop/github/clippi && cargo build 2>&1
```

期望：编译成功，无错误。

如果 `hsla(0., 0., 0., 0.)` 不编译通过，改用 `rgba(0x00000000)`。

如果 `FontWeight::BOLD` / `FontWeight::NORMAL` 不编译通过，检查 gpui 0.2.2 的 `FontWeight` 枚举变体名称（可能是 `BOLD` → `Bold` 或使用数字值 `FontWeight(700)` / `FontWeight(400)`）。

- [ ] **Step 3: 提交 settings/mod.rs**

```bash
git add src/ui/settings/mod.rs
git commit -m "feat: rewrite settings panel layout with nav bar, tab bar, and scroll content

- Add top navigation bar with back button (iconfont \u{e62b}) and 'Settings' title
- Rebuild tab bar with ClippiTheme accent color, bold/active states, 2px underline
- Replace placeholder text with scrollable content area routing to 5 tab stubs
- Define SettingsEvent::Back for return-to-clipboard navigation
- Use cx.entity().clone() pattern for internal state updates in callbacks

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: 在 RootView 中订阅 SettingsEvent::Back

**文件:**
- Modify: `src/ui/root.rs`

- [ ] **Step 1: 在 `RootView::new()` 的 `_subscriptions` 中添加 SettingsEvent::Back 订阅**

在 [src/ui/root.rs](src/ui/root.rs) 第 119 行的 `]` 之前添加新订阅。完整变更：在第 118 行 `},` 之后（titlebar 订阅的闭包结束），第 119 行 `];` 之前，插入：

```rust
            cx.subscribe(
                &settings_panel,
                move |this, _panel, event: &SettingsEvent, cx| match event {
                    SettingsEvent::Back => {
                        this.current_view = "clipboard".into();
                        cx.notify();
                    }
                },
            ),
```

即 `_subscriptions` vec 从：
```rust
        let _subscriptions = vec![
            cx.observe(&search_bar, |_this, _, cx| {
                cx.notify();
            }),
            cx.observe(&tag_filter_panel, |_this, _, cx| {
                cx.notify();
            }),
            cx.subscribe(
                &titlebar,
                move |this, _, event: &TitlebarEvent, cx| match event {
                    // ... TitlebarEvent::TogglePin / OpenSettings ...
                },
            ),
        ];
```

变为：
```rust
        let _subscriptions = vec![
            cx.observe(&search_bar, |_this, _, cx| {
                cx.notify();
            }),
            cx.observe(&tag_filter_panel, |_this, _, cx| {
                cx.notify();
            }),
            cx.subscribe(
                &titlebar,
                move |this, _, event: &TitlebarEvent, cx| match event {
                    // ... (unchanged) ...
                },
            ),
            cx.subscribe(
                &settings_panel,
                move |this, _panel, event: &SettingsEvent, cx| match event {
                    SettingsEvent::Back => {
                        this.current_view = "clipboard".into();
                        cx.notify();
                    }
                },
            ),
        ];
```

同时需要在文件顶部的 import 区域（第 18 行附近）添加 SettingsEvent 的导入。将：

```rust
use super::settings::SettingsPanel;
```

改为：

```rust
use super::settings::{SettingsEvent, SettingsPanel};
```

- [ ] **Step 2: 编译验证**

```bash
cd g:/Develop/github/clippi && cargo build 2>&1
```

期望：编译成功。

- [ ] **Step 3: 提交 root.rs**

```bash
git add src/ui/root.rs
git commit -m "feat: subscribe to SettingsEvent::Back in RootView

- Add SettingsEvent import alongside SettingsPanel
- Subscribe to settings_panel entity, switch view back to clipboard on Back event

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: 最终验证和清理

- [ ] **Step 1: 完整编译 + 检查 clippy 警告**

```bash
cd g:/Develop/github/clippi && cargo clippy 2>&1
```

期望：零警告 (clippy clean)。

- [ ] **Step 2: 确认 WindowManagerEvent::OpenSettings TODO**

检查 [src/ui/root.rs:81-86](src/ui/root.rs#L81-L86) 的 TODO 注释。当前 `WindowManagerEvent::OpenSettings` 被注释掉了。此次迁移完成后，可以解除注释（因为现在设置面板已有完整布局）：

将：
```rust
WindowManagerEvent::OpenSettings => {
    // TODO: Switch to settings view when settings panel
    // is fully migrated to GPUI.
    // this.current_view = "settings".into();
    // cx.notify();
}
```

改为：
```rust
WindowManagerEvent::OpenSettings => {
    this.current_view = "settings".into();
    this.search_bar
        .update(cx, |bar, cx| bar.close_tag_panel(cx));
    cx.notify();
}
```

- [ ] **Step 3: 最终 clippy 检查并提交**

```bash
cd g:/Develop/github/clippi && cargo clippy 2>&1
```

期望：零警告。

```bash
git add src/ui/root.rs
git commit -m "feat: enable OpenSettings from tray to switch to settings view

Settings panel now has full layout — remove TODO and activate the
WindowManagerEvent::OpenSettings handler.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### 完成检查清单

- [ ] `cargo build` 编译通过
- [ ] `cargo clippy` 零警告
- [ ] 设置面板显示：返回按钮 + "Settings" 标题 + 5 个标签
- [ ] 标签切换正常，选中态绿色 accent + 粗体
- [ ] 点击返回按钮回到剪贴板视图
- [ ] 从托盘菜单 "Open Settings" 能打开设置面板
- [ ] 内容区可滚动
- [ ] 标签页占位内容正确显示
