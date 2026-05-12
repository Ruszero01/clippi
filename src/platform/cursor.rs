//! Get the current cursor position in screen coordinates.

/// Returns the cursor position as (x, y) in screen coordinates.
/// Uses platform-specific APIs.
#[cfg(target_os = "windows")]
pub fn get_cursor_pos() -> (i32, i32) {
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) };
    (pt.x, pt.y)
}

#[cfg(target_os = "macos")]
pub fn get_cursor_pos() -> (i32, i32) {
    let loc = unsafe { objc2_app_kit::NSEvent::mouseLocation() };
    // NSEvent.mouseLocation returns the cursor position in screen coordinates,
    // but with y origin at the bottom-left. We convert to top-left origin to
    // match the Slint/winit coordinate system.
    let screen_height = unsafe {
        let screen = objc2_app_kit::NSScreen::mainScreen();
        screen.frame().size.height
    } as i32;
    (loc.x as i32, screen_height - loc.y as i32)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn get_cursor_pos() -> (i32, i32) {
    (0, 0)
}
