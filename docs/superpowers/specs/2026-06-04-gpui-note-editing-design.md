# GPUI 备注内联编辑 — 设计文档

> **目标:** 将悬浮工具栏和右键菜单的备注编辑功能从 Slint 迁移到 GPUI，功能对齐 Slint 版本，使用 GPUI 最佳实践实现内联编辑器。

**日期:** 2026-06-04
**分支:** experiment/gpui-migration
**参考:** Slint 版 `ui/ClipboardList.slint`（内联备注编辑 UI）、`ui/ContextMenu.slint`（备注菜单项）、`ui/app.slint`（editing-note-id/text 状态属性）

---

## 架构概览

### 事件流

```
触发 "edit_note"
  ├── 悬停工具栏备注按钮 → handle_toolbar_action("edit_note")
  │     └── 预填 item.note（现有备注文本）
  └── 右键菜单 "Note" 项 → handle_menu_action("edit_note")
        └── 清空（菜单无备注文本上下文，与 Slint 一致）
              ↓
ClipboardListView 设置编辑状态:
  editing_note_id = item.id
  editing_note_text = item.note / ""
  更新 InputState 的值
              ↓
虚拟列表重渲染:
  item.id == editing_note_id → 渲染内联编辑器（替换卡片内容区）
  其他卡片正常渲染
              ↓
用户提交 (Enter / 确认按钮✓ / 鼠标移到其他卡片):
  → AppState.update_note(id, text)
  → DB 写入 (note + updated_at)
  → editing_note_id = -1（退出编辑）
  → cx.notify() 触发重渲染
```

### 状态归属

编辑状态（`editing_note_id`、`editing_note_text`、`note_input: Entity<InputState>`）放在 `ClipboardListView` 中。

**理由：** `ClipboardListView` 已是卡片交互的中心 — 管理 `hovered_index`、`context_menu_*`、`selected_ids`、`selected_count` 等 UI 交互状态。备注编辑是卡片级 UI 交互，放在此处内聚性最好。

---

## 组件设计

### 1. Database::update_note — 行为修正 (`src/core/db.rs`)

**现状：** `update_note()` 不更新 `updated_at`，与 `update_content()`、`toggle_favorite()` 行为不一致。

**改动：** 添加 `updated_at` 更新，行为统一。

```rust
pub fn update_note(&self, id: i64, note: &str) -> SqlResult<()> {
    let now = chrono::Utc::now().to_rfc3339();
    self.conn.execute(
        "UPDATE clipboard_items SET note = ?1, updated_at = ?2 WHERE id = ?3",
        params![note, now, id],
    )?;
    Ok(())
}
```

### 2. AppState::update_note — 新增 (`src/state/app.rs`)

新增方法，调用 DB 层并同步更新内存中的 `items` 数据（避免全量 reload）：

```rust
/// Update note text for a clipboard item.
/// Updates both the database and the in-memory items list.
pub fn update_note(&mut self, id: i64, note: &str) {
    match self.db.update_note(id, note) {
        Ok(_) => {
            if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
                item.note = note.to_string();
                item.updated_at = chrono::Utc::now().to_rfc3339();
            }
        }
        Err(e) => log::error!("update_note({id}): {e}"),
    }
}
```

**要点：**
- 直接修改 `items` 中对应 item 的 `note` 和 `updated_at` 字段
- 不需要 `mark_dirty()` — GPUI 通过 `cx.notify()` 驱动重渲染
- 不需要 `reload_items()` — 增量更新避免不必要的 DB 查询

### 3. ClipboardListView — 编辑状态管理 (`src/ui/clipboard_list.rs`)

#### 3.1 新增字段

```rust
pub struct ClipboardListView {
    // ... 现有字段保持不变 ...

    // ── Note editing state ──
    /// Which item is currently in note-edit mode (-1 = none).
    editing_note_id: i64,
    /// The editing note text buffer (mirrors InputState value).
    editing_note_text: String,
    /// Shared InputState entity for the inline note editor.
    /// Created once at initialization, reused across edits.
    note_input: Entity<InputState>,
}
```

