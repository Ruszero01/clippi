//! Hotkey management - platform-agnostic trait and Windows implementation

use crate::core::i18n_keys::I18nKey;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Main,
    Quick,
    QuickAction(QuickAction),
    /// Per-item custom hotkey activated, carries the item id.
    CustomItem(i64),
    /// Latest-N hotkey activated, carries the slot index (0-9).
    LatestItem(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAction {
    Previous,
    Next,
    PreviousPage,
    NextPage,
    Paste,
    PasteShift, // Shift+Enter → plain text paste
    PasteCtrl,  // Ctrl/Cmd+Enter → advanced paste
    Close,
    Pick(usize),
    PreviousAltMode, // Ctrl/Cmd+↑ → cycle advanced paste mode
    NextAltMode,     // Ctrl/Cmd+↓ → cycle advanced paste mode
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyRecordingPress {
    Hotkey(String),
    Cancel,
}

/// Hotkey listener - platform-agnostic trait (must be used on main thread)
pub trait HotkeyListener {
    fn stop(&mut self);
    fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String>;
    #[allow(dead_code)]
    fn update_quick_hotkey(&mut self, hotkey_str: &str) -> Result<(), String>;
    fn start_recording(&mut self);
    fn finish_recording(&mut self);
    fn is_recording(&self) -> bool;
    fn poll_event(&mut self) -> Option<HotkeyEvent>;
    fn poll_recording_pressed(&mut self) -> Option<HotkeyRecordingPress>;
    fn set_quick_actions_enabled(&mut self, enabled: bool);
    /// Register a per-item custom hotkey. Returns error on conflict.
    fn register_item_hotkey(&mut self, _id: i64, _hotkey_str: &str) -> Result<(), String> {
        Err("unsupported platform".to_string())
    }
    /// Unregister a per-item custom hotkey.
    fn unregister_item_hotkey(&mut self, _id: i64) {}
    /// Register a latest-N slot hotkey. Returns error on conflict.
    fn register_latest_hotkey(&mut self, _slot: usize, _hotkey_str: &str) -> Result<(), String> {
        Err("unsupported platform".to_string())
    }
    /// Unregister a latest-N slot hotkey.
    fn unregister_latest_hotkey(&mut self, _slot: usize) {}
    /// Bulk reload all custom hotkeys from persisted state.
    fn reload_custom_hotkeys(
        &mut self,
        _item_hotkeys: &[(i64, String)],
        _latest_hotkeys: &[(usize, String)],
    ) {
    }
    /// Begin recording for a custom hotkey (does not unregister existing hotkeys).
    fn start_custom_recording(&mut self);
    /// Temporarily unregister the hotkey (for blacklist).
    /// Does nothing if already unregistered.
    fn unregister(&mut self);
    /// Re-register the hotkey after unregister().
    /// Does nothing if already registered.
    fn register(&mut self);
    /// The actual main hotkey string (may differ from config if fallback was
    /// used during construction).
    fn actual_main_hotkey(&self) -> &str {
        ""
    }
    /// The actual quick hotkey string.
    fn actual_quick_hotkey(&self) -> &str {
        ""
    }
    /// Whether the main hotkey fell back to an alternative.
    fn main_fallback_used(&self) -> bool {
        false
    }
    /// Whether the quick hotkey fell back to an alternative.
    fn quick_fallback_used(&self) -> bool {
        false
    }
}

/// Shared keycode mapping: name string → Code variant (platform-agnostic).
fn hotkey_register_error_message() -> String {
    I18nKey::HotkeyConflictCustom.text().to_string()
}

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
        "up" | "arrowup" => Some(Code::ArrowUp),
        "down" | "arrowdown" => Some(Code::ArrowDown),
        "backspace" => Some(Code::Backspace),
        "=" | "equal" => Some(Code::Equal),
        "-" | "minus" => Some(Code::Minus),
        "[" | "bracketleft" => Some(Code::BracketLeft),
        "]" | "bracketright" => Some(Code::BracketRight),
        "'" | "quote" => Some(Code::Quote),
        ";" | "semicolon" => Some(Code::Semicolon),
        "\\" | "backslash" => Some(Code::Backslash),
        "," | "comma" => Some(Code::Comma),
        "." | "period" => Some(Code::Period),
        "/" | "slash" => Some(Code::Slash),
        "`" | "backquote" => Some(Code::Backquote),
        "insert" | "ins" => Some(Code::Insert),
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
        Code::Equal => "=",
        Code::Minus => "-",
        Code::BracketLeft => "[",
        Code::BracketRight => "]",
        Code::Quote => "'",
        Code::Semicolon => ";",
        Code::Backslash => "\\",
        Code::Comma => ",",
        Code::Period => ".",
        Code::Slash => "/",
        Code::Backquote => "`",
        Code::Insert => "Insert",
        _ => "?",
    }
}

