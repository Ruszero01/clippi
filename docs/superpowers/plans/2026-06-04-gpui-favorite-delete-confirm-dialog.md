# Favorite/Delete Migration + ConfirmDialog — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate favorite and delete functionality (single + batch) from Slint to GPUI, with a reusable confirmation dialog for delete operations.

**Architecture:** New `ConfirmDialog` RenderOnce component (Builder API), 4 new `AppState` methods with tombstone handling + `sync_dirty`, wire up stubbed action handlers in `ClipboardListView`, render dialog overlay in `RootView`.

**Tech Stack:** Rust, GPUI, rusqlite, chrono

**Spec:** `docs/superpowers/specs/2026-06-04-gpui-favorite-delete-confirm-dialog-design.md`

---

### Task 1: Create ConfirmDialog component

**Files:**
- Create: `g:\Develop\github\clippi\src\ui\confirm_dialog.rs`

- [ ] **Step 1: Write the ConfirmDialog struct and full implementation**

```rust
//! Confirmation dialog — reusable modal overlay with configurable content.
//!
//! Usage:
//! ```ignore
//! ConfirmDialog::delete_single("preview text")
//!     .on_confirm(move |_window, cx| { /* do delete */ })
//!     .on_cancel(move |_window, cx| { /* dismiss */ })
//! ```
//!
//! Preset factories:
//! - `ConfirmDialog::delete_single(preview)` — single item delete
//! - `ConfirmDialog::delete_batch(count)` — batch delete
//! - `ConfirmDialog::remove_blacklist(app_name)` — [FUTURE] hotkey blacklist removal

use std::rc::Rc;

use gpui::*;

#[derive(IntoElement)]
pub struct ConfirmDialog {
    title: String,
    message: String,
    confirm_label: String,
    cancel_label: String,
    danger: bool,
    on_confirm: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    on_cancel: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
}

