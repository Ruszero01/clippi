# GPUI 悬浮工具栏与右键菜单迁移 — 设计文档

> **目标:** 将条目卡片悬浮工具栏和右键菜单从 Slint 迁移到 GPUI，对齐 Slint 版本的完整功能。

**日期:** 2026-06-03
**分支:** experiment/gpui-migration
**参考:** Slint 版 `ui/ContextMenu.slint`、`ui/ClipboardList.slint`（卡片悬浮工具栏区域）

---

## 架构概览

```
RootView (叠加层宿主)
├── ClipboardList (虚拟列表 + 上下文菜单状态)
│   └── ClipboardCard (悬停状态 + HoverToolbar 内嵌)
│       └── HoverToolbar (条件按钮组)
├── ContextMenu (绝对定位浮层，RootView 渲染)
└── TagPickerPanel (已有，绝对定位浮层)
```

### 事件流

```
右键卡片 → ClipboardCard.on_right_click
  → ClipboardList 记录 (visible, x, y, item, is_batch)
  → RootView render 检测 visible → 渲染 ContextMenu 叠加层
  → 菜单项点击 → ContextMenu.on_action(action)
    → ClipboardList 处理 action → hide menu
  → 点击背景 → hide menu

悬停卡片 → ClipboardCard mouse_enter/mouse_leave
  → 切换 hovered → render HoverToolbar
  → 工具栏按钮点击 → on_toolbar_action(action)
    → ClipboardList 处理 action
```

---

## 组件设计

### 1. HoverToolbar (`src/ui/hover_toolbar.rs` — 新文件)

独立组件，不需要 Entity，使用 `RenderOnce`。

```rust
pub struct HoverToolbar {
    /// Item properties that determine which buttons to show.
    props: HoverToolbarProps,
    /// Called when any button is clicked: action name string.
    on_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
}

pub struct HoverToolbarProps {
    pub content_type: ContentType,
    pub is_image: bool,
    pub has_qr_code: bool,
    pub is_favorite: bool,
    pub selected_count: usize,
    pub is_selected: bool,
}
```

