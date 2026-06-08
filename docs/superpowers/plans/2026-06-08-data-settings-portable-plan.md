# Data Settings GPUI Migration + Portable Mode — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the Data settings tab from Slint to GPUI and refactor paths to support portable mode (data next to exe when writable).

**Architecture:** Refactor `core/paths.rs` with a portable-mode detection flag; config/log always stay in the base directory; DB/images follow user-chosen data directory. Build the GPUI Data tab with path display + Change/Reset buttons + Max items input, plus a two-option reset dialog for portable mode.

**Tech Stack:** Rust, GPUI, rfd (file dialog), rusqlite

---

### Task 1: Refactor path architecture for portable mode

**Files:**
- Modify: `src/core/paths.rs`

**What:** Add `is_portable_mode()` detection and rewrite all path functions so config/log are fixed per mode, while DB/images follow user choice.

- [ ] **Step 1: Add portable mode detection and exe_dir helper**

In `src/core/paths.rs`, add the new imports and helper functions at the top (after the existing `const` block, before `fn app_data_dir()`):

```rust
use std::sync::atomic::{AtomicBool, Ordering};

/// Cached portable-mode flag — true when the exe directory is writable.
static IS_PORTABLE: AtomicBool = AtomicBool::new(false);

/// Initialise portable mode detection. Call once at startup.
pub fn init_portable_mode() {
    let exe = exe_dir();
    // Try to create + delete a temp file to test writability.
    let probe = exe.join(".clippi_writable_test");
    let writable = std::fs::write(&probe, b"1").is_ok();
    if writable {
        let _ = std::fs::remove_file(&probe);
    }
    IS_PORTABLE.store(writable, Ordering::Relaxed);
    log::info!(
        "Portable mode: {} (exe_dir: {})",
        writable,
        exe.display()
    );
}

/// The directory containing the running executable.
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns true when the exe directory is writable (portable mode active).
pub fn is_portable_mode() -> bool {
    IS_PORTABLE.load(Ordering::Relaxed)
}
```

- [ ] **Step 2: Rewrite `config_path()`**

Replace the existing `config_path()` function:

```rust
/// Config file path — always in the base directory (exe_dir if portable,
/// otherwise platform data dir). Never affected by user db_path changes.
pub fn config_path() -> PathBuf {
    if is_portable_mode() {
        exe_dir().join(CONFIG_FILE)
    } else {
        app_data_dir().join(CONFIG_FILE)
    }
}
```

- [ ] **Step 3: Rewrite `resolve_db_path()` and `log_path()`**

Replace the existing `resolve_db_path()` and `log_path()` functions:

```rust
/// Resolve the database path.
///
/// - If `db_setting` is non-empty, use it directly (user override).
/// - Otherwise, default to exe_dir (portable) or platform data dir.
pub fn resolve_db_path(db_setting: &str) -> PathBuf {
    if !db_setting.is_empty() {
        PathBuf::from(db_setting)
    } else if is_portable_mode() {
        exe_dir().join(DB_FILE)
    } else {
        app_data_dir().join(DB_FILE)
    }
}

/// Log file path — always in the base directory (same as config, not next to DB).
/// No longer takes a `db_path` argument; log location is independent of data dir.
pub fn log_path() -> PathBuf {
    let base = if is_portable_mode() {
        exe_dir()
    } else {
        app_data_dir()
    };
    base.join("clippi.log")
}

/// The directory that contains database + images (resolved from db_setting).
/// Used by init_images_dir to determine where to store clipboard images.
pub fn resolve_data_dir(db_setting: &str) -> PathBuf {
    let db = resolve_db_path(db_setting);
    db.parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}
```

- [ ] **Step 4: Rewrite `init_images_dir()` and `images_dir()`**

Replace the existing `init_images_dir()` and update `images_dir()`:

```rust
/// Initialize the resolved images directory based on db_path.
/// Must be called once at startup before any clipboard capture.
pub fn init_images_dir(db_path: &str) {
    let dir = resolve_data_dir(db_path).join("images");
    let _ = RESOLVED_IMAGES_DIR.set(dir);
}

pub fn images_dir() -> PathBuf {
    let dir = RESOLVED_IMAGES_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| resolve_data_dir("").join("images"));
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}
```

