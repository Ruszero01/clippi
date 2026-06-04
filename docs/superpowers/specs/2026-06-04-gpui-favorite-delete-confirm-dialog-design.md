# GPUI 收藏/删除功能 + 通用确认弹窗 — 设计文档

> **目标:** 将悬浮工具栏和右键菜单的收藏和删除功能（含批量）从 Slint 迁移到 GPUI，并新增删除确认弹窗（通用可复用组件）。

**日期:** 2026-06-04
**分支:** experiment/gpui-migration
**参考:** Slint 版 `ui/ClipboardList.slint`（工具栏/批量栏）、`ui/ContextMenu.slint`（菜单项）、`src/services/clipboard.rs`（ClipboardService 业务逻辑）

---

## 架构概览

### 事件流 — 收藏（无需确认）

```
用户点击收藏按钮 (工具栏或右键菜单)
  → handle_toolbar_action / handle_menu_action: "toggle_favorite" / "batch_favorite"
  → AppState.toggle_favorite(id) 或 batch_toggle_favorite()
    ├── DB toggle_favorite (SQL CASE 翻转 0↔1)
    ├── 墓碑管理:
    │     is_favorite: 1→0 → db.record_unfavorite(hash, now, device)
    │     is_favorite: 0→1 → db.remove_unfavorite(hash)
    ├── 内存 items 增量更新 (不重新加载全量)
    ├── 如果 favorites 过滤激活 → 全量 reload_items + clear_selection
    └── sync_dirty = true
  → cx.notify() 触发重渲染
```

### 事件流 — 删除（需确认）

```
用户点击删除按钮 (工具栏或右键菜单)
  → handle_toolbar_action / handle_menu_action: "delete" / "batch_delete"
  → 设置 confirm_dialog 状态 (不直接执行)
  → RootView 渲染 ConfirmDialog 叠加层:
      ├── 全屏半透明遮罩 rgba(0x00000066)
      └── 居中模态框 (280px 宽, surface 背景, 12px 圆角)
           ├── 标题 + 正文
           ├── [取消] 按钮 → dismiss_confirm_dialog
           └── [删除] 按钮 (红色) → 执行删除:
                ├── AppState.delete_item(id) 或 batch_delete()
                │     ├── DB delete_item (DELETE FROM)
                │     ├── DB record_item_deletion(hash, now, device)
                │     ├── 内存 items 移除已删除项
                │     ├── selected_ids 清理
                │     └── sync_dirty = true
                ├── dismiss_confirm_dialog
                └── cx.notify()
```

### 状态归属

- 业务状态（`sync_dirty`、items、selected_ids）→ `AppState`
- 确认弹窗 UI 状态（`confirm_dialog: Option<ConfirmDialogState>`）→ `ClipboardListView`
- 确认弹窗渲染 → `RootView`（因需要全窗口遮罩，跟随 ContextMenu 模式）

---

## 组件设计

### 1. ConfirmDialog — 通用确认弹窗 (`src/ui/confirm_dialog.rs`)

#### 1.1 设计原则

- **RenderOnce 组件**（与 Toast/ContextMenu 一致）
- **Builder API** 支持链式配置
- **预设工厂方法** 覆盖常见场景（单条删除、批量删除、未来黑名单删除等）
- **全窗口遮罩 + 居中模态框**，点击遮罩取消

#### 1.2 结构体

```rust
#[derive(IntoElement)]
pub struct ConfirmDialog {
    title: String,           // 弹窗标题 (14px bold)
    message: String,         // 正文内容 (12px)
    confirm_label: String,   // 确认按钮文字，默认 "Confirm"
    cancel_label: String,    // 取消按钮文字，默认 "Cancel"
    danger: bool,            // true = 确认按钮使用 danger 颜色 (#ff5f57)
    on_confirm: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    on_cancel: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}
```

#### 1.3 Builder 方法

```rust
pub fn new() -> Self
pub fn title(mut self, title: impl Into<String>) -> Self
pub fn message(mut self, message: impl Into<String>) -> Self
pub fn confirm_label(mut self, label: impl Into<String>) -> Self
pub fn cancel_label(mut self, label: impl Into<String>) -> Self
pub fn danger(mut self, danger: bool) -> Self
pub fn on_confirm(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self
pub fn on_cancel(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self
```

