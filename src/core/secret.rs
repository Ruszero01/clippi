//! Secret (password / API key / token / private key) detection and
//! unified sensitive-content masking for display.
//!
//! Detection is a deterministic pure function. Masking produces structured
//! `SensitivePreviewPart` segments consumed by the shared `SensitiveText`
//! component — UI code never parses `****` strings again.
//!
//! # Safety & logging
//!
//! Never log the original text, prefix, suffix, or matched values.  Logs may
//! contain only the rule name, text length, and error category.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

// ── Rule version ────────────────────────────────────────────────────────────
/// Increment when detection rules change.  Used to invalidate preview caches
/// and to gate one-shot historical backfill.
#[allow(dead_code)]
pub const SECRET_DETECTION_RULE_VERSION: u16 = 1;

// ── Detection result types ──────────────────────────────────────────────────

/// Fine-grained secret category — used for detection strategy, testing, and
/// debug statistics only.  Never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    Password,
    ApiKey,
    AccessToken,
    PrivateKey,
    Credential,
}

/// Confidence level of a secret match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretConfidence {
    High,
    Medium,
}

/// One or more sensitive value ranges found in `text`.
///
/// Ranges use UTF-8 byte offsets and are sorted, deduplicated, and
/// non-overlapping.  Only the bytes inside those ranges should be masked.
#[derive(Debug, Clone)]
pub struct SecretMatch {
    pub kind: SecretKind,
    #[allow(dead_code)]
    pub confidence: SecretConfidence,
    /// Byte ranges of the sensitive value(s).
    pub value_ranges: Vec<std::ops::Range<usize>>,
}

// ── Unified masking types ───────────────────────────────────────────────────

pub const DEFAULT_MASK: &str = "****";

/// A single masked value: visible prefix, mask, visible suffix.
#[derive(Debug, Clone)]
pub struct MaskedValue {
    pub prefix: String,
    pub mask: &'static str,
    pub suffix: String,
}

/// A segment of a sensitive preview — either plain text or a masked value.
#[derive(Debug, Clone)]
pub enum SensitivePreviewPart {
    Plain(String),
    Masked(MaskedValue),
}

/// How many leading / trailing characters to keep visible.
#[derive(Debug, Clone, Copy)]
pub struct MaskRule {
    pub visible_prefix_chars: usize,
    pub visible_suffix_chars: usize,
}

// ── Deterministic secret detection ──────────────────────────────────────────

/// Detect whether `text` contains a secret, and if so, which byte ranges
/// should be masked.  This is a **pure function**: no I/O, no system time,
/// no global mutable state, no randomisation.  The same input on the same
/// rule version always returns the same result.
pub fn detect_secret(text: &str) -> Option<SecretMatch> {
    if text.trim().is_empty() {
        return None;
    }

    // ── 1. Known key formats (vendor prefixes) ──────────────────────────
    if let Some(m) = detect_known_key_format(text) {
        return Some(m);
    }

    // ── 2. Auth headers ─────────────────────────────────────────────────
    if let Some(m) = detect_auth_header(text) {
        return Some(m);
    }

    // ── 3. Private key blocks ───────────────────────────────────────────
    if let Some(m) = detect_private_key_block(text) {
        return Some(m);
    }

    // ── 4. Field-based patterns (password=..., api_key: ..., etc.) ──────
    let field_matches = detect_field_secrets(text);
    if !field_matches.is_empty() {
        let kind = if text.contains("private_key") || text.contains("PRIVATE KEY") {
            SecretKind::PrivateKey
        } else if text.contains("token") || text.contains("access_key") {
            SecretKind::AccessToken
        } else {
            SecretKind::ApiKey
        };
        return Some(SecretMatch {
            kind,
            confidence: SecretConfidence::High,
            value_ranges: field_matches,
        });
    }

    // ── 5. Standalone password-like strings ────────────────────────────
    if let Some(m) = detect_standalone_password(text) {
        return Some(m);
    }

    None
}

// ── Pre-compiled regex sets ─────────────────────────────────────────────────

static KNOWN_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        ^
        (?:
            # GitHub
            ghp_[A-Za-z0-9_]{36,255}
            |gho_[A-Za-z0-9_]{36,255}
            |ghu_[A-Za-z0-9_]{36,255}
            |ghs_[A-Za-z0-9_]{36,255}
            |ghr_[A-Za-z0-9_]{36,255}
            |github_pat_[A-Za-z0-9_]{22,255}
            # OpenAI
            |sk-(?:proj-[A-Za-z0-9_-]+-)?[A-Za-z0-9_-]{20,255}
            # Stripe (secret / restricted only; pk_ is excluded below)
            |(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{24,255}
            # Slack
            |xox[abprs]-[0-9]+-[0-9]+-[A-Za-z0-9]+
            # Google
            |AIza[0-9A-Za-z_-]{35}
            # AWS Access Key ID
            |(?:AKIA|ASIA)[A-Z0-9]{16}
            # JWT — three base64url segments
            |[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+
        )
        $
    ",
    )
    .expect("known key format regex")
});

// Explicit secret fields in env, YAML, and JSON-like configuration.
static FIELD_SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?imx)
        (?:^|[,\{]\s*)
        (?:export\s+)?
        ["']?
        (?P<key>
            password|passwd|pwd|passphrase
            |secret|client_secret
            |api_key|apikey
            |access_key|secret_key
            |access_token|auth_token|refresh_token
            |private_key
        )
        ["']?
        \s*[:=]\s*
        (?:
            "(?P<double>[^"\r\n]*)"
            |'(?P<single>[^'\r\n]*)'
            |(?P<bare>[^,\}\r\n]+)
        )
    "#,
    )
    .expect("field secret regex")
});

