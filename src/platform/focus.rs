//! Focus event listener module
//! Uses Win32 SetWinEventHook for event-driven focus monitoring
//! Uses NSWorkspace notification for macOS focus monitoring

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK, WINEVENTPROC};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, PeekMessageW, PostThreadMessageW, TranslateMessage,
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
    MSG, PM_REMOVE, WM_QUIT,
};

#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
#[cfg(target_os = "macos")]
use std::sync::Arc;

/// Track the previous foreground window (before Clippi took focus)
#[cfg(target_os = "windows")]
static PREVIOUS_FOREGROUND_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Last non-Clippi foreground window (paste target)
#[cfg(target_os = "windows")]
static LAST_NON_CLIPPI_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Temporary storage for window swapping
#[cfg(target_os = "windows")]
static TEMP_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Track the previous foreground PID (before Clippi took focus)
#[cfg(target_os = "macos")]
static PREVIOUS_FOREGROUND_PID: AtomicI32 = AtomicI32::new(0);

/// Last non-Clippi foreground PID (paste target)
#[cfg(target_os = "macos")]
static LAST_NON_CLIPPI_PID: AtomicI32 = AtomicI32::new(0);

/// Temporary storage for PID swapping
#[cfg(target_os = "macos")]
static TEMP_PID: AtomicI32 = AtomicI32::new(0);

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
        let proc: WINEVENTPROC = Some(std::mem::transmute(win_event_proc as *const ()));
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            0 as *mut std::ffi::c_void,
            proc,
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };

    if hook.is_null() {
        return Err("SetWinEventHook failed".to_string());
    }

    let thread = std::thread::spawn(move || {
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

    Ok(FocusWatcher { hook, thread: Some(thread), thread_id: 0 })
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
                    let prev = TEMP_PID.load(Ordering::SeqCst);
                    LAST_NON_CLIPPI_PID.store(prev, Ordering::SeqCst);
                } else {
                    TEMP_PID.store(PREVIOUS_FOREGROUND_PID.load(Ordering::SeqCst), Ordering::SeqCst);
                    PREVIOUS_FOREGROUND_PID.store(pid, Ordering::SeqCst);
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

/// Get the paste target PID
#[cfg(target_os = "macos")]
pub fn get_last_non_clippi_pid() -> Option<i32> {
    let pid = LAST_NON_CLIPPI_PID.load(Ordering::SeqCst);
    if pid == 0 { None } else { Some(pid) }
}

#[cfg(target_os = "windows")]
fn is_clippi_window(hwnd: HWND) -> bool {
    let mut buffer: [u16; 256] = [0; 256];
    let len = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, buffer.as_mut_ptr(), 256) };
    if len > 0 {
        let title = String::from_utf16_lossy(&buffer[..len as usize]);
        title == "Clippi"
    } else {
        false
    }
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
        let prev = TEMP_WINDOW.load(Ordering::SeqCst);
        LAST_NON_CLIPPI_WINDOW.store(prev, Ordering::SeqCst);
        return;
    }

    // Not Clippi - save current PREV to TEMP, then update PREV to current
    TEMP_WINDOW.store(PREVIOUS_FOREGROUND_WINDOW.load(Ordering::SeqCst), Ordering::SeqCst);
    PREVIOUS_FOREGROUND_WINDOW.store(current_fg as usize, Ordering::SeqCst);
    LAST_NON_CLIPPI_WINDOW.store(current_fg as usize, Ordering::SeqCst);
}
