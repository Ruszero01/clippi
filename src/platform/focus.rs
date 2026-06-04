//! Focus event listener module
//! Uses Win32 SetWinEventHook for event-driven focus monitoring
//! Uses NSWorkspace polling for macOS focus monitoring

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::CloseHandle;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Accessibility::{
    SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK, WINEVENTPROC,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DispatchMessageW, GetForegroundWindow, GetWindowTextW,
    GetWindowThreadProcessId, PeekMessageW, PostThreadMessageW, TranslateMessage,
    EVENT_SYSTEM_FOREGROUND, MSG, PM_REMOVE, WINEVENT_OUTOFCONTEXT, WM_QUIT,
};

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_os = "windows")]
use std::sync::Mutex;

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// Foreground application information used by the hotkey blacklist UI.
#[derive(Debug, Clone)]
pub struct ForegroundAppInfo {
    pub app_name: String,
    pub window_title: String,
    pub icon_base64: String,
}

/// Last non-Clippi foreground window (paste target)
#[cfg(target_os = "windows")]
static LAST_NON_CLIPPI_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Our own window handle (set at window creation).
/// Used by `is_clippi_window` to avoid depending on window title.
#[cfg(target_os = "windows")]
static CLIPPI_HWND: AtomicUsize = AtomicUsize::new(0);

/// Last foreground window title (raw UTF-16 buffer)
#[cfg(target_os = "windows")]
static LAST_FOREGROUND_TITLE: Mutex<[u16; 512]> = Mutex::new([0u16; 512]);

/// Last non-Clippi foreground PID (paste target)
#[cfg(target_os = "macos")]
static LAST_NON_CLIPPI_PID: AtomicI32 = AtomicI32::new(0);

/// FocusWatcher handle
pub struct FocusWatcher {
    #[cfg(target_os = "windows")]
    hook: HWINEVENTHOOK,
    #[cfg(target_os = "windows")]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(target_os = "windows")]
    thread_id: u32,
    #[cfg(target_os = "macos")]
    running: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FocusWatcher {
    #[cfg(target_os = "windows")]
    pub fn stop(&mut self) {
        unsafe { UnhookWinEvent(self.hook) };
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    #[cfg(target_os = "macos")]
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    pub fn stop(&mut self) {}
}

#[cfg(target_os = "windows")]
pub fn start_focus_watcher() -> Result<FocusWatcher, String> {
    let hook = unsafe {
        let proc: WINEVENTPROC = Some(std::mem::transmute::<
            *const (),
            unsafe extern "system" fn(
                *mut std::ffi::c_void,
                u32,
                *mut std::ffi::c_void,
                i32,
                i32,
                u32,
                u32,
            ),
        >(win_event_proc as *const ()));
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut::<std::ffi::c_void>(),
            proc,
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };

    if hook.is_null() {
        return Err("SetWinEventHook failed".to_string());
    }

    // Channel to retrieve the actual thread ID from inside the message pump thread
    let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(0);

    let thread = std::thread::spawn(move || {
        let tid = unsafe { GetCurrentThreadId() };
        let _ = tx.send(tid); // blocks until receiver reads — ensures tid is available
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            let ret = unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) };
            if ret == 0 {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            if msg.message == WM_QUIT {
                break;
            }
            unsafe { TranslateMessage(&msg) };
            unsafe { DispatchMessageW(&msg) };
        }
    });

    let thread_id = rx.recv().unwrap_or(0);

    Ok(FocusWatcher {
        hook,
        thread: Some(thread),
        thread_id,
    })
}

