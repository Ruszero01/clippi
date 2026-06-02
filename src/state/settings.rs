//! Settings state — typed accessors for persisted configuration.
//!
//! Wraps `AppSettings` with GPUI-friendly mutation helpers. Settings are
//! persisted to TOML on every change via `AppSettings::save()`.

use crate::core::settings::AppSettings;

/// Wrapper around `AppSettings` providing convenience accessors
/// and auto-save on mutation.
pub struct SettingsState {
    pub inner: AppSettings,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            inner: AppSettings::load(),
        }
    }

    /// Reload settings from disk.
    pub fn reload(&mut self) {
        self.inner = AppSettings::load();
    }

    /// Persist current settings to disk.
    pub fn save(&self) {
        self.inner.save();
    }

    // ── Convenience accessors ──

    pub fn theme(&self) -> &str {
        &self.inner.theme
    }

    pub fn hotkey(&self) -> &str {
        &self.inner.hotkey
    }

    pub fn auto_hide(&self) -> bool {
        self.inner.auto_hide
    }

    pub fn silent_start(&self) -> bool {
        self.inner.silent_start
    }

    pub fn window_position_mode(&self) -> &str {
        &self.inner.window_position_mode
    }

    pub fn card_height_mode(&self) -> &str {
        &self.inner.card_height_mode
    }

    pub fn language(&self) -> &str {
        &self.inner.language
    }

    pub fn saved_window_width(&self) -> f32 {
        self.inner.saved_window_width
    }

    pub fn saved_window_height(&self) -> f32 {
        self.inner.saved_window_height
    }

    pub fn max_items(&self) -> u32 {
        self.inner.max_items
    }

    // ── Mutation helpers ──

    pub fn set_theme(&mut self, theme: String) {
        self.inner.theme = theme;
        self.save();
    }

    pub fn set_hotkey(&mut self, hotkey: String) {
        self.inner.hotkey = hotkey;
        self.save();
    }

    pub fn set_auto_hide(&mut self, v: bool) {
        self.inner.auto_hide = v;
        self.save();
    }

    pub fn set_window_size(&mut self, width: f32, height: f32) {
        self.inner.saved_window_width = width;
        self.inner.saved_window_height = height;
        self.save();
    }
}
