//! Clipboard source application detection and icon extraction
//!
//! Uses GetClipboardOwner() on Windows to identify the process that
//! placed data on the clipboard (not the foreground window).

use crate::core::types::SourceAppInfo;

#[cfg(target_os = "windows")]
mod windows_impl {
    use crate::core::types::SourceAppInfo;
    use base64::Engine;
    use windows_sys::Win32::Foundation::{BOOL, CloseHandle, HANDLE, HWND};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HDC, HGDIOBJ,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHGFI_ICON, SHGFI_LARGEICON, SHFILEINFOW};

    extern "system" {
        fn GetClipboardOwner() -> HWND;
        fn GetWindowThreadProcessId(hwnd: HWND, lpdwProcessId: *mut u32) -> u32;
        fn DrawIconEx(
            hdc: HDC,
            xLeft: i32,
            yTop: i32,
            hIcon: HICON,
            cxWidth: i32,
            cyWidth: i32,
            istepIfAniCur: u32,
            hbrFlickerFreeDraw: HBRUSH,
            diFlags: u32,
        ) -> BOOL;
    }

    #[allow(clippy::upper_case_acronyms)]
    type HICON = HANDLE;
    #[allow(clippy::upper_case_acronyms)]
    type HBRUSH = HANDLE;

    const PROCESS_NAME_WIN32: u32 = 0;
    const DI_NORMAL: u32 = 0x0003;

    pub fn get_clipboard_owner_info() -> Option<SourceAppInfo> {
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
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                buf.as_mut_ptr(),
                &mut len,
            );
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
        let name = std::path::Path::new(exe_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("未知应用");
        let mut chars = name.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => "未知应用".to_string(),
        }
    }

    fn extract_icon_base64(exe_path: &str) -> Option<String> {
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

            let hicon: HICON = shfi.hIcon;
            let icon_size = 32i32;

            let screen_dc = GetDC(std::ptr::null_mut());
            let mem_dc = CreateCompatibleDC(screen_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);

            if mem_dc.is_null() {
                DeleteObject(hicon as HGDIOBJ);
                return None;
            }

            let bmp_info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: icon_size,
                    biHeight: -icon_size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: 0,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [std::mem::zeroed(); 1],
            };

            let mut pixels: *mut std::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(
                mem_dc,
                &bmp_info,
                DIB_RGB_COLORS,
                &mut pixels,
                std::ptr::null_mut(),
                0,
            );
            if dib.is_null() || pixels.is_null() {
                DeleteDC(mem_dc);
                DeleteObject(hicon as HGDIOBJ);
                return None;
            }

            let old_bmp = SelectObject(mem_dc, dib);

            DrawIconEx(mem_dc, 0, 0, hicon, icon_size, icon_size, 0, std::ptr::null_mut(), DI_NORMAL);

            let pixel_count = (icon_size * icon_size) as usize;
            let bgra_data: Vec<u8> =
                std::slice::from_raw_parts(pixels as *const u8, pixel_count * 4).to_vec();

            SelectObject(mem_dc, old_bmp);
            DeleteObject(dib);
            DeleteDC(mem_dc);
            DeleteObject(hicon as HGDIOBJ);

            // BGRA → RGBA
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for chunk in bgra_data.chunks_exact(4) {
                rgba.push(chunk[2]); // R
                rgba.push(chunk[1]); // G
                rgba.push(chunk[0]); // B
                rgba.push(chunk[3]); // A
            }

            let png_bytes = super::super::util::encode_png(&rgba, icon_size as u32, icon_size as u32)?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
            Some(b64)
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use crate::core::types::SourceAppInfo;

    pub fn get_clipboard_owner_info() -> Option<SourceAppInfo> {
        None
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