- [ ] **Step 5: Update `migrate_legacy_files()`**

Update to skip in portable mode (no legacy to migrate from appdata):

```rust
/// One-time migration from legacy CWD/exe-relative paths to platform data dir.
/// Skipped in portable mode — data is already in the exe directory.
/// Non-fatal: logs warnings on failure.
pub fn migrate_legacy_files() {
    // Portable mode — data lives in exe dir, no legacy migration needed.
    if is_portable_mode() {
        log::info!("Portable mode: skipping legacy migration");
        return;
    }

    // Always ensure data directory exists (for fresh installs and after migration)
    if let Err(e) = ensure_app_data_dir() {
        log::error!("failed to create data directory: {e}");
        return;
    }

    let data_dir = app_data_dir();
    let new_config = data_dir.join(CONFIG_FILE);

    // Skip migration if new location already has config
    if new_config.exists() {
        return;
    }

    // Find legacy files in exe's parent directory
    let Some(legacy_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
    else {
        return;
    };

    let legacy_config = legacy_dir.join(CONFIG_FILE);
    let legacy_db = legacy_dir.join(DB_FILE);

    if legacy_config.exists() {
        if let Err(e) = fs::copy(&legacy_config, &new_config) {
            log::error!("failed to migrate config: {e}");
        }
    }

    if legacy_db.exists() {
        let new_db = data_dir.join(DB_FILE);
        if let Err(e) = fs::copy(&legacy_db, &new_db) {
            log::error!("failed to migrate database: {e}");
        }
    }
}
```

- [ ] **Step 6: Add `config_dir()` helper**

Add a `config_dir()` function that returns the directory containing config/log (for callers that need to create parent directories):

```rust
/// Directory containing config and log files (exe_dir or app_data_dir).
pub fn config_dir() -> PathBuf {
    if is_portable_mode() {
        exe_dir()
    } else {
        app_data_dir()
    }
}
```

- [ ] **Step 7: Commit**

```bash
git add src/core/paths.rs
git commit -m "refactor: portable mode path architecture with writability detection

- Add is_portable_mode() detection via temp-file probe at startup
- config_path() and log_path() always resolve to exe_dir (portable) or app_data_dir (system)
- log_path() no longer takes db_path — log stays with config
- resolve_db_path() defaults to exe_dir in portable mode
- resolve_data_dir() returns DB parent for images/ cache
- migrate_legacy_files() skipped in portable mode"
```

---

### Task 2: Update main.rs for new path APIs

**Files:**
- Modify: `src/main.rs`

**What:** Call `init_portable_mode()` early, simplify `init_logging()` (no db_path param), add `init_images_dir()`.

- [ ] **Step 1: Rewrite `init_logging()`**

Replace the existing function signature and body (line 34-52):

```rust
fn init_logging() {
    let log_path = core::paths::log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 1_000_000 {
            let old = log_path.with_extension("log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&log_path, &old);
        }
    }
    if let Ok(file) = std::fs::File::create(&log_path) {
        let _ = simplelog::WriteLogger::init(
            simplelog::LevelFilter::Info,
            simplelog::Config::default(),
            file,
        );
    }
}
```

- [ ] **Step 2: Update `main()` startup sequence**

Replace lines 56-61 in `main()`:

Replace:
```rust
    let db_path = core::paths::resolve_db_path("");
    init_logging(&db_path.to_string_lossy());
```

With:
```rust
    // Detect portable mode before loading any settings (so config/log paths
    // are resolved correctly). Must run before init_logging() and
    // AppSettings::load().
    core::paths::init_portable_mode();
    core::paths::migrate_legacy_files();
    init_logging();
```

And after loading settings (after `let settings = AppSettings::load();` on approx line 73), add:

```rust
        // Initialize images cache directory — follows db_path if set.
        core::paths::init_images_dir(&settings.db_path);
```

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "chore: update main.rs for portable mode path APIs

