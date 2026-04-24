use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

pub struct HotkeyManager {
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
    registered: bool,
}

impl HotkeyManager {
    pub fn new() -> Result<Self, String> {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| format!("Failed to create hotkey manager: {e}"))?;
        let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyV);
        let mut instance = Self {
            manager,
            hotkey,
            registered: false,
        };
        instance.register()?;
        Ok(instance)
    }

    pub fn register(&mut self) -> Result<(), String> {
        if self.registered {
            return Ok(());
        }
        self.manager
            .register(self.hotkey)
            .map_err(|e| format!("Failed to register hotkey: {e}"))?;
        self.registered = true;
        Ok(())
    }

    pub fn unregister(&mut self) -> Result<(), String> {
        if !self.registered {
            return Ok(());
        }
        self.manager
            .unregister(self.hotkey)
            .map_err(|e| format!("Failed to unregister hotkey: {e}"))?;
        self.registered = false;
        Ok(())
    }

    pub fn poll_pressed(&self) -> bool {
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            return event.state() == HotKeyState::Pressed && event.id() == self.hotkey.id();
        }
        false
    }
}
