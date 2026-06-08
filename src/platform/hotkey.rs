//! Hotkey management - platform-agnostic trait and Windows implementation

use std::time::Duration;

use crate::core::i18n;
use global_hotkey::hotkey::Code;

/// Hotkey listener - platform-agnostic trait (must be used on main thread)
pub trait HotkeyListener {
    fn stop(&mut self);
    fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String>;
    fn start_recording(&mut self);
    fn finish_recording(&mut self);
    fn poll_pressed(&self) -> bool;
    fn poll_recording_pressed(&mut self) -> Option<String>;
    /// Temporarily unregister the hotkey (for blacklist).
    /// Does nothing if already unregistered.
    fn unregister(&mut self);
    /// Re-register the hotkey after unregister().
    /// Does nothing if already registered.
    fn register(&mut self);
}

/// Shared keycode mapping: name string → Code variant (platform-agnostic).
pub(crate) fn key_name_to_code(name: &str) -> Option<Code> {
    match name {
        "a" => Some(Code::KeyA),
        "b" => Some(Code::KeyB),
        "c" => Some(Code::KeyC),
        "d" => Some(Code::KeyD),
        "e" => Some(Code::KeyE),
        "f" => Some(Code::KeyF),
        "g" => Some(Code::KeyG),
        "h" => Some(Code::KeyH),
        "i" => Some(Code::KeyI),
        "j" => Some(Code::KeyJ),
        "k" => Some(Code::KeyK),
        "l" => Some(Code::KeyL),
        "m" => Some(Code::KeyM),
        "n" => Some(Code::KeyN),
        "o" => Some(Code::KeyO),
        "p" => Some(Code::KeyP),
        "q" => Some(Code::KeyQ),
        "r" => Some(Code::KeyR),
        "s" => Some(Code::KeyS),
        "t" => Some(Code::KeyT),
        "u" => Some(Code::KeyU),
        "v" => Some(Code::KeyV),
        "w" => Some(Code::KeyW),
        "x" => Some(Code::KeyX),
        "y" => Some(Code::KeyY),
        "z" => Some(Code::KeyZ),
        "0" => Some(Code::Digit0),
        "1" => Some(Code::Digit1),
        "2" => Some(Code::Digit2),
        "3" => Some(Code::Digit3),
        "4" => Some(Code::Digit4),
        "5" => Some(Code::Digit5),
        "6" => Some(Code::Digit6),
        "7" => Some(Code::Digit7),
        "8" => Some(Code::Digit8),
        "9" => Some(Code::Digit9),
        "f1" => Some(Code::F1),
        "f2" => Some(Code::F2),
        "f3" => Some(Code::F3),
        "f4" => Some(Code::F4),
        "f5" => Some(Code::F5),
        "f6" => Some(Code::F6),
        "f7" => Some(Code::F7),
        "f8" => Some(Code::F8),
        "f9" => Some(Code::F9),
        "f10" => Some(Code::F10),
        "f11" => Some(Code::F11),
        "f12" => Some(Code::F12),
        "space" => Some(Code::Space),
        "tab" => Some(Code::Tab),
        "enter" | "return" => Some(Code::Enter),
        "esc" | "escape" => Some(Code::Escape),
        "backspace" => Some(Code::Backspace),
        _ => None,
    }
}