- Call init_portable_mode() before config/log resolution
- init_logging() simplified — log always follows config dir
- init_images_dir() called after settings load"
```

---

### Task 3: Add settings state convenience methods

**Files:**
- Modify: `src/state/settings.rs`

**What:** Add `db_path()`, `set_db_path()`, `set_max_items()` accessors.

- [ ] **Step 1: Add convenience accessors and mutators**

Add the following methods to `impl SettingsState` (after the existing `max_items()` accessor):

```rust
    pub fn db_path(&self) -> &str {
        &self.inner.db_path
    }

    pub fn set_db_path(&mut self, path: String) {
        self.inner.db_path = path;
        self.save();
    }

    pub fn set_max_items(&mut self, n: u32) {
        self.inner.max_items = n;
        self.save();
    }
```

- [ ] **Step 2: Commit**

```bash
git add src/state/settings.rs
git commit -m "feat: add db_path/set_db_path/set_max_items convenience methods to SettingsState"
```

---

### Task 4: Call init_images_dir in AppState

**Files:**
- Modify: `src/state/app.rs`

**What:** Ensure images directory is initialized when AppState is created.

- [ ] **Step 1: Add init_images_dir call in AppState::new()**

In `src/state/app.rs`, inside `AppState::new()`, add after line 75 (after `let db_path = settings.resolve_db_path();`):

```rust
        crate::core::paths::init_images_dir(&settings.db_path);
```

- [ ] **Step 2: Commit**

```bash
git add src/state/app.rs
git commit -m "chore: call init_images_dir in AppState::new()"
```

---

### Task 5: Build Data settings tab UI (new file)

**Files:**
- Create: `src/ui/settings/data.rs`

**What:** Create the GPUI data settings tab with DB path row (Change/Reset buttons) and Max items input row, plus the ResetDataDirDialog for portable mode. Uses `gpui_component::input::{Input, InputState}` for the number input (same pattern as tag_filter.rs).

- [ ] **Step 1: Create file with module structure, imports, and constants**

```rust
//! Data settings tab — database path + max items.
//!
//! Mirrors the original Slint `SettingsTabData.slint` layout.
//! Includes the reset-data-directory dialog for portable mode.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputState};

use crate::core::i18n;
use crate::core::settings::migrate_database;
use crate::ui::settings::SettingsEvent;

use super::SettingsPanel;

/// Which storage mode the reset dialog should target.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    /// Exe directory (portable / "green" mode).
    Portable,
    /// Platform data directory (system default).
    System,
}

/// State for the reset-data-directory dialog.
#[derive(Clone)]
pub struct ResetDataDirState {
    /// Currently selected storage mode.
    pub selected: StorageMode,
    /// Resolved portable path (exe_dir / clippi.db).
    pub portable_path: String,
    /// Resolved system path (app_data_dir / Clippi / clippi.db).
    pub system_path: String,
}
```

- [ ] **Step 2: Add `render_data_tab()` implementation**

In `data.rs`, add. Note: the max-items `InputState` entity is passed from `SettingsPanel` (lazy-created in Task 6):

```rust
impl SettingsPanel {
    pub fn render_data_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = cx.entity().clone();

