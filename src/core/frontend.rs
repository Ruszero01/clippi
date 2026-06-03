//! Frontend management — window position modes and size constants.
//!
//! Framework-agnostic types and helpers used by both Slint (legacy) and
//! GPUI (current) window implementations.

use crate::platform::monitor;

/// Default window size (width, height) in logical pixels.
pub const DEFAULT_WINDOW_WIDTH: f32 = 360.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 480.0;

/// Window minimum/maximum size range in logical pixels.
pub const MIN_WINDOW_WIDTH: f32 = 360.0;
pub const MIN_WINDOW_HEIGHT: f32 = 480.0;
pub const MAX_WINDOW_WIDTH: f32 = 1200.0;
pub const MAX_WINDOW_HEIGHT: f32 = 1200.0;

/// Content panel X offset from the window left edge (logical pixels).
/// Matches the `x: 36px` panel offset in app.slint / root.rs.
pub const PANEL_OFFSET_X: f32 = 36.0;

/// Duration in milliseconds that the auto-hide suppression window lasts
/// after showing or focusing the window. Prevents immediate auto-hide.
pub const SUPPRESS_DURATION_MS: u64 = 600;

/// Window position mode.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PositionMode {
    /// Center on the monitor containing the cursor.
    Center,
    /// Align the content panel with the cursor, offset by PANEL_OFFSET_X.
    FollowMouse,
    /// Restore the last window position; fall back to Center if invalid.
    Remember,
}

impl PositionMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "follow" => Self::FollowMouse,
            "remember" => Self::Remember,
            _ => Self::Center,
        }
    }

    pub fn to_int(self) -> i32 {
        match self {
            Self::Center => 0,
            Self::FollowMouse => 1,
            Self::Remember => 2,
        }
    }

    pub fn from_int(v: i32) -> Self {
        match v {
            1 => Self::FollowMouse,
            2 => Self::Remember,
            _ => Self::Center,
        }
    }
}

/// Clamp a window rectangle to a monitor's work area so the window
/// stays fully visible on screen.
///
/// All parameters are in physical pixels (device pixels) on Windows,
/// logical points on macOS.
pub fn clamp_to_work_area(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    area: &monitor::MonitorRect,
) -> (i32, i32) {
    let max_x = (area.x + area.width - w).max(area.x);
    let max_y = (area.y + area.height - h).max(area.y);
    let x = x.max(area.x).min(max_x);
    let y = y.max(area.y).min(max_y);
    (x, y)
}
