# Data Settings GPUI Migration + Portable Mode

**Date:** 2026-06-08
**Branch:** experiment/gpui-migration
**Status:** Approved

## Overview

Migrate the Data settings tab (`SettingsTabData`) from Slint to GPUI, and simultaneously refactor the path architecture to support a "portable mode" where all data files live next to the executable instead of in `%LOCALAPPDATA%`.

## Motivation

Currently Clippi's files can be spread across three locations:
1. Installation directory (exe)
2. `%LOCALAPPDATA%/Clippi/` (config, DB, logs, images via `dirs::data_dir()`)
3. User-custom data directory (DB + images, if user overrides `db_path`)

This complicates backup, portability (USB drives), and cleanup. By consolidating to the installation directory when writable, users get a fully self-contained "green" app. When the installation directory is not writable (e.g., `C:\Program Files\`), the app falls back to the traditional `%LOCALAPPDATA%` location.

## Design

### 1. Portable Mode Detection (`core/paths.rs`)

At startup, attempt to create and delete a temporary file in the executable's parent directory to determine writability. Cache the result globally.

```rust
// New functions
fn exe_dir() -> PathBuf           // executable's parent directory
pub fn is_portable_mode() -> bool  // true if exe_dir is writable

// Path resolution — config + log are ALWAYS fixed per mode:
pub fn config_path() -> PathBuf {
    if is_portable_mode() { exe_dir().join("clippi.toml") }
    else { data_dir().join("Clippi/clippi.toml") }
}

pub fn log_path() -> PathBuf {
    if is_portable_mode() { exe_dir().join("clippi.log") }
    else { data_dir().join("Clippi/clippi.log") }
}
// NOTE: log_path no longer takes a db_path argument — log always stays
// with config, independent of user-chosen data directory.

// Database + images follow the user's choice:
pub fn resolve_db_path(db_setting: &str) -> PathBuf {
    if !db_setting.is_empty() {
        PathBuf::from(db_setting)              // user override
    } else if is_portable_mode() {
        exe_dir().join("clippi.db")            // portable default
    } else {
        data_dir().join("Clippi/clippi.db")    // system default
    }
}

pub fn resolve_data_dir(db_setting: &str) -> PathBuf {
    // Returns the directory containing the database — used for images/
    resolve_db_path(db_setting).parent().unwrap_or(...).to_path_buf()
}

pub fn init_images_dir(db_path: &str) {
    let dir = resolve_data_dir(db_path).join("images");
    RESOLVED_IMAGES_DIR.set(dir).ok();
}

pub fn images_dir() -> PathBuf {
    RESOLVED_IMAGES_DIR.get().cloned()
        .unwrap_or_else(|| resolve_data_dir("").join("images"))
}
```

### 2. Configuration (`core/settings.rs`)

`AppSettings.db_path` semantics unchanged:
- Empty string = use default (portable or system, depending on mode)
- Non-empty = user-specified path

No new fields needed. The `config_path()` change is transparent to settings.

### 3. Main Entry (`main.rs`)

- `init_logging()` no longer takes `db_path` — uses `paths::log_path()` directly (log follows config dir)
- `paths::init_images_dir()` called early with the resolved db_path

### 4. Data Settings Tab (`ui/settings/data.rs` — NEW)

Renders two setting rows:

#### Database Path Row (76px tall, sub-row layout)

```
┌──────────────────────────────────────────────┐
│  Database path / 数据库路径                    │
│  ┌─────────────────────────┬────────┬──────┐  │
│  │ /path/to/clippi.db...  │ Change │ Reset│  │
│  └─────────────────────────┴────────┴──────┘  │
└──────────────────────────────────────────────┘
```

- Path text: 10px, text_2 color, left-elided (elide-left equivalent via CSS)
- "Change" button: accent background → opens `rfd::FileDialog` save dialog
- "Reset" button: transparent with border → opens reset dialog or resets directly

**Change flow:**
1. User clicks Change → `rfd::FileDialog::new().set_file_name("clippi.db").save_file()`
2. If user picks a path → checkpoint DB → `migrate_database(old, new)` → save `db_path` to settings → `spawn_new_process()` → exit

**Reset flow (portable mode):**
1. User clicks Reset → show `ResetDataDirDialog` overlay
2. User picks "Portable" or "System default" → migrate → save → restart

**Reset flow (non-portable mode):**
1. User clicks Reset → directly reset to system default → migrate → save → restart

#### Max Items Row (66px tall, standard row)

```
┌──────────────────────────────────────────────┐
│  Max items / 最大保存条目数        ┌─────────┐│
│  Set to 0 for unlimited           │   200   ││
│                                   └─────────┘│
└──────────────────────────────────────────────┘
```

- Number input: 28px × 80px, centered text
- When value is 0 and input not focused: show "Unlimited / 不限制" placeholder
- Save on Enter or blur

### 5. Reset Data Directory Dialog (inline in `data.rs`)

Shown only in portable mode. A modal overlay with two selectable option cards:

```
┌─────────────────────────────────────────────┐
│  Reset Data Directory / 重置数据目录          │
│                                              │
│  Choose where to store the database and      │
│  cache files:                                │
│                                              │
│  ┌──────────────────────────────────────┐    │
│  │ ● Portable (install directory)       │    │
│  │   C:\Tools\Clippi\clippi.db         │    │
│  └──────────────────────────────────────┘    │
│  ┌──────────────────────────────────────┐    │
│  │ ○ System default                     │    │
│  │   C:\Users\...\AppData\...\Clippi\  │    │
│  └──────────────────────────────────────┘    │
│                                              │
│              [ Cancel ]  [ Apply ]           │
└─────────────────────────────────────────────┘
```

- Two clickable option cards, selected one highlighted (accent border + accent_soft bg)
- Shows concrete target paths
- Current mode pre-selected
- "Apply" triggers migration + restart

### 6. Settings Panel Integration (`ui/settings/mod.rs`)

- Replace `render_data_tab()` stub with call to actual implementation
- Add `SettingsEvent::DataSettingsChanged { reload_items: bool }` variant
- Add `reset_data_dir_dialog` state to `SettingsPanel` (optional enum for dialog visibility + selection)

### 7. State Layer (`state/settings.rs`)

Add convenience methods:
```rust
pub fn db_path(&self) -> &str { &self.inner.db_path }
pub fn set_db_path(&mut self, path: String) { self.inner.db_path = path; self.save(); }
pub fn set_max_items(&mut self, n: u32) { self.inner.max_items = n; self.save(); }
```

## Files Changed

| File | Type | Summary |
|------|------|---------|
| `src/core/paths.rs` | Refactor | Add `is_portable_mode()`, `exe_dir()`; rewrite path resolvers; `log_path()` drops `db_path` param |
| `src/core/settings.rs` | Minor | No schema changes; `config_path()` becomes transparent |
| `src/main.rs` | Minor | `init_logging()` simplified; `init_images_dir()` added early |
| `src/ui/settings/data.rs` | **New** | Data tab rendering + ResetDataDirDialog |
| `src/ui/settings/mod.rs` | Modify | Wire `render_data_tab()` to new impl; add `SettingsEvent::DataSettingsChanged`; add dialog state |
| `src/state/settings.rs` | Minor | Add `db_path()` / `set_db_path()` / `set_max_items()` |
| `src/state/app.rs` | Minor | Call `init_images_dir()` in `AppState::new()` |

## Compatibility

- **Existing users (non-portable):** `is_portable_mode()` returns false for `C:\Program Files\` installs → all paths resolve to `%LOCALAPPDATA%/Clippi/` exactly as before.
- **Portable new users:** All data self-contained in exe directory.
- **`migrate_legacy_files()`:** Continues to work in non-portable mode; skipped in portable mode (no legacy to migrate).
- **Cross-platform:** macOS app bundles in `/Applications/` are not writable → falls back to `~/Library/Application Support/Clippi/` as before.

## Constraints

- Config (`clippi.toml`) and log (`clippi.log`) are **always** in the base directory (exe_dir or data_dir) — never moved by user data directory changes.
- Database (`clippi.db`) and images cache (`images/`) follow the user-chosen data directory.
- Changing the data directory always requires a process restart (copies DB to new location, spawns new process, exits).
- `rfd` crate is already in `Cargo.toml` dependencies.
