#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

slint::include_modules!();

mod app;
mod core;
mod looper;
mod platform;
mod services;

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

    let slint_app = App::new().unwrap();

    // Register iconfont after app is initialized
    {
        let font_data = include_bytes!("../assets/fonts/iconfont.ttf");
        let blob = slint::fontique_08::fontique::Blob::new(std::sync::Arc::new(font_data.to_vec()));
        let mut collection = slint::fontique_08::shared_collection();
        let _fonts = collection.register_fonts(blob, None);
    }

    let controller = app::AppController::new(&slint_app).expect("Failed to init");

    // Show and activate the window unless silent start is enabled
    if !slint_app.get_silent_start() {
        #[cfg(target_os = "macos")]
        {
            slint_app.window().show().unwrap();
            let mtm = unsafe { objc2::MainThreadMarker::new_unchecked() };
            let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            ns_app.activate();
        }
        #[cfg(not(target_os = "macos"))]
        {
            slint_app.window().show().unwrap();
        }
    }

    slint::run_event_loop_until_quit().unwrap();
    controller.shutdown();
}
