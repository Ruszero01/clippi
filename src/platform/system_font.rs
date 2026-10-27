//! DirectWrite answers about the system UI font, for the workaround in
//! [`crate::ui::font::system_family`] and the font picker's family filter.

use windows::core::PCWSTR;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory5, IDWriteFontCollection1, IDWriteFontSet,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL,
};
use windows_sys::Win32::Graphics::Gdi::LOGFONTW;
use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETICONTITLELOGFONT};

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

/// One snapshot of the machine's DirectWrite families.
///
/// Holding the collection (rather than re-opening it per name) lets the picker
/// filter its whole list for the cost of a single enumeration.
pub struct SystemFamilies {
    /// Kept so the collection's factory outlives the borrowed font set.
    _collection: IDWriteFontCollection1,
    font_set: IDWriteFontSet,
}

impl SystemFamilies {
    /// Open the system collection the same way gpui does —
    /// `GetSystemFontCollection(rotate = false, preferStandardFontProxies = true)`.
    fn open() -> Option<Self> {
        // SAFETY: plain COM construction calls with no caller-side invariants —
        // `collection` is an out-parameter the callee fills in before we read it,
        // and the returned font set keeps itself valid for as long as we hold the
        // collection that produced it.
        unsafe {
            let factory =
                DWriteCreateFactory::<IDWriteFactory5>(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let mut collection: Option<IDWriteFontCollection1> = None;
            if factory
                .GetSystemFontCollection(false, &mut collection, true)
                .is_err()
            {
                return None;
            }
            let collection = collection?;
            let font_set = collection.GetFontSet().ok()?;
            Some(Self {
                _collection: collection,
                font_set,
            })
        }
    }

    /// Does DirectWrite expose a family under this name?
    ///
    /// Mirrors gpui's own lookup — `GetFontSet` then `GetMatchingFonts` at
    /// normal weight and style — so `true` means the name we hand gpui is
    /// guaranteed to produce at least one face. DirectWrite answers for any
    /// localized spelling of a listed family, so the GDI name and the localized
    /// name the font picker shows both resolve here.
    pub fn resolves(&self, name: &str) -> bool {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            self.font_set
                .GetMatchingFonts(
                    PCWSTR(wide.as_ptr()),
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
                .map(|matches| matches.GetFontCount() > 0)
                .unwrap_or(false)
        }
    }
}

/// Does DirectWrite expose a family under this name? See
/// [`SystemFamilies::resolves`].
pub fn family_is_registered(name: &str) -> bool {
    SystemFamilies::open().is_some_and(|families| families.resolves(name))
}

/// Drop the names gpui cannot resolve.
///
/// gpui's Windows text system logs and then *unwraps* its own fallback when a
/// family matches nothing, so a name that does not resolve is not a cosmetic
/// problem — it ends the process. When DirectWrite cannot be opened at all the
/// input is returned untouched: gpui reads the same collection for every glyph
/// it draws, so a machine where this fails has bigger problems than the picker,
/// and an empty list would silently cost the user their font choice.
pub fn retain_registered(names: Vec<String>) -> Vec<String> {
    match SystemFamilies::open() {
        Some(families) => names.into_iter().filter(|n| families.resolves(n)).collect(),
        None => {
            log::warn!("DirectWrite system font collection unavailable; not filtering.");
            names
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe has to agree with gpui's lookup, so the names gpui injects into
    /// `all_font_names` from its fallback stack must read as unresolvable —
    /// handing one to `select_font` is what used to crash the font picker.
    #[test]
    fn gpui_placeholder_families_do_not_resolve() {
        for name in [
            ".ZedMono",
            ".ZedSans",
            ".SystemUIFont",
            "NoSuchFamily 12345",
        ] {
            assert!(
                !family_is_registered(name),
                "{name} resolved but should not"
            );
        }
    }

    #[test]
    fn a_shipped_family_resolves() {
        assert!(family_is_registered("Segoe UI"));
    }

    #[test]
    fn retain_registered_keeps_only_resolvable_names() {
        let names = vec![
            "Segoe UI".to_string(),
            ".ZedMono".to_string(),
            "NoSuchFamily 12345".to_string(),
        ];
        let kept = retain_registered(names);
        assert!(kept.contains(&"Segoe UI".to_string()), "{kept:?}");
        assert_eq!(kept.len(), 1, "{kept:?}");
    }
}
