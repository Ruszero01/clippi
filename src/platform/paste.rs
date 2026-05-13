//! Paste simulation - simulates Ctrl+V to paste content and restore focus

#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_V,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindow, SetForegroundWindow};

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
    let target_hwnd: Option<usize> = crate::platform::focus::get_last_non_clippi_window()
        .map(|h| h as usize);

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
    let target_hwnd: Option<usize> = crate::platform::focus::get_last_non_clippi_window()
        .map(|h| h as usize);
    wait_for_focus_and_send_ctrl_v(target_hwnd);
}

#[cfg(target_os = "windows")]
fn wait_for_focus_and_send_ctrl_v(target_hwnd: Option<usize>) {
    // Initial delay for SetForegroundWindow to take effect
    std::thread::sleep(std::time::Duration::from_millis(BASE_DELAY_MS));

    // Verify target window is actually foreground before pasting
    if let Some(hwnd) = target_hwnd {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        if unsafe { IsWindow(hwnd) } != 0 {
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(FOCUS_TIMEOUT_MS);
            loop {
                if unsafe { GetForegroundWindow() } == hwnd {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(FOCUS_CHECK_INTERVAL_MS));
            }
        }
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
#[allow(deprecated)]
pub fn restore_paste_target() {
    if let Some(pid) = crate::platform::focus::get_last_non_clippi_pid() {
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let apps = workspace.runningApplications();
        for i in 0..apps.count() {
            let app = apps.objectAtIndex(i);
            if app.processIdentifier() == pid {
                let _ = app.activateWithOptions(
                    objc2_app_kit::NSApplicationActivationOptions::ActivateIgnoringOtherApps,
                );
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
#[allow(deprecated)]
pub fn paste_sync() {
    send_cmd_v();
}

#[cfg(target_os = "macos")]
fn send_cmd_v() {
    std::thread::sleep(std::time::Duration::from_millis(SLEEP_MS));
    let source = core_graphics::event_source::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::CombinedSessionState
    );
    let Ok(source) = source else { return };

    let cmd_flag = core_graphics::event::CGEventFlags::CGEventFlagCommand;

    // Cmd down
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x37, true) {
        event.set_flags(cmd_flag);
        event.post(core_graphics::event::CGEventTapLocation::HID);
    }
    // V down (with Cmd flag)
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, true) {
        event.set_flags(cmd_flag);
        event.post(core_graphics::event::CGEventTapLocation::HID);
    }
    // V up (with Cmd flag)
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, false) {
        event.set_flags(cmd_flag);
        event.post(core_graphics::event::CGEventTapLocation::HID);
    }
    // Cmd up
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source, 0x37, false) {
        event.post(core_graphics::event::CGEventTapLocation::HID);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn restore_paste_target() {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_after_delay() {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_sync() {}
