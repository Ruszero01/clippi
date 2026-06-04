# GPUI 备注内联编辑 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标：** 将 Slint 版本的备注内联编辑功能迁移到 GPUI，支持悬停工具栏/右键菜单触发、单行内联编辑器、Enter/按钮/失焦提交。

**架构：** 编辑状态（`editing_note_id`、共享 `Entity<InputState>`）放在 `ClipboardListView` 中。当触发 `"edit_note"` 时设置编辑状态，虚拟列表重渲染对应卡片为内联编辑器。提交时调用 `AppState::update_note()` 写入 DB 并更新内存数据。

**技术栈：** Rust + GPUI 0.2.2 + gpui_component 0.5.1 (Input/InputState)

**设计文档：** `docs/superpowers/specs/2026-06-04-gpui-note-editing-design.md`

---

### Task 1: Database — update_note 同步更新 updated_at

**文件：**
- 修改: `src/core/db.rs:206-212`

- [ ] **Step 1: 修改 update_note 方法，添加 updated_at 字段更新**

将 `src/core/db.rs` 第 206-212 行替换为：

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

- [ ] **Step 2: 编译验证**

```bash
cargo build 2>&1 | head -5
```

预期：编译通过，无错误。

- [ ] **Step 3: 提交**

```bash
git add src/core/db.rs
git commit -m "fix: update_note now also updates updated_at for consistency"
```

---

### Task 2: AppState — 新增 update_note 方法

**文件：**
- 修改: `src/state/app.rs`（在 `paste_as_hex` 方法之后，最后的 `}` 之前）

- [ ] **Step 1: 添加 update_note 方法**

在 `src/state/app.rs` 的 `paste_as_hex` 方法之后、`}` （ impl 闭包）之前插入：

```rust
/// Update the note field for a clipboard item.
/// Writes to DB (includes updated_at) and syncs the in-memory items list.
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

- [ ] **Step 2: 编译验证**

```bash
cargo build 2>&1 | head -5
```

预期：编译通过。

- [ ] **Step 3: 提交**

```bash
git add src/state/app.rs
git commit -m "feat: add AppState::update_note method for note editing"
```

---

### Task 3: ClipboardListView — 添加编辑状态字段 + 修改 new() 签名

**文件：**
- 修改: `src/ui/clipboard_list.rs:6-17` (imports)
- 修改: `src/ui/clipboard_list.rs:19-42` (struct fields)
- 修改: `src/ui/clipboard_list.rs:43-63` (constructor)

- [ ] **Step 1: 添加 import**

在 `src/ui/clipboard_list.rs` 第 12 行（`use gpui_component::VirtualListScrollHandle;`）之后添加：

```rust
use gpui_component::input::{Input, InputEvent, InputState};
```

- [ ] **Step 2: 添加结构体字段**

在 struct 的 `selected_count` 字段之后（第 40 行 `}` 之前）添加：

```rust
    // ── Note editing state ──
    /// Which item is currently in note-edit mode (-1 = none).
    editing_note_id: i64,
    /// Shared InputState entity for the inline note editor.
    /// Created once at init, value is updated when editing starts.
    note_input: Entity<InputState>,
