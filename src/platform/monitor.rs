//! --- Platform monitor APIs - cursor position and multi-monitor work areas ---

#[cfg(target_os = "windows")]
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTONULL,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[cfg(target_os = "macos")]
pub(crate) fn cocoa_rect_to_top_left(
    main_screen_height: f64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> MonitorRect {
    MonitorRect {
        x: x.round() as i32,
        y: (main_screen_height - y - height).round() as i32,
        width: width.round() as i32,
        height: height.round() as i32,
    }
}

#[cfg(target_os = "windows")]
pub fn get_cursor_pos() -> Option<(i32, i32)> {
    unsafe {
        let mut point = std::mem::zeroed();
        if GetCursorPos(&mut point) != 0 {
            Some((point.x, point.y))
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
pub fn get_cursor_pos() -> Option<(i32, i32)> {
    let event = core_graphics::event::CGEvent::new(
        core_graphics::event_source::CGEventSource::new(
            core_graphics::event_source::CGEventSourceStateID::HIDSystemState,
        )
        .ok()?,
    )
    .ok()?;
    let loc = event.location();
    Some((loc.x as i32, loc.y as i32))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn get_cursor_pos() -> Option<(i32, i32)> {
    None
}

#[cfg(target_os = "windows")]
pub fn get_monitor_work_area(x: i32, y: i32) -> Option<MonitorRect> {
    unsafe {
        let hmonitor = MonitorFromPoint(
            windows_sys::Win32::Foundation::POINT { x, y },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmonitor, &mut info.monitorInfo) != 0 {
            let rc = info.monitorInfo.rcWork;
            Some(MonitorRect {
                x: rc.left,
                y: rc.top,
                width: rc.right - rc.left,
                height: rc.bottom - rc.top,
            })
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
pub fn get_monitor_work_area(x: i32, y: i32) -> Option<MonitorRect> {
    let mtm = objc2::MainThreadMarker::new()?;

    let screens = objc2_app_kit::NSScreen::screens(mtm);
    let main_screen_height = objc2_app_kit::NSScreen::mainScreen(mtm)?
        .frame()
        .size
        .height;
    let count = screens.count();
    let mut target_screen = None;
    for i in 0..count {
        let screen = screens.objectAtIndex(i);
        let frame = screen.frame();
        let frame = cocoa_rect_to_top_left(
            main_screen_height,
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        );
        if x >= frame.x
            && x < frame.x + frame.width
            && y >= frame.y
            && y < frame.y + frame.height
        {
            target_screen = Some(screen);
            break;
        }
    }

    let screen = target_screen?;
    let visible = screen.visibleFrame();
    Some(cocoa_rect_to_top_left(
        main_screen_height,
        visible.origin.x,
        visible.origin.y,
        visible.size.width,
        visible.size.height,
    ))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn get_monitor_work_area(_x: i32, _y: i32) -> Option<MonitorRect> {
    None
}

#[cfg(target_os = "windows")]
pub fn is_point_on_monitor(x: i32, y: i32) -> bool {
    unsafe {
        let hmonitor = MonitorFromPoint(
            windows_sys::Win32::Foundation::POINT { x, y },
            MONITOR_DEFAULTTONULL,
        );
        !hmonitor.is_null()
    }
}

#[cfg(target_os = "macos")]
pub fn is_point_on_monitor(x: i32, y: i32) -> bool {
    let mtm = match objc2::MainThreadMarker::new() {
        Some(m) => m,
        None => return false,
    };

    let screens = objc2_app_kit::NSScreen::screens(mtm);
    let Some(main_screen) = objc2_app_kit::NSScreen::mainScreen(mtm) else {
        return false;
    };
    let main_screen_height = main_screen.frame().size.height;
    let count = screens.count();
    for i in 0..count {
        let screen = screens.objectAtIndex(i);
        let frame = screen.frame();
        let frame = cocoa_rect_to_top_left(
            main_screen_height,
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        );
        if x >= frame.x
            && x < frame.x + frame.width
            && y >= frame.y
            && y < frame.y + frame.height
        {
            return true;
        }
    }
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_point_on_monitor(_x: i32, _y: i32) -> bool {
    false
}

/// Get the DPI scale factor for the monitor containing point (x, y).
///
/// On Windows, returns `system_dpi / 96.0`. On macOS, coordinates are
/// already DPI-independent so returns `1.0`. Other platforms return `1.0`.
///
/// The point (x, y) should be in physical pixels on Windows (used only to
/// identify the target monitor; falls back to system DPI if unavailable).
#[cfg(target_os = "windows")]
pub fn get_scale_factor(_x: i32, _y: i32) -> f32 {
    use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
    unsafe {
        let dpi = GetDpiForSystem();
        dpi as f32 / 96.0
    }
}

#[cfg(target_os = "macos")]
pub fn get_scale_factor(_x: i32, _y: i32) -> f32 {
    // macOS CoreGraphics coordinates are already DPI-independent (logical points).
    1.0
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn get_scale_factor(_x: i32, _y: i32) -> f32 {
    1.0
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::cocoa_rect_to_top_left;

    #[test]
    fn converts_cocoa_rects_using_main_screen_coordinate_space() {
        let rect = cocoa_rect_to_top_left(900.0, 1440.0, 180.0, 1920.0, 1080.0);

        assert_eq!(rect.x, 1440);
        assert_eq!(rect.y, -360);
        assert_eq!(rect.width, 1920);
        assert_eq!(rect.height, 1080);
    }
}
