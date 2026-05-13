//! File icon extraction via SHGetFileInfoW (Windows).
//! Returns the system icon for any file path as base64-encoded PNG.

#[cfg(target_os = "windows")]
mod windows_impl {
    use base64::Engine;
    use windows_sys::Win32::Foundation::{BOOL, HANDLE};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HDC, HGDIOBJ,
    };
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHGFI_ICON, SHGFI_LARGEICON, SHFILEINFOW};

    extern "system" {
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

    const DI_NORMAL: u32 = 0x0003;

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
