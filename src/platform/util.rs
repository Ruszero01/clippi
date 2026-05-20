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

/// Trim the process working set so the OS reclaims idle pages.
/// This is a hint — it resets WS limits and lets Windows evict pages
/// that are no longer actively used. Safe to call at any time.
#[cfg(target_os = "windows")]
pub fn trim_process_working_set() {
    unsafe {
        use windows_sys::Win32::System::Memory::SetProcessWorkingSetSizeEx;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        SetProcessWorkingSetSizeEx(GetCurrentProcess(), usize::MAX, usize::MAX, 0);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn trim_process_working_set() {
    // no-op on non-Windows
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
        HDC, HBRUSH,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon;

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
            DestroyIcon(hicon);
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
            DestroyIcon(hicon);
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
        DestroyIcon(hicon);

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
///
/// Uses the highest-resolution NSBitmapImageRep from the image's representations
/// to avoid multi-page TIFF first-page issues (which can pick a tiny 16px icon).
/// Trims transparent padding before resizing so the icon fills its display area.
#[cfg(target_os = "macos")]
pub fn nsimage_to_base64_png(image: &objc2_app_kit::NSImage, size: i32) -> Option<String> {
    unsafe {
        use base64::Engine;
        use objc2::msg_send;
        use objc2::rc::Retained;
        use objc2::sel;

        // 1. Iterate representations to find the highest-resolution NSBitmapImageRep
        let reps: *mut objc2::runtime::NSObject = msg_send![image, representations];
        let count: usize = msg_send![reps, count];
        if count == 0 {
            return None;
        }

        let png_sel = sel!(representationUsingType:properties:);
        let mut png_data: Option<Retained<objc2::runtime::NSObject>> = None;
        let mut best_w: i32 = 0;

        for i in 0..count {
            let rep: *mut objc2::runtime::NSObject = msg_send![reps, objectAtIndex: i];
            let responds: bool = msg_send![rep, respondsToSelector: png_sel];
            if !responds {
                continue;
            }
            let pw: i32 = msg_send![rep, pixelsWide];
            if pw > best_w {
                let data: Option<Retained<objc2::runtime::NSObject>> =
                    msg_send![rep, representationUsingType: 4usize, properties: std::ptr::null::<objc2::runtime::NSObject>()];
                if data.is_some() {
                    best_w = pw;
                    png_data = data;
                }
            }
        }

        // Fallback: TIFF → NSBitmapImageRep if no bitmap rep found
        if png_data.is_none() {
            let tiff: Retained<objc2::runtime::NSObject> = msg_send![image, TIFFRepresentation];
            let rep: Retained<objc2::runtime::NSObject> = msg_send![
                msg_send![objc2::class!(NSBitmapImageRep), alloc],
                initWithData: &*tiff
            ];
            png_data = msg_send![&rep, representationUsingType: 4usize, properties: std::ptr::null::<objc2::runtime::NSObject>()];
        }

        let png_data = png_data?;

        // 2. Extract PNG bytes from NSData
        let bytes: *const std::ffi::c_void = msg_send![&png_data, bytes];
        let len: usize = msg_send![&png_data, length];
        if bytes.is_null() || len == 0 {
            return None;
        }
        let png_bytes = std::slice::from_raw_parts(bytes as *const u8, len);

        // 3. Decode → trim transparent padding → resize → re-encode → base64
        let img = image::load_from_memory(png_bytes).ok()?;
        let trimmed = trim_transparent_edges(&img);
        let resized = trimmed.resize_exact(size as u32, size as u32, image::imageops::FilterType::Lanczos3);
        let mut out = Vec::new();
        resized.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;

        Some(base64::engine::general_purpose::STANDARD.encode(&out))
    }
}

/// Crop transparent edges from an RGBA image so the content fills the frame.
#[cfg(target_os = "macos")]
fn trim_transparent_edges(img: &image::DynamicImage) -> image::DynamicImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return img.clone();
    }

    let threshold: u8 = 10; // treat alpha <= 10 as fully transparent

    // Find bounds of non-transparent pixels
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x: u32 = 0;
    let mut max_y: u32 = 0;

    for y in 0..h {
        for x in 0..w {
            let alpha = rgba.get_pixel(x, y).0[3];
            if alpha > threshold {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    // If no non-transparent pixels found, keep the original
    if max_x < min_x || max_y < min_y {
        return img.clone();
    }

    let crop_w = max_x - min_x + 1;
    let crop_h = max_y - min_y + 1;

    // Never crop to something unreasonably small
    if crop_w < 8 || crop_h < 8 {
        return img.clone();
    }

    let cropped = image::imageops::crop_imm(&rgba, min_x, min_y, crop_w, crop_h).to_image();
    image::DynamicImage::ImageRgba8(cropped)
}
