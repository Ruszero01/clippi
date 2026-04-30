//! Blacklist module - checks if current foreground process is blacklisted
//!
//! This module is reserved for future blacklist functionality.
//! Currently not used by FocusService.

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// Check if Clippi window is currently in foreground (by window title)
#[cfg(target_os = "windows")]
pub fn is_clippi_foreground() -> bool {
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() {
        return false;
    }
    let mut buffer: [u16; 256] = [0; 256];
    let len = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(
            fg,
            buffer.as_mut_ptr(),
            256,
        )
    };
    if len > 0 {
        let fg_title = String::from_utf16_lossy(&buffer[..len as usize]);
        return fg_title == "Clippi";
    }
    false
}

#[cfg(not(target_os = "windows"))]
pub fn is_clippi_foreground() -> bool {
    true
}