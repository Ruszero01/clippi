//! Services module — business logic migrated from Slint callbacks.
//!
//! Slint-dependent modules (clipboard, focus, hotkey, sync, tray)
//! are temporarily disabled until they're refactored to use GPUI types.

pub mod backends;
pub mod clipboard_ops;
pub mod gpui_clipboard;
pub mod gpui_sync;
pub mod poll_loop;
// TODO: Refactor these to remove slint dependencies
// pub mod clipboard;
// pub mod focus;
// pub mod hotkey;
// pub mod sync;
// pub mod tray;
pub mod update;
