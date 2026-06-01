#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::*;

// Core modules are UI-framework independent — keep them
mod core;
// Platform modules are UI-framework independent — keep them
mod platform;
// TODO: Migrate these to GPUI
// mod app;
// mod looper;
// mod services;

struct ClippiApp;

impl Render for ClippiApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgba(0x232425ff))
            .flex()
            .flex_col()
            .justify_center()
            .items_center()
            .child(
                div()
                    .rounded(px(12.))
                    .bg(rgb(0x7ecba3))
                    .px(px(24.))
                    .py(px(12.))
                    .shadow_md()
                    .child("Clippi — GPUI Experiment"),
            )
    }
}

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
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.), px(100.)),
                    size(px(360.), px(480.)),
                ))),
                window_background: WindowBackgroundAppearance::Transparent,
                window_decorations: None,
                ..Default::default()
            },
            |_window, cx| cx.new(|_cx| ClippiApp),
        )
        .unwrap();
    });
}
