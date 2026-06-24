//! Platform text-input anchor lookup used by FollowMouse positioning.
//!
//! The lookup is intentionally best-effort. When an app does not expose a
//! caret or focused text element, callers should fall back to the cursor.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextInputAnchor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn get_text_input_anchor() -> Option<TextInputAnchor> {
    #[cfg(target_os = "windows")]
    {
        windows_text_input_anchor()
    }
    #[cfg(target_os = "macos")]
    {
        macos_text_input_anchor()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "windows")]
fn windows_text_input_anchor() -> Option<TextInputAnchor> {
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCaretPos, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };

    // Prefer the current foreground window — `get_last_non_clippi_window`
    // is designed for paste-target restoration and may be stale when Clippi
    // was recently in the foreground. Fall back to the stored value only when
    // Clippi itself is the foreground window.
    let fg = unsafe { GetForegroundWindow() };
    let foreground_hwnd: Option<HWND> = (|| {
        if fg.is_null() {
            log::debug!("text_input_anchor: GetForegroundWindow returned null");
            return None;
        }
        let mut pid: u32 = 0;
        // SAFETY: GetWindowThreadProcessId is a read-only query.
        let tid = unsafe { GetWindowThreadProcessId(fg, &mut pid) };
        if tid == 0 || pid == std::process::id() {
            log::debug!(
                "text_input_anchor: foreground belongs to Clippi (pid={}, tid={})",
                pid,
                tid
            );
            return None; // window is invalid or belongs to Clippi
        }
        Some(fg)
    })();
    let target = match foreground_hwnd
        .or_else(crate::platform::focus::get_last_non_clippi_window)
    {
        Some(hwnd) => hwnd,
        None => {
            log::debug!("text_input_anchor: no target window (foreground is Clippi, no last-non-clippi window)");
            return None;
        }
    };

    /// Try to produce a TextInputAnchor from a caret point in client coords
    /// of the given window. Validates size, ClientToScreen, and on-monitor.
    unsafe fn anchor_from_caret(
        caret_hwnd: HWND,
        caret_rect: RECT,
        tid_for_log: u32,
    ) -> Option<TextInputAnchor> {
        let width = (caret_rect.right - caret_rect.left).abs().max(1);
        let height = (caret_rect.bottom - caret_rect.top).abs().max(1);
        if width > 200 || height > 200 {
            log::debug!(
                "text_input_anchor: caret rect too large ({}x{}), likely selected text range",
                width,
                height
            );
            return None;
        }

        let mut point = POINT {
            x: caret_rect.left,
            y: caret_rect.top,
        };
        if ClientToScreen(caret_hwnd, &mut point) == 0 {
            log::debug!("text_input_anchor: ClientToScreen failed");
            return None;
        }
        if !crate::platform::monitor::is_point_on_monitor(point.x, point.y) {
            log::debug!(
                "text_input_anchor: caret screen position ({}, {}) is not on any monitor",
                point.x,
                point.y
            );
            return None;
        }

        log::debug!(
            "text_input_anchor: found caret at screen=({},{}), size=({},{}), tid={}",
            point.x,
            point.y,
            width,
            height,
            tid_for_log
        );
        Some(TextInputAnchor {
            x: point.x,
            y: point.y,
            width,
            height,
        })
    }

    // SAFETY: The HWND comes from `GetForegroundWindow` (or the focus watcher
    // fallback). `GetWindowThreadProcessId`, `GetGUIThreadInfo`, `AttachThreadInput`,
    // `GetCaretPos`, and `ClientToScreen` are read-only queries; all structs are
    // properly zeroed and sized before Win32 calls.
    unsafe {
        let tid = GetWindowThreadProcessId(target, std::ptr::null_mut());
        if tid == 0 {
            log::debug!(
                "text_input_anchor: GetWindowThreadProcessId returned 0 for target HWND"
            );
            return None;
        }

        // ── Path A: GetGUIThreadInfo (reliable for classic Win32 apps) ──
        let mut info: GUITHREADINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        if GetGUIThreadInfo(tid, &mut info) != 0 && !info.hwndCaret.is_null() {
            return anchor_from_caret(info.hwndCaret, info.rcCaret, tid);
        }
        log::debug!(
            "text_input_anchor: GetGUIThreadInfo caret unavailable (hwndCaret={:?}), trying AttachThreadInput fallback",
            if info.hwndCaret.is_null() { "null" } else { "non-null" }
        );

        // ── Path B: AttachThreadInput + GetCaretPos (works for browsers / IME-aware apps) ──
        let our_tid = GetCurrentThreadId();
        if our_tid == tid {
            log::debug!("text_input_anchor: our thread == target thread, skipping AttachThreadInput");
            return None;
        }
        // AttachThreadInput couples the input states of two threads. Call
        // with TRUE to attach, then always call with FALSE to detach.
        if AttachThreadInput(our_tid, tid, 1) == 0 {
            log::debug!(
                "text_input_anchor: AttachThreadInput failed (access denied or threads in different desktops)"
            );
            return None;
        }
        // AttachThreadInput succeeded — MUST detach on all exit paths.
        let result = (|| {
            let focused = GetFocus();
            if focused.is_null() {
                log::debug!("text_input_anchor: GetFocus returned null after AttachThreadInput");
                return None;
            }
            let mut caret_point = POINT { x: 0, y: 0 };
            if GetCaretPos(&mut caret_point) == 0 {
                log::debug!("text_input_anchor: GetCaretPos failed after AttachThreadInput");
                return None;
            }
            // GetCaretPos returns client coords relative to the window that owns
            // the caret (the focused window).
            let caret_rect = RECT {
                left: caret_point.x,
                top: caret_point.y,
                right: caret_point.x + 2, // typical caret width
                bottom: caret_point.y + 20, // typical caret height
            };
            log::debug!(
                "text_input_anchor: AttachThreadInput succeeded, caret at client=({},{})",
                caret_point.x,
                caret_point.y
            );
            anchor_from_caret(focused, caret_rect, tid)
        })();
        AttachThreadInput(our_tid, tid, 0); // detach
        result
    }
}

