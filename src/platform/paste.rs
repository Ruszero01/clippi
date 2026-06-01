//! Paste simulation - simulates Ctrl+V to paste content and restore focus

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::CloseHandle;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_MENU, VK_V,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow,
    SetForegroundWindow, GUITHREADINFO,
};

#[cfg(target_os = "windows")]
const BASE_DELAY_MS: u64 = 50;
#[cfg(target_os = "windows")]
const FOCUS_CHECK_INTERVAL_MS: u64 = 10;
#[cfg(target_os = "windows")]
const FOCUS_TIMEOUT_MS: u64 = 500;

/// Restore focus to the last non-Clippi foreground window (paste target)
#[cfg(target_os = "windows")]
pub fn restore_paste_target() {
    if let Some(hwnd) = crate::platform::focus::get_last_non_clippi_window() {
        if unsafe { IsWindow(hwnd) } != 0 {
            unsafe { SetForegroundWindow(hwnd) };
        }
    }
}

/// Simulate Ctrl+V using SendInput after verifying target window has focus.
///
/// Uses `SendInput` (replaces deprecated `keybd_event`) to send all 4 key
/// events atomically, preventing interleaving with real user input.
/// Before sending, polls `GetForegroundWindow` until the target window is
/// actually in the foreground (up to `FOCUS_TIMEOUT_MS`).
#[cfg(target_os = "windows")]
pub fn paste_after_delay() {
    let target_hwnd: Option<usize> =
        crate::platform::focus::get_last_non_clippi_window().map(|h| h as usize);

    std::thread::spawn(move || {
        wait_for_focus_and_send_ctrl_v(target_hwnd);
    });
}

/// Synchronous paste — blocks the calling thread until Ctrl+V is sent.
/// Used for batch paste newline separators to avoid clipboard race conditions
/// between the separator write and the next item write.
/// Caller must call `restore_paste_target()` before invoking.
#[cfg(target_os = "windows")]
pub fn paste_sync() {
    let target_hwnd: Option<usize> =
        crate::platform::focus::get_last_non_clippi_window().map(|h| h as usize);
    wait_for_focus_and_send_ctrl_v(target_hwnd);
}

/// Check if a window belongs to Windows Explorer (explorer.exe).
#[cfg(target_os = "windows")]
fn is_explorer_window(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return false;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut exe_buf = [0u16; 260];
        let mut exe_len = exe_buf.len() as u32;
        let result = QueryFullProcessImageNameW(process, 0, exe_buf.as_mut_ptr(), &mut exe_len);
        CloseHandle(process);
        if result == 0 {
            return false;
        }
        let exe_path = String::from_utf16_lossy(&exe_buf[..exe_len as usize]);
        exe_path.to_lowercase().ends_with("\\explorer.exe")
    }
}

