//! --- Color detection, parsing, normalization, and conversion. ---
//!
//! --- Recognizes HEX (#RGB, #RRGGBB, #RRGGBBAA, raw RRGGBB) and RGB ---
//! --- (rgb/rgba function, comma/space separated) formats. Normalizes ---
//! to a canonical 6-digit uppercase hex for deduplication.

/// Normalized RGB color value used for hashing and conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColorValue {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ColorValue {
    /// Normalized 6-digit uppercase hex string (no `#`), used as the hash source.
    pub fn to_hex_normalized(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// CSS hex format: `#RRGGBB`
    pub fn to_css_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// CSS rgb format: `rgb(R, G, B)`
    pub fn to_rgb(self) -> String {
        format!("rgb({}, {}, {})", self.r, self.g, self.b)
    }
}

/// Try to detect and parse a color from text.
/// Returns the normalized ColorValue if the text represents a color.
pub fn detect_color(text: &str) -> Option<ColorValue> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // --- Multi-line text is never a color ---
    if text.contains('\n') {
        return None;
    }
    // --- Try HEX patterns first (more specific) ---
    if let Some(c) = parse_hex(text) {
        return Some(c);
    }
    // --- Try RGB patterns ---
    if let Some(c) = parse_rgb(text) {
        return Some(c);
    }
    None
}

/// Check whether the original text is in HEX-like format.
/// Used by the UI to decide which conversion to offer.
pub fn is_hex_format(text: &str) -> bool {
    let text = text.trim();
    parse_hex(text).is_some()
}

// --- ── HEX parsing ── ---

fn parse_hex(text: &str) -> Option<ColorValue> {
    let text = text.trim();

    // --- #RGB or #RRGGBB or #RRGGBBAA ---
    if let Some(hex_part) = text.strip_prefix('#') {
        return parse_hex_digits(hex_part);
    }

    // --- Bare hex: exactly 6 hex digits (e.g., FF8000) ---
    if text.len() == 6
        && text.chars().all(|c| c.is_ascii_hexdigit())
        && !text.chars().all(|c| c.is_ascii_digit())
    {
        return parse_hex_digits(text);
    }

    // Bare hex: exactly 3 hex digits (e.g., F80) — ambiguous but treat as hex
    if text.len() == 3
        && text.chars().all(|c| c.is_ascii_hexdigit())
        && !text.chars().all(|c| c.is_ascii_digit())
    {
        return parse_hex_digits(text);
    }

    None
}

fn parse_hex_digits(hex: &str) -> Option<ColorValue> {
    let hex: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            Some(ColorValue { r, g, b })
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(ColorValue { r, g, b })
        }
        8 => {
            // #RRGGBBAA — parse alpha but ignore for now
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(ColorValue { r, g, b })
        }
        _ => None,
    }
}

// --- ── RGB parsing ── ---

fn parse_rgb(text: &str) -> Option<ColorValue> {
    let text = text.trim();

    // --- rgb(R, G, B) or rgba(R, G, B, A) ---
    if let Some(inner) = text
        .strip_prefix("rgb(")
        .or_else(|| text.strip_prefix("RGB("))
        .and_then(|s| s.strip_suffix(')'))
    {
        return parse_rgb_tuple(inner);
    }
    if let Some(inner) = text
        .strip_prefix("rgba(")
        .or_else(|| text.strip_prefix("RGBA("))
        .and_then(|s| s.strip_suffix(')'))
    {
        return parse_rgb_tuple(inner);
    }

    // --- Plain comma-separated: "255, 128, 0" ---
    if text.contains(',') && !text.contains('(') {
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() == 3 || parts.len() == 4 {
            let r = parse_u8_channel(parts[0])?;
            let g = parse_u8_channel(parts[1])?;
            let b = parse_u8_channel(parts[2])?;
            return Some(ColorValue { r, g, b });
        }
    }

    // --- Space-separated: "255 128 0" ---
    if text.contains(' ') && !text.contains(',') && !text.contains('(') {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() == 3 || parts.len() == 4 {
            let r = parse_u8_channel(parts[0])?;
            let g = parse_u8_channel(parts[1])?;
            let b = parse_u8_channel(parts[2])?;
            return Some(ColorValue { r, g, b });
        }
    }

    None
}

fn parse_rgb_tuple(inner: &str) -> Option<ColorValue> {
    // --- Split by comma or whitespace ---
    let parts: Vec<&str> = if inner.contains(',') {
        inner.split(',').collect()
    } else {
        inner.split_whitespace().collect()
    };
    if parts.len() < 3 {
        return None;
    }
    let r = parse_u8_channel(parts[0])?;
    let g = parse_u8_channel(parts[1])?;
    let b = parse_u8_channel(parts[2])?;
    Some(ColorValue { r, g, b })
}

fn parse_u8_channel(s: &str) -> Option<u8> {
    let s = s.trim();
    // --- Handle percentage: "50%" ---
    if let Some(pct) = s.strip_suffix('%') {
        let v: f32 = pct.trim().parse().ok()?;
        return Some((v / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    // --- Handle float: "0.5" ---
    if s.contains('.') {
        let v: f32 = s.parse().ok()?;
        return Some((v * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_6_digit() {
        let c = detect_color("#FF8000").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_hex_3_digit() {
        let c = detect_color("#F80").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 136);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_hex_lowercase() {
        let c = detect_color("#ff8000").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_rgb_function() {
        let c = detect_color("rgb(255, 128, 0)").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_rgba_function() {
        let c = detect_color("rgba(255, 128, 0, 0.5)").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_comma_separated() {
        let c = detect_color("255, 128, 0").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_space_separated() {
        let c = detect_color("255 128 0").unwrap();
        assert_eq!(c.r, 255);
        assert_eq!(c.g, 128);
        assert_eq!(c.b, 0);
    }

    #[test]
    fn test_normalized_dedup() {
        let c1 = detect_color("#FF0000").unwrap();
        let c2 = detect_color("rgb(255, 0, 0)").unwrap();
        assert_eq!(c1.to_hex_normalized(), c2.to_hex_normalized());
    }

    #[test]
    fn test_conversion_roundtrip() {
        let c = detect_color("#FF8000").unwrap();
        assert_eq!(c.to_css_hex(), "#FF8000");
        assert_eq!(c.to_rgb(), "rgb(255, 128, 0)");
    }

    #[test]
    fn test_is_hex_format() {
        assert!(is_hex_format("#FF8000"));
        assert!(is_hex_format("#F80"));
        assert!(!is_hex_format("rgb(255,128,0)"));
        assert!(!is_hex_format("255, 128, 0"));
    }

    #[test]
    fn test_non_color_text() {
        assert!(detect_color("hello world").is_none());
        assert!(detect_color("123456789012345").is_none());
        assert!(detect_color("#GGGGGG").is_none());
        assert!(detect_color("rgb(300, 0, 0)").is_none()); // 300 overflows u8
    }
}
