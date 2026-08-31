//! Window manager — unified window state, positioning, and poll loop.
//!
//! --- Owns the window lifecycle: show/hide, activate, position calculation, ---
//! --- auto-hide on focus loss, and hotkey-triggered show. Replaces the ---
//! Slint-era `Frontend` + `FocusService` + `HotkeyService` + `Looper` combo.

#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Datelike;
use gpui::*;

use crate::core::frontend::{
    clamp_to_work_area, PositionMode, DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH, PANEL_OFFSET_X,
    SUPPRESS_DURATION_MS,
};
use crate::core::i18n_keys::I18nKey;
use crate::platform::focus::{start_focus_watcher, FocusWatcher};
use crate::platform::hotkey::{
    create_hotkey_listener, hotkey_display, HotkeyEvent, HotkeyListener, HotkeyRecordingPress,
    QuickAction,
};
use crate::platform::monitor;
use crate::platform::tray::{TrayAction, TrayManager};
#[cfg(target_os = "windows")]
use crate::platform::windows_hotkeys;
use crate::services::config_sync::{ConfigSyncResult, ConfigSyncService};
use crate::services::gpui_clipboard::GpuiClipboardService;
use crate::services::gpui_sync::GpuiSyncService;
use crate::services::transfer_station::GpuiTransferService;
use crate::services::update;
use crate::state::app::AppState;
#[cfg(target_os = "macos")]
use crate::ui::quick_paste::QUICK_WINDOW_CORNER_RADIUS;
use crate::ui::quick_paste::{
    calc_quick_window_height, QuickPasteEvent, QuickPasteView, QUICK_WINDOW_WIDTH,
};

/// Shared foreground app name for cross-service coordination.
pub type ForegroundAppName = Arc<Mutex<String>>;

/// Result from a completed data maintenance job.
#[derive(Debug, Clone)]
pub enum DataMaintenanceResult {
    Cleanup {
        stats: crate::core::cache_cleanup::CleanupStats,
        cache_marker: Option<String>,
        retention_marker: Option<String>,
        show_toast: bool,
    },
    ClearClipboard {
        stats: crate::core::cache_cleanup::ClearClipboardStats,
        cache_marker: Option<String>,
    },
    Failed(String),
}

/// Backoff applied after a partially failed maintenance job. The 200ms poll
/// loop would otherwise re-trigger the job immediately because the success
/// markers are not advanced.
const MAINTENANCE_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(3600);

/// Trailing-edge debounce before window geometry is persisted after a change.
const GEOMETRY_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);

fn geometry_retry_delay(retry_count: u32) -> Duration {
    let backoff_secs = (1u64 << retry_count.saturating_sub(1).min(5)).min(30);
    Duration::from_secs(backoff_secs)
}

/// Visibility-guard decision for one window (04-spec v2 G2, issue-75).
///
/// Pure and cross-platform so `#[cfg(test)]` can exercise the full
/// state-bit × hwnd-validity × HWND-visibility combination table (AC2)
/// without a live window. It reads only the passed inputs and never
/// consults or writes any Clippi state, so no parallel visibility flag is
/// introduced — `visible` / `quick_visible` remain the only visibility
/// truth (G2/G6).
///
/// Returns `true` (re-hide) exactly when the Clippi state bit says the
/// window should be hidden but the HWND is actually visible:
/// `!state_visible && hwnd_valid && hwnd_visible`. An invalid `hwnd`
/// (`hwnd == 0`) always yields `false` and is safely skipped.
///
/// This cross-platform pure helper is referenced only by the Windows-only
/// guard (`poll_visibility_guard`) and `#[cfg(test)]` modules; on macOS /
/// Linux non-test builds it is intentionally unused, so silence the
/// warn-by-default `dead_code` lint (repo convention: zero warnings).
#[allow(dead_code)]
fn visibility_guard_should_hide(state_visible: bool, hwnd_valid: bool, hwnd_visible: bool) -> bool {
    hwnd_valid && !state_visible && hwnd_visible
}

pub struct WebDavBackendForm {
    pub name: String,
    pub root_url: String,
    pub path: String,
    pub username: String,
    pub password: String,
}

#[cfg(target_os = "windows")]
static BLOCK_SYSTEM_WINDOW_BEHAVIORS: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "windows")]
static ORIGINAL_WNDPROC: AtomicIsize = AtomicIsize::new(0);
#[cfg(target_os = "windows")]
static IN_MOVE_OPERATION: AtomicBool = AtomicBool::new(false);

/// Convert a desired quick-window client size to the HWND's outer size,
/// plus the client-area position offset inside the native window.
///
/// `SetWindowPos` sizes the complete native window, while GPUI lays out inside
/// the client area. GPUI's `WM_NCCALCSIZE` handler applies asymmetric insets
/// (left/right/bottom = frame_thickness, top = 0–1 px) even for borderless
/// popups, shifting the client origin.  The returned offsets let the caller
/// compensate the window position so the rendered content lands at the
/// intended screen coordinates.
#[cfg(target_os = "windows")]
fn quick_outer_size_for_client(
    hwnd: *mut std::ffi::c_void,
    client_width: i32,
    client_height: i32,
) -> (i32, i32, i32, i32) {
    // ^ (outer_width, outer_height, client_left_offset, client_top_offset)
    use windows_sys::Win32::Foundation::{GetLastError, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

    let mut window_rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut client_rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `hwnd` is the quick window owned by this process and both APIs
    // only write to the provided stack-allocated RECT values.
    let measured = unsafe {
        GetWindowRect(hwnd, &mut window_rect) != 0 && GetClientRect(hwnd, &mut client_rect) != 0
    };
    if !measured {
        log::warn!(
            "quick window frame measurement failed: win32 error {}",
            unsafe { GetLastError() }
        );
        return (client_width, client_height, 0, 0);
    }

    let frame_width =
        ((window_rect.right - window_rect.left) - (client_rect.right - client_rect.left)).max(0);
    let frame_height =
        ((window_rect.bottom - window_rect.top) - (client_rect.bottom - client_rect.top)).max(0);

    // Measure where the client origin sits on screen relative to the window
    // origin.  On the first call (window still hidden) the offset is typically
    // (0,0); after `WM_NCCALCSIZE` has run it reflects the actual insets.
    // SAFETY: `hwnd` is our window; `ClientToScreen` writes to stack POINT.
    let mut client_origin = POINT { x: 0, y: 0 };
    let left_offset = unsafe { ClientToScreen(hwnd, &mut client_origin) }
        .checked_sub(1) // 0 means failure
        .map(|_| (client_origin.x - window_rect.left).max(0))
        .unwrap_or(0);
    let top_offset = (client_origin.y - window_rect.top).max(0);

    (
        client_width + frame_width,
        client_height + frame_height,
        left_offset,
        top_offset,
    )
}

/// Move and size the quick window without holding GPUI's application borrow.
///
/// Win32 delivers `WM_DPICHANGED`, `WM_SIZE`, and `WM_MOVE` synchronously from
/// `SetWindowPos`. Calling it from an entity update prevents GPUI's native
/// callbacks from borrowing the app to synchronize `Window::scale_factor()`.
#[cfg(target_os = "windows")]
fn position_quick_window_windows(
    hwnd: isize,
    x: i32,
    y: i32,
    quick_h: f32,
    scale: f32,
    compensate_client_offset: bool,
) {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    };

    let hwnd = hwnd as *mut std::ffi::c_void;
    if hwnd.is_null() {
        return;
    }

    let client_w = (QUICK_WINDOW_WIDTH * scale) as i32;
    let client_h = (quick_h * scale) as i32;
    let (win_w, win_h, left_offset, top_offset) =
        quick_outer_size_for_client(hwnd, client_w, client_h);
    let (left_offset, top_offset) = if compensate_client_offset {
        (left_offset, top_offset)
    } else {
        (0, 0)
    };

    let positioned = unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x - left_offset,
            y - top_offset,
            win_w,
            win_h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
    if positioned == 0 {
        log::warn!("failed to position quick window: win32 error {}", unsafe {
            GetLastError()
        });
    }
}

/// Window subclass procedure that intercepts system behaviors while preserving
/// manual resize. Only active when `BLOCK_SYSTEM_WINDOW_BEHAVIORS` is true.
///
/// Handles:
/// - `WM_NCLBUTTONDBLCLK` on `HTCAPTION` → suppress double-click maximize
/// - `WM_WINDOWPOSCHANGING` during move → set `SWP_NOSIZE` to prevent Aero Snap
#[cfg(target_os = "windows")]
unsafe extern "system" fn clippi_subclass_proc(
    hwnd: *mut std::ffi::c_void,
    msg: u32,
    w_param: usize,
    l_param: isize,
) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HTCAPTION, SWP_NOSIZE, WINDOWPOS, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCLBUTTONDBLCLK,
        WM_WINDOWPOSCHANGING,
    };

    let original = ORIGINAL_WNDPROC.load(Ordering::Acquire);

    if BLOCK_SYSTEM_WINDOW_BEHAVIORS.load(Ordering::Acquire) {
        match msg {
            // Suppress double-click maximize on the title bar
            WM_NCLBUTTONDBLCLK if w_param == HTCAPTION as usize => {
                return 0;
            }
            // Track whether we're in a move operation (started from title bar drag)
            WM_ENTERSIZEMOVE => {
                IN_MOVE_OPERATION.store(w_param == HTCAPTION as usize, Ordering::Release);
            }
            WM_EXITSIZEMOVE => {
                IN_MOVE_OPERATION.store(false, Ordering::Release);
            }
            // Prevent Aero Snap during move operations — during a pure move
            // (title bar drag), any size change indicates snap, which we block.
            WM_WINDOWPOSCHANGING if IN_MOVE_OPERATION.load(Ordering::Acquire) => {
                let wp = &mut *(l_param as *mut WINDOWPOS);
                wp.flags |= SWP_NOSIZE;
            }
            _ => {}
        }
    }

    // Forward to original window procedure
    let orig_fn: unsafe extern "system" fn(*mut std::ffi::c_void, u32, usize, isize) -> isize =
        std::mem::transmute(original);
    orig_fn(hwnd, msg, w_param, l_param)
}

/// Events emitted by WindowManager for consumption by RootView.
pub enum WindowManagerEvent {
    /// Clipboard data changed; RootView should refresh its list.
    ClipboardChanged,
    /// Transfer activity changed without changing list contents.
    TransferStateChanged,
    /// Pin state changed (unpinned on hotkey show, or toggled by titlebar).
    PinnedChanged(bool),
    /// Tray menu "Settings" clicked — switch to settings view.
    /// TODO: Implement when settings panel GPUI migration is complete.
    OpenSettings,
    /// Hotkey recording completed (success or error) — RootView should
    /// notify SettingsPanel to re-render with updated hotkey / recording state.
    HotkeyRecordingComplete,
    /// Sync backend status or settings changed.
    SyncChanged,
    /// A background source-file existence probe completed; repaint cards
    /// without replacing the list or resetting selection.
    FileStatusChanged,
    /// Paste shortcut recording completed for an app.
    PasteShortcutRecorded {
        app_name: String,
        shortcut: String,
    },
    /// Window was hidden — RootView should dismiss all floating panels.
    WindowHidden,
    /// Memory release requested — subscribers must synchronously drop their
    /// live objects (list items, image caches) before the allocator pressure
    /// relief runs in the next app update.
    ReleaseUiResources,
    /// Main window DPI changed — RootView should force a re-render.
    #[cfg(target_os = "windows")]
    DpiChanged,
    /// Open settings and switch to the version tab.
    OpenVersionSettings,
    /// An update is available — RootView should show a notification toast.
    UpdateAvailable,
    /// Update progress changed — RootView should refresh toast / settings page.
    UpdateProgress(update::UpdatePhase),
    BitmapPasteFinished,
    /// A background data maintenance task completed or failed.
    DataMaintenanceToast(String),
    /// Reset RootView to clipboard history page (when always_reset_to_clipboard is on).
    ResetToClipboard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateCheckMode {
    Manual,
    Scheduled,
}

fn cleanup_marker_for_interval(interval: &str, today: chrono::NaiveDate) -> String {
    match interval {
        "weekly" => {
            let wk = today.iso_week();
            format!("{}-W{:02}", wk.year(), wk.week())
        }
        _ => today.format("%Y-%m-%d").to_string(),
    }
}

fn cache_cleanup_due(interval: &str, last_marker: &str, today: chrono::NaiveDate) -> bool {
    interval != "never" && cleanup_marker_for_interval(interval, today) != last_marker
}

fn retention_cleanup_due(retention_days: u32, last_date: &str, today: chrono::NaiveDate) -> bool {
    retention_days > 0 && today.format("%Y-%m-%d").to_string() != last_date
}

fn transfer_cleanup_due(
    transfer_available: bool,
    retention_days: u32,
    last_date: &str,
    today: chrono::NaiveDate,
) -> bool {
    transfer_available && retention_cleanup_due(retention_days, last_date, today)
}

/// Non-persistent runtime status for the Win+V takeover feature (Windows only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum WinVTakeoverStatus {
    Disabled,
    RegistryUpdateRequired,
    HotkeyUnavailable,
    Active,
    RegistryError,
}

/// Master switch for macOS hidden-surface compaction (doc §8.3 rollback).
/// When enabled, hidden windows are resized to 1×1 so GPUI's MetalRenderer
/// drops its large drawable-sized intermediate textures; when disabled, the
/// previous `orderOut + release_memory` behavior is kept unchanged.
#[cfg(target_os = "macos")]
const MACOS_SURFACE_COMPACTION_ENABLED: bool = true;

/// Saved state needed to restore a macOS window after hidden-surface
/// compaction (doc §3.1). Pure data — no Objective-C objects.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct MacosCompactedWindowState {
    /// Logical content size captured before compaction, in points.
    content_size: (f64, f64),
    /// Original content minimum size, restored before un-compacting.
    content_min_size: (f64, f64),
    /// Geometry confirmed before compaction, for shutdown persistence (doc §4.4).
    saved_geometry: (i32, i32, f32, f32),
    /// Generation of the latest compact/restore operation for this window.
    /// Async tasks compare it with `WindowManager::surface_generation` so a
    /// stale operation cannot mutate a newer window state (doc §4.1).
    generation: u64,
}

/// Unified window manager entity.
///
/// Owns the window lifecycle and all cross-service polling. Created once
/// in `main.rs` and stored as an `Entity<WindowManager>`. RootView
/// subscribes to clipboard and pin events.
pub struct WindowManager {
    // --- Window state ---
    position_mode: PositionMode,
    /// When true, the next `calculate_position()` forces "remember" mode.
    /// Set by tray actions so the window opens at the last-saved position
    /// instead of following the cursor (which is near the tray at the screen edge).
    tray_triggered: bool,
    pinned: bool,
    auto_hide: bool,
    visible: bool,
    suppress_until: Option<Instant>,

    // --- Platform resources ---
    hotkey: Option<Box<dyn HotkeyListener>>,
    /// Runtime status for Win+V takeover (Windows only).
    win_v_takeover_status: WinVTakeoverStatus,
    /// Timestamp of last periodic registry re-check for Win+V takeover.
    #[cfg(target_os = "windows")]
    last_win_v_recheck: Option<Instant>,
    focus_watcher: Option<FocusWatcher>,
    foreground_app_name: ForegroundAppName,

    // --- Geometry cache (physical pixels on Windows, logical on macOS) ---
    saved_x: i32,
    saved_y: i32,
    saved_w: f32,
    saved_h: f32,

    // --- Debounced geometry persistence ---
    /// True while the in-memory geometry differs from what is on disk.
    geometry_dirty: bool,
    /// When the geometry last changed; flush happens 500 ms after this.
    geometry_last_change: Option<Instant>,
    /// Failed-write retry count (drives 1/2/4/8/16/30 s backoff).
    geometry_retry_count: u32,
    /// Earliest allowed retry after a failed write.
    geometry_next_retry: Option<Instant>,

    // --- Hotkey blacklist ---
    blacklist: Vec<String>,

    /// When Some(app_name), the current recording is for a paste shortcut (not global hotkey).
    pub recording_paste_shortcut_app: Option<String>,
    /// When Some(id), recording a per-item custom hotkey for this item.
    recording_item_hotkey_id: Option<i64>,
    /// Paste format selected during per-item hotkey recording.
    recording_item_hotkey_format: Option<String>,
    /// When Some(slot), recording a latest-N hotkey for this slot (0-9).
    recording_latest_slot: Option<usize>,

    // --- Dependencies ---
    state: Entity<AppState>,
    clipboard_service: GpuiClipboardService,
    sync_service: GpuiSyncService,
    transfer_service: GpuiTransferService,

    // --- Raw window handle (HWND on Windows) ---
    #[allow(dead_code)]
    hwnd: isize,
    #[cfg(target_os = "windows")]
    last_system_dpi: u32,
    #[cfg(target_os = "macos")]
    ns_window: isize,
    /// GPUI handle for the main window — needed to resize through the GPUI
    /// path so the MetalRenderer rebuilds its drawable-sized resources.
    #[cfg(target_os = "macos")]
    main_window: Option<AnyWindowHandle>,
    /// Non-None while the main window is hidden and compacted to 1×1.
    #[cfg(target_os = "macos")]
    main_compacted_state: Option<MacosCompactedWindowState>,
    /// Non-None while the Quick Paste window is hidden and compacted to 1×1.
    #[cfg(target_os = "macos")]
    quick_compacted_state: Option<MacosCompactedWindowState>,
    /// Bumped on every compact/restore so stale async surface tasks abort.
    #[cfg(target_os = "macos")]
    surface_generation: u64,
    /// Set once when a restore poll times out; disables further compaction so
    /// a broken restore path can never lock the user out of the window.
    #[cfg(target_os = "macos")]
    macos_compaction_disabled: bool,
    /// Keeps the pending compaction confirmation tasks alive (dropping a
    /// `Task` cancels it).
    #[cfg(target_os = "macos")]
    _main_compact_task: Option<Task<()>>,
    #[cfg(target_os = "macos")]
    _quick_compact_task: Option<Task<()>>,
    #[cfg(target_os = "macos")]
    _main_restore_task: Option<Task<()>>,
    #[cfg(target_os = "macos")]
    _quick_restore_task: Option<Task<()>>,
    quick_window: Option<AnyWindowHandle>,
    quick_view: Option<Entity<QuickPasteView>>,
    quick_visible: bool,
    /// Tracks the current mouse-button state so click-outside handling reacts
    /// once per press instead of repeatedly while a button is held.
    quick_mouse_down: bool,
    /// Prevents click-outside and hotkey toggle hides for a short period after
    /// the quick window is shown. This debounces double hotkey events arriving
    /// in the same poll tick and gives the async positioning task time to
    /// complete before the window can be dismissed.
    quick_suppress_until: Option<Instant>,
    _quick_subscription: Option<Subscription>,
    #[cfg(target_os = "windows")]
    quick_hwnd: isize,
    #[cfg(target_os = "macos")]
    quick_ns_window: isize,
    // --- Monitor topology cache (issue-75 P2, Windows only) ---
    /// Last seen monitor snapshot; compared every 200 ms poll to detect
    /// topology / work-area changes (04-spec C1). Seeded on the first poll
    /// without reacting.
    #[cfg(target_os = "windows")]
    last_monitor_snapshot: Option<monitor::MonitorSnapshot>,
    /// Bumped on every detected topology change. Async migration tasks
    /// capture it and abort when a newer change superseded them, so rapid
    /// re-plug sequences never apply stale positions (04-spec C5).
    #[cfg(target_os = "windows")]
    topology_generation: u64,
    /// Keeps the in-flight async monitor-migration positioning tasks alive
    /// (dropping a `Task` cancels it). One slot per window — main and quick
    /// are never visible simultaneously.
    #[cfg(target_os = "windows")]
    _main_monitor_migration_task: Option<Task<()>>,
    #[cfg(target_os = "windows")]
    _quick_monitor_migration_task: Option<Task<()>>,
    // --- Visibility guard log-dedup bookkeeping (issue-75 v2 G5) ---
    /// True while the main window is inside a continuous "state bit hidden
    /// but HWND visible" episode, so `log::warn!` fires once per episode
    /// instead of every 200 ms tick. NOT visibility truth — the only
    /// visibility truth remains `visible` / `quick_visible`; this field
    /// only throttles logging (G5) and never participates in a decision.
    #[cfg(target_os = "windows")]
    main_guard_episode_logged: bool,
    /// Same log-dedup bookkeeping for the quick window (issue-75 v2 G5).
    #[cfg(target_os = "windows")]
    quick_guard_episode_logged: bool,
    // --- System tray ---
    tray: Option<TrayManager>,

    // --- Poll task ---
    _poll_task: Option<Task<()>>,
    /// One-shot main-window show task. Native positioning must run outside an
    /// app/entity update so synchronous DPI callbacks can re-borrow GPUI.
    #[cfg(target_os = "windows")]
    _main_show_task: Option<Task<()>>,
    /// Fast poll for quick-action hotkeys when quick window is visible (16ms ≈ 60fps).
    _quick_poll_task: Option<Task<()>>,
    /// One-shot native positioning task. On Windows this must run outside an
    /// app/entity update so synchronous DPI callbacks can re-borrow GPUI.
    #[cfg(target_os = "windows")]
    _quick_position_task: Option<Task<()>>,
    /// Fast poll used only while recording a shortcut, so a short key press
    /// cannot fall between two ticks of the 200 ms service loop.
    _recording_poll_task: Option<Task<()>>,

    // --- Update state ---
    /// Pending update info from background check thread (consumed by poll loop).
    pending_update: Arc<Mutex<Option<update::UpdateInfo>>>,
    /// Pending update phase from background download/install thread.
    pending_update_phase: Arc<Mutex<update::UpdatePhase>>,
    /// Result of the restart installer launch (consumed by poll_update).
    pending_restart_result: Arc<Mutex<Option<Result<(), String>>>>,
    /// True while the background GitHub release check is running.
    update_check_running: Arc<AtomicBool>,
    /// Set by background cleanup when expired rows were removed and AppState should reload.
    pending_cleanup_refresh: Arc<AtomicBool>,
    /// Custom hotkeys deleted by background cleanup and waiting to be unregistered.
    pending_cleanup_hotkey_unregister: Arc<Mutex<Vec<i64>>>,
    /// If Some, a background maintenance job is running.
    maintenance_job_running: bool,
    /// Pending result from a completed maintenance job.
    pending_maintenance_result: Arc<Mutex<Option<DataMaintenanceResult>>>,
    /// When a maintenance job partially failed, retry no earlier than this
    /// instant (markers are not advanced on failure, so without a backoff
    /// the 200ms poll loop would re-trigger the job immediately).
    maintenance_retry_after: Option<Instant>,

    // ── Config sync ──
    config_sync_service: ConfigSyncService,
    /// Snapshot downloaded from the backend, awaiting user confirmation.
    config_sync_pending_snapshot: Option<crate::core::config_sync::ConfigSnapshot>,
}

impl EventEmitter<WindowManagerEvent> for WindowManager {}

impl WindowManager {
    /// Create the window manager and start all background services.
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let settings = state.read(cx).settings.clone();

        // --- Hotkey is NOT created here — it's deferred via init_hotkey() ---
        // --- so the user can't trigger show_and_focus before GPUI has ---
        // --- finished initialising its input/IME pipeline. ---

        // --- Initialize focus watcher ---
        let focus_watcher = match start_focus_watcher() {
            Ok(fw) => Some(fw),
            Err(e) => {
                log::error!("Failed to start focus watcher: {e}");
                None
            }
        };

        let foreground_app_name = Arc::new(Mutex::new(String::new()));