#### 3.2 构造函数签名变更

添加 `window: &mut Window` 参数以创建 `InputState` Entity：

```rust
pub fn new(
    items: Vec<ClipboardItem>,
    state: Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) -> Self {
    // ...
    Self {
        // ... 现有字段 ...
        editing_note_id: -1,
        editing_note_text: String::new(),
        note_input: cx.new(|cx| InputState::new(window, cx).placeholder("Add a note...")),
    }
}
```

#### 3.3 新增方法

```rust
/// Start editing the note for an item.
/// - `initial_text`: from item.note (hover toolbar) or "" (context menu)
fn start_note_edit(&mut self, id: i64, initial_text: &str, cx: &mut Context<Self>) {
    self.editing_note_id = id;
    self.editing_note_text = initial_text.to_string();
    self.note_input.update(cx, |input, cx| {
        input.set_value(initial_text, cx);
    });
    cx.notify();
}

/// Commit current note to DB and exit edit mode.
fn commit_note_edit(&mut self, cx: &mut Context<Self>) {
    if self.editing_note_id > 0 {
        let id = self.editing_note_id;
        let text = self.editing_note_text.clone();
        self.state.update(cx, |state, _cx| {
            state.update_note(id, &text);
        });
    }
    self.editing_note_id = -1;
    self.editing_note_text.clear();
    cx.notify();
}
```

#### 3.4 修改 action 处理

`handle_menu_action` 中：

```rust
"edit_note" => {
    if let Some(ref item) = self.context_menu_item {
        // Context menu: start with empty (no note context available)
        self.start_note_edit(item.id, "", cx);
    }
}
```

`handle_toolbar_action` 中：

```rust
"edit_note" => {
    if let Some(index) = self.hovered_index {
        if let Some(item) = self.items.get(index) {
            // Hover toolbar: pre-fill existing note
            self.start_note_edit(item.id, &item.note, cx);
        }
    }
}
```

注意：这两个 action handler **不再**调用 `hide_context_menu(cx)` 作为 catch-all — 对于 `"edit_note"`，菜单正常关闭但编辑状态保持不变。

#### 3.5 提交触发点

在现有事件处理中增加编辑提交检测：

| 触发条件 | 实现位置 | 行为 |
|---------|---------|------|
| 确认按钮点击 | ClipboardCard 按钮回调 | `commit_note_edit` |
| Enter 键 | ClipboardCard `on_key_down` | `commit_note_edit` |
| 鼠标移到其他卡片 | ClipboardListView `on_mouse_move` | 先 `commit_note_edit` 再更新 hover |
| 点击列表空白区 | ClipboardListView `on_mouse_down` | `commit_note_edit` |

#### 3.6 虚拟列表渲染改动

在卡片渲染闭包中传入编辑状态：

```rust
let editing = this.editing_note_id == item_id;
// ...
ClipboardCard::new(Rc::new(item_clone), selected, i)
    .editing(editing)
    .note_input(if editing { Some(this.note_input.clone()) } else { None })
    .on_commit_note(Rc::new({ ... commit callback ... }))
    // ... existing props ...
```

### 4. ClipboardCard — 内联编辑器 (`src/ui/clipboard_card.rs`)

#### 4.1 新增字段

```rust
pub struct ClipboardCard {
    // ... 现有字段 ...

    // ── Note editing mode ──
    editing: bool,
    note_input: Option<Entity<InputState>>,
    on_commit_note: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}
```

#### 4.2 新增 Builder

```rust
pub fn editing(mut self, editing: bool) -> Self { ... }
pub fn note_input(mut self, input: Option<Entity<InputState>>) -> Self { ... }
pub fn on_commit_note(mut self, handler: Rc<dyn Fn(&mut Window, &mut App)>) -> Self { ... }
```

#### 4.3 渲染逻辑

