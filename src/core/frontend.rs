//! --- Frontend management — window position modes and size constants. ---
//!
//! Framework-agnostic types and helpers used by the GPUI window implementation.

use crate::platform::monitor;

/// Default window size (width, height) in logical pixels.
pub const DEFAULT_WINDOW_WIDTH: f32 = 324.0;
pub const DEFAULT_WINDOW_HEIGHT: f32 = 480.0;

/// Window minimum/maximum size range in logical pixels.
pub const MIN_WINDOW_WIDTH: f32 = 324.0;
pub const MIN_WINDOW_HEIGHT: f32 = 480.0;
pub const MAX_WINDOW_WIDTH: f32 = 1164.0;
pub const MAX_WINDOW_HEIGHT: f32 = 1200.0;

/// Content panel X offset from the window left edge (logical pixels).
/// Native tag windows leave no inset inside the main window.
pub const PANEL_OFFSET_X: f32 = if cfg!(any(target_os = "windows", target_os = "macos")) {
    0.0
} else {
    36.0
};

/// Transparent inset around the content panel.
///
/// Windows and macOS let the app fill the whole window and hand the rounded
/// corners and the drop shadow to the window manager, so the panel sits flush
/// with the window and keeps no inset. Linux paints its own frame, but its tag
/// strip is an inline gutter instead of a native window, so it needs none
/// either. The constant stays so platform code keeps one source of truth for
/// where the panel starts.
pub const PANEL_MARGIN: f32 = 0.0;

/// True when the window manager draws the window's rounded corners and drop
/// shadow instead of the app.
///
/// On Windows the DWM frame is the only antialiased rounded edge that is not
/// composited through the transparent-window path, so the panel stays square and
/// skips its own stroke there. macOS lets the `NSWindow` content layer clip the
/// same square panel to a rounded corner with Core Animation's own antialiasing
/// and draws the native window shadow, so it is system framed as well. Linux
/// keeps the app-drawn frame because there is no cross-desktop frame API to lean
/// on.
pub const SYSTEM_DRAWN_FRAME: bool = cfg!(any(target_os = "windows", target_os = "macos"));

/// Corner radius of the content panel and the overlays clipped to it.
pub const PANEL_RADIUS: f32 = 12.0;

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
        settings
            .saved_window_width
            .clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_WIDTH)
    } else {
        DEFAULT_WINDOW_WIDTH
    };
    let h = if settings.saved_window_height > 0.0 {
        settings
            .saved_window_height
            .clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_HEIGHT)
    } else {
        DEFAULT_WINDOW_HEIGHT
    };
    (w, h)
}

pub fn has_saved_window_geometry(settings: &crate::core::settings::AppSettings) -> bool {
    settings.saved_window_width > 0.0 && settings.saved_window_height > 0.0
}

/// Calculate the initial window position based on settings.
///
/// Returns `(x, y)` in **physical pixels** (Windows) or logical points (macOS),
/// suitable for `SetWindowPos` on Windows. Returns `None` if the monitor
/// layout is unavailable.
pub fn calculate_initial_position(
    settings: &crate::core::settings::AppSettings,
) -> Option<(i32, i32)> {
    let mode = PositionMode::from_str(&settings.window_position_mode);
    let (win_w, win_h) = effective_window_size(settings);

    match mode {
        PositionMode::Center => {
            let (cx, cy) = monitor::get_cursor_pos()?;
            let scale = monitor::get_scale_factor(cx, cy);
            calc_center(
                (win_w * scale).round() as i32,
                (win_h * scale).round() as i32,
            )
        }
        PositionMode::FollowMouse => {
            let (cx, cy) = monitor::get_cursor_pos()?;
            let scale = monitor::get_scale_factor(cx, cy);
            calc_follow_mouse(
                (win_w * scale).round() as i32,
                (win_h * scale).round() as i32,
            )
        }
        PositionMode::Remember => {
            let (sx, sy) = (settings.saved_window_x, settings.saved_window_y);
            if has_saved_window_geometry(settings) && monitor::is_point_on_monitor(sx, sy) {
                let scale = monitor::get_scale_factor(sx, sy);
                calc_remember(
                    settings,
                    (win_w * scale).round() as i32,
                    (win_h * scale).round() as i32,
                )
            } else {
                let (cx, cy) = monitor::get_cursor_pos()?;
                let scale = monitor::get_scale_factor(cx, cy);
                calc_center(
                    (win_w * scale).round() as i32,
                    (win_h * scale).round() as i32,
                )
            }
        }
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
    // Scale sidebar offset to physical pixels to match cursor coordinates
    let scale = monitor::get_scale_factor(cx, cy);
    let sidebar_offset = (PANEL_OFFSET_X * scale) as i32;
    Some(clamp_to_work_area(
        cx - sidebar_offset,
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
    if !has_saved_window_geometry(settings) {
        return None;
    }
    // Windows geometry capture stores desktop coordinates in physical pixels.
    if !monitor::is_point_on_monitor(sx, sy) {
        return None;
    }
    let area = monitor::get_monitor_work_area(sx, sy)?;
    Some(clamp_to_work_area(sx, sy, win_w, win_h, &area))
}

#[cfg(test)]
mod tests {
    use super::has_saved_window_geometry;
    use crate::core::settings::AppSettings;

    #[test]
    fn negative_coordinates_are_valid_when_window_size_was_saved() {
        let settings = AppSettings {
            saved_window_x: -1440,
            saved_window_y: 120,
            saved_window_width: 360.0,
            saved_window_height: 480.0,
            ..AppSettings::default()
        };

        assert!(has_saved_window_geometry(&settings));
    }
}
