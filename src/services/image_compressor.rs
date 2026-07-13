//! Image compression for sync upload.
//!
//! Sync stores image blobs as PNG by default. When lossy compression is enabled,
//! opaque and sufficiently large images may be converted to JPEG if that is
//! smaller than the optimized PNG.

use image::{DynamicImage, GenericImageView, ImageFormat};
use std::io::Cursor;
use std::path::Path;

/// Result of compressing an image for sync.
pub struct CompressResult {
    pub data: Vec<u8>,
    /// File extension: "png" or "jpg".
    pub ext: String,
}

/// Compress an image file for sync upload.
pub fn compress_for_sync(image_path: &Path, lossy: bool) -> Result<CompressResult, String> {
    let source_data = std::fs::read(image_path).map_err(|e| format!("read image: {e}"))?;
    let img = image::load_from_memory(&source_data).map_err(|e| format!("decode image: {e}"))?;

    let png_data = if image::guess_format(&source_data).ok() == Some(ImageFormat::Png) {
        source_data
    } else {
        encode_png(&img)?
    };

    let optimized = apply_oxipng(&png_data).unwrap_or(png_data);

    if lossy {
        if let Some(jpeg) = try_convert_to_jpeg(&img, optimized.len()) {
            return Ok(CompressResult {
                data: jpeg,
                ext: "jpg".into(),
            });
        }
    }

    Ok(CompressResult {
        data: optimized,
        ext: "png".into(),
    })
}

fn apply_oxipng(data: &[u8]) -> Result<Vec<u8>, String> {
    let opts = oxipng::Options::from_preset(2);
    oxipng::optimize_from_memory(data, &opts).map_err(|e| format!("oxipng: {e}"))
}

fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    Ok(out.into_inner())
}

fn try_convert_to_jpeg(img: &DynamicImage, compare_len: usize) -> Option<Vec<u8>> {
    let (w, h) = img.dimensions();

    if w < 200 || h < 200 {
        return None;
    }

    if img.color().has_alpha() && img.pixels().any(|(_, _, p)| p.0[3] < 250) {
        return None;
    }

    let rgb = img.to_rgb8();
    let jpeg = mozjpeg_rs::Encoder::new(mozjpeg_rs::Preset::ProgressiveSmallest)
        .quality(85)
        .encode_rgb(&rgb, w, h)
        .ok()?;

    if jpeg.len() >= compare_len {
        return None;
    }

    Some(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};

    fn unique_temp_path(ext: &str) -> std::path::PathBuf {
        let name = format!(
            "clippi-image-compressor-{}.{ext}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        );
        std::env::temp_dir().join(name)
    }

    #[test]
    fn transparent_images_stay_png_even_when_lossy_enabled() {
        let path = unique_temp_path("png");
        let img = ImageBuffer::from_fn(16, 16, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([255u8, 0, 0, 128])
            } else {
                Rgba([0u8, 0, 255, 255])
            }
        });
        img.save(&path).expect("save png");

        let result = compress_for_sync(&path, true).expect("compress");

        assert_eq!(result.ext, "png");
        assert!(image::load_from_memory(&result.data).is_ok());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn non_png_input_is_normalized_for_sync() {
        let path = unique_temp_path("jpg");
        let img = ImageBuffer::from_pixel(24, 24, Rgb([24u8, 64, 96]));
        img.save(&path).expect("save jpg");

        let result = compress_for_sync(&path, false).expect("compress");

        assert_eq!(result.ext, "png");
        assert!(image::load_from_memory(&result.data).is_ok());

        let _ = std::fs::remove_file(path);
    }
}
