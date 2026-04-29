//! Hotkey management - platform-agnostic trait and Windows implementation

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Callback invoked when hotkey is pressed
pub type HotkeyCallback = Arc<Mutex<Box<dyn Fn() + Send>>>;
/// Callback invoked when recording detects a new hotkey (None = cancel, Some = new hotkey)
pub type RecordingCallback = Arc<Mutex<Box<dyn Fn(Option<String>) + Send>>>;

/// Hotkey listener - platform-agnostic trait
pub trait HotkeyListener: Send {
    fn stop(&mut self);
    fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String>;
    fn start_recording(&mut self);
    fn finish_recording(&mut self);
}

enum HotkeyCmd {
    Register(String),
    Unregister,
    Update(String),
    StartRecording,
    StopRecording,
    Shutdown,
}

struct HotkeyInner {
    cmd_tx: Sender<HotkeyCmd>,
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::sync::atomic::{AtomicBool, Ordering};
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN,
    };

    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    pub struct WindowsHotkeyListener {
        inner: Option<HotkeyInner>,
        is_recording: Arc<AtomicBool>,
    }

    impl WindowsHotkeyListener {
        pub fn new(
            hotkey_str: &str,
            on_pressed: HotkeyCallback,
            on_recording: Option<RecordingCallback>,
        ) -> Result<Self, String> {
            let is_recording = Arc::new(AtomicBool::new(false));

            let (cmd_tx, cmd_rx) = channel();

            let on_pressed_clone = on_pressed.clone();
            let is_recording_clone = is_recording.clone();
            let on_recording_clone = on_recording.clone();

            thread::spawn(move || {
                let manager = match GlobalHotKeyManager::new() {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Failed to create hotkey manager: {}", e);
                        return;
                    }
                };

                let mut hotkey: Option<HotKey> = None;
                let mut recording_active = false;

                loop {
                    // Handle commands
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        match cmd {
                            HotkeyCmd::Register(ref hs) => {
                                if let Ok(new_hk) = parse_hotkey(hs) {
                                    if let Some(ref old) = hotkey {
                                        let _ = manager.unregister(*old);
                                    }
                                    if manager.register(new_hk).is_ok() {
                                        hotkey = Some(new_hk);
                                    }
                                }
                            }
                            HotkeyCmd::Unregister => {
                                if let Some(ref old) = hotkey {
                                    let _ = manager.unregister(*old);
                                }
                                hotkey = None;
                            }
                            HotkeyCmd::Update(ref hs) => {
                                if let Ok(new_hk) = parse_hotkey(hs) {
                                    if let Some(ref old) = hotkey {
                                        let _ = manager.unregister(*old);
                                    }
                                    if manager.register(new_hk).is_ok() {
                                        hotkey = Some(new_hk);
                                    }
                                }
                            }
                            HotkeyCmd::StartRecording => {
                                recording_active = true;
                                is_recording_clone.store(true, Ordering::SeqCst);
                            }
                            HotkeyCmd::StopRecording => {
                                recording_active = false;
                                is_recording_clone.store(false, Ordering::SeqCst);
                            }
                            HotkeyCmd::Shutdown => return,
                        }
                    }

                    // Check for hotkey events (event-driven)
                    if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                        if event.state() == HotKeyState::Pressed {
                            if let Some(ref hk) = hotkey {
                                if event.id() == hk.id() {
                                    let cb = on_pressed_clone.lock().unwrap();
                                    cb();
                                }
                            }
                        }
                    }

                    // Poll for recording detection
                    if recording_active {
                        if let Some(ref cb) = on_recording_clone {
                            if let Some(new_hotkey) = detect_pressed_hotkey() {
                                let cb = cb.lock().unwrap();
                                cb(Some(new_hotkey));
                                recording_active = false;
                                is_recording_clone.store(false, Ordering::SeqCst);
                            }
                        }
                    }

                    thread::sleep(POLL_INTERVAL);
                }
            });

            // Send initial registration
            let _ = cmd_tx.send(HotkeyCmd::Register(hotkey_str.to_string()));

            Ok(Self {
                inner: Some(HotkeyInner { cmd_tx }),
                is_recording,
            })
        }

        fn send_cmd(&self, cmd: HotkeyCmd) {
            if let Some(ref inner) = self.inner {
                let _ = inner.cmd_tx.send(cmd);
            }
        }
    }

    impl HotkeyListener for WindowsHotkeyListener {
        fn stop(&mut self) {
            self.send_cmd(HotkeyCmd::Unregister);
        }

        fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
            self.send_cmd(HotkeyCmd::Update(hotkey_str.to_string()));
            Ok(())
        }

        fn start_recording(&mut self) {
            self.send_cmd(HotkeyCmd::StartRecording);
        }

        fn finish_recording(&mut self) {
            self.send_cmd(HotkeyCmd::StopRecording);
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
            (0x41, Code::KeyA), (0x42, Code::KeyB), (0x43, Code::KeyC),
            (0x44, Code::KeyD), (0x45, Code::KeyE), (0x46, Code::KeyF),
            (0x47, Code::KeyG), (0x48, Code::KeyH), (0x49, Code::KeyI),
            (0x4A, Code::KeyJ), (0x4B, Code::KeyK), (0x4C, Code::KeyL),
            (0x4D, Code::KeyM), (0x4E, Code::KeyN), (0x4F, Code::KeyO),
            (0x50, Code::KeyP), (0x51, Code::KeyQ), (0x52, Code::KeyR),
            (0x53, Code::KeyS), (0x54, Code::KeyT), (0x55, Code::KeyU),
            (0x56, Code::KeyV), (0x57, Code::KeyW), (0x58, Code::KeyX),
            (0x59, Code::KeyY), (0x5A, Code::KeyZ),
            (0x30, Code::Digit0), (0x31, Code::Digit1), (0x32, Code::Digit2),
            (0x33, Code::Digit3), (0x34, Code::Digit4), (0x35, Code::Digit5),
            (0x36, Code::Digit6), (0x37, Code::Digit7), (0x38, Code::Digit8),
            (0x39, Code::Digit9),
            (0x70, Code::F1), (0x71, Code::F2), (0x72, Code::F3),
            (0x73, Code::F4), (0x74, Code::F5), (0x75, Code::F6),
            (0x76, Code::F7), (0x77, Code::F8), (0x78, Code::F9),
            (0x79, Code::F10), (0x7A, Code::F11), (0x7B, Code::F12),
            (0x20, Code::Space), (0x09, Code::Tab), (0x0D, Code::Enter),
            (0x1B, Code::Escape), (0x08, Code::Backspace),
        ];

        for (vk, code) in vk_map {
            unsafe {
                if GetAsyncKeyState(*vk as i32) < 0 {
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

        let key = key.ok_or("未指定按键")?;
        Ok(HotKey::new(Some(mods), key))
    }

    fn name_to_code(name: &str) -> Option<Code> {
        match name {
            "a" => Some(Code::KeyA), "b" => Some(Code::KeyB), "c" => Some(Code::KeyC),
            "d" => Some(Code::KeyD), "e" => Some(Code::KeyE), "f" => Some(Code::KeyF),
            "g" => Some(Code::KeyG), "h" => Some(Code::KeyH), "i" => Some(Code::KeyI),
            "j" => Some(Code::KeyJ), "k" => Some(Code::KeyK), "l" => Some(Code::KeyL),
            "m" => Some(Code::KeyM), "n" => Some(Code::KeyN), "o" => Some(Code::KeyO),
            "p" => Some(Code::KeyP), "q" => Some(Code::KeyQ), "r" => Some(Code::KeyR),
            "s" => Some(Code::KeyS), "t" => Some(Code::KeyT), "u" => Some(Code::KeyU),
            "v" => Some(Code::KeyV), "w" => Some(Code::KeyW), "x" => Some(Code::KeyX),
            "y" => Some(Code::KeyY), "z" => Some(Code::KeyZ),
            "0" => Some(Code::Digit0), "1" => Some(Code::Digit1), "2" => Some(Code::Digit2),
            "3" => Some(Code::Digit3), "4" => Some(Code::Digit4), "5" => Some(Code::Digit5),
            "6" => Some(Code::Digit6), "7" => Some(Code::Digit7), "8" => Some(Code::Digit8),
            "9" => Some(Code::Digit9),
            "f1" => Some(Code::F1), "f2" => Some(Code::F2), "f3" => Some(Code::F3),
            "f4" => Some(Code::F4), "f5" => Some(Code::F5), "f6" => Some(Code::F6),
            "f7" => Some(Code::F7), "f8" => Some(Code::F8), "f9" => Some(Code::F9),
            "f10" => Some(Code::F10), "f11" => Some(Code::F11), "f12" => Some(Code::F12),
            "space" => Some(Code::Space),
            "tab" => Some(Code::Tab),
            "enter" | "return" => Some(Code::Enter),
            "esc" | "escape" => Some(Code::Escape),
            "backspace" => Some(Code::Backspace),
            _ => None,
        }
    }

    fn code_to_name(code: Code) -> &'static str {
        match code {
            Code::KeyA => "A", Code::KeyB => "B", Code::KeyC => "C",
            Code::KeyD => "D", Code::KeyE => "E", Code::KeyF => "F",
            Code::KeyG => "G", Code::KeyH => "H", Code::KeyI => "I",
            Code::KeyJ => "J", Code::KeyK => "K", Code::KeyL => "L",
            Code::KeyM => "M", Code::KeyN => "N", Code::KeyO => "O",
            Code::KeyP => "P", Code::KeyQ => "Q", Code::KeyR => "R",
            Code::KeyS => "S", Code::KeyT => "T", Code::KeyU => "U",
            Code::KeyV => "V", Code::KeyW => "W", Code::KeyX => "X",
            Code::KeyY => "Y", Code::KeyZ => "Z",
            Code::Digit0 => "0", Code::Digit1 => "1", Code::Digit2 => "2",
            Code::Digit3 => "3", Code::Digit4 => "4", Code::Digit5 => "5",
            Code::Digit6 => "6", Code::Digit7 => "7", Code::Digit8 => "8",
            Code::Digit9 => "9",
            Code::F1 => "F1", Code::F2 => "F2", Code::F3 => "F3",
            Code::F4 => "F4", Code::F5 => "F5", Code::F6 => "F6",
            Code::F7 => "F7", Code::F8 => "F8", Code::F9 => "F9",
            Code::F10 => "F10", Code::F11 => "F11", Code::F12 => "F12",
            Code::Space => "Space", Code::Tab => "Tab", Code::Enter => "Enter",
            Code::Escape => "Esc", Code::Backspace => "Backspace",
            _ => "?",
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub struct MacosHotkeyListener;

    impl MacosHotkeyListener {
        pub fn new(
            _hotkey_str: &str,
            _on_pressed: HotkeyCallback,
            _on_recording: Option<RecordingCallback>,
        ) -> Result<Self, String> {
            Ok(Self)
        }
    }

    impl HotkeyListener for MacosHotkeyListener {
        fn stop(&mut self) {}
        fn update_hotkey(&mut self, _hotkey_str: &str) -> Result<(), String> {
            todo!()
        }
        fn start_recording(&mut self) {}
        fn finish_recording(&mut self) {}
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub struct LinuxHotkeyListener;

    impl LinuxHotkeyListener {
        pub fn new(
            _hotkey_str: &str,
            _on_pressed: HotkeyCallback,
            _on_recording: Option<RecordingCallback>,
        ) -> Result<Self, String> {
            Ok(Self)
        }
    }

    impl HotkeyListener for LinuxHotkeyListener {
        fn stop(&mut self) {}
        fn update_hotkey(&mut self, _hotkey_str: &str) -> Result<(), String> {
            todo!()
        }
        fn start_recording(&mut self) {}
        fn finish_recording(&mut self) {}
    }
}

#[cfg(target_os = "windows")]
pub use windows::WindowsHotkeyListener;

#[cfg(target_os = "macos")]
pub use macos::MacosHotkeyListener;

#[cfg(target_os = "linux")]
pub use linux::LinuxHotkeyListener;

pub fn create_hotkey_listener(
    hotkey_str: &str,
    on_pressed: HotkeyCallback,
    on_recording: Option<RecordingCallback>,
) -> Result<Box<dyn HotkeyListener>, String> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(WindowsHotkeyListener::new(
            hotkey_str,
            on_pressed,
            on_recording,
        )?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(MacosHotkeyListener::new(hotkey_str, on_pressed, on_recording)?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(LinuxHotkeyListener::new(hotkey_str, on_pressed, on_recording)?))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        panic!("Unsupported platform")
    }
}