static AUTH_BEARER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:Authorization:\s*)?Bearer\s+(?P<token>[A-Za-z0-9\-._~+/]+=*)")
        .expect("auth bearer regex")
});

static AUTH_BASIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:Authorization:\s*)?Basic\s+(?P<credentials>[A-Za-z0-9+/]+=*)")
        .expect("auth basic regex")
});

static PRIVATE_KEY_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)-----BEGIN\s+(?:[A-Z]+\s+)?PRIVATE\s+KEY-----\s*.*?\s*-----END\s+(?:[A-Z]+\s+)?PRIVATE\s+KEY-----",
    )
    .expect("private key block regex")
});

// ── Detection sub-functions ─────────────────────────────────────────────────

fn detect_known_key_format(text: &str) -> Option<SecretMatch> {
    let candidate = text.trim();
    if candidate.contains(['\r', '\n']) {
        return None;
    }
    let candidate_start = text.find(candidate)?;

    if let Some(m) = KNOWN_KEY_RE.find(candidate) {
        let matched_range = candidate_start + m.start()..candidate_start + m.end();
        // Exclude Stripe public keys
        if candidate.starts_with("pk_") {
            return None;
        }
        // JWT: validate header decodes as JSON
        if candidate.contains('.') && candidate.matches('.').count() == 2 {
            if !is_plausible_jwt(candidate) {
                return None;
            }
            return Some(SecretMatch {
                kind: SecretKind::AccessToken,
                confidence: SecretConfidence::High,
                value_ranges: vec![matched_range],
            });
        }

        let kind = classify_known_prefix(candidate);
        return Some(SecretMatch {
            kind,
            confidence: SecretConfidence::High,
            value_ranges: vec![matched_range],
        });
    }
    None
}

fn classify_known_prefix(s: &str) -> SecretKind {
    if s.starts_with("ghp_")
        || s.starts_with("gho_")
        || s.starts_with("ghu_")
        || s.starts_with("ghs_")
        || s.starts_with("ghr_")
        || s.starts_with("github_pat_")
    {
        SecretKind::AccessToken
    } else if s.starts_with("sk-") || s.starts_with("sk_") || s.starts_with("rk_") {
        SecretKind::ApiKey
    } else if s.starts_with("xox") {
        SecretKind::AccessToken
    } else if s.starts_with("AIza") {
        SecretKind::ApiKey
    } else if s.starts_with("AKIA") || s.starts_with("ASIA") {
        SecretKind::Credential
    } else {
        SecretKind::ApiKey
    }
}

/// Quick JWT plausibility check: does the header decode to a JSON object?
fn is_plausible_jwt(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(3, '.').collect();
    if parts.len() != 3 {
        return false;
    }
    // Base64url-decode the header
    let header = parts[0];
    if let Ok(decoded) = base64_url_decode(header) {
        // Minimal check: starts with '{' and contains '"'
        let h = String::from_utf8_lossy(&decoded);
        return h.trim_start().starts_with('{') && h.contains('"');
    }
    false
}

fn base64_url_decode(s: &str) -> Result<Vec<u8>, ()> {
    // Convert base64url → standard base64
    let mut b64 = s.replace('-', "+").replace('_', "/");
    // Pad
    let pad = (4 - (b64.len() % 4)) % 4;
    b64.push_str(&"=".repeat(pad));
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|_| ())
}

fn detect_auth_header(text: &str) -> Option<SecretMatch> {
    // Bearer
    if let Some(caps) = AUTH_BEARER_RE.captures(text) {
        if let Some(m) = caps.name("token") {
            return Some(SecretMatch {
                kind: SecretKind::AccessToken,
                confidence: SecretConfidence::High,
                value_ranges: vec![m.range()],
            });
        }
    }
    // Basic
    if let Some(caps) = AUTH_BASIC_RE.captures(text) {
        if let Some(m) = caps.name("credentials") {
            return Some(SecretMatch {
                kind: SecretKind::Credential,
                confidence: SecretConfidence::High,
                value_ranges: vec![m.range()],
            });
        }
    }
    None
}

fn detect_private_key_block(text: &str) -> Option<SecretMatch> {
    if let Some(m) = PRIVATE_KEY_BLOCK_RE.find(text) {
        return Some(SecretMatch {
            kind: SecretKind::PrivateKey,
            confidence: SecretConfidence::High,
            // Full block — will be summarised to "PRIVATE KEY · ****"
            value_ranges: vec![m.range()],
        });
    }
    None
}

fn detect_field_secrets(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for caps in FIELD_SECRET_RE.captures_iter(text) {
        let Some(m) = caps
            .name("double")
            .or_else(|| caps.name("single"))
            .or_else(|| caps.name("bare"))
        else {
            continue;
        };
        let raw = m.as_str();
        let trimmed = raw.trim();
        if is_secret_placeholder(trimmed) {
            continue;
        }
        let leading = raw.len() - raw.trim_start().len();
        let trailing = raw.len() - raw.trim_end().len();
        ranges.push(m.start() + leading..m.end() - trailing);
    }
    // Sort, dedup, merge overlapping
    sort_and_merge_ranges(&mut ranges);
    ranges
}