        let clipboard_service = GpuiClipboardService::new(settings.clipboard_app_blacklist.clone());
        let sync_service = GpuiSyncService::new(&settings, state.read(cx).sync_dirty.clone());
        let transfer_service = GpuiTransferService::new(&settings);
        let config_sync_service = ConfigSyncService::new();

        // --- Initialize tray ---
        let tray = Some(TrayManager::new());

        let mut wm = Self {
            position_mode: PositionMode::from_str(&settings.window_position_mode),
            tray_triggered: false,
            pinned: false,
            auto_hide: settings.auto_hide,
            visible: !settings.silent_start,
            suppress_until: Some(Instant::now() + Duration::from_millis(SUPPRESS_DURATION_MS)),
            hotkey: None,
            win_v_takeover_status: WinVTakeoverStatus::Disabled,
            #[cfg(target_os = "windows")]
            last_win_v_recheck: None,
            focus_watcher,
            foreground_app_name,
            saved_x: settings.saved_window_x,
            saved_y: settings.saved_window_y,
            saved_w: settings.saved_window_width,
            saved_h: settings.saved_window_height,
            geometry_dirty: false,
            geometry_last_change: None,
            geometry_retry_count: 0,
            geometry_next_retry: None,
            blacklist: settings.hotkey_blacklist.clone(),
            recording_paste_shortcut_app: None,
            recording_item_hotkey_id: None,
            recording_item_hotkey_format: None,
            recording_latest_slot: None,
            state,
            clipboard_service,
            sync_service,
            transfer_service,
            tray,
            hwnd: 0,
            #[cfg(target_os = "windows")]
            last_system_dpi: 0,
            #[cfg(target_os = "macos")]
            ns_window: 0,
            #[cfg(target_os = "macos")]
            main_window: None,
            #[cfg(target_os = "macos")]
            main_compacted_state: None,
            #[cfg(target_os = "macos")]
            quick_compacted_state: None,
            #[cfg(target_os = "macos")]
            surface_generation: 0,
            #[cfg(target_os = "macos")]
            macos_compaction_disabled: false,
            #[cfg(target_os = "macos")]
            _main_compact_task: None,
            #[cfg(target_os = "macos")]
            _quick_compact_task: None,
            #[cfg(target_os = "macos")]
            _main_restore_task: None,
            #[cfg(target_os = "macos")]
            _quick_restore_task: None,
            quick_window: None,
            quick_view: None,
            quick_visible: false,
            quick_mouse_down: false,
            quick_suppress_until: None,
            _quick_subscription: None,
            #[cfg(target_os = "windows")]
            quick_hwnd: 0,
            #[cfg(target_os = "macos")]
            quick_ns_window: 0,
            #[cfg(target_os = "windows")]
            last_monitor_snapshot: None,
            #[cfg(target_os = "windows")]
            topology_generation: 0,
            #[cfg(target_os = "windows")]
            _main_monitor_migration_task: None,
            #[cfg(target_os = "windows")]
            _quick_monitor_migration_task: None,
            #[cfg(target_os = "windows")]
            main_guard_episode_logged: false,
            #[cfg(target_os = "windows")]
            quick_guard_episode_logged: false,
            _poll_task: None,
            #[cfg(target_os = "windows")]
            _main_show_task: None,
            _quick_poll_task: None,
            #[cfg(target_os = "windows")]
            _quick_position_task: None,
            _recording_poll_task: None,
            pending_update: Arc::new(Mutex::new(None)),
            pending_update_phase: Arc::new(Mutex::new(update::UpdatePhase::Idle)),
            pending_restart_result: Arc::new(Mutex::new(None)),
            update_check_running: Arc::new(AtomicBool::new(false)),
            pending_cleanup_refresh: Arc::new(AtomicBool::new(false)),
            pending_cleanup_hotkey_unregister: Arc::new(Mutex::new(Vec::new())),
            maintenance_job_running: false,
            pending_maintenance_result: Arc::new(Mutex::new(None)),
            maintenance_retry_after: None,
            config_sync_service,
            config_sync_pending_snapshot: None,
        };

        // --- Share the batch_pasting flag with AppState so it can suppress ---
        // --- clipboard recording during batch paste operations. ---
        let batch_pasting = wm.clipboard_service.batch_pasting();
        // Share the skip_next flag — used for one-shot internal clipboard
        // --- writes (OCR paste, re-copy) that should be consumed by the ---
        // --- listener without creating a new history entry. ---
        let skip_next = wm.clipboard_service.skip_next();
        wm.state.update(cx, |s, _cx| {
            s.batch_pasting = batch_pasting;
            s.skip_next = skip_next;
        });

        // --- Start the unified poll loop ---
        wm.start_poll_loop(cx);