/// Shared keycode mapping: Code variant → display name (platform-agnostic).
pub(crate) fn key_code_to_name(code: Code) -> &'static str {
    match code {
        Code::KeyA => "A",
        Code::KeyB => "B",
        Code::KeyC => "C",
        Code::KeyD => "D",
        Code::KeyE => "E",
        Code::KeyF => "F",
        Code::KeyG => "G",
        Code::KeyH => "H",
        Code::KeyI => "I",
        Code::KeyJ => "J",
        Code::KeyK => "K",
        Code::KeyL => "L",
        Code::KeyM => "M",
        Code::KeyN => "N",
        Code::KeyO => "O",
        Code::KeyP => "P",
        Code::KeyQ => "Q",
        Code::KeyR => "R",
        Code::KeyS => "S",
        Code::KeyT => "T",
        Code::KeyU => "U",
        Code::KeyV => "V",
        Code::KeyW => "W",
        Code::KeyX => "X",
        Code::KeyY => "Y",
        Code::KeyZ => "Z",
        Code::Digit0 => "0",
        Code::Digit1 => "1",
        Code::Digit2 => "2",
        Code::Digit3 => "3",
        Code::Digit4 => "4",
        Code::Digit5 => "5",
        Code::Digit6 => "6",
        Code::Digit7 => "7",
        Code::Digit8 => "8",
        Code::Digit9 => "9",
        Code::F1 => "F1",
        Code::F2 => "F2",
        Code::F3 => "F3",
        Code::F4 => "F4",
        Code::F5 => "F5",
        Code::F6 => "F6",
        Code::F7 => "F7",
        Code::F8 => "F8",
        Code::F9 => "F9",
        Code::F10 => "F10",
        Code::F11 => "F11",
        Code::F12 => "F12",
        Code::Space => "Space",
        Code::Tab => "Tab",
        Code::Enter => "Enter",
        Code::Escape => "Esc",
        Code::Backspace => "Backspace",
        _ => "?",
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct WindowsHotkeyListener {
        manager: GlobalHotKeyManager,
        hotkey: HotKey,
        is_recording: AtomicBool,
        registered: AtomicBool,
    }

    impl WindowsHotkeyListener {
        pub fn new(hotkey_str: &str) -> Result<Self, String> {
            let manager = GlobalHotKeyManager::new()
                .map_err(|e| format!("Failed to create hotkey manager: {e}"))?;
            let hotkey = parse_hotkey(hotkey_str)?;

            // --- Try to register ---
            manager.register(hotkey).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("注册快捷键失败", "Failed to register hotkey")
                )
            })?;

            Ok(Self {
                manager,
                hotkey,
                is_recording: AtomicBool::new(false),
                registered: AtomicBool::new(true),
            })
        }
    }

    impl HotkeyListener for WindowsHotkeyListener {
        fn stop(&mut self) {
            let _ = self.manager.unregister(self.hotkey);
        }

        fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
            // --- Unregister old ---
            let _ = self.manager.unregister(self.hotkey);
            std::thread::sleep(Duration::from_millis(50));

            // --- Parse and register new ---
            let new_hotkey = parse_hotkey(hotkey_str)?;
            self.manager.register(new_hotkey).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("注册快捷键失败", "Failed to register hotkey")
                )
            })?;
            self.hotkey = new_hotkey;
            self.registered.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn start_recording(&mut self) {
            self.is_recording.store(true, Ordering::SeqCst);
        }

        fn finish_recording(&mut self) {
            self.is_recording.store(false, Ordering::SeqCst);
        }

        fn unregister(&mut self) {
            if self.registered.load(Ordering::SeqCst) {
                let _ = self.manager.unregister(self.hotkey);
                self.registered.store(false, Ordering::SeqCst);
            }
        }

        fn register(&mut self) {
            if !self.registered.load(Ordering::SeqCst) {
                match self.manager.register(self.hotkey) {
                    Ok(()) => {
                        self.registered.store(true, Ordering::SeqCst);
                    }
                    Err(e) => {
                        log::error!("hotkey register failed: {e}");
                    }
                }
            }
        }

        fn poll_pressed(&self) -> bool {
            if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.state() == HotKeyState::Pressed {
                    return event.id() == self.hotkey.id();
                }
            }
            false
        }

        fn poll_recording_pressed(&mut self) -> Option<String> {
            if !self.is_recording.load(Ordering::SeqCst) {
                return None;
            }
            detect_pressed_hotkey()
        }
    }

    fn detect_pressed_hotkey() -> Option<String> {
        let mut mods = Modifiers::empty();
        unsafe {
            if GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 {
                mods |= Modifiers::CONTROL;
            }
            if GetAsyncKeyState(VK_MENU.0 as i32) < 0 {
                mods |= Modifiers::ALT;
            }
            if GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 {
                mods |= Modifiers::SHIFT;
            }
            if GetAsyncKeyState(VK_LWIN.0 as i32) < 0 || GetAsyncKeyState(VK_RWIN.0 as i32) < 0 {
                mods |= Modifiers::SUPER;
            }
        }

        if let Some(code) = detect_pressed_key() {
            let mut parts = Vec::new();
            if mods.contains(Modifiers::SUPER) {
                parts.push("Win");
            }
            if mods.contains(Modifiers::CONTROL) {
                parts.push("Ctrl");
            }
            if mods.contains(Modifiers::ALT) {
                parts.push("Alt");
            }
            if mods.contains(Modifiers::SHIFT) {
                parts.push("Shift");
            }
            parts.push(code_to_name(code));
            Some(parts.join("+"))
        } else {
            None
        }
    }

    fn detect_pressed_key() -> Option<Code> {
        let vk_map: &[(i32, Code)] = &[
            (0x41, Code::KeyA),
            (0x42, Code::KeyB),
            (0x43, Code::KeyC),
            (0x44, Code::KeyD),
            (0x45, Code::KeyE),
            (0x46, Code::KeyF),
            (0x47, Code::KeyG),
            (0x48, Code::KeyH),
            (0x49, Code::KeyI),
            (0x4A, Code::KeyJ),
            (0x4B, Code::KeyK),
            (0x4C, Code::KeyL),
            (0x4D, Code::KeyM),
            (0x4E, Code::KeyN),
            (0x4F, Code::KeyO),
            (0x50, Code::KeyP),
            (0x51, Code::KeyQ),
            (0x52, Code::KeyR),
            (0x53, Code::KeyS),
            (0x54, Code::KeyT),
            (0x55, Code::KeyU),
            (0x56, Code::KeyV),
            (0x57, Code::KeyW),
            (0x58, Code::KeyX),
            (0x59, Code::KeyY),
            (0x5A, Code::KeyZ),
            (0x30, Code::Digit0),
            (0x31, Code::Digit1),
            (0x32, Code::Digit2),
            (0x33, Code::Digit3),
            (0x34, Code::Digit4),
            (0x35, Code::Digit5),
            (0x36, Code::Digit6),
            (0x37, Code::Digit7),
            (0x38, Code::Digit8),
            (0x39, Code::Digit9),
            (0x70, Code::F1),
            (0x71, Code::F2),
            (0x72, Code::F3),
            (0x73, Code::F4),
            (0x74, Code::F5),
            (0x75, Code::F6),
            (0x76, Code::F7),
            (0x77, Code::F8),
            (0x78, Code::F9),
            (0x79, Code::F10),
            (0x7A, Code::F11),
            (0x7B, Code::F12),
            (0x20, Code::Space),
            (0x09, Code::Tab),
            (0x0D, Code::Enter),
            (0x1B, Code::Escape),
            (0x08, Code::Backspace),
            // Symbol keys (match macOS coverage)
            (0xBB, Code::Equal),        // VK_OEM_PLUS
            (0xBD, Code::Minus),        // VK_OEM_MINUS
            (0xDB, Code::BracketLeft),  // VK_OEM_4
            (0xDD, Code::BracketRight), // VK_OEM_6
            (0xDE, Code::Quote),        // VK_OEM_7
            (0xBA, Code::Semicolon),    // VK_OEM_1
            (0xDC, Code::Backslash),    // VK_OEM_5
            (0xBC, Code::Comma),        // VK_OEM_COMMA
            (0xBE, Code::Period),       // VK_OEM_PERIOD
            (0xBF, Code::Slash),        // VK_OEM_2
            (0xC0, Code::Backquote),    // VK_OEM_3
        ];

        for (vk, code) in vk_map {
            unsafe {
                if GetAsyncKeyState(*vk) < 0 {
                    return Some(*code);
                }
            }
        }
        None
    }

    fn parse_hotkey(s: &str) -> Result<HotKey, String> {
        let s = s.trim().to_lowercase();
        let mut mods = Modifiers::empty();
        let mut key: Option<Code> = None;

        for part in s.split('+') {
            let part = part.trim();
            match part {
                "ctrl" | "control" => mods |= Modifiers::CONTROL,
                "alt" => mods |= Modifiers::ALT,
                "shift" => mods |= Modifiers::SHIFT,
                "win" | "super" | "meta" => mods |= Modifiers::SUPER,
                _ => {
                    key = name_to_code(part);
                }
            }
        }

        let key = key.ok_or(i18n::tr("未指定按键", "No key specified"))?;
        Ok(HotKey::new(Some(mods), key))
    }

    fn name_to_code(name: &str) -> Option<Code> {
        key_name_to_code(name)
    }

    fn code_to_name(code: Code) -> &'static str {
        key_code_to_name(code)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::sync::atomic::{AtomicBool, Ordering};

    // macOS virtual key codes for modifiers
    const KVK_SHIFT: u16 = 0x38;
    const KVK_CONTROL: u16 = 0x3B;
    const KVK_OPTION: u16 = 0x3A;
    const KVK_COMMAND: u16 = 0x37;

    // FFI binding for CGEventSourceKeyState.
    // --- This function is part of the public CoreGraphics C API but is not yet ---
    // wrapped by the core-graphics crate (checked v0.25). Using a raw extern
    // binding avoids pulling in a second CG bindings crate just for one function.
    extern "C" {
        fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    }

    /// Check if a key is currently pressed via CoreGraphics CGEventSourceKeyState.
    /// Uses CombinedSessionState (0) as the source state.
    fn is_key_pressed(vk: u16) -> bool {
        unsafe { CGEventSourceKeyState(0, vk) }
    }

    pub struct MacosHotkeyListener {
        manager: GlobalHotKeyManager,
        hotkey: HotKey,
        is_recording: AtomicBool,
        registered: AtomicBool,
    }

    impl MacosHotkeyListener {
        pub fn new(hotkey_str: &str) -> Result<Self, String> {
            let manager = GlobalHotKeyManager::new()
                .map_err(|e| format!("Failed to create hotkey manager: {e}"))?;
            let hotkey = parse_hotkey(hotkey_str)?;

            // --- Try to register ---
            manager.register(hotkey).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("注册快捷键失败", "Failed to register hotkey")
                )
            })?;

            Ok(Self {
                manager,
                hotkey,
                is_recording: AtomicBool::new(false),
                registered: AtomicBool::new(true),
            })
        }
    }

    impl HotkeyListener for MacosHotkeyListener {
        fn stop(&mut self) {
            let _ = self.manager.unregister(self.hotkey);
        }

        fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
            // --- Unregister old ---
            let _ = self.manager.unregister(self.hotkey);
            std::thread::sleep(Duration::from_millis(50));

            // --- Parse and register new ---
            let new_hotkey = parse_hotkey(hotkey_str)?;
            self.manager.register(new_hotkey).map_err(|e| {
                format!(
                    "{}: {e}",
                    i18n::tr("注册快捷键失败", "Failed to register hotkey")
                )
            })?;
            self.hotkey = new_hotkey;
            self.registered.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn start_recording(&mut self) {
            self.is_recording.store(true, Ordering::SeqCst);
        }

        fn finish_recording(&mut self) {
            self.is_recording.store(false, Ordering::SeqCst);
        }

        fn unregister(&mut self) {
            if self.registered.load(Ordering::SeqCst) {
                let _ = self.manager.unregister(self.hotkey);
                self.registered.store(false, Ordering::SeqCst);
            }
        }

        fn register(&mut self) {
            if !self.registered.load(Ordering::SeqCst) {
                match self.manager.register(self.hotkey) {
                    Ok(()) => {
                        self.registered.store(true, Ordering::SeqCst);
                    }
                    Err(e) => {
                        log::error!("hotkey register failed: {e}");
                    }
                }
            }
        }

        fn poll_pressed(&self) -> bool {
            if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.state() == HotKeyState::Pressed {
                    return event.id() == self.hotkey.id();
                }
            }
            false
        }

        fn poll_recording_pressed(&mut self) -> Option<String> {
            if !self.is_recording.load(Ordering::SeqCst) {
                return None;
            }
            detect_pressed_hotkey()
        }
    }

    fn detect_pressed_hotkey() -> Option<String> {
        let mut mods = Modifiers::empty();
        if is_key_pressed(KVK_COMMAND) {
            mods |= Modifiers::SUPER;
        }
        if is_key_pressed(KVK_CONTROL) {
            mods |= Modifiers::CONTROL;
        }
        if is_key_pressed(KVK_OPTION) {
            mods |= Modifiers::ALT;
        }
        if is_key_pressed(KVK_SHIFT) {
            mods |= Modifiers::SHIFT;
        }

        if let Some(code) = detect_pressed_key() {
            let mut parts = Vec::new();
            if mods.contains(Modifiers::SUPER) {
                parts.push("Cmd");
            }
            if mods.contains(Modifiers::CONTROL) {
                parts.push("Ctrl");
            }
            if mods.contains(Modifiers::ALT) {
                parts.push("Alt");
            }
            if mods.contains(Modifiers::SHIFT) {
                parts.push("Shift");
            }
            parts.push(code_to_name(code));
            Some(parts.join("+"))
        } else {
            None
        }
    }

    fn detect_pressed_key() -> Option<Code> {
        // --- macOS virtual key code to Code mapping ---
        let vk_map: &[(u16, Code)] = &[
            // --- Letters ---
            (0x00, Code::KeyA),
            (0x01, Code::KeyS),
            (0x02, Code::KeyD),
            (0x03, Code::KeyF),
            (0x04, Code::KeyH),
            (0x05, Code::KeyG),
            (0x06, Code::KeyZ),
            (0x07, Code::KeyX),
            (0x08, Code::KeyC),
            (0x09, Code::KeyV),
            (0x0B, Code::KeyB),
            (0x0C, Code::KeyQ),
            (0x0D, Code::KeyW),
            (0x0E, Code::KeyE),
            (0x0F, Code::KeyR),
            (0x10, Code::KeyY),
            (0x11, Code::KeyT),
            (0x12, Code::Digit1),
            (0x13, Code::Digit2),
            (0x14, Code::Digit3),
            (0x15, Code::Digit4),
            (0x16, Code::Digit6),
            (0x17, Code::Digit5),
            (0x18, Code::Equal),
            (0x19, Code::Digit9),
            (0x1A, Code::Digit7),
            (0x1B, Code::Minus),
            (0x1C, Code::Digit8),
            (0x1D, Code::Digit0),
            (0x1E, Code::BracketRight),
            (0x1F, Code::KeyO),
            (0x20, Code::KeyU),
            (0x21, Code::BracketLeft),
            (0x22, Code::KeyI),
            (0x23, Code::KeyP),
            (0x25, Code::KeyL),
            (0x26, Code::KeyJ),
            (0x27, Code::Quote),
            (0x28, Code::KeyK),
            (0x29, Code::Semicolon),
            (0x2A, Code::Backslash),
            (0x2B, Code::Comma),
            (0x2C, Code::Slash),
            (0x2D, Code::KeyN),
            (0x2E, Code::KeyM),
            (0x2F, Code::Period),
            // --- Special keys ---
            (0x24, Code::Enter),     // kVK_Return
            (0x30, Code::Tab),       // kVK_Tab
            (0x31, Code::Space),     // kVK_Space
            (0x33, Code::Backspace), // kVK_Delete (Backspace)
            (0x35, Code::Escape),    // kVK_Escape
            (0x32, Code::Backquote), // kVK_ANSI_Grave
            // --- Function keys ---
            (0x7A, Code::F1),
            (0x78, Code::F2),
            (0x63, Code::F3),
            (0x76, Code::F4),
            (0x60, Code::F5),
            (0x61, Code::F6),
            (0x62, Code::F7),
            (0x64, Code::F8),
            (0x65, Code::F9),
            (0x6D, Code::F10),
            (0x67, Code::F11),
            (0x6F, Code::F12),
        ];

        for (vk, code) in vk_map {
            if is_key_pressed(*vk) {
                return Some(*code);
            }
        }
        None
    }

    fn parse_hotkey(s: &str) -> Result<HotKey, String> {
        let s = s.trim().to_lowercase();
        let mut mods = Modifiers::empty();
        let mut key: Option<Code> = None;

        for part in s.split('+') {
            let part = part.trim();
            match part {
                "ctrl" | "control" => mods |= Modifiers::CONTROL,
                "alt" | "option" => mods |= Modifiers::ALT,
                "shift" => mods |= Modifiers::SHIFT,
                "cmd" | "command" | "win" | "super" | "meta" => mods |= Modifiers::SUPER,
                _ => {
                    key = name_to_code(part);
                }
            }
        }

        let key = key.ok_or(i18n::tr("未指定按键", "No key specified"))?;
        Ok(HotKey::new(Some(mods), key))
    }

    fn name_to_code(name: &str) -> Option<Code> {
        key_name_to_code(name)
    }

    fn code_to_name(code: Code) -> &'static str {
        key_code_to_name(code)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub struct LinuxHotkeyListener;

    impl LinuxHotkeyListener {
        pub fn new(_hotkey_str: &str) -> Result<Self, String> {
            Ok(Self)
        }
    }

    impl HotkeyListener for LinuxHotkeyListener {
        fn stop(&mut self) {}
        fn update_hotkey(&mut self, _hotkey_str: &str) -> Result<(), String> {
            Ok(())
        }
        fn start_recording(&mut self) {}
        fn finish_recording(&mut self) {}
        fn poll_pressed(&self) -> bool {
            false
        }
        fn poll_recording_pressed(&mut self) -> Option<String> {
            None
        }
        fn unregister(&mut self) {}
        fn register(&mut self) {}
    }
}

#[cfg(target_os = "windows")]
pub use windows::WindowsHotkeyListener;

#[cfg(target_os = "macos")]
pub use macos::MacosHotkeyListener;

#[cfg(target_os = "linux")]
pub use linux::LinuxHotkeyListener;

pub fn create_hotkey_listener(hotkey_str: &str) -> Result<Box<dyn HotkeyListener>, String> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(WindowsHotkeyListener::new(hotkey_str)?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacosHotkeyListener::new(hotkey_str)?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(LinuxHotkeyListener::new(hotkey_str)?))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        panic!("Unsupported platform")
    }
}