/// Explicit non-secret placeholders do not contain a credential value.
fn is_secret_placeholder(s: &str) -> bool {
    let s = s.trim();
    s.is_empty()
        || s.eq_ignore_ascii_case("true")
        || s.eq_ignore_ascii_case("false")
        || s.eq_ignore_ascii_case("null")
        || s.eq_ignore_ascii_case("none")
        || s.starts_with('$')
}

fn detect_standalone_password(text: &str) -> Option<SecretMatch> {
    let t = text.trim();
    // Must be single-line, no internal whitespace
    let value_start = text.find(t)?;
    if t.contains('\n') || t.contains('\r') || t.contains(' ') || t.contains('\t') {
        return None;
    }
    let char_count = t.chars().count();
    if !(12..=256).contains(&char_count) {
        return None;
    }
    // Character class diversity
    let has_upper = t.chars().any(|c| c.is_uppercase());
    let has_lower = t.chars().any(|c| c.is_lowercase());
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let has_symbol = t
        .chars()
        .any(|c| c.is_ascii_punctuation() && c != '_' && c != '-');
    let class_count = [has_upper, has_lower, has_digit, has_symbol]
        .iter()
        .filter(|&&x| x)
        .count();
    if class_count < 3 {
        return None;
    }
    // Reject known non-secret patterns
    if looks_like_url(t)
        || looks_like_email(t)
        || looks_like_phone(t)
        || looks_like_hash(t)
        || looks_like_uuid(t)
        || is_repeating(t)
        || looks_like_file_path(t)
    {
        return None;
    }
    // Simple entropy check: unique chars / total chars
    let unique: HashSet<char> = t.chars().collect();
    let entropy_ratio = unique.len() as f64 / char_count as f64;
    if entropy_ratio < 0.3 {
        return None;
    }

    #[allow(clippy::single_range_in_vec_init)]
    Some(SecretMatch {
        kind: SecretKind::Password,
        confidence: SecretConfidence::Medium,
        value_ranges: vec![value_start..value_start + t.len()],
    })
}

fn looks_like_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("ftp://")
        || s.starts_with("sftp://")
        || s.contains("://")
        || s.starts_with("www.")
}

fn looks_like_email(s: &str) -> bool {
    s.contains('@') && s.contains('.')
}

fn looks_like_phone(s: &str) -> bool {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.len() >= 7 && digits.len() <= 15
}

fn looks_like_hash(s: &str) -> bool {
    // Common hex hashes (MD5: 32, SHA1: 40, SHA256: 64)
    let len = s.len();
    if (len == 32 || len == 40 || len == 64) && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    // Git commit SHA
    if (len == 7 || len == 8 || len == 40) && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
}

fn looks_like_uuid(s: &str) -> bool {
    // e.g. 550e8400-e29b-41d4-a716-446655440000
    s.len() == 36 && s.chars().filter(|&c| c == '-').count() == 4
}

fn is_repeating(s: &str) -> bool {
    if s.len() < 4 {
        return false;
    }
    let chars: Vec<char> = s.chars().collect();
    // Check if > 50% of characters are the same
    let mut counts = std::collections::HashMap::new();
    for &c in &chars {
        *counts.entry(c).or_insert(0) += 1;
    }
    let max_count = counts.values().max().copied().unwrap_or(0);
    if max_count as f64 / chars.len() as f64 > 0.5 {
        return true;
    }
    // Check for repeated short pattern (e.g., "abcabcabc")
    for period in 2..=4 {
        if chars.len().is_multiple_of(period) {
            let pattern = &chars[..period];
            if chars.chunks(period).all(|chunk| chunk == pattern) {
                return true;
            }
        }
    }
    false
}

/// Detect strings that look like file paths (Windows / Unix / UNC).
/// Presence of directory separators (`\` or `/`) plus a file extension
/// is a strong signal that this is not a password.
fn looks_like_file_path(s: &str) -> bool {
    // Windows absolute path: C:\... or D:/...
    if s.len() >= 3
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.as_bytes()[1] == b':'
        && (s.as_bytes()[2] == b'\\' || s.as_bytes()[2] == b'/')
    {
        return true;
    }
    // UNC path: \\server\share\...
    if s.starts_with("\\\\") {
        return true;
    }
    // Unix absolute path: /abs/path (must contain another /)
    if s.starts_with('/') && s.len() >= 3 && s[1..].contains('/') {
        return true;
    }
    // Any string with directory separators and a file extension is likely a path.
    // File extension: a period followed by 1-10 alphanumeric chars near the end.
    if s.contains('\\') || s.contains('/') {
        if let Some(dot) = s.rfind('.') {
            let ext = &s[dot + 1..];
            if !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return true;
            }
        }
    }
    false
}

fn sort_and_merge_ranges(ranges: &mut Vec<std::ops::Range<usize>>) {
    if ranges.len() <= 1 {
        return;
    }
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    let mut cur = ranges[0].clone();
    for r in &ranges[1..] {
        if r.start <= cur.end {
            cur.end = cur.end.max(r.end);
        } else {
            merged.push(cur);
            cur = r.clone();
        }
    }
    merged.push(cur);
    *ranges = merged;
}

// ── URL credential extraction ───────────────────────────────────────────────

/// URL query keys whose values must never be shown in previews.
const SENSITIVE_URL_QUERY_KEYS: &[&str] = &[
    "token",
    "access_token",
    "api_key",
    "apikey",
    "password",
    "passwd",
    "secret",
    "key",
    "auth",
    "api-key",
];

