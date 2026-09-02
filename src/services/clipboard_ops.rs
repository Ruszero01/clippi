//! Platform-agnostic clipboard write and verification utilities.

use crate::core::types::{ClipboardItem, ContentType, RichData};
use crate::platform::clipboard::with_clipboard_context;
use clipboard_rs::common::{RustImage, RustImageData};
use clipboard_rs::{Clipboard, ClipboardContent, ContentFormat};

/// Write a clipboard item's content to the system clipboard.
///
/// When `copy_as_plain_text` is true, rich text is reduced to plain text.
/// This setting does not convert images or files into path text.
/// For images, the backing image file is written as a file reference.
/// For files, file paths are written via `ClipboardContent::Files` (CF_HDROP).
pub fn write_item_to_clipboard(item: &ClipboardItem, copy_as_plain_text: bool) {
    with_clipboard_context(|ctx| {
        if item.content_type == ContentType::Image && !item.image_path.is_empty() {
            if std::path::Path::new(&item.image_path).is_file() {
                let contents = vec![ClipboardContent::Files(vec![item.image_path.clone()])];
                if let Err(e) = Clipboard::set(ctx, contents) {
                    log::warn!(
                        "write_item_to_clipboard: failed to set image file clipboard data for {}: {e}",
                        item.image_path
                    );
                }
            } else {
                log::warn!(
                    "write_item_to_clipboard: image file does not exist: {}",
                    item.image_path
                );
            }
        } else if item.content_type == ContentType::File && !item.file_data.is_empty() {
            // Write file paths to clipboard (CF_HDROP on Windows).
            let file_data = crate::core::types::FileData::from_json(&item.file_data);
            let paths: Vec<String> = file_data.files.iter().map(|f| f.path.clone()).collect();
            let contents = vec![ClipboardContent::Files(paths)];
            let _ = Clipboard::set(ctx, contents);
        } else if copy_as_plain_text {
            let _ = Clipboard::set_text(ctx, plain_text_for_item(item));
        } else {
            let mut contents = vec![ClipboardContent::Text(item.full_text.clone())];
            let rich = RichData::from_json(&item.rich_data);
            if let Some(html) = rich.html {
                #[cfg(target_os = "windows")]
                let html = crate::core::html_text::encode_cf_html(&html);
                #[cfg(target_os = "macos")]
                let html = crate::core::html_text::clipboard_html_for_macos(&html);
                contents.push(ClipboardContent::Html(html));
            }
            if let Some(rtf) = rich.rtf {
                contents.push(ClipboardContent::Rtf(rtf));
            }
            let _ = Clipboard::set(ctx, contents);
        }
    });
}

/// Write an item explicitly as plain text.
///
/// Unlike the global rich-text setting, this operation intentionally converts
/// image and file items into their backing paths.
pub fn write_item_as_plain_text_to_clipboard(item: &ClipboardItem) {
    let text = plain_text_for_item(item);
    let _ = write_text_to_clipboard(&text);
}

/// Resolve the plain-text representation used by all copy/paste paths.
///
/// This is intentionally evaluated at paste time as well as capture time so
/// clipboard history recorded before the WPS/Excel multiline fix is repaired
/// without requiring a database migration or a fresh copy.
pub fn plain_text_for_item(item: &ClipboardItem) -> String {
    if item.content_type == ContentType::Image && !item.image_path.is_empty() {
        item.image_path.clone()
    } else if item.content_type == ContentType::File && !item.file_data.is_empty() {
        let file_data = crate::core::types::FileData::from_json(&item.file_data);
        file_data
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        let rich = RichData::from_json(&item.rich_data);
        if crate::core::html_text::has_spreadsheet_multiline_transport_quotes(&item.full_text) {
            rich.html
                .as_deref()
                .and_then(crate::core::html_text::spreadsheet_plain_text)
                .unwrap_or_else(|| item.full_text.clone())
        } else {
            item.full_text.clone()
        }
    }
}

/// Decode an image file for compatibility paste targets that require bitmap data.
pub fn load_image_bitmap(image_path: &str) -> Option<RustImageData> {
    match RustImageData::from_path(image_path) {
        Ok(img_data) => Some(img_data),
        Err(e) => {
            log::warn!("load_image_bitmap: failed to load image {image_path}: {e}");
            None
        }
    }
}

/// Write decoded image data as native image clipboard formats.
pub fn write_bitmap_image_to_clipboard(img_data: RustImageData) -> bool {
    with_clipboard_context(|ctx| {
        if let Err(e) = ctx.set_image(img_data) {
            log::warn!("write_bitmap_image_to_clipboard: failed to set image data: {e}");
            false
        } else {
            true
        }
    })
    .unwrap_or(false)
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

/// Poll-read the clipboard until a file list is available or `timeout_ms` expires.
pub fn verify_clipboard_files(timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let matches = with_clipboard_context(|ctx| ctx.has(ContentFormat::Files)).unwrap_or(false);
        if matches {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Poll-read the clipboard until an image format is available or `timeout_ms` expires.
pub fn verify_clipboard_image(timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let matches = with_clipboard_context(|ctx| ctx.has(ContentFormat::Image)).unwrap_or(false);
        if matches {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_excel_history_item_is_normalized_when_pasted_as_plain_text() {
        let html = r#"<html xmlns:x="urn:schemas-microsoft-com:office:excel"><body>
<!--StartFragment--><table><tr><td x:str>表格测试纯文本<br>表格测试纯文本</td></tr></table><!--EndFragment-->
</body></html>"#;
        let rich = RichData {
            html: Some(html.into()),
            ..Default::default()
        };
        let item = ClipboardItem::new_text(
            1,
            "\"表格测试纯文本\r\n表格测试纯文本\"",
            ContentType::RichText,
            None,
            Some(&rich),
        );

        assert_eq!(plain_text_for_item(&item), "表格测试纯文本\n表格测试纯文本");
    }

    #[test]
    fn ordinary_quoted_history_text_is_preserved() {
        let item =
            ClipboardItem::new_text(1, "\"用户原始文本\"", ContentType::PlainText, None, None);
        assert_eq!(plain_text_for_item(&item), "\"用户原始文本\"");
    }

    #[test]
    fn ordinary_excel_plain_text_remains_authoritative() {
        let html = r#"<html xmlns:x="urn:schemas-microsoft-com:office:excel"><body>
<table><tr><td> HTML value </td></tr></table></body></html>"#;
        let rich = RichData {
            html: Some(html.into()),
            ..Default::default()
        };
        let item = ClipboardItem::new_text(
            1,
            "  system value  ",
            ContentType::RichText,
            None,
            Some(&rich),
        );

        assert_eq!(plain_text_for_item(&item), "  system value  ");
    }
}
