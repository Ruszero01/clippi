# GPUI 悬浮工具栏与右键菜单迁移 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将条目卡片悬浮工具栏（HoverToolbar）和右键菜单（ContextMenu）从 Slint 迁移到 GPUI，对齐 Slint 版本的完整功能，UI 就绪、回调接口预留。

**Architecture:** HoverToolbar 和 ContextMenu 均为独立 `RenderOnce` 组件。ClipboardCard 通过 `is_hovered` 字段条件渲染 HoverToolbar，悬停状态由 ClipboardList 的 `on_mouse_move` 追踪。ContextMenu 作为 RootView 叠加层，状态（可见性、位置、目标条目）由 ClipboardList 管理。

**Tech Stack:** Rust, GPUI 0.2.2, gpui-component 0.5

---

## 文件结构

| 文件 | 操作 | 职责 |
|------|------|------|
| `src/ui/hover_toolbar.rs` | 新建 | HoverToolbar 组件 — 条件按钮组 |
| `src/ui/context_menu.rs` | 重写 | ContextMenu 组件 — 条件菜单项、iconfont |
| `src/ui/clipboard_card.rs` | 修改 | 添加 is_hovered 字段、嵌入 HoverToolbar |
| `src/ui/clipboard_list.rs` | 修改 | 悬停追踪、右键菜单状态管理、action 路由 |
| `src/ui/root.rs` | 修改 | 渲染 ContextMenu 叠加层 |
| `src/ui/mod.rs` | 修改 | 注册 hover_toolbar 模块 |

---

### Task 1: 创建 HoverToolbar 组件

**Files:**
- Create: `src/ui/hover_toolbar.rs`

- [ ] **Step 1: 创建 `src/ui/hover_toolbar.rs`**

