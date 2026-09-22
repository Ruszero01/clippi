//! Bridges the language setting to gpui-component's own translations.
//!
//! Clippi translates its own strings through `core::i18n`, but a few strings
//! belong to gpui-component (the input context menu, the editor's find/replace
//! bar) and are resolved through rust_i18n. Setting both keeps the UI in one
//! language.

/// Apply a language code (`"en"`, `"zh"`, or empty for the system language).
pub fn set_language(lang: &str) {
    crate::core::i18n::set_language(lang);
    gpui_component::set_locale(if crate::core::i18n::is_en() {
        "en"
    } else {
        "zh-CN"
    });
}