impl ConfirmDialog {
    pub fn new() -> Self {
        Self {
            title: "Confirm".into(),
            message: String::new(),
            confirm_label: "Confirm".into(),
            cancel_label: "Cancel".into(),
            danger: false,
            on_confirm: None,
            on_cancel: None,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn confirm_label(mut self, label: impl Into<String>) -> Self {
        self.confirm_label = label.into();
        self
    }

    pub fn cancel_label(mut self, label: impl Into<String>) -> Self {
        self.cancel_label = label.into();
        self
    }

    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    pub fn on_confirm(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_confirm = Some(Rc::new(handler));
        self
    }

    pub fn on_cancel(
        mut self,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    // ── Preset factories ──

    /// Single item delete confirmation.
    /// `preview` should be a truncated content preview (~30 chars max).
    pub fn delete_single(preview: &str) -> Self {
        Self::new()
            .title("Confirm Delete")
            .message(format!(
                "Delete \"{}\"?\nThis action cannot be undone.",
                preview
            ))
            .confirm_label("Delete")
            .danger(true)
    }

    /// Batch delete confirmation for N selected items.
    pub fn delete_batch(count: usize) -> Self {
        Self::new()
            .title("Confirm Batch Delete")
            .message(format!(
                "Delete {} selected items?\nThis action cannot be undone.",
                count
            ))
            .confirm_label("Delete")
            .danger(true)
    }

    /// [FUTURE] Remove app from hotkey blacklist confirmation.
    /// Called from hotkey settings when user removes a blacklisted app.
    pub fn remove_blacklist(app_name: &str) -> Self {
        Self::new()
            .title("Remove from Blacklist")
            .message(format!(
                "Stop ignoring clipboard from \"{}\"?",
                app_name
            ))
            .confirm_label("Remove")
            .danger(false)
    }
}

impl RenderOnce for ConfirmDialog {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let surface = rgb(0x2c2d2e);
        let text_1 = rgb(0xeaebec);
        let text_2 = rgb(0x919496);
        let danger_color = rgb(0xff5f57);
        let accent = rgb(0x7ecba3);
        let border_color = rgba(0xffffff14);
        let overlay = rgba(0x00000066);

        let confirm_btn_color = if self.danger { danger_color } else { accent };

        let on_confirm = self.on_confirm.clone();
        let on_cancel = self.on_cancel.clone();
        let title = self.title.clone();
        let message = self.message.clone();
        let confirm_label = self.confirm_label.clone();
        let cancel_label = self.cancel_label.clone();

        // Fullscreen overlay
        div()
            .absolute()
            .size_full()
            .bg(overlay)
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, {
                let on_cancel = on_cancel.clone();
                move |_ev, window, cx| {
                    cx.stop_propagation();
                    if let Some(ref handler) = on_cancel {
                        handler(window, cx);
                    }
                }
            })
            .child(
                // Modal card — occluded to prevent click-through to backdrop
                div()
                    .w(px(280.))
                    .bg(surface)
                    .rounded(px(12.))
                    .border(px(1.))
                    .border_color(border_color)
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .occlude()
                    // Title
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(title.clone()),
                    )
                    // Message
                    .child(
                        div()
                            .mt(px(8.))
                            .text_size(px(12.))
                            .text_color(text_2)
                            .child(message.clone()),
                    )
                    // Button row
                    .child(
                        div()
                            .mt(px(16.))
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            // Cancel button
                            .child(
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.))
                                    .text_color(text_2)
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(rgba(0xffffff10)))
                                    .on_mouse_down(MouseButton::Left, {
                                        let on_cancel = on_cancel.clone();
                                        move |_ev, window, cx| {
                                            cx.stop_propagation();
                                            if let Some(ref handler) = on_cancel {
                                                handler(window, cx);
                                            }
                                        }
                                    })
                                    .child(cancel_label.clone()),
                            )
                            // Confirm button
                            .child(
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.))
                                    .text_color(rgb(0xffffff))
                                    .bg(confirm_btn_color)
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.opacity(0.85))
                                    .on_mouse_down(MouseButton::Left, {
                                        let on_confirm = on_confirm.clone();
                                        move |_ev, window, cx| {
                                            cx.stop_propagation();
                                            if let Some(ref handler) = on_confirm {
                                                handler(window, cx);
                                            }
                                        }
                                    })
                                    .child(confirm_label.clone()),
                            ),
                    ),
            )
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/ui/confirm_dialog.rs
git commit -m "feat: add reusable ConfirmDialog component (RenderOnce + Builder API)"
```

---

### Task 2: Add sync_dirty + business logic methods to AppState

**Files:**
- Modify: `g:\Develop\github\clippi\src\state\app.rs`

- [ ] **Step 1: Add `sync_dirty` field and import to AppState struct**

In the imports section (around line 13), ensure `Ordering` is imported from `std::sync::atomic`:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
```

In the `AppState` struct, add the field after `batch_pasting` (around line 41):

```rust
/// Shared with SyncManager — true when local data has changed.
/// [FUTURE] When SyncManager is migrated to GPUI, pass this Arc to
/// SyncManager::new() so it can detect local changes and trigger sync
/// cycles. Tombstone recording (record_item_deletion, record_unfavorite,
/// remove_unfavorite) is already handled in the data mutation methods
/// below — SyncManager only needs to observe this flag.
pub sync_dirty: Arc<AtomicBool>,
```

- [ ] **Step 2: Initialize `sync_dirty` in `AppState::new()`**

In the `new()` method's `Self { ... }` block, add the field:

```rust
sync_dirty: Arc::new(AtomicBool::new(false)),
```

- [ ] **Step 3: Add `toggle_favorite` method**

Before the `fn order_by` method (before line 249), add:

