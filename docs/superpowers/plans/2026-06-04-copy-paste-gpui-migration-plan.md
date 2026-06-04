# Copy/Paste Feature Migration (Slint → GPUI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire up copy/paste/clipboard operations from GPUI UI components (HoverToolbar, ContextMenu, ClipboardCard) to actual clipboard backend operations.

**Architecture:** Extract `write_item_to_clipboard` into a shared `services::clipboard_ops` module; add `copy_item`/`paste_item`/`batch_paste` methods on `AppState`; fill the stub `handle_menu_action`/`handle_toolbar_action` methods in `ClipboardListView` to dispatch to `AppState`; use GPUI's native `click_count` field for double-click detection in `ClipboardCard`.

**Tech Stack:** Rust, GPUI, clipboard-rs, winapi (Windows PNG clipboard), sqlite (rusqlite)

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/services/clipboard_ops.rs` | **Create** | Standalone clipboard write/verify functions, extracted from Slint `app.rs` |
| `src/services/mod.rs` | Modify | Register `clipboard_ops` module |
| `src/state/app.rs` | Modify | Add `copy_item`, `paste_item`, `paste_as_rgb`, `paste_as_hex`, `batch_paste` methods |
| `src/ui/clipboard_list.rs` | Modify | Fill `handle_menu_action`/`handle_toolbar_action` stubs; wire `on_double_click` |
| `src/ui/clipboard_card.rs` | Modify | Add `on_double_click` callback, use `click_count` for double-click detection |
| `src/app.rs` | Modify | Update Slint callback to use new `clipboard_ops` module (keep Slint working) |

No changes:
- `src/ui/hover_toolbar.rs` — already emits correct action strings
- `src/ui/context_menu.rs` — already emits correct action strings
- `src/platform/paste.rs` — `restore_paste_target()`, `paste_after_delay()`, `paste_sync()` ready

---

### Task 1: Create `src/services/clipboard_ops.rs` — extract clipboard functions

**Files:**
- Create: `src/services/clipboard_ops.rs`

- [ ] **Step 1: Create the file with extracted and cleaned-up functions**

Copy `write_item_to_clipboard` from `src/app.rs:1925-2004`, `verify_clipboard_content` from `src/app.rs:2097-2112`, and `verify_clipboard_image` from `src/app.rs:2126-2141`. Remove dependency on `ClipboardShared` parameter — the extracted version uses `ClipboardContext` directly.

```rust
//! Platform-agnostic clipboard write and verification utilities.
//!
//! Extracted from the Slint `app.rs` callback layer so both the Slint and
//! GPUI frontends can share clipboard write logic.

use clipboard_rs::{Clipboard, ClipboardContent, ClipboardContext};

use crate::core::types::{ClipboardItem, ContentType, RichData};