        wm
    }

    // --- Poll loop ---

    fn start_poll_loop(&mut self, cx: &mut Context<Self>) {
        self._poll_task = Some(cx.spawn(async move |weak_self, cx| loop {
            Timer::after(Duration::from_millis(
                crate::services::poll_loop::POLL_INTERVAL_MS,
            ))
            .await;
            let Some(this) = weak_self.upgrade() else {
                break;
            };
            if this.update(cx, |wm, cx| wm.poll(cx)).is_err() {
                break;
            }
        }));
    }

    /// Fast poll (16ms ≈ 60fps) for quick-action hotkeys when quick window is visible.
    /// Gives responsive keyboard navigation without the 200ms main poll delay.
    fn start_quick_poll(&mut self, cx: &mut Context<Self>) {
        self._quick_poll_task = Some(cx.spawn(async move |weak_self, cx| loop {
            Timer::after(Duration::from_millis(16)).await;
            let Some(this) = weak_self.upgrade() else {
                break;
            };
            let visible = this.update(cx, |wm, _cx| wm.quick_visible).unwrap_or(false);
            if !visible {
                break;
            }
            if this
                .update(cx, |wm, cx| {
                    wm.poll_hotkey(cx);
                    wm.poll_quick_click_outside(cx);
                })
                .is_err()
            {
                break;
            }
        }));
    }

    fn start_recording_poll(&mut self, cx: &mut Context<Self>) {
        self._recording_poll_task = Some(cx.spawn(async move |weak_self, cx| loop {
            Timer::after(Duration::from_millis(16)).await;
            let Some(this) = weak_self.upgrade() else {
                break;
            };
            let recording = this
                .update(cx, |wm, cx| {
                    wm.poll_recording(cx);
                    wm.hotkey
                        .as_ref()
                        .is_some_and(|hotkey| hotkey.is_recording())
                })
                .unwrap_or(false);
            if !recording {
                break;
            }
        }));
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        // --- 1. Hotkey press -> show window ---
        self.poll_hotkey(cx);

        // 2. Hotkey recording — check for completion
        self.poll_recording(cx);

        // --- 3. Hotkey blacklist — dynamic register/unregister ---
        self.poll_blacklist();

        // --- 3b. Clean up hotkeys for deleted items ---
        self.poll_hotkey_cleanup(cx);

        // --- 4. Clipboard changes -> update state + notify ---
        self.poll_clipboard(cx);

        self.poll_bitmap_paste(cx);

        // 5. Focus / auto-hide logic (also updates foreground app info in AppState)
        self.poll_focus(cx);

        // --- 6. Tray events ---
        self.poll_tray(cx);

        // 7. Capture window geometry for persistence
        self.capture_window_geometry(cx);
        self.poll_geometry_flush(cx);

        // --- 7b. Monitor topology change detection (Windows, issue-75 C1) ---
        // Periodic snapshot comparison inside the single 200 ms poll loop —
        // no second UI polling loop. Reacts only to actual topology /
        // work-area changes; hidden windows stay hidden (C2).
        #[cfg(target_os = "windows")]
        self.poll_monitor_topology(cx);

        // --- 7c. Visibility guard (Windows, issue-75 v2 G1) ---
        // Every tick, re-hide any window whose HWND was shown by an external
        // path (GPUI display-change handling, system behavior, future paths)
        // while the Clippi state bit says hidden (G1–G6). Runs inside the
        // single 200 ms poll loop — no second UI polling loop. A topology
        // change detected in 7b is covered by the guard in the same tick.
        #[cfg(target_os = "windows")]
        self.poll_visibility_guard(cx);

        // --- 8. Cloud sync ---
        self.poll_sync(cx);

        // --- 9. Auto-update ---
        self.poll_update(cx);

        // --- 10. Periodic cache cleanup ---
        self.poll_cleanup(cx);

        self.poll_cleanup_refresh(cx);

        // --- 10b. Collect maintenance job results ---
        self.poll_maintenance_result(cx);

        // --- 11. Transfer station status check ---
        self.poll_transfer(cx);

        // --- 12. Background file availability probes ---
        if crate::services::file_status::take_status_changed() {
            cx.emit(WindowManagerEvent::FileStatusChanged);
        }

        // --- 13. Periodic Win+V takeover registry re-check (Windows only) ---
        #[cfg(target_os = "windows")]
        self.periodic_win_v_recheck(cx);

        // --- 14. Config sync results ---
        self.poll_config_sync(cx);
    }

    fn poll_cleanup_refresh(&mut self, cx: &mut Context<Self>) {
        let hotkey_ids = self
            .pending_cleanup_hotkey_unregister
            .lock()
            .map(|mut ids| std::mem::take(&mut *ids))
            .unwrap_or_default();
        if !hotkey_ids.is_empty() {
            if let Some(ref mut hk) = self.hotkey {
                for id in hotkey_ids {
                    hk.unregister_item_hotkey(id);
                }
            }
        }
        if self.pending_cleanup_refresh.swap(false, Ordering::AcqRel) {
            self.state.update(cx, |state, _cx| {
                state.reload_items();
                state.reload_tags();
            });
            cx.emit(WindowManagerEvent::ClipboardChanged);
        }
    }

    /// Collect results from a completed background maintenance job.
    fn poll_maintenance_result(&mut self, cx: &mut Context<Self>) {
        let result = self
            .pending_maintenance_result
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());

        let Some(result) = result else {
            return;
        };

        self.maintenance_job_running = false;

        match result {
            DataMaintenanceResult::Cleanup {
                mut stats,
                cache_marker,
                retention_marker,
                show_toast,
            } => {
                let deleted_file_paths = std::mem::take(&mut stats.deleted_file_paths);
                // Success markers only advance for phases that completed
                // without failure (design §11 stage G / §16 Phase 0 #8).
                let cache_ok = stats.scan_complete && stats.cache_remove_failed == 0;
                let retention_ok = !stats.retention_failed;
                let need_retry = (cache_marker.is_some() && !cache_ok)
                    || (retention_marker.is_some() && !retention_ok);
                self.maintenance_retry_after = if need_retry {
                    Some(Instant::now() + MAINTENANCE_RETRY_BACKOFF)
                } else {
                    None
                };
                self.state.update(cx, |state, _cx| {
                    if stats.sync_dirty {
                        state.sync_dirty.store(true, Ordering::SeqCst);
                    }
                    if !deleted_file_paths.is_empty() {
                        state.clear_deleted_file_transfer_associations(deleted_file_paths);
                    }
                    if cache_ok {
                        if let Some(marker) = cache_marker {
                            state.settings.cleanup_last_date = marker;
                        }
                    }
                    if retention_ok {
                        if let Some(marker) = retention_marker {
                            state.settings.retention_cleanup_last_date = marker;
                        }
                    }
                    state.settings.save();
                });
                if stats.expired_items > 0 || stats.stale_items > 0 {
                    self.pending_cleanup_refresh.store(true, Ordering::Release);
                }
                if !stats.deleted_hotkey_item_ids.is_empty() {
                    if let Ok(mut ids) = self.pending_cleanup_hotkey_unregister.lock() {
                        ids.extend(stats.deleted_hotkey_item_ids.iter().copied());
                    }
                }
                if !stats.is_empty() || need_retry {
                    log::info!(
                        "cleanup: {} orphan images, {} unreferenced icons, {} expired tombstones, {} expired items, {} stale items ({} scanned, {} pending, {} protected, {} unknown, {} invalid metadata, {} removal failures; complete: {})",
                        stats.orphan_images,
                        stats.unreferenced_icons,
                        stats.expired_tombstones,
                        stats.expired_items,
                        stats.stale_items,
                        stats.stale_scanned,
                        stats.stale_pending_confirmation,
                        stats.stale_protected,
                        stats.stale_unknown,
                        stats.invalid_metadata,
                        stats.cache_remove_failed,
                        stats.scan_complete,
                    );
                }
                let message = Self::cleanup_toast_message(stats.is_empty(), need_retry);
                if show_toast {
                    cx.emit(WindowManagerEvent::DataMaintenanceToast(message));
                }
            }
            DataMaintenanceResult::ClearClipboard {
                mut stats,
                cache_marker,
            } => {
                let deleted_file_paths = std::mem::take(&mut stats.deleted_file_paths);
                // Post-clear cache maintenance failure must not advance the
                // cache marker either.
                let cache_ok = stats.scan_complete && stats.cache_remove_failed == 0;
                self.maintenance_retry_after = if !cache_ok {
                    Some(Instant::now() + MAINTENANCE_RETRY_BACKOFF)
                } else {
                    None
                };
                self.state.update(cx, |state, _cx| {
                    if stats.sync_dirty {
                        state.sync_dirty.store(true, Ordering::SeqCst);
                    }
                    if !deleted_file_paths.is_empty() {
                        state.clear_deleted_file_transfer_associations(deleted_file_paths);
                    }
                    if cache_ok {
                        if let Some(marker) = cache_marker {
                            state.settings.cleanup_last_date = marker;
                        }
                    }
                    state.settings.save();
                });
                if stats.deleted_items > 0 {
                    self.pending_cleanup_refresh.store(true, Ordering::Release);
                }
                if !stats.deleted_hotkey_item_ids.is_empty() {
                    if let Ok(mut ids) = self.pending_cleanup_hotkey_unregister.lock() {
                        ids.extend(stats.deleted_hotkey_item_ids.iter().copied());
                    }
                }
                log::info!(
                    "clear clipboard: {} items ({} favorites), {} orphan images, {} unreferenced icons ({} removal failures; complete: {})",
                    stats.deleted_items,
                    stats.deleted_favorites,
                    stats.orphan_images,
                    stats.unreferenced_icons,
                    stats.cache_remove_failed,
                    stats.scan_complete,
                );
                let message = if stats.deleted_items == 0 {
                    I18nKey::ToastClearDataEmpty.text().to_string()
                } else {
                    I18nKey::ToastClearDataDone.fmt(&[&stats.deleted_items.to_string()])
                };
                cx.emit(WindowManagerEvent::DataMaintenanceToast(message));
            }
            DataMaintenanceResult::Failed(e) => {
                log::error!("data maintenance failed: {e}");
                self.maintenance_retry_after = Some(Instant::now() + MAINTENANCE_RETRY_BACKOFF);
                cx.emit(WindowManagerEvent::DataMaintenanceToast(
                    I18nKey::ToastDataMaintenanceFailed.text().to_string(),
                ));
            }
        }
    }

    /// Choose the user-facing toast for a completed cleanup run. A failed
    /// scan or removal phase (`need_retry`) always reports the cleanup as
    /// incomplete — otherwise a silent failure would look like a successful
    /// (or empty) cleanup.
    fn cleanup_toast_message(stats_is_empty: bool, need_retry: bool) -> String {
        if need_retry {
            I18nKey::ToastCleanupFailed.text().to_string()
        } else if stats_is_empty {
            I18nKey::ToastCleanupNone.text().to_string()
        } else {
            I18nKey::ToastCleanupDone.text().to_string()
        }
    }

    /// Request an immediate cleanup from the settings UI.
    pub fn request_cleanup(&mut self, cx: &mut Context<Self>) -> bool {
        if self.maintenance_job_running {
            return false;
        }

        let settings = self.state.read(cx).settings.clone();
        let interval = settings.cleanup_interval.as_str();
        let today = chrono::Local::now().date_naive();
        let retention_days = settings.retention_days;

        let options = crate::core::cache_cleanup::CleanupOptions {
            clean_orphan_cache: true,
            clean_expired_tombstones: true,
            retention_days: if retention_days > 0 {
                Some(retention_days)
            } else {
                None
            },
            clean_stale_items: settings.cleanup_stale_items,
        };

        let db_path = settings.resolve_db_path();
        let sync_scope = crate::core::cache_cleanup::CleanupSyncScope {
            include_images: settings.sync_include_images,
            favorites_only: settings.sync_favorites_only,
            device_name: crate::services::backends::local_folder::hostname(),
        };
        let pending_result = self.pending_maintenance_result.clone();
        let cache_marker = cleanup_marker_for_interval(interval, today);
        let retention_marker = (retention_days > 0).then(|| today.format("%Y-%m-%d").to_string());
        self.maintenance_job_running = true;

        std::thread::spawn(move || {
            let result = match crate::core::db::Database::open(&db_path.to_string_lossy()) {
                Ok(db) => {
                    let stats = crate::core::cache_cleanup::run_cleanup_with_options(
                        &db,
                        &options,
                        Some(&sync_scope),
                    );
                    DataMaintenanceResult::Cleanup {
                        stats,
                        cache_marker: Some(cache_marker),
                        retention_marker,
                        show_toast: true,
                    }
                }
                Err(e) => {
                    log::error!("request_cleanup: failed to open DB: {e}");
                    DataMaintenanceResult::Failed(e.to_string())
                }
            };
            if let Ok(mut slot) = pending_result.lock() {
                *slot = Some(result);
            }
        });

        true
    }

    /// Request a clear-clipboard operation from the settings UI.
    pub fn request_clear_data(&mut self, include_favorites: bool, cx: &mut Context<Self>) -> bool {
        if self.maintenance_job_running {
            return false;
        }

        let settings = self.state.read(cx).settings.clone();
        let db_path = settings.resolve_db_path();
        let device_name = crate::services::backends::local_folder::hostname();
        let pending_result = self.pending_maintenance_result.clone();
        let today = chrono::Local::now().date_naive();
        let cache_marker = cleanup_marker_for_interval(settings.cleanup_interval.as_str(), today);
        self.maintenance_job_running = true;

        std::thread::spawn(move || {
            let result = match crate::core::db::Database::open(&db_path.to_string_lossy()) {
                Ok(db) => {
                    // Clear clipboard history in a transaction.
                    let clear_result =
                        match db.clear_clipboard_history(&device_name, include_favorites) {
                            Ok(r) => r,
                            Err(e) => {
                                log::error!("request_clear_data: clear failed: {e}");
                                if let Ok(mut slot) = pending_result.lock() {
                                    *slot = Some(DataMaintenanceResult::Failed(format!(
                                        "Clear failed: {e}"
                                    )));
                                }
                                return;
                            }
                        };

                    // Post-clear cache maintenance.
                    let maintenance = crate::core::cache_cleanup::run_cache_maintenance(&db);

                    let stats = crate::core::cache_cleanup::ClearClipboardStats {
                        deleted_items: clear_result.deleted_items,
                        deleted_favorites: clear_result.deleted_favorites,
                        deleted_hotkey_item_ids: clear_result.deleted_hotkey_item_ids,
                        deleted_file_paths: clear_result.deleted_file_paths,
                        orphan_images: maintenance.orphan_images,
                        unreferenced_icons: maintenance.unreferenced_icons,
                        sync_dirty: clear_result.tombstones_written > 0,
                        cache_remove_failed: maintenance.cache_remove_failed,
                        scan_complete: maintenance.scan_complete,
                    };
                    // Only advance the cache marker when maintenance finished
                    // without failure; otherwise retry on a later cycle.
                    let cache_ok =
                        maintenance.scan_complete && maintenance.cache_remove_failed == 0;
                    DataMaintenanceResult::ClearClipboard {
                        stats,
                        cache_marker: cache_ok.then_some(cache_marker),
                    }
                }
                Err(e) => {
                    log::error!("request_clear_data: failed to open DB: {e}");
                    DataMaintenanceResult::Failed(e.to_string())
                }
            };
            if let Ok(mut slot) = pending_result.lock() {
                *slot = Some(result);
            }
        });

        true
    }

    /// Check if a maintenance job is currently running.
    pub fn is_maintenance_running(&self) -> bool {
        self.maintenance_job_running
    }

    // ── Config sync ──

    /// Collect completed config-sync results from the background thread.
    fn poll_config_sync(&mut self, cx: &mut Context<Self>) {
        let Some(result) = self.config_sync_service.take_result() else {
            return;
        };

        match result {
            ConfigSyncResult::Uploaded => {
                self.state.update(cx, |state, cx| {
                    state.show_toast(I18nKey::ConfigSyncToastUploaded.text());
                    cx.notify();
                });
                cx.emit(WindowManagerEvent::SyncChanged);
            }
            ConfigSyncResult::Downloaded(snapshot) => {
                self.config_sync_pending_snapshot = Some(*snapshot);
                cx.emit(WindowManagerEvent::SyncChanged);
            }
            ConfigSyncResult::Error(e) => {
                use crate::core::config_sync::ConfigSyncError;
                log::warn!("Config sync error: {e}");
                let msg = match &e {
                    ConfigSyncError::RemoteNotFound => {
                        I18nKey::ConfigSyncToastNotFound.text().to_string()
                    }
                    ConfigSyncError::Transport(detail) => {
                        I18nKey::ConfigSyncToastTransport.fmt(&[detail])
                    }
                    ConfigSyncError::TooLarge => {
                        I18nKey::ConfigSyncToastTooLarge.text().to_string()
                    }
                    ConfigSyncError::InvalidSnapshot(detail)
                    | ConfigSyncError::InvalidTimestamp(detail)
                    | ConfigSyncError::InvalidFieldValue(detail) => {
                        I18nKey::ConfigSyncToastInvalidSnapshot.fmt(&[detail])
                    }
                    ConfigSyncError::UnsupportedVersion(v) => {
                        I18nKey::ConfigSyncToastUnsupportedVersion.fmt(&[&v.to_string()])
                    }
                    other => format!("{other}"),
                };
                self.state.update(cx, |state, cx| {
                    state.show_warning_toast(msg);
                    cx.notify();
                });
                cx.emit(WindowManagerEvent::SyncChanged);
            }
        }
    }

    /// Whether a config-sync operation is currently in flight.
    pub fn is_config_sync_busy(&self) -> bool {
        self.config_sync_service.is_busy()
    }

    /// Get the downloaded snapshot (if any) awaiting user confirmation.
    pub fn config_sync_pending_snapshot(
        &self,
    ) -> Option<&crate::core::config_sync::ConfigSnapshot> {
        self.config_sync_pending_snapshot.as_ref()
    }

    /// Clear the pending snapshot (user dismissed the confirmation dialog).
    pub fn clear_config_sync_pending_snapshot(&mut self) {
        self.config_sync_pending_snapshot = None;
    }

    /// Start uploading local config to the given backend.
    pub fn start_config_upload(
        &mut self,
        backend: Arc<dyn crate::services::backends::ConfigSnapshotBackend>,
        cx: &mut Context<Self>,
    ) -> bool {
        let settings = self.state.read(cx).settings.clone();
        self.config_sync_service.start_upload(settings, backend)
    }

    /// Start downloading config from the given backend.
    pub fn start_config_download(
        &mut self,
        backend: Arc<dyn crate::services::backends::ConfigSnapshotBackend>,
    ) -> bool {
        self.config_sync_pending_snapshot = None;
        self.config_sync_service.start_download(backend)
    }

    /// Apply the downloaded snapshot: backup, merge, save, restart.
    pub fn apply_config_snapshot(&mut self, cx: &mut Context<Self>) {
        let snapshot = match self.config_sync_pending_snapshot.take() {
            Some(s) => s,
            None => {
                self.state.update(cx, |state, cx| {
                    state.show_warning_toast(I18nKey::ConfigSyncToastInvalidSnapshot.text());
                    cx.notify();
                });
                return;
            }
        };

        // Merge whitelist fields onto current settings.
        let current = self.state.read(cx).settings.clone();
        let merged = snapshot.settings.apply_to(&current);

        // Backup current config. Backup success is a precondition for
        // continuing — without it there is no way to recover the previous
        // settings if the merge or save fails.
        let config_path = crate::core::paths::config_path();
        let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let backup_path =
            config_path.with_file_name(format!("clippi.toml.before-cloud-apply.{now}.bak"));
        if let Err(e) = std::fs::copy(&config_path, &backup_path) {
            log::error!(
                "Failed to create config backup {}: {e}; aborting apply",
                backup_path.display()
            );
            self.state.update(cx, |state, cx| {
                state.show_warning_toast(I18nKey::ConfigSyncToastSaveFailed.text());
                cx.notify();
            });
            return;
        }
        log::info!("Config backup saved to {}", backup_path.display());

        // Save merged config to disk first, then commit it to memory so a
        // later `settings.save()` (debounced geometry flush, shutdown, any
        // setting change) cannot overwrite the freshly applied cloud config
        // with the stale in-memory copy.
        if let Err(e) = merged.save_atomic_to(&config_path) {
            log::error!("Failed to save merged config: {e}");
            self.state.update(cx, |state, cx| {
                state.show_warning_toast(I18nKey::ConfigSyncToastSaveFailed.text());
                cx.notify();
            });
            return;
        }
        self.state.update(cx, |state, _cx| {
            state.settings = merged.clone();
        });

        // Restart: only shut down when the new process actually started.
        // On spawn failure the merged config stays on disk and in memory
        // (recoverable from the backup), and the current process keeps
        // running — the next manual restart picks up the new config.
        match crate::core::settings::spawn_new_process() {
            Ok(()) => {
                log::info!("Restarting after cloud config apply");
                // Graceful shutdown: WAL checkpoint + geometry save run
                // with the merged settings already committed to AppState.
                self.prepare_shutdown(cx);
                cx.quit();
            }
            Err(e) => {
                log::error!("Restart after config apply failed: {e}");
                self.state.update(cx, |state, cx| {
                    state.show_warning_toast(I18nKey::ConfigSyncToastRestartFailed.text());
                    cx.notify();
                });
            }
        }
    }

    fn poll_transfer(&mut self, cx: &mut Context<Self>) {
        let window_visible = self.visible;
        let service = &mut self.transfer_service;
        let outcome = self
            .state
            .update(cx, |state, _cx| service.poll(state, window_visible));
        if outcome.data_changed {
            // Use TransferStateChanged to avoid scroll reset — the list items
            // are synced in-place via sync_items_from_state, matching tag ops.
            cx.emit(WindowManagerEvent::TransferStateChanged);
        } else if outcome.state_changed {
            cx.emit(WindowManagerEvent::TransferStateChanged);
        }
    }

    fn poll_hotkey(&mut self, cx: &mut Context<Self>) {
        loop {
            let event = self.hotkey.as_mut().and_then(|hk| hk.poll_event());
            let Some(event) = event else {
                break;
            };
            match event {
                HotkeyEvent::Main => self.show_and_focus(cx),
                HotkeyEvent::Quick => {
                    if self.quick_visible {
                        // Debounce: ignore toggle-hide if the window was just
                        // shown — a second hotkey event arriving in the same
                        // poll tick must not immediately dismiss the popup.
                        if self
                            .quick_suppress_until
                            .is_some_and(|until| Instant::now() <= until)
                        {
                            continue;
                        }
                        self.dismiss_quick_window(cx);
                    } else {
                        self.show_quick_window(cx);
                    }
                }
                HotkeyEvent::QuickAction(action) => self.handle_quick_action(action, cx),
                HotkeyEvent::CustomItem(item_id) => {
                    // Dismiss quick window if visible.
                    if self.quick_visible {
                        self.dismiss_quick_window(cx);
                    }
                    let state = self.state.clone();
                    let format = state.read(cx).get_item_hotkey_format(item_id);
                    state.update(cx, move |s, _cx| {
                        Self::paste_item_with_format(s, item_id, format);
                    });
                }
                HotkeyEvent::LatestItem(slot) => {
                    if self.quick_visible {
                        self.dismiss_quick_window(cx);
                    }
                    let state = self.state.clone();
                    let item_id = {
                        let s = state.read(cx);
                        s.latest_hotkey_item_id(slot)
                    };
                    let format = state
                        .read(cx)
                        .settings
                        .latest_hotkeys
                        .get(slot)
                        .and_then(|e| {
                            if e.paste_format.is_empty() {
                                None
                            } else {
                                serde_json::from_str(&e.paste_format).ok()
                            }
                        })
                        .unwrap_or(crate::core::types::HotkeyPasteFormat::Default);
                    if let Some(id) = item_id {
                        state.update(cx, move |s, _cx| {
                            Self::paste_item_with_format(s, id, format);
                        });
                    }
                }
            }
        }
    }

    /// Poll for hotkey recording completion.
    /// When the user is recording a new hotkey (initiated from the settings UI),
    /// check if they've pressed a key combo. On success, update the hotkey and
    /// persist to settings. On failure, store the error for toast display.
    ///
    /// Note: `start_hotkey_recording()` unregisters the current hotkey so that
    /// `poll_hotkey()` won't fire during recording. On completion the new
    /// hotkey is registered (via `update_hotkey`); on error the old hotkey is
    /// re-registered.
    fn poll_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(ref mut hk) = self.hotkey {
            // poll_recording_pressed() returns None when not recording —            // it checks the hotkey's internal is_recording flag directly,
            // --- avoiding any AppState synchronization gap. ---
            if let Some(recording_press) = hk.poll_recording_pressed() {
                let new_hotkey = match recording_press {
                    HotkeyRecordingPress::AwaitingSingle(candidate) => {
                        self.state.update(cx, |state, _cx| {
                            state.pending_single_hotkey = Some(candidate);
                        });
                        cx.notify();
                        return;
                    }
                    // A protected single key (letter/digit/space/...) was
                    // pressed — keep recording and explain why nothing was
                    // recorded.
                    HotkeyRecordingPress::Rejected => {
                        self.state.update(cx, |state, _cx| {
                            state.pending_single_hotkey = None;
                            state.toast_message =
                                Some(I18nKey::HotkeyProtectedSingleKey.text().to_string());
                            state.toast_is_warning = true;
                        });
                        return;
                    }
                    HotkeyRecordingPress::Cancel => {
                        self.recording_paste_shortcut_app = None;
                        self.recording_item_hotkey_id = None;
                        self.recording_item_hotkey_format = None;
                        self.recording_latest_slot = None;
                        hk.finish_recording();
                        hk.register();
                        self.state.update(cx, |state, _cx| {
                            state.hotkey_recording = false;
                            state.recording_quick_hotkey = false;
                        });
                        cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
                        cx.notify();
                        return;
                    }
                    HotkeyRecordingPress::Hotkey(new_hotkey) => {
                        self.state.update(cx, |state, _cx| {
                            state.pending_single_hotkey = None;
                        });
                        new_hotkey
                    }
                };
                // Check if recording for paste shortcut
                if let Some(app_name) = self.recording_paste_shortcut_app.take() {
                    hk.finish_recording();
                    hk.register();
                    if !new_hotkey.is_empty() {
                        cx.emit(WindowManagerEvent::PasteShortcutRecorded {
                            app_name,
                            shortcut: new_hotkey,
                        });
                    }
                    return;
                }

                // Check if recording for quick window hotkey
                let recording_quick = self.state.read(cx).recording_quick_hotkey;
                if recording_quick {
                    hk.finish_recording();
                    hk.register();
                    if !new_hotkey.is_empty() {
                        match hk.update_quick_hotkey(&new_hotkey) {
                            Ok(()) => {
                                self.state.update(cx, |state, _cx| {
                                    state.settings.quick_hotkey = new_hotkey;
                                    state.settings.save();
                                    state.recording_quick_hotkey = false;
                                });
                            }
                            Err(e) => {
                                self.state.update(cx, |state, _cx| {
                                    state.toast_message = Some(e);
                                    state.toast_is_warning = true;
                                    state.recording_quick_hotkey = false;
                                });
                            }
                        }
                    } else {
                        self.state.update(cx, |state, _cx| {
                            state.recording_quick_hotkey = false;
                            state.pending_single_hotkey = None;
                        });
                    }
                    cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
                    return;
                }

                // Check if recording for a per-item custom hotkey.
                if let Some(item_id) = self.recording_item_hotkey_id.take() {
                    let format = self.recording_item_hotkey_format.take().unwrap_or_default();
                    hk.finish_recording();
                    if !new_hotkey.is_empty() {
                        self.commit_item_hotkey(item_id, &new_hotkey, &format, cx);
                    } else {
                        self.cancel_custom_recording(cx);
                    }
                    return;
                }

                // Check if recording for a latest-N slot hotkey.
                if let Some(slot) = self.recording_latest_slot.take() {
                    hk.finish_recording();
                    if !new_hotkey.is_empty() {
                        self.commit_latest_hotkey(slot, &new_hotkey, cx);
                    } else {
                        self.cancel_custom_recording(cx);
                    }
                    return;
                }

                if !new_hotkey.is_empty() {
                    // Guide to takeover if user tries to record Win+V.
                    #[cfg(target_os = "windows")]
                    if new_hotkey.eq_ignore_ascii_case("Win+V")
                        && !self.state.read(cx).settings.replace_system_win_v
                    {
                        hk.finish_recording();
                        hk.register();
                        self.state.update(cx, |state, _cx| {
                            state.hotkey_recording = false;
                            state.toast_message =
                                Some(I18nKey::WinVGuideToTakeover.text().to_string());
                            state.toast_is_warning = true;
                        });
                        cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
                        return;
                    }

                    match hk.update_hotkey(&new_hotkey) {
                        Ok(()) => {
                            // --- update_hotkey already registered the new hotkey. ---
                            hk.finish_recording();
                            hk.register();
                            self.state.update(cx, |state, _cx| {
                                state.settings.hotkey = new_hotkey.clone();
                                state.settings.save();
                                state.hotkey_recording = false;
                            });
                            cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
                        }
                        Err(e) => {
                            // --- update_hotkey failed — the hotkey is still the ---
                            // --- old one and unregistered. Re-register it and ---
                            // --- show the error. ---
                            hk.finish_recording();
                            hk.register();
                            self.state.update(cx, |state, _cx| {
                                state.hotkey_recording = false;
                                state.toast_message = Some(e);
                                state.toast_is_warning = true;
                            });
                            cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
                        }
                    }
                }
            }
        }
    }

    fn poll_blacklist(&mut self) {
        let fg_name = self
            .foreground_app_name
            .lock()
            .ok()
            .map(|fg| fg.clone())
            .unwrap_or_default();

        // While Clippi's own window is foreground, unregister the hotkeys so a
        // single-key hotkey does not swallow input in Clippi's search box.
        // (update_foreground_app_name clears fg_name for self, so the blacklist
        // match alone can never cover this case.)
        let self_foreground = self.is_self_foreground();
        if self_foreground || (!fg_name.is_empty() && self.blacklist.contains(&fg_name)) {
            if let Some(ref mut hk) = self.hotkey {
                hk.unregister();
            }
        } else if let Some(ref mut hk) = self.hotkey {
            hk.register();
        }
    }

    /// Unregister custom hotkeys for deleted items.
    fn poll_hotkey_cleanup(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.state.update(cx, |state, _cx| {
            std::mem::take(&mut state.pending_hotkey_unregister)
        });
        if !ids.is_empty() {
            if let Some(ref mut hk) = self.hotkey {
                for id in ids {
                    hk.unregister_item_hotkey(id);
                }
            }
        }
    }

    fn poll_clipboard(&mut self, cx: &mut Context<Self>) {
        let changed = self
            .state
            .update(cx, |state, _cx| self.clipboard_service.poll_state(state));
        if changed {
            cx.emit(WindowManagerEvent::ClipboardChanged);
            // Async thumbnail generation completed — the main list refreshes
            // via ClipboardChanged, but the Quick Paste popup needs an
            // explicit redraw so placeholders are replaced by thumbnails.
            if self.quick_visible {
                if let Some(view) = self.quick_view.clone() {
                    view.update(cx, |view, cx| view.notify_thumbnail_ready(cx));
                }
            }
        }
    }

    fn poll_bitmap_paste(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).take_bitmap_paste_finished() {
            cx.emit(WindowManagerEvent::BitmapPasteFinished);
        }
    }

    fn poll_sync(&mut self, cx: &mut Context<Self>) {
        let sync_service = &mut self.sync_service;
        let outcome = self.state.update(cx, |state, _cx| sync_service.poll(state));
        if outcome.data_changed {
            cx.emit(WindowManagerEvent::ClipboardChanged);
        }
        if outcome.state_changed {
            cx.emit(WindowManagerEvent::SyncChanged);
        }
    }

    fn poll_focus(&mut self, cx: &mut Context<Self>) {
        // Update foreground app name for blacklist
        self.update_foreground_app_name(cx);

        let is_self_fg = self.is_self_foreground();

        // --- Auto-hide logic for main window ---
        // --- Guard conditions: any true → skip auto-hide ---
        if !self.auto_hide || self.pinned || !self.visible || self.is_suppressed() || is_self_fg {
            return;
        }

        self.hide(cx);
    }

    /// Hide the quick window if the user clicks outside its bounds.
    fn poll_quick_click_outside(&mut self, cx: &mut Context<Self>) {
        if !self.quick_visible {
            return;
        }
        // Debounce: ignore click-outside during the suppress window right after
        // show, before the async positioning task has completed.
        if self
            .quick_suppress_until
            .is_some_and(|until| Instant::now() <= until)
        {
            return;
        }

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Foundation::{POINT, RECT};
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
            use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};

            const VK_LBUTTON: i32 = 0x01;
            const VK_RBUTTON: i32 = 0x02;
            const VK_MBUTTON: i32 = 0x04;
            const VK_SHIFT: i32 = 0x10;
            const VK_CONTROL: i32 = 0x11;

            // Check both the current down bit and the latched "pressed since last
            // poll" bit. The latter catches short clicks that begin and end
            // between two 16 ms quick-loop ticks.
            unsafe {
                let states =
                    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].map(|vkey| GetAsyncKeyState(vkey));
                let mouse_down = states.iter().any(|state| (state & i16::MIN) != 0);
                let pressed = states.iter().any(|state| (state & 0x0001) != 0)
                    || (mouse_down && !self.quick_mouse_down);
                self.quick_mouse_down = mouse_down;

                // ── Modifier key state ──
                let shift_held = (GetAsyncKeyState(VK_SHIFT) & i16::MIN) != 0;
                let ctrl_held = (GetAsyncKeyState(VK_CONTROL) & i16::MIN) != 0;
                if let Some(ref view) = self.quick_view {
                    view.update(cx, |view, cx| {
                        view.set_modifiers(shift_held, ctrl_held, cx);
                    });
                }

                if pressed {
                    // Button activity since last poll — check cursor position.
                    let mut cursor = POINT { x: 0, y: 0 };
                    if GetCursorPos(&mut cursor) != 0 {
                        let hwnd = self.quick_hwnd as *mut std::ffi::c_void;
                        let mut rect = RECT {
                            left: 0,
                            top: 0,
                            right: 0,
                            bottom: 0,
                        };
                        if !hwnd.is_null()
                            && GetWindowRect(hwnd, &mut rect) != 0
                            && (cursor.x < rect.left
                                || cursor.x >= rect.right
                                || cursor.y < rect.top
                                || cursor.y >= rect.bottom)
                        {
                            self.dismiss_quick_window(cx);
                        }
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::NSEvent;

            let mouse_down = NSEvent::pressedMouseButtons() != 0;
            let pressed = mouse_down && !self.quick_mouse_down;
            self.quick_mouse_down = mouse_down;

            // ── Modifier key state ──
            {
                let flags = NSEvent::modifierFlags_class();
                let shift_held = flags.contains(objc2_app_kit::NSEventModifierFlags::Shift);
                let ctrl_held = flags.contains(objc2_app_kit::NSEventModifierFlags::Control)
                    || flags.contains(objc2_app_kit::NSEventModifierFlags::Command);
                if let Some(ref view) = self.quick_view {
                    view.update(cx, |view, cx| {
                        view.set_modifiers(shift_held, ctrl_held, cx);
                    });
                }
            }

            if !pressed || self.quick_ns_window == 0 {
                return;
            }

            // `NSEvent::mouseLocation` and `NSWindow::frame` both use the native
            // bottom-left global coordinate space, so this remains correct on
            // secondary displays without doing a top-left conversion.
            let window = unsafe { &*(self.quick_ns_window as *const objc2_app_kit::NSWindow) };
            let cursor = NSEvent::mouseLocation();
            let frame = window.frame();
            let outside = cursor.x < frame.origin.x
                || cursor.x >= frame.origin.x + frame.size.width
                || cursor.y < frame.origin.y
                || cursor.y >= frame.origin.y + frame.size.height;
            if outside {
                self.dismiss_quick_window(cx);
            }
        }
    }

    // --- Tray event polling ---

    fn poll_tray(&mut self, cx: &mut Context<Self>) {
        let action = match self.tray.as_ref() {
            Some(t) => t.poll(),
            None => return,
        };

        match action {
            Some(TrayAction::Show) => {
                self.tray_triggered = true;
                self.show_and_focus(cx);
                self.tray_triggered = false;
            }
            Some(TrayAction::OpenSettings) => {
                if !self.visible {
                    self.tray_triggered = true;
                    self.show_and_focus(cx);
                    self.tray_triggered = false;
                }
                cx.emit(WindowManagerEvent::OpenSettings);
            }
            Some(TrayAction::Restart) => {
                self.do_restart(cx);
            }
            Some(TrayAction::Quit) => {
                self.do_quit(cx);
            }
            Some(TrayAction::CheckUpdate) => {
                // Emit OpenVersionSettings BEFORE start_update_check so the
                // RootView switches to the version tab first. Otherwise the
                // UpdateProgress(Checking) event arrives while current_view is
                // still "clipboard" and the on-version-tab suppression fails.
                cx.emit(WindowManagerEvent::OpenVersionSettings);
                self.start_update_check(cx);
                if !self.visible {
                    self.tray_triggered = true;
                    self.show_and_focus(cx);
                    self.tray_triggered = false;
                }
            }
            None => {}
        }
    }

    /// Capture the current window size from the platform and update
    /// saved_w / saved_h. Called from the poll loop while the window
    /// is visible.
    fn capture_window_geometry(&mut self, cx: &mut Context<Self>) {
        #[cfg(not(target_os = "windows"))]
        let _ = cx;

        if !self.visible {
            return;
        }

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Foundation::RECT;
            use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
            use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

            if self.hwnd == 0 {
                return;
            }
            let hwnd = self.hwnd as *mut std::ffi::c_void;

            // Detect DPI changes and force layout invalidation.
            let current_dpi = unsafe { GetDpiForWindow(hwnd) } as u32;
            if current_dpi != self.last_system_dpi && self.last_system_dpi != 0 {
                self.last_system_dpi = current_dpi;
                cx.emit(WindowManagerEvent::DpiChanged);
                // GPUI has already handled WM_DPICHANGED and WM_SIZE. Emitting
                // the event is sufficient to invalidate Clippi's own views;
                // synchronously nudging the HWND here would re-enter GPUI while
                // the application state is borrowed.
                return;
            }
            self.last_system_dpi = current_dpi;

            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let ok = unsafe { GetWindowRect(hwnd, &mut rect) };
            if ok == 0 {
                return;
            }
            let phys_w = rect.right - rect.left;
            let phys_h = rect.bottom - rect.top;
            if phys_w <= 0 || phys_h <= 0 {
                return;
            }

            // Convert physical → logical using window's actual DPI.
            let scale = current_dpi as f32 / 96.0;
            let logical_w = phys_w as f32 / scale;
            let logical_h = phys_h as f32 / scale;

            let phys_x = rect.left;
            let phys_y = rect.top;

            let changed = (self.saved_w - logical_w).abs() > 1.0
                || (self.saved_h - logical_h).abs() > 1.0
                || self.saved_x != phys_x
                || self.saved_y != phys_y;

            if changed {
                self.saved_w = logical_w;
                self.saved_h = logical_h;
                self.saved_x = phys_x;
                self.saved_y = phys_y;

                self.mark_geometry_dirty();
            }
        }

        #[cfg(target_os = "macos")]
        {
            // Never read geometry from a compacted (1×1) window — persist only
            // pre-compaction geometry (doc §4.2). `prepare_shutdown` restores
            // the saved values when exiting in compacted state.
            if self.ns_window == 0 || self.main_compacted_state.is_some() {
                return;
            }
            let Some(primary_height) = monitor::primary_screen_height() else {
                return;
            };
            let frame = unsafe { (&*(self.ns_window as *const objc2_app_kit::NSWindow)).frame() };
            let rect = monitor::cocoa_rect_to_top_left(
                primary_height,
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            );
            if rect.width <= 0 || rect.height <= 0 {
                return;
            }

            let changed = self.saved_x != rect.x
                || self.saved_y != rect.y
                || (self.saved_w - rect.width as f32).abs() > 1.0
                || (self.saved_h - rect.height as f32).abs() > 1.0;
            if changed {
                self.saved_x = rect.x;
                self.saved_y = rect.y;
                self.saved_w = rect.width as f32;
                self.saved_h = rect.height as f32;
                self.mark_geometry_dirty();
            }
        }
    }

    /// Mark geometry dirty and schedule a trailing-edge flush 500 ms after
    /// the last change. New changes reset the debounce and the backoff.
    fn mark_geometry_dirty(&mut self) {
        self.geometry_dirty = true;
        self.geometry_last_change = Some(Instant::now());
        self.geometry_retry_count = 0;
        self.geometry_next_retry = None;
    }

    /// Poll-driven trailing-edge flush: writes settings only after the window
    /// geometry has been stable for `GEOMETRY_DEBOUNCE`. Failed writes keep
    /// the dirty flag and retry with 1/2/4/8/16/30 s backoff; a new geometry
    /// change resets the backoff via `mark_geometry_dirty`.
    fn poll_geometry_flush(&mut self, cx: &mut Context<Self>) {
        if !self.geometry_dirty {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.geometry_last_change.unwrap_or(now)) < GEOMETRY_DEBOUNCE {
            return;
        }
        if self.geometry_next_retry.is_some_and(|at| now < at) {
            return;
        }
        self.flush_geometry_now(cx);
    }

    /// Persist immediately and keep the debounce/retry state consistent for
    /// both poll-driven and forced (hide/shutdown) flushes.
    fn flush_geometry_now(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self.persist_geometry(cx) {
            self.geometry_dirty = false;
            self.geometry_last_change = None;
            self.geometry_retry_count = 0;
            self.geometry_next_retry = None;
        } else {
            self.geometry_dirty = true;
            if self.geometry_last_change.is_none() {
                self.geometry_last_change = Some(now);
            }
            self.geometry_retry_count = self.geometry_retry_count.saturating_add(1);
            self.geometry_next_retry = Some(now + geometry_retry_delay(self.geometry_retry_count));
        }
    }

    /// Force a geometry flush (hide / shutdown), bypassing the debounce.
    /// Serializes the latest in-memory `AppState.settings` at flush time so a
    /// config-sync apply (disk-then-memory) can never be overwritten by a
    /// stale settings copy.
    fn persist_geometry(&mut self, cx: &mut Context<Self>) -> bool {
        let (x, y, width, height) = (self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.state.update(cx, |state, _cx| {
            if width > 0.0 && height > 0.0 {
                state.settings.saved_window_x = x;
                state.settings.saved_window_y = y;
                state.settings.saved_window_width = width;
                state.settings.saved_window_height = height;
            }
            match state.settings.save_result() {
                Ok(()) => true,
                Err(e) => {
                    log::error!("Failed to save window geometry settings: {e}");
                    false
                }
            }
        })
    }

    // --- Monitor topology change handling (issue-75 P2, Windows only) ---
    //
    // Poll step 7b detects topology / work-area changes by comparing a cached
    // snapshot against a fresh enumeration, then — for already-visible
    // windows only — migrates them onto a remaining monitor (C3) and
    // re-applies the quick window's design bounds (C4). None of these paths
    // calls any show/focus entry (`show_and_focus` / `show_quick_window`,
    // AC1) and none writes the visibility state bits (C2).

    /// Current dynamic quick-window height from the visible bars, with the
    /// `QUICK_WINDOW_WIDTH` × `calc_quick_window_height` semantics unchanged
    /// (04-spec C4).
    fn current_quick_height(&self, cx: &mut Context<Self>) -> f32 {
        let state = self.state.read(cx);
        let pinned_tag_ids = &state.settings.pinned_tag_ids;
        let tags = &state.tags;
        let has_tag = pinned_tag_ids
            .iter()
            .any(|&id| tags.iter().any(|t| t.id == id));
        let has_type = !state.settings.type_filter_config.is_empty();
        calc_quick_window_height(has_tag, has_type)
    }

    /// C4 terminal positioning fallback for the quick window: when the caret /
    /// cursor monitor queries fail, resolve a deterministic position through
    /// the C3 fallback chain from a fresh monitor snapshot. Never returns
    /// `(0,0)`; `None` means no remaining monitor yields a valid target, in
    /// which case the caller keeps the window hidden (04-spec §4).
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn quick_position_c3_fallback(&self, quick_h: f32) -> Option<(i32, i32)> {
        let snapshot = monitor::enumerate_monitors()?;
        if snapshot.is_empty() {
            return None;
        }
        let target = monitor::pick_migration_target(&snapshot.monitors, monitor::get_cursor_pos())?;
        let scale = monitor::get_scale_factor(target.x, target.y);
        let win_w = (QUICK_WINDOW_WIDTH * scale) as i32;
        let win_h = (quick_h * scale) as i32;
        Some(clamp_to_work_area(
            target.x, target.y, win_w, win_h, &target,
        ))
    }

    /// Poll step 7b: compare the cached monitor snapshot with a fresh
    /// enumeration and react to actual topology / work-area changes (C1:
    /// disconnect, turn-off, signal loss, reconnect, resolution / work-area
    /// change). The snapshot is deterministic (position-sorted, P1), so an
    /// unchanged topology always compares equal and produces no reaction.
    #[cfg(target_os = "windows")]
    fn poll_monitor_topology(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = monitor::enumerate_monitors() else {
            // Enumeration failure: keep the cached snapshot — a transient
            // failure must not be treated as "no monitors".
            log::warn!("monitor topology: EnumDisplayMonitors failed; keeping last snapshot");
            return;
        };

        let Some(previous) = &self.last_monitor_snapshot else {
            // First poll: seed the cache without reacting — there is no
            // baseline to compare against yet.
            self.last_monitor_snapshot = Some(snapshot);
            return;
        };

        if previous == &snapshot {
            return;
        }

        log::info!(
            "monitor topology changed: {} monitor(s) now (was {})",
            snapshot.monitors.len(),
            previous.monitors.len()
        );
        self.topology_generation = self.topology_generation.wrapping_add(1);
        let generation = self.topology_generation;
        self.last_monitor_snapshot = Some(snapshot.clone());

        // C2: read-only visibility branches — hidden windows stay hidden.
        if self.visible {
            self.migrate_main_window_on_topology_change(&snapshot, generation, cx);
        }
        if self.quick_visible {
            self.reapply_quick_window_on_topology_change(&snapshot, generation, cx);
        }
    }

    /// Poll step 7c (Windows, issue-75 v2): visibility guard G1–G6.
    ///
    /// Every 200 ms tick, for each window, compares the Clippi state bit
    /// (`visible` / `quick_visible`) with the HWND's actual visibility
    /// (`IsWindowVisible`). When the state bit says hidden but the HWND is
    /// visible, an external path (GPUI 0.2.2 `handle_display_change_msg`
    /// unconditional `ShowWindow(SW_SHOWNORMAL)` — verified events.rs
    /// L842-844; system behavior; future paths) has shown the window
    /// against the state machine: re-hide it with the exact primitives the
    /// normal hide paths use (G3), leaving every state bit untouched (they
    /// are already false). Restores "state bit hidden ⇒ HWND invisible".
    ///
    /// G1: mounted right after the topology poll (7b) inside the single
    /// 200 ms poll loop — no second UI polling loop; a topology change
    /// detected in 7b is covered by the guard in the same tick.
    ///
    /// G4: the hide calls run synchronously inside the poll update. This is
    /// safe because gpui-0.2.2 handles every message they dispatch without
    /// synchronously re-borrowing the app (source-verified in events.rs):
    /// `WM_SHOWWINDOW(0)` returns `None` without a callback (L1180-1185),
    /// `WM_ACTIVATE` is dispatched through the executor (L749-764), and
    /// `WM_WINDOWPOSCHANGED`/`WM_WINDOWPOSCHANGING` have no GPUI handler
    /// (only Clippi's subclass touches them during title-bar drags). The
    /// `SWP_NOACTIVATE|SWP_NOMOVE|SWP_NOSIZE|SWP_NOZORDER` flags suppress
    /// the geometry messages that drive the documented DPI re-entrancy
    /// trap. No re-entrancy evidence found ⇒ synchronous path kept; if a
    /// future change introduces re-entrancy, switch to `cx.spawn()` +
    /// yield and re-check "state bit still hidden + hwnd unchanged" before
    /// hiding (mirroring the migration-task re-check pattern).
    ///
    /// G5: `log::warn!` fires once per continuous episode (`*_guard_episode_logged`
    /// toggles on first hit and resets when the window is consistent again) —
    /// never every tick.
    ///
    /// G6: the guard can only act while the state bit is false, so it is
    /// mutually exclusive by state bit with the C3 migration (true state)
    /// and the 16 ms quick poll (`quick_visible == true` only); under
    /// `silent_start` (`visible == false` from startup) the guard is active
    /// from the first poll tick. An invalid `hwnd` is skipped safely (G2).
    #[cfg(target_os = "windows")]
    fn poll_visibility_guard(&mut self, _cx: &mut Context<Self>) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            IsWindowVisible, SetWindowPos, ShowWindow, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
        };

        // --- Main window ---
        let main_hwnd = self.hwnd;
        let main_hwnd_visible = unsafe { IsWindowVisible(main_hwnd as *mut std::ffi::c_void) } != 0;
        if visibility_guard_should_hide(self.visible, main_hwnd != 0, main_hwnd_visible) {
            if !self.main_guard_episode_logged {
                self.main_guard_episode_logged = true;
                log::warn!(
                    "visibility guard: main window shown while hidden state bit set (visible={}, hwnd=0x{:X}, IsWindowVisible=true); re-hiding",
                    self.visible,
                    main_hwnd
                );
            }
            // G3: reuse the hide() primitive — SW_HIDE clears WS_VISIBLE;
            // idempotent on an already-hidden window. SAFETY: `hwnd` is our
            // own main window.
            let hwnd = main_hwnd as *mut std::ffi::c_void;
            if !hwnd.is_null() {
                unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
        } else {
            self.main_guard_episode_logged = false;
        }

        // --- Quick window ---
        let quick_hwnd = self.quick_hwnd;
        let quick_hwnd_visible =
            unsafe { IsWindowVisible(quick_hwnd as *mut std::ffi::c_void) } != 0;
        if visibility_guard_should_hide(self.quick_visible, quick_hwnd != 0, quick_hwnd_visible) {
            if !self.quick_guard_episode_logged {
                self.quick_guard_episode_logged = true;
                log::warn!(
                    "visibility guard: quick window shown while hidden state bit set (quick_visible={}, quick_hwnd=0x{:X}, IsWindowVisible=true); re-hiding",
                    self.quick_visible,
                    quick_hwnd
                );
            }
            // G3: reuse the hide_quick_window() primitive —
            // SWP_HIDEWINDOW|NOACTIVATE|NOMOVE|NOSIZE|NOZORDER; idempotent.
            // SAFETY: `hwnd` is our own quick window.
            let hwnd = quick_hwnd as *mut std::ffi::c_void;
            if !hwnd.is_null() {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                    );
                }
            }
        } else {
            self.quick_guard_episode_logged = false;
        }
    }

    /// C3: when the visible main window's bounds no longer overlap any
    /// remaining monitor work area, migrate it onto the deterministic
    /// fallback target (cursor monitor → primary → first remaining) using
    /// the existing `clamp_to_work_area` primitive and the async
    /// `cx.spawn()` + yield `SetWindowPos` pattern (never inside an entity
    /// update — Windows DPI re-entrancy). With no remaining monitor the
    /// window keeps its position and no positioning happens (04-spec §4).
    #[cfg(target_os = "windows")]
    fn migrate_main_window_on_topology_change(
        &mut self,
        snapshot: &monitor::MonitorSnapshot,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

        if self.hwnd == 0 {
            return;
        }
        let hwnd = self.hwnd as *mut std::ffi::c_void;

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
            log::warn!("monitor topology: GetWindowRect failed for main window");
            return;
        }
        let bounds = monitor::MonitorRect {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        };
        if !bounds.is_valid() {
            return;
        }

        if !monitor::window_needs_migration(&bounds, &snapshot.work_areas()) {
            return; // still overlaps a remaining work area — nothing to do.
        }

        let cursor = monitor::get_cursor_pos();
        let Some(target) = monitor::pick_migration_target(&snapshot.monitors, cursor) else {
            log::warn!(
                "monitor topology: no remaining monitor work area; leaving main window at ({},{})",
                bounds.x,
                bounds.y
            );
            return;
        };

        let (x, y) = clamp_to_work_area(bounds.x, bounds.y, bounds.width, bounds.height, &target);
        log::info!(
            "monitor topology: migrating main window ({},{}) → ({},{})",
            bounds.x,
            bounds.y,
            x,
            y
        );

        let hwnd_isize = self.hwnd;
        self._main_monitor_migration_task = Some(cx.spawn(async move |weak_self, cx| {
            // Yield until the poll update has released GPUI's AppCell borrow
            // so synchronous WM_DPICHANGED / WM_SIZE / WM_MOVE callbacks can
            // re-borrow (Windows DPI re-entrancy).
            Timer::after(Duration::from_millis(1)).await;

            let Some(this) = weak_self.upgrade() else {
                return;
            };
            let should_move = this
                .update(cx, |wm, _cx| {
                    wm.visible && wm.hwnd == hwnd_isize && wm.topology_generation == generation
                })
                .unwrap_or(false);
            if !should_move || hwnd_isize == 0 {
                return;
            }

            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            };
            let hwnd = hwnd_isize as *mut std::ffi::c_void;
            // SAFETY: `hwnd` is our own main window. SWP_NOACTIVATE keeps the
            // foreground app and SWP_NOSIZE preserves the GPUI-managed size —
            // only the position changes.
            unsafe {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    x,
                    y,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
                );
            }
        }));
    }

    /// C4: after a topology change, re-apply the quick window's design bounds
    /// (`QUICK_WINDOW_WIDTH` × dynamic height) and, when its current bounds
    /// are off every remaining work area, migrate it through the C3 fallback
    /// chain. Uses the existing two-phase async positioning pattern
    /// (`position_quick_window_windows`), never a synchronous native call.
    #[cfg(target_os = "windows")]
    fn reapply_quick_window_on_topology_change(
        &mut self,
        snapshot: &monitor::MonitorSnapshot,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

        if self.quick_hwnd == 0 {
            return;
        }
        let quick_hwnd = self.quick_hwnd;
        let hwnd = quick_hwnd as *mut std::ffi::c_void;

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let bounds = if unsafe { GetWindowRect(hwnd, &mut rect) } != 0 {
            monitor::MonitorRect {
                x: rect.left,
                y: rect.top,
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
            }
        } else {
            log::warn!("monitor topology: GetWindowRect failed for quick window");
            return;
        };

        let quick_h = self.current_quick_height(cx);

        // Keep the current top-left when it still overlaps a remaining work
        // area; otherwise pick the C3 fallback target (cursor monitor →
        // primary → first remaining). No remaining monitor → keep position,
        // no positioning (04-spec §4).
        let (x, y) = if monitor::window_needs_migration(&bounds, &snapshot.work_areas()) {
            let cursor = monitor::get_cursor_pos();
            match monitor::pick_migration_target(&snapshot.monitors, cursor) {
                Some(target) => {
                    // Clamp the intended client size at the destination DPI;
                    // the old monitor's size can leave the popup off-screen.
                    let scale = monitor::get_scale_factor(target.x, target.y);
                    let pos = clamp_to_work_area(
                        bounds.x,
                        bounds.y,
                        (QUICK_WINDOW_WIDTH * scale) as i32,
                        (quick_h * scale) as i32,
                        &target,
                    );
                    log::info!(
                        "monitor topology: migrating quick window ({},{}) → ({},{})",
                        bounds.x,
                        bounds.y,
                        pos.0,
                        pos.1
                    );
                    pos
                }
                None => {
                    log::warn!(
                        "monitor topology: no remaining monitor work area; leaving quick window at ({},{})",
                        bounds.x,
                        bounds.y
                    );
                    return;
                }
            }
        } else {
            (bounds.x, bounds.y)
        };

        let target_sf = monitor::get_scale_factor(x, y);
        let quick_window = self.quick_window;
        self._quick_monitor_migration_task = Some(cx.spawn(async move |weak_self, cx| {
            // Yield until the poll update has released GPUI's AppCell borrow.
            Timer::after(Duration::from_millis(1)).await;

            let Some(this) = weak_self.upgrade() else {
                return;
            };
            let should_position = this
                .update(cx, |wm, _cx| {
                    wm.quick_visible
                        && wm.quick_hwnd == quick_hwnd
                        && wm.topology_generation == generation
                })
                .unwrap_or(false);
            if !should_position || quick_hwnd == 0 {
                return;
            }

            // Phase 1: place with the window's current DPI so WM_DPICHANGED
            // can update GPUI before the final size compensation.
            let current_sf =
                unsafe { GetDpiForWindow(quick_hwnd as *mut std::ffi::c_void) } as f32 / 96.0;
            position_quick_window_windows(quick_hwnd, x, y, quick_h, current_sf, false);

            // Phase 2: enforce the target-monitor client size with the frame
            // insets measured after the DPI change (two-phase pattern).
            Timer::after(Duration::from_millis(1)).await;
            let should_finish = this
                .update(cx, |wm, _cx| {
                    wm.quick_visible
                        && wm.quick_hwnd == quick_hwnd
                        && wm.topology_generation == generation
                })
                .unwrap_or(false);
            if !should_finish {
                return;
            }

            position_quick_window_windows(quick_hwnd, x, y, quick_h, target_sf, true);

            if let Some(handle) = quick_window {
                let _ = cx.update_window(handle, |_, window, _cx| window.refresh());
            }
        }));
    }

    /// Prepare for graceful shutdown: save geometry, flush WAL,
    /// release platform resources.
    fn prepare_shutdown(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(state) = &self.main_compacted_state {
            // doc §4.4: never persist the compacted 1×1 geometry — reuse the
            // pre-compaction capture instead of re-reading the window.
            let (x, y, w, h) = state.saved_geometry;
            self.saved_x = x;
            self.saved_y = y;
            self.saved_w = w;
            self.saved_h = h;
        } else {
            self.capture_window_geometry(cx);
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.capture_window_geometry(cx);
        }
        // Force a geometry flush on shutdown (bypasses the debounce).
        self.flush_geometry_now(cx);
        self.state.update(cx, |state, _cx| {
            if let Err(e) = state.db.checkpoint() {
                log::error!("WAL checkpoint failed (save geometry): {e}");
            }
        });
        self.shutdown(cx);
    }

    /// Fully quit the application.
    fn do_quit(&mut self, cx: &mut Context<Self>) {
        self.prepare_shutdown(cx);
        cx.quit();
    }

    /// Restart the application: flush, spawn new process, then quit.
    fn do_restart(&mut self, cx: &mut Context<Self>) {
        self.prepare_shutdown(cx);
        let _ = crate::core::settings::spawn_new_process();
        cx.quit();
    }

    // --- Foreground detection ---

    /// Check if our own window is the current foreground window.
    /// Uses direct HWND comparison to avoid dependence on window title.
    fn is_self_foreground(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            // SAFETY: `GetForegroundWindow` is a read-only query safe from any thread.
            self.hwnd != 0 && unsafe { GetForegroundWindow() } as isize == self.hwnd
        }
        #[cfg(not(target_os = "windows"))]
        {
            crate::platform::blacklist::is_clippi_foreground()
        }
    }

    // --- Foreground tracking ---

    fn update_foreground_app_name(&mut self, cx: &mut Context<Self>) {
        use crate::platform::focus::get_foreground_app_info;

        if self.is_self_foreground() {
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                fg.clear();
            }
            // --- Keep the UI showing the last foreground app (don't clear AppState). ---
            return;
        }

        let Some(info) = get_foreground_app_info() else {
            return;
        };
        let app_changed = self
            .foreground_app_name
            .lock()
            .map(|mut foreground| {
                let changed = *foreground != info.app_name;
                if changed {
                    foreground.clone_from(&info.app_name);
                }
                changed
            })
            .unwrap_or(false);

        // Compare the complete payload, not just app name/title: a new process
        // with the same display name can still have a different icon.
        let state_changed = {
            let state = self.state.read(cx);
            state.foreground_app_name != info.app_name
                || state.foreground_window_title != info.window_title
                || state.foreground_app_icon_base64 != info.icon_base64
        };
        if !state_changed {
            return;
        }

        if app_changed {
            let _ = crate::core::paths::cache_app_icon(&info.app_name, &info.icon_base64);
        }
        self.state.update(cx, move |state, _cx| {
            state.foreground_app_name = info.app_name;
            state.foreground_window_title = info.window_title;
            state.foreground_app_icon_base64 = info.icon_base64;
        });
    }

    // --- Position calculation ---

    fn calculate_position(&self) -> Option<(i32, i32)> {
        let (win_w, win_h) = self.effective_window_size();

        // Tray-triggered opens always use "remember" position — the cursor
        // is near the tray at the screen edge and center/follow look awkward.
        let mode = if self.tray_triggered {
            &PositionMode::Remember
        } else {
            &self.position_mode
        };

        match mode {
            PositionMode::Center => {
                let (cx, cy) = monitor::get_cursor_pos()?;
                let scale = monitor::get_scale_factor(cx, cy);
                self.calc_center((win_w * scale) as i32, (win_h * scale) as i32)
            }
            PositionMode::FollowMouse => {
                let (cx, cy) = monitor::get_cursor_pos()?;
                let scale = monitor::get_scale_factor(cx, cy);
                self.calc_follow_mouse(
                    (win_w * scale) as i32,
                    (win_h * scale) as i32,
                    (PANEL_OFFSET_X * scale) as i32,
                )
            }
            PositionMode::Remember => {
                if self.saved_w > 0.0
                    && self.saved_h > 0.0
                    && monitor::is_point_on_monitor(self.saved_x, self.saved_y)
                {
                    let scale = monitor::get_scale_factor(self.saved_x, self.saved_y);
                    self.calc_remember((win_w * scale) as i32, (win_h * scale) as i32)
                } else {
                    let (cx, cy) = monitor::get_cursor_pos()?;
                    let scale = monitor::get_scale_factor(cx, cy);
                    self.calc_center((win_w * scale) as i32, (win_h * scale) as i32)
                }
            }
        }
    }

    fn calc_center(&self, win_w: i32, win_h: i32) -> Option<(i32, i32)> {
        let (cx, cy) = monitor::get_cursor_pos()?;
        let area = monitor::get_monitor_work_area(cx, cy)?;
        let x = area.x + (area.width - win_w) / 2;
        let y = area.y + (area.height - win_h) / 2;
        Some((x, y))
    }

    fn calc_follow_mouse(&self, win_w: i32, win_h: i32, sidebar_offset: i32) -> Option<(i32, i32)> {
        let (cx, cy) = monitor::get_cursor_pos()?;
        let area = monitor::get_monitor_work_area(cx, cy)?;
        // --- Offset by sidebar width so the main panel aligns with the cursor ---
        Some(clamp_to_work_area(
            cx - sidebar_offset,
            cy,
            win_w,
            win_h,
            &area,
        ))
    }

    fn calc_remember(&self, win_w: i32, win_h: i32) -> Option<(i32, i32)> {
        let (sx, sy) = (self.saved_x, self.saved_y);
        if self.saved_w <= 0.0 || self.saved_h <= 0.0 {
            return None;
        }
        if !monitor::is_point_on_monitor(sx, sy) {
            return None;
        }
        let area = monitor::get_monitor_work_area(sx, sy)?;
        Some(clamp_to_work_area(sx, sy, win_w, win_h, &area))
    }

    fn effective_window_size(&self) -> (f32, f32) {
        let w = if self.saved_w > 0.0 {
            self.saved_w.max(DEFAULT_WINDOW_WIDTH)
        } else {
            DEFAULT_WINDOW_WIDTH
        };
        let h = if self.saved_h > 0.0 {
            self.saved_h.max(DEFAULT_WINDOW_HEIGHT)
        } else {
            DEFAULT_WINDOW_HEIGHT
        };
        (w, h)
    }

    fn is_suppressed(&self) -> bool {
        self.suppress_until
            .map(|until| Instant::now() <= until)
            .unwrap_or(false)
    }

    // --- Window operations (platform-specific) ---

    /// Show the window, calculate position, and bring it to foreground.
    ///
    /// When the window is already visible this only extends the suppress
    /// period and brings the window to foreground — it skips repositioning
    /// and item reload to avoid disrupting the current view.
    pub fn show_and_focus(&mut self, cx: &mut Context<Self>) {
        if self.quick_visible {
            self.hide_quick_window(cx);
        }
        self.suppress_until = Some(Instant::now() + Duration::from_millis(SUPPRESS_DURATION_MS));

        let was_already_visible = self.visible;
        self.visible = true;

        if was_already_visible {
            // --- Window is already open (e.g. user is in settings recording a ---
            // --- hotkey). Just bring to foreground without repositioning. ---
            #[cfg(target_os = "windows")]
            {
                use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                let hwnd = self.hwnd as *mut std::ffi::c_void;
                if !hwnd.is_null() {
                    unsafe { SetForegroundWindow(hwnd) };
                }
            }
            #[cfg(target_os = "macos")]
            {
                cx.activate(true);
                self.activate_macos_window();
            }
            cx.notify();
            return;
        }

        self.pinned = false;
        cx.emit(WindowManagerEvent::PinnedChanged(false));

        // --- Reload items from DB (they were cleared on hide) ---
        self.state.update(cx, |state, _cx| state.reload_items());
        cx.emit(WindowManagerEvent::ClipboardChanged);

        // When enabled, reset to clipboard history on every "show" action.
        // Skip when tray_triggered is true — those are directed navigations
        // (OpenSettings / OpenVersionSettings) that emit their own events after.
        if !self.tray_triggered && self.state.read(cx).settings.always_reset_to_clipboard {
            cx.emit(WindowManagerEvent::ResetToClipboard);
        }

        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd;
            let position = self.calculate_position();
            self._main_show_task = Some(cx.spawn(async move |weak_self, cx| {
                // Yield until the WindowManager update that handled the hotkey
                // or tray action has released GPUI's AppCell borrow.
                Timer::after(Duration::from_millis(1)).await;

                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let should_show = this
                    .update(cx, |wm, _cx| wm.visible && wm.hwnd == hwnd)
                    .unwrap_or(false);
                if !should_show || hwnd == 0 {
                    return;
                }

                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOACTIVATE,
                    SWP_NOSIZE, SW_SHOW,
                };
                let hwnd = hwnd as *mut std::ffi::c_void;
                if let Some((x, y)) = position {
                    unsafe {
                        SetWindowPos(hwnd, HWND_TOP, x, y, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
                    }
                }
                unsafe {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                }
            }));
        }

        #[cfg(target_os = "macos")]
        {
            if self.main_compacted_state.is_some() {
                // Restore the full size before making the window visible to
                // avoid a 1×1 flash (doc §3.3).
                self.restore_main_macos_window(cx);
            } else {
                if let Some((x, y)) = self.calculate_position() {
                    self.position_macos_window(x, y);
                }
                cx.activate(true);
                self.activate_macos_window();
            }
        }

        cx.notify();
    }

    #[allow(dead_code)]
    pub fn show_toast(&self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.show_toast(message);
            cx.notify();
        });
    }

    pub fn show_warning_toast(&self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.show_warning_toast(message);
            cx.notify();
        });
    }

    /// Initialise the hotkey listener after GPUI has finished its first render.
    ///
    /// Creating the hotkey during `WindowManager::new()` (inside the `open_window`
    /// callback) registers the OS-level hotkey BEFORE GPUI's input/IME pipeline is
    /// ready. If the user presses the hotkey immediately the resulting `show_and_focus`
    /// shows the window with a non-functional text input pipeline.
    ///
    /// Deferring hotkey creation to after the first frame ensures the input pipeline
    /// is ready before the first `show_and_focus` can be triggered.
    pub fn init_hotkey(&mut self, cx: &mut Context<Self>) {
        let hotkey_str = self.state.read(cx).settings.hotkey.clone();
        let enabled = self.state.read(cx).settings.quick_hotkey_enabled;
        let quick_hotkey_str = if enabled {
            self.state.read(cx).settings.quick_hotkey.clone()
        } else {
            String::new()
        };
        self.hotkey = match create_hotkey_listener(&hotkey_str, &quick_hotkey_str) {
            Ok(hk) => {
                // If a fallback was used, persist the new hotkey and notify the user.
                if hk.main_fallback_used() {
                    let actual = hk.actual_main_hotkey().to_string();
                    let configured_display = hotkey_display(&hotkey_str);
                    let actual_display = hotkey_display(&actual);
                    self.state.update(cx, |state, _cx| {
                        state.settings.hotkey = actual.clone();
                        state.settings.save();
                        state.toast_message = Some(
                            I18nKey::HotkeyFallbackToast
                                .fmt(&[&configured_display, &actual_display]),
                        );
                        state.toast_is_warning = true;
                    });
                }
                if hk.quick_fallback_used() {
                    let actual = hk.actual_quick_hotkey().to_string();
                    let configured_display = hotkey_display(&quick_hotkey_str);
                    let actual_display = hotkey_display(&actual);
                    self.state.update(cx, |state, _cx| {
                        state.settings.quick_hotkey = actual.clone();
                        state.settings.save();
                        state.toast_message = Some(
                            I18nKey::HotkeyFallbackToast
                                .fmt(&[&configured_display, &actual_display]),
                        );
                        state.toast_is_warning = true;
                    });
                }
                Some(hk)
            }
            Err(e) => {
                log::error!("Failed to create hotkey listener: {e}");
                None
            }
        };
        // Reload custom hotkeys from persisted state.
        if let Some(ref mut hk) = self.hotkey {
            let state = self.state.read(cx);
            let item_hotkeys = state.db.get_all_custom_item_hotkeys().unwrap_or_else(|e| {
                log::error!("Failed to load custom item hotkeys: {e}");
                Vec::new()
            });
            let latest_hotkeys: Vec<(usize, String)> = state
                .settings
                .latest_hotkeys
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.hotkey.is_empty())
                .map(|(i, e)| (i, e.hotkey.clone()))
                .collect();
            hk.reload_custom_hotkeys(&item_hotkeys, &latest_hotkeys);
        }

        // After fallback hotkey is registered, attempt Win+V takeover if enabled.
        self.init_win_v_takeover(cx);
    }

    // ── Win+V takeover (Windows only) ──

    /// Try to apply the Win+V takeover after the fallback hotkey is already
    /// registered.  Called from `init_hotkey()` at startup and from the
    /// enable/recheck flows.
    #[cfg(target_os = "windows")]
    fn init_win_v_takeover(&mut self, cx: &mut Context<Self>) {
        let replace = self.state.read(cx).settings.replace_system_win_v;
        if !replace {
            self.win_v_takeover_status = WinVTakeoverStatus::Disabled;
            return;
        }

        match windows_hotkeys::inspect_win_v_registry() {
            Ok(snapshot) => {
                if !snapshot.win_v_disabled {
                    self.win_v_takeover_status = WinVTakeoverStatus::RegistryUpdateRequired;
                    return;
                }
            }
            Err(e) => {
                log::warn!("Failed to read Win+V registry state: {e}");
                self.win_v_takeover_status = WinVTakeoverStatus::RegistryError;
                return;
            }
        }

        // Registry has V — try to register Win+V.
        if let Some(ref mut hk) = self.hotkey {
            match hk.update_hotkey("Win+V") {
                Ok(()) => {
                    log::info!("Win+V takeover: registered successfully");
                    self.win_v_takeover_status = WinVTakeoverStatus::Active;
                }
                Err(e) => {
                    log::warn!("Win+V takeover: register failed: {e}");
                    self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
                }
            }
        } else {
            // Hotkey listener not available — cannot register anything.
            self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
        }
    }

    /// Enable Win+V takeover: write registry, persist setting, try to register.
    #[cfg(target_os = "windows")]
    pub fn enable_win_v_takeover(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let mutation = windows_hotkeys::configure_win_v_takeover()?;
        log::info!("Win+V takeover enable: {mutation}");

        self.state.update(cx, |state, _cx| {
            state.settings.replace_system_win_v = true;
            state.settings.save();
        });

        // Try to register Win+V immediately; keep fallback on failure.
        if let Some(ref mut hk) = self.hotkey {
            match hk.update_hotkey("Win+V") {
                Ok(()) => {
                    self.win_v_takeover_status = WinVTakeoverStatus::Active;
                }
                Err(_) => {
                    self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
                }
            }
        } else {
            self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
        }
        cx.notify();
        Ok(())
    }

    /// Disable Win+V takeover: cleanup registry, then switch to fallback.
    ///
    /// Uses a commit/rollback strategy:
    /// 1. Switch hotkey to fallback.
    /// 2. Clean up the registry.
    /// 3. If registry cleanup fails, switch back to Win+V.
    /// 4. Only persist the setting change after all steps succeed.
    #[cfg(target_os = "windows")]
    pub fn disable_win_v_takeover(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let fallback = self.state.read(cx).settings.hotkey.clone();

        // Step 1: Switch to fallback hotkey.
        if let Some(ref mut hk) = self.hotkey {
            hk.update_hotkey(&fallback).map_err(|e| {
                log::warn!("Win+V takeover: cannot restore fallback hotkey: {e}");
                I18nKey::WinVFallbackUnavailable.text().to_string()
            })?;
        }

        // Step 2: Clean up registry.
        match windows_hotkeys::restore_win_v_if_managed() {
            Ok(mutation) => {
                log::info!("Win+V takeover disable: {mutation}");
            }
            Err(e) => {
                log::error!("Win+V takeover: registry cleanup failed: {e}");
                // Rollback: try to re-register Win+V.
                if let Some(ref mut hk) = self.hotkey {
                    if let Err(re) = hk.update_hotkey("Win+V") {
                        log::error!(
                            "Win+V takeover: rollback also failed ({re}); hotkey state is uncertain"
                        );
                        self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
                        cx.notify();
                        return Err(format!(
                            "Registry cleanup failed and rollback also failed: {e}"
                        ));
                    }
                }
                self.win_v_takeover_status = WinVTakeoverStatus::Active;
                cx.notify();
                return Err(format!("Registry cleanup failed; Win+V restored: {e}"));
            }
        }

        // Step 3: All good — persist the setting change.
        self.state.update(cx, |state, _cx| {
            state.settings.replace_system_win_v = false;
            state.settings.save();
        });

        self.win_v_takeover_status = WinVTakeoverStatus::Disabled;
        cx.notify();
        Ok(())
    }

    /// Get the current Win+V takeover status.
    pub fn win_v_takeover_status(&self) -> WinVTakeoverStatus {
        self.win_v_takeover_status
    }

    #[cfg(not(target_os = "windows"))]
    pub fn enable_win_v_takeover(&mut self, _cx: &mut Context<Self>) -> Result<(), String> {
        Err("Win+V takeover is only available on Windows".into())
    }

    #[cfg(not(target_os = "windows"))]
    pub fn disable_win_v_takeover(&mut self, _cx: &mut Context<Self>) -> Result<(), String> {
        Err("Win+V takeover is only available on Windows".into())
    }

    /// Re-check the registry and try to register Win+V (user-triggered).
    #[cfg(target_os = "windows")]
    pub fn recheck_win_v_takeover(&mut self, cx: &mut Context<Self>) {
        match windows_hotkeys::inspect_win_v_registry() {
            Ok(snapshot) => {
                if !snapshot.win_v_disabled {
                    self.win_v_takeover_status = WinVTakeoverStatus::RegistryUpdateRequired;
                    cx.notify();
                    return;
                }
            }
            Err(e) => {
                log::warn!("Win+V recheck: registry read failed: {e}");
                self.win_v_takeover_status = WinVTakeoverStatus::RegistryError;
                cx.notify();
                return;
            }
        }

        if let Some(ref mut hk) = self.hotkey {
            match hk.update_hotkey("Win+V") {
                Ok(()) => {
                    log::info!("Win+V recheck: registered successfully");
                    self.win_v_takeover_status = WinVTakeoverStatus::Active;
                }
                Err(_) => {
                    self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
                }
            }
        }
        cx.notify();
    }

    #[cfg(not(target_os = "windows"))]
    pub fn recheck_win_v_takeover(&mut self, _cx: &mut Context<Self>) {}

    /// Periodic registry re-check (called from 200 ms poll loop, throttled to 30 s).
    ///
    /// Entry condition is based on the persisted toggle, not the current runtime
    /// status, so that `RegistryUpdateRequired` and `RegistryError` also
    /// participate in automatic recovery.
    #[cfg(target_os = "windows")]
    fn periodic_win_v_recheck(&mut self, cx: &mut Context<Self>) {
        if !self.state.read(cx).settings.replace_system_win_v {
            return;
        }

        let now = Instant::now();
        if let Some(last) = self.last_win_v_recheck {
            if now.duration_since(last) < Duration::from_secs(30) {
                return;
            }
        }
        self.last_win_v_recheck = Some(now);

        match windows_hotkeys::inspect_win_v_registry() {
            Ok(snapshot) => {
                if !snapshot.win_v_disabled {
                    // V missing — if currently Active, fall back to configured hotkey.
                    if self.win_v_takeover_status == WinVTakeoverStatus::Active {
                        log::warn!("Win+V periodic: V removed from DisabledHotkeys; falling back");
                        let fallback = self.state.read(cx).settings.hotkey.clone();
                        if let Some(ref mut hk) = self.hotkey {
                            if hk.update_hotkey(&fallback).is_err() {
                                // Fallback registration failed, but Win+V is still
                                // active (update_hotkey preserves old binding on
                                // error).  Keep Active and retry next cycle.
                                log::error!(
                                    "Win+V periodic: fallback registration failed; Win+V still active"
                                );
                                cx.notify();
                                return;
                            }
                        }
                    }
                    self.win_v_takeover_status = WinVTakeoverStatus::RegistryUpdateRequired;
                    cx.notify();
                    return;
                }

                // V is present — attempt registration if not already Active.
                if self.win_v_takeover_status != WinVTakeoverStatus::Active {
                    if let Some(ref mut hk) = self.hotkey {
                        match hk.update_hotkey("Win+V") {
                            Ok(()) => {
                                log::info!("Win+V periodic: registered successfully");
                                self.win_v_takeover_status = WinVTakeoverStatus::Active;
                                cx.notify();
                            }
                            Err(_) => {
                                self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
                                cx.notify();
                            }
                        }
                    } else {
                        self.win_v_takeover_status = WinVTakeoverStatus::HotkeyUnavailable;
                        cx.notify();
                    }
                }
            }
            Err(e) => {
                log::warn!("Win+V periodic: registry read failed: {e}");
                if self.win_v_takeover_status != WinVTakeoverStatus::RegistryError {
                    self.win_v_takeover_status = WinVTakeoverStatus::RegistryError;
                    cx.notify();
                }
            }
        }
    }

    /// Stubs for non-Windows platforms.
    #[cfg(not(target_os = "windows"))]
    fn init_win_v_takeover(&mut self, _cx: &mut Context<Self>) {
        self.win_v_takeover_status = WinVTakeoverStatus::Disabled;
    }

    /// Release memory without changing window visibility.
    ///
    /// Used when the window starts hidden (silent_start via
    /// `WindowOptions { show: false }`) and on hide — drops the in-memory
    /// items list, releases UI image caches, checkpoints the WAL, and trims
    /// the process working set.
    ///
    /// Cleanup order matters: live objects (items, image caches) must be
    /// dropped *before* `malloc_zone_pressure_relief` runs, otherwise the
    /// allocator cannot return their pages. Subscribers release their
    /// objects synchronously on `ReleaseUiResources`, and the pressure
    /// relief is deferred to the next app update to also cover any
    /// render-side drops.
    ///
    /// With macOS surface compaction the compaction confirmation task runs a
    /// *second* trim strictly after the hidden window's renderer has
    /// processed the 1×1 resize (doc §3.5, `compact_main_macos_window` /
    /// `compact_quick_macos_window`). The deferred trim here stays as a
    /// fallback so paths that never compact (silent start, compaction
    /// disabled) still release allocator pages. The two calls are not in the
    /// same call stack and both are idempotent hints.
    pub fn release_memory(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.clear_items());
        // Synchronously release list items + image caches in subscribers.
        cx.emit(WindowManagerEvent::ReleaseUiResources);
        self.state.update(cx, |state, _cx| {
            if let Err(e) = state.db.checkpoint() {
                log::error!("WAL checkpoint failed (clipboard changed): {e}");
            }
        });
        // Pressure relief must run after the objects above are dropped;
        // defer it to the next app update so render-side drops also land.
        cx.defer(|_cx| crate::platform::util::trim_process_working_set());
    }

    /// Hide the window to background — does NOT exit the process.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        {
            self._main_show_task = None;
            self._main_monitor_migration_task = None;
        }
        if self.quick_visible {
            self.hide_quick_window(cx);
        }
        self.dismiss_ui(cx);
        self.release_memory(cx);
        cx.emit(WindowManagerEvent::WindowHidden);

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

            let hwnd = self.hwnd as *mut std::ffi::c_void;

            // --- Save position in Remember mode ---
            if self.position_mode == PositionMode::Remember && !hwnd.is_null() {
                use windows_sys::Win32::Foundation::POINT;
                use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
                let mut pt = POINT { x: 0, y: 0 };
                // SAFETY: HWND is our own window. `ClientToScreen` translates
                // the client-origin to screen coordinates, writing into a
                // stack-allocated POINT.
                unsafe {
                    ClientToScreen(hwnd, &mut pt);
                }
                self.saved_x = pt.x;
                self.saved_y = pt.y;
            }

            if !hwnd.is_null() {
                // SAFETY: HWND is our own window. `ShowWindow(SW_HIDE)` hides it.
                unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
        }

        #[cfg(target_os = "macos")]
        {
            // Cancel any in-flight restore so it cannot order the window
            // front after this hide (doc §4.1).
            self._main_restore_task = None;
            self.capture_window_geometry(cx);
            self.hide_macos_window();
            if MACOS_SURFACE_COMPACTION_ENABLED && !self.macos_compaction_disabled {
                self.compact_main_macos_window(cx);
            }
        }

        self.visible = false;
        // Persist the final geometry immediately on hide (bypass debounce).
        self.flush_geometry_now(cx);
    }

    fn show_quick_window(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.quick_view.clone() else {
            return;
        };

        let switching_from_main = self.visible;

        // Main and quick window are never shown simultaneously.
        if switching_from_main {
            self.hide(cx);
            // A non-activating popup cannot receive the focus relinquished by
            // the main window. Restore the previous application explicitly so
            // keyboard shortcuts and paste keep targeting it on both platforms.
            crate::platform::paste::restore_paste_target();
        }

        self.state.update(cx, |state, _cx| state.reload_items());
        view.update(cx, |view, cx| view.reset_scroll(cx));
        self.quick_visible = true;
        self.quick_mouse_down = Self::mouse_buttons_down();
        // Debounce: prevent click-outside and hotkey toggle hides briefly after
        // show. This absorbs double hotkey events and lets async positioning
        // complete before the window is dismissible.
        self.quick_suppress_until = Some(Instant::now() + Duration::from_millis(400));
        if let Some(ref mut hotkey) = self.hotkey {
            hotkey.set_quick_actions_enabled(true);
        }

        // Compute dynamic window height based on visible bars (C4: semantics
        // of QUICK_WINDOW_WIDTH × calc_quick_window_height unchanged).
        let quick_h = self.current_quick_height(cx);

        // Positioning priority: Caret (Path A/B) → Cursor (Path D) → C3
        // fallback chain (issue-75 C4). Never fall back to window-centered
        // positioning — the window must follow the text caret or the mouse.
        // On Windows the terminal fallback resolves a deterministic position
        // from a fresh monitor snapshot; when no remaining monitor yields a
        // valid target the quick window stays hidden (04-spec §4 — never
        // (0,0)). macOS uses the same work-area fallback in logical points.
        let Some((x, y)) = self.calculate_quick_position(quick_h).or_else(|| {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            {
                self.quick_position_c3_fallback(quick_h)
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                let (cx, cy) = monitor::get_cursor_pos().unwrap_or((0, 0));
                log::debug!(
                    "show_quick_window: positioning fallback, raw cursor=({},{})",
                    cx,
                    cy
                );
                Some((cx, cy))
            }
        }) else {
            log::warn!(
                "show_quick_window: no valid monitor for positioning (C4 fallback chain exhausted); keeping quick window hidden"
            );
            self.quick_suppress_until = None;
            self.hide_quick_window(cx);
            return;
        };

        #[cfg(target_os = "macos")]
        {
            if self.quick_compacted_state.is_some() {
                // Restore the minimum size and dynamic height, then confirm
                // the resize before ordering front (doc §3.4).
                self.restore_quick_macos_window(x, y, quick_h, cx);
            } else {
                self.position_quick_macos_window(x, y, quick_h);
                self.show_quick_macos_window();
            }
        }

        #[cfg(not(target_os = "windows"))]
        if let Some(handle) = self.quick_window {
            let _ = cx.update_window(handle, |_view, window, _cx| window.refresh());
        }

        // Start fast poll for responsive keyboard navigation.
        self.start_quick_poll(cx);

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;

            let quick_hwnd = self.quick_hwnd;
            let quick_window = self.quick_window;
            let target_sf = monitor::get_scale_factor(x, y);
            self._quick_position_task = Some(cx.spawn(async move |weak_self, cx| {
                // Yield until the WindowManager update that handled the hotkey
                // has released GPUI's AppCell borrow.
                Timer::after(Duration::from_millis(1)).await;

                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let should_position = this
                    .update(cx, |wm, _cx| {
                        wm.quick_visible && wm.quick_hwnd == quick_hwnd
                    })
                    .unwrap_or(false);
                if !should_position || quick_hwnd == 0 {
                    return;
                }

                let hwnd = quick_hwnd as *mut std::ffi::c_void;
                let current_sf = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
                position_quick_window_windows(quick_hwnd, x, y, quick_h, current_sf, false);

                // Let WM_DPICHANGED update GPUI before measuring the new frame
                // insets and enforcing the final target-monitor client size.
                Timer::after(Duration::from_millis(1)).await;
                let should_finish = this
                    .update(cx, |wm, _cx| {
                        wm.quick_visible && wm.quick_hwnd == quick_hwnd
                    })
                    .unwrap_or(false);
                if !should_finish {
                    return;
                }

                position_quick_window_windows(quick_hwnd, x, y, quick_h, target_sf, true);

                if let Some(handle) = quick_window {
                    let _ = cx.update_window(handle, |_, window, _cx| window.refresh());
                }
            }));
        }
    }

    fn hide_quick_window(&mut self, cx: &mut Context<Self>) {
        self._quick_poll_task = None; // cancel fast poll
        #[cfg(target_os = "windows")]
        {
            self._quick_position_task = None;
            self._quick_monitor_migration_task = None;
        }
        self.quick_visible = false;
        self.quick_mouse_down = false;
        if let Some(ref mut hotkey) = self.hotkey {
            hotkey.set_quick_actions_enabled(false);
        }

        // Release decoded thumbnails/favicons/file icons while hidden so the
        // popup no longer pins image memory between uses.
        if let Some(view) = self.quick_view.clone() {
            view.update(cx, |view, cx| view.release_images_for_hide(cx));
        }

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetWindowPos, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            };
            let hwnd = self.quick_hwnd as *mut std::ffi::c_void;
            if !hwnd.is_null() {
                // Use SWP_HIDEWINDOW instead of SW_HIDE — SW_HIDE minimises
                // the window, causing GPUI to save request_frame and restore
                // it with should_resize_renderer=false on next show.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        0,
                        0,
                        0,
                        0,
                        SWP_HIDEWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                    );
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            // A defensive Quick Paste restore poll may still be pending when
            // the user dismisses and immediately reopens the popup.
            self._quick_restore_task = None;
            self.hide_quick_macos_window();
            if MACOS_SURFACE_COMPACTION_ENABLED && !self.macos_compaction_disabled {
                self.compact_quick_macos_window(cx);
            }
        }
    }

    /// Hide quick window and release memory (app goes idle).
    ///
    /// Use when the quick window closes *without* an immediate main-window
    /// transition.  `hide()` and `show_and_focus()` call `hide_quick_window`
    /// directly and manage memory themselves.
    fn dismiss_quick_window(&mut self, cx: &mut Context<Self>) {
        self.hide_quick_window(cx);
        self.release_memory(cx);
    }

    // `return` avoids rust-analyzer false E0308 with #[cfg] arms.
    #[allow(clippy::needless_return)]
    fn mouse_buttons_down() -> bool {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
            return [0x01, 0x02, 0x04]
                .into_iter()
                .any(|button| unsafe { (GetAsyncKeyState(button) & i16::MIN) != 0 });
        }
        #[cfg(target_os = "macos")]
        {
            return objc2_app_kit::NSEvent::pressedMouseButtons() != 0;
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            false
        }
    }

    fn handle_quick_action(&mut self, action: QuickAction, cx: &mut Context<Self>) {
        if !self.quick_visible {
            return;
        }
        let Some(view) = self.quick_view.clone() else {
            return;
        };

        match action {
            QuickAction::Previous => {
                view.update(cx, |view, cx| view.select_previous(cx));
            }
            QuickAction::Next => {
                view.update(cx, |view, cx| view.select_next(cx));
            }
            QuickAction::PreviousPage => {
                view.update(cx, |view, cx| view.select_previous_page(cx));
            }
            QuickAction::NextPage => {
                view.update(cx, |view, cx| view.select_next_page(cx));
            }
            QuickAction::Paste => {
                let (id, shift, ctrl, has_alt) = view.update(cx, |view, vcx| {
                    let id = view.selected_item_id(vcx);
                    let shift = view.shift_held;
                    let ctrl = view.ctrl_held;
                    let has_alt = if ctrl {
                        view.ensure_alt_modes_for_selection(vcx)
                    } else {
                        view.has_alt_modes()
                    };
                    (id, shift, ctrl, has_alt)
                });
                if let Some(id) = id {
                    if shift {
                        self.quick_paste_plain(id, cx);
                    } else if ctrl && has_alt {
                        let alt_mode = view.read(cx).current_alt_mode.clone();
                        self.quick_paste_alt(id, &alt_mode, cx);
                    } else {
                        self.quick_paste_item(id, cx);
                    }
                }
            }
            QuickAction::PasteShift => {
                let id = view.update(cx, |view, vcx| view.selected_item_id(vcx));
                if let Some(id) = id {
                    self.quick_paste_plain(id, cx);
                }
            }
            QuickAction::PasteCtrl => {
                let (id, has_alt, alt_mode) = view.update(cx, |view, vcx| {
                    let id = view.selected_item_id(vcx);
                    let has_alt = view.ensure_alt_modes_for_selection(vcx);
                    let alt_mode = view.current_alt_mode.clone();
                    (id, has_alt, alt_mode)
                });
                if let Some(id) = id {
                    if has_alt {
                        self.quick_paste_alt(id, &alt_mode, cx);
                    } else {
                        self.quick_paste_item(id, cx);
                    }
                }
            }
            QuickAction::Close => {
                self.dismiss_quick_window(cx);
            }
            QuickAction::Pick(slot) => {
                let (id, shift, ctrl, has_alt) = view.update(cx, |view, cx| {
                    let id = view.select_visible_slot(slot, cx);
                    let shift = view.shift_held;
                    let ctrl = view.ctrl_held;
                    let has_alt = if ctrl {
                        view.ensure_alt_modes_for_selection(cx)
                    } else {
                        view.has_alt_modes()
                    };
                    (id, shift, ctrl, has_alt)
                });
                if let Some(id) = id {
                    if shift {
                        self.quick_paste_plain(id, cx);
                    } else if ctrl && has_alt {
                        let alt_mode = view.read(cx).current_alt_mode.clone();
                        self.quick_paste_alt(id, &alt_mode, cx);
                    } else {
                        self.quick_paste_item(id, cx);
                    }
                }
            }
            QuickAction::PreviousAltMode => {
                view.update(cx, |view, cx| view.cycle_alt_mode(-1, cx));
            }
            QuickAction::NextAltMode => {
                view.update(cx, |view, cx| view.cycle_alt_mode(1, cx));
            }
        }
    }

    pub fn quick_paste_item(&mut self, id: i64, cx: &mut Context<Self>) {
        let plain = self.state.read(cx).settings.copy_as_plain_text;
        self.state
            .update(cx, |state, _cx| state.paste_item(id, plain));
        self.clear_quick_modifiers(cx);
        self.dismiss_quick_window(cx);
    }

    pub fn quick_paste_plain(&mut self, id: i64, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, _cx| state.paste_item_plain(id));
        self.clear_quick_modifiers(cx);
        self.dismiss_quick_window(cx);
    }

    pub fn quick_paste_alt(&mut self, id: i64, mode: &str, cx: &mut Context<Self>) {
        match mode {
            "bitmap" => {
                self.state.update(cx, |s, _cx| s.paste_image_as_bitmap(id));
            }
            "path" => {
                self.state.update(cx, |s, _cx| s.paste_image_path(id));
            }
            "ocr" => {
                self.state.update(cx, |s, _cx| s.paste_ocr(id));
            }
            "rgb" => {
                self.state.update(cx, |s, _cx| s.paste_as_rgb(id));
            }
            "hex" => {
                self.state.update(cx, |s, _cx| s.paste_as_hex(id));
            }
            "file_path" => {
                self.state.update(cx, |s, _cx| s.paste_file_path(id));
            }
            _ => {
                let plain = self.state.read(cx).settings.copy_as_plain_text;
                self.state.update(cx, |s, _cx| s.paste_item(id, plain));
            }
        }
        self.clear_quick_modifiers(cx);
        self.dismiss_quick_window(cx);
    }

    /// Paste an item using the given format override (static, called from state.update).
    fn paste_item_with_format(
        state: &mut crate::state::app::AppState,
        id: i64,
        format: crate::core::types::HotkeyPasteFormat,
    ) {
        match format {
            crate::core::types::HotkeyPasteFormat::Default => {
                state.paste_item(id, state.settings.copy_as_plain_text);
            }
            crate::core::types::HotkeyPasteFormat::PlainText => {
                state.paste_item_plain(id);
            }
            crate::core::types::HotkeyPasteFormat::ImageBitmap => {
                state.paste_image_as_bitmap(id);
            }
            crate::core::types::HotkeyPasteFormat::ImagePath => {
                state.paste_image_path(id);
            }
            crate::core::types::HotkeyPasteFormat::OcrText => {
                state.paste_ocr(id);
            }
            crate::core::types::HotkeyPasteFormat::FilePath => {
                state.paste_file_path(id);
            }
            crate::core::types::HotkeyPasteFormat::Rgb => {
                state.paste_as_rgb(id);
            }
            crate::core::types::HotkeyPasteFormat::Hex => {
                state.paste_as_hex(id);
            }
        }
    }

    fn clear_quick_modifiers(&mut self, cx: &mut Context<Self>) {
        if let Some(ref view) = self.quick_view {
            view.update(cx, |view, cx| {
                view.set_modifiers(false, false, cx);
            });
        }
    }

    fn calculate_quick_position(&self, quick_h: f32) -> Option<(i32, i32)> {
        // ── Path A/B/C: caret anchor (Win32 or UIA) ──
        if let Some(anchor) = crate::platform::text_input::get_text_input_anchor() {
            let scale = monitor::get_scale_factor(anchor.x, anchor.y);
            let win_w = (QUICK_WINDOW_WIDTH * scale) as i32;
            let win_h = (quick_h * scale) as i32;
            let area = monitor::get_monitor_work_area(anchor.x, anchor.y)?;
            let gap = (6.0 * scale).round() as i32;
            let below_y = anchor.y + anchor.height.max(1) + gap;
            let above_y = anchor.y - win_h - gap;
            let y = if below_y + win_h <= area.y + area.height {
                below_y
            } else {
                above_y
            };
            let pos = clamp_to_work_area(anchor.x, y, win_w, win_h, &area);
            log::info!(
                "quick_position: caret → ({},{}) scale={:.2}",
                pos.0,
                pos.1,
                scale
            );
            return Some(pos);
        }

        // ── Cursor fallback ──
        let (cx, cy) = monitor::get_cursor_pos()?;
        let scale = monitor::get_scale_factor(cx, cy);
        let win_w = (QUICK_WINDOW_WIDTH * scale) as i32;
        let win_h = (quick_h * scale) as i32;
        let area = monitor::get_monitor_work_area(cx, cy)?;
        let pos = clamp_to_work_area(cx, cy, win_w, win_h, &area);
        log::debug!(
            "quick_position: cursor → ({},{}) scale={:.2}",
            pos.0,
            pos.1,
            scale
        );
        Some(pos)
    }

    /// Clear all floating UI state.
    fn dismiss_ui(&self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| {
            state.clear_selection();
        });
        // --- Note: context_menu, tag_picker etc. will be handled by RootView ---
        // --- observing WindowManager events and clearing its own state. ---
    }

    // --- Public setters ---

    /// Store the raw window handle (HWND on Windows) for platform operations.
    #[cfg(target_os = "windows")]
    pub fn set_hwnd(&mut self, hwnd: isize) {
        self.hwnd = hwnd;
        crate::platform::focus::set_clippi_hwnd(hwnd);
    }

    pub fn set_quick_window(
        &mut self,
        handle: AnyWindowHandle,
        view: Entity<QuickPasteView>,
        cx: &mut Context<Self>,
    ) {
        self.quick_window = Some(handle);
        self.quick_view = Some(view.clone());
        self._quick_subscription = Some(cx.subscribe(
            &view,
            |this, _view, event: &QuickPasteEvent, cx| match event {
                QuickPasteEvent::Paste(id) => this.quick_paste_item(*id, cx),
                QuickPasteEvent::PastePlain(id) => this.quick_paste_plain(*id, cx),
                QuickPasteEvent::PasteAlt(id, mode) => {
                    this.quick_paste_alt(*id, mode, cx);
                }
            },
        ));
    }

    #[cfg(target_os = "windows")]
    pub fn set_quick_hwnd(&mut self, hwnd: isize) {
        use windows_sys::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, SWP_FRAMECHANGED,
            SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        };

        self.quick_hwnd = hwnd;
        let hwnd = hwnd as *mut std::ffi::c_void;
        if hwnd.is_null() {
            return;
        }

        // Keep the quick popup out of Alt-Tab/taskbar and prevent mouse clicks
        // from activating it while still allowing it to receive mouse messages.
        unsafe {
            // Popup windows do not always receive Windows 11's automatic
            // rounding. Explicitly request the standard system radius so DWM
            // clips the fully painted rectangular background and frame as one.
            let corner_preference = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                &corner_preference as *const _ as *const _,
                std::mem::size_of_val(&corner_preference) as u32,
            );
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE)
                | WS_EX_NOACTIVATE as i32
                | WS_EX_TOOLWINDOW as i32
                | WS_EX_TOPMOST as i32;
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED
                    | SWP_HIDEWINDOW
                    | SWP_NOACTIVATE
                    | SWP_NOMOVE
                    | SWP_NOSIZE
                    | SWP_NOZORDER,
            );
        }
    }

    /// Shared macOS window styling — transparent floating panel with no chrome buttons.
    #[cfg(target_os = "macos")]
    fn _style_ns_window(window: &objc2_app_kit::NSWindow) {
        use objc2_app_kit::{NSColor, NSWindowButton};
        window.setLevel(objc2_app_kit::NSFloatingWindowLevel);
        for btn_id in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(btn) = window.standardWindowButton(btn_id) {
                btn.setHidden(true);
            }
        }
        window.setHasShadow(false);
        let clear = NSColor::clearColor();
        window.setBackgroundColor(Some(&clear));
        window.setOpaque(false);
    }

    #[cfg(target_os = "macos")]
    pub fn set_ns_window(&mut self, ns_window: isize) {
        self.ns_window = ns_window;
        if ns_window == 0 {
            return;
        }
        let window = unsafe { &*(ns_window as *const objc2_app_kit::NSWindow) };
        Self::_style_ns_window(window);
    }

    #[cfg(target_os = "macos")]
    pub fn set_quick_ns_window(&mut self, ns_window: isize) {
        use objc2_quartz_core::kCACornerCurveContinuous;

        self.quick_ns_window = ns_window;
        if ns_window == 0 {
            return;
        }
        let window = unsafe { &*(ns_window as *const objc2_app_kit::NSWindow) };
        Self::_style_ns_window(window);
        window.setHasShadow(true);

        // Let AppKit/Core Animation own the actual window clipping. A
        // continuous corner curve matches standard macOS panels and avoids the
        // jagged transparent corners produced by view-only GPUI rounding.
        if let Some(content_view) = window.contentView() {
            content_view.setWantsLayer(true);
            if let Some(layer) = content_view.layer() {
                layer.setCornerRadius(QUICK_WINDOW_CORNER_RADIUS as f64);
                layer.setCornerCurve(unsafe { kCACornerCurveContinuous });
                layer.setMasksToBounds(true);
            }
        }
    }

    /// Store the GPUI handle for the main window so hidden-surface compaction
    /// can resize through the GPUI path (doc §3.2).
    #[cfg(target_os = "macos")]
    pub fn set_main_window(&mut self, handle: AnyWindowHandle) {
        self.main_window = Some(handle);
    }

    /// Read the current content size (in logical points) of a macOS window.
    /// Mirrors GPUI's own `content_size()` implementation
    /// (`NSView::frame(contentView()).size`), so polling this is equivalent to
    /// asking GPUI whether its resize landed.
    #[cfg(target_os = "macos")]
    fn macos_window_content_size(&self, ns_window: isize) -> Option<(f64, f64)> {
        if ns_window == 0 {
            return None;
        }
        let _mtm = objc2::MainThreadMarker::new()?;
        // SAFETY: `ns_window` is one of our own NSWindow pointers, alive for
        // the process lifetime; the pointer is only used on the main thread.
        let window = unsafe { &*(ns_window as *const objc2_app_kit::NSWindow) };
        let content_view = window.contentView()?;
        let size = objc2_app_kit::NSView::frame(&content_view).size;
        Some((size.width, size.height))
    }

    /// True when the window content size matches `(w, h)` within 1 logical
    /// pixel (doc §3.3 tolerance).
    #[cfg(target_os = "macos")]
    fn macos_window_content_size_is(&self, ns_window: isize, w: f64, h: f64) -> bool {
        self.macos_window_content_size(ns_window)
            .is_some_and(|(cw, ch)| (cw - w).abs() <= 1.0 && (ch - h).abs() <= 1.0)
    }

    /// Compact the hidden main window to 1×1 so GPUI's MetalRenderer drops its
    /// large drawable-sized intermediate textures (doc §3.2). The resize is
    /// requested through the GPUI path; a background task confirms it reached
    /// the renderer and only then runs the allocator pressure relief (doc §3.5).
    #[cfg(target_os = "macos")]
    fn compact_main_macos_window(&mut self, cx: &mut Context<Self>) {
        let Some(window_handle) = self.main_window else {
            return;
        };
        if self.ns_window == 0 {
            return;
        }
        let Some(_mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        // SAFETY: `self.ns_window` is our own NSWindow, alive for the process
        // lifetime; used only on the main thread.
        let window = unsafe { &*(self.ns_window as *const objc2_app_kit::NSWindow) };

        // Keep the original restore target while re-compacting a window whose
        // asynchronous restore was interrupted by another hide. Reading the
        // current frame in that state could capture 1×1 or a partially restored
        // size and corrupt both the next show and shutdown persistence.
        let previous_state = self.main_compacted_state;
        let (content_w, content_h, min_size, saved_geometry) = if let Some(state) = previous_state {
            (
                state.content_size.0,
                state.content_size.1,
                objc2_foundation::NSSize::new(state.content_min_size.0, state.content_min_size.1),
                state.saved_geometry,
            )
        } else {
            let Some((content_w, content_h)) = self.macos_window_content_size(self.ns_window)
            else {
                return;
            };
            if content_w <= 1.0 && content_h <= 1.0 {
                return;
            }
            (
                content_w,
                content_h,
                window.contentMinSize(),
                (self.saved_x, self.saved_y, self.saved_w, self.saved_h),
            )
        };

        self.surface_generation = self.surface_generation.wrapping_add(1);
        let generation = self.surface_generation;
        self.main_compacted_state = Some(MacosCompactedWindowState {
            content_size: (content_w, content_h),
            content_min_size: (min_size.width, min_size.height),
            saved_geometry,
            generation,
        });

        // Temporarily relax the minimum size so AppKit doesn't clamp the
        // shrink (doc §4.3).
        window.setContentMinSize(objc2_foundation::NSSize::new(1.0, 1.0));
        if cx
            .update_window(window_handle, |_view, window, _cx| {
                window.resize(size(px(1.0), px(1.0)));
            })
            .is_err()
        {
            // Resize request failed — restore the minimum size and drop the
            // new state. Preserve an older state if this was a re-compaction
            // of an interrupted restore; it still contains the only safe
            // full-size geometry and restore target.
            window.setContentMinSize(min_size);
            self.main_compacted_state = previous_state;
            return;
        }

        log::debug!("surface compact: main {content_w:.0}×{content_h:.0} → 1×1 (gen {generation})");
        let ns_window_ptr = self.ns_window;
        self._main_compact_task = Some(cx.spawn(async move |weak_self, cx| {
            let deadline = Instant::now() + Duration::from_millis(100);
            loop {
                Timer::after(Duration::from_millis(10)).await;
                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let (alive, still_hidden, reached) = this
                    .update(cx, |wm, _cx| {
                        let alive = wm
                            .main_compacted_state
                            .as_ref()
                            .is_some_and(|s| s.generation == generation)
                            && wm.surface_generation == generation;
                        let reached = wm.macos_window_content_size_is(ns_window_ptr, 1.0, 1.0);
                        (alive, !wm.visible, reached)
                    })
                    .unwrap_or((false, false, false));
                if !alive || !still_hidden {
                    return; // superseded or re-shown before the shrink landed
                }
                if reached {
                    log::debug!("surface compact: main 1×1 resize confirmed (gen {generation})");
                    crate::platform::util::trim_process_working_set();
                    return;
                }
                if Instant::now() >= deadline {
                    log::warn!("surface compact: main resize to 1×1 not confirmed within 100ms");
                    return;
                }
            }
        }));
    }

    /// Restore the main window from its compacted 1×1 surface before it is
    /// shown again (doc §3.3). The restore is confirmed by polling the native
    /// content size — the GPUI resize has no completion callback — and the
    /// window is only ordered front afterwards.
    #[cfg(target_os = "macos")]
    fn restore_main_macos_window(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.main_compacted_state else {
            return;
        };
        // Cancel and invalidate the shrink confirmation, but retain the saved
        // compacted state until the resize is confirmed and the window is
        // ordered front. Geometry polling and shutdown must continue to ignore
        // the native frame while this asynchronous restore is in flight.
        self._main_compact_task = None;
        self.surface_generation = self.surface_generation.wrapping_add(1);
        let restore_generation = self.surface_generation;
        if let Some(compacted) = self.main_compacted_state.as_mut() {
            compacted.generation = restore_generation;
        }

        let ns_window = self.ns_window;
        if ns_window == 0 {
            self.macos_compaction_disabled = true;
            log::warn!("surface restore: main NSWindow unavailable; compaction disabled");
            return;
        }
        let Some(_mtm) = objc2::MainThreadMarker::new() else {
            self.macos_compaction_disabled = true;
            log::warn!("surface restore: main-thread marker unavailable; compaction disabled");
            return;
        };
        // SAFETY: our own NSWindow pointer, main thread only.
        let window = unsafe { &*(ns_window as *const objc2_app_kit::NSWindow) };
        // Restore the minimum size first so the resize below isn't clamped
        // (doc §4.3).
        window.setContentMinSize(objc2_foundation::NSSize::new(
            state.content_min_size.0,
            state.content_min_size.1,
        ));

        let target = state.content_size;
        let target_w = target.0;
        let target_h = target.1;
        // Request the restore through the GPUI resize path so the renderer
        // rebuilds drawable-sized resources (doc §2.3).
        let requested = self.main_window.is_some_and(|handle| {
            cx.update_window(handle, |_view, window, _cx| {
                window.resize(size(px(target_w as f32), px(target_h as f32)));
            })
            .is_ok()
        });
        if !requested {
            // Fallback: set the content size synchronously via AppKit.
            window.setContentSize(objc2_foundation::NSSize::new(target_w, target_h));
        }
        log::debug!("surface restore: main → {target_w:.0}×{target_h:.0}");

        self._main_restore_task = Some(cx.spawn(async move |weak_self, cx| {
            let deadline = Instant::now() + Duration::from_millis(100);
            loop {
                Timer::after(Duration::from_millis(10)).await;
                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let (active, still_visible, reached) = this
                    .update(cx, |wm, _cx| {
                        (
                            wm.surface_generation == restore_generation
                                && wm
                                    .main_compacted_state
                                    .as_ref()
                                    .is_some_and(|s| s.generation == restore_generation),
                            wm.visible,
                            wm.macos_window_content_size_is(ns_window, target_w, target_h),
                        )
                    })
                    .unwrap_or((false, false, false));
                if !active || !still_visible {
                    return; // superseded or hidden again while restoring
                }
                if reached {
                    break;
                }
                if Instant::now() >= deadline {
                    // Never leave the user unable to open the window: force
                    // the size synchronously and disable further compaction.
                    let _ = this.update(cx, |wm, _cx| {
                        if wm.surface_generation != restore_generation
                            || wm
                                .main_compacted_state
                                .as_ref()
                                .is_none_or(|s| s.generation != restore_generation)
                        {
                            return;
                        }
                        wm.macos_compaction_disabled = true;
                        let Some(_mtm) = objc2::MainThreadMarker::new() else {
                            return;
                        };
                        // SAFETY: our own NSWindow pointer, main thread only.
                        let window = unsafe { &*(ns_window as *const objc2_app_kit::NSWindow) };
                        window.setContentSize(objc2_foundation::NSSize::new(target_w, target_h));
                    });
                    log::warn!(
                        "surface restore: main resize not confirmed within 100ms; \
                         compaction disabled (gen {restore_generation})"
                    );
                    break;
                }
            }

            // Size reached (or was forced) — show the window now.
            let Some(this) = weak_self.upgrade() else {
                return;
            };
            let shown = this
                .update(cx, |wm, cx| {
                    if !wm.visible
                        || wm.surface_generation != restore_generation
                        || wm
                            .main_compacted_state
                            .as_ref()
                            .is_none_or(|s| s.generation != restore_generation)
                    {
                        return false;
                    }
                    // Clear the guard only in the same update that positions
                    // and shows the confirmed full-size window. This prevents
                    // the 200 ms geometry poll from ever observing an in-flight
                    // 1×1 restore as a normal visible window.
                    wm.main_compacted_state = None;
                    if let Some((x, y)) = wm.calculate_position() {
                        wm.position_macos_window(x, y);
                    }
                    // Keep the existing activation semantics of `show_and_focus`
                    // (doc §3.3): the app must be active before ordering front,
                    // otherwise keyboard events keep going to the previous app.
                    cx.activate(true);
                    wm.activate_macos_window();
                    true
                })
                .unwrap_or(false);
            if shown {
                log::debug!("surface restore: main window shown");
            }
        }));
    }

    /// Compact the hidden Quick Paste window to 1×1 (doc §3.4), mirroring
    /// `compact_main_macos_window`. The dynamic height is always re-applied by
    /// `position_quick_macos_window` on the next show, so only the minimum
    /// size needs to be restored then.
    #[cfg(target_os = "macos")]
    fn compact_quick_macos_window(&mut self, cx: &mut Context<Self>) {
        let Some(window_handle) = self.quick_window else {
            return;
        };
        if self.quick_ns_window == 0 || self.quick_compacted_state.is_some() {
            return;
        }
        let Some((content_w, content_h)) = self.macos_window_content_size(self.quick_ns_window)
        else {
            return;
        };
        if content_w <= 1.0 && content_h <= 1.0 {
            return; // already minimal
        }
        let Some(_mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        // SAFETY: our own NSWindow pointer, main thread only.
        let window = unsafe { &*(self.quick_ns_window as *const objc2_app_kit::NSWindow) };
        let min_size = window.contentMinSize();

        self.surface_generation = self.surface_generation.wrapping_add(1);
        let generation = self.surface_generation;
        self.quick_compacted_state = Some(MacosCompactedWindowState {
            content_size: (content_w, content_h),
            content_min_size: (min_size.width, min_size.height),
            // Quick Paste geometry is never persisted.
            saved_geometry: (0, 0, 0.0, 0.0),
            generation,
        });
        window.setContentMinSize(objc2_foundation::NSSize::new(1.0, 1.0));
        if cx
            .update_window(window_handle, |_view, window, _cx| {
                window.resize(size(px(1.0), px(1.0)));
            })
            .is_err()
        {
            window.setContentMinSize(min_size);
            self.quick_compacted_state = None;
            return;
        }

        log::debug!(
            "surface compact: quick {content_w:.0}×{content_h:.0} → 1×1 (gen {generation})"
        );
        let ns_window_ptr = self.quick_ns_window;
        self._quick_compact_task = Some(cx.spawn(async move |weak_self, cx| {
            let deadline = Instant::now() + Duration::from_millis(100);
            loop {
                Timer::after(Duration::from_millis(10)).await;
                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let (alive, still_hidden, reached) = this
                    .update(cx, |wm, _cx| {
                        let alive = wm
                            .quick_compacted_state
                            .as_ref()
                            .is_some_and(|s| s.generation == generation)
                            && wm.surface_generation == generation;
                        let reached = wm.macos_window_content_size_is(ns_window_ptr, 1.0, 1.0);
                        (alive, !wm.quick_visible, reached)
                    })
                    .unwrap_or((false, false, false));
                if !alive || !still_hidden {
                    return; // superseded or re-shown before the shrink landed
                }
                if reached {
                    log::debug!("surface compact: quick 1×1 resize confirmed (gen {generation})");
                    crate::platform::util::trim_process_working_set();
                    return;
                }
                if Instant::now() >= deadline {
                    log::warn!("surface compact: quick resize to 1×1 not confirmed within 100ms");
                    return;
                }
            }
        }));
    }

    /// Restore the Quick Paste window before showing it (doc §3.4).
    /// `position_quick_macos_window` applies the size synchronously via
    /// AppKit, so the confirmation is usually immediate; only a pathological
    /// case falls back to a short poll.
    #[cfg(target_os = "macos")]
    fn restore_quick_macos_window(&mut self, x: i32, y: i32, height: f32, cx: &mut Context<Self>) {
        if self.quick_ns_window == 0 {
            // Window unavailable — drop the state so the next show uses the
            // normal positioning path.
            self.quick_compacted_state = None;
            return;
        }
        let Some(state) = self.quick_compacted_state.take() else {
            return;
        };
        // Invalidate stale compaction tasks.
        self._quick_compact_task = None;
        self.surface_generation = self.surface_generation.wrapping_add(1);
        let restore_generation = self.surface_generation;

        if let Some(_mtm) = objc2::MainThreadMarker::new() {
            // SAFETY: our own NSWindow pointer, main thread only.
            let window = unsafe { &*(self.quick_ns_window as *const objc2_app_kit::NSWindow) };
            window.setContentMinSize(objc2_foundation::NSSize::new(
                state.content_min_size.0,
                state.content_min_size.1,
            ));
        }
        self.position_quick_macos_window(x, y, height);

        let target_w = QUICK_WINDOW_WIDTH as f64;
        let target_h = height as f64;
        if self.macos_window_content_size_is(self.quick_ns_window, target_w, target_h) {
            // Synchronous restore confirmed — show immediately.
            log::debug!("surface restore: quick size confirmed synchronously");
            self.show_quick_macos_window();
            return;
        }

        // Defensive path: poll briefly, then show regardless so the popup can
        // never be stuck hidden.
        log::warn!("surface restore: quick size not confirmed synchronously; polling");
        let ns_window = self.quick_ns_window;
        self._quick_restore_task = Some(cx.spawn(async move |weak_self, cx| {
            let deadline = Instant::now() + Duration::from_millis(50);
            loop {
                Timer::after(Duration::from_millis(5)).await;
                let Some(this) = weak_self.upgrade() else {
                    return;
                };
                let decision = this
                    .update(cx, |wm, _cx| {
                        if !wm.quick_visible || wm.surface_generation != restore_generation {
                            return Some(false);
                        }
                        if wm.macos_window_content_size_is(ns_window, target_w, target_h) {
                            Some(true)
                        } else if Instant::now() >= deadline {
                            wm.macos_compaction_disabled = true;
                            log::warn!(
                                "surface restore: quick resize not confirmed within 50ms; \
                                 compaction disabled (gen {restore_generation})"
                            );
                            Some(true)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(Some(false));
                match decision {
                    Some(true) => {
                        let _ = this.update(cx, |wm, _cx| {
                            if wm.quick_visible && wm.surface_generation == restore_generation {
                                wm.show_quick_macos_window();
                            }
                        });
                        return;
                    }
                    Some(false) => return,
                    None => {}
                }
            }
        }));
    }

    #[cfg(target_os = "macos")]
    fn activate_macos_window(&self) {
        if self.ns_window == 0 {
            return;
        }
        unsafe {
            let window = &*(self.ns_window as *const objc2_app_kit::NSWindow);
            window.makeKeyAndOrderFront(None);
        }
    }

    #[cfg(target_os = "macos")]
    fn show_quick_macos_window(&self) {
        if self.quick_ns_window == 0 {
            return;
        }
        unsafe {
            let window = &*(self.quick_ns_window as *const objc2_app_kit::NSWindow);
            window.orderFrontRegardless();
        }
    }

    #[cfg(target_os = "macos")]
    fn hide_quick_macos_window(&self) {
        if self.quick_ns_window == 0 {
            return;
        }
        unsafe {
            let window = &*(self.quick_ns_window as *const objc2_app_kit::NSWindow);
            window.orderOut(None);
        }
    }

    #[cfg(target_os = "macos")]
    fn position_quick_macos_window(&self, x: i32, y: i32, height: f32) {
        if self.quick_ns_window == 0 {
            return;
        }
        let Some(primary_height) = monitor::primary_screen_height() else {
            return;
        };
        let top = primary_height - y as f64;
        unsafe {
            let window = &*(self.quick_ns_window as *const objc2_app_kit::NSWindow);
            window.setContentSize(objc2_foundation::NSSize::new(
                QUICK_WINDOW_WIDTH as f64,
                height as f64,
            ));
            window.setFrameTopLeftPoint(objc2_foundation::NSPoint::new(x as f64, top));
        }
    }

    #[cfg(target_os = "macos")]
    fn hide_macos_window(&self) {
        if self.ns_window == 0 {
            return;
        }
        unsafe {
            let window = &*(self.ns_window as *const objc2_app_kit::NSWindow);
            window.orderOut(None);
        }
    }

    #[cfg(target_os = "macos")]
    fn position_macos_window(&self, x: i32, y: i32) {
        if self.ns_window == 0 {
            return;
        }
        let Some(primary_height) = monitor::primary_screen_height() else {
            return;
        };
        let top = primary_height - y as f64;
        unsafe {
            let window = &*(self.ns_window as *const objc2_app_kit::NSWindow);
            window.setFrameTopLeftPoint(objc2_foundation::NSPoint::new(x as f64, top));
        }
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;

        // --- Platform-level topmost / floating window control ---
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                SWP_SHOWWINDOW,
            };
            let hwnd = self.hwnd as *mut std::ffi::c_void;
            if !hwnd.is_null() {
                let insert_after = if pinned { HWND_TOPMOST } else { HWND_NOTOPMOST };
                // SAFETY: HWND is our own window. `SetWindowPos` with
                // SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE only changes Z-order.
                unsafe {
                    SetWindowPos(
                        hwnd,
                        insert_after,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            if self.ns_window != 0 {
                let level = if pinned {
                    objc2_app_kit::NSFloatingWindowLevel
                } else {
                    objc2_app_kit::NSNormalWindowLevel
                };
                unsafe {
                    let window = &*(self.ns_window as *const objc2_app_kit::NSWindow);
                    window.setLevel(level);
                }
            }
        }

        cx.emit(WindowManagerEvent::PinnedChanged(pinned));
    }

    pub fn set_auto_hide(&mut self, auto_hide: bool) {
        self.auto_hide = auto_hide;
    }

    /// Apply taskbar / Dock icon visibility based on settings.
    /// On Windows: toggles WS_EX_TOOLWINDOW extended style.
    /// On macOS: toggles NSApplication activation policy.
    pub fn apply_taskbar_visibility(&self, hide: bool, _cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, SWP_FRAMECHANGED,
                SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW,
                WS_EX_TOOLWINDOW,
            };

            let hwnd = self.hwnd as *mut std::ffi::c_void;
            if hwnd.is_null() {
                return;
            }

            // SAFETY: HWND is our own window. `GetWindowLongW`/`SetWindowLongW`
            // read/write our own extended style bits. `SetWindowPos` with
            // SWP_FRAMECHANGED forces a taskbar re-evaluation.
            unsafe {
                let mut ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
                if hide {
                    // Remove APPWINDOW (forces taskbar) and add TOOLWINDOW (hides from taskbar)
                    ex_style &= !(WS_EX_APPWINDOW as i32);
                    ex_style |= WS_EX_TOOLWINDOW as i32;
                } else {
                    // Restore APPWINDOW (show in taskbar) and remove TOOLWINDOW
                    ex_style |= WS_EX_APPWINDOW as i32;
                    ex_style &= !(WS_EX_TOOLWINDOW as i32);
                }
                SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style);
                // --- Force Windows to re-evaluate the window's taskbar button ---
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
        }
        #[cfg(target_os = "macos")]
        {
            let Some(mtm) = objc2::MainThreadMarker::new() else {
                return;
            };
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let policy = if hide {
                objc2_app_kit::NSApplicationActivationPolicy::Accessory
            } else {
                objc2_app_kit::NSApplicationActivationPolicy::Regular
            };
            if !app.setActivationPolicy(policy) {
                log::warn!("Failed to set macOS activation policy for Dock icon visibility");
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = hide;
            let _ = _cx;
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn set_hide_taskbar_icon(&mut self, hide: bool, cx: &mut Context<Self>) {
        self.apply_taskbar_visibility(hide, cx);
    }

    /// Update tray menu texts when language changes.
    pub fn update_tray_language(&mut self) {
        if let Some(ref mut tray) = self.tray {
            tray.update_language();
        }
    }

    pub fn set_position_mode(&mut self, mode: PositionMode) {
        self.position_mode = mode;
    }

    pub fn start_hotkey_recording(&mut self, cx: &mut Context<Self>) {
        // Reject main hotkey recording when Win+V takeover is enabled.
        if self.win_v_takeover_status != WinVTakeoverStatus::Disabled {
            return;
        }
        if let Some(ref mut hk) = self.hotkey {
            hk.unregister();
            hk.start_recording();
            self.start_recording_poll(cx);
        }
    }

    /// Start recording the quick window hotkey.
    pub fn start_quick_hotkey_recording(&mut self, cx: &mut Context<Self>) {
        if let Some(ref mut hk) = self.hotkey {
            hk.unregister();
            hk.start_recording();
            self.start_recording_poll(cx);
        }
    }

    /// Reload quick hotkey registration (called when quick window is enabled).
    pub fn reload_quick_hotkey(&mut self, cx: &mut Context<Self>) {
        let quick_hotkey = self.state.read(cx).settings.quick_hotkey.clone();
        if let Some(ref mut hk) = self.hotkey {
            if let Err(e) = hk.update_quick_hotkey(&quick_hotkey) {
                self.state.update(cx, |state, _cx| {
                    state.settings.quick_hotkey_enabled = false;
                    state.settings.save();
                    state.toast_message = Some(e);
                    state.toast_is_warning = true;
                });
            }
            hk.set_quick_actions_enabled(false);
        }
    }

    /// Disable quick hotkey (called when quick window is disabled).
    pub fn disable_quick_hotkey(&mut self) {
        if let Some(ref mut hk) = self.hotkey {
            let _ = hk.update_quick_hotkey("");
        }
    }

    /// Start recording a paste shortcut for the given app.
    pub fn start_paste_shortcut_recording(&mut self, app_name: String, cx: &mut Context<Self>) {
        self.recording_paste_shortcut_app = Some(app_name);
        self.start_hotkey_recording(cx); // reuse recording infra
    }

    /// Cancel a paste shortcut recording — re-register the global hotkey
    /// and clear the recording flag.
    pub fn cancel_paste_shortcut_recording(&mut self) {
        self.recording_paste_shortcut_app = None;
        if let Some(ref mut hk) = self.hotkey {
            hk.finish_recording();
            hk.register();
        }
    }

    /// Start recording a per-item custom hotkey.
    pub fn start_item_hotkey_recording(&mut self, id: i64, format: String, cx: &mut Context<Self>) {
        self.recording_item_hotkey_id = Some(id);
        self.recording_item_hotkey_format = Some(format);
        self.recording_latest_slot = None;
        if let Some(ref mut hk) = self.hotkey {
            hk.start_custom_recording();
            self.start_recording_poll(cx);
        }
    }

    pub fn update_recording_item_hotkey_format(&mut self, format: String) {
        if self.recording_item_hotkey_id.is_some() {
            self.recording_item_hotkey_format = Some(format);
        }
    }

    pub fn recording_latest_slot(&self) -> Option<usize> {
        self.recording_latest_slot
    }

    /// Start recording a latest-N slot hotkey.
    pub fn start_latest_slot_recording(&mut self, slot: usize, cx: &mut Context<Self>) {
        self.recording_latest_slot = Some(slot);
        self.recording_item_hotkey_id = None;
        self.recording_item_hotkey_format = None;
        if let Some(ref mut hk) = self.hotkey {
            hk.start_custom_recording();
            self.start_recording_poll(cx);
        }
    }

    /// Unregister a latest-N slot hotkey (called from settings popup clear button).
    pub fn unregister_latest_slot_hotkey(&mut self, slot: usize) {
        if let Some(ref mut hk) = self.hotkey {
            hk.unregister_latest_hotkey(slot);
        }
    }

    /// Unregister a per-item hotkey (called when item is deleted or hotkey cleared).
    pub fn unregister_item_hotkey_if_set(&mut self, id: i64) {
        if let Some(ref mut hk) = self.hotkey {
            hk.unregister_item_hotkey(id);
        }
    }

    /// Cancel any custom recording (per-item or latest-N).
    pub fn cancel_custom_recording(&mut self, cx: &mut Context<Self>) {
        self.recording_item_hotkey_id = None;
        self.recording_item_hotkey_format = None;
        self.recording_latest_slot = None;
        if let Some(ref mut hk) = self.hotkey {
            hk.finish_recording();
            hk.register();
        }
        self.state.update(cx, |state, _cx| {
            state.pending_single_hotkey = None;
        });
    }

    /// Cancel the currently active hotkey recording, if any.
    pub fn cancel_active_hotkey_recording(&mut self, cx: &mut Context<Self>) -> bool {
        let state_recording = self.state.read(cx).hotkey_recording
            || self.state.read(cx).recording_quick_hotkey
            || self.recording_paste_shortcut_app.is_some()
            || self.recording_item_hotkey_id.is_some()
            || self.recording_latest_slot.is_some()
            || self
                .hotkey
                .as_ref()
                .is_some_and(|hotkey| hotkey.is_recording());
        if !state_recording {
            return false;
        }

        self.recording_paste_shortcut_app = None;
        self.recording_item_hotkey_id = None;
        self.recording_item_hotkey_format = None;
        self.recording_latest_slot = None;
        self.state.update(cx, |state, _cx| {
            state.hotkey_recording = false;
            state.recording_quick_hotkey = false;
            state.pending_single_hotkey = None;
        });
        if let Some(ref mut hk) = self.hotkey {
            hk.finish_recording();
            hk.register();
        }
        cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
        cx.notify();
        true
    }

    /// Confirm the first modifier-less key as a single-press hotkey.
    pub fn confirm_pending_single_hotkey(&mut self, cx: &mut Context<Self>) -> bool {
        let confirmed = self
            .hotkey
            .as_mut()
            .is_some_and(|hotkey| hotkey.confirm_pending_single());
        if confirmed {
            cx.notify();
        }
        confirmed
    }

    /// Register the item hotkey that was just recorded.
    fn commit_item_hotkey(&mut self, id: i64, hotkey: &str, format: &str, cx: &mut Context<Self>) {
        let state = self.state.clone();
        if let Some(ref mut hk) = self.hotkey {
            match hk.register_item_hotkey(id, hotkey) {
                Ok(()) => {
                    state.update(cx, move |s, _cx| {
                        s.update_item_hotkey(id, hotkey, format);
                    });
                }
                Err(e) => {
                    state.update(cx, |s, _cx| {
                        s.toast_message = Some(e);
                        s.toast_is_warning = true;
                    });
                }
            }
        }
        self.cancel_custom_recording(cx);
        cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
        cx.notify();
    }

    /// Register the latest-N slot hotkey that was just recorded.
    fn commit_latest_hotkey(&mut self, slot: usize, hotkey: &str, cx: &mut Context<Self>) {
        let state = self.state.clone();
        if let Some(ref mut hk) = self.hotkey {
            match hk.register_latest_hotkey(slot, hotkey) {
                Ok(()) => {
                    state.update(cx, |s, _cx| {
                        if slot < s.settings.latest_hotkeys.len() {
                            s.settings.latest_hotkeys[slot].hotkey = hotkey.to_string();
                            s.settings.save();
                        }
                    });
                }
                Err(e) => {
                    state.update(cx, |s, _cx| {
                        s.toast_message = Some(e);
                        s.toast_is_warning = true;
                    });
                }
            }
        }
        self.cancel_custom_recording(cx);
        cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
        cx.notify();
    }

    pub fn toggle_sync_auto_enabled(&mut self, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            let next = !state.settings.sync_auto_enabled;
            state.settings.sync_auto_enabled = next;
            if next {
                // 重新打开：按记忆恢复后端启用状态
                let saved_ids = &state.settings.saved_enabled_backend_ids;
                for backend in state.settings.sync_backends.iter_mut() {
                    backend.enabled = saved_ids.contains(&backend.id);
                }
            } else {
                // 关闭：保存当前启用状态，再全部关闭
                state.settings.saved_enabled_backend_ids = state
                    .settings
                    .sync_backends
                    .iter()
                    .filter(|b| b.enabled)
                    .map(|b| b.id.clone())
                    .collect();
                for backend in state.settings.sync_backends.iter_mut() {
                    backend.enabled = false;
                }
            }
            state.settings.save();
            state.sync = crate::state::sync::SyncState::from_settings(&state.settings);
            Some(state.settings.clone())
        });
        if let Some(settings) = settings {
            self.sync_service.reload_from_settings(&settings);
            if settings.sync_auto_enabled {
                self.sync_service.trigger_pull_all();
            }
            cx.emit(WindowManagerEvent::SyncChanged);
        } else {
            cx.emit(WindowManagerEvent::SyncChanged);
        }
    }

    pub fn toggle_transfer_station(&mut self, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            if state.settings.transfer_station_enabled && state.transfer_busy {
                state.toast_message = Some(I18nKey::TransferBusy.text().into());
                return state.settings.clone();
            }
            let has_backend = state
                .settings
                .sync_backends
                .iter()
                .any(|backend| backend.enabled);
            if !state.settings.transfer_station_enabled && !has_backend {
                state.toast_message = Some(I18nKey::TransferNoBackend.text().into());
                return state.settings.clone();
            }
            state.settings.transfer_station_enabled = !state.settings.transfer_station_enabled;
            if state.settings.transfer_station_enabled
                && !state.settings.sync_backends.iter().any(|backend| {
                    backend.enabled && backend.id == state.settings.transfer_backend_id
                })
            {
                state.settings.transfer_backend_id = state
                    .settings
                    .sync_backends
                    .iter()
                    .find(|backend| backend.enabled)
                    .map(|backend| backend.id.clone())
                    .unwrap_or_default();
            }
            if !state.settings.transfer_station_enabled {
                state.transfer_filter_active = false;
                state.pending_transfer_commands.clear();
                state.pending_transfer_downloads.clear();
                state.pending_transfer_uploads.clear();
            }
            state.settings.save();
            state.settings.clone()
        });
        self.transfer_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
        cx.emit(WindowManagerEvent::ClipboardChanged);
    }

    pub fn set_transfer_retention_days(&mut self, days: u32, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| {
            state.settings.transfer_retention_days = days;
            state.settings.save();
        });
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    pub fn set_transfer_backend(&mut self, id: &str, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            if state.transfer_busy && state.settings.transfer_backend_id != id {
                state.toast_message = Some(I18nKey::TransferBusy.text().into());
                return state.settings.clone();
            }
            if state
                .settings
                .sync_backends
                .iter()
                .any(|backend| backend.enabled && backend.id == id)
            {
                state.settings.transfer_backend_id = id.to_string();
                state.transfer_entries.clear();
                state.pending_transfer_downloads.clear();
                state.pending_transfer_uploads.clear();
                if state.transfer_filter_active {
                    state
                        .pending_transfer_commands
                        .push_back(crate::services::transfer_station::TransferCommand::Refresh);
                }
                state.settings.save();
            }
            state.settings.clone()
        });
        self.transfer_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
        cx.emit(WindowManagerEvent::ClipboardChanged);
    }

    pub fn toggle_sync_favorites_only(&mut self, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            state.settings.sync_favorites_only = !state.settings.sync_favorites_only;
            state.settings.save();
            state.sync.favorites_only = state.settings.sync_favorites_only;
            state.settings.clone()
        });
        self.sync_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    pub fn toggle_sync_include_images(&mut self, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            state.settings.sync_include_images = !state.settings.sync_include_images;
            // If turning off include_images, also turn off compress_images
            if !state.settings.sync_include_images {
                state.settings.sync_compress_images = false;
            }
            state.settings.save();
            state.sync.include_images = state.settings.sync_include_images;
            state.sync.compress_images = state.settings.sync_compress_images;
            state.settings.clone()
        });
        self.sync_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    pub fn toggle_sync_compress_images(&mut self, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            state.settings.sync_compress_images = !state.settings.sync_compress_images;
            state.settings.save();
            state.sync.compress_images = state.settings.sync_compress_images;
            state.settings.clone()
        });
        self.sync_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    pub fn set_backend_sync_interval(&mut self, id: &str, secs: u64, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            if let Some(config) = state.settings.sync_backends.iter_mut().find(|c| c.id == id) {
                config.sync_interval_secs = Some(secs);
                state.settings.save();
            }
            state.sync = crate::state::sync::SyncState::from_settings(&state.settings);
            state.settings.clone()
        });
        self.sync_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    pub fn sync_backend_now(&mut self, id: &str, cx: &mut Context<Self>) {
        self.sync_service.trigger_backend_sync(id);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    /// Shared helper: push a new backend config and sync state.
    fn _add_backend_config(
        &mut self,
        config: crate::core::settings::BackendConfig,
        cx: &mut Context<Self>,
    ) {
        if !self.state.read(cx).settings.sync_auto_enabled {
            return;
        }
        let settings = self.state.update(cx, |state, _cx| {
            state.settings.sync_backends.push(config);
            state.settings.save();
            state.sync = crate::state::sync::SyncState::from_settings(&state.settings);
            state.settings.clone()
        });
        self.sync_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    /// Shared helper: update an existing backend and sync state.
    fn _update_backend_and_sync(&mut self, cx: &mut Context<Self>) {
        let settings = self.state.update(cx, |state, _cx| {
            state.sync = crate::state::sync::SyncState::from_settings(&state.settings);
            state.settings.clone()
        });
        self.sync_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    pub fn add_local_folder_backend(
        &mut self,
        name: String,
        folder_path: String,
        cx: &mut Context<Self>,
    ) {
        self._add_backend_config(
            crate::core::settings::BackendConfig {
                id: crate::core::settings::generate_id(),
                enabled: true,
                backend_type: "local_folder".into(),
                name,
                folder_path,
                device_name: String::new(),
                last_sync_at: String::new(),
                last_item_count: 0,
                last_tag_count: 0,
                sync_interval_secs: Some(60),
                webdav_url: String::new(),
                webdav_root_url: String::new(),
                webdav_path: String::new(),
                webdav_username: String::new(),
                webdav_password: String::new(),
            },
            cx,
        );
    }

    pub fn add_webdav_backend(&mut self, form: WebDavBackendForm, cx: &mut Context<Self>) {
        let url = crate::core::settings::compose_webdav_url(&form.root_url, &form.path);
        self._add_backend_config(
            crate::core::settings::BackendConfig {
                id: crate::core::settings::generate_id(),
                enabled: true,
                backend_type: "webdav".into(),
                name: form.name,
                folder_path: String::new(),
                device_name: crate::services::backends::local_folder::hostname(),
                last_sync_at: String::new(),
                last_item_count: 0,
                last_tag_count: 0,
                sync_interval_secs: Some(600),
                webdav_url: url,
                webdav_root_url: form.root_url.trim().trim_end_matches('/').to_string(),
                webdav_path: form.path.trim().trim_matches('/').to_string(),
                webdav_username: form.username,
                webdav_password: form.password,
            },
            cx,
        );
    }

    pub fn edit_backend(
        &mut self,
        id: &str,
        name: String,
        folder_path: String,
        cx: &mut Context<Self>,
    ) {
        if !self.state.read(cx).settings.sync_auto_enabled {
            return;
        }
        self.state.update(cx, |state, _cx| {
            if let Some(config) = state.settings.sync_backends.iter_mut().find(|c| c.id == id) {
                config.name = name;
                config.folder_path = folder_path;
                state.settings.save();
            }
        });
        self._update_backend_and_sync(cx);
    }

    pub fn edit_webdav_backend(
        &mut self,
        id: &str,
        form: WebDavBackendForm,
        cx: &mut Context<Self>,
    ) {
        if !self.state.read(cx).settings.sync_auto_enabled {
            return;
        }
        let url = crate::core::settings::compose_webdav_url(&form.root_url, &form.path);
        self.state.update(cx, |state, _cx| {
            if let Some(config) = state.settings.sync_backends.iter_mut().find(|c| c.id == id) {
                config.name = form.name;
                config.webdav_url = url;
                config.webdav_root_url = form.root_url.trim().trim_end_matches('/').to_string();
                config.webdav_path = form.path.trim().trim_matches('/').to_string();
                config.webdav_username = form.username;
                if !form.password.is_empty() {
                    config.webdav_password = form.password;
                }
                state.settings.save();
            }
        });
        self._update_backend_and_sync(cx);
    }

    pub fn remove_sync_backend(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.state.read(cx).settings.sync_auto_enabled {
            return;
        }
        let settings = self.state.update(cx, |state, _cx| {
            state
                .settings
                .sync_backends
                .retain(|config| config.id != id);
            state.settings.save();
            state.sync = crate::state::sync::SyncState::from_settings(&state.settings);
            state.settings.clone()
        });
        self.sync_service.reload_from_settings(&settings);
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    pub fn toggle_sync_backend(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.state.read(cx).settings.sync_auto_enabled {
            return;
        }
        let (settings, enabled) = self.state.update(cx, |state, _cx| {
            let mut enabled = false;
            if let Some(config) = state.settings.sync_backends.iter_mut().find(|c| c.id == id) {
                config.enabled = !config.enabled;
                enabled = config.enabled;
                state.settings.save();
            }
            state.sync = crate::state::sync::SyncState::from_settings(&state.settings);
            (state.settings.clone(), enabled)
        });
        self.sync_service.reload_from_settings(&settings);
        if enabled {
            self.sync_service.trigger_backend_sync(id);
        }
        cx.emit(WindowManagerEvent::SyncChanged);
    }

    /// Apply system window behavior blocking (double-click maximize, Aero Snap, etc.)
    /// while preserving manual window resize.
    ///
    /// On Windows:
    /// - Removes `WS_MAXIMIZEBOX` to disable maximize button + double-click maximize.
    /// - Keeps `WS_THICKFRAME` so manual resize handles remain functional.
    /// - Installs a window subclass that intercepts `WM_WINDOWPOSCHANGING` during
    ///   title-bar drags to prevent Aero Snap (edge-triggered auto-resize).
    pub fn set_block_system_window_behaviors(&mut self, block: bool, _cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetWindowLongW, SetWindowLongPtrW, SetWindowLongW, SetWindowPos, GWLP_WNDPROC,
                GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
                WS_MAXIMIZEBOX,
            };

            BLOCK_SYSTEM_WINDOW_BEHAVIORS.store(block, Ordering::Release);

            let hwnd = self.hwnd as *mut std::ffi::c_void;
            if hwnd.is_null() {
                return;
            }

            // SAFETY: HWND is our own window. `GetWindowLongW`/`SetWindowLongW`
            // read/write our own style bits. We only toggle MAXIMIZEBOX —
            // THICKFRAME (resize handles) is intentionally left untouched so
            // manual resize still works.
            unsafe {
                // --- Toggle WS_MAXIMIZEBOX window style ---
                let style = GetWindowLongW(hwnd, GWL_STYLE);
                let new_style = if block {
                    style & !(WS_MAXIMIZEBOX as i32)
                } else {
                    style | WS_MAXIMIZEBOX as i32
                };
                SetWindowLongW(hwnd, GWL_STYLE, new_style);
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );

                // --- Install window subclass to intercept Aero Snap ---
                // We replace the window procedure once; the subclass proc checks
                // the BLOCK_SYSTEM_WINDOW_BEHAVIORS flag on every message.
                if block && ORIGINAL_WNDPROC.load(Ordering::Acquire) == 0 {
                    let old_proc = SetWindowLongPtrW(
                        hwnd,
                        GWLP_WNDPROC,
                        clippi_subclass_proc as *const () as isize,
                    );
                    ORIGINAL_WNDPROC.store(old_proc, Ordering::Release);
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = block;
            let _ = _cx;
        }
    }

    /// Replace the internal blacklist with the given list (used for sync from settings).
    pub fn set_blacklist(&mut self, blacklist: Vec<String>) {
        self.blacklist = blacklist;
    }

    /// Push the clipboard-app blacklist snapshot to the listener thread.
    /// Call after every add / remove so the next poll picks up the change.
    pub fn set_clipboard_app_blacklist(&self, blacklist: Vec<String>) {
        self.clipboard_service.set_app_blacklist(blacklist);
    }

    // ─── Update ──────────────────────────────────────────────────────────────

    /// Poll the shared update state from background threads and emit events.
    fn poll_update(&mut self, cx: &mut Context<Self>) {
        // 1. Read update phase from background thread
        if let Ok(mut phase) = self.pending_update_phase.lock() {
            let current = phase.clone();
            if *phase != update::UpdatePhase::Idle {
                // Update AppState for settings page rendering
                self.state
                    .update(cx, |s, _| s.update_phase = current.clone());
                cx.emit(WindowManagerEvent::UpdateProgress(current));
                // Reset to Idle after emitting so we don't repeat
                *phase = update::UpdatePhase::Idle;
            }
        }

        // 2. Check restart installer result (non-blocking — see do_update_restart)
        let is_installing = self.state.read(cx).update_phase == update::UpdatePhase::Installing;
        let restart_result: Option<Result<(), String>> = if is_installing {
            self.pending_restart_result
                .lock()
                .ok()
                .and_then(|mut p| p.take())
        } else {
            None
        };
        if let Some(result) = restart_result {
            match result {
                Ok(()) => {
                    log::info!("Windows silent update installer launched");
                    self.prepare_shutdown(cx);
                    cx.quit();
                    return; // Don't continue polling after quit
                }
                Err(error) => {
                    log::error!("Failed to launch prepared update: {error}");
                    let message = update::user_message_for_kind(update::UpdateErrorKind::Launch);
                    self.state.update(cx, |state, _| {
                        state.update_phase = update::UpdatePhase::Error(message.clone())
                    });
                    cx.emit(WindowManagerEvent::UpdateProgress(
                        update::UpdatePhase::Error(message),
                    ));
                }
            }
        }

        // 3. Read update info from background thread
        if let Ok(mut pending) = self.pending_update.lock() {
            if let Some(info) = pending.take() {
                log::info!(
                    "Update available: {} -> {}",
                    env!("CARGO_PKG_VERSION"),
                    info.latest_version
                );
                self.state
                    .update(cx, |s, _| s.update_available = Some(info.clone()));
                // Update tray red dot
                if let Some(ref mut tray) = self.tray {
                    tray.set_update_available(true);
                }
                cx.emit(WindowManagerEvent::UpdateAvailable);
            }
        }

        // 4. Periodic check: use the software update frequency and persist
        // attempt time, so offline restarts do not repeatedly surface errors.
        let settings = self.state.read(cx).settings.clone();
        if settings.auto_check_updates
            && !self.update_check_running.load(Ordering::Acquire)
            && update::scheduled_update_check_due(
                &settings.update_last_check_at,
                chrono::Utc::now(),
            )
        {
            self.start_scheduled_update_check(cx);
        }
    }

    /// Start an update check (manual or periodic). Spawns a background thread.
    pub fn start_update_check(&mut self, cx: &mut Context<Self>) {
        self.start_update_check_with_mode(UpdateCheckMode::Manual, cx);
    }

    fn start_scheduled_update_check(&mut self, cx: &mut Context<Self>) {
        self.start_update_check_with_mode(UpdateCheckMode::Scheduled, cx);
    }

    fn start_update_check_with_mode(&mut self, mode: UpdateCheckMode, cx: &mut Context<Self>) {
        if matches!(
            self.state.read(cx).update_phase,
            update::UpdatePhase::Checking
                | update::UpdatePhase::UpdateAvailable
                | update::UpdatePhase::Downloading { .. }
                | update::UpdatePhase::Verifying
                | update::UpdatePhase::Installing
                | update::UpdatePhase::ReadyToRestart
        ) {
            return;
        }
        if self
            .update_check_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.state.update(cx, |s, _| {
            s.settings.update_last_check_at = chrono::Utc::now().to_rfc3339();
            s.settings.save();
            if mode == UpdateCheckMode::Manual {
                s.update_phase = update::UpdatePhase::Checking;
            }
        });
        if mode == UpdateCheckMode::Manual {
            cx.emit(WindowManagerEvent::UpdateProgress(
                update::UpdatePhase::Checking,
            ));
        }

        let pending = self.pending_update.clone();
        let pending_phase = self.pending_update_phase.clone();
        let running = self.update_check_running.clone();
        std::thread::spawn(move || {
            let checker =
                update::UpdateChecker::new(env!("CARGO_PKG_VERSION"), "Ruszero01", "clippi");
            let result = checker.check_full();
            match result {
                Ok(Some(info)) => {
                    log::info!("[wm] update available: {}", info.latest_version);
                    if let Ok(mut p) = pending.lock() {
                        *p = Some(info);
                    }
                    if let Ok(mut phase) = pending_phase.lock() {
                        *phase = update::UpdatePhase::UpdateAvailable;
                    }
                }
                Ok(None) => {
                    if let Ok(mut phase) = pending_phase.lock() {
                        *phase = update::UpdatePhase::UpToDate;
                    }
                }
                Err(error) => {
                    log::warn!(
                        "[wm] update check failed ({:?}): {}",
                        error.kind(),
                        error.detail()
                    );
                    if mode == UpdateCheckMode::Manual {
                        if let Ok(mut phase) = pending_phase.lock() {
                            *phase = update::UpdatePhase::Error(error.user_message());
                        }
                    } else if !error.is_network_failure() {
                        log::warn!("[wm] scheduled update check suppressed UI error");
                    }
                }
            }
            running.store(false, Ordering::Release);
        });
    }

    /// Start downloading, verifying, and preparing an update.
    pub fn start_update_download(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.state.read(cx).update_phase,
            update::UpdatePhase::Downloading { .. }
                | update::UpdatePhase::Verifying
                | update::UpdatePhase::Installing
        ) {
            return;
        }
        let info = match self.state.read(cx).update_available.clone() {
            Some(info) => info,
            None => return,
        };

        self.state.update(cx, |s, _| {
            s.update_phase = update::UpdatePhase::Downloading { progress: 0 }
        });
        cx.emit(WindowManagerEvent::UpdateProgress(
            update::UpdatePhase::Downloading { progress: 0 },
        ));

        let pending_phase = self.pending_update_phase.clone();
        let pending_phase_err = self.pending_update_phase.clone();
        std::thread::spawn(move || {
            let result = crate::services::updater::download_and_prepare(&info, move |phase| {
                if let Ok(mut p) = pending_phase.lock() {
                    *p = phase;
                }
            });
            if let Err(e) = result {
                log::error!("[wm] update download/prepare failed: {e}");
                if let Ok(mut p) = pending_phase_err.lock() {
                    *p = update::UpdatePhase::Error(update::summarize_update_error(&e));
                }
            }
        });
    }

    /// Launch the prepared platform installer (non-blocking).
    ///
    /// Spawns the installer in a background thread and sets the phase to
    /// `Installing`. The result is picked up by `poll_update` so the UI
    /// stays responsive while waiting for the UAC dialog or platform prompt.
    pub fn do_update_restart(&mut self, cx: &mut Context<Self>) {
        let Some(info) = self.state.read(cx).update_available.clone() else {
            log::error!("do_update_restart called but update_available is None");
            return;
        };

        self.state
            .update(cx, |s, _| s.update_phase = update::UpdatePhase::Installing);
        cx.emit(WindowManagerEvent::UpdateProgress(
            update::UpdatePhase::Installing,
        ));

        #[cfg(target_os = "windows")]
        {
            let hwnd = self.hwnd;
            let pending = self.pending_restart_result.clone();
            std::thread::spawn(move || {
                let result = crate::services::updater::launch_prepared_update(&info, hwnd);
                if let Ok(mut p) = pending.lock() {
                    *p = Some(result);
                }
            });
        }

        #[cfg(not(target_os = "windows"))]
        {
            let pending = self.pending_restart_result.clone();
            std::thread::spawn(move || {
                let result = crate::services::updater::launch_prepared_update(&info, 0);
                if let Ok(mut p) = pending.lock() {
                    *p = Some(result);
                }
            });
        }
    }

    /// Release platform resources on shutdown.
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            // Do not create new compaction tasks while quitting. The existing
            // tasks capture raw NSWindow pointers and must be cancelled before
            // GPUI/AppKit starts destroying those windows.
            self.macos_compaction_disabled = true;
            self.surface_generation = self.surface_generation.wrapping_add(1);
        }
        self.hide_quick_window(cx);
        #[cfg(target_os = "macos")]
        {
            self._main_compact_task = None;
            self._quick_compact_task = None;
            self._main_restore_task = None;
            self._quick_restore_task = None;
        }
        if let Some(ref mut hk) = self.hotkey {
            hk.stop();
        }
        if let Some(ref mut fw) = self.focus_watcher {
            fw.stop();
        }
    }

    /// Run periodic maintenance for local caches, retained history, and transfer files.
    fn poll_cleanup(&mut self, cx: &mut Context<Self>) {
        if self.maintenance_job_running {
            return; // A job is already in progress.
        }
        if let Some(retry_after) = self.maintenance_retry_after {
            if Instant::now() < retry_after {
                return; // A failed job is in its retry backoff window.
            }
            self.maintenance_retry_after = None;
        }

        let settings = self.state.read(cx).settings.clone();
        let transfer_available = self.state.read(cx).transfer_available();
        let interval = settings.cleanup_interval.as_str();
        let retention_days = settings.retention_days;
        let today = chrono::Local::now().date_naive();
        let cache_needed = cache_cleanup_due(interval, &settings.cleanup_last_date, today);
        let retention_needed =
            retention_cleanup_due(retention_days, &settings.retention_cleanup_last_date, today);
        let transfer_needed = transfer_cleanup_due(
            transfer_available,
            settings.transfer_retention_days,
            &settings.transfer_cleanup_last_date,
            today,
        );

        if !cache_needed && !retention_needed && !transfer_needed {
            return;
        }

        if transfer_needed {
            let daily_marker = today.format("%Y-%m-%d").to_string();
            self.state.update(cx, |state, _cx| {
                state.queue_daily_transfer_cleanup();
                state.settings.transfer_cleanup_last_date = daily_marker;
                state.settings.save();
            });
        }

        if !cache_needed && !retention_needed {
            return;
        }

        // Build CleanupOptions based on what's due.
        let stale_enabled = settings.cleanup_stale_items && cache_needed;
        let options = crate::core::cache_cleanup::CleanupOptions {
            clean_orphan_cache: cache_needed,
            clean_expired_tombstones: cache_needed,
            retention_days: if retention_needed && retention_days > 0 {
                Some(retention_days)
            } else {
                None
            },
            clean_stale_items: stale_enabled,
        };

        let db_path = settings.resolve_db_path();
        let sync_scope = crate::core::cache_cleanup::CleanupSyncScope {
            include_images: settings.sync_include_images,
            favorites_only: settings.sync_favorites_only,
            device_name: crate::services::backends::local_folder::hostname(),
        };
        let pending_result = self.pending_maintenance_result.clone();
        let cache_marker = cache_needed.then(|| cleanup_marker_for_interval(interval, today));
        let retention_marker = retention_needed.then(|| today.format("%Y-%m-%d").to_string());
        self.maintenance_job_running = true;

        std::thread::spawn(move || {
            let result = match crate::core::db::Database::open(&db_path.to_string_lossy()) {
                Ok(db) => {
                    let stats = crate::core::cache_cleanup::run_cleanup_with_options(
                        &db,
                        &options,
                        Some(&sync_scope),
                    );
                    DataMaintenanceResult::Cleanup {
                        stats,
                        cache_marker,
                        retention_marker,
                        show_toast: false,
                    }
                }
                Err(e) => {
                    log::error!("periodic cleanup: failed to open DB: {e}");
                    DataMaintenanceResult::Failed(e.to_string())
                }
            };
            if let Ok(mut slot) = pending_result.lock() {
                *slot = Some(result);
            }
        });
    }
}