**外观规格：**
- 高度 22px，圆角 6px
- 背景 `rgba(0x232425, 0.91)` (dark) / `rgba(0xffffff, 0.91)` (light)
- 1px 边框，圆角 pill
- 内部 18×18 按钮，2px 间距，5px 左右内边距
- 按钮 hover 时图标色变为 accent
- 收藏按钮已收藏时图标色为 fav-color (#d8a155)
- 删除按钮 hover 时图标色变为 danger (#ff5f57)

**单条模式按钮 (selected_count <= 1):**

| 按钮 | iconfont | 显示条件 |
|------|----------|---------|
| 复制 | `\u{e600}` | 始终 |
| 打开原图 | `\u{e626}` | content_type == Image |
| 二维码 | `\u{e605}` | content_type == Image && has_qr_code |
| 打开位置 | `\u{e6d7}` | Link / Path / File |
| 编辑 | `\u{e648}` | 非 Image 且非 File |
| 备注 | `\u{e606}` | 始终 |
| 收藏 | `\u{e630}`(已收藏) / `\u{e68d}`(未收藏) | 始终 |
| 删除 | `\u{e8b6}` | 始终 |

**批量模式按钮 (selected_count > 1 && is_selected):**

| 按钮 | iconfont |
|------|----------|
| 批量粘贴 | `\u{e600}` |
| 批量收藏 | `\u{e630}` |
| 批量删除 | `\u{e8b6}` |

**Action 名称：**
- 单条：`"copy"`, `"open_image"`, `"qr_action"`, `"open_location"`, `"edit"`, `"edit_note"`, `"toggle_favorite"`, `"delete"`
- 批量：`"batch_paste"`, `"batch_favorite"`, `"batch_delete"`

---

### 2. ContextMenu (`src/ui/context_menu.rs` — 重写)

使用 `MenuItem` 结构体构建菜单项列表，支持条件项和分隔线。

```rust
pub struct MenuItem {
    pub label: String,
    pub action: String,
    pub icon: String,
    pub danger: bool,
    pub fav_item: bool,
}

pub struct ContextMenu {
    items: Vec<MenuItem>,
    /// Position (top-left corner)
    x: f32,
    y: f32,
    /// Container bounds for clamping
    container_width: f32,
    container_height: f32,
    on_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
    on_dismiss: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}
```

**构建 API：**

```rust
// 单条菜单 — 根据条目属性动态构建
ContextMenu::for_item(&MenuItemContext {
    is_image: bool,
    is_file: bool,
    is_color: bool,
    is_hex: bool,       // true = show "Paste as RGB", false = show "Paste as HEX"
    is_favorite: bool,
})
.with_position(x, y, container_width, container_height)
.on_action(handler)
.on_dismiss(handler)

// 批量菜单
ContextMenu::for_batch(selected_count: usize)
.with_position(x, y, container_width, container_height)
.on_action(handler)
.on_dismiss(handler)
```

**单条菜单项完整列表：**

| 菜单项 | action | iconfont | 显示条件 |
|--------|--------|----------|---------|
| 复制 | `"copy"` | `\u{e600}` | 始终 |
| 粘贴 | `"paste"` | `\u{e600}` | 始终 |
| 粘贴为 RGB / HEX | `"paste_as_rgb"` / `"paste_as_hex"` | `\u{e610}` | is_color |
| --- 分隔线 --- | | | |
| 编辑 | `"edit"` | `\u{e648}` | 非 image/file |
| 备注 | `"edit_note"` | `\u{e606}` | 始终 |
| 打开原图 | `"open_image"` | `\u{e626}` | is_image |
| 粘贴 OCR 文本 | `"paste_ocr"` | `\u{e648}` | is_image |
| 识别二维码 | `"qr_detect"` | `\u{e605}` | is_image |
| 标签 | `"show_tag_picker"` | `\u{ec07}` | 始终 |
| --- 分隔线 --- | | | |
| 收藏/取消收藏 | `"toggle_favorite"` | `\u{e630}`/`\u{e68d}` | 始终 |
| 删除 | `"delete"` | `\u{e8b6}` | 始终 |

**批量菜单项：**

| 菜单项 | action | iconfont |
|--------|--------|----------|
| 粘贴 N 项 | `"batch_paste"` | `\u{e600}` |
| --- 分隔线 --- | | |
| 批量标签 | `"show_tag_picker"` | `\u{ec07}` |
| --- 分隔线 --- | | |
| 批量收藏 | `"batch_favorite"` | `\u{e630}` |
| 批量删除 | `"batch_delete"` | `\u{e8b6}` |

**外观规格：**
- 164px 宽，8px 圆角，4px 内边距
- 背景色匹配主题 `surface`
- 阴影 + 1px 边框
- 每个菜单项 30px 高，5px 圆角，8px 左右内边距
- 图标 13px iconfont + 标签 13px 文字，8px 间距
- hover 时背景变 `btn-hover`，图标/文字变 accent
- 删除项 hover 时变 danger
- 收藏项图标/文字为 fav-color
- 位置使用 clamp 防止超出容器边界
- 点击菜单外部 → 触发 `on_dismiss`

---

### 3. ClipboardCard 改动 (`src/ui/clipboard_card.rs`)

新增字段和回调：

```rust
pub struct ClipboardCard {
    // ... 现有字段
    hovered: bool,                                           // 新增
    on_toolbar_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,  // 新增
    on_mouse_enter: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,   // 新增
    on_mouse_leave: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,   // 新增
}
```

- `on_mouse_enter`/`on_mouse_leave` 通过 GPUI 的 `hover()` 伪类或 `MouseMove` 事件实现悬停检测
- hover 时在卡片右上角渲染 `HoverToolbar`
- `HoverToolbarProps` 从 `ClipboardItem` 和外部传入的 `selected_count` 推导
- `toolbar_action` 标记阻止工具栏点击冒泡到卡片点击

---

### 4. ClipboardList 改动 (`src/ui/clipboard_list.rs`)

新增上下文菜单状态：

```rust
pub struct ClipboardListView {
    // ... 现有字段
    // Context menu state
    context_menu_visible: bool,
    context_menu_x: f32,
    context_menu_y: f32,
    context_menu_item: Option<ClipboardItem>,
    context_menu_is_batch: bool,
}
```

**新增方法：**
- `context_menu_state() -> ContextMenuState` — 供 RootView 读取
- `hide_context_menu(&mut self, cx)` — 隐藏菜单
- `handle_context_menu_action(&mut self, action: &str, cx)` — 分发菜单 action

**虚拟列表行渲染改动：**
- 每行传递 `selected_count` 和 `is_selected` 给卡片
- 传递 `on_right_click` handler，在 handler 中设置上下文菜单状态
- 传递 `on_toolbar_action` handler

---

### 5. RootView 改动 (`src/ui/root.rs`)

- 读取 `ClipboardList` 的上下文菜单状态
- 当 `visible=true` 时渲染：
  ```rust
  if context_menu_visible {
      // Backdrop — 点击关闭菜单
      div().absolute().size_full().on_mouse_down(MouseButton::Left, dismiss_handler)
      // ContextMenu — 绝对定位
      div().absolute().left(x).top(y).occlude().child(context_menu)
  }
  ```
- 标签选择器（已有 TagPickerPanel）也通过上下文菜单触发：`show_tag_picker` action → 切换到 tag picker overlay

---

### 6. 模块注册 (`src/ui/mod.rs`)

- 新增 `pub mod hover_toolbar;`
- 现有 `context_menu` 模块保留，内部重写

---

## 主题色映射

沿用 Slint 暗色主题色板（当前项目使用暗色模式），与现有 `ClipboardCard` 一致：

```rust
accent:     rgb(0x7ecba3)  // #7ecba3
text_1:     rgb(0xeaebec)  // #eaebec
text_2:     rgb(0x919496)  // #919496
danger:     rgb(0xff5f57)  // #ff5f57
fav_color:  rgb(0xd8a155)  // #d8a155
btn_hover:  rgb(0x2b2c2d)  // #2b2c2d
surface:    rgb(0x2c2d2e)  // #2c2d2e
pill_bg:    rgba(0x232425e8)
pill_border: rgba(0xffffff20)
```

---

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/ui/hover_toolbar.rs` | **新建** | 悬浮工具栏组件 (~150 行) |
| `src/ui/context_menu.rs` | **重写** | 支持条件菜单项、iconfont (~200 行) |
| `src/ui/clipboard_card.rs` | **修改** | 添加 hover 状态、HoverToolbar 嵌入 |
| `src/ui/clipboard_list.rs` | **修改** | 右键菜单状态管理、action 路由 |
| `src/ui/root.rs` | **修改** | 渲染 ContextMenu 叠加层 |
| `src/ui/mod.rs` | **修改** | 注册 hover_toolbar 模块 |
