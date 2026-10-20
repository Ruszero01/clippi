//! The tag window renders the same Sidebar entity owned by RootView.

use gpui::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{sidebar::Sidebar, window_manager::WindowManager};
use crate::platform::tag_window::{window_size, Attachment, ROW_HEIGHT};

fn native_handle(window: &Window) -> Option<isize> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        #[cfg(target_os = "windows")]
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(handle) => {
            let view = handle.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
            let native: *mut objc2_app_kit::NSWindow = unsafe { objc2::msg_send![view, window] };
            Some(native as isize)
        }
        _ => None,
    }
}

pub fn open(
    main: &Window,
    sidebar: Entity<Sidebar>,
    manager: Entity<WindowManager>,
    cx: &mut App,
) -> Result<(), String> {
    let Some(main_native) = native_handle(main) else {
        return Err("main window native handle unavailable".into());
    };
    let options = WindowOptions {
        kind: WindowKind::PopUp,
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(main.bounds().origin, {
            let (width, height) = window_size(ROW_HEIGHT);
            size(px(width), px(height))
        }))),
        show: false,
        focus: false,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_min_size: Some(size(px(1.), px(1.))),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        let side_native = native_handle(window);
        let sidebar_for_setup = sidebar.clone();
        let handle = gpui::Window::window_handle(window);
        cx.spawn(async move |cx| {
            Timer::after(std::time::Duration::from_millis(1)).await;
            let result = side_native
                .ok_or("tag window native handle unavailable")
                .and_then(|side| Attachment::install(main_native, side));
            let _ = cx.update(|cx| match result {
                Ok(attachment) => {
                    sidebar_for_setup.update(cx, |sidebar, cx| {
                        sidebar.attach_window(attachment.clone(), cx);
                    });
                    manager.update(cx, |manager, _| manager.set_sidebar_attachment(attachment));
                }
                Err(error) => {
                    log::error!("Failed to attach tag window: {error}");
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
            });
        })
        .detach();
        cx.new(|cx| gpui_component::Root::new(sidebar, window, cx))
    })
    .map_err(|error| error.to_string())?;
    Ok(())
}
