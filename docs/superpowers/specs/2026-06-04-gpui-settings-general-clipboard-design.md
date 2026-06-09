# Design: GPUI Settings — General & Clipboard Tabs

Date: 2026-06-04
Branch: `experiment/gpui-migration`
Status: Approved

## Scope

Migrate the General and Clipboard settings tab content from Slint stubs to
functional GPUI controls. Wire all settings mutations to `AppState` for
immediate effect + disk persistence.

**Out of scope**: Hotkey, Data, Sync tabs (remain stubs). Language selector
(UI rendered, callback wired as no-op — pending GPUI i18n).

## File Changes

```
src/ui/settings/mod.rs       — Keep nav bar + tab bar shell; delegate tab
                                 rendering to sub-modules; accept
                                 Entity<AppState>.
src/ui/settings/general.rs   — NEW: 5 active settings + 1 language placeholder.
src/ui/settings/clipboard.rs — NEW: 8 active settings.
src/main.rs                  — FIX: calculate initial window position from
                                 settings instead of hardcoded (100,100).
```

## Architecture

### State Access

`SettingsPanel` currently loads its own `AppSettings` copy via
`AppSettings::load()`. This is wrong — mutations only hit disk, not the
live `AppState`, so theme/position changes never take effect.

**Change**: `SettingsPanel::new(state: Entity<AppState>, cx)` replaces
`SettingsPanel::new(cx)`. All reads go through `state.read(cx).settings`,
all writes through `state.update(cx, |s,_cx| { s.settings.xxx = y; s.settings.save() })`.

RootView already holds `Entity<AppState>`; it passes it to SettingsPanel:
```rust
let settings_panel = cx.new(|cx| SettingsPanel::new(state.clone(), cx));
```

### SettingsEvent Extension

```rust
pub enum SettingsEvent {
    Back,
    ThemeChanged,  // NEW — RootView rebuilds ClippiTheme and notifies children
}
```

### Reusable Control Helpers

Two private methods on `SettingsPanel` factor out repeated patterns:

1. **`setting_row_with_toggle(label, desc, value, on_toggle)`**
   - 66px row: surface bg, 10px radius, 1px divider border
   - Left: label (12px/600) + desc (10px/text_3), 2px gap
   - Right: toggle switch (40×22px, 11px radius)
     - On: accent bg + white 18px circle at right
     - Off: divider bg + white 18px circle at left

2. **`setting_row_with_options(label, desc, options, active_key, on_select)`**
   - Same 66px row layout
   - Right: horizontal pill group, each 26px tall, 7px radius
     - Selected: accent bg, no border, white 11px/600 text
     - Unselected: transparent bg, 1px divider border, text_2 11px/400 text

### Tab Contents

**General Tab** (`settings/general.rs`):

| Setting | Control | Field | Notes |
|---------|---------|-------|-------|
| Language | Options (中文/English) | `language` | No-op on click; TODO for i18n |
| Auto-start | Toggle | `auto_start` | + calls `set_auto_start()` |
| Auto-hide | Toggle | `auto_hide` | Also calls `WindowManager::set_auto_hide()` |
| Silent start | Toggle | `silent_start` | — |
| Theme | Options (Auto/Dark/Light) | `theme` | Emits `ThemeChanged` |
| Position | Options (Center/Follow/Pin) | `window_position_mode` | Also calls `WindowManager::set_position_mode()` |

**Clipboard Tab** (`settings/clipboard.rs`):

| Setting | Control | Field | Notes |
|---------|---------|-------|-------|
| Sort by created | Toggle | `sort_by_created` | Desc toggles "First created" / "Last modified" |
| Card height | Options (Tall/Med/Short/Auto) | `card_height_mode` | 4 options |
| Show source app | Toggle | `show_source_app` | — |
| Scroll to top | Toggle | `auto_scroll_to_top` | — |
| Copy as plain text | Toggle | `copy_as_plain_text` | — |
| Show original on hover | Toggle | `show_original_on_hover` | — |
| Auto OCR | Toggle | `ocr_enabled` | — |
| Auto QR | Toggle | `qr_enabled` | — |

## Bug Fix: Initial Window Position

**Problem** (`main.rs:77-78`): Window created at hardcoded `(100, 100)`,
ignoring `window_position_mode`.

**Fix**: Before `cx.open_window()`, load settings, calculate position:
- Center → center on cursor's monitor
- FollowMouse → offset from cursor by PANEL_OFFSET_X
- Remember → `saved_window_x/y` (fallback to center if unset/offscreen)

Use existing `WindowManager::calculate_position()` logic. Extract position
calculation to a free function in `core/frontend.rs` so both `main.rs` and
`WindowManager` can reuse it.

```rust
// core/frontend.rs — new free function
pub fn calculate_initial_position(settings: &AppSettings) -> Option<Point<Pixels>> {
    let mode = PositionMode::from_str(&settings.window_position_mode);
    let win_w = settings.saved_window_width.max(DEFAULT_WINDOW_WIDTH) as i32;
    let win_h = settings.saved_window_height.max(DEFAULT_WINDOW_HEIGHT) as i32;
    // ... use monitor helpers, mirroring WindowManager::calculate_position
}
```

`main.rs` then uses this:
```rust
let pos = calculate_initial_position(&settings)
    .unwrap_or(point(px(100.), px(100.)));
```

## Scroll Support

Tab content area uses a scrollable container so all settings rows are
accessible even when they exceed the panel height.

## Testing

- Manual verification: toggle each setting, confirm it persists across
  app restart
- Theme change: verify immediate visual change in both main and settings views
- Window position: verify window appears at correct position on startup
  for all three modes