```rust
/// Toggle favorite status for a single item.
///
/// # Tombstones (sync)
/// - Favorited → unfavorited: records `unfavorited_items` tombstone
/// - Unfavorited → favorited: removes existing `unfavorited_items` tombstone
/// - Sets `sync_dirty = true`
///
/// # Incremental update
/// Updates `item.is_favorite` and `item.updated_at` in `self.items` directly,
/// unless the favorites filter is active (needs full reload for accuracy).
pub fn toggle_favorite(&mut self, id: i64) {
    let needs_full_refresh = self.filters.is_favorites_active();

    // Read current state before toggling (needed for tombstone direction)
    let was_fav = self
        .db
        .get_by_id(id)
        .ok()
        .flatten()
        .is_some_and(|item| item.is_favorite);

    if let Err(e) = self.db.toggle_favorite(id) {
        log::error!("toggle_favorite({id}): {e}");
        return;
    }

    // Tombstone management
    if was_fav {
        // Was favorited, now unfavorited — record tombstone
        if let Ok(Some(item)) = self.db.get_by_id(id) {
            let now = chrono::Utc::now().to_rfc3339();
            let device = crate::services::backends::local_folder::hostname();
            if let Err(e) = self.db.record_unfavorite(item.content_hash, &now, &device) {
                log::error!("record_unfavorite({}): {e}", item.content_hash);
            }
        }
    } else {
        // Was unfavorited, now favorited — remove tombstone
        if let Ok(Some(item)) = self.db.get_by_id(id) {
            if let Err(e) = self.db.remove_unfavorite(item.content_hash) {
                log::error!("remove_unfavorite({}): {e}", item.content_hash);
            }
        }
    }

    self.sync_dirty.store(true, Ordering::SeqCst);

    if needs_full_refresh {
        self.reload_items();
        self.clear_selection();
    } else {
        // Incremental update: flip is_favorite + bump updated_at
        if let Some(item) = self.items.iter_mut().find(|it| it.id == id) {
            item.is_favorite = !item.is_favorite;
            item.updated_at = chrono::Utc::now();
        }
    }
}
```

- [ ] **Step 4: Add `delete_item` method**

```rust
/// Delete a single item and record deletion tombstone for sync.
///
/// # Tombstones (sync)
/// - Records `deleted_items` tombstone with content_hash, timestamp, device_name
/// - Sets `sync_dirty = true`
///
/// # Side effects
/// - Removes item from `self.items`
/// - Removes id from `self.selected_ids`
pub fn delete_item(&mut self, id: i64) {
    // Read item first to get content_hash for tombstone
    if let Ok(Some(item)) = self.db.get_by_id(id) {
        let hash = item.content_hash;
        let now = chrono::Utc::now().to_rfc3339();
        let device = crate::services::backends::local_folder::hostname();

        if let Err(e) = self.db.delete_item(id) {
            log::error!("delete_item({id}): {e}");
            return;
        }

        // Record deletion tombstone for sync propagation
        if let Err(e) = self.db.record_item_deletion(hash, &now, &device) {
            log::error!("record_item_deletion({hash}): {e}");
        }
    } else {
        log::warn!("delete_item({id}): item not found");
        return;
    }

    self.sync_dirty.store(true, Ordering::SeqCst);

    // Remove from in-memory items and selection
    self.items.retain(|it| it.id != id);
    self.selected_ids.retain(|&sid| sid != id);
}
```

- [ ] **Step 5: Add `batch_toggle_favorite` method**

```rust
/// Batch toggle favorite on all selected items.
/// Loops selected_ids, applies the same toggle + tombstone logic per item.
pub fn batch_toggle_favorite(&mut self) {
    let needs_full_refresh = self.filters.is_favorites_active();
    let now = chrono::Utc::now().to_rfc3339();
    let device = crate::services::backends::local_folder::hostname();

    let ids: Vec<i64> = self.selected_ids.clone();
    for &id in &ids {
        let was_fav = self
            .db
            .get_by_id(id)
            .ok()
            .flatten()
            .is_some_and(|item| item.is_favorite);

        if let Err(e) = self.db.toggle_favorite(id) {
            log::error!("batch toggle_favorite({id}): {e}");
            continue;
        }

        if was_fav {
            if let Ok(Some(item)) = self.db.get_by_id(id) {
                if let Err(e) = self.db.record_unfavorite(item.content_hash, &now, &device) {
                    log::error!("batch record_unfavorite({}): {e}", item.content_hash);
                }
            }
        } else {
            if let Ok(Some(item)) = self.db.get_by_id(id) {
                if let Err(e) = self.db.remove_unfavorite(item.content_hash) {
                    log::error!("batch remove_unfavorite({}): {e}", item.content_hash);
                }
            }
        }
    }

    self.sync_dirty.store(true, Ordering::SeqCst);

    if needs_full_refresh {
        self.reload_items();
        self.clear_selection();
    } else {
        // Incremental update: flip is_favorite + bump updated_at for each
        for id in &ids {
            if let Some(item) = self.items.iter_mut().find(|it| &it.id == id) {
                item.is_favorite = !item.is_favorite;
                item.updated_at = chrono::Utc::now();
            }
        }
    }
}
```

