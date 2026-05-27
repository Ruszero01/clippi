//! OCR (Optical Character Recognition) module
//!
//! Platform-agnostic trait with native implementations for Windows and macOS.

use std::path::Path;

/// Result of an OCR operation: the recognized text.
pub type OcrResult = Result<String, String>;

/// Platform-agnostic OCR engine interface.
pub trait OcrEngine: Send {
    /// Recognize text from an image file. Returns the extracted text.
    fn recognize(&self, image_path: &Path) -> OcrResult;
}

// ── Windows: Windows.Media.Ocr ──

#[cfg(target_os = "windows")]
mod win {
    use super::*;
    use windows::core::*;

    pub struct WindowsOcrEngine;

    impl OcrEngine for WindowsOcrEngine {
        fn recognize(&self, image_path: &Path) -> OcrResult {
            let path_str = image_path.to_str().ok_or_else(|| "OCR: invalid image path".to_string())?;

            let file = windows::Storage::StorageFile::GetFileFromPathAsync(&HSTRING::from(path_str))
                .map_err(|e| format!("OCR StorageFile: {e}"))?
                .get()
                .map_err(|e| format!("OCR StorageFile get: {e}"))?;

            let stream = file
                .OpenAsync(windows::Storage::FileAccessMode::Read)
                .map_err(|e| format!("OCR OpenAsync: {e}"))?
                .get()
                .map_err(|e| format!("OCR OpenAsync get: {e}"))?;

            let decoder = windows::Graphics::Imaging::BitmapDecoder::CreateAsync(&stream)
                .map_err(|e| format!("OCR BitmapDecoder: {e}"))?
                .get()
                .map_err(|e| format!("OCR BitmapDecoder get: {e}"))?;

            let raw_bitmap = decoder
                .GetSoftwareBitmapAsync()
                .map_err(|e| format!("OCR GetSoftwareBitmap: {e}"))?
                .get()
                .map_err(|e| format!("OCR GetSoftwareBitmap get: {e}"))?;

            // Convert to Gray8 — the Windows OCR engine works best with grayscale
            let bitmap = windows::Graphics::Imaging::SoftwareBitmap::Convert(
                &raw_bitmap,
                windows::Graphics::Imaging::BitmapPixelFormat::Gray8,
            )
            .map_err(|e| format!("OCR Convert to Gray8: {e}"))?;

            // Language priority mirrors macOS behavior:
            // ff32a47 sets recognitionLanguages = ["zh-Hans", "zh-Hant", "en"]
            // Try zh-Hans first for Chinese accuracy, then user languages, then en-US
            let language_tags: &[(bool, &str)] = &[
                (false, "zh-Hans"),   // Simplified Chinese (primary)
                (true, ""),           // User profile languages
                (false, "en-US"),     // English final fallback
            ];

            for &(use_profile, tag) in language_tags {
                let engine = if use_profile {
                    windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages().ok()
                } else {
                    let lang = match windows::Globalization::Language::CreateLanguage(&HSTRING::from(tag)) {
                        Ok(l) => l,
                        Err(_) => continue,
                    };
                    windows::Media::Ocr::OcrEngine::TryCreateFromLanguage(&lang).ok()
                };

                if let Some(engine) = engine {
                    if let Ok(result) = engine.RecognizeAsync(&bitmap) {
                        if let Ok(result) = result.get() {
                            if let Ok(text) = result.Text() {
                                let s = text.to_string();
                                if !s.trim().is_empty() {
                                    return Ok(s);
                                }
                            }
                        }
                    }
                }
            }

            Err("OCR: no text recognized with any available language".to_string())
        }
    }
}

#[cfg(target_os = "windows")]
pub use win::WindowsOcrEngine;

// ── macOS: Apple Vision Framework via native ObjC helper ──
///
/// Uses a small Objective-C helper (ocr_helper.m) compiled into the binary
/// to avoid objc2 msg_send! type-encoding issues with CGImageRef etc.
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::{c_char, CStr, CString};

    extern "C" {
        fn clippi_ocr_recognize(image_path: *const c_char) -> *mut c_char;
        fn clippi_ocr_free_string(s: *mut c_char);
    }

    pub struct AppleVisionOcrEngine;

    impl OcrEngine for AppleVisionOcrEngine {
        fn recognize(&self, image_path: &Path) -> OcrResult {
            let path_str = image_path
                .to_str()
                .ok_or_else(|| "OCR: non-UTF-8 path".to_string())?;
            let c_path = CString::new(path_str)
                .map_err(|_| "OCR: path contains null byte".to_string())?;

            let ptr = unsafe { clippi_ocr_recognize(c_path.as_ptr()) };
            if ptr.is_null() {
                return Err("OCR: recognition failed".to_string());
            }

            let result = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
            unsafe { clippi_ocr_free_string(ptr) };
            Ok(result)
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::AppleVisionOcrEngine;

// ── Stub (Linux / unsupported) ──

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod stub {
    use super::*;

    pub struct StubOcrEngine;

    impl OcrEngine for StubOcrEngine {
        fn recognize(&self, _image_path: &Path) -> OcrResult {
            Err("OCR not supported on this platform".to_string())
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use stub::StubOcrEngine;

/// Create the platform-appropriate OCR engine.
pub fn create_ocr_engine() -> Box<dyn OcrEngine> {
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsOcrEngine)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(AppleVisionOcrEngine)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Box::new(StubOcrEngine)
    }
}