/// Parse both absolute and protocol-less web URLs without exposing userinfo.
fn parse_web_url(text: &str) -> Option<url::Url> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = url::Url::parse(trimmed) {
        if parsed.host_str().is_some() {
            return Some(parsed);
        }
    }
    if trimmed.contains("://") {
        return None;
    }
    url::Url::parse(&format!("https://{trimmed}"))
        .ok()
        .filter(|parsed| parsed.host_str().is_some())
}

/// Extract sensitive value ranges from a URL (userinfo password + sensitive
/// query params). Ranges always refer to the original, unmodified input.
pub fn url_sensitive_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    if parse_web_url(text).is_none() {
        return Vec::new();
    }

    let trimmed = text.trim();
    let base_offset = text.len() - text.trim_start().len();
    let authority_start = trimmed.find("://").map_or(0, |index| index + 3);
    let authority_end = trimmed[authority_start..]
        .find(['/', '?', '#'])
        .map_or(trimmed.len(), |index| authority_start + index);
    let authority = &trimmed[authority_start..authority_end];
    let mut ranges = Vec::new();

    // Userinfo uses the last @ delimiter; only the password portion is masked.
    if let Some(at) = authority.rfind('@') {
        let userinfo = &authority[..at];
        if let Some(colon) = userinfo.find(':') {
            let password_start = authority_start + colon + 1;
            let password_end = authority_start + at;
            if password_start < password_end {
                ranges.push(base_offset + password_start..base_offset + password_end);
            }
        }
    }

    // Keep raw byte offsets so percent-encoded and duplicate values are masked
    // exactly where they occur in the original URL.
    if let Some(question) = trimmed.find('?') {
        let query_end = trimmed[question + 1..]
            .find('#')
            .map_or(trimmed.len(), |index| question + 1 + index);
        let query = &trimmed[question + 1..query_end];
        let mut pair_offset = question + 1;
        for pair in query.split('&') {
            if let Some(equals) = pair.find('=') {
                let raw_key = &pair[..equals];
                let decoded_key = url::form_urlencoded::parse(raw_key.as_bytes())
                    .next()
                    .map(|(key, _)| key.into_owned())
                    .unwrap_or_else(|| raw_key.to_string());
                let value_start = pair_offset + equals + 1;
                let value_end = pair_offset + pair.len();
                if value_start < value_end
                    && SENSITIVE_URL_QUERY_KEYS
                        .iter()
                        .any(|key| decoded_key.eq_ignore_ascii_case(key))
                {
                    ranges.push(base_offset + value_start..base_offset + value_end);
                }
            }
            pair_offset += pair.len() + 1;
        }
    }

    sort_and_merge_ranges(&mut ranges);
    ranges
}

/// Return a clean host (no userinfo, port, path, or query) for favicon and display.
pub fn url_clean_host(text: &str) -> Option<String> {
    parse_web_url(text).and_then(|url| url.host_str().map(str::to_string))
}
// ── Core masking algorithm ──────────────────────────────────────────────────

/// Mask the middle of `value`, keeping `rule.visible_prefix_chars` at the
/// start and `rule.visible_suffix_chars` at the end.  Short values receive
/// a safe fallback.
pub fn mask_middle(value: &str, rule: MaskRule) -> MaskedValue {
    // Use Unicode chars, not bytes
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();

    if len == 0 {
        return MaskedValue {
            prefix: String::new(),
            mask: DEFAULT_MASK,
            suffix: String::new(),
        };
    }

    // Safe fallback for short values (≤ 7 chars): only show mask
    if len <= 7 {
        return MaskedValue {
            prefix: String::new(),
            mask: DEFAULT_MASK,
            suffix: String::new(),
        };
    }

    let prefix_len = rule.visible_prefix_chars.min(len);
    let suffix_len = rule.visible_suffix_chars.min(len - prefix_len);

    let prefix: String = chars[..prefix_len].iter().collect();
    let suffix: String = chars[len - suffix_len..].iter().collect();

    MaskedValue {
        prefix,
        mask: DEFAULT_MASK,
        suffix,
    }
}

// ── Unified preview parts ───────────────────────────────────────────────────

/// Produce structured preview segments for a given text and meta_type.
///
/// - `secret`: rebuilds only structured sub-ranges; standalone values take a
///   regex-free render path.
/// - `link`: extracts URL credential/query ranges via `url_sensitive_ranges`.
/// - `email` / `phone`: use structured rules (not `****` parsing).
/// - all others: single `Plain` segment.
pub fn sensitive_preview_parts(text: &str, meta_type: &str) -> Vec<SensitivePreviewPart> {
    match meta_type {
        "secret" => {
            if let Some(secret) = rebuild_secret_preview_match(text) {
                if secret.value_ranges.is_empty() {
                    return vec![SensitivePreviewPart::Masked(mask_middle(
                        text,
                        MaskRule {
                            visible_prefix_chars: 3,
                            visible_suffix_chars: 4,
                        },
                    ))];
                }
                // For private key blocks: whole-segment summary
                if secret.kind == SecretKind::PrivateKey {
                    return vec![SensitivePreviewPart::Masked(MaskedValue {
                        prefix: "PRIVATE KEY · ".to_string(),
                        mask: DEFAULT_MASK,
                        suffix: String::new(),
                    })];
                }
                return build_range_masked_parts(text, &secret.value_ranges);
            }
            // Can't rebuild ranges → mask whole thing
            vec![SensitivePreviewPart::Masked(mask_middle(
                text,
                MaskRule {
                    visible_prefix_chars: 3,
                    visible_suffix_chars: 4,
                },
            ))]
        }
        "link" => {
            let ranges = url_sensitive_ranges(text);
            if ranges.is_empty() {
                vec![SensitivePreviewPart::Plain(text.to_string())]
            } else {
                build_range_masked_parts(text, &ranges)
            }
        }
        "email" => email_preview_parts(text),
        "phone" => phone_preview_parts(text),
        _ => vec![SensitivePreviewPart::Plain(text.to_string())],
    }
}