/// Write a `ClipboardItem` to the system clipboard.
///
/// Handles image (PNG on Windows, image object on macOS), file (CF_HDROP),
/// plain text, and rich text (HTML + RTF). When `copy_as_plain_text` is true,
/// rich formatting is discarded.
pub fn write_item_to_clipboard(item: &ClipboardItem, copy_as_plain_text: bool) {
    if let Ok(ctx) = ClipboardContext::new() {
        if item.content_type == ContentType::Image && !item.image_path.is_empty() {
            #[cfg(target_os = "windows")]
            {
                use windows_sys::Win32::System::DataExchange::{
                    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
                    SetClipboardData,
                };
                use windows_sys::Win32::System::Memory::{
                    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
                };

                let png_bytes = std::fs::read(&item.image_path).unwrap_or_default();
                if !png_bytes.is_empty() {
                    let png_name: Vec<u16> = "PNG\0".encode_utf16().collect();
                    let png_fmt = unsafe { RegisterClipboardFormatW(png_name.as_ptr()) };

                    unsafe {
                        if OpenClipboard(std::ptr::null_mut()) != 0 {
                            EmptyClipboard();
                            if png_fmt != 0 {
                                let mem = GlobalAlloc(GMEM_MOVEABLE, png_bytes.len());
                                if !mem.is_null() {
                                    let ptr = GlobalLock(mem);
                                    if !ptr.is_null() {
                                        std::ptr::copy_nonoverlapping(
                                            png_bytes.as_ptr(),
                                            ptr as *mut u8,
                                            png_bytes.len(),
                                        );
                                        GlobalUnlock(mem);
                                        SetClipboardData(png_fmt as u32, mem);
                                    }
                                }
                            }
                            CloseClipboard();
                        }
                    }
                }
            }

            #[cfg(not(target_os = "windows"))]
            {
                use clipboard_rs::common::{RustImage, RustImageData};
                if let Ok(img_data) = RustImageData::from_path(&item.image_path) {
                    let _ = ctx.set_image(img_data);
                }
            }
        } else if item.content_type == ContentType::File && !item.file_data.is_empty() {
            let file_data = crate::core::types::FileData::from_json(&item.file_data);
            let paths: Vec<String> = file_data.files.iter().map(|f| f.path.clone()).collect();
            let contents = vec![ClipboardContent::Files(paths)];
            let _ = Clipboard::set(&ctx, contents);
        } else if copy_as_plain_text {
            let _ = Clipboard::set_text(&ctx, item.full_text.clone());
        } else {
            let mut contents = vec![ClipboardContent::Text(item.full_text.clone())];
            let rich = RichData::from_json(&item.rich_data);
            if let Some(html) = rich.html {
                contents.push(ClipboardContent::Html(html));
            }
            if let Some(rtf) = rich.rtf {
                contents.push(ClipboardContent::Rtf(rtf));
            }
            let _ = Clipboard::set(&ctx, contents);
        }
    }
}

