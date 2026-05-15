//! File icon extraction via SHGetFileInfoW (Windows).
//! Returns the system icon for any file path as base64-encoded PNG.

#[cfg(target_os = "windows")]
mod windows_impl {
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHGFI_ICON, SHGFI_LARGEICON, SHFILEINFOW};

    pub fn extract_file_icon_base64(file_path: &str) -> Option<String> {
        unsafe {
            let wide_path: Vec<u16> = file_path.encode_utf16().chain(std::iter::once(0)).collect();

            let mut shfi: SHFILEINFOW = std::mem::zeroed();
            let result = SHGetFileInfoW(
                wide_path.as_ptr(),
                0,
                &mut shfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            );
            if result == 0 || shfi.hIcon.is_null() {
                return None;
            }

            super::super::util::hicon_to_base64_png(shfi.hIcon, 32)
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod fallback {
    pub fn extract_file_icon_base64(_file_path: &str) -> Option<String> {
        None
    }
}

pub fn extract_file_icon_base64(file_path: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::extract_file_icon_base64(file_path)
    }
    #[cfg(not(target_os = "windows"))]
    {
        fallback::extract_file_icon_base64(file_path)
    }
}
