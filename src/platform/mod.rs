//! --- Platform adaptation layer ---

pub mod blacklist;
pub mod clipboard;
pub mod focus;
pub mod hotkey;
pub mod monitor;
pub mod paste;
pub mod source;
pub mod text_input;
pub mod tray;
pub mod util;
#[cfg(target_os = "windows")]
pub mod windows_hotkeys;