fn parse_hotkey(value: &str) -> Result<HotKey, String> {
    let mut modifiers = Modifiers::empty();
    let mut key = None;

    for part in value.trim().to_lowercase().split('+') {
        match part.trim() {
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            "cmd" | "command" | "win" | "super" | "meta" => modifiers |= Modifiers::SUPER,
            name => key = key_name_to_code(name),
        }
    }

    let key = key.ok_or(I18nKey::HotkeyErrNoKey.text())?;
    Ok(HotKey::new(Some(modifiers), key))
}

fn format_pressed_hotkey(modifiers: Modifiers, code: Code) -> String {
    let mut parts = Vec::new();
    if modifiers.contains(Modifiers::SUPER) {
        parts.push(platform_input::super_key_name());
    }
    if modifiers.contains(Modifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if modifiers.contains(Modifiers::ALT) {
        parts.push("Alt");
    }
    if modifiers.contains(Modifiers::SHIFT) {
        parts.push("Shift");
    }
    parts.push(key_code_to_name(code));
    parts.join("+")
}

fn format_recorded_hotkey(modifiers: Modifiers, code: Code) -> Option<HotkeyRecordingPress> {
    if code == Code::Escape {
        return Some(HotkeyRecordingPress::Cancel);
    }
    if matches!(code, Code::Enter | Code::NumpadEnter) || modifiers.is_empty() {
        None
    } else {
        Some(HotkeyRecordingPress::Hotkey(format_pressed_hotkey(
            modifiers, code,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_recorded_hotkey, hotkey_register_error_message, parse_hotkey, HotkeyRecordingPress,
    };
    use crate::core::i18n_keys::I18nKey;
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};

    #[test]
    fn shared_parser_accepts_platform_modifier_aliases() {
        let expected = HotKey::new(
            Some(Modifiers::SUPER | Modifiers::ALT | Modifiers::SHIFT),
            Code::KeyV,
        );

        for value in [
            "Cmd+Option+Shift+V",
            "Win+Alt+Shift+V",
            "Super+Alt+Shift+V",
            "Meta+Option+Shift+V",
        ] {
            assert_eq!(parse_hotkey(value).unwrap().id(), expected.id(), "{value}");
        }
    }

    #[test]
    fn shared_parser_round_trips_symbol_keys() {
        for (name, code) in [
            ("=", Code::Equal),
            ("-", Code::Minus),
            ("[", Code::BracketLeft),
            ("]", Code::BracketRight),
            ("'", Code::Quote),
            (";", Code::Semicolon),
            ("\\", Code::Backslash),
            (",", Code::Comma),
            (".", Code::Period),
            ("/", Code::Slash),
            ("`", Code::Backquote),
        ] {
            let expected = HotKey::new(Some(Modifiers::ALT), code);
            assert_eq!(
                parse_hotkey(&format!("Alt+{name}")).unwrap().id(),
                expected.id(),
                "{name}"
            );
        }
    }

    #[test]
    fn recording_ignores_control_keys() {
        assert_eq!(
            format_recorded_hotkey(Modifiers::empty(), Code::Escape),
            Some(HotkeyRecordingPress::Cancel)
        );
        assert_eq!(
            format_recorded_hotkey(Modifiers::empty(), Code::Enter),
            None
        );
        assert_eq!(
            format_recorded_hotkey(Modifiers::empty(), Code::NumpadEnter),
            None
        );
        assert_eq!(format_recorded_hotkey(Modifiers::empty(), Code::KeyV), None);
        assert_eq!(
            format_recorded_hotkey(Modifiers::ALT, Code::KeyV),
            Some(HotkeyRecordingPress::Hotkey("Alt+V".to_string()))
        );
    }

    #[test]
    fn register_errors_are_user_facing_conflicts() {
        assert_eq!(
            hotkey_register_error_message(),
            I18nKey::HotkeyConflictCustom.text()
        );
        assert!(!hotkey_register_error_message().contains("HotKey"));
    }

    #[test]
    fn unregister_preserves_custom_hotkey_bindings_for_later_reregister() {
        let source = include_str!("hotkey.rs");
        let impl_start = source
            .rfind("impl HotkeyListener for DesktopHotkeyListener")
            .unwrap();
        let unregister_start = source[impl_start..]
            .find("fn unregister(&mut self)")
            .unwrap()
            + impl_start;
        let register_start = source[unregister_start..]
            .find("fn register(&mut self)")
            .unwrap()
            + unregister_start;
        let unregister_body = &source[unregister_start..register_start];

        assert!(!unregister_body.contains(".drain("));
        assert!(unregister_body.contains("for binding in &mut self.item_hotkeys"));
        assert!(unregister_body.contains("for binding in &mut self.latest_hotkeys"));

        let poll_start = source[register_start..]
            .find("fn poll_event(&mut self)")
            .unwrap()
            + register_start;
        let register_body = &source[register_start..poll_start];
        assert!(register_body.contains("if !binding.registered"));
        assert!(register_body.contains("manager.register(binding.hotkey)"));
    }

    #[test]
    fn custom_recording_temporarily_unregisters_hotkeys() {
        let source = include_str!("hotkey.rs");
        let impl_start = source
            .rfind("impl HotkeyListener for DesktopHotkeyListener")
            .unwrap();
        let custom_start = source[impl_start..]
            .find("fn start_custom_recording(&mut self)")
            .unwrap()
            + impl_start;
        let unregister_start = source[custom_start..]
            .find("fn unregister(&mut self)")
            .unwrap()
            + custom_start;
        let custom_body = &source[custom_start..unregister_start];

        assert!(custom_body.contains("self.unregister();"));
        assert!(custom_body.contains("self.is_recording = true;"));
    }
}

/// Fallback hotkey chain for the main window (V for clipboard).
#[cfg(target_os = "macos")]
const MAIN_FALLBACKS: &[&str] = &["Option+Shift+V", "Ctrl+Option+V", "Cmd+Shift+V"];

/// Fallback hotkey chain for the main window (V for clipboard).
#[cfg(not(target_os = "macos"))]
const MAIN_FALLBACKS: &[&str] = &["Alt+Shift+V", "Ctrl+Alt+V", "Win+Shift+V"];

/// Fallback hotkey chain for the quick paste window (C for clipboard).
#[cfg(target_os = "macos")]
const QUICK_FALLBACKS: &[&str] = &["Option+Shift+C", "Ctrl+Option+C", "Cmd+Shift+C"];

/// Fallback hotkey chain for the quick paste window (C for clipboard).
#[cfg(not(target_os = "macos"))]
const QUICK_FALLBACKS: &[&str] = &["Alt+Shift+C", "Ctrl+Alt+C", "Win+Shift+C"];

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Clone, Copy)]
struct ItemHotkeyBinding {
    id: i64,
    hotkey: HotKey,
    registered: bool,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Clone, Copy)]
