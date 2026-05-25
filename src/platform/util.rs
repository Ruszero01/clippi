//! Shared platform utilities.

/// Encode raw RGBA pixel data as a PNG byte vector.
#[allow(dead_code)]
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use image::{codecs::png::PngEncoder, ImageEncoder};
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
pub fn hicon_to_base64_png(
    hicon: windows_sys::Win32::Foundation::HANDLE,
    size: i32,
) -> Option<String> {
    use base64::Engine;
    use windows_sys::Win32::Foundation::{BOOL, HANDLE};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBRUSH, HDC,
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
        DrawIconEx(
            mem_dc,
            0,
            0,
            hicon,
            size,
            size,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        );

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

// macOS geometry types for NSImage drawing (CGRect/CGSize equivalents
// with objc2::Encode impls, since core_graphics types use a different objc2 version).
#[cfg(target_os = "macos")]
mod macos_geometry {
    use objc2::encode::{Encode, Encoding, RefEncode};

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGRect {
        pub origin: CGPoint,
        pub size: CGSize,
    }

    impl CGPoint {
        pub const fn new(x: f64, y: f64) -> Self {
            Self { x, y }
        }
    }

    impl CGSize {
        pub const fn new(width: f64, height: f64) -> Self {
            Self { width, height }
        }
    }

    impl CGRect {
        pub const fn new(origin: CGPoint, size: CGSize) -> Self {
            Self { origin, size }
        }
    }

    // f64 = CGFloat on 64-bit macOS → encoding "d"
    unsafe impl Encode for CGPoint {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl RefEncode for CGPoint {
        const ENCODING_REF: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }

    unsafe impl Encode for CGSize {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl RefEncode for CGSize {
        const ENCODING_REF: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }

    unsafe impl Encode for CGRect {
        const ENCODING: Encoding =
            Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
    }
    unsafe impl RefEncode for CGRect {
        const ENCODING_REF: Encoding =
            Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
    }
}

/// Convert an NSImage to a base64-encoded PNG at the target size.
///
/// Uses native NSImage drawing (GPU-accelerated) to scale the source icon
/// into a new 32×32 image, then encodes via TIFF → NSBitmapImageRep → PNG.
/// Avoids the `image` crate decode/encode cycle entirely (no Lanczos3 in
/// software, no transparent-edge trimming ― NSImage scaling handles it).
#[cfg(target_os = "macos")]
pub fn nsimage_to_base64_png(image: &objc2_app_kit::NSImage, size: i32) -> Option<String> {
    unsafe {
        use base64::Engine;
        use macos_geometry::{CGPoint, CGRect, CGSize};
        use objc2::class;
        use objc2::msg_send;
        use objc2::rc::Retained;

        let size_f = size as f64;

        // 1. Create target NSImage and draw source into it (GPU-accelerated scale)
        let target: Retained<objc2::runtime::NSObject> = msg_send![
            msg_send![class!(NSImage), alloc],
            initWithSize: CGSize::new(size_f, size_f)
        ];
        let _: () = msg_send![&target, lockFocus];

        let src_size: CGSize = msg_send![image, size];
        let dest_rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(size_f, size_f));
        let src_rect = CGRect::new(CGPoint::new(0.0, 0.0), src_size);
        // NSCompositingOperationCopy = 2
        let _: () = msg_send![image, drawInRect: dest_rect, fromRect: src_rect, operation: 2usize, fraction: 1.0f64];
        let _: () = msg_send![&target, unlockFocus];

        // 2. TIFF → NSBitmapImageRep → PNG
        let tiff: Retained<objc2::runtime::NSObject> = msg_send![&target, TIFFRepresentation];
        let rep: Retained<objc2::runtime::NSObject> = msg_send![
            msg_send![class!(NSBitmapImageRep), alloc],
            initWithData: &*tiff
        ];
        let png_data: Option<Retained<objc2::runtime::NSObject>> = msg_send![
            &rep,
            representationUsingType: 4usize,
            properties: std::ptr::null::<objc2::runtime::NSObject>()
        ];
        let png_data = png_data?;

        // 3. Base64 encode PNG bytes directly (no image crate round-trip)
        let bytes: *const std::ffi::c_void = msg_send![&png_data, bytes];
        let len: usize = msg_send![&png_data, length];
        if bytes.is_null() || len == 0 {
            return None;
        }
        let png_bytes = std::slice::from_raw_parts(bytes as *const u8, len);
        Some(base64::engine::general_purpose::STANDARD.encode(png_bytes))
    }
}