当 `editing == true` 时，**内容区**（content 变量）替换为内联编辑器：

```rust
let content = if editing {
    // ── Inline note editor ──
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            // Single-line text input using shared InputState
            div()
                .w_full()
                .h(px(24.))
                .child(
                    Input::new(&note_input.unwrap())
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .w_full()
                        .h_full()
                        .text_size(px(12.)),
                ),
        )
        .child(
            // Confirm button (✓ icon)
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    div()
                        .w(px(20.))
                        .h(px(20.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.bg(rgba(0xffffff10)))
                        .on_mouse_down(MouseButton::Left, {
                            let commit = on_commit_note.clone();
                            move |_ev, window, cx| {
                                cx.stop_propagation();
                                if let Some(ref handler) = commit {
                                    handler(window, cx);
                                }
                            }
                        })
                        .child(
                            div()
                                .font_family("iconfont")
                                .text_size(px(12.))
                                .text_color(accent)
                                .child("\u{e611}"), // ✓ checkmark
                        ),
                ),
        )
        // Keyboard: Enter to commit
        .on_key_down({
            let commit = on_commit_note.clone();
            move |ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key.as_str() == "enter" {
                    if let Some(ref handler) = commit {
                        handler(window, cx);
                    }
                }
            }
        })
} else if !note.is_empty() {
    // Existing note display (unchanged)
    div().flex_1().flex().items_center().child(...)
} else {
    // Existing content rendering (unchanged)
    match content_type { ... }
};
```

#### 4.4 外观规格

- **输入框：** 全宽，24px 高度，12px 字号，继承卡片内容区背景
- **确认按钮：** 20×20px，右对齐，4px 圆角，hover 显示半透明白色背景
- **间距：** 输入框与确认按钮之间 2px 间距
- **与 Slint 版本对齐：** 单行输入、确认按钮(✓)、Enter 提交

### 5. RootView — 调用方适配 (`src/ui/root.rs`)

`ClipboardListView::new` 签名增加了 `window` 参数，调用处需同步修改：

```rust
// Before:
let list_view = cx.new(|cx| ClipboardListView::new(items, state.clone(), cx));

// After:
let list_view = cx.new(|cx| ClipboardListView::new(items, state.clone(), window, cx));
```

---

## 交互细节对齐

| Slint 行为 | GPUI 实现 |
|-----------|----------|
| 悬停工具栏点击备注 → 预填现有备注 | `start_note_edit(id, &item.note, cx)` |
| 右键菜单点击备注 → 清空备注 | `start_note_edit(id, "", cx)` |
| Enter 键提交 | `on_key_down` 捕获 `"enter"` → `commit_note_edit` |
| 确认按钮(✓)提交 | 按钮 `on_mouse_down` → `commit_note_edit` |
| 点击卡片外部提交 | 鼠标移出/点击其他区域 → 检测 `editing_note_id > 0` → `commit_note_edit` |
| 有备注时卡片高度 68px | 已有实现（`estimate_card_height` 第 452 行） |
| 有备注时显示备注文本（非 hover 模式） | 已有实现（ClipboardCard 第 793-801 行） |

---

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/core/db.rs` | **修改** | `update_note()` 添加 `updated_at` 更新 |
| `src/state/app.rs` | **修改** | 新增 `update_note()` 方法 |
| `src/ui/clipboard_list.rs` | **修改** | 新增编辑状态、InputState、action 处理、渲染改动 |
| `src/ui/clipboard_card.rs` | **修改** | 新增 editing 模式、内联编辑器、回调解绑 |
| `src/ui/root.rs` | **修改** | `new()` 调用适配（增加 window 参数） |

## 不复用的代码

- 不移除 `src/ui/edit_panel.rs` — 它是独立的全文编辑面板，与备注内联编辑是不同的功能
- 不修改 `src/ui/hover_toolbar.rs` — 备注按钮已存在，action 名称不变
- 不修改 `src/ui/context_menu.rs` — 备注菜单项已存在，action 名称不变
