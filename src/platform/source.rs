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
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    extern "system" {
        fn GetClipboardOwner() -> HWND;
        fn GetWindowThreadProcessId(hwnd: HWND, lpdwProcessId: *mut u32) -> u32;
    }

    const PROCESS_NAME_WIN32: u32 = 0;

    /// Processes whose clipboard ownership is a proxy for the real app (e.g.
    /// WebView2 runtime hosts the clipboard on behalf of the parent app).
    const PROXY_EXE_NAMES: &[&str] = &["msedgewebview2.exe"];

    /// Query the executable path of the process that owns `hwnd`.
    /// Returns `None` if the process cannot be opened or queried.
    unsafe fn exe_path_from_hwnd(hwnd: HWND) -> Option<String> {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            // Protected / elevated processes may deny QUERY_LIMITED_INFORMATION.
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
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// Build `SourceAppInfo` from an executable path.
    fn source_info_from_exe(exe_path: &str) -> Option<SourceAppInfo> {
        let app_name = extract_app_name(exe_path);
        let icon_base64 = extract_icon_base64(exe_path)?;
        Some(SourceAppInfo {
            app_name,
            icon_base64,
        })
    }

    fn is_proxy_process(exe_path: &str) -> bool {
        std::path::Path::new(exe_path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|name| {
                let lower = name.to_lowercase();
                PROXY_EXE_NAMES.contains(&lower.as_str())
            })
            .unwrap_or(false)
    }

    pub fn get_clipboard_owner_info() -> Option<SourceAppInfo> {
        unsafe {
            let owner_hwnd = GetClipboardOwner();

            if !owner_hwnd.is_null() {
                if let Some(exe_path) = exe_path_from_hwnd(owner_hwnd) {
                    if !is_proxy_process(&exe_path) {
                        if let Some(info) = source_info_from_exe(&exe_path) {
                            return Some(info);
                        }
                        log::warn!(
                            "get_clipboard_owner_info: icon extraction failed for {exe_path}"
                        );
                    }
                }
            }

            // Fallback: use the foreground window.
            let fg_hwnd = GetForegroundWindow();
            if fg_hwnd.is_null() || fg_hwnd == owner_hwnd {
                return None;
            }
            if let Some(fg_path) = exe_path_from_hwnd(fg_hwnd) {
                if let Some(fg_info) = source_info_from_exe(&fg_path) {
                    return Some(fg_info);
                }
            }
            None
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

    /// Extract the actual embedded icon from a specific file by accessing the
    /// file system. Unlike `get_file_icon_base64`, this does NOT use
    /// `SHGFI_USEFILEATTRIBUTES` — it reads the file's own icon resources.
    /// Returns `None` if the file does not exist or has no icon.
    pub fn get_actual_file_icon_base64(file_path: &str) -> Option<String> {
        // SAFETY: `SHGetFileInfoW` reads file metadata; the input path buffer
        // is null-terminated and stack-allocated. `SHFILEINFOW` is zeroed.
        // The returned `hIcon` is passed to `hicon_to_base64_png` which takes
        // ownership and calls `DestroyIcon`.
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

    /// macOS `iconForFile:` already reads the actual file icon — delegate.
    pub fn get_actual_file_icon_base64(file_path: &str) -> Option<String> {
        get_file_icon_base64(file_path, false)
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

/// Return `true` for file extensions whose files can carry unique embedded
/// icon resources. Per-file icon caching should be used for these extensions
/// so that each file shows its own icon instead of a shared generic one.
pub fn extension_has_embedded_icon(ext: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        matches!(
            ext,
            "exe" | "dll" | "msi" | "scr" | "ocx" | "cpl" | "ico" | "lnk"
        )
    }
    #[cfg(target_os = "macos")]
    {
        matches!(
            ext,
            "app"
                | "appex"
                | "prefpane"
                | "bundle"
                | "framework"
                | "kext"
                | "saver"
                | "icns"
                | "dylib"
        )
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = ext;
        false
    }
}

/// Extract the actual embedded icon from a specific file by accessing the
/// file system. Unlike `get_file_icon_base64`, this reads the file's own
/// icon resources rather than looking up the extension association.
/// Returns `None` if the file does not exist or has no icon.
pub fn get_actual_file_icon_base64(file_path: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::get_actual_file_icon_base64(file_path)
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::get_actual_file_icon_base64(file_path)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = file_path;
        None
    }
}
