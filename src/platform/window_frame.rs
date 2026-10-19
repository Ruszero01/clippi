//! Native window frame policy for the main window.
//!
//! The panel paints its own rounded stroke and drop shadow, so DWM is kept out of
//! the frame and the client area covers the whole window rectangle. That lets the
//! shadow reach the window edge instead of leaving a transparent ring, while
//! `WS_THICKFRAME` still provides the native resize border.

#[cfg(target_os = "windows")]
#[path = "window_frame_windows.rs"]
mod implementation;

#[cfg(target_os = "windows")]
pub use implementation::install;