/// Poll-read clipboard text until it matches `expected` or `timeout_ms` expires.
/// Returns `true` on match, `false` on timeout.
pub fn verify_clipboard_content(expected: &str, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if let Ok(ctx) = ClipboardContext::new() {
            if let Ok(text) = ctx.get_text() {
                if text == expected {
                    return true;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Poll-read clipboard PNG buffer until its length matches `expected_size` or timeout.
/// Returns `true` on match, `false` on timeout.
pub fn verify_clipboard_image(expected_size: u64, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if let Ok(ctx) = ClipboardContext::new() {
            if let Ok(png_bytes) = ctx.get_buffer("PNG") {
                if png_bytes.len() as u64 == expected_size {
                    return true;
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

- [ ] **Step 2: Register module in `src/services/mod.rs`**

Add `pub mod clipboard_ops;` after the existing modules.

In `src/services/mod.rs`, after line 8 (`pub mod poll_loop;`):

```rust
pub mod clipboard_ops;
```

- [ ] **Step 3: Update `src/app.rs` to use the new module**

In `src/app.rs`, replace the body of `write_item_to_clipboard` (lines 1930-2003) with a single delegation call to the new module, and do the same for `verify_clipboard_content` (lines 2097-2112) and `verify_clipboard_image` (lines 2126-2141).

Find the `write_item_to_clipboard` function body in `src/app.rs` (from `let mut pushed = false;` at line 1930 to the closing `}` before line 2005), and replace with:

```rust
    crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);
```

Remove the now-unused `shared` parameter from the function signature:
```
// Before:
fn write_item_to_clipboard(
    item: &crate::core::types::ClipboardItem,
    copy_as_plain_text: bool,
    shared: &ClipboardShared,
) {
// After:
fn write_item_to_clipboard(
    item: &crate::core::types::ClipboardItem,
    copy_as_plain_text: bool,
    _shared: &ClipboardShared,  // kept for API compat, unused
) {
```

Replace `verify_clipboard_content` function body (lines 2097-2112) with:

```rust
fn verify_clipboard_content(expected: &str, timeout_ms: u64) -> bool {
    crate::services::clipboard_ops::verify_clipboard_content(expected, timeout_ms)
}
```

Replace `verify_clipboard_image` function body (lines 2126-2141) with:

```rust
fn verify_clipboard_image(expected_size: u64, timeout_ms: u64) -> bool {
    crate::services::clipboard_ops::verify_clipboard_image(expected_size, timeout_ms)
}
```

Remove unused local variable `pushed` in `write_item_to_clipboard` caller — check that `bind_batch_callbacks` (line 1578) and `on_paste_item` (line 357) callers still compile since they pass `&shared`.

- [ ] **Step 4: Build to verify Slint side still compiles**

Run: `cargo build 2>&1`
Expected: Compilation succeeds with no errors.

- [ ] **Step 5: Commit**

```bash
git add src/services/clipboard_ops.rs src/services/mod.rs src/app.rs
git commit -m "refactor: extract clipboard write/verify functions to services::clipboard_ops

Extract write_item_to_clipboard, verify_clipboard_content, and
verify_clipboard_image from app.rs into a standalone module callable
from both Slint and GPUI frontends.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Add clipboard action methods to `AppState`

**Files:**
- Modify: `src/state/app.rs:258` (after closing brace of `impl AppState`)

- [ ] **Step 1: Add `copy_item` method**

Add to `impl AppState` block in `src/state/app.rs`, before the closing `}` at line 258:

```rust
    /// Copy a single item to the system clipboard (no paste simulation).
    pub fn copy_item(&self, id: i64, copy_as_plain_text: bool) {
        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("copy_item: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("copy_item: db error for {id}: {e}");
                return;
            }
        };
        crate::services::clipboard_ops::write_item_to_clipboard(&item, copy_as_plain_text);
    }
```

- [ ] **Step 2: Add `paste_item` method**

```rust
    /// Paste a single item: write to clipboard, restore focus, simulate Ctrl+V.
    ///
    /// For non-file items, verifies clipboard content before pasting.
    /// The actual paste runs asynchronously via `paste_after_delay()`.
    pub fn paste_item(&self, id: i64, copy_as_plain_text: bool) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            Ok(None) => {
                log::warn!("paste_item: item {id} not found");
                return;
            }
            Err(e) => {
                log::error!("paste_item: db error for {id}: {e}");
                return;
            }
        };

        let is_file = item.content_type == ContentType::File;
        let expected = item.full_text.clone();
        crate::services::clipboard_ops::write_item_to_clipboard(&item, copy_as_plain_text);

        if !expected.is_empty() && !is_file {
            crate::services::clipboard_ops::verify_clipboard_content(&expected, 200);
        }
        if is_file {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        restore_paste_target();
        paste_after_delay();
    }
```

- [ ] **Step 3: Add `paste_as_rgb` and `paste_as_hex` methods**

```rust
    /// Convert a color item from HEX to RGB and paste.
    pub fn paste_as_rgb(&self, id: i64) {
        use crate::core::color::detect_color;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            _ => {
                log::warn!("paste_as_rgb: item {id} not found");
                return;
            }
        };

        if let Some(color) = detect_color(&item.full_text) {
            let rgb_text = color.to_rgb();
            if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                let _ = clipboard_rs::Clipboard::set_text(&ctx, rgb_text);
            }
            crate::services::clipboard_ops::verify_clipboard_content(&rgb_text, 200);
            restore_paste_target();
            paste_after_delay();
        }
    }

    /// Convert a color item from RGB to HEX and paste.
    pub fn paste_as_hex(&self, id: i64) {
        use crate::core::color::detect_color;
        use crate::platform::paste::{paste_after_delay, restore_paste_target};

        let item = match self.db.get_by_id(id) {
            Ok(Some(item)) => item,
            _ => {
                log::warn!("paste_as_hex: item {id} not found");
                return;
            }
        };

        if let Some(color) = detect_color(&item.full_text) {
            let hex_text = color.to_css_hex();
            if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                let _ = clipboard_rs::Clipboard::set_text(&ctx, hex_text);
            }
            crate::services::clipboard_ops::verify_clipboard_content(&hex_text, 200);
            restore_paste_target();
            paste_after_delay();
        }
    }
```

- [ ] **Step 4: Add `batch_paste` method**

```rust
    /// Batch paste multiple items sequentially.
    ///
    /// Each item is written to clipboard, verified, and pasted via Ctrl+V.
    /// Newline separators are pasted between items. The last item uses
    /// async `paste_after_delay()`; all others use synchronous `paste_sync()`
    /// to avoid race conditions between clipboard writes.
    pub fn batch_paste(&self, ids: &[i64], copy_as_plain_text: bool) {
        use crate::core::types::ContentType;
        use crate::platform::paste::{paste_after_delay, paste_sync, restore_paste_target};

        let items: Vec<ClipboardItem> = ids
            .iter()
            .filter_map(|&id| self.db.get_by_id(id).ok().flatten())
            .collect();

        let n = items.len();
        for (i, item) in items.iter().enumerate() {
            // Newline separator between items (not before first)
            if i > 0 {
                if let Ok(ctx) = clipboard_rs::ClipboardContext::new() {
                    let _ = clipboard_rs::Clipboard::set_text(&ctx, "\n".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                restore_paste_target();
                paste_sync();
                std::thread::sleep(std::time::Duration::from_millis(60));
            }

            let expected = item.full_text.clone();
            crate::services::clipboard_ops::write_item_to_clipboard(item, copy_as_plain_text);

            // Verify clipboard before pasting
            if item.content_type == ContentType::Image {
                if let Ok(meta) = std::fs::metadata(&item.image_path) {
                    let size = meta.len();
                    if !crate::services::clipboard_ops::verify_clipboard_image(size, 300) {
                        log::warn!("batch_paste: image verification failed for item {}", item.id);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            } else if item.content_type != ContentType::File {
                if !crate::services::clipboard_ops::verify_clipboard_content(&expected, 300) {
                    log::warn!("batch_paste: text verification timed out for item {}", item.id);
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            restore_paste_target();

            if i < n - 1 {
                // All but last: synchronous paste, then delay
                paste_sync();
                let delay = if item.content_type == ContentType::Image {
                    let file_size = std::fs::metadata(&item.image_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    let size_delay = (file_size / 10_000) as u64;
                    size_delay.clamp(200, 3000)
                } else {
                    100
                };
                std::thread::sleep(std::time::Duration::from_millis(delay));
            } else {
                // Last item: async paste
                paste_after_delay();
            }
        }
    }
```

- [ ] **Step 5: Add required imports at top of `src/state/app.rs`**

Add to the existing imports (after line 10):

```rust
use crate::core::types::ClipboardItem;
```

(Already imported at line 10 — verify.)

- [ ] **Step 6: Build to verify compilation**

Run: `cargo build 2>&1`
Expected: Compilation succeeds.

- [ ] **Step 7: Commit**

```bash
git add src/state/app.rs
git commit -m "feat: add copy/paste action methods to AppState

Add copy_item, paste_item, paste_as_rgb, paste_as_hex, and batch_paste
methods. These delegate to clipboard_ops for write/verify and to
platform::paste for focus restore + Ctrl+V simulation.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Fill `handle_menu_action` in `ClipboardListView`

**Files:**
- Modify: `src/ui/clipboard_list.rs:229-247`

- [ ] **Step 1: Replace the `handle_menu_action` stub with dispatch logic**

Replace lines 229-247 in `src/ui/clipboard_list.rs`:

```rust
    pub(crate) fn handle_menu_action(
        &mut self,
        action: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plain = self.state.read(cx).settings.copy_as_plain_text;
        match action {
            "copy" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.copy_item(item_id, plain));
                }
            }
            "paste" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_item(item_id, plain));
                }
            }
            "paste_as_rgb" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_as_rgb(item_id));
                }
            }
            "paste_as_hex" => {
                if let Some(ref item) = self.context_menu_item {
                    let item_id = item.id;
                    self.state.update(cx, |s, _cx| s.paste_as_hex(item_id));
                }
            }
            "batch_paste" => {
                let ids = self.selected_ids.clone();
                self.state
                    .update(cx, |s, _cx| s.batch_paste(&ids, plain));
            }
            // Other actions deferred to follow-up
            "edit" | "edit_note" | "toggle_favorite" | "delete"
            | "open_image" | "paste_ocr" | "qr_detect" | "show_tag_picker"
            | "batch_favorite" | "batch_delete" => {}
            _ => {}
        }
        self.hide_context_menu(cx);
    }
```

- [ ] **Step 2: Build to verify compilation**

Run: `cargo build 2>&1`
Expected: Compilation succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat: wire up context menu copy/paste actions to AppState

Fill handle_menu_action stub with dispatch to AppState methods for
copy, paste, paste_as_rgb, paste_as_hex, and batch_paste actions.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Fill `handle_toolbar_action` in `ClipboardListView`

**Files:**
- Modify: `src/ui/clipboard_list.rs:249-264`

- [ ] **Step 1: Replace the `handle_toolbar_action` stub with dispatch logic**

Replace lines 249-264 in `src/ui/clipboard_list.rs`:

```rust
    fn handle_toolbar_action(
        &mut self,
        action: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plain = self.state.read(cx).settings.copy_as_plain_text;
        match action {
            "copy" => {
                if let Some(index) = self.hovered_index {
                    if let Some(item) = self.items.get(index) {
                        let item_id = item.id;
                        self.state.update(cx, |s, _cx| s.copy_item(item_id, plain));
                    }
                }
            }
            // Batch toolbar actions
            "batch_paste" => {
                let ids = self.selected_ids.clone();
                self.state
                    .update(cx, |s, _cx| s.batch_paste(&ids, plain));
            }
            // Other hover toolbar actions deferred to follow-up
            "open_image" | "qr_action" | "open_location" | "edit"
            | "edit_note" | "toggle_favorite" | "delete"
            | "batch_favorite" | "batch_delete" => {}
            _ => {}
        }
    }
```

- [ ] **Step 2: Build to verify compilation**

Run: `cargo build 2>&1`
Expected: Compilation succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat: wire up hover toolbar copy/paste actions to AppState

Fill handle_toolbar_action stub with dispatch to AppState for copy
and batch_paste actions.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Add `on_double_click` callback to `ClipboardCard`

**Files:**
- Modify: `src/ui/clipboard_card.rs:503-568`

- [ ] **Step 1: Add `on_double_click` field and builder method**

Add to the `ClipboardCard` struct (after `on_toolbar_action` field, line 513):

```rust
    on_double_click: Option<Rc<dyn Fn(usize, &mut Window, &mut App)>>,
```

Add to the `new()` constructor (after the `on_toolbar_action` init, line 528):

```rust
            on_double_click: None,
```

Add builder method after `on_toolbar_action` method (after line 567):

```rust
    pub fn on_double_click(
        mut self,
        handler: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    ) -> Self {
        self.on_double_click = Some(handler);
        self
    }
```

- [ ] **Step 2: Destructure `on_double_click` in `RenderOnce::render`**

In the `render` method, add `on_double_click` to the destructuring (line 572-582):

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
            on_double_click,   // <-- add this line
        } = self;
```

- [ ] **Step 3: Update the Left click handler to check `click_count`**

Replace the existing `on_mouse_down(MouseButton::Left, ...)` block (lines 628-635) with:

```rust
        // Wire click handler with double-click detection
        let base = if let Some(handler) = on_click {
            let double_click_handler = on_double_click.clone();
            base.cursor(CursorStyle::PointingHand).on_mouse_down(
                MouseButton::Left,
                move |ev, window, cx| {
                    if ev.click_count == 2 {
                        // Double-click → paste
                        if let Some(ref dbl_handler) = double_click_handler {
                            dbl_handler(index, window, cx);
                        }
                    } else {
                        // Single click → select
                        handler(index, ev.modifiers, window, cx);
                    }
                },
            )
        } else {
            base
        };
```

- [ ] **Step 4: Build to verify compilation**

Run: `cargo build 2>&1`
Expected: Compilation succeeds.

- [ ] **Step 5: Commit**

```bash
git add src/ui/clipboard_card.rs
git commit -m "feat: add on_double_click callback to ClipboardCard

Uses GPUI's native click_count field on MouseDownEvent for double-click
detection (click_count == 2). Adds builder method and wires it into
RenderOnce::render alongside the existing single-click handler.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Wire double-click paste in `ClipboardListView` render

**Files:**
- Modify: `src/ui/clipboard_list.rs:517-539` (ClipboardCard creation block)

- [ ] **Step 1: Add `on_double_click` to the ClipboardCard builder chain**

In the virtual list item render closure (around line 517-539), after the `.on_toolbar_action(...)` call, add `.on_double_click(...)`:

```rust
                                                .child(
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
                                                    )
                                                    .on_double_click({
                                                        let list_for_dbl = list_entity.clone();
                                                        Rc::new(
                                                            move |idx, _window, cx| {
                                                                let _ = list_for_dbl.update(
                                                                    cx,
                                                                    |this, cx| {
                                                                        let plain = this
                                                                            .state
                                                                            .read(cx)
                                                                            .settings
                                                                            .copy_as_plain_text;
                                                                        if this.selected_count > 1 {
                                                                            let ids = this
                                                                                .selected_ids
                                                                                .clone();
                                                                            this.state.update(
                                                                                cx,
                                                                                |s, _cx| {
                                                                                    s.batch_paste(
                                                                                        &ids, plain,
                                                                                    );
                                                                                },
                                                                            );
                                                                        } else if let Some(item) =
                                                                            this.items.get(idx)
                                                                        {
                                                                            let item_id = item.id;
                                                                            this.state.update(
                                                                                cx,
                                                                                |s, _cx| {
                                                                                    s.paste_item(
                                                                                        item_id,
                                                                                        plain,
                                                                                    );
                                                                                },
                                                                            );
                                                                        }
                                                                    },
                                                                );
                                                            },
                                                        )
                                                    })
                                                    .into_any_element(),
                                                )
```

- [ ] **Step 2: Build to verify compilation**

Run: `cargo build 2>&1`
Expected: Compilation succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/ui/clipboard_list.rs
git commit -m "feat: wire double-click paste in ClipboardListView

Adds on_double_click handler to ClipboardCard creation in the virtual
list. Single-select double-click triggers paste_item; multi-select
double-click triggers batch_paste.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Final integration build

**Files:** None (verification only)

- [ ] **Step 1: Full build**

Run: `cargo build 2>&1`
Expected: Full compilation succeeds with zero errors and zero warnings.

- [ ] **Step 2: Check for clippy warnings**

Run: `cargo clippy 2>&1`
Expected: No new warnings introduced.

- [ ] **Step 3: Verify Slint backward compatibility**

Confirm the Slint `app.rs` still compiles and the old path still works by checking that no Slint callback signatures were broken.

Run: `cargo build 2>&1`
Expected: No errors from `src/app.rs`.

---

## Verification Checklist

After implementation, manually test:

- [ ] Hover toolbar "Copy" button copies item content to clipboard
- [ ] Context menu "Copy" copies item to clipboard
- [ ] Context menu "Paste" writes item to clipboard and simulates Ctrl+V
- [ ] Context menu "Paste as RGB" converts HEX color and pastes
- [ ] Context menu "Paste as HEX" converts RGB color and pastes
- [ ] Double-click on a card pastes the item (single-select)
- [ ] Double-click with multiple items selected does batch paste
- [ ] Batch paste context menu item works with selected count > 1
- [ ] Slint frontend (`app.rs`) still functions correctly after refactoring