- [ ] **Step 6: Add `batch_delete` method**

```rust
/// Batch delete all selected items.
/// Records deletion tombstones for each deleted item.
pub fn batch_delete(&mut self) {
    let now = chrono::Utc::now().to_rfc3339();
    let device = crate::services::backends::local_folder::hostname();

    // Collect hashes before deleting
    let mut hashes: Vec<u64> = Vec::with_capacity(self.selected_ids.len());
    for &id in &self.selected_ids {
        if let Ok(Some(item)) = self.db.get_by_id(id) {
            hashes.push(item.content_hash);
        }
        if let Err(e) = self.db.delete_item(id) {
            log::error!("batch delete_item({id}): {e}");
        }
    }

    // Record tombstones for sync
    for h in &hashes {
        if let Err(e) = self.db.record_item_deletion(*h, &now, &device) {
            log::error!("batch record_item_deletion({h}): {e}");
        }
    }

    self.sync_dirty.store(true, Ordering::SeqCst);

    // Remove from in-memory items
    let ids: Vec<i64> = self.selected_ids.drain(..).collect();
    self.items.retain(|it| !ids.contains(&it.id));
}
```

- [ ] **Step 7: Commit**

```bash
git add src/state/app.rs
git commit -m "feat: add toggle_favorite, delete_item, batch ops + sync_dirty to AppState"
```

---

### Task 3: Wire up ClipboardListView action handlers + confirm dialog state

**Files:**
- Modify: `g:\Develop\github\clippi\src\ui\clipboard_list.rs`

- [ ] **Step 1: Add `ConfirmDialogState` enum and `confirm_dialog` field**

After the `pub struct ClipboardListView` definition (around line 20), paste this enum above it:

```rust
/// Types of confirmation dialogs that can be shown.
/// [FUTURE] Add variants here for other confirmation scenarios
/// (e.g. RemoveBlacklist { app_name: String } for hotkey settings).
#[derive(Clone)]
enum ConfirmDialogState {
    DeleteSingle { id: i64, preview: String },
    DeleteBatch { count: usize },
}
```

Inside the `ClipboardListView` struct, add the field after `note_input` (around line 46):

```rust
/// Active confirmation dialog (None = hidden).
confirm_dialog: Option<ConfirmDialogState>,
```

- [ ] **Step 2: Initialize `confirm_dialog` in `ClipboardListView::new()`**

In the `Self { ... }` block, after `note_input`, add:

```rust
confirm_dialog: None,
```

- [ ] **Step 3: Add accessor methods**

After the `hide_context_menu` method (around line 270), add:

```rust
/// Get the current confirmation dialog state, if any.
pub fn confirm_dialog_state(&self) -> Option<&ConfirmDialogState> {
    self.confirm_dialog.as_ref()
}

/// Dismiss the active confirmation dialog.
pub fn dismiss_confirm_dialog(&mut self, cx: &mut Context<Self>) {
    self.confirm_dialog = None;
    cx.notify();
}
```

- [ ] **Step 4: Replace `handle_menu_action` stub with real implementations**

Replace lines 315-318:

```rust
            // Other actions deferred to follow-up
            "edit" | "toggle_favorite" | "delete"
            | "open_image" | "paste_ocr" | "qr_detect" | "show_tag_picker"
            | "batch_favorite" | "batch_delete" => {}
```