```rust
//! Hover toolbar — appears on card hover, top-right corner.
//!
//! Matches the original Slint ClipboardList.slint hover toolbar:
//! - 22px height pill, 6px border-radius
//! - Semi-transparent background, 1px border
//! - 18×18 iconfont buttons with 2px spacing
//! - Conditional buttons based on content type and selection count

use std::rc::Rc;

use gpui::*;

use crate::core::types::ContentType;

/// Properties that determine which toolbar buttons to show.
pub struct HoverToolbarProps {
    pub content_type: ContentType,
    pub is_image: bool,
    pub has_qr_code: bool,
    pub is_favorite: bool,
    pub selected_count: usize,
    pub is_selected: bool,
}

impl HoverToolbarProps {
    /// Derive from a ClipboardItem with external selection context.
    pub fn from_item(item: &crate::core::types::ClipboardItem, selected_count: usize, is_selected: bool) -> Self {
        Self {
            content_type: item.content_type,
            is_image: item.content_type == ContentType::Image,
            has_qr_code: item.has_qr_code,
            is_favorite: item.is_favorite,
            selected_count,
            is_selected,
        }
    }
}

#[derive(IntoElement)]
pub struct HoverToolbar {
    props: HoverToolbarProps,
    on_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
}

impl HoverToolbar {
    pub fn new(props: HoverToolbarProps) -> Self {
        Self {
            props,
            on_action: None,
        }
    }

    pub fn on_action(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for HoverToolbar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self { props, on_action } = self;

        // Theme colors (dark mode — matching Slint original)
        let accent = rgb(0x7ecba3);
        let text_2 = rgb(0x919496);
        let danger = rgb(0xff5f57);
        let fav_color = rgb(0xd8a155);
        let pill_bg = rgba(0x232425e8);
        let pill_border = rgba(0xffffff20);

        let is_single = props.selected_count <= 1;
        let is_batch = props.selected_count > 1 && props.is_selected;

        // ── Build button list ──
        // Each entry: (icon, action_name, hover_color_fn)
        let mut buttons: Vec<(&str, &str, Box<dyn Fn(bool) -> Rgba>)> = Vec::new();

        if is_single {
            // ── Single-item toolbar ──
            buttons.push(("\u{e600}", "copy", Box::new(|hovered: bool| if hovered { accent } else { text_2 })));

            if props.content_type == ContentType::Image {
                buttons.push(("\u{e626}", "open_image", Box::new(|hovered: bool| if hovered { accent } else { text_2 })));
            }

            if props.content_type == ContentType::Image && props.has_qr_code {
                buttons.push(("\u{e605}", "qr_action", Box::new(|hovered: bool| if hovered { accent } else { text_2 })));
            }

            if props.content_type == ContentType::Link
                || props.content_type == ContentType::Path
                || props.content_type == ContentType::File
            {
                buttons.push(("\u{e6d7}", "open_location", Box::new(|hovered: bool| if hovered { accent } else { text_2 })));
            }

            if props.content_type != ContentType::Image && props.content_type != ContentType::File {
                buttons.push(("\u{e648}", "edit", Box::new(|hovered: bool| if hovered { accent } else { text_2 })));
            }

            buttons.push(("\u{e606}", "edit_note", Box::new(|hovered: bool| if hovered { accent } else { text_2 })));

            // Favorite: icon changes based on state, color is fav_color
            let fav_icon = if props.is_favorite { "\u{e630}" } else { "\u{e68d}" };
            buttons.push((fav_icon, "toggle_favorite", Box::new(move |_hovered: bool| fav_color)));

            buttons.push(("\u{e8b6}", "delete", Box::new(|hovered: bool| if hovered { danger } else { text_2 })));
        } else if is_batch {
            // ── Batch toolbar ──
            buttons.push(("\u{e600}", "batch_paste", Box::new(|hovered: bool| if hovered { accent } else { text_2 })));
            buttons.push(("\u{e630}", "batch_favorite", Box::new(move |_hovered: bool| fav_color)));
            buttons.push(("\u{e8b6}", "batch_delete", Box::new(|hovered: bool| if hovered { danger } else { text_2 })));
        }

        if buttons.is_empty() {
            return div();
        }

        // Compute width: N buttons × 18px + (N-1) × 2px spacing + 10px padding
        let n = buttons.len();
        let content_w = (n * 18 + (n.saturating_sub(1)) * 2) as f32;
        let toolbar_w = content_w + 10.0;

        let on_action_clone = on_action.clone();
        div()
            .h(px(22.))
            .w(px(toolbar_w))
            .rounded(px(6.))
            .bg(pill_bg)
            .border(px(1.))
            .border_color(pill_border)
            .flex()
            .flex_row()
            .px(px(5.))
            .items_center()
            .gap(px(2.))
            .children(buttons.into_iter().map(move |(icon, action, color_fn)| {
                let on_action = on_action_clone.clone();
                let action = action.to_string();
                let icon = icon.to_string();
                let color_fn = std::rc::Rc::new(color_fn);

                div()
                    .w(px(18.))
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(rgba(0xffffff10)))
                    .child({
                        let icon = icon.clone();
                        let color = color_fn(false);
                        div()
                            .font_family("iconfont")
                            .text_size(px(12.))
                            .text_color(color)
                            .hover({
                                let color_hover = color_fn(true);
                                move |style| style.text_color(color_hover)
                            })
                            .child(icon)
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let action = action.clone();
                        let on_action = on_action.clone();
                        move |_ev, window, cx| {
                            if let Some(ref handler) = on_action {
                                handler(&action, window, cx);
                            }
                        }
                    })
            }))
    }
}
```

- [ ] **Step 2: 编译检查**

```bash
cargo build 2>&1
```

预期：由于 `hover_toolbar` 模块尚未注册，此文件不会被编译。需要在 Task 6 注册后才能完整编译。

---

### Task 2: 重写 ContextMenu 组件

**Files:**
- Rewrite: `src/ui/context_menu.rs`

- [ ] **Step 1: 重写 `src/ui/context_menu.rs`**

