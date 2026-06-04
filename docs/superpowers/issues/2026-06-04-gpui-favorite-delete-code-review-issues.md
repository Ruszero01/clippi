# Code Review Issues — Favorite/Delete Migration + ConfirmDialog

> **来源:** `docs/superpowers/specs/2026-06-04-gpui-favorite-delete-confirm-dialog-design.md`
> **分支:** `experiment/gpui-migration`
> **日期:** 2026-06-04
> **状态:** 记录待跟进

---

## ✅ 已修复

### 1. 删除/收藏后列表不刷新

**严重程度:** 重要
**文件:** `src/ui/clipboard_list.rs`, `src/ui/root.rs`
**修复提交:** `34327f1`

`ClipboardListView.items` 本地副本在 `AppState` 数据变更后未同步，导致删除后卡片仍停留在列表中，直到下次剪贴板事件触发 `set_items()`。

**修复方案:** 新增 `sync_items_from_state()` 方法，在 `toggle_favorite`/`batch_toggle_favorite` 和 RootView 的 delete 确认回调中调用。

---

## ❌ 不修复（Slint 端已知问题，对齐旧行为）

### 2. batch_delete 删除失败仍记录墓碑

**严重程度:** 重要
**文件:** `src/state/app.rs` → `batch_delete()`
**Slint 参考:** `src/services/clipboard.rs` 第 970-976 行

当 `db.get_by_id` 成功但 `db.delete_item` 失败时，content_hash 仍被推入 hashes 并记录为删除墓碑，其它同步设备会收到虚假的删除通知。

**建议修复:** 将 `hashes.push(item.content_hash)` 移到 `db.delete_item(id)` 成功之后。

```rust
// 当前（有 bug）
for &id in &self.selected_ids {
    if let Ok(Some(item)) = self.db.get_by_id(id) {
        hashes.push(item.content_hash); // ← 在删除前推送
    }
    if let Err(e) = self.db.delete_item(id) {
        log::error!("...");
    }
}

// 修复后
for &id in &self.selected_ids {
    if let Ok(Some(item)) = self.db.get_by_id(id) {
        match self.db.delete_item(id) {
            Ok(_) => hashes.push(item.content_hash),
            Err(e) => log::error!("batch delete_item({id}): {e}"),
        }
    }
}
```

### 3. toggle_favorite 重复调用 db.get_by_id

**严重程度:** 次要
**文件:** `src/state/app.rs` → `toggle_favorite()`
**Slint 参考:** `src/services/clipboard.rs` 第 725-749 行

先调用 `db.get_by_id(id)` 读 `was_fav`，再调用一次读 `content_hash` 用于墓碑。第一次读取已包含所有字段，可复用。

---

## ❌ 不修复（预存模式，应统一处理）

### 4. ConfirmDialog 颜色硬编码 vs ClippiTheme

**严重程度:** 重要
**文件:** `src/ui/confirm_dialog.rs`
**同样影响:** `src/ui/context_menu.rs`, `src/ui/toast.rs`

ConfirmDialog 始终使用暗色主题颜色，不随用户主题设置变化。ContextMenu 和 Toast 有同样问题。

**建议修复:** 后续统一为所有浮层组件添加 `theme: &ClippiTheme` 参数，或引入全局主题上下文。

### 5. ConfirmDialog 无键盘支持

**严重程度:** 重要
**文件:** `src/ui/confirm_dialog.rs`
**同样影响:** `src/ui/context_menu.rs`

弹窗不支持 Escape 取消 / Enter 确认。ContextMenu 同样缺失。

**建议修复:** 在浮层容器上添加 `on_key_down` handler：
```rust
.on_key_down(move |ev, window, cx| {
    match ev.keystroke.key.as_str() {
        "escape" => on_cancel(window, cx),
        "enter"  => on_confirm(window, cx),
        _ => {}
    }
})
```

---

## ❌ 不修复（微优化/风格，非 bug）

### 6. toggle_favorite 中 now/device 分支内重复计算

**文件:** `src/state/app.rs` → `toggle_favorite()`

`chrono::Utc::now()` 和 `hostname()` 在 `was_fav` 的 if/else 两分支内各调用一次，可提到分支前计算一次。

### 7. batch_toggle_favorite 中 updated_at 循环内重复计算

**文件:** `src/state/app.rs` → `batch_toggle_favorite()`

增量更新循环每次迭代调用 `chrono::Utc::now()`，可提到循环外赋值一次。

### 8. handle_menu_action 中 batch_delete 双重 cx.notify()

**文件:** `src/ui/clipboard_list.rs`

`batch_delete` 分支显式调用 `cx.notify()` 后又 fall through 到 `hide_context_menu(cx)`，后者内部再次调用 `cx.notify()`。同一帧内两次通知，浪费但不影响功能。

### 9. batch_delete 缺少空选择守卫

**文件:** `src/ui/clipboard_list.rs`

当 `selected_ids` 为空时理论上可弹出 "Delete 0 items?" 对话框。实际不可达（批量 UI 仅当 `selected_count > 1` 时渲染）。

---

## ✅ 无需关注（审查误判）

### 10. RootView 缺少 cx.observe(&list_view)

GPUI 中通过 `child(Entity)` 渲染的子实体在 `cx.notify()` 时会自动触发父级重渲染，无需显式 `cx.observe()`。ContextMenu dismiss 和 ConfirmDialog cancel 均可正常触发 RootView 重渲染。

---

## 跟进优先级建议

| 优先级 | 问题 | 说明 |
|--------|------|------|
| P0 | — | （无阻塞性问题） |
| P1 | #2 batch_delete 墓碑 bug | 影响同步数据正确性，低风险修复 |
| P2 | #4 颜色硬编码 | 用户可见，需统一处理所有浮层 |
| P2 | #5 键盘支持 | 无障碍体验，需统一处理所有浮层 |
| P3 | #3, #6, #7 微优化 | 可择机处理 |
| — | #8, #9 | 无需处理 |
