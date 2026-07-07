//! Window manager — unified window state, positioning, and poll loop.
//!
//! --- Owns the window lifecycle: show/hide, activate, position calculation, ---
//! --- auto-hide on focus loss, and hotkey-triggered show. Replaces the ---
//! Slint-era `Frontend` + `FocusService` + `HotkeyService` + `Looper` combo.

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
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
use crate::platform::hotkey::{create_hotkey_listener, HotkeyEvent, HotkeyListener, QuickAction};
use crate::platform::monitor;
use crate::platform::tray::{TrayAction, TrayManager};
use crate::services::gpui_clipboard::GpuiClipboardService;
use crate::services::gpui_sync::GpuiSyncService;
use crate::services::update;
use crate::state::app::AppState;
#[cfg(target_os = "macos")]
use crate::ui::quick_paste::QUICK_WINDOW_CORNER_RADIUS;
use crate::ui::quick_paste::{
    calc_quick_window_height, QuickPasteEvent, QuickPasteView, QUICK_WINDOW_WIDTH,
};

/// Shared foreground app name for cross-service coordination.
pub type ForegroundAppName = Arc<Mutex<String>>;

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
    /// Paste shortcut recording completed for an app.
    PasteShortcutRecorded {
        app_name: String,
        shortcut: String,
    },
    /// Window was hidden — RootView should dismiss all floating panels.
    WindowHidden,
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
    /// Reset RootView to clipboard history page (when always_reset_to_clipboard is on).
    ResetToClipboard,
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
    focus_watcher: Option<FocusWatcher>,
    foreground_app_name: ForegroundAppName,

    // --- Geometry cache (physical pixels on Windows, logical on macOS) ---
    saved_x: i32,
    saved_y: i32,
    saved_w: f32,
    saved_h: f32,

    // --- Hotkey blacklist ---
    blacklist: Vec<String>,

    /// When Some(app_name), the current recording is for a paste shortcut (not global hotkey).
    pub recording_paste_shortcut_app: Option<String>,

    // --- Dependencies ---
    state: Entity<AppState>,
    clipboard_service: GpuiClipboardService,
    sync_service: GpuiSyncService,

    // --- Raw window handle (HWND on Windows) ---
    #[allow(dead_code)]
    hwnd: isize,
    #[cfg(target_os = "windows")]
    last_system_dpi: u32,
    #[cfg(target_os = "macos")]
    ns_window: isize,
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
    /// Last update check time for 24h periodic check throttling.
    last_update_check: Option<Instant>,
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

        let clipboard_service = GpuiClipboardService::new();
        let sync_service = GpuiSyncService::new(&settings, state.read(cx).sync_dirty.clone());

        // --- Initialize tray ---
        let tray = Some(TrayManager::new());

        let mut wm = Self {
            position_mode: PositionMode::from_str(&settings.window_position_mode),
            tray_triggered: false,
            pinned: false,
            auto_hide: settings.auto_hide,
            // When silent_start is true the window is created hidden
            // (WindowOptions { show: false }), so `visible` starts false.
            visible: !settings.silent_start,
            suppress_until: Some(Instant::now() + Duration::from_millis(SUPPRESS_DURATION_MS)),
            hotkey: None,
            focus_watcher,
            foreground_app_name,
            saved_x: settings.saved_window_x,
            saved_y: settings.saved_window_y,
            saved_w: settings.saved_window_width,
            saved_h: settings.saved_window_height,
            blacklist: settings.hotkey_blacklist.clone(),
            recording_paste_shortcut_app: None,
            state,
            clipboard_service,
            sync_service,
            tray,
            hwnd: 0,
            #[cfg(target_os = "windows")]
            last_system_dpi: 0,
            #[cfg(target_os = "macos")]
            ns_window: 0,
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
            last_update_check: None,
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

        // --- 4. Clipboard changes -> update state + notify ---
        self.poll_clipboard(cx);

        self.poll_bitmap_paste(cx);

        // 5. Focus / auto-hide logic (also updates foreground app info in AppState)
        self.poll_focus(cx);

        // --- 6. Tray events ---
        self.poll_tray(cx);

        // 7. Capture window geometry for persistence
        self.capture_window_geometry(cx);

        // --- 8. Cloud sync ---
        self.poll_sync(cx);

        // --- 9. Auto-update ---
        self.poll_update(cx);

        // --- 10. Periodic cache cleanup ---
        self.poll_cleanup(cx);
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
            if let Some(new_hotkey) = hk.poll_recording_pressed() {
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
                        });
                    }
                    cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
                    return;
                }

                if !new_hotkey.is_empty() {
                    match hk.update_hotkey(&new_hotkey) {
                        Ok(()) => {
                            // --- update_hotkey already registered the new hotkey. ---
                            hk.finish_recording();
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

        if !fg_name.is_empty() && self.blacklist.contains(&fg_name) {
            if let Some(ref mut hk) = self.hotkey {
                hk.unregister();
            }
        } else if let Some(ref mut hk) = self.hotkey {
            hk.register();
        }
    }

    fn poll_clipboard(&mut self, cx: &mut Context<Self>) {
        let changed = self
            .state
            .update(cx, |state, _cx| self.clipboard_service.poll_state(state));
        if changed {
            cx.emit(WindowManagerEvent::ClipboardChanged);
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
                let ctrl_held = flags.contains(objc2_app_kit::NSEventModifierFlags::Control);
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

                self.persist_geometry(cx);
            }
        }

        #[cfg(target_os = "macos")]
        {
            if self.ns_window == 0 {
                return;
            }
            let Some(mtm) = objc2::MainThreadMarker::new() else {
                return;
            };
            let Some(main_screen) = objc2_app_kit::NSScreen::mainScreen(mtm) else {
                return;
            };
            let frame = unsafe { (&*(self.ns_window as *const objc2_app_kit::NSWindow)).frame() };
            let rect = monitor::cocoa_rect_to_top_left(
                main_screen.frame().size.height,
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
                self.persist_geometry(cx);
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            let _ = cx;
        }
    }

    /// Prepare for graceful shutdown: save geometry, flush WAL,
    /// release platform resources.
    fn prepare_shutdown(&mut self, cx: &mut Context<Self>) {
        self.capture_window_geometry(cx);
        let (sx, sy, sw, sh) = (self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.state.update(cx, |state, _cx| {
            if let Err(e) = state.db.checkpoint() {
                log::error!("WAL checkpoint failed (save geometry): {e}");
            }
            let settings = &mut state.settings;
            if sw > 0.0 && sh > 0.0 {
                settings.saved_window_x = sx;
                settings.saved_window_y = sy;
            }
            if sw > 0.0 && sh > 0.0 {
                settings.saved_window_width = sw;
                settings.saved_window_height = sh;
            }
            settings.save();
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
        crate::core::settings::spawn_new_process();
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

        if let Some(info) = get_foreground_app_info() {
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                *fg = info.app_name.clone();
            }
            // Push foreground app info to AppState for the settings UI.
            let app_name = info.app_name.clone();
            let window_title = info.window_title.clone();
            let icon_base64 = info.icon_base64.clone();
            self.state.update(cx, |state, _cx| {
                state.foreground_app_name = app_name;
                state.foreground_window_title = window_title;
                state.foreground_app_icon_base64 = icon_base64;
            });
        }
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
            if let Some((x, y)) = self.calculate_position() {
                self.position_macos_window(x, y);
            }
            cx.activate(true);
            self.activate_macos_window();
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
                    self.state.update(cx, |state, _cx| {
                        state.settings.hotkey = actual.clone();
                        state.settings.save();
                        state.toast_message =
                            Some(I18nKey::HotkeyFallbackToast.fmt(&[&hotkey_str, &actual]));
                        state.toast_is_warning = true;
                    });
                }
                if hk.quick_fallback_used() {
                    let actual = hk.actual_quick_hotkey().to_string();
                    self.state.update(cx, |state, _cx| {
                        state.settings.quick_hotkey = actual.clone();
                        state.settings.save();
                        state.toast_message =
                            Some(I18nKey::HotkeyFallbackToast.fmt(&[&quick_hotkey_str, &actual]));
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
    }

    /// Release memory without changing window visibility.
    ///
    /// Used when the window starts hidden (silent_start via
    /// `WindowOptions { show: false }`) — drops the in-memory items list,
    /// checkpoints the WAL, and trims the process working set.  This is
    /// the same cleanup that `hide()` does, but without the platform
    /// show/hide call.
    pub fn release_memory(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.clear_items());
        cx.emit(WindowManagerEvent::ClipboardChanged);
        self.state.update(cx, |state, _cx| {
            if let Err(e) = state.db.checkpoint() {
                log::error!("WAL checkpoint failed (clipboard changed): {e}");
            }
        });
        crate::platform::util::trim_process_working_set();
    }

    /// Hide the window to background — does NOT exit the process.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        {
            self._main_show_task = None;
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
            self.capture_window_geometry(cx);
            self.hide_macos_window();
        }

        self.visible = false;
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

        // Compute dynamic window height based on visible bars
        let quick_h = {
            let state = self.state.read(cx);
            let pinned_tag_ids = &state.settings.pinned_tag_ids;
            let tags = &state.tags;
            let has_tag = pinned_tag_ids
                .iter()
                .any(|&id| tags.iter().any(|t| t.id == id));
            let has_type = !state.settings.type_filter_config.is_empty();
            calc_quick_window_height(has_tag, has_type)
        };

        // Positioning priority: Caret (Path A/B) → Cursor (Path D) → raw cursor
        // Never fall back to window-centered positioning — the window
        // must follow the text caret or the mouse, never the screen center.
        let (x, y) = self
            .calculate_quick_position(quick_h)
            .or_else(|| {
                // calculate_quick_position already tries cursor as Path D;
                // reaching here means even GetCursorPos failed (extremely rare).
                // Try one more time directly as a last resort.
                let (cx, cy) = monitor::get_cursor_pos().unwrap_or((0, 0));
                log::debug!(
                    "show_quick_window: positioning fallback, raw cursor=({},{})",
                    cx,
                    cy
                );
                Some((cx, cy))
            })
            .unwrap_or((0, 0));

        #[cfg(target_os = "macos")]
        {
            self.position_quick_macos_window(x, y, quick_h);
            self.show_quick_macos_window();
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

    fn hide_quick_window(&mut self, _cx: &mut Context<Self>) {
        self._quick_poll_task = None; // cancel fast poll
        #[cfg(target_os = "windows")]
        {
            self._quick_position_task = None;
        }
        self.quick_visible = false;
        self.quick_mouse_down = false;
        if let Some(ref mut hotkey) = self.hotkey {
            hotkey.set_quick_actions_enabled(false);
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
            self.hide_quick_macos_window();
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
                    let has_alt = view.has_alt_modes();
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
                    let has_alt = view.has_alt_modes();
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
                    let has_alt = view.has_alt_modes();
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
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let Some(main_screen) = objc2_app_kit::NSScreen::mainScreen(mtm) else {
            return;
        };
        let top = main_screen.frame().size.height - y as f64;
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
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let Some(main_screen) = objc2_app_kit::NSScreen::mainScreen(mtm) else {
            return;
        };
        let top = main_screen.frame().size.height - y as f64;
        unsafe {
            let window = &*(self.ns_window as *const objc2_app_kit::NSWindow);
            window.setFrameTopLeftPoint(objc2_foundation::NSPoint::new(x as f64, top));
        }
    }

    fn persist_geometry(&self, cx: &mut Context<Self>) {
        let (x, y, width, height) = (self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.state.update(cx, |state, _cx| {
            if width > 0.0 && height > 0.0 {
                state.settings.saved_window_x = x;
                state.settings.saved_window_y = y;
            }
            if width > 0.0 && height > 0.0 {
                state.settings.saved_window_width = width;
                state.settings.saved_window_height = height;
            }
            state.settings.save();
        });
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
                    self.state.update(cx, |state, _| {
                        state.update_phase = update::UpdatePhase::Error(error.clone())
                    });
                    cx.emit(WindowManagerEvent::UpdateProgress(
                        update::UpdatePhase::Error(error),
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

        // 4. Periodic check: every 24 hours
        let auto_check = self.state.read(cx).settings.auto_check_updates;
        if auto_check {
            let should_check = match self.last_update_check {
                None => true,
                Some(t) => t.elapsed().as_secs() >= 24 * 3600,
            };
            if should_check {
                self.last_update_check = Some(Instant::now());
                self.start_update_check(cx);
            }
        }
    }

    /// Start an update check (manual or periodic). Spawns a background thread.
    pub fn start_update_check(&mut self, cx: &mut Context<Self>) {
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
        self.last_update_check = Some(Instant::now());
        self.state
            .update(cx, |s, _| s.update_phase = update::UpdatePhase::Checking);
        cx.emit(WindowManagerEvent::UpdateProgress(
            update::UpdatePhase::Checking,
        ));

        let pending = self.pending_update.clone();
        let pending_phase = self.pending_update_phase.clone();
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
                    log::warn!("[wm] update check failed: {error}");
                    if let Ok(mut phase) = pending_phase.lock() {
                        *phase = update::UpdatePhase::Error(error);
                    }
                }
            }
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
                if let Ok(mut p) = pending_phase_err.lock() {
                    *p = update::UpdatePhase::Error(e);
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
        self.hide_quick_window(cx);
        if let Some(ref mut hk) = self.hotkey {
            hk.stop();
        }
        if let Some(ref mut fw) = self.focus_watcher {
            fw.stop();
        }
    }

    /// Check if periodic cache cleanup is due and spawn a background thread.
    fn poll_cleanup(&mut self, cx: &mut Context<Self>) {
        let settings = self.state.read(cx).settings.clone();
        let interval = settings.cleanup_interval.as_str();
        if interval == "never" {
            return;
        }

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let should_run = match interval {
            "daily" => settings.cleanup_last_date != today,
            "weekly" => {
                // ISO week-based comparison
                let current_week = chrono::Local::now().iso_week();
                let current_week_str =
                    format!("{}-W{:02}", current_week.year(), current_week.week());
                settings.cleanup_last_date != current_week_str
            }
            _ => false,
        };

        if !should_run {
            return;
        }

        let db_path = settings.resolve_db_path();
        std::thread::spawn(move || {
            match crate::core::db::Database::open(&db_path.to_string_lossy()) {
                Ok(db) => {
                    let stats = crate::core::cache_cleanup::run_cleanup(&db);
                    if !stats.is_empty() {
                        log::info!(
                            "periodic cleanup: {} orphan images, {} unreferenced icons, {} expired tombstones",
                            stats.orphan_images,
                            stats.unreferenced_icons,
                            stats.expired_tombstones,
                        );
                    }
                }
                Err(e) => log::error!("periodic cleanup: failed to open DB: {e}"),
            }
        });

        // Update last cleanup date immediately (don't wait for thread).
        let new_date = match interval {
            "daily" => today,
            "weekly" => {
                let wk = chrono::Local::now().iso_week();
                format!("{}-W{:02}", wk.year(), wk.week())
            }
            _ => return,
        };
        self.state.update(cx, |s, _cx| {
            s.settings.cleanup_last_date = new_date;
            s.settings.save();
        });
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
