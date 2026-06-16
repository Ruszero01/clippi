//! Paste simulation - simulates paste keystrokes and restore focus.
//!
//! On Windows, the paste shortcut is resolved via:
//! 1. User-configured per-process shortcuts (from settings)
//! 2. Automatic detection of console/terminal windows → Shift+Insert
//! 3. Default: Ctrl+V

use std::sync::Arc;

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::CloseHandle;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_EXTENDEDKEY,
    VK_CONTROL, VK_INSERT, VK_SHIFT, VK_V,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
    IsWindow, SetForegroundWindow,
};

#[cfg(target_os = "windows")]
const BASE_DELAY_MS: u64 = 50;
#[cfg(target_os = "windows")]
const FOCUS_CHECK_INTERVAL_MS: u64 = 10;
#[cfg(target_os = "windows")]
const FOCUS_TIMEOUT_MS: u64 = 500;

/// Detect whether a window is a console/terminal window that doesn't support Ctrl+V.
///
/// Checks the window class name against known terminal classes:
/// - `ConsoleWindowClass` — classic console host (conhost.exe: cmd, PowerShell 5)
/// - `CASCADIA_HOSTING_WINDOW_CLASS` — Windows Terminal
#[cfg(target_os = "windows")]
fn is_console_window(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    let mut class_name = [0u16; 64];
    // SAFETY: `GetClassNameW` reads the window class name into a stack-allocated
    // buffer of 64 WCHARs. The HWND is validated by `IsWindow` before this call
    // in `wait_for_focus_and_send_paste`.
    let len = unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
    if len == 0 {
        return false;
    }
    let class_str = String::from_utf16_lossy(&class_name[..len as usize]);
    // Also check for popular third-party terminals via partial match
    class_str == "ConsoleWindowClass"
        || class_str == "CASCADIA_HOSTING_WINDOW_CLASS"
        || class_str.starts_with("PuTTY")
        || class_str == "mintty"
}

/// Extract process name (exe stem) from a window handle.
/// Returns None if any API call fails.
#[cfg(target_os = "windows")]
fn get_process_name_from_hwnd(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<String> {
    // SAFETY: `GetWindowThreadProcessId` is a read-only query that extracts the
    // PID from a valid HWND. `OpenProcess` with PROCESS_QUERY_LIMITED_INFORMATION
    // is the least-privilege access mode. `QueryFullProcessImageNameW` writes into
    // a stack-allocated buffer with correct length. `CloseHandle` is always called
    // on non-null handles.
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }

        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }

        let mut exe_buf = [0u16; 260];
        let mut exe_len = exe_buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            0, // PROCESS_NAME_WIN32
            exe_buf.as_mut_ptr(),
            &mut exe_len,
        );
        CloseHandle(process);

        if result == 0 {
            return None;
        }

        let exe_path = String::from_utf16_lossy(&exe_buf[..exe_len as usize]);
        std::path::Path::new(&exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    }
}

/// Resolve which paste keystroke to use:
/// 1. User-configured per-process shortcut (highest priority)
/// 2. Smart detection of console/terminal windows → Shift+Insert
/// 3. Default: Ctrl+V
///
/// Returns true if Shift+Insert should be used, false for Ctrl+V.
#[cfg(target_os = "windows")]
fn resolve_paste_shortcut(
    target_hwnd: Option<usize>,
    paste_shortcuts: &[crate::core::settings::PasteShortcutEntry],
) -> bool {
    let Some(hwnd) = target_hwnd else {
        return false;
    };
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;

    // 1. Check user-configured shortcuts
    if !paste_shortcuts.is_empty() {
        if let Some(proc_name) = get_process_name_from_hwnd(hwnd) {
            for entry in paste_shortcuts {
                if entry.app_name.eq_ignore_ascii_case(&proc_name) {
                    // If shortcut contains "shift" it's likely Shift+Insert
                    return entry.shortcut.to_lowercase().contains("shift");
                }
            }
        }
    }

    // 2. Smart detection: console/terminal → Shift+Insert
    if is_console_window(hwnd) {
        return true;
    }

    // 3. Default: Ctrl+V
    false
}

