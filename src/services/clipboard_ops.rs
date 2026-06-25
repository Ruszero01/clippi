//! --- Platform-agnostic clipboard write and verification utilities. ---
//!
//! Extracted from the Slint `app.rs` callback layer so both the Slint and
//! --- GPUI frontends can share clipboard write logic. ---

use crate::core::types::{ClipboardItem, ContentType, RichData};
use crate::platform::clipboard::with_clipboard_context;
use clipboard_rs::{Clipboard, ClipboardContent};

/// Write a clipboard item's content to the system clipboard.
///
/// When `copy_as_plain_text` is true, only plain text is written; otherwise
/// HTML and RTF formats are also restored from `rich_data`.
/// For images, the PNG file is loaded and written as an image format.
/// For files, file paths are written via `ClipboardContent::Files` (CF_HDROP).
pub fn write_item_to_clipboard(item: &ClipboardItem, copy_as_plain_text: bool) {
    with_clipboard_context(|ctx| {
        if item.content_type == ContentType::Image && !item.image_path.is_empty() {
            #[cfg(target_os = "windows")]
            {
                use windows_sys::Win32::System::DataExchange::{
                    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
                    SetClipboardData,
                };
                use windows_sys::Win32::System::Memory::{
                    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
                };

                let png_bytes = std::fs::read(&item.image_path).unwrap_or_default();
                if !png_bytes.is_empty() {
                    let png_name: Vec<u16> = "PNG\0".encode_utf16().collect();
                    // SAFETY: `png_name` is a NUL-terminated UTF-16 string.
                    // `RegisterClipboardFormatW` is a read-only string lookup
                    // that returns a format ID (non-zero on success).
                    let png_fmt = unsafe { RegisterClipboardFormatW(png_name.as_ptr()) };

                    // SAFETY: All clipboard API calls here follow the standard
                    // Windows clipboard sequence: OpenClipboard → EmptyClipboard →
                    // SetClipboardData → CloseClipboard. `GlobalAlloc`/`GlobalLock`/
                    // `GlobalUnlock` manage the HGLOBAL memory correctly – ownership
                    // is transferred to the clipboard via `SetClipboardData` and
                    // will be freed by the system. The `png_bytes` buffer is
                    // `copy_nonoverlapping`'d into the HGLOBAL before unlock.
                    unsafe {
                        if OpenClipboard(std::ptr::null_mut()) != 0 {
                            EmptyClipboard();
                            if png_fmt != 0 {
                                let mem = GlobalAlloc(GMEM_MOVEABLE, png_bytes.len());
                                if !mem.is_null() {
                                    let ptr = GlobalLock(mem);
                                    if !ptr.is_null() {
                                        std::ptr::copy_nonoverlapping(
                                            png_bytes.as_ptr(),
                                            ptr as *mut u8,
                                            png_bytes.len(),
                                        );
                                        GlobalUnlock(mem);
                                        SetClipboardData(png_fmt as u32, mem);
                                    }
                                }
                            }
                            CloseClipboard();
                        }
                    }
                }
            }

            #[cfg(not(target_os = "windows"))]
            {
                use clipboard_rs::common::{RustImage, RustImageData};
                if let Ok(img_data) = RustImageData::from_path(&item.image_path) {
                    let _ = ctx.set_image(img_data);
                }
            }
        } else if item.content_type == ContentType::File && !item.file_data.is_empty() {
            // --- Write file paths to clipboard (CF_HDROP on Windows) ---
            let file_data = crate::core::types::FileData::from_json(&item.file_data);
            let paths: Vec<String> = file_data.files.iter().map(|f| f.path.clone()).collect();
            let contents = vec![ClipboardContent::Files(paths)];
            let _ = Clipboard::set(ctx, contents);
        } else if copy_as_plain_text {
            let _ = Clipboard::set_text(ctx, item.full_text.clone());
        } else {
            let mut contents = vec![ClipboardContent::Text(item.full_text.clone())];
            let rich = RichData::from_json(&item.rich_data);
            if let Some(html) = rich.html {
                contents.push(ClipboardContent::Html(html));
            }
            if let Some(rtf) = rich.rtf {
                contents.push(ClipboardContent::Rtf(rtf));
            }
            let _ = Clipboard::set(ctx, contents);
        }
    });
}

/// Write plain text while sharing the same access guard as the listener.
pub fn write_text_to_clipboard(text: &str) -> bool {
    with_clipboard_context(|ctx| Clipboard::set_text(ctx, text.to_owned()).is_ok()).unwrap_or(false)
}

/// Poll-read clipboard text until it matches `expected` or `timeout_ms` expires.
pub fn verify_clipboard_content(expected: &str, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let matches =
            with_clipboard_context(|ctx| ctx.get_text().is_ok_and(|text| text == expected))
                .unwrap_or(false);
        if matches {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Poll-read clipboard PNG buffer until its length matches `expected_size` or `timeout_ms` expires.
pub fn verify_clipboard_image(expected_size: u64, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let matches = with_clipboard_context(|ctx| {
            ctx.get_buffer("PNG")
                .is_ok_and(|png_bytes| png_bytes.len() as u64 == expected_size)
        })
        .unwrap_or(false);
        if matches {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