        // Snapshot current values.
        let app = self.state.read(cx);
        let db_path_display = app.settings.resolve_db_path();
        let db_path_str = db_path_display.to_string_lossy().to_string();
        let max_items = app.settings.max_items;
        let is_portable = crate::core::paths::is_portable_mode();
        // borrow released

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .pt(px(8.))
            // ── Database path row (76px, sub-row layout) ──
            .child({
                let state = state.clone();
                let this = this.clone();
                let db_path_str = db_path_str.clone();

                let theme = &self.theme;
                let surface = theme.surface;
                let divider = theme.divider;
                let text_1 = theme.text_1;
                let text_2 = theme.text_2;
                let text_3 = theme.text_3;
                let accent = theme.accent;

                div()
                    .h(px(76.))
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .px(px(14.))
                    .pt(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    // Title
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(i18n::tr("数据库路径", "Database path")),
                    )
                    // Path display + buttons row
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.))
                            // Path display (flex_1 to fill available space, left-elided)
                            .child({
                                let theme = &self.theme;
                                div()
                                    .flex_1()
                                    .h(px(28.))
                                    .rounded(px(7.))
                                    .bg(if theme.bg == rgb(0x191a1b) {
                                        rgb(0x191a1b)
                                    } else {
                                        rgb(0xf2f3f8)
                                    })
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(text_2)
                                            .whitespace_nowrap()
                                            .child(db_path_str.clone()),
                                    )
                            })
                            // Change button
                            .child({
                                let state = state.clone();
                                let this = this.clone();
                                div()
                                    .h(px(28.))
                                    .px(px(10.))
                                    .rounded(px(7.))
                                    .bg(accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |s| s.opacity(0.85))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, _cx| {
                                        let result = rfd::FileDialog::new()
                                            .set_file_name("clippi.db")
                                            .save_file();
                                        if let Some(new_path) = result {
                                            let path_str =
                                                new_path.to_string_lossy().to_string();
                                            // Re-load settings from the state, then
                                            // check if the path actually changed.
                                            let old = state.read(_cx)
                                                .settings
                                                .resolve_db_path();
                                            if old == new_path {
                                                return; // No change — skip
                                            }
                                            // Checkpoint DB before migration
                                            {
                                                let s = state.read(_cx);
                                                if let Err(e) = s.db.checkpoint() {
                                                    log::error!(
                                                        "checkpoint failed before migration: {e}"
                                                    );
                                                }
                                            }
                                            match migrate_database(&old, &new_path) {
                                                Ok(()) => {
                                                    state.update(_cx, |s, _cx| {
                                                        s.settings.db_path = path_str;
                                                        s.settings.save();
                                                    });
                                                    // Restart to pick up new DB path
                                                    crate::core::settings::spawn_new_process();
                                                    _cx.shutdown();
                                                }
                                                Err(e) => {
                                                    let _ = this.update(_cx, |_panel, cx| {
                                                        cx.emit(SettingsEvent::DataError(e));
                                                    });
                                                }
                                            }
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xffffff))
                                            .child(i18n::tr("更改", "Change")),
                                    )
                            })
                            // Reset button
                            .child({
                                let state = state.clone();
                                let this = this.clone();
                                div()
                                    .h(px(28.))
                                    .px(px(10.))
                                    .rounded(px(7.))
                                    .bg(rgba(0x00000000))
                                    .border(px(1.))
                                    .border_color(text_3)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(move |s| s.bg(rgba(0xffffff10)))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, _cx| {
                                        // In portable mode, show the two-option dialog.
                                        // In non-portable mode, reset directly to system default.
                                        if crate::core::paths::is_portable_mode() {
                                            let _ = this.update(_cx, |panel, cx| {
                                                panel.show_reset_data_dialog(cx);
                                            });
                                        } else {
                                            let old = state.read(_cx)
                                                .settings
                                                .resolve_db_path();
                                            let default_db =
                                                crate::core::paths::resolve_db_path("");
                                            if old == default_db {
                                                return;
                                            }
                                            // Checkpoint
                                            {
                                                let s = state.read(_cx);
                                                if let Err(e) = s.db.checkpoint() {
                                                    log::error!(
                                                        "checkpoint failed before reset: {e}"
                                                    );
                                                }
                                            }
                                            match migrate_database(&old, &default_db) {
                                                Ok(()) => {
                                                    state.update(_cx, |s, _cx| {
                                                        s.settings.db_path = String::new();
                                                        s.settings.save();
                                                    });
                                                    crate::core::settings::spawn_new_process();
                                                    _cx.shutdown();
                                                }
                                                Err(e) => {
                                                    let _ = this.update(_cx, |_panel, cx| {
                                                        cx.emit(SettingsEvent::DataError(e));
                                                    });
                                                }
                                            }
                                        }
                                    })
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(text_3)
                                            .child(i18n::tr("重置", "Reset")),
                                    )
                            })
                    )
            })
            // ── Max items row (66px, standard row) ──
            .child({
                let state = state.clone();
                let theme = &self.theme;
                let surface = theme.surface;
                let divider = theme.divider;
                let text_1 = theme.text_1;
                let text_3 = theme.text_3;
                let input_bg = if theme.bg == rgb(0x191a1b) {
                    rgb(0x191a1b)
                } else {
                    rgb(0xf2f3f8)
                };

                div()
                    .h(px(66.))
                    .rounded(px(10.))
                    .bg(surface)
                    .border(px(1.))
                    .border_color(divider)
                    .px(px(14.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    // Left: label + description
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(i18n::tr("最大保存条目数", "Max items")),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(i18n::tr(
                                        "设为 0 不限制条目数",
                                        "Set to 0 for unlimited items",
                                    )),
                            ),
                    )
                    // Right: number input (80×28, via InputState entity stored in SettingsPanel)
                    .child(
                        div()
                            .w(px(80.))
                            .h(px(28.))
                            .rounded(px(7.))
                            .bg(input_bg)
                            .border(px(1.))
                            .border_color(divider)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child({
                                let state = state.clone();
                                div()
                                    .w(px(64.))
                                    .child(
                                        Input::new(
                                            self.max_items_input
                                                .as_ref()
                                                .expect("max_items_input not initialized"),
                                        )
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .w_full()
                                        .h(px(20.))
                                        .text_size(px(12.))
                                        .text_color(text_1)
                                        .text_align(gpui::TextAlign::Center)
                                        .on_accept({
                                            let state = state.clone();
                                            move |text, _window, _cx| {
                                                let n: u32 = text.parse().unwrap_or(0);
                                                state.update(_cx, |s, _cx| {
                                                    s.settings.max_items = n;
                                                    s.settings.save();
                                                });
                                            }
                                        }),
                                    )
                            }),
                    )
            })
    }
}
```

> **Note:** The TextInput API in gpui v2.x needs to be verified against the actual crate version. If the API differs, we will adapt the `on_accept` / `on_blur` callbacks accordingly.

- [ ] **Step 3: Add dialog management methods to SettingsPanel**

After the `render_data_tab()` method, add:

```rust
    /// Show the reset-data-directory dialog. Only called in portable mode.
    fn show_reset_data_dialog(&mut self, cx: &mut Context<Self>) {
        let portable_path = crate::core::paths::resolve_db_path("")
            .to_string_lossy()
            .to_string();
        let system_path = {
            let data_dir = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("Clippi")
                .join("clippi.db");
            data_dir.to_string_lossy().to_string()
        };

        // Determine the currently active mode by checking if db_path is empty
        // (empty = default = currently portable) or non-empty (user set a path).
        let app = self.state.read(cx);
        let currently_portable = app.settings.db_path.is_empty();
        // borrow released

        self.reset_data_dialog = Some(ResetDataDirState {
            selected: if currently_portable {
                StorageMode::Portable
            } else {
                StorageMode::System
            },
            portable_path,
            system_path,
        });
        cx.notify();
    }

    /// Dismiss the reset-data-directory dialog.
    fn dismiss_reset_dialog(&mut self, cx: &mut Context<Self>) {
        self.reset_data_dialog = None;
        cx.notify();
    }

    /// Apply the selected reset target: migrate DB, update settings, restart.
    fn apply_reset_data_dir(&mut self, cx: &mut Context<Self>) {
        let dialog = match self.reset_data_dialog.take() {
            Some(d) => d,
            None => return,
        };

        let target_path = match dialog.selected {
            StorageMode::Portable => {
                crate::core::paths::resolve_db_path("")
            }
            StorageMode::System => {
                // Non-portable default path (force system data dir)
                dirs::data_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("Clippi")
                    .join("clippi.db")
            }
        };

        let old_path = self.state.read(cx).settings.resolve_db_path();
        if old_path == target_path {
            cx.notify();
            return;
        }

        // Checkpoint before migration
        {
            let s = self.state.read(cx);
            if let Err(e) = s.db.checkpoint() {
                log::error!("checkpoint failed before reset: {e}");
            }
        }

        match migrate_database(&old_path, &target_path) {
            Ok(()) => {
                let new_db_path = match dialog.selected {
                    StorageMode::Portable => String::new(),
                    StorageMode::System => target_path
                        .to_string_lossy()
                        .to_string(),
                };
                self.state.update(cx, |s, _cx| {
                    s.settings.db_path = new_db_path;
                    s.settings.save();
                });
                crate::core::settings::spawn_new_process();
                cx.shutdown();
            }
            Err(e) => {
                cx.emit(SettingsEvent::DataError(e));
            }
        }
    }

    /// Render the reset-data-directory dialog overlay (portable mode only).
    pub fn render_reset_data_dialog(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dialog = match &self.reset_data_dialog {
            Some(d) => d.clone(),
            None => return div().into_any_element(),
        };

        let theme = &self.theme;
        let surface = theme.surface;
        let accent = theme.accent;
        let accent_soft = theme.accent_soft();
        let text_1 = theme.text_1;
        let text_2 = theme.text_2;
        let text_3 = theme.text_3;
        let divider = theme.divider;

        let this = cx.entity().clone();

        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .z_index(100)
            // Backdrop click → dismiss
            .on_mouse_down(MouseButton::Left, {
                let this = this.clone();
                move |_ev, _window, cx| {
                    cx.stop_propagation();
                    let _ = this.update(cx, |panel, cx| {
                        panel.dismiss_reset_dialog(cx);
                    });
                }
            })
            .child(
                // Modal card
                div()
                    .w(px(300.))
                    .bg(surface)
                    .rounded(px(12.))
                    .border(px(1.))
                    .border_color(divider)
                    .p(px(16.))
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    // Title
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_1)
                            .child(i18n::tr("重置数据目录", "Reset Data Directory")),
                    )
                    // Description
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(text_3)
                            .child(i18n::tr(
                                "选择数据库和缓存文件的存储位置：",
                                "Choose where to store the database and cache files:",
                            )),
                    )
                    // Option: Portable
                    .child({
                        let selected = dialog.selected == StorageMode::Portable;
                        let this = this.clone();
                        div()
                            .rounded(px(8.))
                            .border(px(1.))
                            .border_color(if selected { accent } else { divider })
                            .bg(if selected {
                                accent_soft
                            } else {
                                rgba(0x00000000)
                            })
                            .p(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.stop_propagation();
                                let _ = this.update(cx, |panel, cx| {
                                    if let Some(ref mut d) = panel.reset_data_dialog {
                                        d.selected = StorageMode::Portable;
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(i18n::tr(
                                        "便携（安装目录）",
                                        "Portable (install directory)",
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(dialog.portable_path.clone()),
                            )
                    })
                    // Option: System
                    .child({
                        let selected = dialog.selected == StorageMode::System;
                        let this = this.clone();
                        div()
                            .rounded(px(8.))
                            .border(px(1.))
                            .border_color(if selected { accent } else { divider })
                            .bg(if selected {
                                accent_soft
                            } else {
                                rgba(0x00000000)
                            })
                            .p(px(10.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                cx.stop_propagation();
                                let _ = this.update(cx, |panel, cx| {
                                    if let Some(ref mut d) = panel.reset_data_dialog {
                                        d.selected = StorageMode::System;
                                    }
                                    cx.notify();
                                });
                            })
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(text_1)
                                    .child(i18n::tr("系统默认", "System default")),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(text_3)
                                    .child(dialog.system_path.clone()),
                            )
                    })
                    // Button row
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.))
                            .mt(px(4.))
                            // Cancel
                            .child({
                                let this = this.clone();
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .text_size(px(12.))
                                    .text_color(text_2)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|s| s.bg(rgba(0xffffff10)))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        let _ = this.update(cx, |panel, cx| {
                                            panel.dismiss_reset_dialog(cx);
                                        });
                                    })
                                    .child(i18n::tr("取消", "Cancel"))
                            })
                            // Apply
                            .child({
                                let this = this.clone();
                                div()
                                    .h(px(24.))
                                    .px(px(12.))
                                    .rounded(px(4.))
                                    .text_size(px(12.))
                                    .text_color(rgb(0xffffff))
                                    .bg(accent)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor(CursorStyle::PointingHand)
                                    .hover(|s| s.opacity(0.85))
                                    .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                        cx.stop_propagation();
                                        let _ = this.update(cx, |panel, cx| {
                                            panel.apply_reset_data_dir(cx);
                                        });
                                    })
                                    .child(i18n::tr("应用", "Apply"))
                            }),
                    ),
            )
            .into_any_element()
    }