/// Send the paste keystroke via SendInput.
/// When `use_shift_insert` is true, sends Shift+Insert instead of Ctrl+V.
#[cfg(target_os = "windows")]
fn send_paste_keystroke(use_shift_insert: bool) {
    // SAFETY: `SendInput` with a correctly-initialised INPUT array is the standard
    // Windows API for synthesizing keyboard input. All INPUT structs are fully
    // initialised via zeroed() + field assignment before the call, and the array
    // size matches the count parameter.
    unsafe {
        if use_shift_insert {
            let mut inputs: [INPUT; 4] = std::mem::zeroed();

            // Shift down
            inputs[0].r#type = INPUT_KEYBOARD;
            inputs[0].Anonymous.ki = KEYBDINPUT {
                wVk: VK_SHIFT,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            };

            // Insert down (extended key)
            inputs[1].r#type = INPUT_KEYBOARD;
            inputs[1].Anonymous.ki = KEYBDINPUT {
                wVk: VK_INSERT,
                wScan: 0,
                dwFlags: KEYEVENTF_EXTENDEDKEY,
                time: 0,
                dwExtraInfo: 0,
            };

            // Insert up (extended key)
            inputs[2].r#type = INPUT_KEYBOARD;
            inputs[2].Anonymous.ki = KEYBDINPUT {
                wVk: VK_INSERT,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP | KEYEVENTF_EXTENDEDKEY,
                time: 0,
                dwExtraInfo: 0,
            };

            // Shift up
            inputs[3].r#type = INPUT_KEYBOARD;
            inputs[3].Anonymous.ki = KEYBDINPUT {
                wVk: VK_SHIFT,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            };

            SendInput(4, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        } else {
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
}

/// Restore focus to the last non-Clippi foreground window (paste target)
#[cfg(target_os = "windows")]
pub fn restore_paste_target() {
    if let Some(hwnd) = crate::platform::focus::get_last_non_clippi_window() {
        // SAFETY: `IsWindow` only reads window validity; `SetForegroundWindow`
        // is safe when the HWND is known valid (IsWindow check) and belongs to
        // a non-Clippi process.
        if unsafe { IsWindow(hwnd) } != 0 {
            unsafe { SetForegroundWindow(hwnd) };
        }
    }
}

/// Simulate paste after restoring focus.
///
/// Accepts per-process shortcut config for override. The shortcut decision
/// is resolved before spawning the thread so the boolean (not the settings
/// collection) is captured.
#[cfg(target_os = "windows")]
pub fn paste_after_delay(paste_shortcuts: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    let target_hwnd: Option<usize> =
        crate::platform::focus::get_last_non_clippi_window().map(|h| h as usize);

    let use_shift_insert = resolve_paste_shortcut(target_hwnd, &paste_shortcuts);

    std::thread::spawn(move || {
        wait_for_focus_and_send_paste(target_hwnd, use_shift_insert);
    });
}

/// Synchronous paste — blocks the calling thread until paste keystroke is sent.
///
/// Used for batch paste newline separators to avoid clipboard race conditions
/// between the separator write and the next item write.
/// Caller must call `restore_paste_target()` before invoking.
#[cfg(target_os = "windows")]
pub fn paste_sync(paste_shortcuts: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    let target_hwnd: Option<usize> =
        crate::platform::focus::get_last_non_clippi_window().map(|h| h as usize);
    let use_shift_insert = resolve_paste_shortcut(target_hwnd, &paste_shortcuts);
    wait_for_focus_and_send_paste(target_hwnd, use_shift_insert);
}

#[cfg(target_os = "windows")]
fn wait_for_focus_and_send_paste(
    target_hwnd: Option<usize>,
    use_shift_insert: bool,
) {
    // Initial delay for SetForegroundWindow to take effect
    std::thread::sleep(std::time::Duration::from_millis(BASE_DELAY_MS));

    // Verify target window is actually foreground before pasting
    if let Some(hwnd) = target_hwnd {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        // SAFETY: `IsWindow` is a read-only query on a known HWND value from the
        // focus watcher; `GetForegroundWindow` returns the current foreground HWND
        // which is always valid or null-safe.
        if unsafe { IsWindow(hwnd) } != 0 {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(FOCUS_TIMEOUT_MS);
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

    send_paste_keystroke(use_shift_insert);
}

#[cfg(any(target_os = "macos", test))]
fn should_request_accessibility_permission(is_trusted: bool, already_requested: bool) -> bool {
    !is_trusted && !already_requested
}

#[cfg(target_os = "macos")]
pub fn check_accessibility_permission() -> bool {
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }
    unsafe { AXIsProcessTrusted() }
}

/// Ask macOS to show the Accessibility permission prompt when needed.
///
/// The prompt is asynchronous. The return value reports the permission state
/// before the prompt, so callers should not assume permission was granted yet.
#[cfg(target_os = "macos")]
pub fn request_accessibility_permission() -> bool {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    use std::sync::atomic::{AtomicBool, Ordering};

    extern "C" {
        static kAXTrustedCheckOptionPrompt: CFTypeRef;
        fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
    }

    static PROMPT_REQUESTED: AtomicBool = AtomicBool::new(false);

    let is_trusted = check_accessibility_permission();
    let already_requested = PROMPT_REQUESTED.swap(true, Ordering::SeqCst);
    if !should_request_accessibility_permission(is_trusted, already_requested) {
        return is_trusted;
    }

    let prompt_key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt.cast()) };
    let options =
        CFDictionary::from_CFType_pairs(&[(prompt_key, CFBoolean::true_value())]).to_untyped();
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef().cast()) }
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
                // --- activation behavior on macOS 12–13 (our minimum is 12.0). ---
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
pub fn paste_after_delay(_paste_shortcuts: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    std::thread::spawn(move || {
        send_cmd_v();
    });
}

/// Synchronous paste — blocks until Cmd+V is sent (used for batch paste separators).
/// Caller must call `restore_paste_target()` before invoking.
#[cfg(target_os = "macos")]
pub fn paste_sync(_paste_shortcuts: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {
    send_cmd_v();
}

/// Send Cmd+V keyboard events to the HID event stream.
///
/// Uses `CGEventPost` to the HID event tap location so the system delivers
/// Cmd+V to whichever application is frontmost at the time of posting.
/// The caller must have called `restore_paste_target()` beforehand to activate
/// the target application and waited long enough for it to become frontmost.
///
/// This requires the Accessibility permission to be granted in
/// System Settings → Privacy & Security → Accessibility.
/// Without it, macOS silently drops the events and nothing happens.
#[cfg(target_os = "macos")]
fn send_cmd_v() {
    std::thread::sleep(std::time::Duration::from_millis(SLEEP_MS));

    if !check_accessibility_permission() {
        log::warn!("macOS Accessibility permission is missing; Cmd+V event was not sent");
        return;
    }

    let source = core_graphics::event_source::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::CombinedSessionState,
    );
    let Ok(source) = source else { return };

    let cmd_flag = core_graphics::event::CGEventFlags::CGEventFlagCommand;
    let hid = core_graphics::event::CGEventTapLocation::HID;

    // --- Cmd down — modifiers were NOT active before pressing Cmd ---
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x37, true)
    {
        event.post(hid);
    }
    // --- V down — Cmd IS held ---
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, true)
    {
        event.set_flags(cmd_flag);
        event.post(hid);
    }
    // --- V up — Cmd IS held ---
    if let Ok(event) =
        core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, false)
    {
        event.set_flags(cmd_flag);
        event.post(hid);
    }
    // --- Cmd up — Cmd WAS held before releasing ---
    if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source, 0x37, false) {
        event.set_flags(cmd_flag);
        event.post(hid);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn restore_paste_target() {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_after_delay(_paste_shortcuts: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_sync(_paste_shortcuts: Arc<Vec<crate::core::settings::PasteShortcutEntry>>) {}

#[cfg(test)]
mod tests {
    use super::should_request_accessibility_permission;

    #[test]
    fn accessibility_prompt_is_requested_once_when_permission_is_missing() {
        assert!(should_request_accessibility_permission(false, false));
        assert!(!should_request_accessibility_permission(false, true));
        assert!(!should_request_accessibility_permission(true, false));
    }
}
