//! --- Platform adaptation layer ---

pub mod accent;
pub mod blacklist;
pub mod clipboard;
pub mod focus;
pub mod hotkey;
#[cfg(target_os = "windows")]
pub mod ime;
pub mod keyboard_hook;
pub mod monitor;
pub mod paste;
#[cfg(target_os = "windows")]
pub mod quick_window;
pub mod remote_path;
pub mod source;
pub mod tag_window;
pub mod text_input;
pub mod tray;
pub mod util;
pub mod window_frame;
#[cfg(target_os = "windows")]
pub mod windows_hotkeys;
