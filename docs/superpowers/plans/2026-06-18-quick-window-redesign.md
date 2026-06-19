# Quick Window Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign the quick paste popup with themed styling, type filter bar, pinned tag row, and relative time display — sharing AppState with the main window.

**Architecture:** Rewrite `QuickPasteView` to read from shared `Entity<AppState>` instead of holding its own items. Add filter/tag bars that mirror the main window's filter state (same `ClipboardFilters` + `type_filter_config` + `pinned_tag_ids`). Keep existing no-focus window creation and keyboard/mouse interaction logic.

**Tech Stack:** Rust, GPUI, ClippiTheme

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/core/settings.rs` | Modify 1 line | Change default `quick_hotkey` to `Alt+C` |
| `src/ui/quick_paste.rs` | **Rewrite** | New `QuickPasteView` with theme, filter bars, time |
| `src/ui/window_manager.rs` | Modify ~15 lines | Simplify `show_quick_window`, fix `selected_item_id(cx)` |

No new files. No test files (UI-only change; existing navigation logic preserved).

---

### Task 1: Change default quick hotkey

**Files:** Modify `src/core/settings.rs:147-149`

- [ ] **Step 1: Change default**

Replace:

```rust
fn default_quick_hotkey() -> String {
    "Alt+Shift+V".to_string()
}
```

With:

```rust
fn default_quick_hotkey() -> String {
    "Alt+C".to_string()
}
```

- [ ] **Step 2: Build check**

```bash
cargo check 2>&1 | head -10
```

- [ ] **Step 3: Commit**

```bash
git add src/core/settings.rs
git commit -m "chore: change default quick hotkey to Alt+C

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Rewrite QuickPasteView

**Files:** Rewrite `src/ui/quick_paste.rs`

- [ ] **Step 1: Write the new file**

Replace the entire content of `src/ui/quick_paste.rs` with:

```rust
//! Quick paste popup — compact, non-focus clipboard candidate list.

use gpui::*;

use crate::core::i18n_keys::I18nKey;
use crate::core::settings::TypeFilterEntry;
use crate::core::types::{format_relative_time, ClipboardItem, ContentType, DisplayKind, FileData};
use crate::state::app::AppState;
use crate::ui::search_bar::filter_type_display;
use crate::ui::theme::ClippiTheme;

const VISIBLE_ROWS: usize = 5;
const ROW_HEIGHT: f32 = 44.0;
pub const QUICK_WINDOW_WIDTH: f32 = 430.0;
// Height: 5 rows (220) + type bar (30) + tag bar (26) + dividers (2) + outer padding (16) = 294
pub const QUICK_WINDOW_HEIGHT: f32 = 294.0;

const TYPE_BAR_HEIGHT: f32 = 30.0;
const TAG_ROW_HEIGHT: f32 = 26.0;
const TYPE_FILTER_BUTTON_SIZE: f32 = 22.0;
const OUTER_PADDING: f32 = 8.0;
const HORIZONTAL_PADDING: f32 = 10.0;

pub enum QuickPasteEvent {
    Paste(i64),
}

pub struct QuickPasteView {
    state: Entity<AppState>,
    selected_index: usize,
    first_visible: usize,
}

impl EventEmitter<QuickPasteEvent> for QuickPasteView {}

impl QuickPasteView {
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            selected_index: 0,
            first_visible: 0,
        }
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if len == 0 {
            return;
        }
        self.selected_index = (self.selected_index + 1).min(len - 1);
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        let len = self.state.read(cx).items.len();
        if len == 0 {
            return;
        }
        self.selected_index = self.selected_index.saturating_sub(1);
        self.ensure_selected_visible();
        cx.notify();
    }

    pub fn select_visible_slot(&mut self, slot: usize, cx: &mut Context<Self>) -> Option<i64> {
        let index = self.first_visible + slot;
        if index >= self.state.read(cx).items.len() {
            return None;
        }
        self.selected_index = index;
        self.ensure_selected_visible();
        cx.notify();
        self.selected_item_id(cx)
    }

    pub fn selected_item_id(&self, cx: &Context<Self>) -> Option<i64> {
        self.state
            .read(cx)
            .items
            .get(self.selected_index)
            .map(|item| item.id)
    }

    fn ensure_selected_visible(&mut self) {
        if self.selected_index < self.first_visible {
            self.first_visible = self.selected_index;
        }
        if self.selected_index >= self.first_visible + VISIBLE_ROWS {
            self.first_visible = self.selected_index + 1 - VISIBLE_ROWS;
        }
    }

    fn visible_items(&self, cx: &Context<Self>) -> Vec<(usize, ClipboardItem)> {
        self.state
            .read(cx)
            .items
            .iter()
            .skip(self.first_visible)
            .take(VISIBLE_ROWS)
            .enumerate()
            .map(|(slot, item)| (slot, item.clone()))
            .collect()
    }

    fn theme(&self, cx: &Context<Self>) -> ClippiTheme {
        let is_dark = self.state.read(cx).settings.theme != "light";
        if is_dark {
            ClippiTheme::dark()
        } else {
            ClippiTheme::light()
        }
    }
}

impl Render for QuickPasteView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme(cx);

        let state = self.state.read(cx);
        let type_config: Vec<TypeFilterEntry> = state.settings.type_filter_config.clone();
        let pinned_tag_ids: Vec<i64> = state.settings.pinned_tag_ids.clone();
        let filters = state.filters.clone();
        let tags_snapshot = state.tags.clone();
        let items_count = state.items.len();
        drop(state);

        // Resolve pinned tags from settings (only show if they exist in DB)
        let pinned_tags: Vec<(i64, String, String)> = pinned_tag_ids
            .iter()
            .filter_map(|&id| {
                tags_snapshot
                    .iter()
                    .find(|t| t.id == id)
                    .map(|t| (t.id, t.name.clone(), t.color.clone()))
            })
            .collect();

        let has_type_bar = !type_config.is_empty();
        let has_tag_row = !pinned_tags.is_empty();

        div()
            .size_full()
            .p(px(OUTER_PADDING))
            .child(
                div()
                    .size_full()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.divider)
                    .bg(theme.bg)
                    .shadow_lg()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    // ── Type filter bar ──
                    .when(has_type_bar, |parent| {
                        parent.child(
                            div()
                                .h(px(TYPE_BAR_HEIGHT))
                                .px(px(HORIZONTAL_PADDING))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .children(type_config.iter().filter(|e| e.visible).map(|entry| {
                                    let active = filters.is_type_active(&entry.key);
                                    let (icon, _label) = filter_type_display(&entry.key)
                                        .unwrap_or(("\u{e60e}", "".into()));
                                    let key = entry.key.clone();
                                    let t = theme.clone();
                                    let app_state = self.state.clone();
                                    div()
                                        .w(px(TYPE_FILTER_BUTTON_SIZE))
                                        .h(px(TYPE_FILTER_BUTTON_SIZE))
                                        .rounded(px(5.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(if active { t.accent } else { rgba(0x00000000) })
                                        .text_color(if active { rgb(0xffffff) } else { t.text_2 })
                                        .text_size(px(12.0))
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, {
                                            let s = app_state.clone();
                                            let k = key.clone();
                                            move |_, _window, cx| {
                                                s.update(cx, |s, _cx| {
                                                    s.toggle_type_filter(&k);
                                                });
                                            }
                                        })
                                        .child(icon)
                                        .into_any_element()
                                })),
                        )
                    })
                    .when(has_type_bar, |parent| {
                        parent.child(div().h(px(1.0)).w_full().bg(theme.divider))
                    })
                    // ── Pinned tag row ──
                    .when(has_tag_row, |parent| {
                        parent.child(
                            div()
                                .h(px(TAG_ROW_HEIGHT))
                                .px(px(HORIZONTAL_PADDING))
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .children(pinned_tags.iter().map(|(id, name, color_hex)| {
                                    let active = filters.tag_ids.contains(id);
                                    let tag_color = parse_hex_for_tag(color_hex);
                                    let tag_id = *id;
                                    let app_state = self.state.clone();
                                    div()
                                        .h(px(20.0))
                                        .px(px(6.0))
                                        .rounded(px(4.0))
                                        .flex()
                                        .items_center()
                                        .text_size(px(10.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .bg(if active { theme.accent } else { tag_color })
                                        .text_color(rgb(0xffffff))
                                        .cursor(CursorStyle::PointingHand)
                                        .on_mouse_down(MouseButton::Left, {
                                            let s = app_state.clone();
                                            move |_, _window, cx| {
                                                s.update(cx, |s, _cx| {
                                                    s.toggle_tag_filter(tag_id);
                                                });
                                            }
                                        })
                                        .child(name.clone())
                                        .into_any_element()
                                })),
                        )
                    })
                    .when(has_tag_row, |parent| {
                        parent.child(div().h(px(1.0)).w_full().bg(theme.divider))
                    })
                    // ── Empty state ──
                    .when(items_count == 0, |parent| {
                        parent.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(13.0))
                                .text_color(theme.text_2)
                                .child("No clipboard items"),
                        )
                    })
                    // ── List rows ──
                    .children({
                        let t = theme.clone();
                        let selected_index = self.selected_index;
                        let view_entity = cx.entity();
                        self.visible_items(cx)
                            .into_iter()
                            .map(move |(slot, item)| {
                                let index = self.first_visible + slot;
                                let selected = index == selected_index;
                                let item_id = item.id;
                                let icon = type_icon(&item);
                                let preview = preview_text(&item);
                                let time = format_relative_time(item.updated_at, None);
                                let t = t.clone();
                                let ve = view_entity.clone();

                                div()
                                    .h(px(ROW_HEIGHT))
                                    .px(px(HORIZONTAL_PADDING))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .bg(if selected { t.accent_overlay() } else { rgba(0x00000000) })
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let ve = ve.clone();
                                        move |ev, _window, cx| {
                                            if ev.click_count == 2 {
                                                ve.update(cx, |_, cx| {
                                                    cx.emit(QuickPasteEvent::Paste(item_id));
                                                });
                                            }
                                        }
                                    })
                                    // Slot number badge
                                    .child(
                                        div()
                                            .w(px(22.0))
                                            .h(px(22.0))
                                            .rounded(px(5.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(if selected { t.accent } else { rgba(0x00000000) })
                                            .text_color(if selected { rgb(0xffffff) } else { t.text_2 })
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::BOLD)
                                            .child((slot + 1).to_string()),
                                    )
                                    // Type icon
                                    .child(
                                        div()
                                            .w(px(16.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(12.0))
                                            .text_color(if selected { t.accent } else { t.text_2 })
                                            .child(icon),
                                    )
                                    // Content preview
                                    .child(
                                        div()
                                            .flex_1()
                                            .overflow_hidden()
                                            .text_size(px(13.0))
                                            .text_color(t.text_1)
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(preview),
                                    )
                                    // Relative time
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(t.text_3)
                                            .whitespace_nowrap()
                                            .child(time),
                                    )
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
            )
    }
}

// ── Helpers (adapted from clipboard_card.rs) ──

fn type_icon(item: &ClipboardItem) -> &'static str {
    if item.meta_type == "email" {
        return "\u{e604}";
    }
    if item.meta_type == "phone" {
        return "\u{e966}";
    }
    // QR code detected in image
    if item.content_type == ContentType::Image
        && crate::core::types::RichData::from_json(&item.rich_data)
            .qr_text
            .is_some()
    {
        return "\u{e605}";
    }
    match item.display_kind() {
        DisplayKind::PlainText => "\u{e60e}",
        DisplayKind::Html | DisplayKind::Markdown | DisplayKind::Rtf => "\u{e6ae}",
        DisplayKind::Image => "\u{e626}",
        DisplayKind::File => "\u{e68a}",
        DisplayKind::Link => "\u{e6d7}",
        DisplayKind::Path => "\u{e60f}",
        DisplayKind::Color => "\u{e610}",
        DisplayKind::Email => "\u{e604}",
        DisplayKind::Phone => "\u{e966}",
    }
}

fn preview_text(item: &ClipboardItem) -> String {
    let raw = match item.content_type {
        ContentType::Image => {
            if item.image_width > 0 && item.image_height > 0 {
                format!("Image {}×{}", item.image_width, item.image_height)
            } else {
                "Image".to_string()
            }
        }
        ContentType::File => {
            let data = FileData::from_json(&item.file_data);
            if data.files.is_empty() {
                item.full_text.clone()
            } else if data.files.len() == 1 {
                data.files[0].name.clone()
            } else {
                format!("{} files", data.files.len())
            }
        }
        _ => item.full_text.clone(),
    };
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_hex_for_tag(hex: &str) -> Rgba {
    use crate::core::types::parse_hex_color;
    parse_hex_color(hex)
        .map(|(r, g, b)| rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32))
        .unwrap_or(rgb(0x3b82f6))
}
```

