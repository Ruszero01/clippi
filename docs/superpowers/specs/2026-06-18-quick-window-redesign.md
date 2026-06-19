# Quick Window Redesign

## Summary

Redesign the quick paste popup (`QuickPasteView`) to match the main window's visual style, add type/tag filter bars, and share AppState with the main window.

## Current State (feat/quick-window branch)

- `quick_paste.rs`: Minimal 5-row list with hardcoded colors, no filters, no tags
- `text_input.rs`: Text input caret position detection (Win/Mac)
- `window_manager.rs`: Quick window lifecycle (show/hide/position), hotkey dispatch for Alt+C, QuickAction handling
- `main.rs`: Second `cx.open_window()` with `WindowKind::PopUp`, `focus: false`
- `settings.rs`: `quick_hotkey` field (default "Alt+Shift+V")
- `hotkey.rs`: Dual hotkey support via `HotkeyEvent` enum + `QuickAction`

## What Stays (preserved from existing)

- `WS_EX_NOACTIVATE` / `SWP_NOACTIVATE` on Windows — window shows without stealing focus
- `NSFloatingWindowLevel` on macOS — same behavior
- `WindowKind::PopUp` — no title bar, no taskbar entry
- `show: false`, `focus: false` in `WindowOptions`
- Text input anchor positioning (caret-aware, fallback to cursor)
- Keyboard/mouse interaction: Up/Down arrows, Enter to paste, Esc to close, scroll wheel, double-click to paste
- Digit keys 1-5 for quick pick
- Complimentary mode: opening one window hides the other

## What Changes

### Architecture

- `QuickPasteView` shares the **same** `Entity<AppState>` as the main window
- No independent items list — reads `state.items` directly (filtered by shared `state.filters`)
- No `set_items()` needed — `cx.notify()` on filter changes triggers re-render

### UI Layout (top to bottom)

```
┌───────────────────────────────────────┐
│  📝  📰  🖼  📁  🔗  📂  🎨  📞     │  ← Type filter bar (icon-only, 24px)
│  🔖 TagA  TagB  TagC                 │  ← Pinned tag chips (clickable, 22px)
├───────────────────────────────────────┤
│  1  [📝]  clipboard preview text...   3分钟前 │
│  2  [🖼]  image preview text...       1小时前 │
│  3  [🔗]  https://example.com...      2小时前 │  ← 5 visible rows, 44px each
│  4  [📁]  C:\Users\...\file.txt       昨天   │
│  5  [🎨]  #3B82F6                     3天前  │
└───────────────────────────────────────┘
```

**Type filter bar (top)**:
- Icon-only buttons from `AppSettings.type_filter_config` (same config as main window search bar)
- Only visible entries shown, in user-defined order
- Active state: accent color fill + white icon
- Inactive state: muted icon on transparent bg
- Hover: slight background highlight

**Pinned tag row**:
- Shows tags from `AppSettings.pinned_tag_ids`
- Click to toggle filter (adds/removes tag from `state.filters.tag_ids`)
- Active: accent background + white text
- Inactive: tag color background + white text

**List rows** (44px height):
- Slot number badge (1-5, accent bg when selected)
- Type icon (small, muted)
- Content preview (single line, ellipsis overflow, flex-1)
- Relative time (muted, right-aligned) — `format_relative_time()` from `types.rs`
- Selected row: subtle accent background

**No footer** — no position counter, no status bar.

### Theme

Use `ClippiTheme` from `src/ui/theme.rs`:
- Background: `theme.bg`
- Border: `theme.border` or `theme.divider`
- Text: `theme.text`
- Muted/secondary: `theme.muted`
- Accent: `theme.accent` (blue)
- Selected background: `theme.surface` or lighter accent tint

### Window Dimensions

- Width: 430px (unchanged)
- Height: ~336px (5 rows × 44px + filter bar 38px + tag row 30px + padding)

### Filter Behavior

- Quick window and main window share the SAME filter state
- Toggling a type filter in quick window also affects main window (and vice versa)
- Pinned tags are read-only from settings; tag filter toggle is reflected in shared state
- Items reload from DB when filters change

### Hotkey Default

Change `default_quick_hotkey()` from `"Alt+Shift+V"` to `"Alt+C"`.

## Implementation Plan

### Files to modify

| File | Changes |
|------|---------|
| `src/core/settings.rs` | Change default quick_hotkey to `Alt+C` |
| `src/ui/quick_paste.rs` | **Rewrite** — new render with theme, filter bars, tag row |
| `src/ui/window_manager.rs` | Minor: remove `set_items()` call, notify-only on show; remove position counter logic |

### Implementation order

1. Change default hotkey to Alt+C
2. Rewrite `QuickPasteView`:
   - Remove `items: Vec<ClipboardItem>` field — read from `state.items`
   - Add type filter bar rendering using `state.filters` + `type_filter_config`
   - Add pinned tag row rendering using `pinned_tag_ids`
   - Style with `ClippiTheme`
   - Simplify preview text
3. Adjust window height constant
4. Update `show_quick_window()` — remove `reload_items()` + `set_items()`, just `cx.notify()` on the view
5. Test build