#[cfg(test)]
mod geometry_persistence_tests {
    use super::geometry_retry_delay;
    use std::time::Duration;

    #[test]
    fn geometry_retry_delay_is_exponential_and_capped() {
        let delays: Vec<_> = (1..=7).map(geometry_retry_delay).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30].map(Duration::from_secs));
    }
}

#[cfg(test)]
mod cleanup_schedule_tests {
    use super::{
        cache_cleanup_due, cleanup_marker_for_interval, retention_cleanup_due, transfer_cleanup_due,
    };
    use chrono::NaiveDate;

    #[test]
    fn weekly_cache_marker_does_not_drive_daily_retention_marker() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let weekly_marker = cleanup_marker_for_interval("weekly", today);

        assert_eq!(weekly_marker, "2026-W30");
        assert!(!cache_cleanup_due("weekly", &weekly_marker, today));
        assert!(retention_cleanup_due(7, &weekly_marker, today));
        assert!(!retention_cleanup_due(7, "2026-07-20", today));
    }

    #[test]
    fn never_cache_interval_still_allows_retention_cleanup() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

        assert!(!cache_cleanup_due("never", "", today));
        assert!(retention_cleanup_due(1, "", today));
    }

    #[test]
    fn transfer_cleanup_runs_daily_when_the_backend_is_available() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

        assert!(transfer_cleanup_due(true, 3, "", today));
        assert!(!transfer_cleanup_due(true, 3, "2026-07-20", today));
        assert!(!transfer_cleanup_due(false, 3, "", today));
        assert!(!transfer_cleanup_due(true, 0, "", today));
    }
}