struct LatestHotkeyBinding {
    slot: usize,
    hotkey: HotKey,
    registered: bool,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
struct DesktopHotkeyListener {
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
    quick_hotkey: HotKey,
    is_recording: bool,
    registered: bool,
    quick_enabled: bool,
    quick_registered: bool,
    quick_action_hotkeys: Vec<(QuickAction, HotKey)>,
    quick_actions_registered: bool,
    /// Actual main hotkey string (may differ from config if fallback used).
    actual_main_hotkey: String,
    /// Actual quick hotkey string (may differ from config if fallback used).
    actual_quick_hotkey: String,
    /// True when the configured main hotkey was unavailable → fallback used.
    main_fallback_used: bool,
    /// True when the configured quick hotkey was unavailable → fallback used.
    quick_fallback_used: bool,
    /// Per-item custom hotkeys.
    item_hotkeys: Vec<ItemHotkeyBinding>,
    /// Latest-N slot hotkeys.
    latest_hotkeys: Vec<LatestHotkeyBinding>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl DesktopHotkeyListener {
    fn new(hotkey_str: &str, quick_hotkey_str: &str) -> Result<Self, String> {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| format!("Failed to create hotkey manager: {e}"))?;

        // ── Main hotkey: try configured → fallback chain ──
        let mut actual_main = hotkey_str.to_string();
        let mut main_fallback = false;
        let mut hotkey = parse_hotkey(hotkey_str)?;
        let mut registered = true;
        if let Err(e) = manager.register(hotkey) {
            log::warn!("main hotkey register failed ({hotkey_str}): {e}");
            registered = false;
            // Walk the fallback chain.
            for &fb in MAIN_FALLBACKS {
                if let Ok(fb_key) = parse_hotkey(fb) {
                    if let Err(e) = manager.register(fb_key) {
                        log::warn!("main fallback register failed ({fb}): {e}");
                    } else {
                        hotkey = fb_key;
                        actual_main = fb.to_string();
                        main_fallback = true;
                        registered = true;
                        break;
                    }
                }
            }
        }
        if !registered {
            return Err(hotkey_register_error_message());
        }

        // ── Quick hotkey: try configured → fallback chain ──
        let mut actual_quick = quick_hotkey_str.to_string();
        let mut quick_fallback = false;
        let mut quick_registered = false;
        let quick_enabled = !quick_hotkey_str.is_empty();
        let mut quick_hotkey;
        if !quick_enabled {
            quick_hotkey = HotKey::new(None, global_hotkey::hotkey::Code::F24);
        } else {
            quick_hotkey = parse_hotkey(quick_hotkey_str)?;
            // Skip if same as the already-registered main hotkey.
            if registered && quick_hotkey.id() == hotkey.id() {
                log::warn!("quick hotkey conflicts with main hotkey, trying fallbacks");
                quick_registered = false;
            } else if let Err(e) = manager.register(quick_hotkey) {
                log::warn!("quick hotkey register failed ({quick_hotkey_str}): {e}");
                quick_registered = false;
            } else {
                quick_registered = true;
                actual_quick = quick_hotkey_str.to_string();
            }
            if !quick_registered {
                for &fb in QUICK_FALLBACKS {
                    if let Ok(fb_key) = parse_hotkey(fb) {
                        // Also skip if it collides with the actual main hotkey.
                        if registered && fb_key.id() == hotkey.id() {
                            continue;
                        }
                        if let Err(e) = manager.register(fb_key) {
                            log::warn!("quick fallback register failed ({fb}): {e}");
                        } else {
                            quick_hotkey = fb_key;
                            actual_quick = fb.to_string();
                            quick_fallback = true;
                            quick_registered = true;
                            break;
                        }
                    }
                }
                if !quick_registered {
                    let _ = manager.unregister(hotkey);
                    return Err(hotkey_register_error_message());
                }
            }
        }

        Ok(Self {
            manager,
            hotkey,
            quick_hotkey,
            is_recording: false,
            registered,
            quick_enabled,
            quick_registered,
            quick_action_hotkeys: quick_action_hotkeys(),
            quick_actions_registered: false,
            actual_main_hotkey: actual_main,
            actual_quick_hotkey: actual_quick,
            main_fallback_used: main_fallback,
            quick_fallback_used: quick_fallback,
            item_hotkeys: Vec::new(),
            latest_hotkeys: Vec::new(),
        })
    }

