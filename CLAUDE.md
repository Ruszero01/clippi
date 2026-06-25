# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Clippi is a lightweight clipboard manager built with Rust + GPUI (Rust-native UI framework). It watches the clipboard (text, images, files, links, rich text, colors), records history, and provides a compact UI for quick copy/paste. Supports Windows and macOS.

## Build & Run

```bash
cargo build
cargo run
cargo test
```

### Build Profiles

- **`[profile.release]`**: `opt-level = "s"` (size-optimized), `lto = true`, `strip = true`, `codegen-units = 1` — produces the smallest binary.
- **`[profile.dist]`**: inherits release but uses `lto = "thin"` — used by `cargo-dist` for CI/CD release builds; balances build time vs binary size.

### Windows Subsystem

`main.rs` uses `#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]` to suppress the console window on Windows. Debug output goes to the log file, not stdout.

## Key Dependencies

- **gpui 0.2.2** — Native GPU-rendered UI framework (Zed's toolkit). Window management, element rendering, event system.
- **gpui-component 0.5** — Provides `v_virtual_list` (variable-height virtual scrolling), `Tooltip`, `ContextMenu`, and theme infrastructure. Theme changes must be synchronized between `ClippiTheme` and gpui-component's global theme.
- **gpui_transitions 0.1.5** — Animation primitives (sidebar expand/collapse, toast enter/exit).
- **rusqlite 0.32 (bundled)** — No external SQLite dependency.
- **clipboard-rs 0.2** — Cross-platform clipboard read/write.
- **serde 1 / serde_json 1 / toml 0.8** — Core serialization stack for settings, sync payloads, DB data.
- **rfd 0.17** — Async native file dialogs (folder picker for sync backends, save dialog for DB migration). Must be called asynchronously to avoid blocking the GPUI main thread.
- **image 0.25** — PNG encoding for icon-to-base64 pipeline (source app icons, file icons).
- **base64 0.22** — Base64 encoding for icon data embedded in clipboard items.
- **pinyin 0.11** — Chinese pinyin search (full pinyin and initial abbreviations).
- **rqrr 0.7** — QR code detection from clipboard images.
- **ureq 2** — HTTP client for GitHub Releases API, WebDAV sync, favicon fetching.
- **global-hotkey 0.6** — Cross-platform global hotkey registration.
- **tray-icon 0.19** — System tray icon and context menu.
- **raw-window-handle 0.6** — Window handle access for tray-icon integration.
- **semver 1** — Semantic version comparison for update checks.
- **sha2 0.10** — SHA256 checksum verification for downloaded updates.
- **simplelog 0.12** — File-based logging with auto-rotation at 1MB (`LevelFilter::Info`).
- **hostname 0.4 / libc 0.2** — Device hostname for sync backend identification.
- **percent-encoding 2** — URL percent encoding for WebDAV paths.
- **cc 1** (build-dependency) — Compiles macOS ObjC OCR helper (`src/platform/ocr_helper.m`).

### Platform-Specific Dependencies

**Windows:**
- **windows-sys 0.59** — Win32 API bindings (HiDPI, input, GDI, DWM, shell, threading).
- **windows 0.58 / windows-core 0.58** — Windows Runtime bindings (OCR, COM, UIA accessibility).
- **winreg 0.56** — Registry access for OneDrive detection and auto-start configuration.

**macOS:**
- **objc2 0.6 / objc2-foundation 0.3 / objc2-app-kit 0.3 / objc2-quartz-core 0.3** — Apple framework bindings (accessibility, paste, tray, OCR, caret detection).
- **core-graphics 0.25 / core-foundation 0.10** — CoreGraphics events for paste simulation and focus detection.

## Architecture

### Source Layout

```
src/
├── main.rs              # Entry point, GPUI App launch, font registration, tray setup
├── core/                # Platform-agnostic logic (UI-framework independent)
│   ├── db.rs            # SQLite persistence (rusqlite bundled)
│   ├── types.rs         # ClipboardItem, ContentType, TagInfo, URL/path helpers
│   ├── filters.rs       # Extensible filter system (type + keyword + tag + fav, AND logic)
│   ├── settings.rs      # TOML-based settings, auto-start, DB migration
│   ├── frontend.rs      # Window position modes, size constants
│   ├── color.rs         # Color detection (HEX/RGB), parsing, normalization, conversion
│   ├── paths.rs         # Platform-aware paths + legacy migration
│   ├── sync.rs          # Sync data types, merge logic (v3 protocol, 4-phase merge)
│   ├── migration.rs     # DB schema and sync protocol versioned migration framework
│   ├── i18n.rs          # Atomic-based language flag, zero-allocation translation
│   ├── i18n_keys.rs     # I18n key enum generated via define_i18n! macro
│   ├── cache_cleanup.rs # Cleanup unused image/file caches (runs at startup)
│   ├── ocr.rs           # OCR text recognition (Windows WinRT / Apple Vision)
│   └── qr.rs            # QR code detection from clipboard images
├── state/               # GPUI Entity-based application state
│   ├── app.rs           # AppState — root entity holding all shared data (items, tags, filters, settings, DB)
│   └── sync.rs          # SyncState — per-backend sync status snapshots
├── ui/                  # GPUI UI components (all UI is Rust code, no .slint files)
│   ├── root.rs          # RootView — main window layout, drag/resize, theme
│   ├── window_manager.rs # WindowManager — lifecycle, positioning, poll loop, auto-hide, hotkey, tray dispatch
│   ├── clipboard_list.rs # Main clipboard list with search, filters, cards, batch ops
│   ├── clipboard_card.rs # Individual clipboard item card rendering
│   ├── quick_paste.rs   # Non-focus-stealing quick paste popup window
│   ├── search_bar.rs    # Search bar + filter buttons
│   ├── type_filter_config.rs # Customizable type filter bar layout (drag-to-reorder)
│   ├── sidebar.rs       # Side tag bar (pinned tags)
│   ├── titlebar.rs      # Title bar (pin toggle, app icon)
│   ├── context_menu.rs  # Right-click context menu (single + batch modes)
│   ├── edit_panel.rs    # Full-text editor for clipboard entries
│   ├── add_backend.rs   # Sync backend add/edit floating panel
│   ├── tag_filter.rs    # Tag filter panel (toggle filter, CRUD)
│   ├── tag_picker.rs    # Tag picker panel (per-item / batch tag assignment)
│   ├── hover_toolbar.rs # Hover toolbar on clipboard cards
│   ├── rich_preview.rs  # Rich text / HTML preview
│   ├── theme.rs         # ClippiTheme struct + light/dark themes
│   ├── components/      # Reusable GPUI components
│   │   ├── confirm_dialog.rs
│   │   ├── toast.rs
│   │   └── toggle.rs
│   └── settings/        # Settings panel (GPUI)
│       ├── mod.rs       # SettingsPanel — tab container
│       ├── general.rs   # General settings tab
│       ├── clipboard.rs # Clipboard behavior settings tab
│       ├── hotkey.rs    # Hotkey recording settings tab
│       ├── data.rs      # Database path, max items tab
│       ├── sync.rs      # Cloud sync settings, backend list, interval
│       └── version.rs   # Update status, release notes, download button
├── services/            # Business logic (GPUI-aware)
│   ├── gpui_clipboard.rs # GpuiClipboardService — clipboard processing + DB sync
│   ├── gpui_sync.rs     # GpuiSyncService — multi-backend sync orchestration
│   ├── clipboard_ops.rs # Clipboard read/write helpers
│   ├── update.rs        # GitHub Releases checker (semver compare, asset selection)
│   ├── downloader.rs    # Streaming download + SHA256 verification
│   ├── updater.rs       # Update orchestrator (download→verify→prepare→restart)
│   ├── install.rs       # Platform installers (NSIS / DMG mount + codesign)
│   ├── poll_loop.rs     # Shared polling constants (POLL_INTERVAL_MS = 200)
│   ├── favicon.rs       # Website favicon fetching via Google favicon service
│   └── backends/
│       ├── mod.rs        # SyncBackend trait
│       ├── local_folder.rs  # Local-folder sync backend (clippi_sync.json)
│       └── webdav.rs     # WebDAV sync backend (ETag cache, Basic Auth)
└── platform/            # OS-specific implementations
    ├── clipboard.rs     # ClipboardShared + per-OS listeners (thread + polling)
    ├── hotkey.rs        # HotkeyListener trait + Windows/macOS/Linux impls
    ├── tray.rs          # TrayManager — tray-icon integration
    ├── focus.rs         # FocusWatcher (WinEventHook / NSWorkspace polling)
    ├── paste.rs         # Paste simulation (Ctrl+V / Cmd+V)
    ├── monitor.rs       # Cursor position, multi-monitor work areas
    ├── blacklist.rs     # Foreground window detection
    ├── source.rs        # Clipboard source app detection + icon extraction (Win)
    ├── text_input.rs    # Three-path caret detection (Win32 + UIA + macOS AX)
    └── util.rs          # Shared platform utilities (encode_png, HICON→base64 PNG)
```

### Application State (`src/state/app.rs`)
- `AppState` is the root GPUI entity (`Entity<AppState>`) holding all shared data
- Fields: settings, db, items, tags, filters, selected_ids, sync state, toast, clipboard coordination flags (`Arc<AtomicBool>`)
- Methods for data mutations: CRUD items/tags, filter management, batch operations, clipboard copy/paste
- Uses GPUI's `cx.read_entity()` / `cx.update_entity()` for access from child views
- `sync_dirty: Arc<AtomicBool>` shared with GpuiSyncService for triggering sync cycles
- `batch_pasting` / `skip_next` shared with clipboard listener thread to prevent self-recording

### Window Manager (`src/ui/window_manager.rs`)
- `WindowManager` — unified entity owning window lifecycle: show/hide, activate, position calculation
- Replaces Slint-era `Frontend` + `FocusService` + `HotkeyService` + `Looper`
- Owns a GPUI poll loop at 200ms (`POLL_INTERVAL_MS`) via `cx.spawn()` + `Timer::after()`
- Polling responsibilities: clipboard processing, hotkey events, focus/auto-hide, sync results, tray menu events, quick paste window lifecycle
- Manages both the main window and the quick paste window (separate GPUI window handles, show/hide, position)
- Emits `WindowManagerEvent` enum for RootView consumption (ClipboardChanged, PinnedChanged, OpenSettings, QuickPaste, etc.)
- Manages `ForegroundAppName` (`Arc<Mutex<String>>`) shared with platform focus watcher thread
- Orchestrates update flow: check → download → verify → install → restart, driving `UpdatePhase` state machine

### GPUI Clipboard Service (`src/services/gpui_clipboard.rs`)
- `GpuiClipboardService` — processes clipboard pending items, upserts to DB, updates AppState items list
- Runs within WindowManager's poll loop (no separate thread needed for processing)
- Platform clipboard listener still runs in a **dedicated thread** (50ms polling), pushes to shared buffer

### Clipboard Watcher (`src/platform/clipboard.rs`)
- Runs in a **dedicated thread** polling every 50ms via `ClipboardContext`
- Multi-format detection priority: Files > Image > Link > Color > RichText > PlainText
- Pushes items to `ClipboardShared.pending` (Arc<Mutex<Vec>>)
- Images saved as PNG to `%LOCALAPPDATA%/Clippi/images/` (or macOS equivalent)
- Batch-paste guard skips clipboard recording to avoid redundant entries
- Captures clipboard source app info on detection (Windows)

### Quick Paste Window (`src/ui/quick_paste.rs`)

A compact, non-focus-stealing floating window for rapid clipboard pasting without interrupting the active application. Uses a **separate GPUI window** with tool-window styling (not a popup/overlay) so it never steals focus.

- Renders a 5-row virtual list with optional type filter bar and pinned tag row.
- Dynamic height via `calc_quick_window_height(has_tag_row, has_type_bar)` — shrinks when bars are hidden to avoid wasted space.
- Keyboard navigation: `↑/↓` select, `←/→` page (5 items at a time), `1-5` numeric direct paste of visible items, `Enter` paste selected, `Esc` close.
- `QUICK_WINDOW_WIDTH` = 430px, `ROW_HEIGHT` = 44px, `QUICK_WINDOW_CORNER_RADIUS` = 10px.
- Positioned near the text cursor via `platform::text_input::get_text_input_anchor()`, falling back to the mouse cursor.
- Emits `QuickPasteEvent::Paste(i64)` consumed by `WindowManager`, which performs the paste and hides the quick window.
- Email/phone entries show masked preview by default; entries with notes display the note text instead of content (mirrors main window "show original on hover" behavior).

### Caret Detection (`src/platform/text_input.rs`)

Best-effort text cursor position detection for positioning the quick paste window near the user's input caret. Returns `TextInputAnchor { x, y, width, height }` or `None` (callers fall back to the mouse cursor).

**Windows — three-path detection (in priority order):**

1. **Path A: GetGUIThreadInfo** — queries `GUITHREADINFO.hwndCaret` / `rcCaret` for classic Win32 edit controls. Fastest and most reliable. Validates size ≤ 200×200 (larger rects are likely text selections, not carets).
2. **Path B: AttachThreadInput + retry** — attaches our thread to the target UI thread, retries GetGUIThreadInfo, then falls back to `GetFocus()` + `GetCaretPos()`. Handles UIPI-isolated processes. Rejects (0,0) caret positions.
3. **Path C: UI Automation** — uses `IUIAutomationTextPattern2::GetCaretRange()` for browsers (Chrome, Edge), Electron apps, and other modern UIA-enabled applications. Requires COM initialization with `COINIT_MULTITHREADED`.

All paths validate: reject oversized rects, verify coordinates are on a monitor (`is_point_on_monitor`), and guard against targeting Clippi's own windows.

**macOS — Accessibility API:**
- Obtains `AXFocusedUIElement` from the system-wide accessibility object.
- Tries `AXSelectedTextRange` → `AXBoundsForRange` for precise caret position.
- Falls back to `AXPosition` + `AXSize` for the focused element's bounding rect.
- Requires accessibility permission; returns `None` silently if not granted.

### Database (`src/core/db.rs`)
- `rusqlite` with `bundled` feature — no external SQLite dependency
- Located at `%LOCALAPPDATA%/Clippi/clippi.db` on Windows, `~/Library/Application Support/Clippi/` on macOS
- Schema: `clipboard_items` (id, content_type, meta_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, is_favorite, note, source_app_name, source_app_icon)
- Tag tables: `tags` (id, name, color, updated_at) + `item_tags` (tag_id, item_id, used_at)
- Tombstone tables: `deleted_items` + `deleted_tags` for cross-device deletion propagation
- `upsert()` updates `updated_at` on duplicate hash, otherwise inserts
- Queries support filter dimensions via `ClipboardFilters.db_where()` with AND logic (type + keyword + fav + tag)
- Lazy content type reclassification on load (link→path correction)
- Tag CRUD: `create_tag`, `delete_tag`, `update_tag`, `add_item_tag`, `remove_item_tag`
- Sync helpers: `get_all_sync_items_with_tags`, `insert_sync_item_raw`, `update_sync_item`, tombstone operations

### Settings (`src/core/settings.rs`)
- TOML file at `%LOCALAPPDATA%/Clippi/clippi.toml`
- Fields: theme, language, hotkey, quick_window_hotkey, auto_start, auto_hide, silent_start, show_taskbar_icon, db_path, sort_by_created, window_position_mode, saved_window_x/y, saved_window_width/height, card_height_mode, show_source_app, auto_scroll_to_top, copy_as_plain_text, show_original_on_hover, max_items, ocr_enabled, qr_enabled, pinned_tag_ids, sync_auto_enabled, sync_interval_secs, sync_backends, quick_window_enabled, type_filter_order, disable_system_window_behavior, auto_focus_search, per_app_paste_hotkeys, blacklisted_hotkey_apps
- `BackendConfig` struct: id, enabled, backend_type, name, folder_path, device_name, webdav_url, webdav_username, webdav_password, sync_interval_secs, last_sync_at, last_sync_items, last_sync_tags
- Auto-start via Windows registry (`HKCU\...\Run`) or macOS LaunchAgent plist
- `migrate_database()` copies DB to a user-chosen path, then spawns a new process and quits
- Legacy migration: old `sync_enabled` + `sync_data_dir` fields → `sync_backends` list

### Paths & Migration (`src/core/paths.rs`)
- Resolves config/DB paths via `dirs::data_dir()` → `$LOCALAPPDATA/Clippi/` (Windows) / `~/Library/Application Support/Clippi/` (macOS)
- **Portable mode**: If `clippi.toml` exists next to the executable at startup, all data (config, DB, logs, images) lives in the exe directory instead of the platform data dir. Detected via `init_portable_mode()` and cached in `AtomicBool`. When switching from a standard install to portable, `migrate_portable_data()` auto-copies existing data from the system data directory to the exe directory.
- `migrate_legacy_files()` one-time copies config + DB from exe dir to platform data dir (skipped in portable mode)
- `init_images_dir(db_path)` must be called once at startup before any clipboard capture; pre-creates `icons/` and `file_icons/` cache directories
- `merge_images_dir()` copies missing images between data directories (used during DB path migration)

### Application Startup (`src/main.rs`)

**Single-instance enforcement**: `ensure_single_instance()` binds TCP port `127.0.0.1:19876`. If the bind fails, a second instance is detected and the app exits silently. This prevents data corruption from concurrent DB access.

**Logging**: `init_logging()` sets up `simplelog::WriteLogger` writing to `clippi.log` in the data directory (or exe dir in portable mode). Auto-rotates at 1MB (renames to `.log.old`). `LevelFilter::Info` — debug/trace messages are stripped. Log path is independent of DB path.

**Icon font**: Registers a custom icon font (`assets/fonts/iconfont.ttf`) embedded via `include_bytes!`. The font provides custom glyphs for type filter buttons, action icons, and status indicators. Copied to a temp directory for system-level font registration.

### Async File Dialogs (`rfd`)

The `rfd` crate provides async native file dialogs. All dialog calls must be `async` (`.pick_folder()`, `.save_file()`) — they run on a background thread via `cx.spawn()` to avoid blocking the GPUI main thread. Used in:
- `add_backend.rs` — folder picker for local-folder sync backend setup
- `settings/data.rs` — save dialog for database path migration

### Migration Framework (`src/core/migration.rs`)

Manages two independently versioned concerns:

**Database schema** — `DB_MIGRATIONS` is a const array of sequential `DbMigration` entries (version, description, SQL). `run_db_migrations()` reads `PRAGMA user_version` and applies pending migrations in order. Current migrations:
- v1: Unique indexes on tombstone tables (prevent unbounded growth)
- v2: `meta_type` column for email/phone plain-text subtypes
- v3: Indexes on `content_type` and `is_favorite` for filtered queries
- v4: Unify content_type — migrate link/path/color to `plain_text` with `meta_type`

To add a migration: append to `DB_MIGRATIONS` with the next sequential version number.

**Sync protocol** — `SYNC_VERSION` (currently 3) is written into every `SyncPayload`. `migrate_sync_payload()` upgrades older payloads to the current version on receipt (v1→v2 added `meta_type` with serde default; v2→v3 normalizes legacy link/path/color content_type strings to plain_text + meta_type).

Rule: DB schema must support all fields the current sync protocol carries. The two versions evolve independently — bumping sync protocol does not require a DB migration (and vice versa) unless new persistent fields are added.

### Internationalization (`src/core/i18n.rs` + `i18n_keys.rs`)

Global atomic-based design — no settings reference threaded through the call chain:

- `IS_ENGLISH: AtomicBool` — set once at startup and on language switch via `set_language()`. Default is Chinese (zh_CN).
- `LANG_VERSION: AtomicU64` — monotonic counter incremented on every language change. Components cache this value to detect when they need to refresh i18n text.
- `define_i18n!` macro generates the `I18nKey` enum with `text() → &'static str` (zero allocation) and `fmt(args: &[&str]) → String` (positional placeholder replacement with `{0}`, `{1}`, …).
- Usage: `I18nKey::SomeLabel.text()` — returns the translated string for the current language without any heap allocation.

### Frontend (`src/core/frontend.rs`)
- Position modes: `Center` (cursor's monitor), `FollowMouse` (at cursor), `Remember` (last position)
- Multi-monitor aware via `monitor::get_monitor_work_area()`
- Suppress period (200ms) after show to prevent immediate auto-hide
- Window size constants: `DEFAULT_WINDOW_WIDTH` (320), `DEFAULT_WINDOW_HEIGHT` (480)
- Window size persistence across sessions; actual window rendering and resize/drag handles are in `ui::root::RootView`

### Filters (`src/core/filters.rs`)
- `ClipboardFilters` combines type filters (plain_text, rich_text, image, file, link/path, color, contact/email/phone) + keyword search + favorites + tag IDs
- All dimensions combine with AND logic; tag filter uses OR across selected tags (switchable to AND mode)
- Both in-memory matching (`matches_item()`) and SQL generation (`db_where()`)
- "link" filter auto-expands to include "path" type; "file" filter auto-expands to include "image" type
- Keyword search also matches tag names and OCR text; image-type items excluded from keyword match
- Pinyin search support: full pinyin and initial abbreviations match Chinese content
- Type filter bar is customizable via `type_filter_config.rs` — right-click to show/hide/reorder type buttons; auto-collapses to icon-only mode in narrow windows. Order persisted in settings.

### Color Detection (`src/core/color.rs`)
- Recognizes HEX (#RGB, #RRGGBB, #RRGGBBAA, bare RRGGBB) and RGB (rgb/rgba function, comma/space separated)
- Normalizes to canonical 6-digit uppercase hex for deduplication
- Supports percentage and float channel values
- Conversion to `ColorValue` → to_css_hex / to_rgb for paste-as format

### OCR & QR Detection (`src/core/ocr.rs` + `qr.rs`)

- **OCR**: `OcrEngine` trait with platform-specific implementations — Windows WinRT (`Windows.Media.Ocr`) and macOS Apple Vision (`VNRecognizeTextRequest`). Runs on a background thread; results are cached in `RichData.ocr_text` keyed by content hash. Auto-OCR on clipboard capture (when enabled in settings); manual OCR via right-click "Paste OCR text". OCR text is searchable via keyword filter (matches `rich_data` column for image-type items).
- **QR**: `rqrr` crate-based QR code detection. Auto-detection on image clipboard capture. Right-click "Scan QR code" copies the decoded URL/text. QR-detected images show a distinct icon in the card view.

### Cloud Sync (`src/core/sync.rs` + `src/services/gpui_sync.rs`)

- **v3 protocol**: JSON-based `clippi_sync.json` placed in cloud-synced folder (OneDrive, iCloud, WebDAV server).
- Transport-agnostic `SyncBackend` trait with two implementations:
  - `LocalFolderBackend` — reads/writes `clippi_sync.json` to a local or cloud-synced folder. Uses file mtime for cache-aware pulls.
  - `WebDAVBackend` — HTTP-based (NAS, NextCloud, etc.). Uses ETag/If-None-Match for cache-aware pulls, Basic Auth. 30-second request timeout.
- **4-phase merge**: clean tombstones → process item deletions → process tag deletions → merge tags → merge items
- Last-writer-wins conflict resolution by `updated_at` RFC3339 timestamp compared via `DateTime::parse_from_rfc3339()` (not raw string compare — fractional seconds vary).
- **Tombstone mechanism**: deleted items/tags recorded locally for 30-day propagation window. Tombstones use timestamp-aware LWW: if a remote item/tag's `updated_at` is newer than its tombstone's `deleted_at`, the tombstone is cleared and the data is imported (handles delete-then-recreate scenarios). Items/tags recreated on the same device clear their own tombstones to prevent self-blocking.
- **Semantic hash**: deterministic snapshot hash (all arrays sorted by natural key) used to skip no-op pushes and prevent infinite sync loops. Hash differs but merge stats are empty → push is also skipped to break ping-pong cycles.
- `GpuiSyncService` runs within WindowManager's poll loop: checks backend status, starts async sync cycles, collects completed results, refreshes `AppState.items`/tags/`SyncState`.
- Each backend sync runs in a dedicated background thread (pull → merge → build snapshot → push).
- Auto-sync on dirty flag change (5s cooldown) + periodic interval polling (configurable per-backend).
- Sync dirty flag (`AppState.sync_dirty: Arc<AtomicBool>`) is shared with GpuiSyncService.
- Excludes image/file content types from sync payload. Favorites-only sync mode available.
- Backends support individual enable/disable, independent sync intervals (30s / 1min / 10min / 30min), connection testing, and per-backend last-sync/item/tag tracking.

### Tag System
- Tags stored in SQLite (`tags` + `item_tags` tables)
- Tags synced across devices via sync payload (name-based merge key, color with timestamp conflict resolution)
- 12 preset colors in round-robin assignment
- Filter by tag (OR logic across selected tags, AND with other filter dimensions); switchable to AND mode
- Batch tag operations: add/remove/clear tags on multiple selected items
- Tag deletion propagates to other devices via tombstone mechanism
- **Side tag bar** (`ui::sidebar`): pinned tags appear as a vertical bar on the left side of the window. Animations via `gpui_transitions` — slide expand/collapse (300ms), opacity fade (250ms). Tags can be pinned/unpinned via right-click. `pinned_tag_ids` persisted in TOML settings. Window width dynamically adapts (320→380px when sidebar is visible).

### Tray (`src/platform/tray.rs`)
- Built with `tray-icon` crate
- Context menu: current version label, "显示窗口" (Show), "设置" (Settings), "检查更新" (Check for Updates), "退出" (Quit)
- Shows a dot indicator on the tray icon when an update is available
- Double-clicking the tray icon shows the window
- "检查更新" opens the main window and navigates to Settings → Version tab

### Paste Simulation (`src/platform/paste.rs`)
- Windows: uses `SendInput` to simulate Ctrl+V (4 INPUT events: Ctrl down, V down, V up, Ctrl up)
- macOS: uses `CGEvent` to simulate Cmd+V
- Restores focus to previous window via `SetForegroundWindow` before pasting
- `paste_sync` blocks until the paste sequence completes; `paste_after_delay` spawns a thread

### Update System (`src/services/update.rs` + `downloader.rs` + `updater.rs` + `install.rs`)

Four-module pipeline for automatic updates from GitHub Releases:

- **`update.rs`** — `UpdateChecker` queries GitHub Releases API (`api.github.com/repos/{owner}/{repo}/releases/latest`), compares `semver::Version`, selects the platform-appropriate asset (NSIS `.exe` on Windows x64, `.dmg` on macOS aarch64/x86_64), and returns `UpdateInfo` (version, release notes, download URL, checksum URL, asset name, size). `UpdatePhase` enum drives the UI state machine: `Idle → Checking → UpToDate | UpdateAvailable → Downloading(progress) → Verifying → Installing → ReadyToRestart | Error`.

- **`downloader.rs`** — Streaming HTTP download with percentage progress callbacks (8KB buffer). SHA256 verification via `sha2` crate against checksum file fetched from the release. `fetch_checksum()` parses the `<hash>  <filename>` format from `.sha256` files. All blocking I/O — call from a background thread.

- **`updater.rs`** — High-level orchestrator: `download_and_prepare()` runs download → verify → prepare on a background thread, reporting phase transitions via callback. `launch_prepared_update()` starts the platform installer; the caller must quit immediately after. `cleanup_temp()` clears `%TEMP%/clippi-update` (or `$TMPDIR`) on startup.

- **`install.rs`** — Platform-specific installation:
  - **Windows**: `ShellExecuteW` with `runas` verb launches NSIS installer normally — user completes the wizard manually. The installer handles restarting Clippi after installation.
  - **macOS**: `hdiutil attach` mounts the DMG, `ditto` extracts the `.app` bundle, `codesign --verify --deep --strict` checks signature. A shell script (waits for process exit → backs up old app → `ditto` new app → `xattr -dr com.apple.quarantine` → `open`) performs atomic replacement. Falls back to `osascript` with admin privileges if the Applications folder isn't writable. Translocation detection prevents updating from a quarantined location.

Startup check runs silently; re-checks every 24 hours. Tray icon shows a dot indicator when an update is available. The settings "Version" tab shows current version, update status, release notes, and a download/install button.

### Types (`src/core/types.rs`)
- `ContentType` enum: PlainText, RichText, Image, File (Link, Path, Color, Email, Phone are now PlainText subtypes tracked via `meta_type`)
- `DisplayKind` enum: resolves the visual type for rendering — maps `meta_type` values (link, path, color, email, phone) to distinct display categories, falls back to `content_type`
- `ClipboardItem`: id, content_type, meta_type, full_text, content_hash, created_at, updated_at, image_path, rich_data, file_data, is_favorite, note, source_app_name, source_app_icon, tags
- `TagInfo`: id, name, color (6-digit uppercase hex), updated_at
- `RichData`: HTML, RTF, and OCR text content
- `FileData` / `FileInfo`: list of file paths with name and is_dir
- `SourceAppInfo`: app name + base64 icon
- `content_hash` via `DefaultHasher`; color hash via normalized hex
- URL detection: `is_url()` (requires >10 chars), `url_domain()`, `url_path()`
- Path detection: `is_path()` (Windows drive letters, UNC paths, Unix absolute paths)
- `format_relative_time()` for Chinese-language relative time display
- `mask_sensitive_preview()` for email (first 2 chars + domain) and phone (first 3 + last 4 digits) privacy masking
- `parse_hex_color()` for hex string → (r,g,b) conversion

## Key Concurrency Model

All UI mutations happen on the GPUI main thread. `WindowManager` drives a 200ms poll loop (`POLL_INTERVAL_MS`) using `cx.spawn()` + `Timer::after()`:
1. Clipboard processing — drains `ClipboardShared.pending`, upserts to DB, updates `AppState.items`
2. Tray menu events — polls `TrayManager` event receiver
3. Hotkey events — polls `GlobalHotKeyEvent` receiver + recording detection
4. Focus/auto-hide — checks foreground window via `FocusWatcher`
5. Sync orchestration — `GpuiSyncService` collects completed sync results, checks backend status, starts due cycles

Background threads:
- Clipboard listener thread (50ms polling loop via `ClipboardContext`, pushes to shared buffer)
- Focus watcher thread (WinEventHook message pump on Windows, NSWorkspace polling on macOS)
- Batch paste thread (sequential paste with delay, coordinated via `AtomicBool` flags in `AppState`)
- Async paste thread (one-shot Ctrl+V with delay, spawned per paste)
- Sync threads (one per backend cycle: pull → merge → snapshot → push)
- Update check thread (GitHub Releases API polling, every 24h)
- Update download thread (streaming download + SHA256 verify + platform prepare)
- OCR processing thread (per-image, signals refresh on completion)
- Cache cleanup thread (startup: removes orphaned images + expired icon caches)

## Platform Notes

- **Windows**: Full support (clipboard, paste via SendInput, hotkey via GetAsyncKeyState, auto-start, focus monitoring via WinEventHook, source app detection + icon extraction, favicon fetching via Google favicon service, OneDrive detection via env/registry, UI Automation caret detection via IUIAutomationTextPattern2 for modern apps, Windows OCR via WinRT)
- **macOS**: Full support (clipboard, paste via CGEvent, hotkey via CGEventSourceKeyState, auto-start via LaunchAgent, focus via NSWorkspace, iCloud detection, Apple Vision OCR, Accessibility API caret detection via AXFocusedUIElement)
- **Linux**: Stub implementations only (not yet functional, all platform services return no-ops)
- `build.rs` generates Windows resources (icon, metadata) via `winresource` crate; no longer compiles Slint

### ⚠️ Windows DPI / SetWindowPos 重入陷阱（重要）

**症状**：多显示器不同 DPI 切换时，窗口缩放比例不正确、定位偏移、快速窗口边缘出现空隙。

**根本原因**：在 GPUI entity update（`cx.update_entity()` / `WindowManager::update()`）中同步调用 Win32 `SetWindowPos` 会导致 **AppCell 重入冲突**：

1. `SetWindowPos` 是同步 API，Windows 会在调用返回前同步派发 `WM_DPICHANGED`、`WM_SIZE`、`WM_MOVE` 等消息
2. GPUI 的 native callback 收到这些消息时需要调用 `cx.update()` 来同步 `Window::scale_factor()` 等内部状态
3. 但此时调用栈还在 entity update 中，`AppCell` 处于借出（borrowed）状态
4. Native callback 无法重新借用，**静默失败** — GPUI 内部缓存的 scale_factor 与实际窗口 DPI 永久不一致
5. GPUI 后续用这个过时的 DPI 缓存计算窗口大小 → 所有依赖物理像素的计算都出错

**关键表现**：
- `Window::scale_factor()` 返回的值与实际 `GetDpiForWindow()` 不一致
- 窗口从高 DPI 显示器拖到低 DPI 显示器后 viewport 偏小，反之偏大
- Quick 窗口的 `window_min_size` 用旧 DPI 的 NC 边框偏移（non-client border offset）计算最小外框，导致窗口被夹宽/夹窄

**正确做法**：
- **所有 `SetWindowPos` 调用必须通过 `cx.spawn()` 异步执行**，先 `Timer::after(Duration::from_millis(1)).await` yield 出 AppCell 借用，让 GPUI 的 native callbacks 能正常处理 DPI 变化消息
- 快速窗口采用两阶段定位：(1) 先用当前 DPI 放置窗口让 WM_DPICHANGED 触发，(2) 短暂 delay 后用目标显示器 DPI + client offset 补偿精确定位
- DPI 变化时只需 emit 事件触发 GPUI 重绘，**不要同步 nudge HWND**（同样会重入）
- 所有 scale_factor 计算必须按目标显示器位置获取（`get_scale_factor(x, y)`），不能用 `(0, 0)`（可能对应不同 DPI 的显示器）
- 主窗口创建时用 `SWP_NOSIZE` 仅移动位置，保留 GPUI WindowOptions 设置的逻辑尺寸
- Windows 平台 Quick 窗口禁用 `window_min_size: None`，避免 GPUI 用缓存的旧 DPI 边框数据夹宽窗口

## CI/CD

GitHub Actions workflow (`.github/workflows/release.yml`) uses **cargo-dist v0.31** for automated builds:

- **Triggers**: PRs to main and version tags (`[0-9]+.[0-9]+.[0-9]+*`).
- **Windows x86_64**: Builds NSIS installer (`installer.nsi`). Output: `ClippiSetup-{version}.exe`.
- **macOS aarch64 + x86_64**: Builds `.app` bundle with ad-hoc codesigning (`--requirements '=designated => identifier "com.clippi.app"'`). Output: `.dmg` disk image.
- Creates GitHub Release with all platform assets attached.

### NSIS Installer (`installer.nsi`)
- Auto-update launches installer via `ShellExecuteW` with `runas` — user completes the wizard manually
- Start menu and desktop shortcuts; uninstall with optional data purge
- Taskkill fallback for in-process app termination during upgrade

### macOS Bundle Metadata (`Cargo.toml [package.metadata.bundle]`)
- Bundle identifier: `com.clippi.app`, minimum macOS 12.0
- App icon: `assets/LOGO.icns`, category: Utility

## Tests

All tests are `#[cfg(test)]` inline unit tests within source modules — no separate `tests/` integration test directory. Key areas covered:

- `src/core/sync.rs` — 28+ tests for sync merge logic, tombstone handling, snapshot hashing
- `src/core/db.rs` — 11 tests for CRUD operations, upsert, filter queries
- `src/core/color.rs` — 10+ tests for color parsing and normalization
- `src/core/settings.rs` — 6 tests for TOML serialization and legacy field migration
- `src/core/paths.rs` — 5 tests for legacy migration and image merging
- `src/state/app.rs` — 18+ tests for entity state mutations

Run all tests with `cargo test`. No dev-dependencies required — all test-only dependencies are in-tree.

## Code Conventions

- Window size uses constants `DEFAULT_WINDOW_WIDTH` / `DEFAULT_WINDOW_HEIGHT` from `core::frontend`
- GPUI state management: `Entity<T>` model — `AppState` is the root entity, child views access it via `cx.read_entity()` / `cx.update_entity()`
- `Arc<AtomicBool>` is used for cross-thread coordination flags shared between GPUI main thread and platform listener threads (clipboard, focus)
- Win32 API type aliases (HICON, HBRUSH) are marked `#[allow(clippy::upper_case_acronyms)]` to match Windows SDK naming
- Platform-specific code is behind `#[cfg(target_os = "...")]` with separate modules per platform
- GPUI `Render` impls delegate to sub-functions (e.g., `render_type_picker()`, `render_local_form()`) — avoid heavy logic directly in `render()`
- `expect()` is used for Mutex lock poisoning (treated as unrecoverable)
- `log::error!` / `log::warn!` for non-critical failures (replaces `eprintln!`)
- Clippy clean: zero warnings enforced