#[cfg(test)]
mod cleanup_toast_tests {
    use super::WindowManager;
    use crate::core::i18n_keys::I18nKey;

    #[test]
    fn cleanup_toast_prefers_retry_over_empty_or_done() {
        // A failed scan with no deletions must not report "nothing to clean"
        // or "cleanup complete".
        assert_eq!(
            WindowManager::cleanup_toast_message(true, true),
            I18nKey::ToastCleanupFailed.text()
        );
        assert_eq!(
            WindowManager::cleanup_toast_message(false, true),
            I18nKey::ToastCleanupFailed.text()
        );
    }

    #[test]
    fn cleanup_toast_distinguishes_empty_from_done() {
        assert_eq!(
            WindowManager::cleanup_toast_message(true, false),
            I18nKey::ToastCleanupNone.text()
        );
        assert_eq!(
            WindowManager::cleanup_toast_message(false, false),
            I18nKey::ToastCleanupDone.text()
        );
    }
}

#[cfg(test)]
mod exit_tests {
    #[test]
    fn tray_quit_requests_platform_termination() {
        let source = include_str!("window_manager.rs");
        let start = source.find("fn do_quit").unwrap();
        let end = source[start..].find("fn do_restart").unwrap() + start;
        let body = &source[start..end];

        assert!(body.contains("cx.quit();"));
        assert!(!body.contains("cx.shutdown();"));
    }
}

