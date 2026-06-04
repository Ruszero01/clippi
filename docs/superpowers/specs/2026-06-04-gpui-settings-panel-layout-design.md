# GPUI 设置面板布局迁移 — 设计文档

**日期**: 2026-06-04
**分支**: experiment/gpui-migration
**状态**: 已批准

## 目标

将 Slint `SettingsPanel` 的整体布局、样式和返回按钮迁移到 GPUI，对齐视觉风格，预留功能接口供后续逐步完善各标签页内的具体设置项。

## 范围

**本次包含**：
- 顶部导航栏（返回按钮 + "Settings" 标题）
- 标签栏改造（绿色 accent 对齐、字重区分、等宽布局）
- 内容区域改为 ScrollView 包裹
- 使用 `ClippiTheme` 替换硬编码颜色
- 定义 `SettingsEvent::Back` 事件，通过 `cx.emit()` 发布
- 5 个标签页的占位渲染方法

**本次不包含**：
- 各标签页内的具体设置控件（Toggle、按钮组、输入框等）
- 设置项与 `AppSettings` 的数据绑定
- 同步后端管理面板

## 设计

### 组件层级

```
SettingsPanel (Entity)
├── 导航栏 (Navigation Bar) — 36px 高
│   ├── 返回按钮 (← iconfont `\u{e62b}`, 28x28, hover 变色)
│   └── "Settings" 标题 (14px, 700 weight, text_1)
├── 标签栏 (Tab Bar) — 36px 高
│   ├── General | Clipboard | Hotkey | Data | Sync (各 20% 宽)
│   ├── 选中: accent 绿色文字 + 2px accent 下划线
│   └── 未选中: text_2 灰色文字 + 透明下划线
└── 内容区 (ScrollView, flex_1)
    └── 条件渲染: render_general_tab / render_clipboard_tab / ...
```

### Slint → GPUI 映射

| Slint (`SettingsPanel.slint`) | GPUI (`settings/mod.rs`) |
|------|------|
| 第 86-122 行: 导航栏 HorizontalLayout | `render_nav_bar()` 方法 |
| 第 102 行: `back-ta` TouchArea → `root.back()` | `cx.emit(SettingsEvent::Back)` |
| 第 108 行: 图标 `\u{e62b}` font-family "iconfont" | `.font_family("iconfont").child("\u{e62b}")` |
| 第 111 行: hover 态颜色 `accent` | `cx.on_mouse_...` + `hover` 状态 |
| 第 125-291 行: 标签栏 horizontal | `render_tab_bar()` 方法 |
| 第 157 行: accent 下划线 + 200ms animate | 直接用 accent 颜色（暂不加动画） |
| 第 294-380 行: ScrollView 内容区 | `div().flex_1().overflow_y_scroll()` |
| 第 80-81 行: 主题色从 dark-mode 派生 | `ClippiTheme::from_setting()` |

### 数据结构

```rust
pub struct SettingsPanel {
    active_tab: usize,
    settings: AppSettings,
    theme: ClippiTheme,
}

pub enum SettingsEvent {
    Back,
}

impl EventEmitter<SettingsEvent> for SettingsPanel {}
```

### 事件流

```
SettingsPanel (返回按钮 on_mouse_down)
    → cx.emit(SettingsEvent::Back)
    → RootView (订阅 SettingsPanel 事件)
        → this.current_view = "clipboard"
        → cx.notify()
```

与现有 `TitlebarEvent::OpenSettings` 流程对称：
```
Titlebar (设置按钮) → TitlebarEvent::OpenSettings
    → RootView: current_view = "settings"
```

### 文件变更

1. **`src/ui/settings/mod.rs`** — 主变更文件，重写 `render()` 方法 + 新增事件
2. **`src/ui/root.rs`** — 订阅 `SettingsEvent::Back`，切换视图回剪贴板

### 主题颜色使用

| 元素 | Slint | GPUI `ClippiTheme` |
|------|-------|-----|
| 面板背景 | `bg` = `#191a1b` | `theme.bg` |
| 导航栏/标签栏背景 | 面板背景 | `theme.bg` |
| 标题文字 | `dark ? #eaebec : #1a1c2e` | `theme.text_1` |
| 未选中标签文字 | `text-2` = `#919496` | `theme.text_2` |
| 选中标签/accent | `accent` = `#7ecba3` | `theme.accent` |
| 标签栏底部分隔线 | (Slint 无) | `theme.divider` |
| 占位文字 | (Slint 无占位) | `theme.text_3` |

### 预留接口

每个标签页渲染方法签名：

```rust
fn render_general_tab(&self, theme: &ClippiTheme) -> impl IntoElement
fn render_clipboard_tab(&self, theme: &ClippiTheme) -> impl IntoElement
fn render_hotkey_tab(&self, theme: &ClippiTheme) -> impl IntoElement
fn render_data_tab(&self, theme: &ClippiTheme) -> impl IntoElement
fn render_sync_tab(&self, theme: &ClippiTheme) -> impl IntoElement
```

这些方法目前返回简单的占位容器（标签页标题 + "coming soon" 提示），后续可直接替换内容。

## 待解决 / 后续

- 标签栏下划线切换动画（GPUI 目前无内置 transition API，后续可考虑用 `Animation` 扩展）
- 标签页内的具体设置控件（Toggle、按钮选择组、数字输入等）
- 设置项与 `AppSettings` 的数据双向绑定
- `WindowManagerEvent::OpenSettings` 的 TODO 解除（[root.rs:81-86](src/ui/root.rs#L81-L86)）