#### 1.4 预设工厂方法

```rust
/// 删除单条确认
/// - preview: 卡片内容预览文本（截断到 ~30 字符）
pub fn delete_single(preview: &str) -> Self

/// 批量删除确认
/// - count: 选中项数量
pub fn delete_batch(count: usize) -> Self

/// [预留] 快捷键黑名单 — 移除确认
/// 后续 /hotkey-blacklist 任务中可直接调用 ConfirmDialog::remove_blacklist("AppName")
pub fn remove_blacklist(app_name: &str) -> Self
```

#### 1.5 渲染规格

| 层级 | 元素 | 规格 |
|------|------|------|
| 外层 | 全屏遮罩 | `absolute()` + `size_full()`，`bg(rgba(0x00000066))` |
| 遮罩交互 | 点击关闭 | `on_mouse_down(Left)` → `on_cancel` |
| 模态框容器 | 居中 | `flex()` + `items_center()` + `justify_center()`，`occlude()` 防止穿透 |
| 模态框 | 卡片 | 280px 宽，`surface` (#2c2d2e)，12px 圆角，1px 边框 `rgba(0xffffff14)`，16px padding |
| 标题 | 文字 | 14px bold，`text_1` (#eaebec) |
| 正文 | 文字 | 12px，`text_2` (#919496)，上间距 8px |
| 按钮区 | 布局 | flex row，`justify_end()`，gap 8px，上间距 16px |
| 取消按钮 | 按钮 | 24px 高，padding 12px，`text_2` 颜色，hover 变亮，4px 圆角 |
| 确认按钮 | 按钮 | 24px 高，padding 12px，`danger=true` 时 `danger` (#ff5f57)，否则 `accent` (#7ecba3)，4px 圆角 |

#### 1.6 主题变量

使用与 ClippiTheme 一致的硬编码颜色（与 ContextMenu/Toast 对齐）：
- `surface`: `rgb(0x2c2d2e)`
- `text_1`: `rgb(0xeaebec)`
- `text_2`: `rgb(0x919496)`
- `danger`: `rgb(0xff5f57)`
- `accent`: `rgb(0x7ecba3)`
- `border`: `rgba(0xffffff14)`
- `overlay`: `rgba(0x00000066)`

---

### 2. AppState — 新增方法 (`src/state/app.rs`)

#### 2.1 新增字段

```rust
pub struct AppState {
    // ... 现有字段 ...

    /// Shared with SyncManager — true when local data has changed.
    /// [FUTURE] 当 SyncManager 迁移到 GPUI 时，此字段将像旧 ClipboardService::sync_dirty
    /// 一样传入 SyncManager::new()，用于触发同步周期。墓碑逻辑（record_item_deletion、
    /// record_unfavorite、remove_unfavorite）已内置于下方方法中，SyncManager 无需额外适配。
    pub sync_dirty: Arc<AtomicBool>,
}
```

#### 2.2 `toggle_favorite(id)` — 翻转收藏

```rust
/// Toggle favorite status for a single item.
///
/// # Tombstones (sync)
/// - Favorited → unfavorited: records `unfavorited_items` tombstone
/// - Unfavorited → favorited: removes existing `unfavorited_items` tombstone
/// - Marks `sync_dirty = true`
///
/// # Incremental update
/// - Updates item.is_favorite and item.updated_at in `self.items` directly
/// - Unless favorites filter is active (needs full reload to maintain filter accuracy)
pub fn toggle_favorite(&mut self, id: i64) { ... }
```

**逻辑（对齐旧 `ClipboardService::toggle_favorite`）：**
1. `needs_full_refresh = self.filters.is_favorites_active()`
2. 通过 DB 读取当前 `is_favorite`（用于判断墓碑方向）
3. `db.toggle_favorite(id)` — SQL CASE 翻转
4. 根据 `was_fav` 方向：`record_unfavorite` 或 `remove_unfavorite`
5. `self.sync_dirty.store(true, ...)`
6. 如果 favorites 过滤激活 → `reload_items()` + `clear_selection()`
   否则 → 更新 `items` 中对应 item 的 `is_favorite` 和 `updated_at`

#### 2.3 `delete_item(id)` — 删除单条

```rust
/// Delete a single item and record deletion tombstone for sync.
///
/// # Tombstones (sync)
/// - Records `deleted_items` tombstone with content_hash, timestamp, device_name
/// - Marks `sync_dirty = true`
///
/// # Side effects
/// - Removes item from `self.items`
/// - Removes id from `self.selected_ids`
pub fn delete_item(&mut self, id: i64) { ... }
```

**逻辑（对齐旧 `ClipboardService::delete_item`）：**
1. `db.get_by_id(id)` 获取 `content_hash`
2. `db.delete_item(id)`
3. `db.record_item_deletion(hash, now, hostname())`
4. `self.sync_dirty.store(true, ...)`
5. `items.retain(|it| it.id != id)`
6. `selected_ids.retain(|&sid| sid != id)`

#### 2.4 `batch_toggle_favorite()` — 批量翻转收藏

```rust
/// Batch toggle favorite on all selected items.
/// Loops selected_ids, applies the same toggle+tombstone logic per item.
pub fn batch_toggle_favorite(&mut self) { ... }
```

**逻辑（对齐旧 `ClipboardService::batch_toggle_favorite`）：**
1. 遍历 `selected_ids`，对每个 id 执行与 `toggle_favorite` 相同的 DB + 墓碑逻辑
2. `sync_dirty = true`
3. 如果 favorites 过滤激活 → `reload_items()` + `clear_selection()`
   否则 → 更新 `items` 中对应每个 item + 清理 selection

#### 2.5 `batch_delete()` — 批量删除

```rust
/// Batch delete all selected items.
/// Records deletion tombstones for each deleted item in one batch.
pub fn batch_delete(&mut self) { ... }
```

**逻辑（对齐旧 `ClipboardService::batch_delete`）：**
1. 先收集所有 selected_ids 的 `content_hash` 列表
2. 逐个 `db.delete_item(id)`
3. 逐个 `db.record_item_deletion(hash, now, hostname())`
4. `sync_dirty = true`
5. `items.retain(|it| !selected_ids.contains(&it.id))`
6. `selected_ids.clear()`

#### 2.6 辅助方法 — `hostname()`

`AppState` 不需要自己实现 — 直接调用 `crate::services::backends::local_folder::hostname()`。

---

### 3. ClipboardListView — 改造 (`src/ui/clipboard_list.rs`)

#### 3.1 新增状态

```rust
/// Active confirmation dialog (None = hidden).
confirm_dialog: Option<ConfirmDialogState>,

pub enum ConfirmDialogState {
    DeleteSingle { id: i64, preview: String },
    DeleteBatch { count: usize },
}
```

#### 3.2 新增访问器

```rust
pub fn confirm_dialog_state(&self) -> Option<&ConfirmDialogState> {
    self.confirm_dialog.as_ref()
}

pub fn dismiss_confirm_dialog(&mut self, cx: &mut Context<Self>) {
    self.confirm_dialog = None;
    cx.notify();
}
```

#### 3.3 修改 `handle_toolbar_action` — 淘汰 stub

```diff
- "open_image" | "qr_action" | "open_location" | "edit"
- | "toggle_favorite" | "delete"
- | "batch_favorite" | "batch_delete" => {}
+ "toggle_favorite" => {
+     if let Some(index) = self.hovered_index {
+         if let Some(item) = self.items.get(index) {
+             let id = item.id;
+             self.state.update(cx, |s, _cx| s.toggle_favorite(id));
+         }
+     }
+ }
+ "delete" => {
+     if let Some(index) = self.hovered_index {
+         if let Some(item) = self.items.get(index) {
+             let preview = truncate_preview(&item.full_text);
+             self.confirm_dialog = Some(ConfirmDialogState::DeleteSingle {
+                 id: item.id,
+                 preview,
+             });
+             cx.notify();
+         }
+     }
+ }
+ "batch_favorite" => {
+     self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
+ }
+ "batch_delete" => {
+     let count = self.selected_ids.len();
+     self.confirm_dialog = Some(ConfirmDialogState::DeleteBatch { count });
+     cx.notify();
+ }
```

#### 3.4 修改 `handle_menu_action` — 淘汰 stub

同样的模式，但使用 `self.context_menu_item` 获取当前项。

注意菜单 action 处理后继续调用 `self.hide_context_menu(cx)` — 对于 delete 需要在弹出 confirm dialog 前关闭菜单，所以在设置 `confirm_dialog` 之前先 `hide_context_menu(cx)`（或隐藏菜单后再设置弹窗，不受影响）。

对于 toggle_favorite/batch_favorite：保持现有模式 — 执行后 `hide_context_menu(cx)`。

对于 batch_delete：与工具栏相同，设置 `confirm_dialog` + `cx.notify()`。菜单隐藏时机不变。

#### 3.5 `truncate_preview` 辅助函数

```rust
/// Truncate text for confirm dialog preview, max ~30 chars.
fn truncate_preview(text: &str) -> String {
    let text = text.trim().replace('\n', " ");
    if text.chars().count() > 30 {
        format!("{}...", text.chars().take(30).collect::<String>())
    } else if text.is_empty() {
        "(empty)".into()
    } else {
        text
    }
}
```

---

### 4. RootView — 渲染 ConfirmDialog (`src/ui/root.rs`)

跟随 ContextMenu 叠加层模式，在 RootView::render() 末尾 `.when()` 链中增加：

```rust
.when(
    self.list_view.read(cx).confirm_dialog_state().is_some() && is_clipboard,
    |root| {
        let list = self.list_view.clone();
        let state = self.state.clone();
        let dialog_state = list.read(cx).confirm_dialog_state().unwrap();

        let (confirm, cancel) = match dialog_state {
            ConfirmDialogState::DeleteSingle { id, preview } => {
                let id = *id;
                let preview = preview.clone();
                let s = state.clone();
                let l = list.clone();
                (
                    {
                        let s = s.clone(); let l = l.clone();
                        ConfirmDialog::delete_single(&preview)
                            .on_confirm(move |_window, cx| {
                                s.update(cx, |s, _cx| s.delete_item(id));
                                l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                            })
                            .on_cancel(move |_window, cx| {
                                l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                            })
                            .into_any_element()
                    },
                    div().into_any_element(), // unused
                );
                (confirm, div().into_any_element())
            }
            ConfirmDialogState::DeleteBatch { count } => {
                let count = *count;
                let s = state.clone();
                let l = list.clone();
                ConfirmDialog::delete_batch(count)
                    .on_confirm(move |_window, cx| {
                        s.update(cx, |s, _cx| s.batch_delete());
                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                    })
                    .on_cancel({
                        let l = list.clone();
                        move |_window, cx| {
                            l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                        }
                    })
                    .into_any_element()
            }
        };

        root.child(
            div().absolute().size_full().bg(rgba(0x00000066))
                .on_mouse_down(MouseButton::Left, {
                    let l = list.clone();
                    move |_ev, _window, cx| {
                        cx.stop_propagation();
                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                    }
                })
        )
        .child(
            div().absolute().size_full().flex().items_center().justify_center().occlude()
                .child(match dialog_state {
                    ConfirmDialogState::DeleteSingle { id, preview } => { ... }
                    ConfirmDialogState::DeleteBatch { count } => { ... }
                })
        )
    },
)
```

> **注意:** 实际实现时需仔细处理 borrow checker — `list.read(cx)` 获取 dialog_state 后立即解构/克隆所需字段，避免在闭包中持有借用的引用。

---

### 5. 模块注册 (`src/ui/mod.rs`)

```rust
pub mod confirm_dialog;
```

---

## 同步墓碑对齐说明

当前 GPUI 版 `AppState` 尚未集成 `SyncManager`。此次变更在 `AppState` 中预留了 `sync_dirty: Arc<AtomicBool>` 字段，并在每个数据变更方法中：

1. **收藏翻转时** → `record_unfavorite` / `remove_unfavorite` tombstone（对齐旧 `ClipboardService::toggle_favorite` 第 734-748 行）
2. **删除时** → `record_item_deletion` tombstone（对齐旧 `ClipboardService::delete_item` 第 767-776 行）
3. **操作后** → `sync_dirty.store(true, Ordering::SeqCst)`（对齐旧 `ClipboardService::mark_dirty` 第 821-823 行）

### [FUTURE] SyncManager 对接指南

当 `SyncManager` 迁移到 GPUI 时，只需：

1. 在创建 `SyncManager` 时传入 `state.sync_dirty.clone()`
2. `SyncManager` 读取 dirty flag 触发同步周期（与旧代码 `src/services/sync.rs:533` 模式一致）
3. 墓碑记录（`record_item_deletion`、`record_unfavorite`、`remove_unfavorite`）已在 `AppState` 方法中完成，无需 SyncManager 处理

旧代码参考位置：
- `src/app.rs:164-171` — SyncManager 创建时接收 `sync_dirty_flag`
- `src/services/sync.rs:533` — `self.dirty.load(Ordering::SeqCst)` 触发同步
- `src/services/sync.rs:552` — `self.dirty.swap(false, Ordering::SeqCst)` 消费 dirty flag

### [FUTURE] ConfirmDialog 扩展指南

`ConfirmDialog` 预设了 `remove_blacklist(app_name)` 工厂方法。后续快捷键黑名单任务中，直接调用：

```rust
self.confirm_dialog = Some(ConfirmDialogState::RemoveBlacklist {
    app_name: "Chrome".into(),
});
```

然后在 `ConfirmDialogState` 枚举中添加对应变体，在 RootView 渲染 match 中增加对应分支。

---

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/ui/confirm_dialog.rs` | **新增** | 通用确认弹窗（RenderOnce，Builder API，预设工厂） |
| `src/ui/mod.rs` | 修改 | 添加 `pub mod confirm_dialog` |
| `src/state/app.rs` | 修改 | +`sync_dirty` 字段，+`toggle_favorite`、`delete_item`、`batch_toggle_favorite`、`batch_delete` 方法（含墓碑+sync dirty） |
| `src/ui/clipboard_list.rs` | 修改 | +`confirm_dialog` 状态，+`ConfirmDialogState` 枚举，+`truncate_preview` 辅助函数，淘汰 fav/delete/batch stub |
| `src/ui/root.rs` | 修改 | 渲染 ConfirmDialog 叠加层 |

## 不复用的代码

- 不移除 `src/services/clipboard.rs` 中的旧 ClipboardService（Slint 主进程仍在运行，不能破坏）
- 不修改 `src/ui/hover_toolbar.rs`（按钮已渲染，action 名称不变）
- 不修改 `src/ui/context_menu.rs`（菜单项已渲染，action 名称不变）
- 不修改 `src/core/db.rs`（所有 DB 方法已就绪）

## 交互细节对齐

| Slint 行为 | GPUI 实现 |
|-----------|----------|
| 悬停工具栏点击收藏 → 翻转 is_favorite | `handle_toolbar_action("toggle_favorite")` → `AppState.toggle_favorite(id)` |
| 右键菜单点击 Fav/Unfav → 翻转 | `handle_menu_action("toggle_favorite")` → 同上 |
| 批量工具栏点击收藏 → 批量翻转 | `handle_toolbar_action("batch_favorite")` → `AppState.batch_toggle_favorite()` |
| 右键菜单点击 Batch fav → 批量翻转 | `handle_menu_action("batch_favorite")` → 同上 |
| 收藏翻转后 DB + tombstone | `AppState` 方法内置墓碑处理 |
| 删除 — **新增**确认弹窗 | `handle_toolbar_action("delete")` → 设置 `confirm_dialog` → `ConfirmDialog` 叠加层 → 确认后 `AppState.delete_item(id)` |
| 批量删除 — **新增**确认弹窗 | `handle_toolbar_action("batch_delete")` → 设置 `confirm_dialog` → 确认后 `AppState.batch_delete()` |
| 卡片左侧收藏指示线 (fav_color) | ClipboardCard 已有实现 |
| 菜单中 Fav 图标根据 is_favorite 切换 | ContextMenu::for_item 已基于 `MenuItemContext.is_favorite` 动态切换 |