```rust
//! Context menu — right-click menu for clipboard items.
//!
//! Matches the original Slint ContextMenu.slint design:
//! - 164px width, 8px border-radius, 4px padding
//! - 30px item height, 5px border-radius per item
//! - Icon (13px iconfont) + label (13px) per row
//! - Separators: 3px gap + 1px line + 3px gap
//! - Single and batch mode variants with conditional items
//! - Position clamping to container bounds

use std::rc::Rc;

use gpui::*;

/// Context describing which menu items to show.
pub struct MenuItemContext {
    pub is_image: bool,
    pub is_file: bool,
    pub is_color: bool,
    pub is_hex: bool,
    pub is_favorite: bool,
}

impl Default for MenuItemContext {
    fn default() -> Self {
        Self {
            is_image: false,
            is_file: false,
            is_color: false,
            is_hex: false,
            is_favorite: false,
        }
    }
}

impl MenuItemContext {
    pub fn from_item(item: &crate::core::types::ClipboardItem) -> Self {
        use crate::core::types::ContentType;
        let text = item.full_text.trim();
        let is_color = item.content_type == ContentType::Color;
        // Determine if the color is in HEX format (starts with #)
        // is_hex = true → show "Paste as RGB" (convert FROM hex)
        // is_hex = false → show "Paste as HEX" (convert FROM rgb/hsl)
        let is_hex = if is_color {
            let t = text.trim_start().to_lowercase();
            t.starts_with('#')
        } else {
            false
        };
        // is_hex = true means "Paste as RGB" shown (current format IS hex),
        // is_hex = false means "Paste as HEX" shown (current format IS rgb/hsl)
        Self {
            is_image: item.content_type == ContentType::Image,
            is_file: item.content_type == ContentType::File,
            is_color,
            is_hex, // true → show "Paste as RGB", false → show "Paste as HEX"
            is_favorite: item.is_favorite,
        }
    }
}

#[derive(IntoElement)]
pub struct ContextMenu {
    items: Vec<RawMenuItem>,
    x: f32,
    y: f32,
    container_width: f32,
    container_height: f32,
    on_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
    on_dismiss: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

/// Internal menu item descriptor.
struct RawMenuItem {
    label: String,
    action: String,
    icon: String,
    danger: bool,
    fav: bool,
}

const SEPARATOR_LABEL: &str = "__sep__";
const MENU_WIDTH: f32 = 164.0;

impl ContextMenu {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            x: 0.0,
            y: 0.0,
            container_width: f32::MAX,
            container_height: f32::MAX,
            on_action: None,
            on_dismiss: None,
        }
    }

    /// Build a single-item context menu from item context.
    pub fn for_item(ctx: &MenuItemContext) -> Self {
        let mut items: Vec<RawMenuItem> = Vec::new();

        // Copy
        items.push(RawMenuItem {
            label: "Copy".into(),
            action: "copy".into(),
            icon: "\u{e600}".into(),
            danger: false,
            fav: false,
        });
        // Paste
        items.push(RawMenuItem {
            label: "Paste".into(),
            action: "paste".into(),
            icon: "\u{e600}".into(),
            danger: false,
            fav: false,
        });

        // Color conversion (only for color type)
        if ctx.is_color {
            let (label, action) = if ctx.is_hex {
                ("Paste as RGB", "paste_as_rgb")
            } else {
                ("Paste as HEX", "paste_as_hex")
            };
            items.push(RawMenuItem {
                label: label.into(),
                action: action.into(),
                icon: "\u{e610}".into(),
                danger: false,
                fav: false,
            });
        }

        // Separator
        items.push(RawMenuItem {
            label: SEPARATOR_LABEL.into(),
            action: String::new(),
            icon: String::new(),
            danger: false,
            fav: false,
        });

        // Edit (not for image/file)
        if !ctx.is_image && !ctx.is_file {
            items.push(RawMenuItem {
                label: "Edit".into(),
                action: "edit".into(),
                icon: "\u{e648}".into(),
                danger: false,
                fav: false,
            });
        }

        // Note
        items.push(RawMenuItem {
            label: "Note".into(),
            action: "edit_note".into(),
            icon: "\u{e606}".into(),
            danger: false,
            fav: false,
        });

        // Open original image (image only)
        if ctx.is_image {
            items.push(RawMenuItem {
                label: "Open image".into(),
                action: "open_image".into(),
                icon: "\u{e626}".into(),
                danger: false,
                fav: false,
            });
            items.push(RawMenuItem {
                label: "Paste OCR text".into(),
                action: "paste_ocr".into(),
                icon: "\u{e648}".into(),
                danger: false,
                fav: false,
            });
            items.push(RawMenuItem {
                label: "Detect QR Code".into(),
                action: "qr_detect".into(),
                icon: "\u{e605}".into(),
                danger: false,
                fav: false,
            });
        }

        // Tag
        items.push(RawMenuItem {
            label: "Tag".into(),
            action: "show_tag_picker".into(),
            icon: "\u{ec07}".into(),
            danger: false,
            fav: false,
        });

        // Separator
        items.push(RawMenuItem {
            label: SEPARATOR_LABEL.into(),
            action: String::new(),
            icon: String::new(),
            danger: false,
            fav: false,
        });

        // Favorite
        let (fav_label, fav_icon) = if ctx.is_favorite {
            ("Unfav", "\u{e630}")
        } else {
            ("Fav", "\u{e68d}")
        };
        items.push(RawMenuItem {
            label: fav_label.into(),
            action: "toggle_favorite".into(),
            icon: fav_icon.into(),
            danger: false,
            fav: true,
        });

        // Delete
        items.push(RawMenuItem {
            label: "Delete".into(),
            action: "delete".into(),
            icon: "\u{e8b6}".into(),
            danger: true,
            fav: false,
        });

        Self::new().items(items)
    }

    /// Build a batch context menu.
    pub fn for_batch(selected_count: usize) -> Self {
        let items = vec![
            RawMenuItem {
                label: format!("Paste {} items", selected_count),
                action: "batch_paste".into(),
                icon: "\u{e600}".into(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: SEPARATOR_LABEL.into(),
                action: String::new(),
                icon: String::new(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: "Batch tag".into(),
                action: "show_tag_picker".into(),
                icon: "\u{ec07}".into(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: SEPARATOR_LABEL.into(),
                action: String::new(),
                icon: String::new(),
                danger: false,
                fav: false,
            },
            RawMenuItem {
                label: "Batch fav".into(),
                action: "batch_favorite".into(),
                icon: "\u{e630}".into(),
                danger: false,
                fav: true,
            },
            RawMenuItem {
                label: "Batch delete".into(),
                action: "batch_delete".into(),
                icon: "\u{e8b6}".into(),
                danger: true,
                fav: false,
            },
        ];
        Self::new().items(items)
    }

    fn items(mut self, items: Vec<RawMenuItem>) -> Self {
        self.items = items;
        self
    }

    pub fn with_position(
        mut self,
        x: f32,
        y: f32,
        container_width: f32,
        container_height: f32,
    ) -> Self {
        self.x = x;
        self.y = y;
        self.container_width = container_width;
        self.container_height = container_height;
        self
    }

    pub fn on_action(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_action = Some(Rc::new(handler));
        self
    }

    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            items,
            x,
            y,
            container_width,
            container_height,
            on_action,
            on_dismiss,
        } = self;

        // Dark theme colors (matching Slint original)
        let surface = rgb(0x2c2d2e);
        let sep_line = rgba(0xffffff0d);
        let btn_hover = rgb(0x2b2c2d);
        let accent = rgb(0x7ecba3);
        let text_1 = rgb(0xeaebec);
        let text_2 = rgb(0x919496);
        let danger = rgb(0xff5f57);
        let fav_color = rgb(0xd8a155);

        // Clamp position to container bounds
        let menu_w = MENU_WIDTH;
        let clamped_x = x.clamp(4.0, (container_width - menu_w - 4.0).max(4.0));
        // Estimate height roughly; exact clamping is done by the caller.
        let clamped_y = y.clamp(4.0, (container_height - 4.0).max(4.0));

        // Count separator items to correctly estimate height for clamping
        let item_count = items.len();

        div()
            .absolute()
            .left(px(clamped_x))
            .top(px(clamped_y))
            .w(px(menu_w))
            .rounded(px(8.))
            .bg(surface)
            .border(px(1.))
            .border_color(rgba(0xffffff14))
            .shadow_lg()
            .p(px(4.))
            .flex()
            .flex_col()
            .children(items.into_iter().enumerate().map(|(idx, item)| {
                let on_action = on_action.clone();
                let on_dismiss = on_dismiss.clone();

                // Render separator
                if item.label == SEPARATOR_LABEL {
                    return div()
                        .w(px(156.))
                        .flex()
                        .flex_col()
                        .child(div().h(px(3.)))
                        .child(div().w_full().h(px(1.)).bg(sep_line))
                        .child(div().h(px(3.)));
                }

                let is_danger = item.danger;
                let is_fav = item.fav;
                let action = item.action.clone();
                let icon = item.icon.clone();
                let label = item.label.clone();
                let is_last = idx == item_count - 1;

                let normal_icon = if is_fav { fav_color } else if is_danger { danger } else { text_2 };
                let normal_text = if is_fav { fav_color } else if is_danger { danger } else { text_1 };

                // Hover colors: fav stays fav, danger stays danger, otherwise accent
                let hover_icon = if is_fav { fav_color } else if is_danger { danger } else { accent };
                let hover_text = if is_fav { fav_color } else if is_danger { danger } else { accent };

                div()
                    .w(px(156.))
                    .h(px(30.))
                    .rounded(px(5.))
                    .when(is_last, |el| el.mb(px(0.)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(px(8.))
                    .gap(px(8.))
                    .cursor(CursorStyle::PointingHand)
                    .hover(move |style| style.bg(btn_hover))
                    .child({
                        let icon = icon.clone();
                        div()
                            .font_family("iconfont")
                            .text_size(px(13.))
                            .text_color(normal_icon)
                            .hover(move |style| style.text_color(hover_icon))
                            .child(icon)
                    })
                    .child({
                        let label = label.clone();
                        div()
                            .text_size(px(13.))
                            .text_color(normal_text)
                            .hover(move |style| style.text_color(hover_text))
                            .child(label)
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let action = action.clone();
                        let on_dismiss = on_dismiss.clone();
                        move |_ev, window, cx| {
                            if let Some(ref handler) = on_action {
                                handler(&action, window, cx);
                            }
                            // Dismiss after action
                            if let Some(ref dismiss) = on_dismiss {
                                dismiss(window, cx);
                            }
                        }
                    })
            }))
    }
}
```