- [ ] **Step 2: Build check and fix errors**

```bash
cargo check 2>&1
```

Key things to verify against compiler:
- `filter_type_display` is `pub(crate)` in `search_bar.rs` — accessible from `quick_paste.rs`
- `TypeFilterEntry` is `pub` in `settings.rs` — accessible
- `ClipboardItem.clone()` works — it doesn't implement `Clone` in the struct definition. If compiler errors on `.clone()`, change `visible_items` to return `Vec<(usize, &ClipboardItem)>` and adjust render accordingly — or add `#[derive(Clone)]` to `ClipboardItem` in `types.rs`
- `format_relative_time(item.updated_at, None)` — verify the second parameter type
- `gpui::*` imports `rgb`, `rgba`, `Rgba`, `FontWeight`, `CursorStyle`, `MouseButton`
- `text_ellipsis()` — verify this method exists on `Div`

Fix any compile errors, then proceed.

- [ ] **Step 3: Commit**

```bash
git add src/ui/quick_paste.rs
git commit -m "feat: redesign quick paste view with theme, filters, and tags

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Update WindowManager for new QuickPasteView API

**Files:** Modify `src/ui/window_manager.rs`

- [ ] **Step 1: Simplify `show_quick_window`**

Remove the `reload_items()` + `set_items()` pattern. In `fn show_quick_window`, replace:

```rust
        self.state.update(cx, |state, _cx| state.reload_items());
        let items = self.state.read(cx).items.clone();
        view.update(cx, |view, cx| view.set_items(items, cx));
