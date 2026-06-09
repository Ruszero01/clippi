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
        // --- Use Cow::Owned so font_kit takes the memory path ---
        // --- (Handle::from_memory) instead of the CoreGraphics bridge ---
        // --- (Handle::from_native). The native path via CGFont may not ---
        // --- correctly expose the font family name to the font database ---
        // --- on macOS, causing all icon glyphs to render as tofu (□). ---
        if let Err(err) = cx.text_system().add_fonts(vec![Cow::Owned(
            include_bytes!("../assets/fonts/iconfont.ttf").to_vec(),
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
        // before the settings are moved into AppState. We use SetWindowPos on
        // Windows because GPUI's window_bounds with transparent windows is unreliable.
        let initial_phys_pos = core::frontend::calculate_initial_position(&settings);
        let (initial_logical_w, initial_logical_h) =
            core::frontend::effective_window_size(&settings);

        let mut window_options = WindowOptions {
            window_background: WindowBackgroundAppearance::Transparent,
            titlebar: Some(TitlebarOptions {
                appears_transparent: true,
                ..Default::default()
            }),
            window_min_size: Some(size(
                px(core::frontend::MIN_WINDOW_WIDTH),
                px(core::frontend::MIN_WINDOW_HEIGHT),
            )),
            ..Default::default()
        };

        #[cfg(target_os = "macos")]
        {
            let origin = initial_phys_pos
                .map(|(x, y)| point(px(x as f32), px(y as f32)))
                .unwrap_or_else(|| {
                    Bounds::centered(
                        None,
                        size(px(initial_logical_w), px(initial_logical_h)),
                        cx,
                    )
                    .origin
                });
            window_options.window_bounds = Some(WindowBounds::Windowed(Bounds::new(
                origin,
                size(px(initial_logical_w), px(initial_logical_h)),
            )));
        }

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

                let view =
                    cx.new(|cx| RootView::new(window, state.clone(), window_manager.clone(), cx));

                // --- Intercept window close — hide to background instead of ---
                // --- destroying the window. Returns false to prevent GPUI ---
                // --- from closing the window and exiting the process. ---
                let wm_close = window_manager.clone();
                window.on_window_should_close(cx, move |_window, cx| {
                    wm_close.update(cx, |wm, cx| wm.hide(cx));
                    false
                });

                // --- Silent start: defer hide until after window is fully initialized, ---
                // --- so GPUI doesn't override the hidden state with its own show. ---
                if silent_start {
                    let wm_hide = window_manager.clone();
                    cx.defer(move |cx| {
                        wm_hide.update(cx, |wm, cx| wm.hide(cx));
                    });
                }

                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            },
        )
        .unwrap();
    });
}
