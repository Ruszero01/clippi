//! Platform monitor APIs - cursor position and multi-monitor work areas

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
            core_graphics::event_source::CGEventSourceStateID::HIDSystemState
        ).ok()?
    ).ok()?;
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
    let count = screens.count();
    let mut target_screen = None;
    for i in 0..count {
        let screen = screens.objectAtIndex(i);
        let frame = screen.frame();
        let fx = frame.origin.x as i32;
        let fy = frame.origin.y as i32;
        let fw = frame.size.width as i32;
        let fh = frame.size.height as i32;
        if x >= fx && x < fx + fw && y >= fy && y < fy + fh {
            target_screen = Some(screen);
            break;
        }
    }

    let screen = target_screen?;
    let visible = screen.visibleFrame();
    // macOS coordinate system: Y starts from bottom, need to convert to top-left.
    // Use the matched screen's own frame height (not mainScreen), in case
    // multiple monitors have different resolutions.
    let screen_height = screen.frame().size.height as i32;
    Some(MonitorRect {
        x: visible.origin.x as i32,
        y: screen_height - (visible.origin.y as i32) - (visible.size.height as i32),
        width: visible.size.width as i32,
        height: visible.size.height as i32,
    })
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
    let count = screens.count();
    for i in 0..count {
        let screen = screens.objectAtIndex(i);
        let frame = screen.frame();
        let fx = frame.origin.x as i32;
        let fy = frame.origin.y as i32;
        let fw = frame.size.width as i32;
        let fh = frame.size.height as i32;
        if x >= fx && x < fx + fw && y >= fy && y < fy + fh {
            return true;
        }
    }
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_point_on_monitor(_x: i32, _y: i32) -> bool {
    false
}
