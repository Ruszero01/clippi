//! --- Shared platform utilities. ---

/// Encode raw RGBA pixel data as a PNG byte vector.
#[cfg(target_os = "windows")]
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
///
/// Closing the panel calls this (through [`crate::ui::window_manager`]), which
/// is what brings the process back down to a few MB in Task Manager. The
/// silent-start release deliberately does *not* call it: trimming right after
/// launch evicts pages the first summon immediately needs again.
#[cfg(target_os = "windows")]
pub fn trim_process_working_set() {
    // SAFETY: `SetProcessWorkingSetSizeEx` with `SIZE_T(-1, -1)` and no-flags
    // provides a trim hint to the OS on the current process. It can be called
    // from any thread and never fails in a way that corrupts state.
    unsafe {
        use windows_sys::Win32::System::Memory::SetProcessWorkingSetSizeEx;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        SetProcessWorkingSetSizeEx(GetCurrentProcess(), usize::MAX, usize::MAX, 0);
    }
}

#[cfg(target_os = "macos")]
pub fn trim_process_working_set() {
    use std::ffi::c_void;

    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }

    // SAFETY: These macOS malloc APIs only provide a best-effort pressure
    // relief hint. A null zone asks malloc to examine all zones.
    unsafe {
        let _ = malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn trim_process_working_set() {
    // --- no-op on unsupported platforms ---
}

/// Set when a background WAL checkpoint finished and the process should trim
/// its working set once that thread is gone.
///
/// The trim must not run inside the checkpoint thread: that thread's stack and
/// its SQLite buffers are still mapped at that moment, so the working set only
/// settles a few MB higher than it should. The UI poll loop picks the request
/// up on its next tick (≤200 ms), after the thread has exited.
static WORKING_SET_TRIM_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn request_working_set_trim() {
    WORKING_SET_TRIM_PENDING.store(true, std::sync::atomic::Ordering::Release);
}

/// Take the pending trim request, if any.
pub fn take_working_set_trim_request() -> bool {
    WORKING_SET_TRIM_PENDING.swap(false, std::sync::atomic::Ordering::AcqRel)
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

    // SAFETY: GDI drawing calls (`GetDC`, `CreateCompatibleDC`, `CreateDIBSection`,
    // `DrawIconEx`) are safe when used with valid DCs and properly-initialised
    // BITMAPINFO. The raw pixel buffer (`pixels`) is owned by the DIB section
    // and copied via `from_raw_parts` before the DIB/DC are destroyed.
    // `DestroyIcon` takes ownership of `hicon` and is always called (even on
    // error paths). The BGRA→RGBA swizzle uses safe Rust slices.
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

        // --- BGRA → RGBA ---
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

/// Decode a byte run that a document escaped as single-byte `\'hh` sequences.
///
/// `codepage` is the document's declared ANSI code page (`\ansicpgN` in RTF),
/// or 0 when it is unknown and the system default should be used. RichEdit-based
/// Windows sources (Event Viewer, WordPad, most Win32 rich-edit controls) escape
/// CJK text in the system code page — cp936 for Chinese Windows — so the bytes
/// must go through the code page instead of being mapped one-to-one to scalars,
/// which would produce mojibake such as `´íÎó` for `错误` (issue #114).
#[cfg(target_os = "windows")]
pub fn decode_ansi_bytes(codepage: u16, bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{IsValidCodePage, MultiByteToWideChar, CP_ACP};

    if bytes.is_empty() {
        return String::new();
    }
    // SAFETY: `IsValidCodePage` only inspects its value argument.
    let code_page_is_known = codepage != 0 && unsafe { IsValidCodePage(codepage as u32) } != 0;
    let page = if code_page_is_known {
        codepage as u32
    } else {
        CP_ACP
    };

    // SAFETY: `MultiByteToWideChar` reads exactly `bytes.len()` bytes from the
    // input pointer and writes at most `wide.len()` UTF-16 units into the
    // output buffer. Both calls use the same input, and the second call is only
    // made when the first reported a positive unit count.
    unsafe {
        let needed = MultiByteToWideChar(
            page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if needed <= 0 {
            return latin1_to_string(bytes);
        }
        let mut wide = vec![0u16; needed as usize];
        let written = MultiByteToWideChar(
            page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            needed,
        );
        if written <= 0 {
            return latin1_to_string(bytes);
        }
        wide.truncate(written as usize);
        String::from_utf16_lossy(&wide)
    }
}

/// Decode declared Windows code pages with CoreFoundation on macOS.
/// Documents without a code-page hint use RTF's Windows-1252 default.
#[cfg(target_os = "macos")]
pub fn decode_ansi_bytes(codepage: u16, bytes: &[u8]) -> String {
    use core_foundation::base::TCFType;
    use core_foundation::string::{
        kCFStringEncodingInvalidId, CFString, CFStringConvertWindowsCodepageToEncoding,
        CFStringCreateWithBytes,
    };

    if bytes.is_empty() {
        return String::new();
    }
    if codepage == 65001 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let page = if codepage == 0 { 1252 } else { codepage };
    // SAFETY: the conversion only inspects its numeric argument.
    let encoding = unsafe { CFStringConvertWindowsCodepageToEncoding(u32::from(page)) };
    if encoding == kCFStringEncodingInvalidId {
        return latin1_to_string(bytes);
    }
    // SAFETY: the input slice remains valid during creation. A successful
    // create result is owned by CFString, which releases it when dropped.
    let string = unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            encoding,
            0,
        )
    };
    if string.is_null() {
        return latin1_to_string(bytes);
    }
    unsafe { CFString::wrap_under_create_rule(string) }.to_string()
}

/// Best-effort fallback for platforms without a code-page API.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn decode_ansi_bytes(codepage: u16, bytes: &[u8]) -> String {
    if codepage == 65001 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    latin1_to_string(bytes)
}