#[cfg(test)]
mod visibility_guard_tests {
    use super::visibility_guard_should_hide;

    /// Mirror the actual main-window call form (`poll_visibility_guard`,
    /// 04-spec v2 G2): `hwnd_valid = main_hwnd != 0`.
    fn main_should_hide(state_visible: bool, hwnd_valid: bool, hwnd_visible: bool) -> bool {
        visibility_guard_should_hide(state_visible, hwnd_valid, hwnd_visible)
    }

    /// Mirror the actual quick-window call form (`poll_visibility_guard`,
    /// 04-spec v2 G2): `hwnd_valid = quick_hwnd != 0`.
    fn quick_should_hide(state_visible: bool, hwnd_valid: bool, hwnd_visible: bool) -> bool {
        visibility_guard_should_hide(state_visible, hwnd_valid, hwnd_visible)
    }

    // AC2 combination table (04-spec v2 §3): {state-bit, hwnd-valid,
    // HWND-visible} -> should-hide. The main and quick windows share the
    // same pure decision, so each row asserts both invocation forms.

    #[test]
    fn hidden_state_with_invisible_hwnd_does_not_hide_main() {
        // {隐藏, 有效, 不可见} -> 不动作
        assert!(!main_should_hide(false, true, false));
    }

    #[test]
    fn hidden_state_with_invisible_hwnd_does_not_hide_quick() {
        // {隐藏, 有效, 不可见} -> 不动作
        assert!(!quick_should_hide(false, true, false));
    }

