#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::borrow::Cow;

use gpui::*;

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

// Core modules are UI-framework independent — keep them
mod core;
// Platform modules are UI-framework independent — keep them
mod platform;
// GPUI state layer (new)
mod state;
// GPUI UI components (new)
mod ui;
// GPUI polling and services (migrating from Slint)
mod services;
// Slint-era modules (disabled, being migrated)
// mod app;
// mod looper;

// Root view lives in ui::root — use that instead of inline ClippiApp
use ui::root::RootView;
use ui::window_manager::WindowManager;
use state::app::AppState;
use core::settings::AppSettings;

fn ensure_single_instance() -> bool {
    std::net::TcpListener::bind("127.0.0.1:19876").is_ok()
}

fn init_logging(db_path: &str) {
    let log_path = core::paths::log_path(db_path);
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

    let db_path = core::paths::resolve_db_path("");
    init_logging(&db_path.to_string_lossy());

    log::info!("Starting Clippi (GPUI experiment)");

    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        if let Err(err) = cx.text_system().add_fonts(vec![Cow::Borrowed(
            include_bytes!("../assets/fonts/iconfont.ttf").as_slice(),
        )]) {
            log::error!("Failed to load iconfont.ttf: {err}");
        }
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
        gpui_component::Theme::global_mut(cx).background = Hsla::transparent_black();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.), px(100.)),
                    size(px(360.), px(480.)),
                ))),
                window_background: WindowBackgroundAppearance::Transparent,
                titlebar: Some(TitlebarOptions {
                    title: Some("Clippi".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
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
                let state = cx.new(|_cx| AppState::new(AppSettings::load()));
                let window_manager =
                    cx.new(|cx| WindowManager::new(state.clone(), cx));

                // ── Store raw window handle for platform operations ──
                #[cfg(target_os = "windows")]
                {
                    if let Ok(handle) = window.window_handle() {
                        if let RawWindowHandle::Win32(wh) = handle.as_raw() {
                            let hwnd = wh.hwnd.get() as isize;
                            window_manager.update(cx, |wm, _cx| wm.set_hwnd(hwnd));
                        }
                    }
                }

                let view = cx.new(|cx| {
                    RootView::new(window, state.clone(), window_manager.clone(), cx)
                });

                // Intercept window close — hide to background instead of
                // destroying the window. Returns false to prevent GPUI
                // from closing the window and exiting the process.
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