```

- [ ] **Step 4: Commit**

```bash
git add src/ui/settings/data.rs
git commit -m "feat: add data settings tab with DB path/max-items UI and reset dialog"
```

---

### Task 6: Wire data tab into SettingsPanel

**Files:**
- Modify: `src/ui/settings/mod.rs`

**What:** Add the `data` module, `DataError` event variant, `reset_data_dialog` state field, and replace the stub `render_data_tab()`.

- [ ] **Step 1: Add module declaration and update imports**

At the top of `src/ui/settings/mod.rs`, after line 19 (`mod hotkey;`):

```rust
mod data;
```

After line 23 (`use crate::ui::components::toggle...`), add the `ResetDataDirState` import:

```rust
use data::{ResetDataDirState, StorageMode};
```

- [ ] **Step 2: Add `DataError` variant to `SettingsEvent`**

After line 42 (`ShowHotkeyConfirm(HotkeyConfirmAction),`):

```rust
    /// Data settings error — RootView should show a toast.
    DataError(String),
```

- [ ] **Step 3: Add new fields to `SettingsPanel` struct**

After line 57 (`pub hotkey_confirm: Option<HotkeyConfirmAction>,`):

```rust
    /// Reset-data-directory dialog state (portable mode only).
    pub reset_data_dialog: Option<ResetDataDirState>,
