//! Shared platform utilities.

/// Encode raw RGBA pixel data as a PNG byte vector.
#[allow(dead_code)]
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use image::{ImageEncoder, codecs::png::PngEncoder};
    let mut buf = Vec::new();
    let encoder = PngEncoder::new(&mut buf);
    encoder
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(buf)
}

/// Render an HICON to a 32x32 BGRA DIB, convert to RGBA PNG, and base64-encode.
/// Shared by source app icon extraction and file icon extraction on Windows.
#[cfg(target_os = "windows")]
pub fn hicon_to_base64_png(hicon: windows_sys::Win32::Foundation::HANDLE, size: i32) -> Option<String> {
    use base64::Engine;
    use windows_sys::Win32::Foundation::{BOOL, HANDLE};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HDC, HGDIOBJ, HBRUSH,
    };

    extern "system" {
        fn DrawIconEx(
            hdc: HDC,
            xLeft: i32,
            yTop: i32,
            hIcon: HANDLE,
            cxWidth: i32,
            cyWidth: i32,
            istepIfAniCur: u32,
            hbrFlickerFreeDraw: HBRUSH,
            diFlags: u32,
        ) -> BOOL;
    }

    const DI_NORMAL: u32 = 0x0003;

    unsafe {
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
                biWidth: size,
                biHeight: -size,
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
        DrawIconEx(mem_dc, 0, 0, hicon, size, size, 0, std::ptr::null_mut(), DI_NORMAL);

        let pixel_count = (size * size) as usize;
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

        let png_bytes = encode_png(&rgba, size as u32, size as u32)?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        Some(b64)
    }
}

/// Convert an NSImage to a base64-encoded PNG.
/// Shared by source app icon extraction and file icon extraction on macOS.
#[cfg(target_os = "macos")]
pub fn nsimage_to_base64_png(image: &objc2_app_kit::NSImage, size: i32) -> Option<String> {
    unsafe {
        use base64::Engine;
        use objc2::msg_send;
        use objc2::rc::Retained;

        // Extract TIFF data from NSImage
        let tiff: Retained<objc2::runtime::NSObject> = msg_send![image, TIFFRepresentation];
        let bytes: *const std::ffi::c_void = msg_send![&tiff, bytes];
        let bytes = bytes as *const u8;
        let len: usize = msg_send![&tiff, length];
        let tiff_bytes = std::slice::from_raw_parts(bytes, len);

        // Decode TIFF → resize → encode PNG → base64
        let img = image::load_from_memory(tiff_bytes).ok()?;
        let resized = img.resize_exact(size as u32, size as u32, image::imageops::FilterType::Lanczos3);
        let mut png_bytes = Vec::new();
        resized.write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png).ok()?;

        Some(base64::engine::general_purpose::STANDARD.encode(&png_bytes))
    }
}
