use std::path::Path;

use image::Luma;

/// Try to detect and decode a QR code from an image file.
/// Returns `Some(decoded_text)` on success, `None` if no QR code found.
/// Falls back to inverted detection for white-on-dark QR codes (e.g., Firefox share).
pub fn detect_qr(image_path: &Path) -> Result<Option<String>, String> {
    let img = image::open(image_path)
        .map_err(|e| format!("Failed to open image for QR detection: {e}"))?;
    let gray = img.to_luma8();

    // Normal pass: dark modules on light background
    if let Some(text) = try_decode(&gray) {
        return Ok(Some(text));
    }

    // Inverted pass: light modules on dark background (e.g., Firefox share QR)
    let inverted = image::ImageBuffer::from_fn(gray.width(), gray.height(), |x, y| {
        let p = gray.get_pixel(x, y);
        Luma([255u8.saturating_sub(p.0[0])])
    });
    if let Some(text) = try_decode(&inverted) {
        return Ok(Some(text));
    }

    Ok(None)
}

fn try_decode(gray: &image::GrayImage) -> Option<String> {
    let mut prepared = rqrr::PreparedImage::prepare(gray.clone());
    for grid in prepared.detect_grids() {
        if let Ok((_meta, content)) = grid.decode() {
            let text = content.trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}
