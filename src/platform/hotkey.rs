//! Hotkey management - platform-agnostic trait and Windows implementation

use std::error::Error;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// Hotkey listener - platform-agnostic trait
pub trait HotkeyListener: Send {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>>;
    fn stop(&mut self);
    fn poll_pressed(&self) -> bool;
    fn current_display(&self) -> String;
    fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String>;
    fn start_recording(&mut self);
    fn finish_recording(&mut self);
    fn poll_recording_pressed(&self) -> Option<String>;
}

enum HotkeyCmd {
    Register(String),
    Unregister,
    Update(String),
    GetDisplay,
    StartRecording,
    StopRecording,
    Shutdown,
}

struct HotkeyThread {
    cmd_tx: Sender<HotkeyCmd>,
    event_rx: Receiver<HotkeyEvent>,
    display_rx: Receiver<String>,
}

enum HotkeyEvent {
    Pressed,
    Recording(Option<String>),
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN,
    };
    use std::time::Instant;

    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    pub struct WindowsHotkeyListener {
        thread: Option<HotkeyThread>,
        registered: Arc<AtomicBool>,
        is_recording: Arc<AtomicBool>,
        last_pressed: Arc<AtomicBool>,
        current_display: Arc<Mutex<String>>,
    }

    impl WindowsHotkeyListener {
        pub fn new(hotkey_str: &str) -> Result<Self, String> {
            let registered = Arc::new(AtomicBool::new(false));
            let is_recording = Arc::new(AtomicBool::new(false));
            let last_pressed = Arc::new(AtomicBool::new(false));
            let current_display = Arc::new(Mutex::new(hotkey_str.to_string()));

            let (cmd_tx, cmd_rx) = channel();
            let (event_tx, event_rx) = channel();
            let (display_tx, display_rx) = channel();

            let registered_clone = registered.clone();
            let current_display_clone = current_display.clone();

            thread::spawn(move || {
                let manager = match GlobalHotKeyManager::new() {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Failed to create hotkey manager: {}", e);
                        return;
                    }
                };

                let mut hotkey: Option<HotKey> = None;

                loop {
                    // Handle commands
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        match cmd {
                            HotkeyCmd::Register(ref hs) => {
                                if let Ok(new_hk) = parse_hotkey(hs) {
                                    // Unregister old
                                    if let Some(ref old) = hotkey {
                                        let _ = manager.unregister(*old);
                                    }
                                    // Register new
                                    if manager.register(new_hk).is_ok() {
                                        hotkey = Some(new_hk);
                                        registered_clone.store(true, Ordering::SeqCst);
                                        *current_display_clone.lock().unwrap() = hs.clone();
                                    }
                                }
                            }
                            HotkeyCmd::Unregister => {
                                if let Some(ref old) = hotkey {
                                    let _ = manager.unregister(*old);
                                }
                                hotkey = None;
                                registered_clone.store(false, Ordering::SeqCst);
                            }
                            HotkeyCmd::Update(ref hs) => {
                                if let Ok(new_hk) = parse_hotkey(hs) {
                                    if let Some(ref old) = hotkey {
                                        let _ = manager.unregister(*old);
                                    }
                                    if manager.register(new_hk).is_ok() {
                                        hotkey = Some(new_hk);
                                        *current_display_clone.lock().unwrap() = hs.clone();
                                    }
                                }
                            }
                            HotkeyCmd::GetDisplay => {
                                let display = current_display_clone.lock().unwrap().clone();
                                let _ = display_tx.send(display);
                            }
                            HotkeyCmd::StartRecording => {}
                            HotkeyCmd::StopRecording => {}
                            HotkeyCmd::Shutdown => return,
                        }
                    }

                    // Check for hotkey events
                    if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                        if event.state() == HotKeyState::Pressed {
                            if let Some(ref hk) = hotkey {
                                if event.id() == hk.id() {
                                    let _ = event_tx.send(HotkeyEvent::Pressed);
                                }
                            }
                        }
                    }

                    thread::sleep(POLL_INTERVAL);
                }
            });

            // Send initial registration
            let _ = cmd_tx.send(HotkeyCmd::Register(hotkey_str.to_string()));

            Ok(Self {
                thread: Some(HotkeyThread {
                    cmd_tx,
                    event_rx,
                    display_rx,
                }),
                registered,
                is_recording,
                last_pressed,
                current_display,
            })
        }

        fn send_cmd(&self, cmd: HotkeyCmd) {
            if let Some(ref t) = self.thread {
                let _ = t.cmd_tx.send(cmd);
            }
        }
    }

    impl HotkeyListener for WindowsHotkeyListener {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            Ok(())
        }

        fn stop(&mut self) {
            self.send_cmd(HotkeyCmd::Unregister);
        }

        fn poll_pressed(&self) -> bool {
            if let Some(ref t) = self.thread {
                while let Ok(event) = t.event_rx.try_recv() {
                    if let HotkeyEvent::Pressed = event {
                        return true;
                    }
                }
            }
            false
        }

        fn current_display(&self) -> String {
            self.send_cmd(HotkeyCmd::GetDisplay);
            if let Some(ref t) = self.thread {
                if let Ok(display) = t.display_rx.recv_timeout(Duration::from_millis(100)) {
                    return display;
                }
            }
            self.current_display.lock().unwrap().clone()
        }

        fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
            self.send_cmd(HotkeyCmd::Update(hotkey_str.to_string()));
            *self.current_display.lock().unwrap() = hotkey_str.to_string();
            Ok(())
        }

        fn start_recording(&mut self) {
            self.is_recording.store(true, Ordering::SeqCst);
            self.send_cmd(HotkeyCmd::StartRecording);
        }

        fn finish_recording(&mut self) {
            self.is_recording.store(false, Ordering::SeqCst);
            self.send_cmd(HotkeyCmd::StopRecording);
        }

        fn poll_recording_pressed(&self) -> Option<String> {
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
            "insert" => Some(Code::Insert),
            "delete" => Some(Code::Delete),
            "home" => Some(Code::Home),
            "end" => Some(Code::End),
            "pageup" => Some(Code::PageUp),
            "pagedown" => Some(Code::PageDown),
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
            Code::Insert => "Insert", Code::Delete => "Delete",
            Code::Home => "Home", Code::End => "End",
            Code::PageUp => "PageUp", Code::PageDown => "PageDown",
            _ => "?",
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub struct MacosHotkeyListener;

    impl MacosHotkeyListener {
        pub fn new(_hotkey_str: &str) -> Result<Self, String> {
            Ok(Self)
        }
    }

    impl HotkeyListener for MacosHotkeyListener {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            todo!("macOS hotkey")
        }
        fn stop(&mut self) {}
        fn poll_pressed(&self) -> bool {
            false
        }
        fn current_display(&self) -> String {
            "".to_string()
        }
        fn update_hotkey(&mut self, _hotkey_str: &str) -> Result<(), String> {
            todo!()
        }
        fn start_recording(&mut self) {}
        fn finish_recording(&mut self) {}
        fn poll_recording_pressed(&self) -> Option<String> {
            None
        }
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
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            todo!("Linux hotkey")
        }
        fn stop(&mut self) {}
        fn poll_pressed(&self) -> bool {
            false
        }
        fn current_display(&self) -> String {
            "".to_string()
        }
        fn update_hotkey(&mut self, _hotkey_str: &str) -> Result<(), String> {
            todo!()
        }
        fn start_recording(&mut self) {}
        fn finish_recording(&mut self) {}
        fn poll_recording_pressed(&self) -> Option<String> {
            None
        }
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