//! Paste simulation - simulates Ctrl+V to paste content and restore focus

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, VK_CONTROL, VK_V};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

#[cfg(target_os = "windows")]
const SLEEP_MS: u64 = 50;

/// Store the previous window handle when showing clippi
#[cfg(target_os = "windows")]
static PREVIOUS_WINDOW: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Get the currently foreground window handle
#[cfg(target_os = "windows")]
fn get_foreground_window() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        None
    } else {
        Some(hwnd)
    }
}

/// Record the previous window (call before showing clippi)
#[cfg(target_os = "windows")]
pub fn record_previous_window() {
    if let Some(hwnd) = get_foreground_window() {
        PREVIOUS_WINDOW.store(hwnd as usize, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Restore focus to the previous window
#[cfg(target_os = "windows")]
pub fn restore_previous_focus() {
    let ptr = PREVIOUS_WINDOW.load(std::sync::atomic::Ordering::SeqCst);
    if ptr != 0 {
        unsafe { SetForegroundWindow(ptr as HWND) };
    }
}

/// Simulate Ctrl+V using keybd_event after a short delay
#[cfg(target_os = "windows")]
pub fn paste_after_delay() {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(SLEEP_MS));
        unsafe {
            let vk_ctrl = VK_CONTROL as u8;
            let vk_v = VK_V as u8;
            // Press Ctrl
            keybd_event(vk_ctrl, 0, 0, 0);
            // Press V
            keybd_event(vk_v, 0, 0, 0);
            // Release V
            keybd_event(vk_v, 0, 2, 0);
            // Release Ctrl
            keybd_event(vk_ctrl, 0, 2, 0);
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn record_previous_window() {}

#[cfg(not(target_os = "windows"))]
pub fn restore_previous_focus() {}

#[cfg(not(target_os = "windows"))]
pub fn paste_after_delay() {}