/// Map raw bytes to the Latin-1 scalars with the same values (never fails).
fn latin1_to_string(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

// macOS geometry types for use with objc2 Encode (available for future needs).
// Kept for potential use with CoreGraphics rendering paths.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
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

    // --- f64 = CGFloat on 64-bit macOS → encoding "d" ---
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

/// Convert an NSImage to a base64-encoded PNG (no resize — caller can scale via UI).
///
/// Uses TIFF → NSBitmapImageRep → PNG pipeline which is thread-safe
/// (unlike lockFocus/unlockFocus which require the main thread).
/// GPUI's `img()` element handles display sizing via `object_fit`.
#[cfg(target_os = "macos")]
pub fn nsimage_to_base64_png(image: &objc2_app_kit::NSImage, _size: i32) -> Option<String> {
    unsafe {
        use std::ffi::{c_char, c_void, CStr};

        extern "C" {
            fn clippi_nsimage_to_png_base64(image: *const c_void, target_size: i32) -> *mut c_char;
            fn clippi_ocr_free_string(s: *mut c_char);
        }

        let ptr = clippi_nsimage_to_png_base64(image as *const _ as *const c_void, _size);
        if ptr.is_null() {
            return None;
        }
        let result = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        clippi_ocr_free_string(ptr);
        Some(result)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod codepage_tests {
    use super::decode_ansi_bytes;

    #[test]
    fn declared_windows_code_pages_decode_on_macos() {
        assert_eq!(decode_ansi_bytes(936, &[0xb4, 0xed, 0xce, 0xf3]), "错误");
        assert_eq!(decode_ansi_bytes(950, &[0xba, 0x7e, 0xa6, 0x72]), "漢字");
        assert_eq!(
            decode_ansi_bytes(932, &[0x93, 0xfa, 0x96, 0x7b, 0x8c, 0xea]),
            "日本語"
        );
        assert_eq!(
            decode_ansi_bytes(949, &[0xc7, 0xd1, 0xb1, 0xb9, 0xbe, 0xee]),
            "한국어"
        );
        assert_eq!(
            decode_ansi_bytes(1251, &[0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2]),
            "Привет"
        );
        assert_eq!(
            decode_ansi_bytes(1252, &[0x80, 0x20, 0x93, 0x63, 0x61, 0x66, 0xe9, 0x94]),
            "€ “café”"
        );
    }

    #[test]
    fn unknown_and_invalid_code_pages_have_safe_fallbacks() {
        assert_eq!(decode_ansi_bytes(0, &[0x80]), "€");
        assert_eq!(decode_ansi_bytes(65535, &[0xe9]), "é");
        assert_eq!(decode_ansi_bytes(936, &[0x81]), "\u{81}");
        assert_eq!(decode_ansi_bytes(936, &[]), "");
        assert_eq!(decode_ansi_bytes(65001, "😀中文".as_bytes()), "😀中文");
    }
}
