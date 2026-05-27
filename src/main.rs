#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

slint::include_modules!();

use crate::core::frontend::{DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH};
use slint::{ComponentHandle, LogicalSize};

mod app;
mod core;
mod looper;
mod platform;
mod services;

/// Returns `true` if this is the first instance and the app should start.
/// Uses a localhost TCP port as a cross-process lock — the OS releases it
/// automatically when the owning process exits (cleanly or via crash).
fn ensure_single_instance() -> bool {
    std::net::TcpListener::bind("127.0.0.1:19876").is_ok()
}

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
    // Prevent multiple instances — desktop clipboard manager must be a singleton.
    if !ensure_single_instance() {
        return;
    }

    // On macOS, disable Slint's default menu bar to avoid muda class name conflict
    // with tray-icon. Both Slint and tray-icon depend on muda (different versions)
    // which register the same ObjC class "MudaMenuItem", causing a crash.
    #[cfg(target_os = "macos")]
    {
        let backend = i_slint_backend_winit::Backend::builder()
            .with_default_menu_bar(false)
            .build()
            .expect("Failed to create Slint backend");
        slint::platform::set_platform(Box::new(backend)).expect("Failed to set Slint platform");
    }

    // Load settings early so we can initialize logging before UI setup
    let mut settings = crate::core::settings::AppSettings::load();
    init_logging(&settings.db_path);

    // Detect system language on first run.
    if settings.language.is_empty() {
        settings.language = crate::core::settings::detect_system_language();
        settings.save();
    }
    crate::core::i18n::set_language(&settings.language);

    let slint_app = App::new().unwrap();

    // select_bundled_translation MUST be called after the first component is created.
    // Always call it: for "en", Slint uses msgid as-is; for "zh_CN", it loads translations.
    slint::select_bundled_translation(&settings.language)
        .unwrap_or_else(|e| eprintln!("Failed to set language: {e}"));

    // Register iconfont after app is initialized
    {
        let font_data = include_bytes!("../assets/fonts/iconfont.ttf");
        let blob = slint::fontique_08::fontique::Blob::new(std::sync::Arc::new(font_data.to_vec()));
        let mut collection = slint::fontique_08::shared_collection();
        let _fonts = collection.register_fonts(blob, None);
    }

    // On macOS, the default SansSerif font (SF Pro) lacks CJK glyphs.
    // Register STHeiti system font under a known family name for CJK rendering.
    #[cfg(target_os = "macos")]
    {
        if let Ok(font_data) = std::fs::read("/System/Library/Fonts/STHeiti Medium.ttc") {
            let blob = slint::fontique_08::fontique::Blob::new(std::sync::Arc::new(font_data));
            let mut collection = slint::fontique_08::shared_collection();
            let cjk_override = slint::fontique_08::fontique::FontInfoOverride {
                family_name: Some("system-cjk"),
                ..Default::default()
            };
            collection.register_fonts(blob, Some(cjk_override));
        }
    }

    let controller = app::AppController::new(&slint_app).expect("Failed to init");
    let restart_flag = controller.restart_flag();

    // Show window first to initialize layout, then apply physical-pixel sizing
    // to prevent DPI-scaling from inflating the window (logical px * scale factor).
    // This runs before the event loop, so no visible flash occurs.
    // When silent_start is enabled, skip showing the window entirely.
    if !slint_app.get_silent_start() {
        #[cfg(target_os = "macos")]
        {
            slint_app.window().show().unwrap();
            slint_app.window().set_size(LogicalSize::new(
                DEFAULT_WINDOW_WIDTH,
                DEFAULT_WINDOW_HEIGHT,
            ));
            let mtm = objc2::MainThreadMarker::new().unwrap();
            let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            ns_app.activate();
        }
        #[cfg(not(target_os = "macos"))]
        {
            slint_app.window().show().unwrap();
            slint_app.window().set_size(LogicalSize::new(
                DEFAULT_WINDOW_WIDTH,
                DEFAULT_WINDOW_HEIGHT,
            ));
        }
    }

    slint::run_event_loop_until_quit().unwrap();

    if restart_flag.load(std::sync::atomic::Ordering::SeqCst) {
        controller.prepare_restart();
        crate::core::settings::spawn_new_process();
    }

    controller.shutdown();
}
