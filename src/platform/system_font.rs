//! DirectWrite answers about the system UI font, for the workaround in
//! [`crate::ui::font::system_family`].

use windows::core::PCWSTR;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, IDWriteFactory5,
    IDWriteFontCollection1,
};
use windows_sys::Win32::Graphics::Gdi::LOGFONTW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETICONTITLELOGFONT,
};

/// The face Windows uses for shell/icon text. gpui substitutes it for its
/// `.SystemUIFont` sentinel, so this is the exact name gpui will ask
/// DirectWrite for — including gpui's NUL trimming of the fixed-size
/// `lfFaceName` buffer, and its "Segoe UI" default when the call fails.
pub fn shell_ui_font_name() -> Option<String> {
    let mut logfont: LOGFONTW = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETICONTITLELOGFONT,
            std::mem::size_of::<LOGFONTW>() as u32,
            &mut logfont as *mut _ as *mut _,
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    let name = String::from_utf16_lossy(&logfont.lfFaceName);
    let name = name.trim_matches(char::from(0)).to_string();
    (!name.is_empty()).then_some(name)
}

/// Does DirectWrite expose a family under this name?
///
/// Mirrors gpui's own lookup — `GetSystemFontCollection` then `GetMatchingFonts`
/// at normal weight and style — so `true` means the name we hand gpui is
/// guaranteed to produce at least one face. DirectWrite answers for any
/// localized spelling of a listed family, so the GDI name and the localized name
/// the font picker shows both resolve here.
pub fn family_is_registered(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let Ok(factory) = DWriteCreateFactory::<IDWriteFactory5>(DWRITE_FACTORY_TYPE_SHARED) else {
            return false;
        };
        let mut collection: Option<IDWriteFontCollection1> = None;
        if factory
            .GetSystemFontCollection(false, &mut collection, true)
            .is_err()
        {
            return false;
        }
        let Some(collection) = collection else {
            return false;
        };
        let Ok(fontset) = collection.GetFontSet() else {
            return false;
        };
        let Ok(matches) = fontset.GetMatchingFonts(
            PCWSTR(wide.as_ptr()),
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        ) else {
            return false;
        };
        matches.GetFontCount() > 0
    }
}
