//! Platform adaptation layer

pub mod clipboard;
pub mod hotkey;

pub use clipboard::ClipboardListener;
pub use hotkey::{create_hotkey_listener, HotkeyListener};