/// Rebuild only the ranges needed to display an item that has already been
/// classified as secret. This deliberately avoids the full detector on GPUI's
/// render path.
#[allow(clippy::single_range_in_vec_init)]
fn rebuild_secret_preview_match(text: &str) -> Option<SecretMatch> {
    if text.trim().is_empty() {
        return None;
    }

    if text.contains("PRIVATE KEY") || text.contains("private_key") {
        return Some(SecretMatch {
            kind: SecretKind::PrivateKey,
            confidence: SecretConfidence::High,
            value_ranges: vec![0..text.len()],
        });
    }

    let value_start = text.len() - text.trim_start().len();
    let value_end = text.trim_end().len();
    let may_have_structured_prefix = text[value_start..value_end]
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, ':' | '=' | '{' | ','));
    if !may_have_structured_prefix {
        return Some(SecretMatch {
            kind: SecretKind::Password,
            confidence: SecretConfidence::High,
            value_ranges: vec![value_start..value_end],
        });
    }

    if let Some(secret) = detect_auth_header(text) {
        return Some(secret);
    }

    let value_ranges = detect_field_secrets(text);
    if !value_ranges.is_empty() {
        return Some(SecretMatch {
            kind: SecretKind::Credential,
            confidence: SecretConfidence::High,
            value_ranges,
        });
    }

    None
}

/// Flatten structured sensitive parts for text-only preview surfaces.
pub fn sensitive_preview_to_text(text: &str, meta_type: &str) -> String {
    let mut result = String::new();
    for part in sensitive_preview_parts(text, meta_type) {
        match part {
            SensitivePreviewPart::Plain(value) => result.push_str(&value),
            SensitivePreviewPart::Masked(value) => {
                result.push_str(&value.prefix);
                result.push_str(value.mask);
                result.push_str(&value.suffix);
            }
        }
    }
    if result.is_empty() {
        DEFAULT_MASK.to_string()
    } else {
        result
    }
}
/// Split `text` into Plain / Masked segments based on `ranges`.
fn build_range_masked_parts(
    text: &str,
    ranges: &[std::ops::Range<usize>],
) -> Vec<SensitivePreviewPart> {
    let mut parts = Vec::new();
    let mut cursor = 0;
    // Validate all ranges are on char boundaries
    for r in ranges {
        if !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
            // Safety: skip invalid ranges, mask whole text
            return vec![SensitivePreviewPart::Masked(mask_middle(
                text,
                MaskRule {
                    visible_prefix_chars: 3,
                    visible_suffix_chars: 4,
                },
            ))];
        }
    }
    for r in ranges {
        if r.start > cursor {
            parts.push(SensitivePreviewPart::Plain(
                text[cursor..r.start].to_string(),
            ));
        }
        let value = &text[r.start..r.end];
        parts.push(SensitivePreviewPart::Masked(mask_middle(
            value,
            MaskRule {
                visible_prefix_chars: 3,
                visible_suffix_chars: 4,
            },
        )));
        cursor = r.end;
    }
    if cursor < text.len() {
        parts.push(SensitivePreviewPart::Plain(text[cursor..].to_string()));
    }
    parts
}

fn email_preview_parts(text: &str) -> Vec<SensitivePreviewPart> {
    if let Some(at) = text.find('@') {
        let local = &text[..at];
        let domain = &text[at..];
        let local_chars: Vec<char> = local.chars().collect();
        let masked_local = if local_chars.len() <= 3 {
            // Short local part: show only first char + mask
            let first: String = local_chars
                .first()
                .map(|c| c.to_string())
                .unwrap_or_default();
            format!("{}****", first)
        } else {
            let prefix: String = local_chars[..3].iter().collect();
            format!("{}****", prefix)
        };
        let display_text = format!("{}{}", masked_local, domain);
        // Split on **** for styled rendering
        if let Some(pos) = display_text.find(DEFAULT_MASK) {
            let prefix = display_text[..pos].to_string();
            let suffix = display_text[pos + DEFAULT_MASK.len()..].to_string();
            return vec![SensitivePreviewPart::Masked(MaskedValue {
                prefix,
                mask: DEFAULT_MASK,
                suffix,
            })];
        }
    }
    vec![SensitivePreviewPart::Plain(text.to_string())]
}

