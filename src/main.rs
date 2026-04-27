slint::include_modules!();

mod app;
mod blacklist;
mod clipboard;
mod db;
mod focus;
mod history;
mod hotkey;
mod paste;
mod settings;
mod tray;
mod types;

fn main() {
    let slint_app = App::new().unwrap();

    // Register iconfont after app is initialized
    {
        let font_data = include_bytes!("../assets/fonts/iconfont.ttf");
        let blob = slint::fontique_08::fontique::Blob::new(std::sync::Arc::new(font_data.to_vec()));
        let mut collection = slint::fontique_08::shared_collection();
        let _fonts = collection.register_fonts(blob, None);
    }
    let tray = std::rc::Rc::new(tray::TrayManager::new());
    let controller = app::AppController::new(&slint_app, tray);
    slint_app.window().show().unwrap();
    slint::run_event_loop_until_quit().unwrap();
    controller.shutdown();
}
