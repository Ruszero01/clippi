use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_DEFAULT,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const SUBCLASS: usize = 0x434C_4652;

pub fn install(main: isize) {
    let hwnd = main as HWND;
    // SAFETY: `hwnd` is the live main window handle for the process lifetime.
    unsafe {
        // Let DWM own the frame: rounded corners, the 1px window stroke and the
        // drop shadow. These are the only antialiased rounded edges in the
        // window that are not composited through the app's transparent-surface
        // path, so the corners stay crisp; the panel itself is painted square
        // and gets clipped by the system frame.
        let size = std::mem::size_of::<u32>() as u32;
        let rounded = DWMWCP_ROUND as u32;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &rounded as *const _ as _,
            size,
        );
        // `DWMWA_COLOR_DEFAULT` keeps the system stroke color, which follows the
        // light/dark appearance instead of a hard-coded color.
        let border = DWMWA_COLOR_DEFAULT;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            &border as *const _ as _,
            size,
        );
        // The client area still reaches the window edge so the app owns its
        // layout; the frame procedure keeps native resizing alive at the edges
        // that the client now covers.
        SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS, 0);
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED
                | SWP_NOACTIVATE
                | SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOZORDER
                | SWP_NOOWNERZORDER
                | SWP_NOSENDCHANGING,
        );
    }
}

fn is_resize_hit_test(hit_test: isize) -> bool {
    let hit_test = hit_test as u32;
    matches!(
        hit_test,
        HTLEFT | HTRIGHT | HTTOP | HTTOPLEFT | HTTOPRIGHT | HTBOTTOM | HTBOTTOMLEFT | HTBOTTOMRIGHT
    )
}

/// Width of the resize band measured inwards from the window edge, in logical
/// pixels. It covers the transparent shadow ring plus the panel's stroke, so the
/// window still resizes where a native frame would.
const RESIZE_BAND: f32 = 10.;

/// Resize edge for a point given in window-relative physical pixels.
fn resize_hit_for(width: i32, height: i32, x: i32, y: i32, band: i32) -> Option<u32> {
    if width <= 0 || height <= 0 || x < 0 || y < 0 || x >= width || y >= height {
        return None;
    }
    let band = band.max(1).min(width).min(height);
    let on_left = x < band;
    let on_right = x >= width - band;
    let on_top = y < band;
    let on_bottom = y >= height - band;
    let hit = match (on_left, on_right, on_top, on_bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, false, false, false) => HTLEFT,
        (false, true, false, false) => HTRIGHT,
        (false, false, true, false) => HTTOP,
        (false, false, false, true) => HTBOTTOM,
        _ => return None,
    };
    Some(hit)
}

fn resize_hit_test(hwnd: HWND, lparam: LPARAM) -> Option<LRESULT> {
    // WM_NCHITTEST carries screen coordinates packed into the low/high words.
    let x = (lparam as usize & 0xffff) as u16 as i16 as i32;
    let y = ((lparam as usize >> 16) & 0xffff) as u16 as i16 as i32;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: read-only queries on our own window.
    unsafe {
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }
        let dpi = GetDpiForWindow(hwnd).max(96) as f32;
        let band = (RESIZE_BAND * dpi / 96.).round() as i32;
        resize_hit_for(
            rect.right - rect.left,
            rect.bottom - rect.top,
            x - rect.left,
            y - rect.top,
            band,
        )
        .map(|hit| hit as LRESULT)
    }
}

unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    let zoomed_or_minimized = unsafe { IsZoomed(hwnd) != 0 || IsIconic(hwnd) != 0 };
    if !zoomed_or_minimized {
        // `rgrc[0]` already holds the window rectangle, so returning 0 makes the
        // client cover it and the painted shadow can reach every edge. Maximized
        // windows keep GPUI's work-area inset instead.
        if message == WM_NCCALCSIZE && wparam != 0 {
            return 0;
        }
        // Because the client covers the whole window, the default hit test no
        // longer finds a non-client resize border. Keep GPUI's answer (custom
        // titlebar drag areas, window buttons) and fall back to our own band.
        if message == WM_NCHITTEST {
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            if result == HTCLIENT as LRESULT {
                if let Some(edge) = resize_hit_test(hwnd, lparam) {
                    return edge;
                }
            }
            return result;
        }
        // The panel covers the resize border, so a press there would reach app
        // content and never start a native resize. Forward those presses to the
        // default procedure so the system keeps resize ownership.
        if (message == WM_NCLBUTTONDOWN || message == WM_NCLBUTTONDBLCLK)
            && is_resize_hit_test(wparam as isize)
        {
            return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::{is_resize_hit_test, resize_hit_for};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT, HTMAXBUTTON,
        HTNOWHERE, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
    };

    #[test]
    fn only_edges_and_corners_are_treated_as_resize_hit_tests() {
        for hit_test in [
            HTLEFT,
            HTRIGHT,
            HTTOP,
            HTTOPLEFT,
            HTTOPRIGHT,
            HTBOTTOM,
            HTBOTTOMLEFT,
            HTBOTTOMRIGHT,
        ] {
            assert!(is_resize_hit_test(hit_test as isize));
        }
        for hit_test in [HTNOWHERE, HTCLIENT, HTCAPTION, HTCLOSE, HTMAXBUTTON] {
            assert!(!is_resize_hit_test(hit_test as isize));
        }
    }

    #[test]
    fn resize_band_covers_edges_corners_and_leaves_the_inside_alone() {
        let (width, height, band) = (500, 700, 15);
        // Inside the panel: no resize.
        assert_eq!(resize_hit_for(width, height, 250, 350, band), None);
        assert_eq!(resize_hit_for(width, height, 15, 15, band), None);
        // Edges.
        assert_eq!(resize_hit_for(width, height, 0, 350, band), Some(HTLEFT));
        assert_eq!(resize_hit_for(width, height, 499, 350, band), Some(HTRIGHT));
        assert_eq!(resize_hit_for(width, height, 250, 0, band), Some(HTTOP));
        assert_eq!(
            resize_hit_for(width, height, 250, 699, band),
            Some(HTBOTTOM)
        );
        // Corners win over edges.
        assert_eq!(resize_hit_for(width, height, 0, 0, band), Some(HTTOPLEFT));
        assert_eq!(
            resize_hit_for(width, height, 499, 699, band),
            Some(HTBOTTOMRIGHT)
        );
        // Outside the window rectangle.
        assert_eq!(resize_hit_for(width, height, -1, 350, band), None);
        assert_eq!(resize_hit_for(width, height, 500, 350, band), None);
        // Degenerate windows still resolve without panicking.
        assert!(resize_hit_for(0, 0, 0, 0, band).is_none());
        assert_eq!(resize_hit_for(8, 8, 4, 4, band), Some(HTTOPLEFT));
    }
}