fn phone_preview_parts(text: &str) -> Vec<SensitivePreviewPart> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = cleaned.chars().collect();
    if chars.len() <= 7 {
        // Short phone: only show mask (security fix from old behavior)
        return vec![SensitivePreviewPart::Masked(MaskedValue {
            prefix: String::new(),
            mask: DEFAULT_MASK,
            suffix: String::new(),
        })];
    }
    let prefix: String = chars[..3].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    vec![SensitivePreviewPart::Masked(MaskedValue {
        prefix,
        mask: DEFAULT_MASK,
        suffix,
    })]
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── mask_middle ─────────────────────────────────────────────────────

    #[test]
    fn mask_middle_normal() {
        let mv = mask_middle(
            "sk-proj-abcdefghijklmnop",
            MaskRule {
                visible_prefix_chars: 3,
                visible_suffix_chars: 4,
            },
        );
        assert_eq!(mv.prefix, "sk-");
        assert_eq!(mv.mask, "****");
        assert_eq!(mv.suffix, "mnop");
    }

    #[test]
    fn mask_middle_short_value() {
        let mv = mask_middle(
            "abc123",
            MaskRule {
                visible_prefix_chars: 3,
                visible_suffix_chars: 4,
            },
        );
        assert_eq!(mv.prefix, "");
        assert_eq!(mv.mask, "****");
        assert_eq!(mv.suffix, "");
    }

    #[test]
    fn mask_middle_exactly_seven() {
        let mv = mask_middle(
            "1234567",
            MaskRule {
                visible_prefix_chars: 3,
                visible_suffix_chars: 4,
            },
        );
        assert_eq!(mv.prefix, "");
        assert_eq!(mv.mask, "****");
        assert_eq!(mv.suffix, "");
    }

    #[test]
    fn mask_middle_eight() {
        let mv = mask_middle(
            "12345678",
            MaskRule {
                visible_prefix_chars: 3,
                visible_suffix_chars: 4,
            },
        );
        assert_eq!(mv.prefix, "123");
        assert_eq!(mv.suffix, "5678");
    }

    #[test]
    fn mask_middle_unicode() {
        let mv = mask_middle(
            "密码abcdefgh测试",
            MaskRule {
                visible_prefix_chars: 3,
                visible_suffix_chars: 4,
            },
        );
        assert_eq!(mv.prefix, "密码a");
        assert_eq!(mv.mask, "****");
        assert_eq!(mv.suffix, "gh测试");
    }

    // ── detect_secret: known key formats ─────────────────────────────────

    #[test]
    fn detect_github_token() {
        let m = detect_secret("ghp_abcdefghijklmnopqrstuvwxyz1234567890ABCD").unwrap();
        assert_eq!(m.kind, SecretKind::AccessToken);
        assert_eq!(m.confidence, SecretConfidence::High);
    }

    #[test]
    fn detect_openai_key() {
        let m = detect_secret("sk-proj-myproject-abcdefghijklmnopqrstuvwxyz1234567890AB").unwrap();
        assert_eq!(m.kind, SecretKind::ApiKey);
    }

    #[test]
    fn detect_openai_short_key() {
        let m = detect_secret("sk-abcdefghijklmnopqrstuvw").unwrap();
        assert_eq!(m.kind, SecretKind::ApiKey);
    }

    #[test]
    fn detect_google_api_key() {
        // AIza + exactly 35 chars = 39 total
        let m = detect_secret("AIzaSyD8J7F9kL3mN4pQ6rT0vW2xY5zA8bC1dE3").unwrap();
        assert_eq!(m.kind, SecretKind::ApiKey);
    }

    #[test]
    fn detect_aws_access_key() {
        let m = detect_secret("AKIAIOSFODNN7EXAMPLE").unwrap();
        assert_eq!(m.kind, SecretKind::Credential);
    }

    #[test]
    fn detect_slack_token() {
        let m = detect_secret(concat!(
            "xoxb-",
            "1234567890-",
            "123456789012-",
            "abcdefghijklmnopqrstuvwx"
        ))
        .unwrap();
        assert_eq!(m.kind, SecretKind::AccessToken);
    }

    #[test]
    fn detect_stripe_secret_key() {
        let m = detect_secret(concat!("sk_live_", "abcdefghijklmnopqrstuvwx")).unwrap();
        assert_eq!(m.kind, SecretKind::ApiKey);
    }

    #[test]
    fn reject_stripe_public_key() {
        assert!(detect_secret("pk_live_abcdefghijklmnopqrstuvwx").is_none());
    }

    #[test]
    fn known_key_ranges_use_original_coordinates() {
        let token = "sk-abcdefghijklmnopqrstuvw";
        let text = format!("  {token}  ");
        let detected = detect_secret(&text).unwrap();
        assert_eq!(&text[detected.value_ranges[0].clone()], token);
        assert_eq!(
            sensitive_preview_to_text(&text, "secret"),
            "  sk-****tuvw  "
        );
    }

    #[test]
    fn split_known_key_is_not_reassembled() {
        assert!(detect_secret("sk-abcdefghij\nklmnopqrstuvw").is_none());
    }
    #[test]

    fn detect_jwt() {
        let m = detect_secret("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U").unwrap();
        assert_eq!(m.kind, SecretKind::AccessToken);
    }

    #[test]
    fn reject_invalid_jwt_header() {
        // Header that doesn't decode to JSON
        assert!(detect_secret(
            "!!!!!.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
        )
        .is_none());
    }

    // ── detect_secret: auth headers ─────────────────────────────────────

    #[test]
    fn detect_bearer_token() {
        let m = detect_secret(
            "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signature",
        )
        .unwrap();
        assert_eq!(m.confidence, SecretConfidence::High);
    }

    #[test]
    fn detect_bearer_without_prefix() {
        let m = detect_secret("Bearer abcdefghijklmnopqrstuvwxyz123456").unwrap();
        assert_eq!(m.confidence, SecretConfidence::High);
    }

    #[test]
    fn detect_basic_auth() {
        let m = detect_secret("Authorization: Basic dXNlcjpwYXNzd29yZA==").unwrap();
        assert_eq!(m.kind, SecretKind::Credential);
    }

    // ── detect_secret: field patterns ───────────────────────────────────

    #[test]
    fn detect_password_field() {
        let m = detect_secret("password=my-secret-password-123").unwrap();
        assert_eq!(m.confidence, SecretConfidence::High);
    }

    #[test]
    fn detect_api_key_field() {
        let m = detect_secret("API_KEY=sk-abcdefghijklmnopqrstuvwx").unwrap();
        assert_eq!(m.confidence, SecretConfidence::High);
    }

    #[test]
    fn detect_env_file() {
        let text = "API_KEY=sk-test-key-12345\nPASSWORD=\"correct-horse-battery-staple\"\nREGION=ap-east-1";
        let m = detect_secret(text).unwrap();
        // Should have 2 ranges (API_KEY value and PASSWORD value), not REGION
        assert!(m.value_ranges.len() >= 2);
    }

    #[test]
    fn detect_json_secret() {
        let text = r#"{"access_token":"token-value-123","password":"abc1"}"#;
        let detected = detect_secret(text).unwrap();
        assert_eq!(detected.confidence, SecretConfidence::High);
        assert_eq!(detected.value_ranges.len(), 2);
        assert_eq!(&text[detected.value_ranges[0].clone()], "token-value-123");
        assert_eq!(&text[detected.value_ranges[1].clone()], "abc1");
    }

    #[test]
    fn detect_short_and_path_like_explicit_passwords() {
        for text in ["password=1234", r#"{"password":"a/b\\c"}"#] {
            let detected = detect_secret(text).unwrap();
            assert_eq!(detected.value_ranges.len(), 1);
        }
    }
    // ── detect_secret: private key blocks ───────────────────────────────

    #[test]
    fn detect_pem_private_key() {
        let text = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCtest\n-----END PRIVATE KEY-----";
        let m = detect_secret(text).unwrap();
        assert_eq!(m.kind, SecretKind::PrivateKey);
    }

    #[test]
    fn detect_rsa_private_key() {
        let text =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEAtest\n-----END RSA PRIVATE KEY-----";
        let m = detect_secret(text).unwrap();
        assert_eq!(m.kind, SecretKind::PrivateKey);
    }

    // ── detect_secret: standalone passwords ─────────────────────────────

    #[test]
    fn detect_strong_password() {
        let m = detect_secret("C0rrect-H0rse-Battery-Staple!").unwrap();
        assert_eq!(m.kind, SecretKind::Password);
        assert_eq!(m.confidence, SecretConfidence::Medium);
    }

    #[test]
    fn reject_short_password() {
        assert!(detect_secret("Abc123!").is_none());
    }

    #[test]
    fn reject_repeating_chars() {
        assert!(detect_secret("aaaaaaaaaaaaaaa").is_none());
    }

    #[test]
    fn reject_uuid() {
        assert!(detect_secret("550e8400-e29b-41d4-a716-446655440000").is_none());
    }

    #[test]
    fn reject_sha256_hash() {
        assert!(
            detect_secret("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .is_none()
        );
    }

    #[test]
    fn reject_url() {
        assert!(detect_secret("https://example.com/path/to/resource").is_none());
    }

    // ── URL credential ranges ───────────────────────────────────────────

    #[test]
    fn url_userinfo_password_range() {
        let ranges = url_sensitive_ranges("https://user:secret123@example.com/path");
        assert!(!ranges.is_empty());
        // The password "secret123" should be in the ranges
        let has_password = ranges.iter().any(|r| {
            let s = &"https://user:secret123@example.com/path"[r.clone()];
            s == "secret123"
        });
        assert!(has_password);
    }

    #[test]
    fn url_sensitive_query_param() {
        let ranges =
            url_sensitive_ranges("https://api.example.com/v1?token=mysecret&other=visible");
        assert!(!ranges.is_empty());
    }

    #[test]
    fn url_no_sensitive_parts() {
        let ranges = url_sensitive_ranges("https://example.com/path?page=1&sort=asc");
        assert!(ranges.is_empty());
    }

    #[test]
    fn url_clean_host_strips_userinfo() {
        let host = url_clean_host("https://user:pass@example.com/path");
        assert_eq!(host, Some("example.com".to_string()));
    }

    #[test]
    fn url_ranges_cover_encoded_duplicate_and_case_insensitive_keys() {
        let text = "example.com/path?TOKEN=first-value&token=sec%72et-value&other=x#frag";
        let ranges = url_sensitive_ranges(text);
        assert_eq!(ranges.len(), 2);
        assert_eq!(&text[ranges[0].clone()], "first-value");
        assert_eq!(&text[ranges[1].clone()], "sec%72et-value");
        assert_eq!(
            sensitive_preview_to_text(text, "link"),
            "example.com/path?TOKEN=fir****alue&token=sec****alue&other=x#frag"
        );
    }

    #[test]
    fn protocol_less_url_host_strips_userinfo() {
        assert_eq!(
            url_clean_host("user:pass@example.com/path"),
            Some("example.com".to_string())
        );
        assert_eq!(
            sensitive_preview_to_text("user:pass@example.com/path", "link"),
            "user:****@example.com/path"
        );
    }
    // ── sensitive_preview_parts ─────────────────────────────────────────

    #[test]
    fn secret_preview_parts_openai() {
        let parts = sensitive_preview_parts(
            "sk-proj-test-abcdefghijklmnopqrstuvwxyz1234567890AB",
            "secret",
        );
        // Should have a Masked part with prefix "sk-" and suffix "90AB"
        assert!(matches!(
            parts.first(),
            Some(SensitivePreviewPart::Masked(_))
        ));
    }

    #[test]
    fn secret_fallback_masks_whole() {
        // A random string that doesn't match any rule, but meta_type is "secret"
        let parts = sensitive_preview_parts("some-random-string-here", "secret");
        assert!(matches!(
            parts.first(),
            Some(SensitivePreviewPart::Masked(_))
        ));
    }

    #[test]
    fn structured_secret_preview_keeps_only_the_field_value_masked() {
        assert_eq!(
            sensitive_preview_to_text("password=short", "secret"),
            "password=****"
        );
    }

    #[test]
    fn auth_header_preview_keeps_the_header_visible() {
        assert_eq!(
            sensitive_preview_to_text("Authorization: Bearer abcdefghijklmnop", "secret"),
            "Authorization: Bearer abc****mnop"
        );
    }

    #[test]
    fn private_key_preview_always_uses_summary() {
        let text = "-----BEGIN PRIVATE KEY-----\nshort\n-----END PRIVATE KEY-----";
        assert_eq!(
            sensitive_preview_to_text(text, "secret"),
            "PRIVATE KEY · ****"
        );
    }

    #[test]
    fn email_preview_parts_normal() {
        let parts = sensitive_preview_parts("abcdef@gmail.com", "email");
        assert!(matches!(
            parts.first(),
            Some(SensitivePreviewPart::Masked(_))
        ));
        if let Some(SensitivePreviewPart::Masked(mv)) = parts.first() {
            assert_eq!(mv.prefix, "abc");
            assert_eq!(mv.suffix, "@gmail.com");
        }
    }

    #[test]
    fn email_short_local_part() {
        let parts = sensitive_preview_parts("ab@gmail.com", "email");
        if let Some(SensitivePreviewPart::Masked(mv)) = parts.first() {
            assert_eq!(mv.prefix, "a");
            assert_eq!(mv.suffix, "@gmail.com");
        }
    }

    #[test]
    fn phone_preview_parts_normal() {
        let parts = sensitive_preview_parts("13812345678", "phone");
        if let Some(SensitivePreviewPart::Masked(mv)) = parts.first() {
            assert_eq!(mv.prefix, "138");
            assert_eq!(mv.suffix, "5678");
        }
    }

    #[test]
    fn phone_short_value_only_mask() {
        let parts = sensitive_preview_parts("1234567", "phone");
        if let Some(SensitivePreviewPart::Masked(mv)) = parts.first() {
            assert_eq!(mv.prefix, "");
            assert_eq!(mv.suffix, "");
        }
    }

    // ── Determinism ─────────────────────────────────────────────────────

    #[test]
    fn same_input_same_result() {
        let text = "API_KEY=sk-abcdefghijklmnopqrstuvwxyz1234567890AB";
        let m1 = detect_secret(text);
        let m2 = detect_secret(text);
        // Compare debug representations since SecretMatch has no PartialEq
        assert_eq!(format!("{:?}", m1), format!("{:?}", m2));
    }

    #[test]
    fn empty_text_no_match() {
        assert!(detect_secret("").is_none());
        assert!(detect_secret("   ").is_none());
    }

    // ── File path rejection ────────────────────────────────────────────────

    #[test]
    fn reject_windows_file_path_as_secret() {
        // Drive letter + backslash path with file extension
        assert!(
            detect_secret("G:\\Develop\\github\\clippi\\RELEASES.md").is_none(),
            "Windows file path should not be detected as secret"
        );
        assert!(
            detect_secret("C:\\Users\\name\\Documents\\file.txt").is_none(),
            "Windows file path should not be detected as secret"
        );
        assert!(
            detect_secret("D:/Projects/code/src/main.rs").is_none(),
            "Windows forward-slash path should not be detected as secret"
        );
    }

    #[test]
    fn reject_unc_path_as_secret() {
        assert!(
            detect_secret("\\\\server\\share\\folder\\file.txt").is_none(),
            "UNC path should not be detected as secret"
        );
    }

    #[test]
    fn reject_unix_absolute_path_as_secret() {
        assert!(
            detect_secret("/usr/local/bin/tool.sh").is_none(),
            "Unix absolute path should not be detected as secret"
        );
        assert!(
            detect_secret("/home/user/projects/my-app/src/main.rs").is_none(),
            "Unix absolute path should not be detected as secret"
        );
    }

    #[test]
    fn looks_like_file_path_detects_windows_paths() {
        assert!(looks_like_file_path(
            "G:\\Develop\\github\\clippi\\RELEASES.md"
        ));
        assert!(looks_like_file_path("C:\\Windows\\System32\\cmd.exe"));
        assert!(looks_like_file_path("D:/data/images/photo.png"));
    }

    #[test]
    fn looks_like_file_path_detects_unc() {
        assert!(looks_like_file_path("\\\\server\\share\\folder"));
        assert!(looks_like_file_path("\\\\192.168.1.1\\data\\file.bin"));
    }

    #[test]
    fn looks_like_file_path_detects_unix_paths() {
        assert!(looks_like_file_path("/usr/bin/gcc"));
        assert!(looks_like_file_path("/home/user/.config/app/config.toml"));
    }

    #[test]
    fn looks_like_file_path_rejects_non_paths() {
        assert!(!looks_like_file_path("C0rrect-H0rse-Battery-Staple!"));
        assert!(!looks_like_file_path(
            "sk-proj-test-abcdefghijklmnopqrstuvwxyz1234567890AB"
        ));
        assert!(!looks_like_file_path(
            "ghp_abcdefghijklmnopqrstuvwxyz1234567890ABCD"
        ));
        // Forward slash but no file extension — may be ambiguous
        assert!(!looks_like_file_path("usr/local/bin"));
    }
}