- [ ] **Step 2: 删除旧 ContextMenu 中不再需要的字段和方法**

旧文件中的 `MenuItem`（public struct）、`SEPARATOR` 常量、`single_item()`、`batch()` 方法被完全替换。整个文件已经在上一步完整重写。

---

### Task 3: 修改 ClipboardCard — 支持 hover 和 HoverToolbar

**Files:**
- Modify: `src/ui/clipboard_card.rs`

- [ ] **Step 1: 添加 is_hovered 字段和 on_toolbar_action 回调**

在 `ClipboardCard` 结构体（约第 502-509 行）的现有字段之后添加：

```rust
    is_hovered: bool,
    selected_count: usize,
    on_toolbar_action: Option<Rc<dyn Fn(&str, &mut Window, &mut App)>>,
```

- [ ] **Step 2: 修改构造函数和 builder 方法**

将 `new()` 方法中初始化后添加默认值：

在 `ClipboardCard::new()` 的 `Self { ... }` 构造体中，现有字段之后加入：

```rust
            is_hovered: false,
            selected_count: 0,
            on_toolbar_action: None,
```

新增 builder 方法（在 `on_right_click` 方法之后）：

```rust
    pub fn is_hovered(mut self, hovered: bool) -> Self {
        self.is_hovered = hovered;
        self
    }

    pub fn selected_count(mut self, count: usize) -> Self {
        self.selected_count = count;
        self
    }

    pub fn on_toolbar_action(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toolbar_action = Some(Rc::new(handler));
        self
    }
```

