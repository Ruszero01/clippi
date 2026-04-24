slint::include_modules!();

mod app;
mod clipboard;
mod db;
mod history;
mod paste;
mod settings;
mod tray;
mod types;

fn main() {
    let slint_app = App::new().unwrap();
    let tray = std::rc::Rc::new(tray::TrayManager::new());
    let controller = app::AppController::new(&slint_app, tray);
    slint_app.window().show().unwrap();
    slint::run_event_loop_until_quit().unwrap();
    controller.shutdown();
}
