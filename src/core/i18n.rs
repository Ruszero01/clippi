//! Lightweight i18n helpers for Rust-side user-visible strings.
//! Uses a global atomic flag so any code path can check the current language
//! without threading a settings reference through every call chain.

use std::sync::atomic::{AtomicBool, Ordering};

static IS_ENGLISH: AtomicBool = AtomicBool::new(false);

/// Set the current language. Call once at startup and on every language switch.
pub fn set_language(lang: &str) {
    IS_ENGLISH.store(lang == "en", Ordering::Relaxed);
}

#[inline]
pub fn is_en() -> bool {
    IS_ENGLISH.load(Ordering::Relaxed)
}

/// Simple static translation: `tr("中文", "English")` returns the right string.
#[inline]
pub fn tr<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if is_en() {
        en
    } else {
        zh
    }
}