- [ ] **Step 3: 在 RenderOnce 的 render 方法中添加 HoverToolbar 渲染**

在 `RenderOnce` 的 `render` 方法解构中添加新字段：

```rust
        let Self {
            item,
            selected,
            index,
            selection_order,
            on_click,
            on_right_click,
            is_hovered,
            selected_count,
            on_toolbar_action,
        } = self;
```

在 `render` 方法的 card 组装代码的末尾（`// ── Assemble card ──` 之后的 card 变量），添加 HoverToolbar：

```rust
        // ── Hover toolbar ──
        let card = if is_hovered {
            let toolbar_props =
                HoverToolbarProps::from_item(&item, selected_count, selected);
            let toolbar_action = on_toolbar_action.clone();
            card.child(
                div()
                    .absolute()
                    .top(px(3.))
                    .right(px(4.))
                    .child(super::hover_toolbar::HoverToolbar::new(toolbar_props)
                        .on_action(move |action, window, cx| {
                            if let Some(ref handler) = toolbar_action {
                                handler(action, window, cx);
                            }
                        })),
            )
        } else {
            card
        };
```

- [ ] **Step 4: 添加 use 导入**

在文件顶部的 `use` 区域，添加：

```rust
use super::hover_toolbar::{HoverToolbar, HoverToolbarProps};
```

---

### Task 4: 修改 ClipboardList — 悬停追踪和右键菜单状态

**Files:**
- Modify: `src/ui/clipboard_list.rs`

- [ ] **Step 1: 添加新字段到 ClipboardListView 结构体**

