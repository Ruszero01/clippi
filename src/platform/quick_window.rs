//! Native input policy for the Windows quick popup.
//!
//! The popup is a display surface, never a keyboard-focus owner. Search input
//! comes exclusively from `keyboard_hook`; no input queues are joined and no
//! focus is acquired, reasserted, or restored by this module.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST, MA_NOACTIVATE,
    PA_NOACTIVATE, SWP_FRAMECHANGED, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
    SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WM_DISPLAYCHANGE, WM_MOUSEACTIVATE, WM_NCDESTROY,
    WM_POINTERACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

const SUBCLASS_ID: usize = 0x434C_5050;

/// Install the mandatory non-activating policy before the popup can be shown.
/// Failure leaves the caller responsible for keeping the popup hidden.
pub fn install(hwnd: isize) -> Result<(), &'static str> {
    let hwnd = hwnd as HWND;
    if hwnd.is_null() {
        return Err("missing quick window handle");
    }
    // SAFETY: the caller supplies its own GPUI window on the owning UI thread.
    // The subclass only makes activation decisions and forwards other messages.
    unsafe {
        if SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, 0) == 0 {
            return Err("could not install quick window input policy");
        }
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE)
            | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST) as i32;
        SetWindowLongW(hwnd, GWL_EXSTYLE, style);
        if GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_NOACTIVATE as i32 == 0 {
            return Err("could not apply WS_EX_NOACTIVATE");
        }
        if SetWindowPos(
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
        ) == 0
        {
            return Err("could not initialize non-activating quick window");
        }
    }
    Ok(())
}

/// The only show primitive for the quick popup. Callers cannot opt into activation.
pub fn show_at(hwnd: isize, x: i32, y: i32, width: i32, height: i32) -> bool {
    // SAFETY: the caller supplies the installed popup's HWND on the UI thread.
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
        ) != 0
    }
}

/// Hide without transferring activation to any owner window.
pub fn hide(hwnd: isize) -> bool {
    // SAFETY: the caller supplies the installed popup's HWND on the UI thread.
    unsafe {
        SetWindowPos(
            hwnd as HWND,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_HIDEWINDOW
                | SWP_NOACTIVATE
                | SWP_NOOWNERZORDER
                | SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOZORDER,
        ) != 0
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match message {
        // Handle both ordinary clicks/active-window tracking and touch/pen.
        // Preserve the input message so buttons and search still receive clicks.
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_POINTERACTIVATE => PA_NOACTIVATE as LRESULT,
        // GPUI 0.2.2 restores disconnected-monitor windows with activating
        // ShowWindow(SW_SHOWNORMAL). The WindowManager topology poll already
        // handles migration with show_at, so that fallback must never run here.
        WM_DISPLAYCHANGE => 0,
        WM_NCDESTROY => {
            // SAFETY: remove only our subclass and forward final destruction.
            unsafe {
                RemoveWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        // SAFETY: all remaining native behavior belongs to GPUI/Windows.
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
    use windows_sys::Win32::UI::Shell::GetWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetForegroundWindow, GetGUIThreadInfo,
        GetWindowThreadProcessId, IsWindowVisible, SendMessageW, GUITHREADINFO, WS_POPUP,
    };

    struct TestWindow(HWND);

    impl TestWindow {
        fn new(owner: HWND) -> Self {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: built-in window class, no pointer parameters or callbacks.
            // Off-screen and initially hidden: this never opens an application.
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_POPUP,
                    -32000,
                    -32000,
                    1,
                    1,
                    owner,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null(), "create native test window");
            Self(hwnd)
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: created on this test thread and destroyed exactly once.
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }

    fn input_snapshot() -> (HWND, HWND, HWND) {
        // SAFETY: read-only queries; initialized GUITHREADINFO with required size.
        unsafe {
            let foreground = GetForegroundWindow();
            let thread = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            assert_ne!(GetGUIThreadInfo(thread, &mut info), 0);
            (foreground, info.hwndFocus, GetFocus())
        }
    }

    #[test]
    fn repeated_show_hide_and_pointer_activation_preserve_input_owner() {
        let owner = TestWindow::new(std::ptr::null_mut());
        let popup = TestWindow::new(owner.0);
        install(popup.0 as isize).expect("install non-activating policy");
        let before = input_snapshot();
        let mut data = 0;
        assert_ne!(
            unsafe { GetWindowSubclass(popup.0, Some(window_proc), SUBCLASS_ID, &mut data) },
            0
        );

        for _ in 0..100 {
            assert!(show_at(popup.0 as isize, -32000, -32000, 1, 1));
            assert_eq!(input_snapshot(), before, "show changed input ownership");
            // SAFETY: send activation queries to our own live window on its thread.
            unsafe {
                assert_ne!(IsWindowVisible(popup.0), 0);
                assert_ne!(
                    GetWindowSubclass(popup.0, Some(window_proc), SUBCLASS_ID, &mut data),
                    0,
                    "native subclass lost after show"
                );
                assert_eq!(
                    SendMessageW(popup.0, WM_MOUSEACTIVATE, owner.0 as usize, 0),
                    MA_NOACTIVATE as isize
                );
                assert_eq!(
                    // User32 discards synthetic pointer messages without a live
                    // pointer ID; exercise the installed callback without injecting
                    // touch input into the user's desktop.
                    window_proc(
                        popup.0,
                        WM_POINTERACTIVATE,
                        owner.0 as usize,
                        0,
                        SUBCLASS_ID,
                        0
                    ),
                    PA_NOACTIVATE as isize
                );
            }
            assert_eq!(input_snapshot(), before, "pointer changed input ownership");
            assert!(hide(popup.0 as isize));
            assert_eq!(input_snapshot(), before, "hide changed input ownership");
            // A display notification must not let GPUI resurrect a hidden popup.
            unsafe {
                assert_eq!(SendMessageW(popup.0, WM_DISPLAYCHANGE, 32, 0), 0);
                assert_eq!(IsWindowVisible(popup.0), 0);
            }
            assert_eq!(
                input_snapshot(),
                before,
                "display change changed input ownership"
            );
        }
    }

    #[test]
    fn missing_window_cannot_install_policy() {
        assert!(install(0).is_err());
    }
}
