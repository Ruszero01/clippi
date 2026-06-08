//! Window manager — unified window state, positioning, and poll loop.
//!
//! Owns the window lifecycle: show/hide, activate, position calculation,
//! auto-hide on focus loss, and hotkey-triggered show. Replaces the
//! Slint-era `Frontend` + `FocusService` + `HotkeyService` + `Looper` combo.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::*;

use crate::core::frontend::{
    clamp_to_work_area, PositionMode, DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH, PANEL_OFFSET_X,
    SUPPRESS_DURATION_MS,
};
use crate::core::settings::AppSettings;
use crate::platform::focus::{start_focus_watcher, FocusWatcher};
use crate::platform::hotkey::{create_hotkey_listener, HotkeyListener};
use crate::platform::monitor;
use crate::platform::tray::{TrayAction, TrayManager};
use crate::services::gpui_clipboard::GpuiClipboardService;
use crate::services::update;
use crate::state::app::AppState;

/// Shared foreground app name for cross-service coordination.
pub type ForegroundAppName = Arc<Mutex<String>>;

/// GitHub releases page for the Clippi project.
const RELEASES_URL: &str = "https://github.com/Ruszero01/clippi/releases";

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
}

/// Unified window manager entity.
///
/// Owns the window lifecycle and all cross-service polling. Created once
/// in `main.rs` and stored as an `Entity<WindowManager>`. RootView
/// subscribes to clipboard and pin events.
pub struct WindowManager {
    // ── Window state ──
    position_mode: PositionMode,
    pinned: bool,
    auto_hide: bool,
    visible: bool,
    suppress_until: Option<Instant>,

    // ── Platform resources ──
    hotkey: Option<Box<dyn HotkeyListener>>,
    focus_watcher: Option<FocusWatcher>,
    foreground_app_name: ForegroundAppName,

    // ── Geometry cache (physical pixels on Windows, logical on macOS) ──
    saved_x: i32,
    saved_y: i32,
    saved_w: f32,
    saved_h: f32,

    // ── Hotkey blacklist ──
    blacklist: Vec<String>,

    // ── Dependencies ──
    state: Entity<AppState>,
    clipboard_service: GpuiClipboardService,

    // ── Raw window handle (HWND on Windows) ──
    #[allow(dead_code)]
    hwnd: isize,
    window_handle: Option<AnyWindowHandle>,

    // ── 系统托盘 ──
    tray: Option<TrayManager>,

    // ── Poll task ──
    _poll_task: Option<Task<()>>,
}

impl EventEmitter<WindowManagerEvent> for WindowManager {}

impl WindowManager {
    /// Create the window manager and start all background services.
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let settings = state.read(cx).settings.clone();

        // ── Initialize hotkey listener ──
        let hotkey = match create_hotkey_listener(&settings.hotkey) {
            Ok(hk) => Some(hk),
            Err(e) => {
                log::error!("Failed to create hotkey listener: {e}");
                None
            }
        };

        // ── Initialize focus watcher ──
        let focus_watcher = match start_focus_watcher() {
            Ok(fw) => Some(fw),
            Err(e) => {
                log::error!("Failed to start focus watcher: {e}");
                None
            }
        };

        let foreground_app_name = Arc::new(Mutex::new(String::new()));

        let clipboard_service = GpuiClipboardService::new();

        // ── Initialize tray ──
        let tray = Some(TrayManager::new());

        let mut wm = Self {
            position_mode: PositionMode::from_str(&settings.window_position_mode),
            pinned: false,
            auto_hide: settings.auto_hide,
            visible: true,
            suppress_until: Some(Instant::now() + Duration::from_millis(SUPPRESS_DURATION_MS)),
            hotkey,
            focus_watcher,
            foreground_app_name,
            saved_x: settings.saved_window_x,
            saved_y: settings.saved_window_y,
            saved_w: settings.saved_window_width,
            saved_h: settings.saved_window_height,
            blacklist: settings.hotkey_blacklist.clone(),
            state,
            clipboard_service,
            tray,
            hwnd: 0,
            window_handle: None,
            _poll_task: None,
        };

        // Share the batch_pasting flag with AppState so it can suppress
        // clipboard recording during batch paste operations.
        let batch_pasting = wm.clipboard_service.batch_pasting();
        // Share the skip_next flag — used for one-shot internal clipboard
        // writes (OCR paste, re-copy) that should be consumed by the
        // listener without creating a new history entry.
        let skip_next = wm.clipboard_service.skip_next();
        wm.state.update(cx, |s, _cx| {
            s.batch_pasting = batch_pasting;
            s.skip_next = skip_next;
        });

        // Start the unified poll loop
        wm.start_poll_loop(cx);