在 `ClipboardListView` 结构体的 `state` 字段之后添加：

```rust
    // ── Hover tracking ──
    hovered_index: Option<usize>,
    // ── Context menu state ──
    context_menu_visible: bool,
    context_menu_x: f32,
    context_menu_y: f32,
    context_menu_item: Option<ClipboardItem>,
    context_menu_is_batch: bool,
    // ── Selected count (cached for toolbar/menu) ──
    selected_count: usize,
```

在 `ClipboardListView::new()` 的 `Self { ... }` 构造体中，`state` 之后添加：

```rust
            hovered_index: None,
            context_menu_visible: false,
            context_menu_x: 0.0,
            context_menu_y: 0.0,
            context_menu_item: None,
            context_menu_is_batch: false,
            selected_count: 0,
```

- [ ] **Step 2: 添加公开方法**

在 `ClipboardListView` 的 `impl` 块中，`compute_sizes` 方法之前，添加：

```rust
    /// Return the current context menu state for RootView to read.
    pub fn context_menu_visible(&self) -> bool {
        self.context_menu_visible
    }

    pub fn context_menu_is_batch(&self) -> bool {
        self.context_menu_is_batch
    }

    pub fn context_menu_position(&self) -> (f32, f32) {
        (self.context_menu_x, self.context_menu_y)
    }

    pub fn context_menu_item(&self) -> Option<&ClipboardItem> {
        self.context_menu_item.as_ref()
    }

    /// Hide the context menu.
    fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_visible = false;
        self.context_menu_item = None;
        cx.notify();
    }

    /// Handle a context menu action string.
    fn handle_menu_action(
        &mut self,
        action: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Map action string to the appropriate AppState or service call.
        // For now, actions are forwarded; actual implementation exists in
        // the Rust backend and will be wired by RootView.
        log::info!("Context menu action: {}", action);
        // Actions that the menu directly handles:
        match action {
            "copy" | "paste" | "edit" | "edit_note" | "toggle_favorite" | "delete"
            | "paste_as_rgb" | "paste_as_hex" | "open_image" | "paste_ocr"
            | "qr_detect" | "show_tag_picker" | "batch_paste" | "batch_favorite"
            | "batch_delete" => {
                // TODO: Wire to AppState services in follow-up migration tasks.
                // For now, hide menu after action.
                self.hide_context_menu(cx);
            }
            _ => {
                self.hide_context_menu(cx);
            }
        }
    }

    /// Handle toolbar action.
    fn handle_toolbar_action(
        &mut self,
        action: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::info!("Toolbar action: {}", action);
        match action {
            "copy" | "open_image" | "qr_action" | "open_location" | "edit"
            | "edit_note" | "toggle_favorite" | "delete" | "batch_paste"
            | "batch_favorite" | "batch_delete" => {
                // TODO: Wire to AppState services in follow-up migration tasks.
            }
            _ => {}
        }
    }

    /// Method exposed for RootView to call when user clicks outside menu.
    pub fn dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        self.hide_context_menu(cx);
        // Also dismiss hover state
        self.hovered_index = None;
        cx.notify();
    }
```

- [ ] **Step 3: 更新 set_items 方法 — 更新 selected_count**

在 `set_items` 方法的末尾（`cx.notify();` 之前）添加：

```rust
        self.selected_count = self.selected_ids.len();
        self.hovered_index = None;
```

- [ ] **Step 4: 更新 select_index_without_scroll — 更新 selected_count**

在 `select_index_without_scroll` 方法的末尾（`cx.notify();` 之前）添加：

```rust
        self.selected_count = self.selected_ids.len();
```

- [ ] **Step 5: 修改 Render 方法 — 虚拟列表行渲染**

修改 `v_virtual_list` 的回调闭包。在当前代码约第 227-254 行的渲染闭包中，将每行的渲染改为：

