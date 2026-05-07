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

#[cfg(target_os = "macos")]
const SLEEP_MS: u64 = 50;

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
        std::thread::sleep(std::time::Duration::from_millis(SLEEP_MS));
        let source = core_graphics::event_source::CGEventSource::new(
            core_graphics::event_source::CGEventSourceStateID::CombinedSessionState
        );
        let Ok(source) = source else { return };

        // Cmd down
        if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x37, true) {
            event.post(core_graphics::event::CGEventTapLocation::HID);
        }
        // V down
        if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, true) {
            event.post(core_graphics::event::CGEventTapLocation::HID);
        }
        // V up
        if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0x09, false) {
            event.post(core_graphics::event::CGEventTapLocation::HID);
        }
        // Cmd up
        if let Ok(event) = core_graphics::event::CGEvent::new_keyboard_event(source, 0x37, false) {
            event.post(core_graphics::event::CGEventTapLocation::HID);
        }
    });
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn restore_paste_target() {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn paste_after_delay() {}
