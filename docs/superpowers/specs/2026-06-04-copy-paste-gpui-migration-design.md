# Copy/Paste Feature Migration (Slint → GPUI)

**Date**: 2026-06-04
**Branch**: `experiment/gpui-migration`
**Status**: Design Approved

## Overview

Wire up the copy/paste functionality in the GPUI UI layer by connecting the already-migrated `HoverToolbar` and `ContextMenu` components to actual clipboard operations, and add double-click paste support to `ClipboardCard`.

## Scope

| Feature | UI Component | Action |
|---------|-------------|--------|
| Hover toolbar "Copy" button | `HoverToolbar` | Copy item content to clipboard |
| Context menu "Copy" / "Paste" | `ContextMenu` | Copy to clipboard / Copy + simulate Ctrl+V |
| Context menu "Paste as RGB/HEX" | `ContextMenu` | Color format conversion + paste |
| Context menu "Paste N items" (batch) | `ContextMenu` | Batch paste with newline separators |
| Double-click card | `ClipboardCard` | Paste (same as single paste) |

Non-scope (already wired in Slint, deferred to follow-up):
- Edit, Note, Favorite, Delete, Open Image, QR Code, Tag Picker — these already have action dispatch setup; backend wiring deferred.

## Architecture

### Data Flow

```
User Action          UI Component            ClipboardListView          AppState
───────────          ────────────            ────────────────          ────────
Double-click card →  ClipboardCard          → on_double_click         → paste_item(id)
                      (click_count==2)                                    → write to clipboard
                                                                          → restore target focus
                                                                          → paste_after_delay()

Hover "Copy" btn  →  HoverToolbar           → handle_toolbar_action   → copy_item(id)
                      (on_action="copy")                                  → write to clipboard

Menu "Copy" click →  ContextMenu            → handle_menu_action      → copy_item(id)
                      (on_action="copy")                                  → write to clipboard

Menu "Paste" click → ContextMenu            → handle_menu_action      → paste_item(id)
                      (on_action="paste")                                 → write + simulate Ctrl+V

Menu "Paste as X"  → ContextMenu            → handle_menu_action      → paste_as_rgb/hex(id)
                      (on_action="paste_as_*")                            → format convert + paste

Double-click batch → ClipboardCard          → on_double_click         → batch_paste(ids)
                      (multi-select)                                      → for each: write + sync paste
```

### Principle
- **AppState** owns `Database` and provides data-access methods (read item, write to clipboard, trigger paste).
- **ClipboardListView** receives UI events, dispatches to `AppState` methods.
- **HoverToolbar / ContextMenu / ClipboardCard** remain pure UI — they only emit action strings, no business logic.

### Which files change

| File | Change |
|------|--------|
| `src/services/clipboard_ops.rs` | **NEW** — extract `write_item_to_clipboard` from Slint `app.rs` |
| `src/state/app.rs` | **NEW methods** — `copy_item`, `paste_item`, `paste_as_rgb`, `paste_as_hex`, `batch_paste` |
| `src/ui/clipboard_list.rs` | Fill `handle_menu_action` + `handle_toolbar_action` stubs |
| `src/ui/clipboard_card.rs` | Add double-click detection via `click_count`, wire `on_double_click` |

No changes needed:
- `src/ui/hover_toolbar.rs` — already emits action strings correctly
- `src/ui/context_menu.rs` — already emits action strings correctly
- `src/platform/paste.rs` — `restore_paste_target()`, `paste_after_delay()`, `paste_sync()` are ready

## Detailed Design

### 1. `src/services/clipboard_ops.rs` (new file)

Extract `write_item_to_clipboard` (currently at `app.rs:1925-2004`) into a standalone public function:

```rust
/// Write a ClipboardItem to the system clipboard.
/// Handles image (PNG), file (CF_HDROP), plain text, and rich text (HTML+RTF).
/// The `copy_as_plain_text` flag discards rich formatting.
pub fn write_item_to_clipboard(item: &ClipboardItem, copy_as_plain_text: bool);
```

The function is self-contained: it uses `clipboard_rs::ClipboardContext` directly with no dependency on `ClipboardShared`. This makes it callable from both GPUI (`AppState`) and Slint (`app.rs`) paths.

### 2. `AppState` methods (`src/state/app.rs`)

Add five public methods. All are synchronous and called from the GPUI main thread:

```rust
impl AppState {
    /// Copy item content to system clipboard (no paste).
    pub fn copy_item(&self, id: i64, copy_as_plain_text: bool);

    /// Write item to clipboard, restore previous window focus, simulate Ctrl+V.
    pub fn paste_item(&self, id: i64, copy_as_plain_text: bool);

    /// Paste color as RGB (convert from HEX).
    pub fn paste_as_rgb(&self, id: i64);

    /// Paste color as HEX (convert from RGB).
    pub fn paste_as_hex(&self, id: i64);

    /// Batch paste: write each item to clipboard then Ctrl+V, with newline
    /// separators between items via synchronous paste to avoid race conditions.
    pub fn batch_paste(&self, ids: &[i64], copy_as_plain_text: bool);
}
```