```rust
                            move |this, range, _window, _cx| {
                                let selected_count = this.selected_count;
                                range
                                    .filter_map(|i| {
                                        let item = this.items.get(i)?;
                                        let item_id = item.id;
                                        let selected = this.selected_ids.contains(&item_id);
                                        let is_hovered = this.hovered_index == Some(i);
                                        let list_view = list_entity.clone();
                                        let focus_handle = this.focus_handle.clone();
                                        let item_clone = item.clone();

                                        let click_handler: Rc<dyn Fn(usize, &mut Window, &mut App)> =
                                            Rc::new(move |idx, window, cx| {
                                                focus_handle.focus(window);
                                                let _ = list_view.update(cx, move |this, cx| {
                                                    this.select_index_without_scroll(idx, cx);
                                                });
                                            });

                                        let list_for_right = list_entity.clone();

                                        let toolbar_list = list_entity.clone();
                                        let toolbar_handler: Rc<dyn Fn(&str, &mut Window, &mut App)> =
                                            Rc::new(move |action, window, cx| {
                                                let _ = toolbar_list.update(cx, move |this, cx| {
                                                    this.handle_toolbar_action(action, window, cx);
                                                });
                                            });

                                        Some(
                                            div()
                                                .w_full()
                                                .h_full()
                                                .py(px(5.))
                                                .on_mouse_move({
                                                    let list_for_hover = list_entity.clone();
                                                    move |_ev, _window, cx| {
                                                        let _ = list_for_hover.update(cx, |this, cx| {
                                                            if this.hovered_index != Some(i) {
                                                                this.hovered_index = Some(i);
                                                                cx.notify();
                                                            }
                                                        });
                                                    }
                                                })
                                                .on_mouse_down(
                                                    MouseButton::Right,
                                                    {
                                                        let list_ctx = list_for_right.clone();
                                                        move |ev: &MouseDownEvent, _window, cx| {
                                                            let _ = list_ctx.update(cx, |this, cx| {
                                                                if let Some(item) = this.items.get(i) {
                                                                    let is_batch = this.selected_ids.len() > 1
                                                                        && this.selected_ids.contains(&item.id);
                                                                    this.context_menu_visible = true;
                                                                    this.context_menu_x = ev.position.x.0;
                                                                    this.context_menu_y = ev.position.y.0;
                                                                    this.context_menu_item = Some(item.clone());
                                                                    this.context_menu_is_batch = is_batch;
                                                                    cx.notify();
                                                                }
                                                            });
                                                        }
                                                    },
                                                )
                                                .child(
                                                    ClipboardCard::new(
                                                        Rc::new(item_clone),
                                                        selected,
                                                        i,
                                                    )
                                                    .is_hovered(is_hovered)
                                                    .selected_count(selected_count)
                                                    .on_click(click_handler)
                                                    .on_toolbar_action(move |action, window, cx| {
                                                        toolbar_handler(action, window, cx);
                                                    }),
                                                )
                                                .into_any_element(),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            },
```

- [ ] **Step 6: 编译检查**

```bash
cargo build 2>&1
```

预期：由于 `hover_toolbar` 模块尚未注册，此文件编译时会报错。需要在 Task 6 中注册模块后一起编译。

---

### Task 5: 修改 RootView — 渲染 ContextMenu 叠加层

**Files:**
- Modify: `src/ui/root.rs`

- [ ] **Step 1: 添加 use 导入**

在文件顶部添加：

```rust
use crate::ui::context_menu::{ContextMenu, MenuItemContext};
```

- [ ] **Step 2: 在 Render 方法中添加 ContextMenu 渲染**

在 `Render for RootView` 的 `render` 方法末尾，`when(tag_panel_open && is_clipboard, ...)` 闭包之后（即最后一个 `.when(...)` 之后）、`}`之前，添加：

