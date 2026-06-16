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
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
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
#[cfg(target_os = "windows")]
const VK_MENU_KEY: u16 = 0x12;
#[cfg(target_os = "windows")]
const VK_LWIN_KEY: u16 = 0x5B;

#[cfg(target_os = "windows")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct PasteShortcut {
    modifiers: Vec<u16>,
    key: u16,
}

#[cfg(target_os = "windows")]
impl PasteShortcut {
    fn ctrl_v() -> Self {
        Self {
            modifiers: vec![VK_CONTROL],
            key: VK_V,
        }
    }

    fn shift_insert() -> Self {
        Self {
            modifiers: vec![VK_SHIFT],
            key: VK_INSERT,
        }
    }
}

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
#[cfg(target_os = "windows")]
fn resolve_paste_shortcut(
    target_hwnd: Option<usize>,
    paste_shortcuts: &[crate::core::settings::PasteShortcutEntry],
) -> PasteShortcut {
    let Some(hwnd) = target_hwnd else {
        return PasteShortcut::ctrl_v();
    };
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;

    // 1. Check user-configured shortcuts
    if !paste_shortcuts.is_empty() {
        if let Some(proc_name) = get_process_name_from_hwnd(hwnd) {
            for entry in paste_shortcuts {
                if entry.app_name.eq_ignore_ascii_case(&proc_name) {
                    if let Some(shortcut) = parse_windows_paste_shortcut(&entry.shortcut) {
                        return shortcut;
                    }
                    log::warn!(
                        "Invalid paste shortcut {:?} for app {:?}; falling back to default paste",
                        entry.shortcut,
                        entry.app_name
                    );
                    return PasteShortcut::ctrl_v();
                }
            }
        }
    }

    // 2. Smart detection: console/terminal → Shift+Insert
    if is_console_window(hwnd) {
        return PasteShortcut::shift_insert();
    }

    // 3. Default: Ctrl+V
    PasteShortcut::ctrl_v()
}

#[cfg(target_os = "windows")]
fn parse_windows_paste_shortcut(value: &str) -> Option<PasteShortcut> {
    let mut modifiers = Vec::new();
    let mut key = None;

    for part in value.trim().split('+') {
        let part = part.trim().to_lowercase();
        match part.as_str() {
            "ctrl" | "control" => push_unique(&mut modifiers, VK_CONTROL),
            "alt" | "option" => push_unique(&mut modifiers, VK_MENU_KEY),
            "shift" => push_unique(&mut modifiers, VK_SHIFT),
            "win" | "cmd" | "command" | "super" | "meta" => {
                push_unique(&mut modifiers, VK_LWIN_KEY)
            }
            name => key = windows_key_name_to_vk(name),
        }
    }

    key.map(|key| PasteShortcut { modifiers, key })
}

#[cfg(target_os = "windows")]
fn push_unique(values: &mut Vec<u16>, value: u16) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(target_os = "windows")]
fn windows_key_name_to_vk(name: &str) -> Option<u16> {
    match name {
        "a" => Some(0x41),
        "b" => Some(0x42),
        "c" => Some(0x43),
        "d" => Some(0x44),
        "e" => Some(0x45),
        "f" => Some(0x46),
        "g" => Some(0x47),
        "h" => Some(0x48),
        "i" => Some(0x49),
        "j" => Some(0x4A),
        "k" => Some(0x4B),
        "l" => Some(0x4C),
        "m" => Some(0x4D),
        "n" => Some(0x4E),
        "o" => Some(0x4F),
        "p" => Some(0x50),
        "q" => Some(0x51),
        "r" => Some(0x52),
        "s" => Some(0x53),
        "t" => Some(0x54),
        "u" => Some(0x55),
        "v" => Some(VK_V),
        "w" => Some(0x57),
        "x" => Some(0x58),
        "y" => Some(0x59),
        "z" => Some(0x5A),
        "0" => Some(0x30),
        "1" => Some(0x31),
        "2" => Some(0x32),
        "3" => Some(0x33),
        "4" => Some(0x34),
        "5" => Some(0x35),
        "6" => Some(0x36),
        "7" => Some(0x37),
        "8" => Some(0x38),
        "9" => Some(0x39),
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),
        "space" => Some(0x20),
        "tab" => Some(0x09),
        "enter" | "return" => Some(0x0D),
        "esc" | "escape" => Some(0x1B),
        "backspace" => Some(0x08),
        "=" | "equal" => Some(0xBB),
        "-" | "minus" => Some(0xBD),
        "[" | "bracketleft" => Some(0xDB),
        "]" | "bracketright" => Some(0xDD),
        "'" | "quote" => Some(0xDE),
        ";" | "semicolon" => Some(0xBA),
        "\\" | "backslash" => Some(0xDC),
        "," | "comma" => Some(0xBC),
        "." | "period" => Some(0xBE),
        "/" | "slash" => Some(0xBF),
        "`" | "backquote" => Some(0xC0),
        "insert" | "ins" => Some(VK_INSERT),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn is_extended_key(vk: u16) -> bool {
    vk == VK_INSERT || vk == VK_LWIN_KEY
}

/// Send the paste keystroke via SendInput.
#[cfg(target_os = "windows")]
fn send_paste_keystroke(shortcut: &PasteShortcut) {
    // SAFETY: `SendInput` with a correctly-initialised INPUT array is the standard
    // Windows API for synthesizing keyboard input. All INPUT structs are fully
    // initialised via zeroed() + field assignment before the call, and the array
    // size matches the count parameter.
    unsafe {
        let input_count = (shortcut.modifiers.len() + 1) * 2;
        let mut inputs: Vec<INPUT> = (0..input_count).map(|_| std::mem::zeroed()).collect();
        let mut idx = 0;

        for &vk in &shortcut.modifiers {
            set_key_input(&mut inputs[idx], vk, false);
            idx += 1;
        }
        set_key_input(&mut inputs[idx], shortcut.key, false);
        idx += 1;
        set_key_input(&mut inputs[idx], shortcut.key, true);
        idx += 1;
        for &vk in shortcut.modifiers.iter().rev() {
            set_key_input(&mut inputs[idx], vk, true);
            idx += 1;
        }

        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

#[cfg(target_os = "windows")]
fn set_key_input(input: &mut INPUT, vk: u16, key_up: bool) {
    input.r#type = INPUT_KEYBOARD;
    let mut flags = if key_up { KEYEVENTF_KEYUP } else { 0 };
    if is_extended_key(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    input.Anonymous.ki = KEYBDINPUT {
        wVk: vk,
        wScan: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
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

    let shortcut = resolve_paste_shortcut(target_hwnd, &paste_shortcuts);

    std::thread::spawn(move || {
        wait_for_focus_and_send_paste(target_hwnd, shortcut);
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
    let shortcut = resolve_paste_shortcut(target_hwnd, &paste_shortcuts);
    wait_for_focus_and_send_paste(target_hwnd, shortcut);
}

#[cfg(target_os = "windows")]
fn wait_for_focus_and_send_paste(
    target_hwnd: Option<usize>,
    shortcut: PasteShortcut,
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

    send_paste_keystroke(&shortcut);
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