/// Send Alt+D to open the address bar in Explorer (selects all text).
/// Works in Explorer and most browsers.
#[cfg(target_os = "windows")]
unsafe fn send_alt_d() {
    let mut inputs: [INPUT; 4] = std::mem::zeroed();

    // Alt down
    inputs[0].r#type = INPUT_KEYBOARD;
    inputs[0].Anonymous.ki = KEYBDINPUT {
        wVk: VK_MENU,
        wScan: 0,
        dwFlags: 0,
        time: 0,
        dwExtraInfo: 0,
    };

    // D down
    inputs[1].r#type = INPUT_KEYBOARD;
    inputs[1].Anonymous.ki = KEYBDINPUT {
        wVk: 0x44,
        wScan: 0,
        dwFlags: 0,
        time: 0,
        dwExtraInfo: 0,
    };

    // D up
    inputs[2].r#type = INPUT_KEYBOARD;
    inputs[2].Anonymous.ki = KEYBDINPUT {
        wVk: 0x44,
        wScan: 0,
        dwFlags: KEYEVENTF_KEYUP,
        time: 0,
        dwExtraInfo: 0,
    };

    // Alt up
    inputs[3].r#type = INPUT_KEYBOARD;
    inputs[3].Anonymous.ki = KEYBDINPUT {
        wVk: VK_MENU,
        wScan: 0,
        dwFlags: KEYEVENTF_KEYUP,
        time: 0,
        dwExtraInfo: 0,
    };

    SendInput(4, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

/// Check if the currently focused control in a window is an Edit control.
/// Explorer's search box is a persistent Edit — if it has focus, we paste
/// directly instead of sending Alt+D (which would jump to the address bar).
#[cfg(target_os = "windows")]
unsafe fn is_focused_edit(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    let thread_id = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
    if thread_id == 0 {
        return false;
    }
    let mut gui_info: GUITHREADINFO = std::mem::zeroed();
    gui_info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
    if GetGUIThreadInfo(thread_id, &mut gui_info) == 0 {
        return false;
    }
    if gui_info.hwndFocus.is_null() {
        return false;
    }
    let mut class_buf = [0u16; 16];
    let len = GetClassNameW(
        gui_info.hwndFocus,
        class_buf.as_mut_ptr(),
        class_buf.len() as i32,
    );
    len > 0 && String::from_utf16_lossy(&class_buf[..len as usize]) == "Edit"
}

#[cfg(target_os = "windows")]
fn wait_for_focus_and_send_ctrl_v(target_hwnd: Option<usize>) {
    // Initial delay for SetForegroundWindow to take effect
    std::thread::sleep(std::time::Duration::from_millis(BASE_DELAY_MS));

    let mut paste_to_explorer = false;

    // Verify target window is actually foreground before pasting
    if let Some(hwnd) = target_hwnd {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        if unsafe { IsWindow(hwnd) } != 0 && is_explorer_window(hwnd) {
            let deadline =
                std::time::Instant::now()
                    + std::time::Duration::from_millis(FOCUS_TIMEOUT_MS);
            loop {
                if unsafe { GetForegroundWindow() } == hwnd {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(
                    FOCUS_CHECK_INTERVAL_MS,
                ));
            }

            // Only send Alt+D if the focused control in Explorer is NOT an Edit.
            // If an Edit control has focus (e.g., search box), it persisted through
            // the focus-loss and we can paste directly into it.
            if !unsafe { is_focused_edit(hwnd) } {
                paste_to_explorer = true;
            }
        }
    }

    // Explorer's address bar exits edit mode on focus loss.
    // Send Alt+D to re-open the address bar (selects all text) before pasting.
    if paste_to_explorer {
        unsafe { send_alt_d() };
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Send Ctrl+V atomically via SendInput
    unsafe {
        let mut inputs: [INPUT; 4] = std::mem::zeroed();

        // Ctrl down
        inputs[0].r#type = INPUT_KEYBOARD;
        inputs[0].Anonymous.ki = KEYBDINPUT {
            wVk: VK_CONTROL,
            wScan: 0,
            dwFlags: 0,
            time: 0,
            dwExtraInfo: 0,
        };

        // V down
        inputs[1].r#type = INPUT_KEYBOARD;
        inputs[1].Anonymous.ki = KEYBDINPUT {
            wVk: VK_V,
            wScan: 0,
            dwFlags: 0,
            time: 0,
            dwExtraInfo: 0,
        };

        // V up
        inputs[2].r#type = INPUT_KEYBOARD;
        inputs[2].Anonymous.ki = KEYBDINPUT {
            wVk: VK_V,
            wScan: 0,
            dwFlags: KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
        };

        // Ctrl up
        inputs[3].r#type = INPUT_KEYBOARD;
        inputs[3].Anonymous.ki = KEYBDINPUT {
            wVk: VK_CONTROL,
            wScan: 0,
            dwFlags: KEYEVENTF_KEYUP,
            time: 0,
            dwExtraInfo: 0,
        };

        SendInput(4, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(target_os = "macos")]
const SLEEP_MS: u64 = 100;

#[cfg(target_os = "macos")]
pub fn restore_paste_target() {
    if let Some(pid) = crate::platform::focus::get_last_non_clippi_pid() {
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let apps = workspace.runningApplications();
        for i in 0..apps.count() {
            let app = apps.objectAtIndex(i);
            if app.processIdentifier() == pid {
                // Use raw value to avoid deprecated NSApplicationActivateIgnoringOtherApps.
                // This flag is a no-op on macOS 14+ but still required for correct
                // activation behavior on macOS 12–13 (our minimum is 12.0).
                let options: u64 = 1 << 1; // NSApplicationActivateIgnoringOtherApps
                unsafe {
                    let _: bool = objc2::msg_send![&app, activateWithOptions: options];
                }
                break;
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub fn paste_after_delay() {
    std::thread::spawn(move || {
        send_cmd_v();
    });
}

/// Synchronous paste — blocks until Cmd+V is sent (used for batch paste separators).
/// Caller must call `restore_paste_target()` before invoking.
#[cfg(target_os = "macos")]
pub fn paste_sync() {
    send_cmd_v();
}

/// Send Cmd+V keyboard events directly to the target process via CGEventPostToPid.
/// This bypasses the HID event tap and does not require the target app to be
/// frontmost, avoiding TCC Accessibility permission issues in release builds.
#[cfg(target_os = "macos")]
fn send_cmd_v() {
    std::thread::sleep(std::time::Duration::from_millis(SLEEP_MS));

    // Get the target PID — must be a valid non-Clippi process
    let target_pid = crate::platform::focus::get_last_non_clippi_pid();
    let Some(pid) = target_pid else { return };

    let source = core_graphics::event_source::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::CombinedSessionState,
    );
    let Ok(source) = source else { return };

    let cmd_flag = core_graphics::event::CGEventFlags::CGEventFlagCommand;

    // Cmd down — modifiers were NOT active before pressing Cmd
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x37, true)
    {
        event.post_to_pid(pid);
    }
    // V down — Cmd IS held
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, true)
    {
        event.set_flags(cmd_flag);
        event.post_to_pid(pid);
    }
    // V up — Cmd IS held
    if let Ok(event) =
        core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, false)
    {
        event.set_flags(cmd_flag);
        event.post_to_pid(pid);
    }
    // Cmd up — Cmd WAS held before releasing
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source, 0x37, false) {
        event.set_flags(cmd_flag);
        event.post_to_pid(pid);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn restore_paste_target() {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_after_delay() {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_sync() {}
