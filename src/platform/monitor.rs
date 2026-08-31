//! --- Platform monitor APIs - cursor position and multi-monitor work areas ---
//!
//! Beyond the per-point queries this module also provides a monitor-set /
//! work-area enumeration capability with a deterministic snapshot structure
//! (Windows: `EnumDisplayMonitors` + `GetMonitorInfoW`; macOS: `NSScreen`;
//! other platforms: stub), plus cross-platform pure logic for the issue-75
//! window-migration contracts: the "window visible on any remaining work
//! area" judgment and the deterministic migration-target fallback chain
//! (C3/C4). The pure logic is platform-independent and directly testable.

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
#[cfg(target_os = "windows")]
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Cocoa and CoreGraphics use the primary display as their shared origin.
/// `mainScreen` follows keyboard focus and must not be used for conversion.
#[cfg(target_os = "macos")]
pub(crate) fn primary_screen_height() -> Option<f64> {
    let screens = objc2_app_kit::NSScreen::screens(objc2::MainThreadMarker::new()?);
    screens
        .firstObject()
        .map(|screen| screen.frame().size.height)
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
    // SAFETY: `GetCursorPos` is safe to call from any thread; the POINT struct
    // is stack-allocated and properly initialised via zeroed().
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
    // SAFETY: `MonitorFromPoint` and `GetMonitorInfoW` read system monitor
    // configuration; both are thread-safe. `MONITORINFOEXW` is stack-allocated
    // and its cbSize field is initialised before the call.
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
    let main_screen_height = screens.firstObject()?.frame().size.height;
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
        if x >= frame.x && x < frame.x + frame.width && y >= frame.y && y < frame.y + frame.height {
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
    // SAFETY: `MonitorFromPoint` with `MONITOR_DEFAULTTONULL` is a read-only
    // query safe to call from any thread.
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
    let Some(main_screen) = screens.firstObject() else {
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
        if x >= frame.x && x < frame.x + frame.width && y >= frame.y && y < frame.y + frame.height {
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
/// On Windows, queries per-monitor DPI via `GetDpiForMonitor` so the
/// returned scale matches the monitor the window actually occupies.
/// Falls back to system DPI if the monitor cannot be resolved.
/// On macOS, coordinates are already DPI-independent so returns `1.0`.
/// Other platforms return `1.0`.
///
/// The point (x, y) should be in physical pixels on Windows.
#[cfg(target_os = "windows")]
pub fn get_scale_factor(x: i32, y: i32) -> f32 {
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForSystem, MDT_EFFECTIVE_DPI};

    // SAFETY: All Win32 APIs used here are read-only queries callable from
    // any thread. `GetDpiForMonitor` writes to stack-allocated u32 values.
    unsafe {
        let hmonitor = MonitorFromPoint(
            windows_sys::Win32::Foundation::POINT { x, y },
            MONITOR_DEFAULTTONEAREST,
        );
        if !hmonitor.is_null() {
            let mut dpi_x: u32 = 96;
            let mut dpi_y: u32 = 96;
            if GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) == 0 {
                return dpi_x as f32 / 96.0;
            }
        }
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

// === Issue-75 P1: monitor-set / work-area enumeration and pure migration logic ===
//
// These capabilities and pure functions are consumed by Phase 2
// (`ui/window_manager.rs`) and by the Phase 1 unit-test assignment
// (`p1-justice-pure-tests`); until then they are intentionally unreferenced,
// hence the targeted `#[allow(dead_code)]` attributes below.

/// One enumerated monitor: full bounds, visible work area and primary flag.
///
/// On Windows these map to `rcMonitor` / `rcWork` of `MONITORINFOEXW`; the
/// primary flag comes from `MONITORINFOF_PRIMARY`.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Full monitor bounds in virtual-screen physical pixels.
    pub rect: MonitorRect,
    /// Visible work area in virtual-screen physical pixels.
    pub work_area: MonitorRect,
    /// True when this is the primary monitor.
    pub is_primary: bool,
}

/// Deterministic snapshot of the current monitor topology (issue-75 C1).
///
/// The poll loop compares snapshots across iterations and only reacts when the
/// topology / work-area set actually changed (04-spec §5.1). The order is
/// deterministic (sorted by position), so an unchanged topology always yields
/// an identical snapshot.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSnapshot {
    /// Monitors in deterministic (position-sorted) order.
    pub monitors: Vec<MonitorInfo>,
}

#[allow(dead_code)]
impl MonitorSnapshot {
    /// Work areas of all monitors, in snapshot order.
    pub fn work_areas(&self) -> Vec<MonitorRect> {
        self.monitors.iter().map(|m| m.work_area).collect()
    }

    /// True when the snapshot contains no monitors (extreme topology).
    pub fn is_empty(&self) -> bool {
        self.monitors.is_empty()
    }
}

/// Sort monitors deterministically (by position) and wrap them into a snapshot.
/// OS enumeration order is not guaranteed to be stable across reconnects;
/// normalising it makes snapshot comparison order-independent.
#[allow(dead_code)]
fn normalize_snapshot(mut monitors: Vec<MonitorInfo>) -> MonitorSnapshot {
    monitors.sort_by_key(|m| {
        (
            m.rect.x,
            m.rect.y,
            m.rect.width,
            m.rect.height,
            m.work_area.x,
            m.work_area.y,
            m.work_area.width,
            m.work_area.height,
        )
    });
    MonitorSnapshot { monitors }
}

#[allow(dead_code)]
impl MonitorRect {
    /// Right edge (`x + width`), saturating so degenerate rects stay well-defined.
    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.width)
    }

    /// Bottom edge (`y + height`), saturating.
    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height)
    }

    /// True when the rect has positive size (a usable work area / window bounds).
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// True when `(x, y)` lies inside the half-open range `[x, right) × [y, bottom)`.
    /// Degenerate rects (non-positive size) never contain a point.
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// Strict positive-area rectangle intersection (issue-75 C3).
///
/// Two rects intersect only when they overlap with non-zero area; touching
/// edges (shared boundary, zero overlap) do **not** count as intersection.
/// Degenerate rects (non-positive size) never intersect anything.
#[allow(dead_code)]
pub fn rects_intersect(a: &MonitorRect, b: &MonitorRect) -> bool {
    a.is_valid()
        && b.is_valid()
        && a.x < b.right()
        && b.x < a.right()
        && a.y < b.bottom()
        && b.y < a.bottom()
}