```

Also add the max-items input entity field in the struct (after `search_bar` / similar entity fields, before `toggle_states`):

```rust
    /// Input state for the max-items number input (lazy-created).
    max_items_input: Option<Entity<InputState>>,
```

Add the import for `InputState` in `mod.rs`:
```rust
use gpui_component::input::InputState;
```

In the `new()` constructor, add:

```rust
            reset_data_dialog: None,
            max_items_input: None,
```

- [ ] **Step 4: Replace the stub `render_data_tab()`**

Delete the existing stub (lines 428-437):

```rust
    fn render_data_tab(&self) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(100.))
            .text_color(self.theme.text_3)
            .text_size(px(13.))
            .child("Data settings")
    }
```

The new `render_data_tab()` is defined in `data.rs` via `impl SettingsPanel` — no replacement needed in `mod.rs`.

- [ ] **Step 5: Render the reset dialog in settings panel render()**

In the `render()` method, the reset dialog should be rendered when active. Add after the scroll content area (after the closing of the scroll area div, at the end of the `render()` method's child chain, before the final `div` is returned):

After the scroll container close, add:
```rust
            // ── Reset data directory dialog (overlay) ──
            .child(
                self.render_reset_data_dialog(window, cx)
                    .into_any_element(),
            )
```

- [ ] **Step 6: Commit**

```bash
git add src/ui/settings/mod.rs
git commit -m "feat: wire data settings tab and reset dialog into SettingsPanel"
```

---

### Task 7: Handle new SettingsEvent in RootView

**Files:**
- Modify: `src/ui/root.rs`

**What:** Add handler for `SettingsEvent::DataError` to show toast.

- [ ] **Step 1: Add `DataError` handler**

After line 274 (the closing of `ShowHotkeyConfirm` handler), in the settings subscription match block, add:

```rust
                    SettingsEvent::DataError(msg) => {
                        this.state.update(cx, |s, _cx| {
                            s.toast_message =
                                Some(format!("{}: {msg}", i18n::tr("数据操作失败", "Data operation failed")));
                        });
                        cx.notify();
                    }
```

- [ ] **Step 2: Commit**

```bash
git add src/ui/root.rs
git commit -m "feat: handle DataError event from settings panel in RootView"
```

---

### Task 8: Build and verify

**Files:**
- None (verification only)

- [ ] **Step 1: Build the project**

```bash
cargo build 2>&1
```

Expected: Compilation succeeds with no errors.

- [ ] **Step 2: Fix any compilation errors**

If there are API mismatches (e.g., `TextInput` API in the actual GPUI version), adjust the code to match the actual crate version. Common issues:
- `gpui::TextInput` → may need `gpui::input::TextInput` or similar path
- `on_accept` → may need different callback signature
- Check `accent_soft()` — if `ClippiTheme` doesn't have it, compute inline: `accent.blend_over(rgba(0x0000000A))` or add it to `ClippiTheme`

- [ ] **Step 3: Verify clippy is clean**

```bash
cargo clippy 2>&1
```

Expected: Zero warnings (project policy).

- [ ] **Step 4: Commit any fixes**

```bash
git add -u
git commit -m "fix: resolve build/clippy issues from data settings migration"
```