```

- [ ] **Step 3: 修改 new() 签名和实现**

将第 43 行的方法签名从：

```rust
pub fn new(items: Vec<ClipboardItem>, state: Entity<AppState>, cx: &mut App) -> Self {
```

改为：

```rust
pub fn new(items: Vec<ClipboardItem>, state: Entity<AppState>, window: &mut Window, cx: &mut App) -> Self {
```

在构造函数末尾、`selected_count: 0,` 之后（第 61 行）添加：

```rust
            editing_note_id: -1,
            note_input: cx.new(|cx| InputState::new(window, cx).placeholder("Add a note...")),
```

注意在 `selected_count: 0,` 后面加逗号。

- [ ] **Step 4: 编译验证**

```bash
cargo build 2>&1 | head -20
```

预期：RootView 处有编译错误（`new()` 调用缺少 `window` 参数），这是预期的 — 将在 Task 10 修复。

- [ ] **Step 5: 提交**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat(clipboard_list): add note editing state fields and InputState entity"
```

---

### Task 4: ClipboardListView — 添加 start_note_edit 和 commit_note_edit

**文件：**
- 修改: `src/ui/clipboard_list.rs`（在 dismiss_context_menu 方法之后，handle_menu_action 之前）

- [ ] **Step 1: 添加两个编辑方法**

在 `dismiss_context_menu` 方法之后、`hide_context_menu` 方法之前（约第 221 行之后）插入：

```rust
    /// Start editing the note for an item.
    /// `initial_text` — from item.note (hover toolbar) or "" (context menu).
    fn start_note_edit(&mut self, id: i64, initial_text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.editing_note_id = id;
        self.note_input.update(cx, |input, cx| {
            input.set_value(initial_text, window, cx);
        });
        cx.notify();
    }

    /// Commit the current note edit to DB and exit edit mode.
    fn commit_note_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing_note_id > 0 {
            let id = self.editing_note_id;
            let text = self.note_input.read(cx).value().to_string();
            if !text.is_empty() {
                self.state.update(cx, |state, _cx| {
                    state.update_note(id, &text);
                });
            }
        }
        self.editing_note_id = -1;
        cx.notify();
    }
```

- [ ] **Step 2: 编译验证**

```bash
cargo build 2>&1 | head -5
```

预期：方法本身编译通过（尚未被调用）。

- [ ] **Step 3: 提交**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat(clipboard_list): add start_note_edit and commit_note_edit methods"
```

---

### Task 5: ClipboardListView — 接入 action 处理

**文件：**
- 修改: `src/ui/clipboard_list.rs:267-269` (handle_menu_action)
- 修改: `src/ui/clipboard_list.rs:298-300` (handle_toolbar_action)

- [ ] **Step 1: 修改 handle_menu_action**

将第 267-269 行：

```rust
            "edit" | "edit_note" | "toggle_favorite" | "delete"
            | "open_image" | "paste_ocr" | "qr_detect" | "show_tag_picker"
            | "batch_favorite" | "batch_delete" => {}
```

替换为：

```rust
            "edit_note" => {
                if let Some(ref item) = self.context_menu_item {
                    // Context menu: start with empty (no note context)
                    self.start_note_edit(item.id, "", _window, cx);
                }
            }
            "edit" | "toggle_favorite" | "delete"
            | "open_image" | "paste_ocr" | "qr_detect" | "show_tag_picker"
            | "batch_favorite" | "batch_delete" => {}
```

注意：原有的 `self.hide_context_menu(cx);` 在 match 之后统一调用，关闭菜单并清理状态。`start_note_edit` 设置的编辑状态不受影响。

- [ ] **Step 2: 修改 handle_toolbar_action**

将第 298-300 行：

```rust
            "open_image" | "qr_action" | "open_location" | "edit"
            | "edit_note" | "toggle_favorite" | "delete"
            | "batch_favorite" | "batch_delete" => {}
```

替换为：

```rust
            "edit_note" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        // Hover toolbar: pre-fill existing note
                        self.start_note_edit(item.id, &item.note, _window, cx);
                    }
                }
            }
            "open_image" | "qr_action" | "open_location" | "edit"
            | "toggle_favorite" | "delete"
            | "batch_favorite" | "batch_delete" => {}
```

- [ ] **Step 3: 编译验证**

```bash
cargo build 2>&1 | head -10
```

预期：编译通过。`edit_note` 不再是 no-op。

- [ ] **Step 4: 提交**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat(clipboard_list): wire up edit_note action in toolbar and context menu"
```

---

### Task 6: ClipboardListView — 鼠标移出自动提交

**文件：**
- 修改: `src/ui/clipboard_list.rs:383-393` (列表 on_mouse_move)
- 修改: `src/ui/clipboard_list.rs:481-493` (卡片行 on_mouse_move)

- [ ] **Step 1: 修改列表背景 on_mouse_move — 鼠标移出所有卡片时提交**

将第 383-393 行的 `on_mouse_move` 闭包改为：