        wm
    }

    // ── Poll loop ──────────────────────────────────────────────────────

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

    fn poll(&mut self, cx: &mut Context<Self>) {
        // 1. Hotkey press -> show window
        self.poll_hotkey(cx);

        // 2. Hotkey recording — check for completion
        self.poll_recording(cx);

        // 3. Hotkey blacklist — dynamic register/unregister
        self.poll_blacklist();

        // 4. Clipboard changes -> update state + notify
        self.poll_clipboard(cx);

        // 5. Focus / auto-hide logic (also updates foreground app info in AppState)
        self.poll_focus(cx);

        // 6. Tray events
        self.poll_tray(cx);

        // 7. Capture window geometry for persistence
        self.capture_window_geometry(cx);
    }

    fn poll_hotkey(&mut self, cx: &mut Context<Self>) {
        if let Some(ref hk) = self.hotkey {
            if hk.poll_pressed() {
                self.show_and_focus(cx);
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
            // poll_recording_pressed() returns None when not recording —
            // it checks the hotkey's internal is_recording flag directly,
            // avoiding any AppState synchronization gap.
            if let Some(new_hotkey) = hk.poll_recording_pressed() {
                if !new_hotkey.is_empty() {
                    match hk.update_hotkey(&new_hotkey) {
                        Ok(()) => {
                            // update_hotkey already registered the new hotkey.
                            hk.finish_recording();
                            self.state.update(cx, |state, _cx| {
                                state.settings.hotkey = new_hotkey.clone();
                                state.settings.save();
                                state.hotkey_recording = false;
                            });
                            cx.emit(WindowManagerEvent::HotkeyRecordingComplete);
                        }
                        Err(e) => {
                            // update_hotkey failed — the hotkey is still the
                            // old one and unregistered. Re-register it and
                            // show the error.
                            hk.finish_recording();
                            hk.register();
                            self.state.update(cx, |state, _cx| {
                                state.hotkey_recording = false;
                                state.toast_message = Some(e);
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

    fn poll_focus(&mut self, cx: &mut Context<Self>) {
        // Update foreground app name for blacklist
        self.update_foreground_app_name(cx);

        let is_self_fg = self.is_self_foreground();

        // ── Auto-hide logic ──
        // Guard conditions: any true → skip auto-hide
        if !self.auto_hide || self.pinned || !self.visible || self.is_suppressed() || is_self_fg {
            return;
        }

        self.hide(cx);
    }

    // ── Tray event polling ───────────────────────────────────────────

    fn poll_tray(&mut self, cx: &mut Context<Self>) {
        let action = match self.tray.as_ref() {
            Some(t) => t.poll(),
            None => return,
        };

        match action {
            Some(TrayAction::Show) => {
                self.show_and_focus(cx);
            }
            Some(TrayAction::OpenSettings) => {
                cx.emit(WindowManagerEvent::OpenSettings);
            }
            Some(TrayAction::Restart) => {
                self.do_restart(cx);
            }
            Some(TrayAction::Quit) => {
                self.do_quit(cx);
            }
            Some(TrayAction::CheckUpdate) => {
                // Spawn on a background thread — ShellExecuteW can pump
                // Windows messages internally (DDE/COM) and deadlock if
                // called from the GPUI main thread event handler.
                std::thread::spawn(|| {
                    update::open_releases_page(RELEASES_URL);
                });
            }
            None => {}
        }
    }

    /// Capture the current window size from the platform and update
    /// saved_w / saved_h. Called from the poll loop while the window
    /// is visible. On Windows this uses GetWindowRect; on other platforms
    /// it is a no-op (window size is not yet tracked).
    fn capture_window_geometry(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Foundation::RECT;
            use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

            if self.hwnd == 0 {
                return;
            }
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let ok = unsafe { GetWindowRect(self.hwnd as *mut std::ffi::c_void, &mut rect) };
            if ok == 0 {
                return;
            }
            let phys_w = rect.right - rect.left;
            let phys_h = rect.bottom - rect.top;
            if phys_w <= 0 || phys_h <= 0 {
                return;
            }

            // Convert physical → logical pixels
            let scale = crate::platform::monitor::get_scale_factor(rect.left, rect.top);
            let logical_w = phys_w as f32 / scale;
            let logical_h = phys_h as f32 / scale;

            // Also capture the position (in physical pixels — save_geometry expects logical)
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

                // Persist to settings
                self.state.update(cx, |state, _cx| {
                    let settings = &mut state.settings;
                    if self.saved_x >= 0 && self.saved_y >= 0 {
                        settings.saved_window_x = self.saved_x;
                        settings.saved_window_y = self.saved_y;
                    }
                    if self.saved_w > 0.0 && self.saved_h > 0.0 {
                        settings.saved_window_width = self.saved_w;
                        settings.saved_window_height = self.saved_h;
                    }
                    settings.save();
                });
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = cx;
        }
    }

    /// Prepare for graceful shutdown: save geometry, flush WAL,
    /// release platform resources.
    fn prepare_shutdown(&mut self, cx: &mut Context<Self>) {
        let (sx, sy, sw, sh) = (self.saved_x, self.saved_y, self.saved_w, self.saved_h);
        self.state.update(cx, |state, _cx| {
            let _ = state.db.checkpoint();
            let settings = &mut state.settings;
            if sx >= 0 && sy >= 0 {
                settings.saved_window_x = sx;
                settings.saved_window_y = sy;
            }
            if sw > 0.0 && sh > 0.0 {
                settings.saved_window_width = sw;
                settings.saved_window_height = sh;
            }
            settings.save();
        });
        self.shutdown();
    }

    /// Fully quit the application.
    fn do_quit(&mut self, cx: &mut Context<Self>) {
        self.prepare_shutdown(cx);
        cx.shutdown();
    }

    /// Restart the application: flush, spawn new process, then quit.
    fn do_restart(&mut self, cx: &mut Context<Self>) {
        self.prepare_shutdown(cx);
        crate::core::settings::spawn_new_process();
        cx.shutdown();
    }

    // ── Foreground detection ──────────────────────────────────────────

    /// Check if our own window is the current foreground window.
    /// Uses direct HWND comparison to avoid dependence on window title.
    fn is_self_foreground(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            self.hwnd != 0 && unsafe { GetForegroundWindow() } as isize == self.hwnd
        }
        #[cfg(not(target_os = "windows"))]
        {
            crate::platform::blacklist::is_clippi_foreground()
        }
    }

    // ── Foreground tracking ───────────────────────────────────────────

    fn update_foreground_app_name(&mut self, cx: &mut Context<Self>) {
        use crate::platform::focus::get_foreground_app_info;

        if self.is_self_foreground() {
            if let Ok(mut fg) = self.foreground_app_name.lock() {
                fg.clear();
            }
            // Keep the UI showing the last foreground app (don't clear AppState).
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

    // ── Position calculation ──────────────────────────────────────────

    fn calculate_position(&self) -> Option<(i32, i32)> {
        let (win_w, win_h) = self.effective_window_size();
        // Convert logical → physical pixels for Windows SetWindowPos.
        // monitor::get_cursor_pos() returns physical pixels; window dimensions
        // and sidebar offset are in logical pixels and must be scaled.
        let scale = monitor::get_scale_factor(0, 0);
        let win_w_phys = (win_w * scale) as i32;
        let win_h_phys = (win_h * scale) as i32;
        let sidebar_offset = (PANEL_OFFSET_X * scale) as i32;

        match self.position_mode {
            PositionMode::Center => self.calc_center(win_w_phys, win_h_phys),
            PositionMode::FollowMouse => {
                self.calc_follow_mouse(win_w_phys, win_h_phys, sidebar_offset)
            }
            PositionMode::Remember => self
                .calc_remember(win_w_phys, win_h_phys)
                .or_else(|| self.calc_center(win_w_phys, win_h_phys)),
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
        // Offset by sidebar width so the main panel aligns with the cursor
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
        if sx < 0 || sy < 0 {
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

    // ── Window operations (platform-specific) ────────────────────────

    /// Show the window, calculate position, and bring it to foreground.
    ///
    /// When the window is already visible this only extends the suppress
    /// period and brings the window to foreground — it skips repositioning
    /// and item reload to avoid disrupting the current view.
    pub fn show_and_focus(&mut self, cx: &mut Context<Self>) {
        self.suppress_until = Some(Instant::now() + Duration::from_millis(SUPPRESS_DURATION_MS));

        let was_already_visible = self.visible;
        self.visible = true;

        if was_already_visible {
            // Window is already open (e.g. user is in settings recording a
            // hotkey). Just bring to foreground without repositioning.
            #[cfg(target_os = "windows")]
            {
                use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                let hwnd = self.hwnd as *mut std::ffi::c_void;
                if !hwnd.is_null() {
                    unsafe { SetForegroundWindow(hwnd) };
                }
            }
            cx.notify();
            return;
        }

        self.pinned = false;
        cx.emit(WindowManagerEvent::PinnedChanged(false));

        // ── Reload items from DB (they were cleared on hide) ──
        self.state.update(cx, |state, _cx| state.reload_items());
        cx.emit(WindowManagerEvent::ClipboardChanged);

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOACTIVATE,
                SWP_NOSIZE, SW_SHOW,
            };

            let hwnd = self.hwnd as *mut std::ffi::c_void;
            if !hwnd.is_null() {
                if let Some((x, y)) = self.calculate_position() {
                    unsafe {
                        SetWindowPos(hwnd, HWND_TOP, x, y, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
                    }
                }
                unsafe {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                };
            }
        }

        #[cfg(target_os = "macos")]
        {
            // On macOS, activate the app and let the window become visible
            let mtm = objc2::MainThreadMarker::new().unwrap();
            let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            ns_app.activateIgnoringOtherApps(true);
        }

        cx.notify();
    }

    /// Hide the window to background — does NOT exit the process.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.dismiss_ui(cx);

        // ── Release memory: clear items list (mirrors Slint release_model_resources) ──
        self.state.update(cx, |state, _cx| state.clear_items());
        cx.emit(WindowManagerEvent::ClipboardChanged);

        // ── Flush WAL and trim working set (mirrors Slint periodic maintenance) ──
        self.state.update(cx, |state, _cx| {
            let _ = state.db.checkpoint();
        });
        crate::platform::util::trim_process_working_set();

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

            let hwnd = self.hwnd as *mut std::ffi::c_void;

            // Save position in Remember mode
            if self.position_mode == PositionMode::Remember && !hwnd.is_null() {
                use windows_sys::Win32::Foundation::POINT;
                use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
                let mut pt = POINT { x: 0, y: 0 };
                unsafe {
                    ClientToScreen(hwnd, &mut pt);
                }
                self.saved_x = pt.x;
                self.saved_y = pt.y;
            }

            if !hwnd.is_null() {
                unsafe { ShowWindow(hwnd, SW_HIDE) };
            }
        }

        #[cfg(target_os = "macos")]
        {
            // macOS: hide is handled via NSApplication
        }

        self.visible = false;
    }

    /// Clear all floating UI state.
    fn dismiss_ui(&self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| {
            state.clear_selection();
        });
        // Note: context_menu, tag_picker etc. will be handled by RootView
        // observing WindowManager events and clearing its own state.
    }

    // ── Public setters ────────────────────────────────────────────────

    /// Store the raw window handle (HWND on Windows) for platform operations.
    #[cfg(target_os = "windows")]
    pub fn set_hwnd(&mut self, hwnd: isize) {
        self.hwnd = hwnd;
        crate::platform::focus::set_clippi_hwnd(hwnd);
    }

    #[cfg(not(target_os = "windows"))]
    pub fn set_hwnd(&mut self, _hwnd: isize) {}

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        cx.emit(WindowManagerEvent::PinnedChanged(pinned));
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn set_auto_hide(&mut self, auto_hide: bool) {
        self.auto_hide = auto_hide;
    }

    pub fn set_position_mode(&mut self, mode: PositionMode) {
        self.position_mode = mode;
    }

    pub fn set_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
        if let Some(ref mut hk) = self.hotkey {
            hk.update_hotkey(hotkey_str)
        } else {
            Err("No hotkey listener".to_string())
        }
    }

    pub fn start_hotkey_recording(&mut self) {
        if let Some(ref mut hk) = self.hotkey {
            // Unregister the current hotkey before recording so the old
            // hotkey doesn't fire poll_pressed() during the recording
            // session (which would trigger show_and_focus / reposition).
            hk.unregister();
            hk.start_recording();
        }
    }

    /// Get a clone of the current foreground app name.
    pub fn foreground_app_name_clone(&self) -> String {
        self.foreground_app_name
            .lock()
            .ok()
            .map(|fg| fg.clone())
            .unwrap_or_default()
    }

    // ── Blacklist management ──────────────────────────────────────────

    /// Replace the internal blacklist with the given list (used for sync from settings).
    pub fn set_blacklist(&mut self, blacklist: Vec<String>) {
        self.blacklist = blacklist;
    }

    pub fn blacklist_add(&mut self, app_name: &str, settings: &mut AppSettings) {
        if app_name.is_empty() || self.blacklist.contains(&app_name.to_string()) {
            return;
        }
        self.blacklist.push(app_name.to_string());
        settings.hotkey_blacklist = self.blacklist.clone();
        settings.save();
    }

    pub fn blacklist_remove(&mut self, app_name: &str, settings: &mut AppSettings) {
        self.blacklist.retain(|a| a != app_name);
        settings.hotkey_blacklist = self.blacklist.clone();
        settings.save();
    }

    /// Persist current geometry to settings.
    pub fn save_geometry(&self, settings: &mut AppSettings) {
        if self.saved_x >= 0 && self.saved_y >= 0 {
            settings.saved_window_x = self.saved_x;
            settings.saved_window_y = self.saved_y;
        }
        if self.saved_w > 0.0 && self.saved_h > 0.0 {
            settings.saved_window_width = self.saved_w;
            settings.saved_window_height = self.saved_h;
        }
    }

    /// Release platform resources on shutdown.
    pub fn shutdown(&mut self) {
        if let Some(ref mut hk) = self.hotkey {
            hk.stop();
        }
        if let Some(ref mut fw) = self.focus_watcher {
            fw.stop();
        }
    }
}