`paste_item` flow:
1. `db.get_by_id(id)` → get item
2. `clipboard_ops::write_item_to_clipboard(&item, copy_as_plain_text)`
3. For text/link/color: `verify_clipboard_content()` (also extracted from `app.rs`)
4. `restore_paste_target()`
5. `paste_after_delay()` (spawns a thread, returns immediately)

`batch_paste` flow:
1. For each item (except last): write to clipboard → `paste_sync()` (blocks)
2. For last item: write to clipboard → `paste_after_delay()` (async)
3. Between items: write newline separator to clipboard → `paste_sync()`

### 3. `ClipboardListView` action handling (`src/ui/clipboard_list.rs`)

Fill the existing stub methods to dispatch to `AppState`:

```rust
fn handle_menu_action(&mut self, action: &str, cx: &mut Context<Self>) {
    let plain = self.state.read(cx).settings.copy_as_plain_text;
    match action {
        "copy" => {
            if let Some(item) = &self.context_menu_item {
                self.state.update(cx, |s, _cx| s.copy_item(item.id, plain));
            }
        }
        "paste" => {
            if let Some(item) = &self.context_menu_item {
                self.state.update(cx, |s, _cx| s.paste_item(item.id, plain));
            }
        }
        "paste_as_rgb" => {
            if let Some(item) = &self.context_menu_item {
                self.state.update(cx, |s, _cx| s.paste_as_rgb(item.id));
            }
        }
        "paste_as_hex" => {
            if let Some(item) = &self.context_menu_item {
                self.state.update(cx, |s, _cx| s.paste_as_hex(item.id));
            }
        }
        "batch_paste" => {
            let ids = self.selected_ids.clone();
            self.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
        }
        // Other actions (edit, delete, etc.) remain no-op for now
        _ => {}
    }
    self.hide_context_menu(cx);
}

fn handle_toolbar_action(&mut self, action: &str, cx: &mut Context<Self>) {
    let plain = self.state.read(cx).settings.copy_as_plain_text;
    match action {
        "copy" => {
            if let Some(index) = self.hovered_index {
                if let Some(item) = self.items.get(index) {
                    self.state.update(cx, |s, _cx| s.copy_item(item.id, plain));
                }
            }
        }
        // Other hover toolbar actions remain no-op for now
        _ => {}
    }
}
```

### 4. `ClipboardCard` double-click (`src/ui/clipboard_card.rs`)

Add `on_double_click` callback to `ClipboardCard` struct. In the existing `on_mouse_down(MouseButton::Left, ...)` handler, check `ev.click_count`:

```rust
// In clipboard_card.rs render:
let base = if let Some(handler) = on_click {
    let double_click_handler = on_double_click.clone();
    base.cursor(CursorStyle::PointingHand).on_mouse_down(
        MouseButton::Left,
        move |ev, window, cx| {
            if ev.click_count == 2 {
                // Double-click → paste
                if let Some(ref handler) = double_click_handler {
                    handler(index, window, cx);
                }
            } else {
                // Single click → select
                handler(index, ev.modifiers, window, cx);
            }
        },
    )
} else { base };
```

In `clipboard_list.rs` render, wire `on_double_click` to dispatch paste:

```rust
// When creating ClipboardCard in the virtual list:
.on_double_click({
    let list = list_entity.clone();
    let state = state_entity.clone();
    move |idx, _, cx| {
        let _ = list.update(cx, |this, cx| {
            if this.selected_count > 1 {
                let ids = this.selected_ids.clone();
                let plain = this.state.read(cx).settings.copy_as_plain_text;
                this.state.update(cx, |s, _cx| s.batch_paste(&ids, plain));
            } else if let Some(item) = this.items.get(idx) {
                let plain = this.state.read(cx).settings.copy_as_plain_text;
                let item_id = item.id;
                this.state.update(cx, |s, _cx| s.paste_item(item_id, plain));
            }
        });
    }
})
```

### 5. Extracted utilities

Also move `verify_clipboard_content` from `app.rs:2097` to `clipboard_ops.rs`:

```rust
/// Verify the clipboard contains expected text within timeout (ms).
/// Returns true on match, false on timeout.
pub fn verify_clipboard_content(expected: &str, timeout_ms: u64) -> bool;
```

## Error Handling

- All clipboard operations are best-effort: `eprintln!` on failure, never panic.
- `write_item_to_clipboard` returns silently if `ClipboardContext::new()` fails.
- `verify_clipboard_content` has a 200ms timeout; paste proceeds regardless.

## Testing

Manual verification checklist:
- [ ] Hover toolbar "Copy" button copies text to clipboard
- [ ] Context menu "Copy" copies item to clipboard
- [ ] Context menu "Paste" pastes item into target app
- [ ] Double-click on card pastes item
- [ ] Double-click with multi-select does batch paste
- [ ] Color item "Paste as RGB" / "Paste as HEX" works
- [ ] Slint build still compiles (write_item_to_clipboard is shared, not removed)
