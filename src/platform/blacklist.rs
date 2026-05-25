//! Blacklist module — detects whether Clippi is the foreground application.
//!
//! Used by FocusService to decide when to auto-hide the window.

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
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(fg, buffer.as_mut_ptr(), 256)
    };
    if len > 0 {
        let fg_title = String::from_utf16_lossy(&buffer[..len as usize]);
        return fg_title == "Clippi";
    }
    false
}

#[cfg(target_os = "macos")]
pub fn is_clippi_foreground() -> bool {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        return app.processIdentifier() == std::process::id() as i32;
    }
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_clippi_foreground() -> bool {
    true
}