```

With a single notify call — the view reads from shared state in render:

```rust
        // QuickPasteView reads from shared AppState — just notify to re-render.
        view.update(cx, |_, cx| cx.notify());
```

- [ ] **Step 2: Fix `selected_item_id(cx)` call**

In `handle_quick_action`, the `QuickAction::Paste` arm. Replace:

```rust
                let id = view.read(cx).selected_item_id();
```

With:

```rust
                let id = view.read(cx).selected_item_id(cx);
```

- [ ] **Step 3: Build check**

```bash
cargo check 2>&1 | head -20
```

- [ ] **Step 4: Commit**

```bash
git add src/ui/window_manager.rs
git commit -m "refactor: simplify quick window to use shared state

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: Verify and finalize

**Files:** None (verification only)

- [ ] **Step 1: Full build**

```bash
cargo build 2>&1
```

Expected: Compiles successfully, zero warnings.

- [ ] **Step 2: Check diff is clean**

```bash
git diff --stat
```

Expected: Only changes in `settings.rs`, `quick_paste.rs`, `window_manager.rs`.

- [ ] **Step 3: Commit any remaining tweaks**

```bash
git add -A
git commit -m "chore: final adjustments for quick window redesign

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Notes for Implementation

- `toggle_type_filter()` and `toggle_tag_filter()` internally call `reload_items()` — no need to call separately
- In mouse handlers, use `app_state.update(cx, ...)` where `cx` is the window/App context from the GPUI callback (not the view context)
- Theme is built from `state.settings.theme != "light"` check — matches existing quick_paste.rs behavior ("system" and "dark" both produce dark theme)
- The `Clone` derive on `ClipboardItem` may be needed — if not present, add `#[derive(Clone)]` or use references in `visible_items()`
