//! Frontend management - window visibility and UI operations (pure, no platform code)

use crate::App;
use slint::{ComponentHandle, PhysicalPosition};

pub struct Frontend {
    app: slint::Weak<App>,
    visible: bool,
}

impl Frontend {
    pub fn new(app: &App) -> Self {
        Self {
            app: app.as_weak(),
            visible: true,
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        if let Some(app) = self.app.upgrade() {
            app.window().show().ok();
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        if let Some(app) = self.app.upgrade() {
            app.window().hide().ok();
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn move_window(&self, dx: f32, dy: f32) {
        if let Some(app) = self.app.upgrade() {
            let window = app.window();
            let pos = window.position();
            let scale = window.scale_factor();
            window.set_position(PhysicalPosition::new(
                pos.x + (dx * scale) as i32,
                pos.y + (dy * scale) as i32,
            ));
        }
    }
}