#[cfg(target_os = "macos")]
pub fn start_focus_watcher() -> Result<FocusWatcher, String> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    let my_pid = std::process::id() as i32;

    let thread = std::thread::spawn(move || {
        while running_clone.load(Ordering::SeqCst) {
            let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
            if let Some(app) = workspace.frontmostApplication() {
                let pid = app.processIdentifier();

                if pid == my_pid {
                    // LAST_NON_CLIPPI_PID already holds the correct paste target.
                } else {
                    LAST_NON_CLIPPI_PID.store(pid, Ordering::SeqCst);
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });

    Ok(FocusWatcher {
        running,
        thread: Some(thread),
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn start_focus_watcher() -> Result<FocusWatcher, String> {
    Ok(FocusWatcher {})
}

/// Get the paste target window handle
#[cfg(target_os = "windows")]
pub fn get_last_non_clippi_window() -> Option<HWND> {
    let ptr = LAST_NON_CLIPPI_WINDOW.load(Ordering::SeqCst);
    if ptr == 0 {
        None
    } else {
        Some(ptr as HWND)
    }
}

/// Register our window HWND so the focus watcher can identify us.
#[cfg(target_os = "windows")]
pub fn set_clippi_hwnd(hwnd: isize) {
    CLIPPI_HWND.store(hwnd as usize, Ordering::SeqCst);
}

/// Check if the given HWND is our Clippi window.
#[cfg(target_os = "windows")]
pub fn is_our_window(hwnd: isize) -> bool {
    let our = CLIPPI_HWND.load(Ordering::SeqCst);
    our != 0 && hwnd as usize == our
}

/// Get the paste target PID
#[cfg(target_os = "macos")]
pub fn get_last_non_clippi_pid() -> Option<i32> {
    let pid = LAST_NON_CLIPPI_PID.load(Ordering::SeqCst);
    if pid == 0 {
        None
    } else {
        Some(pid)
    }
}

#[cfg(target_os = "windows")]
fn is_clippi_window(hwnd: HWND) -> bool {
    let our_hwnd = CLIPPI_HWND.load(Ordering::SeqCst);
    our_hwnd != 0 && hwnd as usize == our_hwnd
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn win_event_proc(
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread_id: u32,
    _timestamp: u32,
) {
    let current_fg = GetForegroundWindow();
    if current_fg.is_null() {
        return;
    }

    let is_clippi_now = is_clippi_window(current_fg);

    if is_clippi_now {
        // LAST_NON_CLIPPI_WINDOW already holds the correct paste target from
        // the most recent non-Clippi focus event. Do not overwrite it.
    } else {
        LAST_NON_CLIPPI_WINDOW.store(current_fg as usize, Ordering::SeqCst);
        // Capture window title alongside HWND
        if let Ok(mut buf) = LAST_FOREGROUND_TITLE.lock() {
            let len = GetWindowTextW(current_fg, buf.as_mut_ptr(), 512);
            if len > 0 {
                // Fill remaining with zeros
                for i in len as usize..512 {
                    buf[i] = 0;
                }
            } else {
                buf[0] = 0;
            }
        }
    }
}

// ── Foreground app info extraction ──

/// Get information about the current foreground application.
/// Returns None on unsupported platforms or if the info is unavailable.
pub fn get_foreground_app_info() -> Option<ForegroundAppInfo> {
    #[cfg(target_os = "windows")]
    {
        windows_foreground_info()
    }
    #[cfg(target_os = "macos")]
    {
        macos_foreground_info()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn windows_foreground_info() -> Option<ForegroundAppInfo> {
    unsafe {
        // Use the stored non-Clippi window; fall back to current foreground
        let hwnd = get_last_non_clippi_window().or_else(|| {
            let fg = GetForegroundWindow();
            if fg.is_null() || is_clippi_window(fg) {
                None
            } else {
                Some(fg)
            }
        })?;

        // Window title from stored buffer
        let window_title = if let Ok(buf) = LAST_FOREGROUND_TITLE.lock() {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(512);
            String::from_utf16_lossy(&buf[..end])
        } else {
            String::new()
        };

        // Get PID
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return Some(ForegroundAppInfo {
                app_name: String::new(),
                window_title,
                icon_base64: String::new(),
            });
        }

        // Get exe path
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Some(ForegroundAppInfo {
                app_name: String::new(),
                window_title,
                icon_base64: String::new(),
            });
        }
        let mut exe_buf = [0u16; 260];
        let mut exe_len = exe_buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            0, // PROCESS_NAME_WIN32
            exe_buf.as_mut_ptr(),
            &mut exe_len,
        );
        CloseHandle(process);
        if result == 0 {
            return Some(ForegroundAppInfo {
                app_name: String::new(),
                window_title,
                icon_base64: String::new(),
            });
        }

        let exe_path = String::from_utf16_lossy(&exe_buf[..exe_len as usize]);
        let app_name = std::path::Path::new(&exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| {
                let mut chars = s.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .unwrap_or_default();

        // Extract icon
        let wide_path: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut shfi: SHFILEINFOW = std::mem::zeroed();
        let icon_result = SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        let icon_base64 = if icon_result != 0 && !shfi.hIcon.is_null() {
            let result = super::util::hicon_to_base64_png(shfi.hIcon, 32).unwrap_or_default();
            DestroyIcon(shfi.hIcon);
            result
        } else {
            String::new()
        };

        Some(ForegroundAppInfo {
            app_name,
            window_title,
            icon_base64,
        })
    }
}

#[cfg(target_os = "macos")]
fn macos_foreground_info() -> Option<ForegroundAppInfo> {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;

    if app.processIdentifier() == std::process::id() as i32 {
        // Clippi itself — no foreground info to show
        return None;
    }

    // Use generated methods (nil-safe via Option)
    let app_name = app
        .localizedName()
        .map(|n| n.to_string())
        .unwrap_or_default();
    let icon_base64 = app
        .icon()
        .and_then(|i| super::util::nsimage_to_base64_png(&i, 32))
        .unwrap_or_default();

    Some(ForegroundAppInfo {
        app_name,
        window_title: String::new(), // macOS window title extraction requires extra permissions
        icon_base64,
    })
}