/// True when `window` overlaps (positive area) at least one of the work areas.
#[allow(dead_code)]
pub fn window_visible_on_any(window: &MonitorRect, work_areas: &[MonitorRect]) -> bool {
    work_areas.iter().any(|wa| rects_intersect(window, wa))
}

/// C3 migration decision: the window needs migration when its bounds do not
/// overlap **any** of the remaining monitors' work areas.
///
/// An empty `remaining_work_areas` (no remaining monitor) yields `true`; the
/// migration-target layer then reports "don't migrate / don't position" via
/// `pick_migration_target` returning `None` (04-spec §4).
#[allow(dead_code)]
pub fn window_needs_migration(window: &MonitorRect, remaining_work_areas: &[MonitorRect]) -> bool {
    !window_visible_on_any(window, remaining_work_areas)
}

/// Deterministic migration target from the C3 fallback chain.
///
/// Order (fixed):
/// 1. the work area of the monitor currently containing the cursor, provided
///    that monitor still exists in `remaining` (a cursor on a disconnected
///    monitor fails containment and falls through);
/// 2. the primary monitor's work area;
/// 3. the first remaining monitor's work area (snapshot order).
///
/// Returns `None` when no remaining monitor has a valid work area — the caller
/// must then keep the window where it is (no popup, no crash, no invalid
/// position, 04-spec §4). Any returned work area has positive size, so the
/// result can never be a degenerate `(0,0,0,0)` target (C4).
#[allow(dead_code)]
pub fn pick_migration_target(
    remaining: &[MonitorInfo],
    cursor_pos: Option<(i32, i32)>,
) -> Option<MonitorRect> {
    let valid: Vec<&MonitorInfo> = remaining
        .iter()
        .filter(|m| m.work_area.is_valid())
        .collect();
    if valid.is_empty() {
        return None;
    }

    // 1. Cursor's monitor, if it still exists among the remaining monitors.
    if let Some((cx, cy)) = cursor_pos {
        if let Some(m) = valid.iter().find(|m| m.rect.contains_point(cx, cy)) {
            return Some(m.work_area);
        }
    }

    // 2. Primary monitor.
    if let Some(m) = valid.iter().find(|m| m.is_primary) {
        return Some(m.work_area);
    }

    // 3. Any remaining monitor (deterministic: first in snapshot order).
    Some(valid[0].work_area)
}

