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

/// Compute effective window dimensions from settings.
///
/// Uses saved dimensions when > 0 (with DEFAULT minimum), otherwise returns
/// the DEFAULT dimensions. Returns `(width, height)` in logical pixels.
pub fn effective_window_size(settings: &crate::core::settings::AppSettings) -> (f32, f32) {
    let w = if settings.saved_window_width > 0.0 {
        settings.saved_window_width.max(DEFAULT_WINDOW_WIDTH)
    } else {
        DEFAULT_WINDOW_WIDTH
    };
    let h = if settings.saved_window_height > 0.0 {
        settings.saved_window_height.max(DEFAULT_WINDOW_HEIGHT)
    } else {
        DEFAULT_WINDOW_HEIGHT
    };
    (w, h)
}

/// Calculate the initial window position based on settings.
///
/// Returns `(x, y)` in physical pixels (Windows) or logical points (macOS),
/// or `None` if the monitor layout is unavailable (falls back to a safe default).
pub fn calculate_initial_position(settings: &crate::core::settings::AppSettings) -> Option<(i32, i32)> {
    let mode = PositionMode::from_str(&settings.window_position_mode);
    let (win_w, win_h) = effective_window_size(settings);
    let win_w = win_w as i32;
    let win_h = win_h as i32;

    match mode {
        PositionMode::Center => calc_center(win_w, win_h),
        PositionMode::FollowMouse => calc_follow_mouse(win_w, win_h),
        PositionMode::Remember => calc_remember(settings, win_w, win_h)
            .or_else(|| calc_center(win_w, win_h)),
    }
}

fn calc_center(win_w: i32, win_h: i32) -> Option<(i32, i32)> {
    let (cx, cy) = monitor::get_cursor_pos()?;
    let area = monitor::get_monitor_work_area(cx, cy)?;
    let x = area.x + (area.width - win_w) / 2;
    let y = area.y + (area.height - win_h) / 2;
    Some((x, y))
}

fn calc_follow_mouse(win_w: i32, win_h: i32) -> Option<(i32, i32)> {
    let (cx, cy) = monitor::get_cursor_pos()?;
    let area = monitor::get_monitor_work_area(cx, cy)?;
    Some(clamp_to_work_area(
        cx - PANEL_OFFSET_X as i32,
        cy,
        win_w,
        win_h,
        &area,
    ))
}

fn calc_remember(
    settings: &crate::core::settings::AppSettings,
    win_w: i32,
    win_h: i32,
) -> Option<(i32, i32)> {
    let (sx, sy) = (settings.saved_window_x, settings.saved_window_y);
    if sx < 0 || sy < 0 {
        return None;
    }
    if !monitor::is_point_on_monitor(sx, sy) {
        return None;
    }
    let area = monitor::get_monitor_work_area(sx, sy)?;
    Some(clamp_to_work_area(sx, sy, win_w, win_h, &area))
}