```rust
            .on_mouse_move({
                let list_for_clear = list_entity.clone();
                move |_ev, _window, cx| {
                    let _ = list_for_clear.update(cx, |this, cx| {
                        // If editing, commit before clearing hover
                        if this.editing_note_id > 0 {
                            this.commit_note_edit(cx);
                        }
                        if this.hovered_index.is_some() {
                            this.hovered_index = None;
                            cx.notify();
                        }
                    });
                }
            })
```

- [ ] **Step 2: 修改卡片行 on_mouse_move — 切换到其他卡片时先提交**

将第 481-493 行的卡片行 `on_mouse_move` 中的逻辑改为：

```rust
                                                .on_mouse_move({
                                                    move |_ev, _window, cx| {
                                                        cx.stop_propagation();
                                                        let _ = list_for_hover.update(
                                                            cx,
                                                            |this, cx| {
                                                                // If editing a different card, commit before changing hover
                                                                let target_id = this
                                                                    .items
                                                                    .get(i)
                                                                    .map(|it| it.id)
                                                                    .unwrap_or(-1);
                                                                if this.editing_note_id > 0
                                                                    && this.editing_note_id != target_id
                                                                {
                                                                    this.commit_note_edit(cx);
                                                                }
                                                                if this.hovered_index != Some(i) {
                                                                    this.hovered_index = Some(i);
                                                                    cx.notify();
                                                                }
                                                            },
                                                        );
                                                    }
                                                })
```

- [ ] **Step 3: 编译验证**

```bash
cargo build 2>&1 | head -10
```

预期：编译通过。

- [ ] **Step 4: 提交**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat(clipboard_list): auto-commit note edit on mouse leave"
```

---

### Task 7: ClipboardCard — 添加 editing 相关字段和 builder

**文件：**
- 修改: `src/ui/clipboard_card.rs:503-578`（struct 定义 + builder）

- [ ] **Step 1: 在 struct 中添加字段**

在 `ClipboardCard` struct 的 `on_double_click` 字段之后（第 514 行，`}` 之前）添加：

```rust
    /// Whether this card is in note-editing mode (shows inline editor).
    editing: bool,
    /// Shared InputState from ClipboardListView (only Some when editing is true).
    note_input: Option<Entity<InputState>>,
    /// Called when note editing is committed (Enter / confirm button).
    on_commit_note: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
```

`InputState` 类型已经在 `clipboard_card.rs` 中可用吗？需要检查是否有 import...

实际上 `clipboard_card.rs` 目前没有引入 `gpui_component::input`。需要加 import，但因为 `note_input` 是 `Option<Entity<InputState>>`，需要 `InputState` 类型在作用域内。

在 `clipboard_card.rs` 第 12 行（`use gpui::*;` 之后）添加 import：

```rust
use gpui_component::input::InputState;
```

等等，但 `Entity` 是 `gpui::Entity`。`Entity<InputState>` 需要 `InputState` 可见。或者用完全限定路径？

实际上在结构体定义中使用 `Entity<gpui_component::input::InputState>` 就不需要额外的 import。但为了可读性，添加 import 更好。

但在第 22 行已有：
```rust
use super::hover_toolbar::{HoverToolbar, HoverToolbarProps};
```

添加一行：
```rust
use gpui_component::input::{Input, InputState};
```

- [ ] **Step 2: 在构造函数中添加默认值**

在 `ClipboardCard::new` 返回的 `Self` 结构体字面量末尾（`on_double_click: None,` 之后）添加：

```rust
            editing: false,
            note_input: None,
            on_commit_note: None,
```

- [ ] **Step 3: 添加 builder 方法**

在 `on_double_click` builder 之后（第 577 行之后、`}` 之前）添加：

```rust
    /// Set whether this card is in note-editing mode.
    pub fn editing(mut self, editing: bool) -> Self {
        self.editing = editing;
        self
    }

    /// Set the shared InputState for inline note editing.
    pub fn note_input(mut self, input: Entity<InputState>) -> Self {
        self.note_input = Some(input);
        self
    }

    /// Called when note is committed (Enter / confirm button).
    pub fn on_commit_note(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_commit_note = Some(Rc::new(handler));
        self
    }
