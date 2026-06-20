#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::borrow::Cow;

use gpui::*;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

// --- Core modules are UI-framework independent — keep them ---
mod core;
// --- Platform modules are UI-framework independent — keep them ---
mod platform;
// --- GPUI state layer (new) ---
mod state;
// --- GPUI UI components (new) ---
mod ui;
// --- GPUI polling and services ---
mod services;

// Root view lives in ui::root — use that instead of inline ClippiApp
use core::settings::AppSettings;
use state::app::AppState;
use ui::quick_paste::{calc_quick_window_height, QuickPasteView, QUICK_WINDOW_WIDTH};
use ui::root::RootView;
use ui::window_manager::WindowManager;

fn ensure_single_instance() -> bool {
    std::net::TcpListener::bind("127.0.0.1:19876").is_ok()
}

fn init_logging() {
    let log_path = core::paths::log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 1_000_000 {
            let old = log_path.with_extension("log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&log_path, &old);
        }
    }
    if let Ok(file) = std::fs::File::create(&log_path) {
        let _ = simplelog::WriteLogger::init(
            simplelog::LevelFilter::Info,
            simplelog::Config::default(),
            file,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeferredStartupAction {
    InitializeHotkey,
    /// Release items list + checkpoint + trim working set.
    /// Used when the window starts hidden (silent_start) to keep
    /// memory low without calling hide() (which would be redundant).
    ReleaseMemory,
}

fn deferred_startup_actions(silent_start: bool) -> Vec<DeferredStartupAction> {
    let mut actions = vec![DeferredStartupAction::InitializeHotkey];
    if silent_start {
        // Window is created hidden via WindowOptions { show: false }.
        // release_memory() drops the in-memory items list, checkpoints
        // the WAL, and trims the process working set — same cleanup
        // that hide() normally does, but without the platform show/hide
        // call (the window is already hidden).
        actions.push(DeferredStartupAction::ReleaseMemory);
    }
    actions
}

/// Parse an ICO file from embedded bytes and create an HICON.
///
/// The ICO format structure:
/// - Header (6 bytes): reserved(u16) + type(u16) + count(u16)
/// - Directory entries (16 bytes each): width, height, colors, reserved,
///   planes, bpp, image_size, image_offset
/// - At each image_offset: raw DIB image data that `CreateIconFromResourceEx`
///   accepts directly.
///
/// Picks the largest embedded icon for best visual quality.
#[cfg(target_os = "windows")]
fn load_icon_from_embedded_bytes(ico_data: &[u8]) -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::CreateIconFromResourceEx;

    // Parse ICO header
    if ico_data.len() < 6 {
        return None;
    }
    let _reserved = u16::from_le_bytes([ico_data[0], ico_data[1]]);
    let img_type = u16::from_le_bytes([ico_data[2], ico_data[3]]);
    let count = u16::from_le_bytes([ico_data[4], ico_data[5]]) as usize;
    if img_type != 1 {
        return None;
    } // 1 = ICO

    let dir_start = 6;
    let dir_size = count * 16;
    if ico_data.len() < dir_start + dir_size {
        return None;
    }

    // Find the entry with the largest image data (best quality icon)
    let mut best_offset = 0u32;
    let mut best_size = 0u32;

    for i in 0..count {
        let entry = dir_start + i * 16;
        let img_size = u32::from_le_bytes([
            ico_data[entry + 8],
            ico_data[entry + 9],
            ico_data[entry + 10],
            ico_data[entry + 11],
        ]);
        if img_size > best_size {
            best_size = img_size;
            best_offset = u32::from_le_bytes([
                ico_data[entry + 12],
                ico_data[entry + 13],
                ico_data[entry + 14],
                ico_data[entry + 15],
            ]);
        }
    }

    if best_size == 0 || best_offset == 0 {
        return None;
    }
    let offset = best_offset as usize;
    let size = best_size as usize;
    if offset + size > ico_data.len() {
        return None;
    }

    // CreateIconFromResourceEx expects the raw icon image bits
    // (BITMAPINFOHEADER + pixels + mask) — exactly what's stored
    // at the directory entry's offset in an .ico file.
    let hicon = unsafe {
        CreateIconFromResourceEx(
            ico_data[offset..].as_ptr(),
            size as u32,
            1,          // fIcon = TRUE
            0x00030000, // version (standard for all Windows icons)
            0,          // cxDesired (use size from data)
            0,          // cyDesired (use size from data)
            0,          // flags (LR_DEFAULTCOLOR)
        )
    };

    if hicon.is_null() {
        None
    } else {
        Some(hicon as isize)
    }
}

fn main() {
    if !ensure_single_instance() {
        return;
    }

    // --- Detect portable mode before loading any settings (so config/log paths ---
    // --- are resolved correctly). Must run before init_logging() and ---
    // --- AppSettings::load(). ---
    core::paths::init_portable_mode();
    core::paths::migrate_legacy_files();
    // If running in portable mode for the first time after upgrading from
    // --- a non-portable install, migrate existing data from the system dir. ---
    core::paths::migrate_portable_data();
    init_logging();

    log::info!("Starting Clippi (GPUI experiment)");

    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        // --- iconfont.ttf has an 'm' glyph added (mapped to first icon ---
        // --- glyph) because GPUI's load_family skips any font without ---
        // --- glyph_for_char('m') — icon-only fonts would be silently ---
        // --- dropped and render as tofu (□). The CTFontManager path ---
        // --- registers with the system as a fallback. ---
        #[cfg(target_os = "macos")]
        {
            use objc2::AnyThread;
            use objc2_foundation::NSURL;
            use std::io::Write;

            let font_bytes = include_bytes!("../assets/fonts/iconfont.ttf");
            let tmp_dir = std::env::temp_dir();
            let font_path = tmp_dir.join("clippi_iconfont.ttf");
            if let Ok(mut f) = std::fs::File::create(&font_path) {
                if f.write_all(font_bytes).is_ok() {
                    let path_str = font_path.to_string_lossy();
                    let path_ns = objc2_foundation::NSString::from_str(&path_str);
                    let url = NSURL::initFileURLWithPath(NSURL::alloc(), &path_ns);
                    extern "C" {
                        fn CTFontManagerRegisterFontsForURL(
                            url: *const std::ffi::c_void,
                            scope: u32,
                            error: *mut *const std::ffi::c_void,
                        ) -> bool;
                    }
                    const K_CT_FONT_MANAGER_SCOPE_PROCESS: u32 = 1;
                    // Retained<T>::as_ptr is the safe way to get the inner
                    // pointer without transmute.
                    let url_ptr: *const std::ffi::c_void =
                        objc2::rc::Retained::as_ptr(&url) as *const std::ffi::c_void;
                    unsafe {
                        CTFontManagerRegisterFontsForURL(
                            url_ptr,
                            K_CT_FONT_MANAGER_SCOPE_PROCESS,
                            std::ptr::null_mut(),
                        );
                    }
                }
            }
        }

        // --- CGFont→Handle::from_native preserves PostScript name & ---
        // --- CoreText traits that GPUI's load_family requires. ---
        // --- (Handle::from_memory via Cow::Owned may lose PostScript ---
        // --- name, causing zed-font-kit to silently drop the font.) ---
        if let Err(err) = cx.text_system().add_fonts(vec![Cow::Borrowed(
            include_bytes!("../assets/fonts/iconfont.ttf").as_slice(),
        )]) {
            log::error!("Failed to load iconfont.ttf: {err}");
        }

        let settings = AppSettings::load();
        let effective_language = if settings.language.is_empty() {
            core::settings::detect_system_language()
        } else {
            settings.language.clone()
        };
        core::i18n::set_language(&effective_language);

        // --- Warn if Accessibility permission is missing on macOS — ---
        // --- required for CGEventPost to HID (Cmd+V paste simulation). ---
        #[cfg(target_os = "macos")]
        {
            if !crate::platform::paste::request_accessibility_permission() {
                log::warn!(
                    "Accessibility 权限未授予，已请求 macOS 显示授权提示。请在 系统设置 → 隐私与安全性 → 辅助功能 中启用 Clippi。"
                );
            }
        }

        // Initialize images cache directory — follows db_path if set.
        core::paths::init_images_dir(&settings.db_path);

        // --- Set gpui_component theme based on user settings (not hardcoded Dark). ---
        let is_dark = match settings.theme.as_str() {
            "dark" => true,
            "light" => false,
            _ => core::settings::is_system_dark_mode(),
        };
        let theme_mode = if is_dark {
            gpui_component::ThemeMode::Dark
        } else {
            gpui_component::ThemeMode::Light
        };
        gpui_component::Theme::change(theme_mode, None, cx);
        gpui_component::Theme::global_mut(cx).background = Hsla::transparent_black();

        // Calculate initial position (physical pixels) and size (logical pixels)
        // before the settings are moved into AppState.
        let initial_phys_pos = core::frontend::calculate_initial_position(&settings);
        #[cfg(not(target_os = "windows"))]
        let _ = &initial_phys_pos; // used only in the Windows cfg block below
        let (initial_logical_w, initial_logical_h) =
            core::frontend::effective_window_size(&settings);

        // --- Set window_bounds on all platforms so the window is created at the ---
        // --- correct size from the start — avoids a one-frame flash at default ---
        // --- size before SetWindowPos (Windows) or setFrameTopLeftPoint (macOS) ---
        // --- adjusts the position during show_and_focus. ---
        // --- Origin is centered on the primary monitor as a sensible default; ---
        // --- the platform-specific positioning in show_and_focus will move it ---
        // --- to the correct FollowMouse / Remember position on first show. ---
        // --- When silent_start is enabled, create the window hidden ---
        // --- (show: false, focus: false) to avoid a one-frame flash ---
        // --- of the window outline and taskbar icon before the deferred ---
        // --- hide takes effect.  The window is shown later via ---
        // --- WindowManager::show_and_focus() (hotkey / tray). ---
        let window_options = WindowOptions {
            window_background: WindowBackgroundAppearance::Transparent,
            titlebar: Some(TitlebarOptions {
                title: Some("Clippi".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                Bounds::centered(
                    None,
                    size(px(initial_logical_w), px(initial_logical_h)),
                    cx,
                )
                .origin,
                size(px(initial_logical_w), px(initial_logical_h)),
            ))),
            window_min_size: Some(size(
                px(core::frontend::MIN_WINDOW_WIDTH),
                px(core::frontend::MIN_WINDOW_HEIGHT),
            )),
            show: !settings.silent_start,
            focus: !settings.silent_start,
            ..Default::default()
        };

        cx.open_window(
            window_options,
            |window, cx| {
                #[cfg(target_os = "windows")]
                unsafe {
                    use windows_sys::Win32::Graphics::Dwm::{
                        DwmSetWindowAttribute, DWMWA_NCRENDERING_POLICY,
                    };
                    const DWMNCRP_DISABLED: u32 = 1;
                    if let Ok(handle) = window.window_handle() {
                        if let RawWindowHandle::Win32(wh) = handle.as_raw() {
                            let _ = DwmSetWindowAttribute(
                                wh.hwnd.get() as _,
                                DWMWA_NCRENDERING_POLICY as u32,
                                &DWMNCRP_DISABLED as *const u32 as *const _,
                                std::mem::size_of::<u32>() as u32,
                            );
                        }
                    }
                }
                let silent_start = settings.silent_start;
                let state = cx.new(|_cx| AppState::new(settings));
                let window_manager = cx.new(|cx| WindowManager::new(state.clone(), cx));

                // --- ── Store raw window handle + set initial position/size ── ---
                #[cfg(target_os = "windows")]
                {
                    if let Ok(handle) = window.window_handle() {
                        if let RawWindowHandle::Win32(wh) = handle.as_raw() {
                            let hwnd = wh.hwnd.get();
                            window_manager.update(cx, |wm, _cx| wm.set_hwnd(hwnd));

                            // --- Set initial position and size via platform API. ---
                            // --- calculate_initial_position returns physical pixels ---
                            // (matching SetWindowPos convention on Windows).
                            use windows_sys::Win32::UI::WindowsAndMessaging::{
                                SetWindowPos, HWND_TOP, SWP_NOACTIVATE,
                            };
                            if let Some((x, y)) = initial_phys_pos {
                                let scale = platform::monitor::get_scale_factor(x, y);
                                let phys_w = (initial_logical_w * scale) as i32;
                                let phys_h = (initial_logical_h * scale) as i32;
                                unsafe {
                                    SetWindowPos(
                                        hwnd as _,
                                        HWND_TOP,
                                        x,
                                        y,
                                        phys_w,
                                        phys_h,
                                        SWP_NOACTIVATE,
                                    );
                                }
                            }

                            // --- Set window icon via WM_SETICON using embedded ICO bytes. ---
                            // --- Parses the ICO binary directly to create an HICON — ---
                            // --- no filesystem dependency (unlike LoadImageW + temp file). ---
                            {
                                use windows_sys::Win32::UI::WindowsAndMessaging::{
                                    SendMessageW, ICON_BIG, ICON_SMALL, WM_SETICON,
                                };
                                const LOGO_ICO: &[u8] =
                                    include_bytes!("../assets/LOGO.ico");
                                if let Some(hicon) = load_icon_from_embedded_bytes(LOGO_ICO)
                                {
                                    unsafe {
                                        SendMessageW(
                                            hwnd as _,
                                            WM_SETICON,
                                            ICON_BIG as usize,
                                            hicon,
                                        );
                                        SendMessageW(
                                            hwnd as _,
                                            WM_SETICON,
                                            ICON_SMALL as usize,
                                            hicon,
                                        );
                                    }
                                    log::info!("Window icon set from embedded bytes");
                                } else {
                                    log::warn!("Failed to parse embedded ICO data");
                                }
                            }

                        }
                    }
                }

                #[cfg(target_os = "macos")]
                {
                    if let Ok(handle) = window.window_handle() {
                        if let RawWindowHandle::AppKit(wh) = handle.as_raw() {
                            let ns_view =
                                wh.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
                            let ns_window: *mut objc2_app_kit::NSWindow =
                                unsafe { objc2::msg_send![ns_view, window] };
                            window_manager.update(cx, |wm, _cx| {
                                wm.set_ns_window(ns_window as isize)
                            });
                        }
                    }
                }

                // --- Apply taskbar icon visibility based on settings ---
                {
                    let hide_taskbar = state.read(cx).settings.hide_taskbar_icon;
                    window_manager.update(cx, |wm, cx| {
                        wm.apply_taskbar_visibility(hide_taskbar, cx);
                    });
                }

                // --- Apply system window behaviors blocking based on settings ---
                {
                    let block_behaviors =
                        state.read(cx).settings.block_system_window_behaviors;
                    if block_behaviors {
                        window_manager.update(cx, |wm, cx| {
                            wm.set_block_system_window_behaviors(true, cx);
                        });
                    }
                }

                let view =
                    cx.new(|cx| RootView::new(window, state.clone(), window_manager.clone(), cx));

                let quick_h = {
                    let s = state.read(cx);
                    let has_tag = s
                        .settings
                        .pinned_tag_ids
                        .iter()
                        .any(|&id| s.tags.iter().any(|t| t.id == id));
                    let has_type = !s.settings.type_filter_config.is_empty();
                    calc_quick_window_height(has_tag, has_type)
                };

                let quick_options = WindowOptions {
                    window_background: WindowBackgroundAppearance::Transparent,
                    titlebar: Some(TitlebarOptions {
                        title: Some("Clippi Quick Paste".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Bounds::centered(
                            None,
                            size(px(QUICK_WINDOW_WIDTH), px(quick_h)),
                            cx,
                        )
                        .origin,
                        size(px(QUICK_WINDOW_WIDTH), px(quick_h)),
                    ))),
                    window_min_size: Some(size(px(QUICK_WINDOW_WIDTH), px(quick_h))),
                    show: false,
                    focus: false,
                    kind: WindowKind::PopUp,
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    ..Default::default()
                };
                let quick_state = state.clone();
                let quick_wm = window_manager.clone();
                if let Err(err) = cx.open_window(quick_options, move |quick_window, cx| {
                    let quick_view = cx.new(|_cx| QuickPasteView::new(quick_state.clone()));
                    let quick_handle = gpui::Window::window_handle(quick_window);
                    quick_wm.update(cx, |wm, cx| {
                        wm.set_quick_window(quick_handle, quick_view.clone(), cx);
                    });

                    #[cfg(target_os = "windows")]
                    {
                        if let Ok(handle) =
                            raw_window_handle::HasWindowHandle::window_handle(quick_window)
                        {
                            if let RawWindowHandle::Win32(wh) = handle.as_raw() {
                                quick_wm.update(cx, |wm, _cx| wm.set_quick_hwnd(wh.hwnd.get()));
                            }
                        }
                    }

                    #[cfg(target_os = "macos")]
                    {
                        if let Ok(handle) =
                            raw_window_handle::HasWindowHandle::window_handle(quick_window)
                        {
                            if let RawWindowHandle::AppKit(wh) = handle.as_raw() {
                                let ns_view =
                                    wh.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
                                let ns_window: *mut objc2_app_kit::NSWindow =
                                    unsafe { objc2::msg_send![ns_view, window] };
                                quick_wm.update(cx, |wm, _cx| {
                                    wm.set_quick_ns_window(ns_window as isize)
                                });
                            }
                        }
                    }

                    cx.new(|cx| gpui_component::Root::new(quick_view, quick_window, cx))
                }) {
                    log::error!("Failed to create quick paste window: {err}");
                }

                // Defer startup actions until GPUI has completed the first render.
                // Hotkey registration must happen on every desktop platform after
                // the input pipeline and native window handle are ready.
                for action in deferred_startup_actions(silent_start) {
                    let wm = window_manager.clone();
                    cx.defer(move |cx| match action {
                        DeferredStartupAction::InitializeHotkey => {
                            wm.update(cx, |wm, cx| wm.init_hotkey(cx));
                        }
                        DeferredStartupAction::ReleaseMemory => {
                            wm.update(cx, |wm, cx| wm.release_memory(cx));
                        }
                    });
                }

                // --- Intercept window close — hide to background instead of ---
                // --- destroying the window. Returns false to prevent GPUI ---
                // --- from closing the window and exiting the process. ---
                let wm_close = window_manager.clone();
                window.on_window_should_close(cx, move |_window, cx| {
                    wm_close.update(cx, |wm, cx| wm.hide(cx));
                    false
                });

                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            },
        )
        .unwrap();
    });
}

#[cfg(test)]
mod tests {
    use super::{deferred_startup_actions, DeferredStartupAction};

    #[test]
    fn startup_always_initializes_hotkey_after_window_setup() {
        // When silent_start is false: only InitializeHotkey is deferred.
        assert_eq!(
            deferred_startup_actions(false),
            vec![DeferredStartupAction::InitializeHotkey]
        );
        // When silent_start is true: ReleaseMemory follows InitializeHotkey
        // (the window starts hidden via WindowOptions { show: false }, so
        // we release the in-memory items without a redundant hide() call).
        assert_eq!(
            deferred_startup_actions(true),
            vec![
                DeferredStartupAction::InitializeHotkey,
                DeferredStartupAction::ReleaseMemory,
            ]
        );
    }
}
