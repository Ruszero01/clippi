# i18n GPUI Migration Design

**Date:** 2026-06-08
**Branch:** experiment/gpui-migration
**Status:** Approved

## Overview

Rewrite the Slint-era `i18n::tr("中文", "English")` inline-translation system into a structured, compile-time-safe key-based system using a `define_i18n!` macro. Add language selection to the settings UI (General tab), and apply i18n to all UI code that currently has hardcoded English strings.

## Motivation

- Current `tr()` pattern works but mixes data with logic; a macro-generated `I18nKey` enum provides compile-time key validation
- Most GPUI UI code (`general.rs`, `clipboard.rs`, `hotkey.rs`, `sync.rs`, `sidebar.rs`, etc.) uses hardcoded English labels — no i18n at all
- The settings page has no language dropdown, despite `AppSettings.language` field already existing
- Slint-era tray menu required app restart for language switch; GPUI's `muda`-backed `tray-icon` supports live `set_text()` — no restart needed

## Design

### File Layout

```
src/core/
├── i18n.rs          # Engine: AtomicBool state + define_i18n! macro + set_language()
└── i18n_keys.rs     # Data:   define_i18n! { key: ("中文", "English"), ... }
```

### Engine (`i18n.rs`)

- `AtomicBool IS_ENGLISH` — global language flag, zero-contention relaxed ordering
- `set_language(lang: &str)` — called at startup and on every language switch
- `is_en() -> bool` — inline check for code paths that need conditional logic
- `current_language() -> &'static str` — returns `"en"` or `"zh_CN"`
- `define_i18n!` macro — accepts `key: ("中文", "English")` pairs, generates:
  - `enum I18nKey` with one variant per key
  - `I18nKey::text(self) -> &'static str` — zero-allocation lookup via match
  - `I18nKey::fmt(self, args: &[&str]) -> String` — `{0}`, `{1}` placeholder interpolation

### Data (`i18n_keys.rs`)

Single `define_i18n!` invocation listing all user-visible strings, organized by module:

```rust
use crate::core::i18n::define_i18n;
define_i18n! {
    // ─── Tray ───
    tray_show:           ("显示窗口", "Show Window"),
    tray_settings:       ("设置", "Settings"),
    // ─── Settings tabs ───
    tab_general:         ("通用", "General"),
    tab_clipboard:       ("剪贴板", "Clipboard"),
    // ─── General settings ───
    setting_language:    ("语言", "Language"),
    desc_language:       ("选择界面语言", "Select interface language"),
    // ... ~120 keys total
}
```

### Usage Pattern

```rust
use crate::core::i18n_keys::I18nKey;

// Static text (zero allocation)
.child(I18nKey::TabGeneral.text())

// With parameter interpolation
I18nKey::SelectedCount.fmt(&[&n.to_string()])
// "已选择 3 项" / "3 items selected"
```

### Language Settings Integration

**General tab** ([ui/settings/general.rs](src/ui/settings/general.rs)):

Add a Language row between Theme and Position:

- Options: `"跟随系统" / "Auto"` | `"中文"` | `"English"`
- System = empty string in `AppSettings.language`, calls `detect_system_language()`
- On change: `i18n::set_language()` → `wm.tray.update_language()` → `cx.notify()`
- All UI re-renders with new language immediately

### Tray Menu Live Update

[src/platform/tray.rs](src/platform/tray.rs):

Add `TrayManager::update_language(&mut self)` — calls `MenuItem::set_text()` on all menu items using `I18nKey` values. `muda` (underlying `tray-icon` backend) supports live text updates; no tray recreation needed.

### System Language Detection

Keep `detect_system_language()` in [settings.rs](src/core/settings.rs). On Windows, reads `HKCU\Control Panel\International\LocaleName`; on macOS, reads `NSLocale.currentLocale.languageCode`. Returns `"zh_CN"` for Chinese systems, `"en"` otherwise.

## Implementation Plan (4 Steps)

### Step 1: Engine Layer
- Rewrite `src/core/i18n.rs` with `define_i18n!` macro
- Create `src/core/i18n_keys.rs` with all translation keys
- Old `i18n::tr()` remains functional during transition

### Step 2: Core + Services Layer
- Replace `i18n::tr("中文", "English")` → `I18nKey::KeyName.text()` in:
  - `src/core/settings.rs`
  - `src/core/types.rs`
  - `src/platform/hotkey.rs`
  - `src/platform/source.rs`
  - `src/platform/tray.rs` (constructor + add `update_language()`)
  - `src/services/backends/local_folder.rs`
  - `src/services/backends/webdav.rs`

### Step 3: UI Layer
- Replace hardcoded English strings with `I18nKey` in:
  - `src/ui/settings/mod.rs` — `TAB_NAMES` → function call
  - `src/ui/settings/general.rs` — add Language dropdown + i18n all labels
  - `src/ui/settings/clipboard.rs` — i18n all labels
  - `src/ui/settings/hotkey.rs` — i18n all labels
  - `src/ui/settings/data.rs` — `i18n::tr()` → `I18nKey`
  - `src/ui/settings/sync.rs` — i18n all labels
  - `src/ui/root.rs` — i18n all labels
  - `src/ui/sidebar.rs` — i18n all labels
  - `src/ui/context_menu.rs` — i18n all labels
  - `src/ui/clipboard_list.rs` — i18n all labels
  - `src/ui/search_bar.rs` — i18n all labels
  - `src/ui/hover_toolbar.rs` — i18n all labels
  - `src/ui/titlebar.rs` — i18n all labels
  - `src/ui/add_backend.rs` — i18n all labels
  - `src/ui/tag_filter.rs` — i18n all labels
  - `src/ui/tag_picker.rs` — i18n all labels
  - `src/ui/edit_panel.rs` — i18n all labels
  - `src/ui/components/confirm_dialog.rs` — i18n all labels
  - `src/ui/components/toast.rs` — i18n all labels
- Wire `TrayManager::update_language()` in language change callback

### Step 4: Cleanup
- Remove old `i18n::tr()` function — compile errors will catch any missed call sites
- Update Slint-era comments to reference current GPUI file names
- `cargo build` + `cargo clippy` — ensure zero warnings

## Future Extensibility

To add a third language (e.g., Japanese):
1. Replace `(String, String)` tuples with `[String; 3]` arrays in `define_i18n!`
2. Add `Lang::Ja` to a `Lang` enum
3. Each key gains a third string literal
4. Macro engine unchanged — just the match arms grow

Explicitly NOT in scope for this design (YAGNI).

## Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| Missing keys discovered at runtime | Compile-time enum — referencing a nonexistent key is a compile error |
| `I18nKey::text()` called before `set_language()` | `IS_ENGLISH` defaults to `false` (Chinese), matching default behavior |
| Tray `MenuItem::set_text()` unsupported on some platform | `muda` supports Windows/macOS/Linux; fallback: log error and skip |