    fn register_hotkey(&self, hotkey: HotKey) -> Result<(), String> {
        self.manager
            .register(hotkey)
            .map_err(|_| hotkey_register_error_message())
    }

    fn is_conflicting_except(
        &self,
        hotkey: HotKey,
        ignore_item_id: Option<i64>,
        ignore_latest_slot: Option<usize>,
        ignore_main: bool,
        ignore_quick: bool,
    ) -> bool {
        let id = hotkey.id();
        // Check against main hotkey.
        if !ignore_main && self.hotkey.id() == id {
            return true;
        }
        // Check against quick hotkey.
        if !ignore_quick && self.quick_enabled && self.quick_hotkey.id() == id {
            return true;
        }
        // Check against item hotkeys.
        if self
            .item_hotkeys
            .iter()
            .any(|binding| Some(binding.id) != ignore_item_id && binding.hotkey.id() == id)
        {
            return true;
        }
        // Check against latest hotkeys.
        if self
            .latest_hotkeys
            .iter()
            .any(|binding| Some(binding.slot) != ignore_latest_slot && binding.hotkey.id() == id)
        {
            return true;
        }
        false
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn quick_action_hotkeys() -> Vec<(QuickAction, HotKey)> {
    use global_hotkey::hotkey::Code;

    let mut hotkeys = vec![
        (QuickAction::Previous, HotKey::new(None, Code::ArrowUp)),
        (QuickAction::Next, HotKey::new(None, Code::ArrowDown)),
        (
            QuickAction::PreviousPage,
            HotKey::new(None, Code::ArrowLeft),
        ),
        (QuickAction::NextPage, HotKey::new(None, Code::ArrowRight)),
        (QuickAction::Paste, HotKey::new(None, Code::Enter)),
        (
            QuickAction::PasteShift,
            HotKey::new(Some(Modifiers::SHIFT), Code::Enter),
        ),
        (
            QuickAction::PasteCtrl,
            HotKey::new(Some(Modifiers::CONTROL), Code::Enter),
        ),
        (QuickAction::Close, HotKey::new(None, Code::Escape)),
    ];
    for (index, code) in [
        Code::Digit1,
        Code::Digit2,
        Code::Digit3,
        Code::Digit4,
        Code::Digit5,
        Code::Digit6,
        Code::Digit7,
        Code::Digit8,
        Code::Digit9,
    ]
    .into_iter()
    .enumerate()
    {
        hotkeys.push((QuickAction::Pick(index), HotKey::new(None, code)));
    }
    // Advanced modifier + Arrow → cycle advanced paste mode.
    hotkeys.push((
        QuickAction::PreviousAltMode,
        HotKey::new(Some(Modifiers::CONTROL), Code::ArrowUp),
    ));
    hotkeys.push((
        QuickAction::NextAltMode,
        HotKey::new(Some(Modifiers::CONTROL), Code::ArrowDown),
    ));
    #[cfg(target_os = "macos")]
    {
        hotkeys.push((
            QuickAction::PasteCtrl,
            HotKey::new(Some(Modifiers::SUPER), Code::Enter),
        ));
        hotkeys.push((
            QuickAction::PreviousAltMode,
            HotKey::new(Some(Modifiers::SUPER), Code::ArrowUp),
        ));
        hotkeys.push((
            QuickAction::NextAltMode,
            HotKey::new(Some(Modifiers::SUPER), Code::ArrowDown),
        ));
    }
    hotkeys
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl HotkeyListener for DesktopHotkeyListener {
    fn stop(&mut self) {
        self.unregister();
        self.set_quick_actions_enabled(false);
    }

    fn update_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
        let new_hotkey = parse_hotkey(hotkey_str)?;
        // Refuse to shadow the quick-window hotkey.
        if self.is_conflicting_except(new_hotkey, None, None, true, false) {
            return Err(I18nKey::HotkeyConflictCustom.text().to_string());
        }
        // Register the new hotkey *before* dropping the old one so a
        // conflict with another application leaves the current hotkey intact.
        self.manager
            .register(new_hotkey)
            .map_err(|_| hotkey_register_error_message())?;
        if self.registered {
            let _ = self.manager.unregister(self.hotkey);
        }
        self.hotkey = new_hotkey;
        self.registered = true;
        Ok(())
    }

    fn update_quick_hotkey(&mut self, hotkey_str: &str) -> Result<(), String> {
        // Empty string means disable quick hotkey.
        if hotkey_str.is_empty() {
            if self.quick_registered {
                let _ = self.manager.unregister(self.quick_hotkey);
                self.quick_registered = false;
            }
            self.quick_enabled = false;
            return Ok(());
        }

        let new_hotkey = parse_hotkey(hotkey_str)?;
        // Refuse to shadow the main hotkey.
        if self.is_conflicting_except(new_hotkey, None, None, false, true) {
            return Err(I18nKey::HotkeyConflictCustom.text().to_string());
        }
        if self.quick_registered && new_hotkey.id() == self.quick_hotkey.id() {
            self.quick_enabled = true;
            return Ok(());
        }

        // Register first so a conflict does not silently disable the previous
        // working quick hotkey. Only swap state after registration succeeds.
        self.register_hotkey(new_hotkey)?;
        if self.quick_registered {
            let _ = self.manager.unregister(self.quick_hotkey);
        }
        self.quick_hotkey = new_hotkey;
        self.quick_enabled = true;
        self.quick_registered = true;
        Ok(())
    }

    fn actual_main_hotkey(&self) -> &str {
        &self.actual_main_hotkey
    }

    fn actual_quick_hotkey(&self) -> &str {
        &self.actual_quick_hotkey
    }

    fn main_fallback_used(&self) -> bool {
        self.main_fallback_used
    }

    fn quick_fallback_used(&self) -> bool {
        self.quick_fallback_used
    }

    fn start_recording(&mut self) {
        self.is_recording = true;
    }

    fn finish_recording(&mut self) {
        self.is_recording = false;
    }

    fn is_recording(&self) -> bool {
        self.is_recording
    }

    fn start_custom_recording(&mut self) {
        self.unregister();
        self.is_recording = true;
    }

    fn unregister(&mut self) {
        if self.registered {
            let _ = self.manager.unregister(self.hotkey);
            self.registered = false;
        }
        if self.quick_registered {
            let _ = self.manager.unregister(self.quick_hotkey);
            self.quick_registered = false;
        }
        for binding in &mut self.item_hotkeys {
            if binding.registered {
                let _ = self.manager.unregister(binding.hotkey);
                binding.registered = false;
            }
        }
        for binding in &mut self.latest_hotkeys {
            if binding.registered {
                let _ = self.manager.unregister(binding.hotkey);
                binding.registered = false;
            }
        }
    }

    fn register(&mut self) {
        // The blacklist poll calls this periodically. Do not let it restore the
        // active shortcuts while the recorder intentionally has them disabled.
        if self.is_recording {
            return;
        }
        if !self.registered {
            match self.register_hotkey(self.hotkey) {
                Ok(()) => self.registered = true,
                Err(e) => log::error!("hotkey register failed: {e}"),
            }
        }
        if self.quick_enabled && !self.quick_registered {
            match self.register_hotkey(self.quick_hotkey) {
                Ok(()) => self.quick_registered = true,
                Err(e) => log::error!("quick hotkey register failed: {e}"),
            }
        }
        let manager = &self.manager;
        for binding in &mut self.item_hotkeys {
            if !binding.registered {
                match manager.register(binding.hotkey) {
                    Ok(()) => binding.registered = true,
                    Err(e) => log::error!("item hotkey register failed ({}): {e}", binding.id),
                }
            }
        }
        for binding in &mut self.latest_hotkeys {
            if !binding.registered {
                match manager.register(binding.hotkey) {
                    Ok(()) => binding.registered = true,
                    Err(e) => log::error!("latest hotkey register failed ({}): {e}", binding.slot),
                }
            }
        }
    }

    fn poll_event(&mut self) -> Option<HotkeyEvent> {
        if self.is_recording {
            while GlobalHotKeyEvent::receiver().try_recv().is_ok() {}
            return None;
        }
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            // Released and stale/unrecognised events must not stop draining the
            // shared queue; otherwise the following press waits for another poll.
            if event.state() != HotKeyState::Pressed {
                continue;
            }
            let id = event.id();
            if id == self.hotkey.id() {
                return Some(HotkeyEvent::Main);
            }
            if self.quick_enabled && id == self.quick_hotkey.id() {
                return Some(HotkeyEvent::Quick);
            }
            if let Some(action) = self
                .quick_action_hotkeys
                .iter()
                .find_map(|(action, hotkey)| {
                    (id == hotkey.id()).then_some(HotkeyEvent::QuickAction(*action))
                })
            {
                return Some(action);
            }
            // Check per-item custom hotkeys.
            if let Some(binding) = self
                .item_hotkeys
                .iter()
                .find(|binding| binding.registered && binding.hotkey.id() == id)
            {
                return Some(HotkeyEvent::CustomItem(binding.id));
            }
            // Check latest-N slot hotkeys.
            if let Some(binding) = self
                .latest_hotkeys
                .iter()
                .find(|binding| binding.registered && binding.hotkey.id() == id)
            {
                return Some(HotkeyEvent::LatestItem(binding.slot));
            }
        }
        None
    }

    fn poll_recording_pressed(&mut self) -> Option<HotkeyRecordingPress> {
        if !self.is_recording {
            return None;
        }
        let modifiers = platform_input::pressed_modifiers();
        platform_input::pressed_key().and_then(|code| format_recorded_hotkey(modifiers, code))
    }

    fn register_item_hotkey(&mut self, id: i64, hotkey_str: &str) -> Result<(), String> {
        let hk = parse_hotkey(hotkey_str)?;
        if self.is_conflicting_except(hk, Some(id), None, false, false) {
            return Err(I18nKey::HotkeyConflictCustom.text().to_string());
        }
        let existing = self
            .item_hotkeys
            .iter()
            .position(|binding| binding.id == id);
        if existing.is_some_and(|pos| self.item_hotkeys[pos].hotkey.id() == hk.id()) {
            if !self.item_hotkeys[existing.unwrap()].registered {
                self.register_hotkey(hk)?;
                self.item_hotkeys[existing.unwrap()].registered = true;
            }
            return Ok(());
        }
        self.register_hotkey(hk)?;
        if let Some(pos) = existing {
            if self.item_hotkeys[pos].registered {
                let _ = self.manager.unregister(self.item_hotkeys[pos].hotkey);
            }
            self.item_hotkeys[pos] = ItemHotkeyBinding {
                id,
                hotkey: hk,
                registered: true,
            };
        } else {
            self.item_hotkeys.push(ItemHotkeyBinding {
                id,
                hotkey: hk,
                registered: true,
            });
        }
        Ok(())
    }

    fn unregister_item_hotkey(&mut self, id: i64) {
        if let Some(pos) = self
            .item_hotkeys
            .iter()
            .position(|binding| binding.id == id)
        {
            let binding = self.item_hotkeys.remove(pos);
            if binding.registered {
                let _ = self.manager.unregister(binding.hotkey);
            }
        }
    }

    fn register_latest_hotkey(&mut self, slot: usize, hotkey_str: &str) -> Result<(), String> {
        let hk = parse_hotkey(hotkey_str)?;
        if self.is_conflicting_except(hk, None, Some(slot), false, false) {
            return Err(I18nKey::HotkeyConflictCustom.text().to_string());
        }
        let existing = self
            .latest_hotkeys
            .iter()
            .position(|binding| binding.slot == slot);
        if existing.is_some_and(|pos| self.latest_hotkeys[pos].hotkey.id() == hk.id()) {
            if !self.latest_hotkeys[existing.unwrap()].registered {
                self.register_hotkey(hk)?;
                self.latest_hotkeys[existing.unwrap()].registered = true;
            }
            return Ok(());
        }
        self.register_hotkey(hk)?;
        if let Some(pos) = existing {
            if self.latest_hotkeys[pos].registered {
                let _ = self.manager.unregister(self.latest_hotkeys[pos].hotkey);
            }
            self.latest_hotkeys[pos] = LatestHotkeyBinding {
                slot,
                hotkey: hk,
                registered: true,
            };
        } else {
            self.latest_hotkeys.push(LatestHotkeyBinding {
                slot,
                hotkey: hk,
                registered: true,
            });
        }
        Ok(())
    }

    fn unregister_latest_hotkey(&mut self, slot: usize) {
        if let Some(pos) = self
            .latest_hotkeys
            .iter()
            .position(|binding| binding.slot == slot)
        {
            let binding = self.latest_hotkeys.remove(pos);
            if binding.registered {
                let _ = self.manager.unregister(binding.hotkey);
            }
        }
    }

    fn reload_custom_hotkeys(
        &mut self,
        item_hotkeys: &[(i64, String)],
        latest_hotkeys: &[(usize, String)],
    ) {
        // Unregister all existing custom hotkeys.
        for binding in self.item_hotkeys.drain(..) {
            if binding.registered {
                let _ = self.manager.unregister(binding.hotkey);
            }
        }
        for binding in self.latest_hotkeys.drain(..) {
            if binding.registered {
                let _ = self.manager.unregister(binding.hotkey);
            }
        }
        // Re-register from the provided lists.
        for &(id, ref s) in item_hotkeys {
            if let Ok(hk) = parse_hotkey(s) {
                if !self.is_conflicting_except(hk, None, None, false, false)
                    && self.manager.register(hk).is_ok()
                {
                    self.item_hotkeys.push(ItemHotkeyBinding {
                        id,
                        hotkey: hk,
                        registered: true,
                    });
                }
            }
        }
        for &(slot, ref s) in latest_hotkeys {
            if let Ok(hk) = parse_hotkey(s) {
                if !self.is_conflicting_except(hk, None, None, false, false)
                    && self.manager.register(hk).is_ok()
                {
                    self.latest_hotkeys.push(LatestHotkeyBinding {
                        slot,
                        hotkey: hk,
                        registered: true,
                    });
                }
            }
        }
    }

    fn set_quick_actions_enabled(&mut self, enabled: bool) {
        if enabled == self.quick_actions_registered {
            return;
        }

        if enabled {
            for (_, hotkey) in &self.quick_action_hotkeys {
                if let Err(e) = self.manager.register(*hotkey) {
                    log::warn!("quick action hotkey register failed: {e}");
                }
            }
        } else {
            for (_, hotkey) in &self.quick_action_hotkeys {
                let _ = self.manager.unregister(*hotkey);
            }
        }
        self.quick_actions_registered = enabled;
    }
}

#[cfg(target_os = "windows")]
mod platform_input {
    use super::{Code, Modifiers};
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };

