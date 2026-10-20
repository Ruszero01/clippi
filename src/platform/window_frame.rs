//! Native window frame policy for the main window.
//!
//! DWM owns the frame: it draws the rounded corners, the 1px window stroke and
//! the drop shadow, so the panel fills the client area as a square and gets
//! clipped by the system frame. The client still covers the whole window
//! rectangle, which keeps the painted content flush with the window edges while
//! `WS_THICKFRAME` keeps the native resize border alive.

#[cfg(target_os = "windows")]
#[path = "window_frame_windows.rs"]
mod implementation;

#[cfg(target_os = "windows")]
pub use implementation::install;