```

- [ ] **Step 4: 编译验证**

```bash
cargo build 2>&1 | head -10
```

预期：编译通过（新字段和方法尚未被使用）。

- [ ] **Step 5: 提交**

```bash
git add src/ui/clipboard_card.rs
git commit -m "feat(clipboard_card): add editing props for inline note editor"
```

---

### Task 8: ClipboardCard — 渲染内联编辑器

**文件：**
- 修改: `src/ui/clipboard_card.rs:793-801`（content 变量 — note 显示逻辑）

- [ ] **Step 1: 修改内容区渲染逻辑**

当前内容区渲染（第 793 行开始）：

```rust
        let content = if !note.is_empty() {
            div().flex_1().flex().items_center().child(
                div()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(text_2)
                    .overflow_hidden()
                    .child(note),
            )
        } else {
```

替换为：

```rust
        let content = if editing {
            // ── Inline note editor ──
            let commit = on_commit_note.clone();
            let note_input_ref = note_input.clone();

            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    // Single-line text input
                    div()
                        .w_full()
                        .h(px(24.))
                        .child(
                            Input::new(&note_input_ref.expect("note_input must be set when editing"))
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .w_full()
                                .h_full()
                                .text_size(px(12.)),
                        ),
                )
                .child(
                    // Confirm button (checkmark icon)
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
                                    let commit = commit.clone();
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
                .on_key_down({
                    let commit = commit.clone();
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
            div().flex_1().flex().items_center().child(
                div()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(text_2)
                    .overflow_hidden()
                    .child(note),
            )
        } else {
```

注意需要在 `KeyDownEvent` 类型可用 — `gpui::*` 已包含。

- [ ] **Step 2: 在结构体解构中添加新字段**

在 `render` 方法的 `let Self {` 解构中添加（第 582-593 行）：

```rust
            editing,
            note_input,
            on_commit_note,
```

- [ ] **Step 3: 编译验证**

```bash
cargo build 2>&1 | head -20
```

预期：可能有未使用变量的 warning，或缺少 `KeyDownEvent` import。修复所有 issue。

- [ ] **Step 4: 提交**

```bash
git add src/ui/clipboard_card.rs
git commit -m "feat(clipboard_card): render inline note editor when editing=true"
```

---

### Task 9: ClipboardListView — 在虚拟列表中传递编辑 props

**文件：**
- 修改: `src/ui/clipboard_list.rs:429-625`（虚拟列表渲染闭包）

- [ ] **Step 1: 在虚拟列表闭包中解构编辑状态**

在闭包开头（第 430 行 `let selected_count = this.selected_count;` 之后）添加：

```rust
                                let editing_note_id = this.editing_note_id;
                                let note_input = this.note_input.clone();
                                let list_for_note_commit = list_entity.clone();
```

- [ ] **Step 2: 添加 editing 判断和 props**

在第 556-565 行的 `ClipboardCard::new(...)` 调用中添加 editing 相关 props：

当前代码：
```rust
                                                    ClipboardCard::new(
                                                        Rc::new(item_clone),
                                                        selected,
                                                        i,
                                                    )
                                                    .hovered(is_hovered)
                                                    .selected_count(selected_count)
                                                    .selection_order(selection_order)
                                                    .on_click(click_handler)
                                                    .on_toolbar_action(
```

改为：

```rust
                                                    ClipboardCard::new(
                                                        Rc::new(item_clone),
                                                        selected,
                                                        i,
                                                    )
                                                    .hovered(is_hovered)
                                                    .selected_count(selected_count)
                                                    .selection_order(selection_order)
                                                    .editing(editing_note_id == item_id)
                                                    .when(editing_note_id == item_id, |card| {
                                                        card.note_input(note_input.clone())
                                                    })
                                                    .on_commit_note({
                                                        let list_for_commit = list_for_note_commit.clone();
                                                        move |_window, cx| {
                                                            let _ = list_for_commit.update(cx, |this, cx| {
                                                                this.commit_note_edit(cx);
                                                            });
                                                        }
                                                    })
                                                    .on_click(click_handler)
                                                    .on_toolbar_action(
```

- [ ] **Step 3: 编译验证**

```bash
cargo build 2>&1 | head -30
```

预期：`.when()` 可能不可用（取决于 GPUI API 版本）。如果 `.when()` 不可用于 `ClipboardCard`（它不是 `InteractiveElement`），改为条件调用：

```rust
                                                    let card = ClipboardCard::new(
                                                        Rc::new(item_clone),
                                                        selected,
                                                        i,
                                                    )
                                                    .hovered(is_hovered)
                                                    .selected_count(selected_count)
                                                    .selection_order(selection_order)
                                                    .editing(editing_note_id == item_id)
                                                    .on_commit_note({
                                                        let list_for_commit = list_for_note_commit.clone();
                                                        move |_window, cx| {
                                                            let _ = list_for_commit.update(cx, |this, cx| {
                                                                this.commit_note_edit(cx);
                                                            });
                                                        }
                                                    })
                                                    .on_click(click_handler)
                                                    .on_toolbar_action(
                                                        move |action, window, cx| {
                                                            let _ = list_for_toolbar.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.handle_toolbar_action(
                                                                        action, window, cx,
                                                                    );
                                                                },
                                                            );
                                                        },
                                                    );

                                                    let card = if editing_note_id == item_id {
                                                        card.note_input(note_input.clone())
                                                    } else {
                                                        card
                                                    };
```

并在 `ClipboardCard` 的 builder chain 末尾使用 `card`。

- [ ] **Step 4: 编译验证**

```bash
cargo build 2>&1 | head -10
```

预期：编译通过。

- [ ] **Step 5: 提交**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat(clipboard_list): pass editing props to ClipboardCard in virtual list"
```

---

### Task 10: RootView — 适配 ClipboardListView::new 调用

**文件：**
- 修改: `src/ui/root.rs:49`

- [ ] **Step 1: 修改 list_view 创建调用**

将第 49 行：

```rust
        let list_view = cx.new(|cx| ClipboardListView::new(items, state.clone(), cx));
```

改为：

```rust
        let list_view = cx.new(|cx| ClipboardListView::new(items, state.clone(), window, cx));
```

- [ ] **Step 2: 编译验证**

```bash
cargo build 2>&1 | head -10
```

预期：编译通过。

- [ ] **Step 3: 提交**

```bash
git add src/ui/root.rs
git commit -m "fix(root): pass window to ClipboardListView::new for InputState creation"
```

---

### Task 11: 全量编译验证

- [ ] **Step 1: 完整构建**

```bash
cargo build 2>&1
```

预期：0 errors, 0 warnings（或只有已有的 unrelated warnings）。

- [ ] **Step 2: 检查 clippy**

```bash
cargo clippy 2>&1
```

预期：无新增 clippy warnings。

- [ ] **Step 3: 最终提交（如有未提交改动）**

```bash
git status
git add -A
git commit -m "feat: complete GPUI note inline editing migration"
```

---

## 实现注意事项

1. **`start_note_edit` 的 `window` 参数**：从 action handler 传入（`handle_toolbar_action` 和 `handle_menu_action` 已有 `_window: &mut Window`）。

2. **`commit_note_edit` 不需要 window**：因为 `InputState::value()` 只需要 `&App`（`Context<Self>` 自动 deref）。

3. **编辑时卡片高度**：`estimate_card_height` 已有 `if !item.note.is_empty() { return 68.0; }` 的逻辑，编辑模式下编辑器高度也是 68px 级别，无需额外处理。

4. **现有备注显示逻辑保持不变**：当 `editing == false` 且有备注时，仍然显示备注文本（第 793-801 行的原有逻辑）。

5. **行为对齐 Slint**：
   - 悬停工具栏触发 → 预填现有备注 ✅
   - 右键菜单触发 → 清空备注 ✅
   - Enter 键提交 ✅
   - 确认按钮提交 ✅
   - 鼠标移出提交 ✅
