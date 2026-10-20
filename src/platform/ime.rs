//! Preserve IME-owned keystrokes before GPUI 0.2.2 decodes VK_PROCESSKEY.
//!
//! Sogou can keep preedit text in its candidate window without giving GPUI a
//! marked range. GPUI then treats the recovered virtual key as an editor action,
//! so Backspace deletes committed text instead of editing the composition.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_PROCESSKEY;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessagePos, GetMessageTime, TranslateMessage, MSG, WM_KEYDOWN, WM_KEYUP, WM_NCDESTROY,
};

const SUBCLASS_ID: usize = 0x434C_494D;

/// Called on the main window's owning UI thread, before accepting text input.
pub fn install(hwnd: isize) -> Result<(), &'static str> {
    let hwnd = hwnd as HWND;
    if hwnd.is_null() {
        return Err("missing main window handle");
    }
    // SAFETY: the caller owns this GPUI window and runs on its UI thread.
    if unsafe { SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID, 0) } == 0 {
        return Err("could not install IME key handling");
    }
    Ok(())
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
        WM_KEYDOWN if wparam == VK_PROCESSKEY as usize => {
            // Preserve VK_PROCESSKEY: Windows/IME still needs TranslateMessage
            // to produce composition/result messages. Do not recover the original
            // key or dispatch it as a second editor/navigation action.
            // SAFETY: this message belongs to the current thread's window.
            unsafe {
                let position = GetMessagePos();
                let msg = MSG {
                    hwnd,
                    message,
                    wParam: wparam,
                    lParam: lparam,
                    time: GetMessageTime() as u32,
                    pt: windows_sys::Win32::Foundation::POINT {
                        x: position as u16 as i16 as i32,
                        y: (position >> 16) as u16 as i16 as i32,
                    },
                };
                if TranslateMessage(&msg) == 0 {
                    log::warn!(
                        "ime: TranslateMessage failed for VK_PROCESSKEY: {}",
                        std::io::Error::last_os_error()
                    );
                }
            }
            0
        }
        WM_KEYUP if wparam == VK_PROCESSKEY as usize => 0,
        WM_NCDESTROY => {
            // SAFETY: remove only this module's subclass and forward destruction.
            unsafe {
                RemoveWindowSubclass(hwnd, Some(window_proc), SUBCLASS_ID);
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
        }
        // Composition, committed text, ordinary keys and focus messages must
        // continue through the existing GPUI input pipeline unchanged.
        _ => unsafe { DefSubclassProc(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_BACK;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SendMessageW, WM_IME_COMPOSITION,
    };

    unsafe extern "system" fn observe(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        if matches!(message, WM_KEYDOWN | WM_KEYUP | WM_IME_COMPOSITION) {
            // SAFETY: the test keeps this Cell alive until window destruction.
            let count = unsafe { &*(data as *const Cell<usize>) };
            count.set(count.get() + 1);
            return 42;
        }
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }

    #[test]
    fn ime_owned_keys_bypass_editor_but_other_input_is_forwarded() {
        // Hidden native window: exercises the real subclass chain without
        // launching Clippi, taking focus, or requiring an installed IME.
        unsafe {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            let count = Cell::new(0usize);
            assert_ne!(
                SetWindowSubclass(hwnd, Some(observe), 1, &count as *const _ as usize),
                0
            );
            install(hwnd as isize).unwrap();
            for message in [WM_KEYDOWN, WM_KEYUP] {
                assert_eq!(SendMessageW(hwnd, message, VK_PROCESSKEY as usize, 0), 0);
            }
            assert_eq!(count.get(), 0);
            for (message, key) in [
                (WM_KEYDOWN, VK_BACK as usize),
                (WM_KEYUP, VK_BACK as usize),
                (WM_IME_COMPOSITION, 0),
            ] {
                assert_eq!(SendMessageW(hwnd, message, key, 0), 42);
            }
            assert_eq!(count.get(), 3);
            assert_ne!(DestroyWindow(hwnd), 0);
        }
    }
}
