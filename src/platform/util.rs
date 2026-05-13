//! Shared platform utilities.

/// Encode raw RGBA pixel data as a PNG byte vector.
pub fn encode_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    use image::{ImageEncoder, codecs::png::PngEncoder};
    let mut buf = Vec::new();
    let encoder = PngEncoder::new(&mut buf);
    encoder
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(buf)
}
