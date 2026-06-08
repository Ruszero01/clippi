//! Compile-time-safe i18n engine.
//!
//! Uses a global atomic flag so any code path can check the current language
//! without threading a settings reference through every call chain.
//!
//! Translation keys are defined in `i18n_keys.rs` via the `define_i18n!` macro.

use std::sync::atomic::{AtomicBool, Ordering};

static IS_ENGLISH: AtomicBool = AtomicBool::new(false);

/// Set the current language. Call once at startup and on every language switch.
pub fn set_language(lang: &str) {
    IS_ENGLISH.store(lang == "en", Ordering::Relaxed);
}

/// Check if the current language is English.
#[inline]
pub fn is_en() -> bool {
    IS_ENGLISH.load(Ordering::Relaxed)
}

/// Get the current language code.
#[inline]
pub fn current_language() -> &'static str {
    if is_en() {
        "en"
    } else {
        "zh_CN"
    }
}

/// Defines the `I18nKey` enum and its `text()` / `fmt()` methods.
///
/// Usage in `i18n_keys.rs`:
/// ```ignore
/// define_i18n! {
///     key_name: ("中文", "English"),
/// }
/// ```
///
/// Then use: `I18nKey::KeyName.text()`
#[macro_export]
macro_rules! define_i18n {
    ($($key:ident: ($zh:literal, $en:literal)),* $(,)?) => {
        /// Every user-visible string key — compile-time safe.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum I18nKey {
            $($key,)*
        }

        impl I18nKey {
            /// Look up the translation for the current language.
            /// Returns `&'static str` — zero allocation.
            pub fn text(self) -> &'static str {
                if $crate::core::i18n::is_en() {
                    match self {
                        $(I18nKey::$key => $en,)*
                    }
                } else {
                    match self {
                        $(I18nKey::$key => $zh,)*
                    }
                }
            }

            /// Format with positional placeholders: `{0}`, `{1}`, …
            /// Allocates a `String` only when args are provided.
            pub fn fmt(self, args: &[&str]) -> String {
                let tmpl = self.text();
                if args.is_empty() {
                    return tmpl.to_string();
                }
                let mut result = tmpl.to_string();
                for (i, arg) in args.iter().enumerate() {
                    result = result.replace(&format!("{{{i}}}"), arg);
                }
                result
            }
        }
    };
}

