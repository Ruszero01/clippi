# Hotkey Settings Tab — GPUI Migration Design

**Date:** 2026-06-08
**Branch:** `experiment/gpui-migration`
**Source:** `ui/SettingsTabHotkey.slint` → `src/ui/settings/hotkey.rs`

## Overview

Migrate the hotkey settings tab from Slint to GPUI, matching the original layout
and behaviour. Use existing GPUI components (`ConfirmDialog`, `Toast`) for
confirmation dialogs and error notifications.

## Architecture

### File Changes

| File | Action | Description |
|------|--------|-------------|
| `src/ui/settings/hotkey.rs` | **New** | Full hotkey tab render + hotkey recording state management |
| `src/ui/settings/mod.rs` | Edit | Add `mod hotkey;`, replace `render_hotkey_tab()` stub, add new `SettingsEvent` variants |
| `src/ui/components/confirm_dialog.rs` | Edit | Add `add_blacklist()` preset factory method |
| `src/ui/root.rs` | Edit | Handle hotkey-related confirm dialogs and toast from SettingsPanel |

### Page Layout

```
┌─────────────────────────────────────┐
│  ┌─ Hotkey Recording Card ────────┐│
│  │ "Hotkey"              [Ctrl+F1]││  ← 66px card, accent border when recording
│  │ "Press new hotkey..."          ││
│  └────────────────────────────────┘│
│                                     │
│  ┌─ Blacklist Section ────────────┐│
│  │ [icon] App Name — Window Title ││  ← 44px info bar with foreground app
│  │                          [🚫]  ││  ← block button → ConfirmDialog
│  ├─────────────────────────────────┤│
│  │ [icon] BlacklistedApp1    [✕]  ││  ← 36px scrollable list rows
│  │ [icon] BlacklistedApp2    [✕]  ││  ← delete button → ConfirmDialog
│  └─────────────────────────────────┘│
│  (or: "No blacklisted apps")        │
└─────────────────────────────────────┘
```

## Components & Data Flow

### 1. Hotkey Recording

- **Data sources**: `AppState.settings.hotkey` (current hotkey string), recording flag (local state in `SettingsPanel`)
- **UI**: A 66px card with label "Hotkey" + description text + button showing current key combo
- **Recording active**: border turns accent color, description changes to "Press new hotkey...", button shows accent_soft background
- **Interaction**: Click button → call hotkey listener's `start_recording()` → poll for result → on success save to settings → on error show Toast
- **Error handling**: Hotkey conflict errors display via **Toast** component with error message

### 2. Blacklist Management

- **Data sources**: `AppState.settings.hotkey_blacklist: Vec<String>`, foreground app info (name, window title, icon)
- **Info bar**: Shows current foreground app icon + name + window title, with a block button on the right
- **Add to blacklist**: Click block button → **ConfirmDialog** with `add_blacklist()` preset → confirm → push to `hotkey_blacklist` → persist
- **Remove from blacklist**: Click delete button on a list row → **ConfirmDialog** with `remove_blacklist()` preset → confirm → remove → persist
- **List**: Scrollable at 160px max height, each row 36px with app icon + name + delete button
- **Empty state**: "No blacklisted apps" message when list is empty

### 3. ConfirmDialog Presets (New)

- `add_blacklist(app_name)` — "Add to Blacklist" / "Disable Clippi hotkey in {app_name}?" / Confirm label "Add"

### 4. Events

New `SettingsEvent` variants:
- `HotkeyError(String)` — Show toast with hotkey conflict/error message
- `ShowConfirmDialog(ConfirmDialogKind)` — Show confirmation dialog from settings context

## States & Edge Cases

- **Recording in progress**: Button shows accent_soft background, border is accent color
- **No foreground app**: Info bar shows "No foreground app", block button hidden
- **Empty blacklist**: Show placeholder text instead of scroll view
- **Blacklist with items**: Show divider line + scrollable list
- **Duplicate blacklist entry**: Silently ignored (checked before add)
- **Hotkey registration conflict**: Toast shows the error, recording cancelled