    pub fn super_key_name() -> &'static str {
        "Win"
    }

    pub fn pressed_modifiers() -> Modifiers {
        let mut modifiers = Modifiers::empty();
        // SAFETY: `GetAsyncKeyState` is a non-blocking poll of the physical key
        // state; callable from any thread. The key codes (VK_CONTROL, VK_MENU,
        // VK_SHIFT, VK_LWIN, VK_RWIN) are well-known constants on Windows.
        unsafe {
            if GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 {
                modifiers |= Modifiers::CONTROL;
            }
            if GetAsyncKeyState(VK_MENU.0 as i32) < 0 {
                modifiers |= Modifiers::ALT;
            }
            if GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 {
                modifiers |= Modifiers::SHIFT;
            }
            if GetAsyncKeyState(VK_LWIN.0 as i32) < 0 || GetAsyncKeyState(VK_RWIN.0 as i32) < 0 {
                modifiers |= Modifiers::SUPER;
            }
        }
        modifiers
    }

    pub fn pressed_key() -> Option<Code> {
        let key_map: &[(i32, Code)] = &[
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
            (0x2D, Code::Insert),       // VK_INSERT
        ];

        // SAFETY: `GetAsyncKeyState` is a non-blocking poll callable from
        // any thread. All virtual key codes in `key_map` are well-known
        // Windows VK_ constants.
        key_map.iter().find_map(|(virtual_key, code)| unsafe {
            if GetAsyncKeyState(*virtual_key) < 0 {
                Some(*code)
            } else {
                None
            }
        })
    }
}

