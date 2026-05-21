#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

slint::include_modules!();

use crate::core::frontend::{DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT};
use slint::{ComponentHandle, LogicalSize};

mod app;
mod core;
mod looper;
mod platform;
mod services;

fn init_logging(db_path: &str) {
    let log_path = crate::core::paths::log_path(db_path);
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Rotate: if log > 1 MB, rename to .old before starting fresh
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
    // On macOS, disable Slint's default menu bar to avoid muda class name conflict
    // with tray-icon. Both Slint and tray-icon depend on muda (different versions)
    // which register the same ObjC class "MudaMenuItem", causing a crash.
    #[cfg(target_os = "macos")]
    {
        let backend = i_slint_backend_winit::Backend::builder()
            .with_default_menu_bar(false)
            .build()
            .expect("Failed to create Slint backend");
        slint::platform::set_platform(Box::new(backend))
            .expect("Failed to set Slint platform");
    }

    // Load settings early so we can initialize logging before UI setup
    let settings = crate::core::settings::AppSettings::load();
    init_logging(&settings.db_path);

    let slint_app = App::new().unwrap();

    // Register iconfont after app is initialized
    {
        let font_data = include_bytes!("../assets/fonts/iconfont.ttf");
        let blob = slint::fontique_08::fontique::Blob::new(std::sync::Arc::new(font_data.to_vec()));
        let mut collection = slint::fontique_08::shared_collection();
        let _fonts = collection.register_fonts(blob, None);
    }

    let controller = app::AppController::new(&slint_app).expect("Failed to init");
    let restart_flag = controller.restart_flag();

    // Show window first to initialize layout, then apply physical-pixel sizing
    // to prevent DPI-scaling from inflating the window (logical px * scale factor).
    // This runs before the event loop, so no visible flash occurs.
    // When silent_start is enabled, skip showing the window entirely — avoids
    // creating GPU textures just to immediately hide the window.
    if !slint_app.get_silent_start() {
        #[cfg(target_os = "macos")]
        {
            slint_app.window().show().unwrap();
            slint_app.window().set_size(LogicalSize::new(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT));
            let mtm = unsafe { objc2::MainThreadMarker::new_unchecked() };
            let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            ns_app.activate();
        }
        #[cfg(not(target_os = "macos"))]
        {
            slint_app.window().show().unwrap();
            slint_app.window().set_size(LogicalSize::new(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT));
        }
    }

    slint::run_event_loop_until_quit().unwrap();

    if restart_flag.load(std::sync::atomic::Ordering::SeqCst) {
        controller.prepare_restart();
        crate::core::settings::spawn_new_process();
    }

    controller.shutdown();
}