```rust
            .when(
                self.list_view.read(cx).context_menu_visible() && is_clipboard,
                |root| {
                    let list = self.list_view.clone();
                    let list_for_action = self.list_view.clone();
                    let (menu_x, menu_y) = list.read(cx).context_menu_position();
                    let is_batch = list.read(cx).context_menu_is_batch();
                    let item = list.read(cx).context_menu_item().cloned();

                    // Backdrop — click to dismiss
                    root.child(
                        div()
                            .absolute()
                            .size_full()
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                let _ = list.update(cx, |l, cx| l.dismiss_context_menu(cx));
                            }),
                    )
                    .child(
                        div().absolute().occlude().child({
                            if is_batch {
                                let count = list_for_action.read(cx).selected_count;
                                ContextMenu::for_batch(count)
                                    .with_position(menu_x, menu_y, 360.0, 600.0)
                                    .on_action({
                                        let l = list_for_action.clone();
                                        move |action, window, cx| {
                                            let _ = l.update(cx, |list, cx| {
                                                list.handle_menu_action(action, window, cx);
                                            });
                                        }
                                    })
                                    .on_dismiss({
                                        let l = list_for_action.clone();
                                        move |_window, cx| {
                                            let _ = l.update(cx, |list, cx| {
                                                list.hide_context_menu(cx);
                                            });
                                        }
                                    })
                            } else if let Some(ref clip_item) = item {
                                let ctx = MenuItemContext::from_item(clip_item);
                                ContextMenu::for_item(&ctx)
                                    .with_position(menu_x, menu_y, 360.0, 600.0)
                                    .on_action({
                                        let l = list_for_action.clone();
                                        move |action, window, cx| {
                                            let _ = l.update(cx, |list, cx| {
                                                list.handle_menu_action(action, window, cx);
                                            });
                                        }
                                    })
                                    .on_dismiss({
                                        let l = list_for_action.clone();
                                        move |_window, cx| {
                                            let _ = l.update(cx, |list, cx| {
                                                list.hide_context_menu(cx);
                                            });
                                        }
                                    })
                            } else {
                                div()
                            }
                        }),
                    )
                },
            )
```

- [ ] **Step 3: 编译检查**

```bash
cargo build 2>&1
```

预期：由于 `hover_toolbar` 模块尚未注册，编译会报错。需要在 Task 6 完成后统一编译。

---

### Task 6: 注册 hover_toolbar 模块

**Files:**
- Modify: `src/ui/mod.rs`

- [ ] **Step 1: 读取 `src/ui/mod.rs` 以确认当前内容**

确认模块声明位置。

- [ ] **Step 2: 添加 hover_toolbar 模块声明**

在 `src/ui/mod.rs` 中，`pub mod context_menu;` 之后添加：

```rust
pub mod hover_toolbar;
```

- [ ] **Step 3: 编译检查**

```bash
cargo build 2>&1
```

预期：编译通过，无警告。

---

### Task 7: 修复编译问题并验证

**Files:**
- Verify: `src/ui/hover_toolbar.rs`
- Verify: `src/ui/context_menu.rs`
- Verify: `src/ui/clipboard_card.rs`
- Verify: `src/ui/clipboard_list.rs`
- Verify: `src/ui/root.rs`
- Verify: `src/ui/mod.rs`

- [ ] **Step 1: 编译检查**

```bash
cargo build 2>&1
```

修复所有编译错误。常见的可能需要修复的点：
- `ContentType` 的 match — 确保 import 包含所有需要的枚举变体
- 类型推断问题 — 显式标注闭包类型
- `MouseButton` 导入 — 确保 `use gpui::*` 覆盖所有需要的类型

- [ ] **Step 2: 运行 Clippy**

```bash
cargo clippy -- -D warnings 2>&1
```

修复所有警告。

- [ ] **Step 3: 确认所有模块注册正确**

```bash
cargo check 2>&1
```

预期：编译通过，0 错误，0 警告。

---

### Task 8: 最终验证清单

- [ ] `cargo build` 通过
- [ ] `cargo clippy -- -D warnings` 无警告
- [ ] 悬停卡片时工具栏出现在右上角
- [ ] 工具栏按钮根据条目类型正确显示/隐藏
- [ ] 工具栏按钮点击触发正确的 action 字符串
- [ ] 工具栏点击不触发卡片选择
- [ ] 右键菜单在点击位置附近显示
- [ ] 单条条目显示完整菜单（含条件项）
- [ ] 多选条目右键显示批量菜单
- [ ] 菜单项 hover 高亮效果
- [ ] 点击菜单外部关闭菜单
- [ ] 菜单项点击后关闭菜单

---

### Task 9: Commit

```bash
git add src/ui/hover_toolbar.rs src/ui/context_menu.rs src/ui/clipboard_card.rs src/ui/clipboard_list.rs src/ui/root.rs src/ui/mod.rs
git commit -m "feat: migrate hover toolbar and context menu from Slint to GPUI

- Add HoverToolbar component with conditional iconfont buttons
- Rewrite ContextMenu with dynamic menu items and position clamping
- Add hover tracking to ClipboardList via on_mouse_move
- Embed HoverToolbar in ClipboardCard on hover
- Render ContextMenu as RootView overlay with dismiss backdrop
- Support single-item and batch mode for both toolbar and menu
- UI layer ready; backend action routing as TODO follow-ups

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```