#[cfg(target_os = "macos")]
mod platform_input {
    use super::{Code, Modifiers};

    const KVK_SHIFT: u16 = 0x38;
    const KVK_CONTROL: u16 = 0x3B;
    const KVK_OPTION: u16 = 0x3A;
    const KVK_COMMAND: u16 = 0x37;

    extern "C" {
        fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    }

    fn is_key_pressed(vk: u16) -> bool {
        // SAFETY: `CGEventSourceKeyState` with stateID 0 (kCGEventSourceStateCombined)
        // queries the hardware key state; it is callable from any thread.
        unsafe { CGEventSourceKeyState(0, vk) }
    }

    pub fn super_key_name() -> &'static str {
        "Cmd"
    }

    pub fn pressed_modifiers() -> Modifiers {
        let mut modifiers = Modifiers::empty();
        if is_key_pressed(KVK_COMMAND) {
            modifiers |= Modifiers::SUPER;
        }
        if is_key_pressed(KVK_CONTROL) {
            modifiers |= Modifiers::CONTROL;
        }
        if is_key_pressed(KVK_OPTION) {
            modifiers |= Modifiers::ALT;
        }
        if is_key_pressed(KVK_SHIFT) {
            modifiers |= Modifiers::SHIFT;
        }
        modifiers
    }

    pub fn pressed_key() -> Option<Code> {
        let key_map: &[(u16, Code)] = &[
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
            (0x24, Code::Enter),
            (0x30, Code::Tab),
            (0x31, Code::Space),
            (0x33, Code::Backspace),
            (0x35, Code::Escape),
            (0x32, Code::Backquote),
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

        key_map
            .iter()
            .find_map(|(virtual_key, code)| is_key_pressed(*virtual_key).then_some(*code))
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
        fn update_quick_hotkey(&mut self, _hotkey_str: &str) -> Result<(), String> {
            Ok(())
        }
        fn start_recording(&mut self) {}
        fn finish_recording(&mut self) {}
        fn is_recording(&self) -> bool {
            false
        }
        fn poll_event(&mut self) -> Option<HotkeyEvent> {
            None
        }
        fn poll_recording_pressed(&mut self) -> Option<HotkeyRecordingPress> {
            None
        }
        fn set_quick_actions_enabled(&mut self, _enabled: bool) {}
        fn start_custom_recording(&mut self) {}
        fn unregister(&mut self) {}
        fn register(&mut self) {}
    }
}

#[cfg(target_os = "linux")]
pub use linux::LinuxHotkeyListener;

pub fn create_hotkey_listener(
    hotkey_str: &str,
    quick_hotkey_str: &str,
) -> Result<Box<dyn HotkeyListener>, String> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        Ok(Box::new(DesktopHotkeyListener::new(
            hotkey_str,
            quick_hotkey_str,
        )?))
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
