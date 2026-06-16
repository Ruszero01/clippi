//! --- Clipboard source application detection and icon extraction ---
//!
//! --- Uses GetClipboardOwner() on Windows to identify the process that ---
//! --- placed data on the clipboard (not the foreground window). ---

use crate::core::types::SourceAppInfo;

#[cfg(target_os = "windows")]
mod windows_impl {
    use crate::core::i18n_keys::I18nKey;
    use crate::core::types::SourceAppInfo;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::Shell::{
        SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON,
        SHGFI_USEFILEATTRIBUTES,
    };

    extern "system" {
        fn GetClipboardOwner() -> HWND;
        fn GetWindowThreadProcessId(hwnd: HWND, lpdwProcessId: *mut u32) -> u32;
    }

    const PROCESS_NAME_WIN32: u32 = 0;

    pub fn get_clipboard_owner_info() -> Option<SourceAppInfo> {
        // SAFETY: `GetClipboardOwner` reads the current clipboard owner HWND
        // (query-only, no side effects). `OpenProcess` with
        // PROCESS_QUERY_LIMITED_INFORMATION is the least-privilege access mode.
        // `QueryFullProcessImageNameW` writes into a stack-allocated buffer
        // whose capacity matches the reported `len`. `CloseHandle` is always
        // called on non-null handles.
        unsafe {
            let hwnd = GetClipboardOwner();
            if hwnd.is_null() {
                return None;
            }

            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == 0 {
                return None;
            }

            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut buf = [0u16; 260];
            let mut len = buf.len() as u32;
            let result =
                QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(process);
            if result == 0 {
                return None;
            }

            let exe_path = String::from_utf16_lossy(&buf[..len as usize]);
            let app_name = extract_app_name(&exe_path);
            let icon_base64 = extract_icon_base64(&exe_path)?;

            Some(SourceAppInfo {
                app_name,
                icon_base64,
            })
        }
    }

    fn extract_app_name(exe_path: &str) -> String {
        let fallback = I18nKey::UnknownApp.text();
        let name = std::path::Path::new(exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(fallback);
        let mut chars = name.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => fallback.to_string(),
        }
    }

    fn extract_icon_base64(exe_path: &str) -> Option<String> {
        // SAFETY: `SHGetFileInfoW` reads file metadata only; the input path
        // buffer is null-terminated and stack-allocated. `SHFILEINFOW` is
        // zeroed before the call. The returned `hIcon` is passed to
        // `hicon_to_base64_png` which takes ownership and calls `DestroyIcon`.
        unsafe {
            let wide_path: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();

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

    pub fn get_file_icon_base64(file_path: &str, is_dir: bool) -> Option<String> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        };
        let attrs = if is_dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        // SAFETY: `SHGetFileInfoW` with `SHGFI_USEFILEATTRIBUTES` does not
        // access the file system — it returns the icon associated with the
        // file extension. The input buffer is null-terminated, the output
        // struct is zeroed, and `hicon_to_base64_png` takes HICON ownership.
        unsafe {
            let wide_path: Vec<u16> = file_path.encode_utf16().chain(std::iter::once(0)).collect();

            let mut shfi: SHFILEINFOW = std::mem::zeroed();
            let result = SHGetFileInfoW(
                wide_path.as_ptr(),
                attrs,
                &mut shfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
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
    use crate::core::i18n_keys::I18nKey;
    use crate::core::types::SourceAppInfo;
    use crate::platform::util::nsimage_to_base64_png;
    use objc2_app_kit::NSWorkspace;

    pub fn get_clipboard_owner_info() -> Option<SourceAppInfo> {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace.frontmostApplication()?;

        // --- Don't record Clippi itself as the source ---
        if app.processIdentifier() == std::process::id() as i32 {
            return None;
        }

        // --- Use generated methods (nil-safe via Option) ---
        let app_name = app
            .localizedName()
            .map(|n| n.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| I18nKey::UnknownApp.text().to_string());
        let icon_base64 = app
            .icon()
            .and_then(|i| nsimage_to_base64_png(&i, 32))
            .unwrap_or_default();

        Some(SourceAppInfo {
            app_name,
            icon_base64,
        })
    }

    pub fn get_file_icon_base64(file_path: &str, _is_dir: bool) -> Option<String> {
        let path = objc2_foundation::NSString::from_str(file_path);
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let icon = workspace.iconForFile(&path);
        super::super::util::nsimage_to_base64_png(&icon, 32)
    }
}

pub fn get_clipboard_owner_info() -> Option<SourceAppInfo> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::get_clipboard_owner_info()
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::get_clipboard_owner_info()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

/// Get a file's associated system icon as a base64-encoded PNG (32×32).
/// Uses `SHGetFileInfoW` with `SHGFI_USEFILEATTRIBUTES` on Windows so the
/// file doesn't need to exist — extension-based lookup.
pub fn get_file_icon_base64(file_path: &str, is_dir: bool) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::get_file_icon_base64(file_path, is_dir)
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::get_file_icon_base64(file_path, is_dir)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (file_path, is_dir);
        None
    }
}