With:

```rust
            "toggle_favorite" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    self.state.update(cx, |s, _cx| s.toggle_favorite(id));
                }
            }
            "delete" => {
                if let Some(ref item) = self.context_menu_item {
                    let id = item.id;
                    let preview = truncate_for_dialog(&item.full_text);
                    self.hide_context_menu(cx);
                    self.confirm_dialog = Some(ConfirmDialogState::DeleteSingle {
                        id,
                        preview,
                    });
                    cx.notify();
                }
            }
            "batch_favorite" => {
                self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
            }
            "batch_delete" => {
                let count = self.selected_ids.len();
                self.confirm_dialog = Some(ConfirmDialogState::DeleteBatch { count });
                cx.notify();
            }
            // Other actions still deferred to follow-up
            "edit" | "open_image" | "paste_ocr" | "qr_detect" | "show_tag_picker" => {}
```

- [ ] **Step 5: Replace `handle_toolbar_action` stub with real implementations**

Replace lines 356-359:

```rust
            // Other hover toolbar actions deferred to follow-up
            "open_image" | "qr_action" | "open_location" | "edit"
            | "toggle_favorite" | "delete"
            | "batch_favorite" | "batch_delete" => {}
```

With:

```rust
            "toggle_favorite" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        self.state.update(cx, |s, _cx| s.toggle_favorite(item.id));
                    }
                }
            }
            "delete" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        let preview = truncate_for_dialog(&item.full_text);
                        self.confirm_dialog = Some(ConfirmDialogState::DeleteSingle {
                            id: item.id,
                            preview,
                        });
                        cx.notify();
                    }
                }
            }
            "batch_favorite" => {
                self.state.update(cx, |s, _cx| s.batch_toggle_favorite());
            }
            "batch_delete" => {
                let count = self.selected_ids.len();
                self.confirm_dialog = Some(ConfirmDialogState::DeleteBatch { count });
                cx.notify();
            }
            // Other actions still deferred to follow-up
            "open_image" | "qr_action" | "open_location" | "edit" => {}
```

- [ ] **Step 6: Add `truncate_for_dialog` helper function**

At the bottom of the file (before the `impl Render` block), add:

```rust
/// Truncate text for confirm dialog preview.
/// Max ~30 chars, newlines collapsed to spaces.
fn truncate_for_dialog(text: &str) -> String {
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

- [ ] **Step 7: Commit**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat: wire up favorite/delete actions + confirm dialog state in ClipboardListView"
```

---

### Task 4: Render ConfirmDialog in RootView

**Files:**
- Modify: `g:\Develop\github\clippi\src\ui\root.rs`

- [ ] **Step 1: Add import for ConfirmDialog**

After the existing imports, add:

```rust
use super::confirm_dialog::ConfirmDialog;
use super::clipboard_list::ConfirmDialogState;
```

> **Note:** `ConfirmDialogState` must be `pub(crate)` in clipboard_list.rs. Update the enum declaration:

```rust
pub(crate) enum ConfirmDialogState {
    DeleteSingle { id: i64, preview: String },
    DeleteBatch { count: usize },
}
```

- [ ] **Step 2: Add ConfirmDialog rendering block in RootView::render()**

In the `render` method, after the ContextMenu `.when()` block (after line 289, just before the closing `}`), add:

```rust
            .when(
                self.list_view.read(cx).confirm_dialog_state().is_some() && is_clipboard,
                |root| {
                    let list = self.list_view.clone();
                    let app_state = self.state.clone();

                    // Read dialog state and clone what we need before closures
                    let dialog = list.read(cx).confirm_dialog_state().cloned();
                    let dialog_element: AnyElement = match dialog {
                        Some(ConfirmDialogState::DeleteSingle { id, preview }) => {
                            ConfirmDialog::delete_single(&preview)
                                .on_confirm({
                                    let s = app_state.clone();
                                    let l = list.clone();
                                    move |_window, cx| {
                                        s.update(cx, |s, _cx| s.delete_item(id));
                                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                                    }
                                })
                                .on_cancel({
                                    let l = list.clone();
                                    move |_window, cx| {
                                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                                    }
                                })
                                .into_any_element()
                        }
                        Some(ConfirmDialogState::DeleteBatch { count }) => {
                            ConfirmDialog::delete_batch(count)
                                .on_confirm({
                                    let s = app_state.clone();
                                    let l = list.clone();
                                    move |_window, cx| {
                                        s.update(cx, |s, _cx| s.batch_delete());
                                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                                    }
                                })
                                .on_cancel({
                                    let l = list.clone();
                                    move |_window, cx| {
                                        l.update(cx, |lst, cx| lst.dismiss_confirm_dialog(cx));
                                    }
                                })
                                .into_any_element()
                        }
                        None => div().into_any_element(),
                    };

                    root.child(dialog_element)
                },
            )
```

- [ ] **Step 3: Make `ConfirmDialogState` visibility public**

If not done in Task 3, ensure the enum in `clipboard_list.rs` is `pub(crate)`:

```rust
#[derive(Clone)]
pub(crate) enum ConfirmDialogState { ... }
```

- [ ] **Step 4: Commit**

```bash
git add src/ui/root.rs
git commit -m "feat: render ConfirmDialog overlay in RootView"
```

---

### Task 5: Register module and verify build

**Files:**
- Modify: `g:\Develop\github\clippi\src\ui\mod.rs`

- [ ] **Step 1: Register the new module**

Add after `pub mod clipboard_list;` (or anywhere in the list):

```rust
pub mod confirm_dialog;
```

- [ ] **Step 2: Build and fix any compilation errors**

```bash
cargo build 2>&1
```

Expected: Build succeeds with zero warnings.

- [ ] **Step 3: Run Clippy**

```bash
cargo clippy -- -D warnings 2>&1
```

Expected: Zero warnings.

- [ ] **Step 4: Commit**

```bash
git add src/ui/mod.rs
git commit -m "chore: register confirm_dialog module"
```

---

## Self-Review

### 1. Spec coverage

| Spec section | Task coverage |
|---|---|
| ConfirmDialog component (struct, Builder, RenderOnce) | Task 1 — full implementation |
| Preset factories (delete_single, delete_batch, remove_blacklist) | Task 1 — all three implemented |
| AppState.sync_dirty field | Task 2 Step 1-2 |
| AppState.toggle_favorite | Task 2 Step 3 |
| AppState.delete_item | Task 2 Step 4 |
| AppState.batch_toggle_favorite | Task 2 Step 5 |
| AppState.batch_delete | Task 2 Step 6 |
| ClipboardListView ConfirmDialogState enum | Task 3 Step 1 |
| ClipboardListView confirm_dialog field + accessors | Task 3 Step 1-3 |
| handle_menu_action wiring (fav/delete/batch) | Task 3 Step 4 |
| handle_toolbar_action wiring (fav/delete/batch) | Task 3 Step 5 |
| truncate_preview helper | Task 3 Step 6 |
| RootView ConfirmDialog rendering | Task 4 |
| Module registration | Task 5 |
| Tombstone alignment (sync) | Task 2 Steps 3-6 (inline in each method) |
| [FUTURE] markers for sync + blacklist | Task 1 (comment on remove_blacklist), Task 2 Step 1 (doc comment) |

### 2. Placeholder scan
- No TBD/TODO
- No "add error handling" without actual code
- No "write tests for the above"
- Every step has complete code

### 3. Type consistency
- `ConfirmDialogState` enum: same variants used in Task 3 (definition), Task 4 (match in RootView)
- `ConfirmDialogState` visibility: `pub(crate)` — Task 3 Step 6 + Task 4 Step 1 (import) agree
- Method signatures consistent across spec and tasks
- `id` is `i64` throughout (matches DB and ClipboardItem)
- `preview: String` consistent across ClipboardListView creation and ConfirmDialog::delete_single usage