/// `MONITORINFOF_PRIMARY` is not exported by windows-sys 0.59; value from winuser.h.
#[cfg(target_os = "windows")]
const MONITORINFOF_PRIMARY: u32 = 1;

/// Enumerate all monitors with their full bounds, work areas and primary flag
/// (Windows: `EnumDisplayMonitors` + `GetMonitorInfoW`). Returns `None` when
/// the enumeration fails; an empty set is reported as `Some(empty snapshot)`.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
pub fn enumerate_monitors() -> Option<MonitorSnapshot> {
    let mut monitors: Vec<MonitorInfo> = Vec::new();
    // SAFETY: `EnumDisplayMonitors` enumerates all monitors synchronously on
    // the calling thread. `monitors` is only borrowed for the duration of the
    // call: the address of the Vec struct (installed as LPARAM) is stable
    // while the callback runs and no other code touches `monitors` until the
    // call returns.
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut::<core::ffi::c_void>(),
            std::ptr::null::<RECT>(),
            Some(collect_monitor_info),
            &mut monitors as *mut Vec<MonitorInfo> as LPARAM,
        )
    };
    if ok == 0 {
        return None;
    }
    Some(normalize_snapshot(monitors))
}

/// `MONITORENUMPROC` callback: query one monitor via `GetMonitorInfoW` and
/// append it to the `Vec<MonitorInfo>` passed through `lparam`.
#[cfg(target_os = "windows")]
unsafe extern "system" fn collect_monitor_info(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _lprc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<MonitorInfo>` address installed by
    // `enumerate_monitors` and stays valid for the whole synchronous
    // `EnumDisplayMonitors` call. `GetMonitorInfoW` writes into the
    // zero-initialised `MONITORINFOEXW` whose `cbSize` is set beforehand.
    let monitors = unsafe { &mut *(lparam as *mut Vec<MonitorInfo>) };
    let mut info: MONITORINFOEXW = std::mem::zeroed();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info.monitorInfo) };
    // A partial snapshot during hot-plug is not a reliable topology change.
    if ok == 0 {
        return 0;
    }
    let mi = info.monitorInfo;
    monitors.push(MonitorInfo {
        rect: MonitorRect {
            x: mi.rcMonitor.left,
            y: mi.rcMonitor.top,
            width: mi.rcMonitor.right - mi.rcMonitor.left,
            height: mi.rcMonitor.bottom - mi.rcMonitor.top,
        },
        work_area: MonitorRect {
            x: mi.rcWork.left,
            y: mi.rcWork.top,
            width: mi.rcWork.right - mi.rcWork.left,
            height: mi.rcWork.bottom - mi.rcWork.top,
        },
        is_primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
    });
    TRUE
}

/// Enumerate all screens with full bounds, visible frames and primary flag
/// (macOS: `NSScreen`). Cross-platform default capability; introduces no
/// Windows-specific behaviour (C6).
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn enumerate_monitors() -> Option<MonitorSnapshot> {
    let mtm = objc2::MainThreadMarker::new()?;
    let screens = objc2_app_kit::NSScreen::screens(mtm);
    let main_screen = screens.firstObject()?;
    let main_frame = main_screen.frame();
    let main_screen_height = main_frame.size.height;
    let count = screens.count();
    let mut monitors = Vec::with_capacity(count);
    for i in 0..count {
        let screen = screens.objectAtIndex(i);
        let frame = screen.frame();
        let is_primary = i == 0;
        let visible = screen.visibleFrame();
        monitors.push(MonitorInfo {
            rect: cocoa_rect_to_top_left(
                main_screen_height,
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            ),
            work_area: cocoa_rect_to_top_left(
                main_screen_height,
                visible.origin.x,
                visible.origin.y,
                visible.size.width,
                visible.size.height,
            ),
            is_primary,
        });
    }
    Some(normalize_snapshot(monitors))
}

/// Stub: Linux and other platforms do not enumerate monitors (matches the
/// existing stub behaviour of the other monitor APIs in this file).
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[allow(dead_code)]
pub fn enumerate_monitors() -> Option<MonitorSnapshot> {
    None
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

// Issue-75 AC2: unit tests for the cross-platform pure migration logic
// (intersection judgment + C3 fallback chain). These are cfg-independent and
// run on every platform the pure logic compiles on (including Windows).
#[cfg(test)]
mod pure_logic_tests {
    use super::{
        normalize_snapshot, pick_migration_target, rects_intersect, window_needs_migration,
        window_visible_on_any, MonitorInfo, MonitorRect, MonitorSnapshot,
    };

    // ---- rects_intersect ----

    #[test]
    fn separated_rects_do_not_intersect() {
        let a = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let b = MonitorRect {
            x: 200,
            y: 0,
            width: 100,
            height: 100,
        };
        assert!(!rects_intersect(&a, &b));
        assert!(!rects_intersect(&b, &a));
    }

    #[test]
    fn vertical_edge_touching_is_not_intersection() {
        let a = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let b = MonitorRect {
            x: 100,
            y: 0,
            width: 100,
            height: 100,
        };
        // a.right() == 100 == b.x: shared boundary with zero overlap area.
        assert!(!rects_intersect(&a, &b));
        assert!(!rects_intersect(&b, &a));
    }

    #[test]
    fn corner_touching_is_not_intersection() {
        let a = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let b = MonitorRect {
            x: 100,
            y: 100,
            width: 50,
            height: 50,
        };
        assert!(!rects_intersect(&a, &b));
    }

    #[test]
    fn contained_rect_intersects() {
        let outer = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let inner = MonitorRect {
            x: 100,
            y: 100,
            width: 50,
            height: 50,
        };
        assert!(rects_intersect(&outer, &inner));
        assert!(rects_intersect(&inner, &outer));
    }

    #[test]
    fn partial_overlap_intersects() {
        let a = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let b = MonitorRect {
            x: 50,
            y: 50,
            width: 100,
            height: 100,
        };
        assert!(rects_intersect(&a, &b));
    }

    #[test]
    fn identical_positive_rects_intersect() {
        let a = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        assert!(rects_intersect(&a, &a));
    }

    #[test]
    fn degenerate_rects_never_intersect() {
        let zero = MonitorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let ok = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        assert!(!rects_intersect(&zero, &ok));
        assert!(!rects_intersect(&ok, &zero));
        assert!(!rects_intersect(&zero, &zero));
        let zero_w = MonitorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 100,
        };
        let zero_h = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 0,
        };
        let neg = MonitorRect {
            x: 0,
            y: 0,
            width: -10,
            height: -10,
        };
        assert!(!rects_intersect(&zero_w, &ok));
        assert!(!rects_intersect(&zero_h, &ok));
        assert!(!rects_intersect(&neg, &ok));
    }

    // ---- window_visible_on_any / window_needs_migration ----

    #[test]
    fn window_visible_when_contained_in_work_area() {
        let window = MonitorRect {
            x: 200,
            y: 200,
            width: 400,
            height: 300,
        };
        let wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(window_visible_on_any(&window, &[wa]));
        assert!(!window_needs_migration(&window, &[wa]));
    }

    #[test]
    fn window_visible_on_partial_overlap() {
        let window = MonitorRect {
            x: 1800,
            y: 900,
            width: 400,
            height: 300,
        };
        let wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(window_visible_on_any(&window, &[wa]));
        assert!(!window_needs_migration(&window, &[wa]));
    }

    #[test]
    fn window_not_visible_when_no_work_area_overlap() {
        let window = MonitorRect {
            x: 5000,
            y: 5000,
            width: 100,
            height: 100,
        };
        let wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(!window_visible_on_any(&window, &[wa]));
        assert!(window_needs_migration(&window, &[wa]));
    }

    #[test]
    fn edge_touching_is_not_visible_and_needs_migration() {
        // Window only touches the work area's right edge → zero overlap area.
        let window = MonitorRect {
            x: 1920,
            y: 0,
            width: 100,
            height: 100,
        };
        let wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(!window_visible_on_any(&window, &[wa]));
        assert!(window_needs_migration(&window, &[wa]));
    }

    #[test]
    fn visible_across_multiple_work_areas() {
        let window = MonitorRect {
            x: -500,
            y: 100,
            width: 100,
            height: 100,
        };
        let primary = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let secondary = MonitorRect {
            x: -1280,
            y: 0,
            width: 1280,
            height: 1024,
        };
        assert!(window_visible_on_any(&window, &[primary, secondary]));
        assert!(!window_needs_migration(&window, &[primary, secondary]));
    }

    #[test]
    fn empty_work_areas_always_needs_migration() {
        let window = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        assert!(!window_visible_on_any(&window, &[]));
        assert!(window_needs_migration(&window, &[]));
    }

    // ---- pick_migration_target fallback chain (C3) ----

    fn monitor(rect: MonitorRect, wa: MonitorRect, primary: bool) -> MonitorInfo {
        MonitorInfo {
            rect,
            work_area: wa,
            is_primary: primary,
        }
    }

    #[test]
    fn cursor_monitor_is_preferred_over_primary() {
        let primary_wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let secondary_wa = MonitorRect {
            x: -1280,
            y: 0,
            width: 1280,
            height: 1024,
        };
        let primary = monitor(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            primary_wa,
            true,
        );
        let secondary = monitor(
            MonitorRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
            secondary_wa,
            false,
        );
        // Cursor on the secondary monitor, which still exists in `remaining`.
        let target = pick_migration_target(&[primary, secondary], Some((-600, 500)))
            .expect("a remaining monitor should be picked");
        assert_eq!(target, secondary_wa);
    }

    #[test]
    fn cursor_on_disconnected_monitor_falls_to_primary() {
        let primary_wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let primary = monitor(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            primary_wa,
            true,
        );
        // Cursor sits where a disconnected monitor used to be: outside every
        // remaining monitor's full rect → falls through to the primary.
        let target = pick_migration_target(&[primary], Some((5000, 5000)))
            .expect("fallback should pick primary");
        assert_eq!(target, primary_wa);
    }

    #[test]
    fn cursor_none_falls_to_primary() {
        let primary_wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let primary = monitor(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            primary_wa,
            true,
        );
        let target = pick_migration_target(&[primary], None).expect("fallback should pick primary");
        assert_eq!(target, primary_wa);
    }

    #[test]
    fn no_primary_falls_to_first_remaining() {
        let a_wa = MonitorRect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };
        let b_wa = MonitorRect {
            x: 800,
            y: 0,
            width: 800,
            height: 600,
        };
        let a = monitor(
            MonitorRect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
            a_wa,
            false,
        );
        let b = monitor(
            MonitorRect {
                x: 800,
                y: 0,
                width: 800,
                height: 600,
            },
            b_wa,
            false,
        );
        // Cursor off every monitor and no primary → first remaining (slice order).
        let target = pick_migration_target(&[a, b], Some((4000, 4000)))
            .expect("first remaining should be picked");
        assert_eq!(target, a_wa);
    }

    #[test]
    fn empty_remaining_returns_none() {
        assert_eq!(pick_migration_target(&[], Some((0, 0))), None);
        assert_eq!(pick_migration_target(&[], None), None);
    }

    #[test]
    fn all_degenerate_work_areas_returns_none() {
        let a = monitor(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            true,
        );
        let b = monitor(
            MonitorRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
            MonitorRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            false,
        );
        assert_eq!(pick_migration_target(&[a, b], Some((-600, 500))), None);
    }

    #[test]
    fn cursor_monitor_with_degenerate_work_area_falls_through() {
        // Cursor is on `secondary` but its work area is degenerate → skipped,
        // so the chain falls through to the primary.
        let primary_wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let primary = monitor(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            primary_wa,
            true,
        );
        let secondary = monitor(
            MonitorRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
            MonitorRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            false,
        );
        let target = pick_migration_target(&[primary, secondary], Some((-600, 500)))
            .expect("should fall through to primary");
        assert_eq!(target, primary_wa);
    }

    #[test]
    fn returned_target_is_never_degenerate() {
        // C4: any returned target must have positive size (width>0 && height>0),
        // so a degenerate `(0,0,0,0)` can never be a final position.
        let primary_wa = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let primary = monitor(
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            primary_wa,
            true,
        );
        let secondary = monitor(
            MonitorRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
            MonitorRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
            false,
        );

        type TargetScenario = (Vec<MonitorInfo>, Option<(i32, i32)>);
        let scenarios: Vec<TargetScenario> = vec![
            (vec![primary], Some((100, 100))),
            (vec![primary], Some((5000, 5000))),
            (vec![primary], None),
            (vec![secondary], Some((-600, 500))),
            (vec![primary, secondary], Some((-600, 500))),
            (vec![primary, secondary], None),
        ];
        for (remaining, cursor) in scenarios {
            if let Some(target) = pick_migration_target(&remaining, cursor) {
                assert!(
                    target.is_valid(),
                    "C4 violation: target {target:?} must have positive size"
                );
                assert!(target.width > 0 && target.height > 0);
            }
        }
    }

    // ---- MonitorRect helper semantics ----

    #[test]
    fn contains_point_uses_half_open_range() {
        let r = MonitorRect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        // Left/top edges are inclusive.
        assert!(r.contains_point(10, 20));
        assert!(r.contains_point(109, 69));
        // Right/bottom edges are exclusive.
        assert!(!r.contains_point(110, 20));
        assert!(!r.contains_point(10, 70));
        assert!(!r.contains_point(109, 70));
    }

    #[test]
    fn degenerate_rect_never_contains_a_point() {
        let zero = MonitorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        assert!(!zero.contains_point(0, 0));
        let zero_w = MonitorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 100,
        };
        assert!(!zero_w.contains_point(0, 50));
        let neg = MonitorRect {
            x: 0,
            y: 0,
            width: -10,
            height: -10,
        };
        assert!(!neg.contains_point(0, 0));
    }

    #[test]
    fn is_valid_reflects_positive_size() {
        assert!(MonitorRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1
        }
        .is_valid());
        assert!(!MonitorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0
        }
        .is_valid());
        assert!(!MonitorRect {
            x: 0,
            y: 0,
            width: 0,
            height: 100
        }
        .is_valid());
        assert!(!MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 0
        }
        .is_valid());
        assert!(!MonitorRect {
            x: 0,
            y: 0,
            width: -10,
            height: -10
        }
        .is_valid());
    }

    #[test]
    fn right_and_bottom_match_rect_extents() {
        let r = MonitorRect {
            x: 5,
            y: 7,
            width: 100,
            height: 50,
        };
        assert_eq!(r.right(), 105);
        assert_eq!(r.bottom(), 57);
    }

    // ---- MonitorSnapshot determinism ----

    #[test]
    fn snapshot_normalization_is_order_independent() {
        let a = MonitorInfo {
            rect: MonitorRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
            work_area: MonitorRect {
                x: -1280,
                y: 0,
                width: 1280,
                height: 1024,
            },
            is_primary: false,
        };
        let b = MonitorInfo {
            rect: MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            work_area: MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
            },
            is_primary: true,
        };
        let ordered = normalize_snapshot(vec![a, b]);
        let reversed = normalize_snapshot(vec![b, a]);
        assert_eq!(ordered, reversed);
        assert_eq!(ordered.work_areas(), reversed.work_areas());
        assert!(!ordered.is_empty());
    }

    #[test]
    fn empty_snapshot_is_empty() {
        let snap: MonitorSnapshot = normalize_snapshot(vec![]);
        assert!(snap.is_empty());
        assert!(snap.work_areas().is_empty());
    }
}
