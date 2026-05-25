//! File icon extraction via SHGetFileInfoW (Windows).
//! Returns the system icon for any file path as base64-encoded PNG.

#[cfg(target_os = "windows")]
mod windows_impl {
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};

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

#[cfg(target_os = "macos")]
mod macos_impl {
    use crate::platform::util::nsimage_to_base64_png;
    use objc2_app_kit::{NSImage, NSWorkspace};
    use objc2_foundation::NSString;

    pub fn extract_file_icon_base64(file_path: &str) -> Option<String> {
        unsafe {
            // NSWorkspace.iconForFile: returns the icon associated with the file
            use objc2::msg_send;
            use objc2::rc::Retained;
            let workspace = NSWorkspace::sharedWorkspace();
            let path_str = NSString::from_str(file_path);
            let icon: Option<Retained<NSImage>> = msg_send![&workspace, iconForFile: &*path_str];
            let icon = icon?;
            nsimage_to_base64_png(&icon, 32)
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
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
    #[cfg(target_os = "macos")]
    {
        macos_impl::extract_file_icon_base64(file_path)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        fallback::extract_file_icon_base64(file_path)
    }
}
