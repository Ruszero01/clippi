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

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
pub fn is_point_on_monitor(_x: i32, _y: i32) -> bool {
    false
}