#[cfg(target_os = "macos")]
fn macos_text_input_anchor() -> Option<TextInputAnchor> {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use std::ffi::c_void;

    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    const AX_VALUE_CG_POINT: i32 = 1;
    const AX_VALUE_CG_SIZE: i32 = 2;
    const AX_VALUE_CG_RECT: i32 = 3;
    const AX_VALUE_CF_RANGE: i32 = 4;
    const AX_ERROR_SUCCESS: i32 = 0;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CFRange {
        location: isize,
        length: isize,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementCopyParameterizedAttributeValue(
            element: AXUIElementRef,
            parameterized_attribute: CFStringRef,
            parameter: CFTypeRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXValueCreate(value_type: i32, value: *const c_void) -> AXValueRef;
        fn AXValueGetValue(value: AXValueRef, value_type: i32, value: *mut c_void) -> bool;
        fn CFRelease(value: CFTypeRef);
    }

    unsafe fn copy_attribute(element: AXUIElementRef, name: &str) -> Option<CFTypeRef> {
        let attr = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
        if err == AX_ERROR_SUCCESS && !value.is_null() {
            Some(value)
        } else {
            None
        }
    }

    unsafe fn ax_value<T: Default + Copy>(value: CFTypeRef, value_type: i32) -> Option<T> {
        let mut out = T::default();
        if AXValueGetValue(
            value as AXValueRef,
            value_type,
            &mut out as *mut T as *mut c_void,
        ) {
            Some(out)
        } else {
            None
        }
    }

    unsafe fn rect_for_selected_range(element: AXUIElementRef) -> Option<CGRect> {
        let range_value = copy_attribute(element, "AXSelectedTextRange")?;
        let range = ax_value::<CFRange>(range_value, AX_VALUE_CF_RANGE);
        CFRelease(range_value);
        let range = range?;

        let range_param =
            AXValueCreate(AX_VALUE_CF_RANGE, &range as *const CFRange as *const c_void);
        if range_param.is_null() {
            return None;
        }

        let parameterized = CFString::new("AXBoundsForRange");
        let mut bounds_value: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyParameterizedAttributeValue(
            element,
            parameterized.as_concrete_TypeRef(),
            range_param as CFTypeRef,
            &mut bounds_value,
        );
        CFRelease(range_param as CFTypeRef);
        if err != AX_ERROR_SUCCESS || bounds_value.is_null() {
            return None;
        }
        let rect = ax_value::<CGRect>(bounds_value, AX_VALUE_CG_RECT);
        CFRelease(bounds_value);
        rect
    }

    unsafe fn rect_for_focused_element(element: AXUIElementRef) -> Option<CGRect> {
        let pos_value = copy_attribute(element, "AXPosition")?;
        let size_value = match copy_attribute(element, "AXSize") {
            Some(value) => value,
            None => {
                CFRelease(pos_value);
                return None;
            }
        };
        let position = ax_value::<CGPoint>(pos_value, AX_VALUE_CG_POINT);
        let size = ax_value::<CGSize>(size_value, AX_VALUE_CG_SIZE);
        CFRelease(pos_value);
        CFRelease(size_value);

        Some(CGRect::new(&position?, &size?))
    }

    fn anchor_from_top_left_rect(rect: CGRect) -> Option<TextInputAnchor> {
        let x = rect.origin.x.round() as i32;
        let y = rect.origin.y.round() as i32;
        let width = rect.size.width.round().max(1.0) as i32;
        let height = rect.size.height.round().max(1.0) as i32;
        if width <= 0 || height <= 0 || width > 2000 || height > 2000 {
            return None;
        }

        if crate::platform::monitor::is_point_on_monitor(x, y) {
            return Some(TextInputAnchor {
                x,
                y,
                width,
                height,
            });
        }

        None
    }

    fn anchor_from_cocoa_rect(rect: CGRect) -> Option<TextInputAnchor> {
        let mtm = objc2::MainThreadMarker::new()?;
        let main_screen_height = objc2_app_kit::NSScreen::mainScreen(mtm)?
            .frame()
            .size
            .height;
        let flipped = crate::platform::monitor::cocoa_rect_to_top_left(
            main_screen_height,
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        );
        if !crate::platform::monitor::is_point_on_monitor(flipped.x, flipped.y) {
            return None;
        }

        Some(TextInputAnchor {
            x: flipped.x,
            y: flipped.y,
            width: flipped.width.max(1),
            height: flipped.height.max(1),
        })
    }

    fn to_anchor(rect: CGRect, prefer_flipped: bool) -> Option<TextInputAnchor> {
        let raw = anchor_from_top_left_rect(rect);
        let flipped = anchor_from_cocoa_rect(rect);
        if prefer_flipped {
            return flipped.or(raw);
        }

        match (raw, flipped) {
            (Some(raw), Some(_flipped)) => Some(raw),
            (Some(raw), None) => Some(raw),
            (None, Some(flipped)) => Some(flipped),
            (None, None) => None,
        }
    }

    fn is_external_app_frontmost() -> bool {
        let Some(_mtm) = objc2::MainThreadMarker::new() else {
            return false;
        };
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let Some(app) = workspace.frontmostApplication() else {
            return false;
        };
        app.processIdentifier() != std::process::id() as i32
    }

    // SAFETY: AX calls are read-only. Returned Core Foundation objects follow
    // create/copy ownership and are released on every successful copy path.
    unsafe {
        if !crate::platform::paste::check_accessibility_permission() {
            return None;
        }
        // Try every external application, including browsers. Apps that do not
        // expose the relevant AX attributes naturally fall back to the cursor.
        if !is_external_app_frontmost() {
            return None;
        }

        let system = AXUIElementCreateSystemWide();
        if system.is_null() {
            return None;
        }

        let focused = match copy_attribute(system, "AXFocusedUIElement") {
            Some(value) => value,
            None => {
                CFRelease(system as CFTypeRef);
                return None;
            }
        };
        let rect = rect_for_selected_range(focused as AXUIElementRef)
            .or_else(|| rect_for_focused_element(focused as AXUIElementRef));
        CFRelease(focused);
        CFRelease(system as CFTypeRef);
        rect.and_then(|rect| to_anchor(rect, false))
    }
}
