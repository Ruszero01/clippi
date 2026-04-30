//! Paste simulation - simulates Ctrl+V to paste content and restore focus

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, VK_CONTROL, VK_V};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};

#[cfg(target_os = "windows")]
const SLEEP_MS: u64 = 50;

/// Restore focus to the last non-Clippi foreground window (paste target)
#[cfg(target_os = "windows")]
pub fn restore_paste_target() {
    if let Some(hwnd) = crate::platform::focus::get_last_non_clippi_window() {
        if unsafe { IsWindow(hwnd) } != 0 {
            unsafe { SetForegroundWindow(hwnd) };
        }
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
            keybd_event(vk_ctrl, 0, 0, 0);
            keybd_event(vk_v, 0, 0, 0);
            keybd_event(vk_v, 0, 2, 0);
            keybd_event(vk_ctrl, 0, 2, 0);
        }
    });
}

#[cfg(not(target_os = "windows"))]
pub fn restore_paste_target() {}

#[cfg(not(target_os = "windows"))]
pub fn paste_after_delay() {}