    #[test]
    fn hidden_state_with_visible_hwnd_hides_main() {
        // {隐藏, 有效, 可见} -> 应隐藏（核心：GPUI 越权调起场景）
        assert!(main_should_hide(false, true, true));
    }

    #[test]
    fn hidden_state_with_visible_hwnd_hides_quick() {
        // {隐藏, 有效, 可见} -> 应隐藏
        assert!(quick_should_hide(false, true, true));
    }

    #[test]
    fn visible_state_with_visible_hwnd_does_not_hide_main() {
        // {可见, 有效, 可见} -> 不动作（主动唤出，守卫不误伤）
        assert!(!main_should_hide(true, true, true));
    }

    #[test]
    fn visible_state_with_visible_hwnd_does_not_hide_quick() {
        // {可见, 有效, 可见} -> 不动作（主动唤出，守卫不误伤）
        assert!(!quick_should_hide(true, true, true));
    }

    #[test]
    fn visible_state_with_invisible_hwnd_does_not_hide_main() {
        // {可见, 有效, 不可见} -> 不动作
        assert!(!main_should_hide(true, true, false));
    }

    #[test]
    fn visible_state_with_invisible_hwnd_does_not_hide_quick() {
        // {可见, 有效, 不可见} -> 不动作
        assert!(!quick_should_hide(true, true, false));
    }

    #[test]
    fn invalid_hwnd_never_hides_main_regardless_of_hwnd_visibility() {
        // {隐藏, hwnd=0(无效), HWND 任意} -> 不动作（hwnd_valid=false 时
        // hwnd_visible 无论何值均不动作，安全跳过）。
        assert!(!main_should_hide(false, false, false));
        assert!(!main_should_hide(false, false, true));
    }

    #[test]
    fn invalid_hwnd_never_hides_quick_regardless_of_hwnd_visibility() {
        // {隐藏, hwnd=0(无效), HWND 任意} -> 不动作。
        assert!(!quick_should_hide(false, false, false));
        assert!(!quick_should_hide(false, false, true));
    }

    #[test]
    fn visible_state_with_invalid_hwnd_does_not_hide() {
        // 状态位为可见 + hwnd 无效：任何 HWND 可见性均不动作。
        assert!(!main_should_hide(true, false, false));
        assert!(!main_should_hide(true, false, true));
        assert!(!quick_should_hide(true, false, false));
        assert!(!quick_should_hide(true, false, true));
    }
}
